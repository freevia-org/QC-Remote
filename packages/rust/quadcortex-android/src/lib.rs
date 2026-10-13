mod preset_workflow;
// Closed native handles use `-3` for integer-returning JNI methods. Methods
// returning strings throw `IllegalStateException`; void lifecycle callbacks
// are idempotent no-ops, including destroy.
const CLOSED_HANDLE_SENTINEL: jint = -3;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jbyteArray, jint, jlong, jstring};
use jni::JNIEnv;
use quadcortex::commands::{self, OutboundMessage};
use quadcortex::device::backup::BackupAction;
use quadcortex::device::catalog::CatalogVerificationPolicy;
use quadcortex::device::correlation::{RequestDeadline, ResponseDecision, ResponseExpectation};
use quadcortex::device::delivery::{DeliveryStop, DeliveryTicket, ReportOutcome};
use quadcortex::device::owner::{
    DispatchContext, GatewayReadScope, NativeDispatchTicket, ObservationTicket, OperationTicket,
    SessionRuntime,
};
use quadcortex::device::read::GatewayReadRuntime;
use quadcortex::device::request::{
    assert_expected_parameter, assert_expected_preset, compose_global_tempo_settings,
    finalize_device_backup, gateway_correlated_readback_delay, gateway_read_followup_method,
    gateway_verification_policy, gateway_write_is_realtime, gateway_write_verification_policy,
    merge_expected_state, plan_gateway_read, plan_gateway_write_with_state, plan_preset_recall,
    GatewayReadPlan, GatewayResponseProjection, GatewayVerification, GatewayVerificationAction,
    GatewayVerificationRuntime, GatewayWritePreflightDecision, GatewayWriteVerificationPolicy,
    PlannedWrite,
};
use quadcortex::device::session::{DispatchOutcome, LifecycleTicket, SessionDriver, SessionEpoch};
use quadcortex::device::state_runtime::DeviceStateRuntime;
use quadcortex::device::transport::{ReportLayout, TransportRuntime};
use quadcortex::framing;
use quadcortex::state::parse_model_repo;
use quadcortex::sync::RecoverPoison;
use serde_json::Value;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

struct DecoderHandle {
    session: Mutex<SessionRuntime>,
}

struct SessionComponent<'a, T> {
    owner: MutexGuard<'a, SessionRuntime>,
    get: fn(&SessionRuntime) -> &T,
    get_mut: fn(&mut SessionRuntime) -> &mut T,
}
impl<T> Deref for SessionComponent<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        (self.get)(&self.owner)
    }
}
impl<T> DerefMut for SessionComponent<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        (self.get_mut)(&mut self.owner)
    }
}
impl DecoderHandle {
    fn component<T>(
        &self,
        get: fn(&SessionRuntime) -> &T,
        get_mut: fn(&mut SessionRuntime) -> &mut T,
    ) -> SessionComponent<'_, T> {
        SessionComponent {
            owner: self.session.lock_recover(),
            get,
            get_mut,
        }
    }
    fn lock_state(&self) -> SessionComponent<'_, DeviceStateRuntime> {
        self.component(|s| &s.state, |s| &mut s.state)
    }
    fn lock_transport(&self) -> SessionComponent<'_, TransportRuntime> {
        self.component(|s| &s.transport, |s| &mut s.transport)
    }
    fn lock_lifecycle(&self) -> SessionComponent<'_, Option<SessionDriver>> {
        self.component(|s| &s.lifecycle, |s| &mut s.lifecycle)
    }
}

fn handles() -> &'static Mutex<HashMap<jlong, Arc<DecoderHandle>>> {
    static HANDLES: OnceLock<Mutex<HashMap<jlong, Arc<DecoderHandle>>>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn handle(value: jlong) -> Result<Arc<DecoderHandle>, String> {
    if value <= 0 {
        return Err("native QC decoder handle is invalid".into());
    }
    handles()
        .lock_recover()
        .get(&value)
        .cloned()
        .ok_or_else(|| "native QC decoder handle is closed".to_string())
}

fn register_handle() -> jlong {
    static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);
    let decoder = Arc::new(DecoderHandle {
        session: Mutex::new(SessionRuntime::new(0)),
    });
    let value = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    if value <= 0 {
        return 0;
    }
    handles().lock_recover().insert(value, decoder);
    value
}

fn stamp_android_file_mutation_request_id(
    owner: &mut SessionRuntime,
    message: &mut OutboundMessage,
) -> Result<Option<u64>, String> {
    let Some(lifecycle) = owner.lifecycle.as_mut() else {
        if message.message_type == quadcortex::profile::MESSAGE_TYPE_FILE {
            return Err("no native QC lifecycle is active for a File command".into());
        }
        return Ok(None);
    };
    lifecycle
        .stamp_file_mutation_request_id(message)
        .map_err(|error| format!("could not stamp QC File mutation request id: {error}"))
}

fn unregister_handle(value: jlong) {
    handles().lock_recover().remove(&value);
}

/// Run a JNI export body so a Rust panic never unwinds into the JVM.
///
/// Unwinding across an `extern "system"` boundary is undefined behaviour (and
/// aborts the whole Android process on current toolchains), so every export
/// funnels its body through here. A contained panic comes back as `Err` with
/// the panic message; the export then reports it through the same channel it
/// already uses for ordinary failures.
fn catch_panic<R>(body: impl FnOnce() -> R) -> Result<R, String> {
    catch_unwind(AssertUnwindSafe(body)).map_err(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_string());
        format!("native QC engine panicked: {detail}")
    })
}

/// Report a contained panic as a Java `IllegalStateException` and return the
/// export's ordinary error value. An exception the body already raised before
/// panicking is left in place rather than replaced.
fn or_throw<R>(outcome: Result<R, String>, env: &mut JNIEnv, default: R) -> R {
    outcome.unwrap_or_else(|error| {
        if !env.exception_check().unwrap_or(false) {
            let _ = env.throw_new("java/lang/IllegalStateException", error);
        }
        default
    })
}

fn json_result(env: &mut JNIEnv, result: Result<String, String>) -> jstring {
    match result {
        Ok(value) => env
            .new_string(value)
            .map(|value| value.into_raw())
            .unwrap_or(ptr::null_mut()),
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalStateException", error);
            ptr::null_mut()
        }
    }
}

fn bytes_result(env: &mut JNIEnv, result: Result<Vec<u8>, String>) -> jbyteArray {
    match result {
        Ok(value) => env
            .byte_array_from_slice(&value)
            .map(|value| value.into_raw())
            .unwrap_or(ptr::null_mut()),
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalStateException", error);
            ptr::null_mut()
        }
    }
}

// Called while holding the lifecycle lock. No host callback interprets startup
// phases to decide readiness. Every synchronization transition is applied before
// releasing the lock, so an older Java response cannot restore stale readiness.
fn lifecycle_status(owner: &mut SessionRuntime, now_ms: u64) -> Result<String, String> {
    let runtime = owner
        .lifecycle
        .as_mut()
        .ok_or("no native QC lifecycle is active")?;
    if let Some(synchronized) = runtime.take_synchronization_change() {
        if runtime.failure().is_none() {
            owner
                .transport
                .synchronization_completed(now_ms, synchronized);
        }
    }
    serde_json::to_string(&serde_json::json!({
        "phase": runtime.phase().as_str(),
        "sessionValidated": runtime.session_validated(),
        "error": runtime.failure().map(|failure| format!("{failure:?}")),
    }))
    .map_err(|error| error.to_string())
}

fn observation_ticket(env: &mut JNIEnv, ticket: JString) -> Result<ObservationTicket, String> {
    serde_json::from_str(
        &env.get_string(&ticket)
            .map_err(|e| e.to_string())?
            .to_string_lossy(),
    )
    .map_err(|e| format!("invalid QC observation ticket: {e}"))
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeObservationCurrent(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    ticket: JString,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let ticket = observation_ticket(&mut env, ticket)?;
                let owner = native.session.lock_recover();
                Ok::<_, String>(owner.accepts_observation(&ticket))
            })();
            match result {
                Ok(current) => current as jint,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeMergeExpectedState(
    mut env: JNIEnv,
    _class: JClass,
    params_json: JString,
    expected_json: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let params: Value = serde_json::from_str(
                    &env.get_string(&params_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let expected: Value = serde_json::from_str(
                    &env.get_string(&expected_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                serde_json::to_string(&merge_expected_state(&params, &expected))
                    .map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

fn messages_json(messages: Vec<OutboundMessage>) -> Vec<Value> {
    messages
        .into_iter()
        .map(|message| {
            serde_json::json!({
                "messageType": message.message_type,
                "payload": message.payload,
            })
        })
        .collect()
}

fn bind_plan_envelope(envelope: String, context: &DispatchContext) -> Result<String, String> {
    let mut plan: Value = serde_json::from_str(&envelope).map_err(|error| error.to_string())?;
    plan["sessionFence"] = serde_json::to_value(context).map_err(|error| error.to_string())?;
    serde_json::to_string(&plan).map_err(|error| error.to_string())
}

fn gateway_write_envelope(
    method: &str,
    detail: &str,
    verification: &GatewayVerification,
    realtime: bool,
    policy: &GatewayWriteVerificationPolicy,
    write: PlannedWrite,
) -> Result<String, String> {
    let catalog_policy = (method == "device.deleteSetlist")
        .then(CatalogVerificationPolicy::persistent_file_mutation);
    let inter_message_interval_ms = write.inter_message_interval_ms();
    let (lane, controller, value, messages) = match write {
        PlannedWrite::HidCommand(command) => (
            "hid",
            0_u8,
            0_u8,
            vec![command.try_encode().map_err(|error| error.to_string())?],
        ),
        PlannedWrite::HidOperation(operation) => (
            "hid",
            0,
            0,
            operation.try_encode().map_err(|error| error.to_string())?,
        ),
        PlannedWrite::MidiControlChange { controller, value } => {
            ("midi", controller, value, Vec::new())
        }
    };
    serde_json::to_string(&serde_json::json!({
        "method": method,
        "detail": detail,
        "verification": verification,
        "observedStateKinds": verification.observed_state_kinds(),
        "lane": lane,
        "controller": controller,
        "value": value,
        "realtime": realtime,
        "interMessageIntervalMs": inter_message_interval_ms,
        "confirmationTimeoutMs": policy.timeout_ms,
        "preflightMethod": policy.preflight_method,
        "preflightObservationSource": policy.preflight_observation_source,
        "readbackMethod": policy.readback_method,
        "catalogPolicy": catalog_policy,
        "messages": messages_json(messages),
    }))
    .map_err(|error| error.to_string())
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeOperationObservationIsFresh(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
    observation: JString,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let operation: String = env
                    .get_string(&operation)
                    .map_err(|e| e.to_string())?
                    .into();
                let observation: String = env
                    .get_string(&observation)
                    .map_err(|e| e.to_string())?
                    .into();
                let operation: OperationTicket =
                    serde_json::from_str(&operation).map_err(|e| e.to_string())?;
                let observation: ObservationTicket =
                    serde_json::from_str(&observation).map_err(|e| e.to_string())?;
                let native = handle(value)?;
                let fresh = native
                    .session
                    .lock_recover()
                    .operation_observation_is_fresh(&operation, &observation);
                Ok::<bool, String>(fresh)
            })();
            match result {
                Ok(fresh) => {
                    if fresh {
                        1
                    } else {
                        0
                    }
                }
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayWriteEchoMatches(
    mut env: JNIEnv,
    _class: JClass,
    method: JString,
    params: JString,
    message_type: jint,
    payload: JByteArray,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let method: String = env.get_string(&method).map_err(|e| e.to_string())?.into();
                let params: String = env.get_string(&params).map_err(|e| e.to_string())?.into();
                let params: Value = serde_json::from_str(&params).map_err(|e| e.to_string())?;
                let message_type = u16::try_from(message_type).map_err(|e| e.to_string())?;
                let payload = env
                    .convert_byte_array(&payload)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(quadcortex::device::request::gateway_write_echo_matches(
                    &method,
                    &params,
                    message_type,
                    &payload,
                ) as jint)
            })();
            match result {
                Ok(matched) => matched,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalArgumentException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayReadbackRetryDelay(
    mut env: JNIEnv,
    _class: JClass,
    method: JString,
    attempt: jint,
) -> jlong {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let method = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let attempt = usize::try_from(attempt)
                    .map_err(|_| "gateway readback attempt must be non-negative".to_string())?;
                Ok::<_, String>(
                    gateway_correlated_readback_delay(&method, attempt)
                        .map(|delay| delay as jlong)
                        .unwrap_or(-1),
                )
            })();
            match result {
                Ok(delay) => delay,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalArgumentException", error);
                    -1
                }
            }
        }),
        &mut env,
        -1,
    )
}

fn gateway_read_envelope(method: &str, plan: GatewayReadPlan) -> Result<String, String> {
    let messages = plan
        .operation
        .try_encode()
        .map_err(|error| error.to_string())?;
    serde_json::to_string(&serde_json::json!({
        "responseType": plan.response_type,
        "timeoutMs": plan.timeout_ms,
        "projection": plan.projection,
        "followupMethod": gateway_read_followup_method(method),
        "messages": messages_json(messages),
    }))
    .map_err(|error| error.to_string())
}

fn unsigned(args: &Value, name: &str) -> Result<u32, String> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("{name} must be a non-negative integer"))
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeEncodeCommand(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    command: JString,
    args_json: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let command = env
                    .get_string(&command)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let args_json = env
                    .get_string(&args_json)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let args: Value =
                    serde_json::from_str(&args_json).map_err(|error| error.to_string())?;
                let messages = match command.as_str() {
                    "screenSwipe" => commands::screen_drag(
                        unsigned(&args, "x")? as f32,
                        unsigned(&args, "y")? as f32,
                        unsigned(&args, "toX")? as f32,
                        unsigned(&args, "toY")? as f32,
                    )
                    .to_vec(),
                    "keepalive" => vec![commands::keepalive()],
                    "backup" => vec![commands::create_local_backup()],
                    "disconnect" => {
                        let request_id = native
                            .lock_lifecycle()
                            .as_mut()
                            .ok_or_else(|| "no native QC lifecycle is active".to_string())?
                            .reserve_request_id();
                        commands::disconnect_initialization_exact(request_id)
                    }
                    other => return Err(format!("unknown native QC command: {other}")),
                };
                serde_json::to_string(&messages_json(messages)).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

/// Gateway methods the shared planner knows but the Android host withholds.
///
/// `device.mouseFrame` is withheld because a raw `press` frame leaves a pointer
/// down on the unit until a `release` frame lifts it, and the Windows broker
/// tracks that (refusing other gestures meanwhile; see
/// `quadcortex::device::touch_guard`; the host never releases a held pointer by
/// itself). Android has no such tracking and it cannot be exercised offline here,
/// so it refuses the raw frame rather than risk a stranded pointer.
///
/// `device.dragScreen` used to be withheld too, because it was a long paced
/// stream of touch frames that Android's frame writer could not clean up after.
/// It is now the same single `DRAG` frame as `device.swipeScreen` (MOVE had no
/// observed effect on CorOS 4.1.0, 2026-10-06), which begins no held touch and
/// has nothing to release, so Android runs it like a swipe.
///
/// A hold (`device.longTapScreen`, a touch-down, a wait and a touch-up) is run
/// on Android by `QcUsbPlugin.writeHoldMessages`, which sends one release of its
/// own if the touch-up cannot be written after the touch-down was, on the same
/// USB connection only. The Python direct transport does the same in
/// `remote_control.hold_screen`.
const ANDROID_WITHHELD_METHODS: [&str; 1] = ["device.mouseFrame"];

fn refuse_withheld_on_android(method: &str) -> Result<(), String> {
    if ANDROID_WITHHELD_METHODS.contains(&method) {
        Err(format!(
            "{method} is not supported by the Android host yet: it cannot release a touch it \
             has started (a raw press that nobody releases). Use the Windows host."
        ))
    } else {
        Ok(())
    }
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePlanGatewayWrite(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    method: JString,
    args_json: JString,
    started_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let method = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let args_json = env
                    .get_string(&args_json)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                refuse_withheld_on_android(&method)?;
                let args: Value =
                    serde_json::from_str(&args_json).map_err(|error| error.to_string())?;
                let state = native.lock_state();
                if method == "device.setParameter" {
                    let row = unsigned(&args, "row")?;
                    let column = unsigned(&args, "column")?;
                    let parameter_index = unsigned(&args, "parameterIndex")?;
                    let actual = state.block_details(row, column).and_then(|details| {
                        details
                            .parameters
                            .into_iter()
                            .find(|parameter| parameter.index == parameter_index)
                            .and_then(|parameter| parameter.normalized_value)
                    });
                    assert_expected_parameter(actual, &args)?;
                }
                let snapshot = state.snapshot();
                let (detail, verification, write, no_op) = match method.as_str() {
                    "device.recallPreset" | "device.navigateBank" | "device.reloadPreset" => {
                        let plan = plan_preset_recall(&method, &args, Some(snapshot))?;
                        (
                            plan.detail.clone(),
                            plan.verification(),
                            PlannedWrite::HidCommand(plan.command),
                            false,
                        )
                    }
                    _ => {
                        let plan = plan_gateway_write_with_state(&method, &args, &state)?;
                        {
                            let no_op = plan.is_noop();
                            (plan.detail, plan.verification, plan.write, no_op)
                        }
                    }
                };
                let context = state.owner.context_for_gateway_write(
                    &method,
                    &args,
                    &write,
                    started_ms.max(0) as u64,
                )?;
                let envelope = gateway_write_envelope(
                    &method,
                    &detail,
                    &verification,
                    gateway_write_is_realtime(&method),
                    &gateway_write_verification_policy(&method),
                    write,
                )?;
                let mut envelope: Value =
                    serde_json::from_str(&envelope).map_err(|error| error.to_string())?;
                envelope["noOp"] = Value::Bool(no_op);
                bind_plan_envelope(envelope.to_string(), &context)
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBeginGatewayReadScope(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    method: JString,
    args_json: JString,
    context_json: JString,
    started_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let method: String = env.get_string(&method).map_err(|e| e.to_string())?.into();
                let args: Value = serde_json::from_str(
                    &env.get_string(&args_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let plan = plan_gateway_read(&method, &args, 0)?;
                let context: String = env
                    .get_string(&context_json)
                    .map_err(|e| e.to_string())?
                    .into();
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let scope = if context.is_empty() {
                    owner.begin_gateway_read_scope(started_ms.max(0) as u64, plan.timeout_ms)?
                } else {
                    let context: DispatchContext =
                        serde_json::from_str(&context).map_err(|e| e.to_string())?;
                    owner.begin_gateway_read_scope_for(
                        &context,
                        started_ms.max(0) as u64,
                        plan.timeout_ms,
                    )?
                };
                serde_json::to_string(&scope).map_err(|e| e.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBeginGatewayReadbackScope(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    context_json: JString,
    started_ms: jlong,
    timeout_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let context: DispatchContext = serde_json::from_str(
                    &env.get_string(&context_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let scope = owner.begin_gateway_read_scope_for(
                    &context,
                    started_ms.max(0) as u64,
                    timeout_ms.max(0) as u64,
                )?;
                serde_json::to_string(&scope).map_err(|e| e.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayReadScopeRemaining(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    scope_json: JString,
    now_ms: jlong,
) -> jlong {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let scope: GatewayReadScope = serde_json::from_str(
                    &env.get_string(&scope_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let now = (now_ms.max(0) as u64).saturating_add(
                    call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                );
                owner.gateway_read_scope_remaining(&scope, now)
            })();
            match result {
                Ok(remaining) => remaining.min(i64::MAX as u64) as jlong,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePlanGatewayRead(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    method: JString,
    args_json: JString,
    scope_json: JString,
    started_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let method: String = env.get_string(&method).map_err(|e| e.to_string())?.into();
                let args: Value = serde_json::from_str(
                    &env.get_string(&args_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let scope_json: String = env
                    .get_string(&scope_json)
                    .map_err(|e| e.to_string())?
                    .into();
                let preview = plan_gateway_read(&method, &args, 0)?;
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                let started_ms = started_ms.max(0) as u64;
                let now = || {
                    started_ms.saturating_add(
                        call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    )
                };
                let scope: GatewayReadScope = if scope_json.is_empty() {
                    owner.begin_gateway_read_scope(started_ms, preview.timeout_ms)?
                } else {
                    serde_json::from_str(&scope_json).map_err(|e| e.to_string())?
                };
                owner.gateway_read_scope_remaining(&scope, now())?;
                let runtime = owner
                    .lifecycle
                    .as_mut()
                    .filter(|runtime| runtime.is_connected())
                    .ok_or("no connected QC session for read planning")?;
                // Allocate correlation only after checking the logical read's original session.
                let request_id = runtime.reserve_request_id();
                let epoch = runtime.epoch();
                let mut read_plan = plan_gateway_read(&method, &args, request_id)?;
                read_plan.timeout_ms = read_plan.timeout_ms.min(scope.remaining_ms(started_ms));
                let mut plan: Value =
                    serde_json::from_str(&gateway_read_envelope(&method, read_plan)?)
                        .map_err(|e| e.to_string())?;
                plan["sessionEpoch"] = serde_json::to_value(epoch).map_err(|e| e.to_string())?;
                owner.gateway_read_scope_remaining(&scope, now())?;
                bind_plan_envelope(
                    serde_json::to_string(&plan).map_err(|e| e.to_string())?,
                    scope.context(),
                )
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeReadEpochCurrent(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    epoch_json: JString,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let epoch: SessionEpoch = serde_json::from_str(
                    &env.get_string(&epoch_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let lifecycle = native.lock_lifecycle();
                Ok::<_, String>(
                    lifecycle
                        .as_ref()
                        .is_some_and(|runtime| runtime.matches_epoch(&epoch)),
                )
            })();
            match result {
                Ok(current) => current as jint,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeObserveGatewayRead(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    ticket: JString,
    context: JString,
    projection_json: JString,
    payload: JByteArray,
    expected_type: jint,
    actual_type: jint,
    started_at_ms: jlong,
    timeout_ms: jlong,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let native = handle(value)?;
                let ticket = observation_ticket(&mut env, ticket)?;
                let context: DispatchContext = serde_json::from_str(
                    &env.get_string(&context)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let projection_json = env
                    .get_string(&projection_json)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let projection: GatewayResponseProjection =
                    serde_json::from_str(&projection_json).map_err(|error| error.to_string())?;
                let payload = env
                    .convert_byte_array(payload)
                    .map_err(|error| error.to_string())?;
                let expected_type = u16::try_from(expected_type)
                    .map_err(|_| "invalid expected QC message type".to_string())?;
                let actual_type = u16::try_from(actual_type)
                    .map_err(|_| "invalid actual QC message type".to_string())?;
                let runtime = GatewayReadRuntime::new(
                    expected_type,
                    projection,
                    started_at_ms.max(0) as u64,
                    timeout_ms.max(0) as u64,
                );
                if !runtime.accepts_ingress(ticket.ingress(), now_ms.max(0) as u64) {
                    return Ok("{\"status\":\"unrelated\"}".into());
                }
                let value = runtime.observe_with_clock(actual_type, &payload, || {
                    (now_ms.max(0) as u64).saturating_add(
                        call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    )
                });
                let owner = native.session.lock_recover();
                if !owner.accepts_observation_for(context.session_fence(), &ticket) {
                    return Ok("{\"status\":\"unrelated\"}".into());
                }
                // The unit states its CorOS version in its update request, not in
                // the reply a read decodes; the session holds what it stated.
                let value = runtime.with_session_facts(value, owner.observed_cor_os_version());
                let finished_ms = (now_ms.max(0) as u64).saturating_add(
                    call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                );
                if runtime.remaining_ms(finished_ms) == 0 {
                    return Ok("{\"status\":\"expired\"}".into());
                }
                serde_json::to_string(&value).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayResponseDecision(
    mut env: JNIEnv,
    _class: JClass,
    projection_json: JString,
    expected_type: jint,
    actual_type: jint,
    payload: JByteArray,
    started_at_ms: jlong,
    timeout_ms: jlong,
    now_ms: jlong,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let projection_json = env
                    .get_string(&projection_json)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let projection: GatewayResponseProjection =
                    serde_json::from_str(&projection_json).map_err(|error| error.to_string())?;
                let expected_type = u16::try_from(expected_type)
                    .map_err(|_| "invalid expected QC message type".to_string())?;
                let actual_type = u16::try_from(actual_type)
                    .map_err(|_| "invalid actual QC message type".to_string())?;
                let payload = env
                    .convert_byte_array(payload)
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(
                    ResponseExpectation::new(
                        expected_type,
                        projection.expected_request_id(),
                        started_at_ms.max(0) as u64,
                        timeout_ms.max(0) as u64,
                    )
                    .observe(actual_type, &payload, now_ms.max(0) as u64),
                )
            })();
            match result {
                Ok(ResponseDecision::Matched) => 1,
                Ok(ResponseDecision::Unrelated) => 0,
                Ok(ResponseDecision::Expired) => 2,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalArgumentException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeRequestExpired(
    _env: JNIEnv,
    _class: JClass,
    started_at_ms: jlong,
    timeout_ms: jlong,
    now_ms: jlong,
) -> jint {
    catch_panic(|| {
        RequestDeadline::new(started_at_ms.max(0) as u64, timeout_ms.max(0) as u64)
            .expired(now_ms.max(0) as u64) as jint
    })
    .unwrap_or(0)
}

/// 0 = pending, 1 = verified, 2 = timed out. Freshness, matching, and timeout
/// are evaluated by the same runtime used by the Windows broker.
#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayVerificationStarted(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    verification_json: JString,
    context_json: JString,
    policy_method: JString,
    after_sequence: jlong,
    started_at_ms: jlong,
    timeout_ms: jlong,
) -> jlong {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let verification: GatewayVerification = serde_json::from_str(
                    &env.get_string(&verification_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let method = env
                    .get_string(&policy_method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let policy = if method.is_empty() {
                    gateway_verification_policy(timeout_ms.max(0) as u64)
                } else {
                    gateway_write_verification_policy(&method)
                };
                let context: DispatchContext = serde_json::from_str(
                    &env.get_string(&context_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let verification_id = native.session.lock_recover().begin_gateway_verification(
                    &context,
                    GatewayVerificationRuntime::new(
                        verification,
                        after_sequence.max(0) as u128,
                        started_at_ms.max(0) as u64,
                        &policy,
                    ),
                )?;
                Ok::<_, String>(verification_id as jlong)
            })();
            match result {
                Ok(verification_id) => verification_id,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayVerificationAdvance(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    verification_id: jlong,
    observation_sequence: jlong,
    now_ms: jlong,
    method: JString,
    verification_json: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let native = handle(value)?;
                let verification_id = u64::try_from(verification_id)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| "native gateway verification id is invalid".to_string())?;
                let method = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let verification_json = env
                    .get_string(&verification_json)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let mut owner = native.session.lock_recover();
                let now_ms = (now_ms.max(0) as u64).saturating_add(
                    call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                );
                advance_gateway_verification_json(
                    &mut owner,
                    verification_id,
                    observation_sequence.max(0) as u128,
                    now_ms,
                    &method,
                    &verification_json,
                )
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

/// Owner advancement and wire serialization shared by the JNI export and unit tests.
/// Caller retains the native owner lock; JNI clock compensation and exceptions stay outside.
fn advance_gateway_verification_json(
    owner: &mut SessionRuntime,
    verification_id: u64,
    observation_sequence: u128,
    now_ms: u64,
    method: &str,
    verification_json: &str,
) -> Result<String, String> {
    let (action, _context, _remaining_ms) =
        owner.advance_gateway_verification(verification_id, observation_sequence, now_ms)?;
    let value = match action {
        GatewayVerificationAction::Wait { delay_ms } => {
            serde_json::json!({"kind": "wait", "delayMs": delay_ms})
        }
        GatewayVerificationAction::Verified => {
            verified_action(method, verification_json, owner.state.snapshot())
        }
        GatewayVerificationAction::TimedOut => serde_json::json!({"kind": "timedOut"}),
    };
    serde_json::to_string(&value).map_err(|error| error.to_string())
}

/// The `verified` action, carrying the shared `pluginBlockReadback` for a verified
/// `device.addBlock` of a plugin model. `snapshot` is the owner state the verification just
/// matched, read under the same lock, so it is the authoritative readback and nothing is
/// re-read. An unreadable verification predicate only omits the readback.
fn verified_action(
    method: &str,
    verification_json: &str,
    snapshot: &quadcortex::device::GatewaySnapshot,
) -> serde_json::Value {
    // A recall, bank navigation or reload only observes the address in a position update
    // (no request id, so not causal proof), and the preset body cannot be bound to it. The
    // shared outcome under `outcome` replaces the
    // verified fields QcUsbPlugin would otherwise report, as on the Windows broker.
    if matches!(
        method,
        "device.recallPreset" | "device.navigateBank" | "device.reloadPreset"
    ) {
        if let Some(outcome) = serde_json::from_str::<GatewayVerification>(verification_json)
            .ok()
            .and_then(|verification| {
                quadcortex::device::request::preset_address_observed_result(&verification, snapshot)
            })
        {
            return serde_json::json!({"kind": "verified", "outcome": outcome});
        }
    }
    let readback = serde_json::from_str::<GatewayVerification>(verification_json)
        .ok()
        .and_then(|verification| {
            quadcortex::device::request::verified_plugin_block_readback(
                method,
                &verification,
                snapshot,
            )
        })
        .and_then(|readback| serde_json::to_value(readback).ok());
    match readback {
        Some(readback) => serde_json::json!({"kind": "verified", "pluginBlockReadback": readback}),
        None => serde_json::json!({"kind": "verified"}),
    }
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayVerificationCancelled(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    verification_id: jlong,
) {
    catch_panic(|| {
        let Ok(native) = handle(value) else {
            return;
        };
        native
            .session
            .lock_recover()
            .cancel_gateway_verification(verification_id.max(0) as u64);
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeGatewayReadbackMatches(
    mut env: JNIEnv,
    _class: JClass,
    method: JString,
    params_json: JString,
    response_json: JString,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let method = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let params: Value = serde_json::from_str(
                    &env.get_string(&params_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let response: Value = serde_json::from_str(
                    &env.get_string(&response_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                Ok::<_, String>(quadcortex::device::request::gateway_write_readback_matches(
                    &method, &params, &response,
                ))
            })();
            match result {
                Ok(true) => 1,
                Ok(false) => 0,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalArgumentException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeComposeGlobalTempoSettings(
    mut env: JNIEnv,
    _class: JClass,
    global_json: JString,
    preset_json: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let global: Value = serde_json::from_str(
                    &env.get_string(&global_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let preset: Value = serde_json::from_str(
                    &env.get_string(&preset_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                serde_json::to_string(&compose_global_tempo_settings(&global, &preset)?)
                    .map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBindGatewayWritePreflight(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    context_json: JString,
    method: JString,
    params_json: JString,
    response_json: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let context: DispatchContext = serde_json::from_str(
                    &env.get_string(&context_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let method = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let params: Value = serde_json::from_str(
                    &env.get_string(&params_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let response: Value = serde_json::from_str(
                    &env.get_string(&response_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let (context, decision) = owner.bind_write_preflight(
                    &context,
                    &method,
                    &params,
                    &response,
                    (now_ms.max(0) as u64).saturating_add(
                        call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    ),
                )?;
                let decision = match decision {
                    GatewayWritePreflightDecision::Proceed => 1,
                    GatewayWritePreflightDecision::AlreadyMatched => 2,
                };
                Ok(serde_json::json!({"context": context, "decision": decision}).to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePlanOperation(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    fence: JString,
    now_ms: jlong,
    timeout_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let text: String = env
                    .get_string(&fence)
                    .map_err(|error| error.to_string())?
                    .into();
                let fence: Option<DispatchContext> = if text.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&text).map_err(|error| error.to_string())?)
                };
                let mut owner = native.session.lock_recover();
                let ticket = match fence {
                    Some(fence) => owner.plan_operation_in_context(
                        &fence,
                        now_ms.max(0) as u64,
                        timeout_ms.max(0) as u64,
                    )?,
                    None => owner.plan_operation(now_ms.max(0) as u64, timeout_ms.max(0) as u64)?,
                };
                serde_json::to_string(&ticket).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeOperationRemaining(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation_json: JString,
    now_ms: jlong,
) -> jlong {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let operation: OperationTicket = serde_json::from_str(
                    &env.get_string(&operation_json)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let now = (now_ms.max(0) as u64).saturating_add(
                    call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                );
                owner.validate_operation(&operation, now)?;
                Ok::<_, String>(operation.remaining_ms(now))
            })();
            match result {
                Ok(remaining) => remaining.min(i64::MAX as u64) as jlong,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePrepareDispatch(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let text: String = env
                    .get_string(&operation)
                    .map_err(|error| error.to_string())?
                    .into();
                let operation: Option<OperationTicket> = if text.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&text).map_err(|error| error.to_string())?)
                };
                let ticket = native.session.lock_recover().prepare_dispatch(
                    operation.as_ref(),
                    None,
                    now_ms.max(0) as u64,
                )?;
                serde_json::to_string(&ticket).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePrepareDelivery(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    dispatch: JString,
    message_type: jint,
    report_count: jint,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let native = handle(value)?;
                let text: String = env
                    .get_string(&dispatch)
                    .map_err(|error| error.to_string())?
                    .into();
                let dispatch: NativeDispatchTicket =
                    serde_json::from_str(&text).map_err(|error| error.to_string())?;
                let mut owner = native.session.lock_recover();
                let ticket = owner.prepare_delivery(
                    &dispatch,
                    u16::try_from(message_type).map_err(|_| "invalid delivery message type")?,
                    usize::try_from(report_count).map_err(|_| "invalid delivery report count")?,
                    (now_ms.max(0) as u64).saturating_add(
                        call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    ),
                )?;
                serde_json::to_string(&ticket).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

fn delivery_event(
    owner: &mut SessionRuntime,
    ticket: &DeliveryTicket,
    event: jint,
    index: usize,
    now_ms: u64,
) -> Result<(), String> {
    match event {
        0 => owner.delivery_admitted(ticket),
        1 => owner.begin_native_report(ticket, index, now_ms),
        2 => owner.finish_native_report(ticket, index, ReportOutcome::Delivered, now_ms),
        3 => owner.finish_native_report(ticket, index, ReportOutcome::ExpectedStatusStall, now_ms),
        4 => owner.finish_native_report(ticket, index, ReportOutcome::Failed, now_ms),
        5 => owner.stop_delivery(ticket, DeliveryStop::Stopped, now_ms),
        6 => owner.require_delivery_complete(ticket).map(|_| ()),
        _ => Err("invalid native delivery event".into()),
    }
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeDeliveryEvent(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    ticket: JString,
    event: jint,
    index: jint,
    now_ms: jlong,
) {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let native = handle(value)?;
                let text: String = env
                    .get_string(&ticket)
                    .map_err(|error| error.to_string())?
                    .into();
                let ticket: DeliveryTicket =
                    serde_json::from_str(&text).map_err(|error| error.to_string())?;
                let mut owner = native.session.lock_recover();
                delivery_event(
                    &mut owner,
                    &ticket,
                    event,
                    usize::try_from(index).map_err(|_| "invalid delivery report index")?,
                    (now_ms.max(0) as u64).saturating_add(
                        call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    ),
                )
            })();
            if let Err(error) = result {
                let _ = env.throw_new("java/lang/IllegalStateException", error);
            }
        }),
        &mut env,
        (),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeEncodeReports(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    message_type: jint,
    payload: JByteArray,
    include_report_id: jint,
    dispatch_ticket: JString,
    now_ms: jlong,
) -> jbyteArray {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let message_type = u16::try_from(message_type)
                    .map_err(|_| "invalid QC message type".to_string())?;
                let payload = env
                    .convert_byte_array(payload)
                    .map_err(|error| error.to_string())?;
                let native = handle(value)?;
                let ticket_text: String = env
                    .get_string(&dispatch_ticket)
                    .map_err(|error| error.to_string())?
                    .into();
                let dispatch_ticket: Option<NativeDispatchTicket> = if ticket_text.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&ticket_text).map_err(|error| error.to_string())?)
                };
                let mut message = OutboundMessage {
                    message_type,
                    payload,
                };
                let mut owner = native.session.lock_recover();
                if let Some(ticket) = &dispatch_ticket {
                    owner.validate_dispatch(ticket, false, now_ms.max(0) as u64)?;
                }
                stamp_android_file_mutation_request_id(&mut owner, &mut message)?;
                let layout = if include_report_id != 0 {
                    ReportLayout::ReportIdPrefixed
                } else {
                    ReportLayout::BodyOnly
                };
                let encoded = owner
                    .transport
                    .encode_reports_with_session(&message, layout)
                    .map_err(|error| format!("could not encode QC frame: {error}"))?
                    .into_iter()
                    .flatten()
                    .collect();
                Ok(encoded)
            })();
            bytes_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeCaptureReportEpoch(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    connection: jlong,
) -> jlong {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                owner.capture_report_epoch(connection.max(0) as u64)
            })();
            match result {
                Ok(epoch) => epoch as jlong,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

// Called under the same owner lock as report assembly. The command thread
// cannot admit an operation between a completed frame and its state reduction.
fn complete_native_frame(
    owner: &mut SessionRuntime,
    frame: quadcortex::device::transport::InboundFrame,
    ticket: ObservationTicket,
    now_ms: u64,
) -> Result<Vec<u8>, String> {
    let observation = if frame.message_type == quadcortex::profile::MESSAGE_TYPE_MODEL_REPO {
        // Metadata parsing and its lifecycle validation stay on the metadata lane.
        None
    } else {
        owner.observe_lifecycle(&ticket, frame.message_type, &frame.payload, now_ms);
        owner.observe_state(&ticket, frame.message_type, &frame.payload)?
    };
    let lifecycle: Value = serde_json::from_str(&lifecycle_status(owner, now_ms)?)
        .map_err(|error| error.to_string())?;
    let header = serde_json::to_vec(&serde_json::json!({
        "observation": ticket,
        "states": observation.as_ref().map(|value| value.states.as_slice()).unwrap_or(&[]),
        "tempoClock": observation.and_then(|value| value.tempo_clock),
        "lifecycle": lifecycle,
    }))
    .map_err(|error| error.to_string())?;
    let header_length =
        u32::try_from(header.len()).map_err(|_| "QC decoded header is too large")?;
    let mut result = Vec::with_capacity(frame.payload.len() + header.len() + 6);
    result.extend_from_slice(&frame.message_type.to_le_bytes());
    result.extend_from_slice(&header_length.to_le_bytes());
    result.extend_from_slice(&header);
    result.extend_from_slice(&frame.payload);
    Ok(result)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePushReport(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    connection: jlong,
    report_epoch: jlong,
    report: JByteArray,
    received_at_ms: jlong,
) -> jbyteArray {
    or_throw(
        catch_panic(|| {
            // A malformed or truncated native USB read must never unwind across JNI,
            // which would abort the entire Android process. Convert any unexpected
            // decoder panic into the same Java-side error path as a frame error.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let report = env
                    .convert_byte_array(report)
                    .map_err(|error| error.to_string())?;
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                let connection =
                    u64::try_from(connection).map_err(|_| "invalid QC reader identity")?;
                let Some((frame, observation)) = owner.push_native_report(
                    connection,
                    report_epoch.max(0) as u64,
                    &report,
                    received_at_ms.max(0) as u64,
                )?
                else {
                    return Ok(Vec::new());
                };
                complete_native_frame(&mut owner, frame, observation, received_at_ms.max(0) as u64)
            }))
            .unwrap_or_else(|_| Err("native QC report decoder panicked".to_string()));
            bytes_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeReportSize(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    catch_panic(|| framing::REPORT_SIZE as jint).unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeOutboundReportId(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    catch_panic(|| framing::OUT_REPORT_ID as jint).unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeInboundReportId(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    catch_panic(|| framing::IN_REPORT_ID as jint).unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeCreate(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    catch_panic(register_handle).unwrap_or(0)
}

fn backup_operation(env: &mut JNIEnv, operation: &JString) -> Result<OperationTicket, String> {
    serde_json::from_str(
        &env.get_string(operation)
            .map_err(|error| error.to_string())?
            .to_string_lossy(),
    )
    .map_err(|error| error.to_string())
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeConsumeBackupChunk(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
    observation: JString,
    payload: JByteArray,
    name: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let operation = backup_operation(&mut env, &operation)?;
                let observation: ObservationTicket = serde_json::from_str(
                    &env.get_string(&observation)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let bytes = env
                    .convert_byte_array(payload)
                    .map_err(|error| error.to_string())?;
                let name = env
                    .get_string(&name)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let native = handle(value)?;
                let progress = {
                    let mut owner = native.session.lock_recover();
                    let now = (now_ms.max(0) as u64).saturating_add(
                        call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    );
                    owner.consume_backup(&operation, &observation, &bytes, now)?
                };
                let Some(progress) = progress else {
                    return Ok(serde_json::json!({"complete": false, "ignored": true}).to_string());
                };
                let backup = progress
                    .document
                    .map(|raw| finalize_device_backup(&raw, &name))
                    .transpose()?;
                Ok(serde_json::json!({
                    "complete": backup.is_some(), "backup": backup,
                    "started": progress.started, "chunks": progress.chunks,
                    "ignoredPrefixChunks": progress.ignored_prefix_chunks,
                    "ignoredPrefixTerminators": progress.ignored_prefix_terminators,
                })
                .to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBackupStarted(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let mut operation = backup_operation(&mut env, &operation)?;
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                let now = (now_ms.max(0) as u64).saturating_add(
                    call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                );
                owner.begin_backup(&mut operation, now)?;
                serde_json::to_string(&operation).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBackupAdvance(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let call_started = std::time::Instant::now();
            let result = (|| {
                let operation = backup_operation(&mut env, &operation)?;
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                let now = (now_ms.max(0) as u64).saturating_add(
                    call_started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                );
                let (action, started) = owner.advance_backup(&operation, now)?;
                let (kind, error) = match action {
                    BackupAction::Wait => ("wait", None),
                    BackupAction::Keepalive => ("keepalive", None),
                    BackupAction::Failed(error) => ("failed", Some(error)),
                };
                Ok(
                    serde_json::json!({"action": kind, "error": error, "started": started})
                        .to_string(),
                )
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeClaimBackupRecovery(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
) {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let operation = backup_operation(&mut env, &operation)?;
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                owner.claim_backup_recovery(&operation)
            })();
            if let Err(error) = result {
                let _ = env.throw_new("java/lang/IllegalStateException", error);
            }
        }),
        &mut env,
        (),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBackupCancelled(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    operation: JString,
) {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let operation = backup_operation(&mut env, &operation)?;
                let native = handle(value)?;
                native.session.lock_recover().cancel_backup(&operation);
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                let _ = env.throw_new("java/lang/IllegalStateException", error);
            }
        }),
        &mut env,
        (),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeHandshakeAttempt(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
    session_id: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let session_id = env
                    .get_string(&session_id)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                if session_id.is_empty() {
                    return Err("startup session id must not be empty".to_string());
                }
                let now_ms = now_ms.max(0) as u64;
                let mut owner = native.session.lock_recover();
                if owner.transport.handshake_timed_out(now_ms) {
                    return serde_json::to_string(&serde_json::json!({ "kind": 2 }))
                        .map_err(|error| error.to_string());
                }
                let Some(write) = owner.next_handshake_write(now_ms, session_id, None) else {
                    return serde_json::to_string(&serde_json::json!({ "kind": 0 }))
                        .map_err(|error| error.to_string());
                };
                let request_id = write.request_id();
                serde_json::to_string(&serde_json::json!({
                    "kind": 1,
                    "attempt": write.attempt,
                    "requestId": request_id,
                    "includeReportId": write.layout == ReportLayout::ReportIdPrefixed,
                }))
                .map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeLifecyclePoll(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let mut lifecycle = native.lock_lifecycle();
                let Some(runtime) = lifecycle.as_mut() else {
                    return Ok("{\"phase\":\"inactive\",\"error\":null}".to_string());
                };
                runtime.tick(now_ms.max(0) as u64);
                lifecycle_status(&mut lifecycle.owner, now_ms.max(0) as u64)
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeLifecycleNextBatch(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
    unix_time_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let mut lifecycle = native.lock_lifecycle();
                let Some(runtime) = lifecycle.as_mut() else {
                    return Ok("null".to_string());
                };
                let batch = runtime.next_batch(now_ms.max(0) as u64, unix_time_ms.max(0) as u64);
                lifecycle_status(&mut lifecycle.owner, now_ms.max(0) as u64)?;
                match batch {
            None => Ok("null".to_string()),
            Some(batch) => serde_json::to_string(&serde_json::json!({
                "ticket": batch.ticket, "messages": messages_json(batch.messages),
                "dispatch": lifecycle.owner.prepare_dispatch(None, None, now_ms.max(0) as u64)?,
            }))
            .map_err(|error| error.to_string()),
        }
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeLifecycleBatchCurrent(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    ticket: JString,
) -> jint {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let ticket: LifecycleTicket = serde_json::from_str(
                    &env.get_string(&ticket)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let lifecycle = native.lock_lifecycle();
                Ok::<_, String>(
                    lifecycle
                        .as_ref()
                        .is_some_and(|runtime| runtime.is_current(&ticket)),
                )
            })();
            match result {
                Ok(current) => current as jint,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeLifecycleDispatched(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    ticket: JString,
    accepted: jint,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let ticket: LifecycleTicket = serde_json::from_str(
                    &env.get_string(&ticket)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|e| e.to_string())?;
                let mut lifecycle = native.lock_lifecycle();
                let Some(runtime) = lifecycle.as_mut() else {
                    // A terminal native write may already have closed the transport.
                    return Ok("{\"phase\":\"inactive\",\"error\":null}".to_string());
                };
                runtime.dispatched(
                    &ticket,
                    if accepted != 0 {
                        DispatchOutcome::Accepted
                    } else {
                        DispatchOutcome::Failed
                    },
                );
                lifecycle_status(&mut lifecycle.owner, now_ms.max(0) as u64)
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeStartupConnected(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jint {
    catch_panic(|| {
        handle(value)
            .ok()
            .and_then(|native| {
                native
                    .lock_lifecycle()
                    .as_ref()
                    .map(SessionDriver::is_connected)
            })
            .unwrap_or(false) as jint
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeStartupActive(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jint {
    catch_panic(|| {
        handle(value)
            .ok()
            .and_then(|native| {
                native
                    .lock_lifecycle()
                    .as_ref()
                    .map(SessionDriver::is_active)
            })
            .unwrap_or(false) as jint
    })
    .unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeCatalogContext(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    context_json: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let text: String = env
                    .get_string(&context_json)
                    .map_err(|error| error.to_string())?
                    .into();
                let owner = native.session.lock_recover();
                let context = if text.is_empty() {
                    owner.session_context()?
                } else {
                    serde_json::from_str::<DispatchContext>(&text)
                        .map_err(|error| error.to_string())?
                };
                if !owner.accepts_fence(context.session_fence()) {
                    return Err("QC catalog session changed".into());
                }
                serde_json::to_string(&context).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

fn with_transport(value: jlong, action: impl FnOnce(&mut TransportRuntime) -> jint) -> jint {
    let Ok(handle) = handle(value) else {
        return CLOSED_HANDLE_SENTINEL;
    };
    let mut transport = handle.lock_transport();
    action(&mut transport)
}

fn schedule_reconnect(handle: &DecoderHandle, now_ms: u64) -> Result<u64, String> {
    let mut owner = handle.session.lock_recover();
    let delay = owner.transport.schedule_reconnect(now_ms);
    owner.reset_state();
    Ok(delay)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionOpened(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) -> jlong {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let mut owner = native.session.lock_recover();
                let connection = owner.opened(now_ms.max(0) as u64)?;
                i64::try_from(connection)
                    .map_err(|_| "QC connection identity exceeds JNI range".to_string())
            })();
            match result {
                Ok(connection) => connection,
                Err(error) => {
                    let _ = env.throw_new("java/lang/IllegalStateException", error);
                    0
                }
            }
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionConnected(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jint {
    catch_panic(|| {
        handle(value)
            .ok()
            .map(|native| native.session.lock_recover().is_connected() as jint)
            .unwrap_or(CLOSED_HANDLE_SENTINEL)
    })
    .unwrap_or(CLOSED_HANDLE_SENTINEL)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionSynchronized(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jint {
    catch_panic(|| {
        handle(value)
            .ok()
            .map(|native| native.session.lock_recover().is_synchronized() as jint)
            .unwrap_or(CLOSED_HANDLE_SENTINEL)
    })
    .unwrap_or(CLOSED_HANDLE_SENTINEL)
}

/// The session owner's suspension record as JSON: `active` is null unless the
/// device holds the session suspended.
#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSuspension(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let diagnostics = native.session.lock_recover().suspension();
                serde_json::to_string(&diagnostics).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

/// The refusal for a client request that would reach the device, or an empty
/// string when the device is not suspended. A returned refusal is counted.
#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSuspensionRefusal(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = handle(value).map(|native| {
                native
                    .session
                    .lock_recover()
                    .suspension_refusal()
                    .unwrap_or_default()
            });
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

/// The reason of an ended suspension that requires a full session resync,
/// exactly once, or an empty string.
#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeTakeSuspensionResync(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = handle(value).map(|native| {
                native
                    .session
                    .lock_recover()
                    .take_suspension_resync()
                    .unwrap_or_default()
            });
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionShouldKeepalive(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) -> jint {
    catch_panic(|| {
        with_transport(value, |transport| {
            transport.keepalive_due(now_ms.max(0) as u64) as jint
        })
    })
    .unwrap_or(CLOSED_HANDLE_SENTINEL)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionKeepaliveSent(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) {
    catch_panic(|| {
        let _ = with_transport(value, |transport| {
            transport.keepalive_sent(now_ms.max(0) as u64);
            0
        });
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionDisconnected(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) {
    catch_panic(|| {
        if let Ok(native) = handle(value) {
            let mut owner = native.session.lock_recover();
            owner.transport.disconnect(now_ms.max(0) as u64, false);
            owner.reset_state();
        }
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionScheduleReconnect(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) -> jlong {
    catch_panic(|| {
        handle(value)
            .and_then(|handle| schedule_reconnect(&handle, now_ms.max(0) as u64))
            .and_then(|delay| {
                i64::try_from(delay)
                    .map_err(|_| "QC reconnect delay exceeds the JNI range".to_string())
            })
            .unwrap_or(CLOSED_HANDLE_SENTINEL as jlong)
    })
    .unwrap_or(CLOSED_HANDLE_SENTINEL as jlong)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionReconnectDue(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) -> jint {
    catch_panic(|| {
        with_transport(value, |transport| {
            transport.reconnect_due(now_ms.max(0) as u64) as jint
        })
    })
    .unwrap_or(CLOSED_HANDLE_SENTINEL)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionReconnectAttempted(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) {
    catch_panic(|| {
        let _ = with_transport(value, |transport| {
            transport.reconnect_attempted(now_ms.max(0) as u64);
            0
        });
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSessionTerminalReadFailed(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jint {
    catch_panic(|| with_transport(value, |transport| transport.terminal_read_failed() as jint))
        .unwrap_or(CLOSED_HANDLE_SENTINEL)
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeReset(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) {
    or_throw(
        catch_panic(|| {
            let Ok(handle) = handle(value) else {
                let _ = env.throw_new(
                    "java/lang/IllegalStateException",
                    "native QC decoder handle is closed",
                );
                return;
            };
            handle.session.lock_recover().reset_state();
            // Transport closure/reconnect policy remains with the same session owner.
        }),
        &mut env,
        (),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    value: jlong,
) {
    catch_panic(|| {
        unregister_handle(value);
    })
    .unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeQueueModelRepo(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    ticket: JString,
    payload: JByteArray,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let ticket = observation_ticket(&mut env, ticket)?;
                let payload = env
                    .convert_byte_array(payload)
                    .map_err(|error| error.to_string())?;
                let mut owner = native.session.lock_recover();
                Ok(owner.queue_model_repo(ticket, payload).to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeModelRepoReady(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                Ok(owner.model_repo_ready().to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

fn parse_next_model_repo(native: &DecoderHandle, now_ms: u64) -> Result<String, String> {
    let started = std::time::Instant::now();
    let work = native.session.lock_recover().take_model_repo();
    let Some(work) = work else {
        return Ok("null".into());
    };
    // The payload stays in Rust and parsing never holds the session mutex.
    let catalog = parse_model_repo(&work.payload).map_err(|error| error.to_string());
    let mut owner = native.session.lock_recover();
    let completed_ms =
        now_ms.saturating_add(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
    let (states, error) = match owner.complete_model_repo(&work.ticket, catalog, completed_ms) {
        Ok(states) => (states.unwrap_or_default(), None),
        Err(error) => (Vec::new(), Some(error)),
    };
    drop(owner);
    serde_json::to_string(&serde_json::json!({
        "observationTicket": serde_json::to_string(&work.ticket).map_err(|error| error.to_string())?,
        "states": states,
        "error": error,
    })).map_err(|error| error.to_string())
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeStatePublicationsReady(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                Ok(owner.state_publications_ready().to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeTakeStatePublications(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let publications = native.session.lock_recover().take_state_publications()?;
                // Encode after releasing the owner. The delivery worker validates each
                // receipt again under the Java publication/reset fence.
                serde_json::to_string(&publications).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeParseNextModelRepo(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = handle(value)
                .and_then(|native| parse_next_model_repo(&native, now_ms.max(0) as u64));
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeSnapshot(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let state = native.lock_state();
                serde_json::to_string(state.snapshot()).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePublishedSettings(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    message_type: jint,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let message_type = u16::try_from(message_type)
                    .map_err(|_| "settings message type must be a valid QC type".to_string())?;
                // The same projection the Windows broker applies to its retained
                // publication, over the same shared fold - so both hosts answer
                // device.generalSettings and device.ioSettings identically, and
                // neither puts a READ on the wire that the QC ignores.
                let projection = match message_type {
                    quadcortex::profile::MESSAGE_TYPE_GENERAL_SETTINGS => {
                        quadcortex::device::request::GatewayResponseProjection::GeneralSettings
                    }
                    quadcortex::profile::MESSAGE_TYPE_IO_SETTINGS => {
                        quadcortex::device::request::GatewayResponseProjection::IoSettings
                    }
                    _ => {
                        return Err(format!(
                            "message type {message_type} is not a settings publication"
                        ))
                    }
                };
                let state = native.lock_state();
                let payload = state
                    .published_settings(message_type)
                    .ok_or("QC session has not published the requested settings")?
                    .to_vec();
                drop(state);
                let value = projection.decode(&payload)?;
                serde_json::to_string(&value).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePreflightObservation(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    method: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let method = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let state = native.lock_state();
                // Each guard reads its own projection. The tempo guards compare
                // device-global PRESET/GLOBAL mode, which the GatewaySnapshot cannot
                // answer: its `mode` is the footswitch mode and also contains
                // "PRESET". Keep this arm identical to the shared Rust owner.
                let observation = match method.as_str() {
                    "device.setTempoMode"
                    | "device.setGlobalTempo"
                    | "device.setFavorite"
                    | "device.setModelPinned" => state.external_preflight_observation(&method),
                    _ => serde_json::to_value(state.snapshot()).ok(),
                };
                let observation = observation.ok_or_else(|| {
                    quadcortex::device::request::gateway_preflight_unavailable_message(&method)
                })?;
                serde_json::to_string(&observation).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeCurrentPreset(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let snapshot = owner
                    .current_preset()
                    .filter(|snapshot| {
                        !snapshot.setlist_key.is_empty() && !snapshot.preset_name.is_empty()
                    })
                    .ok_or("No Quad Cortex preset has been synchronized yet")?;
                serde_json::to_string(snapshot).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeCurrentPresetObservation(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let owner = native.session.lock_recover();
                let snapshot = owner
                    .current_preset()
                    .filter(|snapshot| {
                        !snapshot.setlist_key.is_empty() && !snapshot.preset_name.is_empty()
                    })
                    .ok_or("No Quad Cortex preset has been synchronized yet")?;
                let observation = serde_json::json!({
                    "snapshot": snapshot,
                    "fullPresetRevision": owner.full_preset_revision(),
                });
                serde_json::to_string(&observation).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeFullPresetRevision(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jlong {
    or_throw(
        catch_panic(|| {
            handle(value)
                .map(|native| native.session.lock_recover().full_preset_revision() as jlong)
        })
        .and_then(|result| result),
        &mut env,
        -1,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeAssertExpectedPreset(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    params: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let params: serde_json::Value = serde_json::from_str(&String::from(
                    env.get_string(&params).map_err(|error| error.to_string())?,
                ))
                .map_err(|error| error.to_string())?;
                let owner = native.session.lock_recover();
                assert_expected_preset(owner.current_preset(), &params)?;
                Ok("null".to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeBlockDetails(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    row: i32,
    column: i32,
    params: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let params: serde_json::Value = serde_json::from_str(&String::from(
                    env.get_string(&params).map_err(|error| error.to_string())?,
                ))
                .map_err(|error| error.to_string())?;
                let owner = native.session.lock_recover();
                let current = owner
                    .current_preset()
                    .ok_or("No Quad Cortex preset has been synchronized yet")?;
                assert_expected_preset(Some(current), &params)?;
                if !current.grid_layout_complete {
                    return Err("STATE_UNAVAILABLE: The retained block details are not authoritative while Grid topology is incomplete".into());
                }
                let details = owner
                    .state
                    .block_details(
                        u32::try_from(row).map_err(|_| "invalid row".to_string())?,
                        u32::try_from(column).map_err(|_| "invalid column".to_string())?,
                    )
                    .ok_or_else(|| "There is no block in that Grid position.".to_string())?;
                serde_json::to_string(&details).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeLaneControlDetails(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    row: i32,
    control: JString,
    params: JString,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let control = String::from(
                    env.get_string(&control)
                        .map_err(|error| error.to_string())?,
                );
                let params: serde_json::Value = serde_json::from_str(&String::from(
                    env.get_string(&params).map_err(|error| error.to_string())?,
                ))
                .map_err(|error| error.to_string())?;
                let owner = native.session.lock_recover();
                assert_expected_preset(owner.current_preset(), &params)?;
                let details = owner
                    .state
                    .lane_control_details(
                        u32::try_from(row).map_err(|_| "invalid row".to_string())?,
                        &control,
                    )
                    .ok_or_else(|| "There is no matching lane control on that row.".to_string())?;
                serde_json::to_string(&details).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeModelCount(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jint {
    or_throw(
        catch_panic(|| {
            let Ok(native) = handle(value) else {
                let _ = env.throw_new(
                    "java/lang/IllegalStateException",
                    "native QC decoder handle is closed",
                );
                return 0;
            };
            let count = native.lock_state().model_count().min(i32::MAX as usize) as jint;
            count
        }),
        &mut env,
        0,
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeModelList(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let decoder = native.lock_state();
                serde_json::to_string(&decoder.model_list()?).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativeRetainedCatalog(
    mut env: JNIEnv,
    _class: JClass,
    value: jlong,
    context_json: JString,
    method: JString,
    key: JString,
    now_ms: jlong,
) -> jstring {
    or_throw(
        catch_panic(|| {
            let result = (|| {
                let native = handle(value)?;
                let context: DispatchContext = serde_json::from_str(
                    &env.get_string(&context_json)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy(),
                )
                .map_err(|error| error.to_string())?;
                let method: String = env
                    .get_string(&method)
                    .map_err(|error| error.to_string())?
                    .into();
                let key: String = env
                    .get_string(&key)
                    .map_err(|error| error.to_string())?
                    .into();
                let owner = native.session.lock_recover();
                let value = owner.retained_catalog(
                    context.session_fence(),
                    &method,
                    &key,
                    now_ms.max(0) as u64,
                )?;
                let value = serde_json::json!({"ready": value.is_some(), "value": value});
                serde_json::to_string(&value).map_err(|error| error.to_string())
            })();
            json_result(&mut env, result)
        }),
        &mut env,
        ptr::null_mut(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_android_frame_reduces_state_before_native_admission() {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        let snapshot = owner.state.snapshot_mut();
        snapshot.has_preset = true;
        snapshot.preset_name = "Preset".into();
        snapshot.preset_position = 1;
        snapshot.setlist_key = "/media/p4/Presets/Live".into();
        let context = owner
            .context_for_write(
                "device.selectScene",
                &serde_json::json!({"scene": 2, "expectedPresetName": "Preset"}),
                &PlannedWrite::HidCommand(commands::DeviceCommand::SelectScene(2)),
            )
            .unwrap();
        let (context, _) = owner
            .bind_write_preflight(
                &context,
                "device.selectScene",
                &serde_json::json!({"scene": 2, "expectedPresetName": "Preset"}),
                &serde_json::json!({"presetName": "Preset", "activeScene": 0}),
                0,
            )
            .unwrap();
        let operation = owner.plan_operation_in_context(&context, 0, 3000).unwrap();
        let ticket = owner.capture_observation(connection).unwrap();
        let frame = quadcortex::device::transport::InboundFrame {
            message_type: quadcortex::profile::MESSAGE_TYPE_SETLIST_POSITION,
            // SetlistPosition UPDATE, position 4.
            payload: vec![8, 1, 32, 4],
            report_count: 1,
            ingress: None,
            device_bytes: [0, 0],
        };
        let payload = frame.payload.clone();
        let packet = complete_native_frame(&mut owner, frame, ticket.clone(), 1).unwrap();
        // Java has not received or decoded this packet yet.
        assert_eq!(owner.state.snapshot().preset_position, 4);
        assert!(owner
            .validate_operation(&operation, 1)
            .unwrap_err()
            .contains("target changed"));
        let length = u32::from_le_bytes(packet[2..6].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&packet[6..6 + length]).unwrap();
        assert_eq!(header["observation"], serde_json::to_value(ticket).unwrap());
        assert_eq!(header["states"][0]["position"], 4);
        assert_eq!(&packet[6 + length..], payload.as_slice());
        let queued = serde_json::to_value(owner.take_state_publications().unwrap()).unwrap();
        assert_eq!(queued.as_array().unwrap().len(), 1);
        assert_eq!(queued[0]["observation"], header["observation"]);
        assert_eq!(queued[0]["states"], header["states"]);
        assert!(
            queued[0]["states"][0]["stateVersion"]["revision"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(!owner.state_publications_ready());
    }

    #[test]
    fn late_android_frame_invalidates_lifecycle_before_returning_to_java() {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        owner.state.snapshot_mut().has_preset = true;
        let operation = owner.plan_operation(0, 120_000).unwrap();
        let ticket = owner.capture_observation(connection).unwrap();
        let frame = quadcortex::device::transport::InboundFrame {
            message_type: quadcortex::profile::MESSAGE_TYPE_SETLIST_POSITION,
            payload: vec![8, 1, 32, 4],
            report_count: 1,
            ingress: None,
            device_bytes: [0, 0],
        };
        let now_ms = quadcortex::profile::READY_WAIT_TIMEOUT_MS + 1;
        let packet = complete_native_frame(&mut owner, frame, ticket, now_ms).unwrap();
        assert!(owner.lifecycle.as_ref().unwrap().failure().is_some());
        assert!(!owner.state.snapshot().has_preset);
        assert!(owner
            .validate_operation(&operation, now_ms)
            .unwrap_err()
            .contains("session changed"));
        let length = u32::from_le_bytes(packet[2..6].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&packet[6..6 + length]).unwrap();
        assert!(!header["lifecycle"]["error"].is_null());
        assert_eq!(header["states"], serde_json::json!([]));
    }

    #[test]
    fn android_delivery_events_preserve_native_outcome_and_original_session() {
        use quadcortex::device::delivery::DeliveryStatus;
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        let dispatch = owner.prepare_dispatch(None, None, 0).unwrap();
        let ticket = owner.prepare_delivery(&dispatch, 4, 2, 0).unwrap();
        let ticket: DeliveryTicket =
            serde_json::from_str(&serde_json::to_string(&ticket).unwrap()).unwrap();
        delivery_event(&mut owner, &ticket, 0, 0, 0).unwrap();
        assert!(delivery_event(&mut owner, &ticket, 6, 0, 0).is_err());
        delivery_event(&mut owner, &ticket, 1, 0, 1).unwrap();
        delivery_event(&mut owner, &ticket, 3, 0, 2).unwrap();
        delivery_event(&mut owner, &ticket, 1, 1, 3).unwrap();
        delivery_event(&mut owner, &ticket, 2, 1, 4).unwrap();
        delivery_event(&mut owner, &ticket, 6, 0, 4).unwrap();
        delivery_event(&mut owner, &ticket, 5, 0, 4).unwrap();
        let record = owner.require_delivery_complete(&ticket).unwrap();
        assert_eq!(record.expected_stalls, 1);
        assert_eq!(record.delivered_reports, 2);
        let stale = owner.prepare_delivery(&dispatch, 4, 1, 4).unwrap();
        delivery_event(&mut owner, &stale, 1, 0, 5).unwrap();
        owner.opened(6).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 6));
        delivery_event(&mut owner, &stale, 2, 0, 7).unwrap();
        assert_eq!(
            owner.delivery_record(&stale).unwrap().status,
            DeliveryStatus::Indeterminate
        );
        assert!(delivery_event(&mut owner, &stale, 6, 0, 7).is_err());
        assert!(delivery_event(&mut owner, &stale, 99, 0, 7).is_err());
    }

    #[test]
    fn android_metadata_pull_parses_native_pending_work_and_releases_errors() {
        let value = register_handle();
        let native = handle(value).unwrap();
        let ticket = {
            let mut owner = native.session.lock().unwrap();
            let connection = owner.opened(0).unwrap();
            owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
            let ticket = owner.capture_observation(connection).unwrap();
            let xml = br#"<Models><Category name="Drive"><Model id="7" name="Native catalog" /></Category></Models>"#;
            assert!(xml.len() < 128);
            // ProductionAutomation.proto: action UPDATE in field 1, XML in field 3.
            let mut payload = vec![8, 1, 26, xml.len() as u8];
            payload.extend_from_slice(xml);
            assert!(owner.queue_model_repo(ticket.clone(), payload));
            ticket
        };
        let result: Value =
            serde_json::from_str(&parse_next_model_repo(&native, 0).unwrap()).unwrap();
        assert!(result["error"].is_null());
        let returned: ObservationTicket =
            serde_json::from_str(result["observationTicket"].as_str().unwrap()).unwrap();
        assert_eq!(returned, ticket);
        assert_eq!(
            native.lock_state().model_list().unwrap().models[0].name,
            "Native catalog"
        );
        assert_eq!(parse_next_model_repo(&native, 0).unwrap(), "null");
        {
            let mut owner = native.session.lock().unwrap();
            let connection = owner.opened(0).unwrap();
            owner.lifecycle = Some(SessionDriver::start(7, "replacement".into(), 0));
            let invalid = owner.capture_observation(connection).unwrap();
            assert!(owner.queue_model_repo(invalid, vec![0xff]));
        }
        let failed: Value =
            serde_json::from_str(&parse_next_model_repo(&native, 0).unwrap()).unwrap();
        assert!(!failed["error"].as_str().unwrap().is_empty());
        assert!(failed["states"].as_array().unwrap().is_empty());
        assert!(!native.session.lock().unwrap().model_repo_ready());
        assert_eq!(parse_next_model_repo(&native, 0).unwrap(), "null");
        unregister_handle(value);
    }

    #[test]
    fn registered_handles_make_destroy_idempotent_and_keep_in_flight_calls_alive() {
        let value = register_handle();
        assert!(value > 0);
        let in_flight = handle(value).expect("registered handle");

        unregister_handle(value);
        unregister_handle(value);

        assert!(handle(value).is_err());
        assert_eq!(in_flight.lock_state().model_count(), 0);
    }

    #[test]
    fn invalid_handles_are_rejected_without_dereferencing_foreign_memory() {
        assert!(handle(0).is_err());
        assert!(handle(-1).is_err());
        assert!(handle(i64::MAX).is_err());
    }

    #[test]
    fn android_final_send_stamps_file_mutations_from_the_shared_lifecycle() {
        let value = register_handle();
        let native = handle(value).expect("registered handle");
        let mut before_start = commands::move_preset("source".into(), "preset".into(), 3);
        assert!(stamp_android_file_mutation_request_id(
            &mut native.session.lock().unwrap(),
            &mut before_start
        )
        .unwrap_err()
        .contains("no native QC lifecycle"));

        let lifecycle = SessionDriver::start(40, "session".into(), 0);
        *native.lock_lifecycle() = Some(lifecycle);
        let mut message = commands::move_preset("source".into(), "preset".into(), 3);
        assert_eq!(
            stamp_android_file_mutation_request_id(
                &mut native.session.lock().unwrap(),
                &mut message
            )
            .unwrap(),
            Some(41)
        );
        assert_eq!(quadcortex::wire::request_id(4, &message.payload), Some(41));
        assert_eq!(
            stamp_android_file_mutation_request_id(
                &mut native.session.lock().unwrap(),
                &mut message
            )
            .unwrap(),
            Some(41)
        );
        assert_eq!(
            native
                .lock_lifecycle()
                .as_mut()
                .unwrap()
                .reserve_request_id(),
            42
        );

        let mut copy = commands::copy_preset(
            "source".into(),
            "preset".into(),
            false,
            "destination".into(),
            4,
        );
        assert_eq!(
            stamp_android_file_mutation_request_id(&mut native.session.lock().unwrap(), &mut copy)
                .unwrap(),
            Some(43)
        );
        assert_eq!(quadcortex::wire::request_id(4, &copy.payload), Some(43));

        let mut read = commands::read_folders(false);
        assert_eq!(
            stamp_android_file_mutation_request_id(&mut native.session.lock().unwrap(), &mut read)
                .unwrap(),
            None
        );
        unregister_handle(value);
    }

    #[test]
    fn lifecycle_receipts_roundtrip_and_cannot_cross_android_sessions() {
        let mut first = SessionDriver::start(1, "first".into(), 0);
        let reset = first.next_batch(0, 0).unwrap();
        let encoded = serde_json::to_string(&reset.ticket).unwrap();
        let ticket: LifecycleTicket = serde_json::from_str(&encoded).unwrap();
        assert!(first.is_current(&ticket));
        let mut second = SessionDriver::start(1, "second".into(), 0);
        let current = second.next_batch(0, 0).unwrap();
        assert!(!second.dispatched(&ticket, DispatchOutcome::Failed));
        assert!(second.is_current(&current.ticket));
        assert!(!second.is_connected());
    }

    #[test]
    fn lifecycle_failure_status_does_not_mark_android_transport_connected() {
        let value = register_handle();
        let native = handle(value).unwrap();
        native.lock_transport().transport_opened(0);
        let mut runtime = SessionDriver::start(1, "session".into(), 0);
        runtime.transport_failed();
        let status: Value = {
            let mut owner = native.session.lock().unwrap();
            owner.lifecycle = Some(runtime);
            serde_json::from_str(&lifecycle_status(&mut owner, 1).unwrap()).unwrap()
        };
        assert_eq!(status["error"], "Dispatch");
        assert!(!native.lock_transport().is_connected());
        unregister_handle(value);
    }

    #[test]
    fn scheduling_reconnect_discards_the_android_lifecycle_epoch() {
        let value = register_handle();
        let native = handle(value).expect("registered handle");
        let lifecycle = SessionDriver::start(1, "session".into(), 0);
        *native.lock_lifecycle() = Some(lifecycle);

        assert_eq!(
            schedule_reconnect(&native, 100).expect("reconnect schedule"),
            quadcortex::profile::RECONNECT_INTERVAL_MS
        );
        assert!(native.lock_lifecycle().is_none());
        unregister_handle(value);
    }

    #[test]
    fn serialized_android_plan_keeps_the_session_that_supplied_its_state() {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "same-wire-id".into(), 0));
        let original = serde_json::json!({"messages": [{"messageType": 13}], "detail": "scene"});
        owner.state.snapshot_mut().has_preset = true;
        owner.state.snapshot_mut().preset_name = "Preset".into();
        let context = owner
            .context_for_write(
                "device.selectScene",
                &serde_json::json!({"scene": 1, "expectedPresetName": "Preset"}),
                &quadcortex::device::request::PlannedWrite::HidCommand(
                    quadcortex::commands::DeviceCommand::SelectScene(1),
                ),
            )
            .unwrap();
        let (context, _) = owner
            .bind_write_preflight(
                &context,
                "device.selectScene",
                &serde_json::json!({"scene": 1, "expectedPresetName": "Preset"}),
                &serde_json::json!({"presetName": "Preset", "activeScene": 0}),
                0,
            )
            .unwrap();
        let bound: Value =
            serde_json::from_str(&bind_plan_envelope(original.to_string(), &context).unwrap())
                .unwrap();
        assert_eq!(bound["messages"], original["messages"]);
        assert_eq!(bound["detail"], original["detail"]);
        let fence: DispatchContext = serde_json::from_value(bound["sessionFence"].clone()).unwrap();
        assert!(owner.plan_operation_in_context(&fence, 100, 1000).is_ok());
        owner.state.snapshot_mut().preset_position += 1;
        assert!(owner
            .plan_operation_in_context(&fence, 100, 1000)
            .unwrap_err()
            .contains("target changed"));
        owner.opened(101).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "same-wire-id".into(), 101));
        assert!(owner.plan_operation_in_context(&fence, 101, 1000).is_err());
    }

    #[test]
    fn gateway_write_envelope_exposes_shared_realtime_completion_policy() {
        let detail = "Scene A sent";
        let envelope = gateway_write_envelope(
            "device.selectScene",
            detail,
            &GatewayVerification::Scene { scene: 0 },
            true,
            &gateway_write_verification_policy("device.selectScene"),
            PlannedWrite::MidiControlChange {
                controller: 35,
                value: 127,
            },
        )
        .expect("gateway envelope");
        let envelope: Value = serde_json::from_str(&envelope).expect("JSON gateway envelope");
        assert_eq!(envelope["detail"], detail);
        assert_eq!(
            envelope["observedStateKinds"],
            serde_json::json!(["preset", "scene"])
        );
        assert_eq!(envelope["lane"], "midi");
        assert_eq!(envelope["controller"], 35);
        assert_eq!(envelope["value"], 127);
        assert_eq!(envelope["realtime"], true);
        assert_eq!(envelope["interMessageIntervalMs"], 0);
        assert_eq!(envelope["preflightMethod"], "device.currentPreset");
        assert_eq!(envelope["preflightObservationSource"], "decodedSnapshot");
        assert_eq!(
            envelope["confirmationTimeoutMs"],
            quadcortex::profile::COMMAND_CONFIRMATION_TIMEOUT_MS
        );
        assert_eq!(envelope["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn android_adapter_rejects_invalid_commands_before_wire_encoding() {
        let invalid =
            PlannedWrite::HidCommand(quadcortex::commands::DeviceCommand::SelectScene(u32::MAX));
        let error = gateway_write_envelope(
            "device.selectScene",
            "invalid",
            &GatewayVerification::None,
            false,
            &gateway_write_verification_policy("device.selectScene"),
            invalid,
        )
        .expect_err("invalid command must fail closed");
        assert!(error.contains("unsupported scene"));
    }

    #[test]
    fn gateway_write_envelope_exposes_shared_remote_gesture_pacing() {
        let swipe = gateway_write_envelope(
            "device.swipeScreen",
            "Screen swipe sent",
            &GatewayVerification::None,
            false,
            &gateway_write_verification_policy("device.swipeScreen"),
            PlannedWrite::HidOperation(quadcortex::commands::DeviceOperation::ScreenDrag {
                x: 10.0,
                y: 20.0,
                to_x: 30.0,
                to_y: 40.0,
            }),
        )
        .expect("gateway envelope");
        let swipe: Value = serde_json::from_str(&swipe).expect("JSON gateway envelope");
        assert_eq!(swipe["lane"], "hid");
        // A swipe is one DRAG frame with no begin or end frame, so the Android
        // frame writer has nothing to pace: the plan carries one message and a
        // zero interval (CorOS 4.1.0, 2026-10-06: one frame scrolls, the old
        // three-frame sequence did not).
        assert_eq!(swipe["interMessageIntervalMs"], 0);
        assert_eq!(swipe["messages"].as_array().unwrap().len(), 1);

        let tap = gateway_write_envelope(
            "device.tapScreen",
            "Screen tap sent",
            &GatewayVerification::None,
            false,
            &gateway_write_verification_policy("device.tapScreen"),
            PlannedWrite::HidOperation(quadcortex::commands::DeviceOperation::ScreenTap {
                x: 799.0,
                y: 479.0,
            }),
        )
        .expect("tap gateway envelope");
        let tap: Value = serde_json::from_str(&tap).expect("JSON tap gateway envelope");
        assert!(tap["preflightMethod"].is_null());
        assert!(tap["readbackMethod"].is_null());
        // A tap is the single TAP frame: one message and no interval.
        assert_eq!(tap["messages"].as_array().unwrap().len(), 1);
        assert_eq!(tap["interMessageIntervalMs"], 0);
    }

    #[test]
    fn android_withholds_only_mouse_frame_with_a_typed_refusal_and_keeps_every_other_gesture() {
        // drag_screen is the same single DRAG frame as swipe_screen now, so it is
        // not withheld; the raw frame is, because a lone press stays down.
        // Every other gesture and write is untouched.
        for method in [
            "device.tapScreen",
            "device.longTapScreen",
            "device.swipeScreen",
            "device.dragScreen",
            "device.selectScene",
            "device.captureScreen",
        ] {
            assert_eq!(refuse_withheld_on_android(method), Ok(()), "{method}");
        }
        assert_eq!(ANDROID_WITHHELD_METHODS, ["device.mouseFrame"]);
        let error = refuse_withheld_on_android("device.mouseFrame")
            .expect_err("mouse_frame must be refused on Android");
        assert!(error.starts_with("device.mouseFrame is not supported by the Android host"));
        // The refusal sits in the JNI planning entry point before any state
        // or argument handling. Source-level check, because that function
        // needs a live JNI environment.
        let source = include_str!("lib.rs");
        let entry = source
            .split("fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePlanGatewayWrite(")
            .nth(1)
            .expect("planning entry point");
        let refusal = entry
            .find("refuse_withheld_on_android(&method)?")
            .expect("refusal");
        let first_use = entry.find("native.lock_state()").expect("state use");
        assert!(refusal < first_use);
    }

    #[test]
    fn history_envelope_is_one_frame_with_no_refresh_or_readback() {
        for (method, operation) in [
            ("device.undo", quadcortex::commands::DeviceOperation::Undo),
            ("device.redo", quadcortex::commands::DeviceOperation::Redo),
        ] {
            let envelope = gateway_write_envelope(
                method,
                "Device history command sent",
                &GatewayVerification::None,
                false,
                &gateway_write_verification_policy(method),
                PlannedWrite::HidOperation(operation),
            )
            .expect("gateway envelope");
            let envelope: Value = serde_json::from_str(&envelope).expect("JSON gateway envelope");
            assert_eq!(envelope["messages"].as_array().unwrap().len(), 1);
            // One UndoRedo UPDATE and nothing after it: no refresh field
            // exists any more, and no readback or preflight is planned.
            assert!(envelope.get("postWriteRefreshMethod").is_none());
            assert!(envelope.get("postWriteRefreshDelayMs").is_none());
            assert!(envelope["readbackMethod"].is_null());
            assert!(envelope["preflightMethod"].is_null());
        }
    }

    #[test]
    fn delete_setlist_envelope_is_single_send_with_shared_catalog_policy() {
        let envelope = gateway_write_envelope(
            "device.deleteSetlist",
            "deleteSetlist sent to the Quad Cortex",
            &GatewayVerification::None,
            false,
            &gateway_write_verification_policy("device.deleteSetlist"),
            PlannedWrite::HidOperation(quadcortex::commands::DeviceOperation::DeleteSetlist {
                name: "Tour".into(),
            }),
        )
        .expect("delete-setlist gateway envelope");
        let envelope: Value = serde_json::from_str(&envelope).expect("delete-setlist JSON");
        assert_eq!(envelope["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            envelope["messages"][0]["messageType"],
            quadcortex::profile::MESSAGE_TYPE_FILE
        );
        assert_eq!(
            envelope["catalogPolicy"]["timeoutMs"],
            quadcortex::profile::PERSISTENT_FILE_VERIFICATION_TIMEOUT_MS
        );
        assert_eq!(
            envelope["catalogPolicy"]["initialRetryMs"],
            quadcortex::profile::PERSISTENT_FILE_VERIFICATION_RETRY_MS
        );
    }

    #[test]
    fn android_state_follows_explicit_deletion_and_recreation() {
        use prost::Message;
        use quadcortex::proto::cortex_protobuf_v2 as pa;
        let value = register_handle();
        let native = handle(value).expect("registered Android state handle");
        let key = "/media/p4/Presets/My Presets";
        let product_key = "/media/p4/Presets/My Presets/Disposable.pb";
        let mut state = native.lock_state();
        for (action, present) in [
            (pa::message_action::Enum::Create, true),
            (pa::message_action::Enum::Delete, false),
            (pa::message_action::Enum::Create, true),
        ] {
            let payload = pa::FileMessage {
                action: action as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(key.into())),
                    files: vec![pa::ProductData {
                        index: Some(pa::product_data::Index::Index(30)),
                        key: Some(pa::product_data::Key::Key(product_key.into())),
                        name: Some(pa::product_data::Name::Name("Disposable".into())),
                        instrument: Some(pa::product_data::Instrument::Instrument(2)),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec();
            state
                .ingest(quadcortex::profile::MESSAGE_TYPE_FILE, &payload)
                .unwrap();
            assert_eq!(state.preset_library().entry(key, 30).is_some(), present);
        }
        drop(state);
        unregister_handle(value);
    }

    #[test]
    fn catch_panic_turns_a_panicking_export_body_into_an_error() {
        assert_eq!(catch_panic(|| 7 as jint), Ok(7));
        let static_message = catch_panic(|| -> jint { panic!("static failure") });
        assert_eq!(
            static_message,
            Err("native QC engine panicked: static failure".to_string())
        );
        let formatted = catch_panic(|| -> jstring { panic!("index {} out of range", 9) });
        assert_eq!(
            formatted,
            Err("native QC engine panicked: index 9 out of range".to_string())
        );
        // Silent exports map a contained panic onto their existing sentinel.
        assert_eq!(
            catch_panic(|| -> jint { panic!("transport fault") }).unwrap_or(CLOSED_HANDLE_SENTINEL),
            CLOSED_HANDLE_SENTINEL
        );
        let unit: () = catch_panic(|| -> () { panic!("void export") }).unwrap_or_default();
        assert_eq!(unit, ());
    }

    #[test]
    fn a_poisoned_session_lock_is_recovered_instead_of_bricking_the_handle() {
        let value = register_handle();
        let native = handle(value).expect("registered handle");
        let poisoner = Arc::clone(&native);
        let outcome = std::thread::spawn(move || {
            let _owner = poisoner.session.lock().unwrap();
            panic!("panic while the session lock is held");
        })
        .join();
        assert!(outcome.is_err());
        assert!(native.session.is_poisoned());

        // Every lock path still reaches the session after the poisoning panic.
        assert_eq!(with_transport(value, |_| 5), 5);
        assert!(schedule_reconnect(&native, 0).is_ok());
        assert_eq!(native.lock_state().model_count(), 0);
        assert!(native.lock_lifecycle().is_none());
        native.lock_transport().transport_opened(0);
        // Not the closed-handle sentinel: the transport is live.
        assert_ne!(
            with_transport(value, |transport| transport.is_connected() as jint),
            CLOSED_HANDLE_SENTINEL
        );
        unregister_handle(value);
        assert!(handle(value).is_err());
    }

    #[test]
    fn a_poisoned_handle_registry_still_registers_and_resolves_handles() {
        let outcome = std::thread::spawn(|| {
            let _registry = handles().lock_recover();
            panic!("panic while the handle registry lock is held");
        })
        .join();
        assert!(outcome.is_err());
        assert!(handles().is_poisoned());
        let value = register_handle();
        assert!(value > 0);
        assert!(handle(value).is_ok());
        unregister_handle(value);
        assert!(handle(value).is_err());
    }

    #[test]
    fn a_recall_with_only_an_observed_address_hands_java_the_unbound_outcome() {
        // Preset A ("Preset A") at slot 8. The predicate below matches a mirror at the
        // destination address that still holds A's old body: QcUsbPlugin reported exactly that
        // as verified:true, authoritative_readback before the address-only outcome existed.
        // The native action must hand it the address-only outcome instead.
        let mut before = quadcortex::device::GatewaySnapshot {
            preset_name: "Preset A".into(),
            setlist_key: "/user/live".into(),
            preset_position: 8,
            position_revision: 14,
            full_preset_revision: 14,
            ..Default::default()
        };
        // The unit has reported its dirty flag (clean): since the initial-dirty guard, a recall
        // or bank change planned against the placeholder flag is refused before any frame.
        before.unobserved.retain(|field| *field != "dirty");
        for (method, args, target) in [
            (
                "device.recallPreset",
                serde_json::json!({"setlistKey": "/user/live", "position": 9,
                    "expectedPresetName": "Preset A", "expectedPosition": 8}),
                9_u32,
            ),
            (
                "device.navigateBank",
                serde_json::json!({"direction": 1, "expectedPresetName": "Preset A",
                    "expectedPosition": 8}),
                16,
            ),
            (
                "device.reloadPreset",
                serde_json::json!({"expectedPresetName": "Preset A", "expectedPosition": 8}),
                8,
            ),
        ] {
            let plan = plan_preset_recall(method, &args, Some(&before)).unwrap();
            let verification = plan.verification();
            let mut after = before.clone();
            after.preset_position = target;
            after.position_revision = 15;
            after.full_preset_revision = 15;
            assert!(verification.matches(&after, None), "{method}");
            let action = verified_action(
                method,
                &serde_json::to_string(&verification).unwrap(),
                &after,
            );
            assert_eq!(
                action["kind"], "verified",
                "{method}: older Java keeps working"
            );
            let outcome = &action["outcome"];
            assert_eq!(outcome["verified"], false, "{method}: {action}");
            assert_eq!(outcome["verification"], "accepted_unverified", "{method}");
            assert_eq!(outcome["observation"]["position"], target, "{method}");
            assert_eq!(outcome["observation"]["body"], "unbound", "{method}");
            assert!(
                outcome.get("snapshot").is_none(),
                "{method}: Java attaches the relay"
            );
        }

        // Every other verified write keeps its action unchanged.
        let scene = verified_action(
            "device.selectScene",
            &serde_json::to_string(&GatewayVerification::Scene { scene: 0 }).unwrap(),
            &before,
        );
        assert_eq!(scene, serde_json::json!({"kind": "verified"}));
    }

    #[test]
    fn the_android_recall_outcome_never_claims_verification() {
        let mut before = quadcortex::device::GatewaySnapshot {
            preset_name: "Preset A".into(),
            setlist_key: "/user/live".into(),
            preset_position: 8,
            position_revision: 14,
            full_preset_revision: 14,
            ..Default::default()
        };
        before.unobserved.retain(|field| *field != "dirty");
        for (method, args, detail) in [
            (
                "device.recallPreset",
                serde_json::json!({"setlistKey": "/user/live", "position": 9,
                    "expectedPresetName": "Preset A", "expectedPosition": 8}),
                "Preset recall requested",
            ),
            (
                "device.navigateBank",
                serde_json::json!({"direction": 1, "expectedPresetName": "Preset A",
                    "expectedPosition": 8}),
                "Preset bank change requested",
            ),
            (
                "device.reloadPreset",
                serde_json::json!({"expectedPresetName": "Preset A", "expectedPosition": 8}),
                "Active preset address resent",
            ),
        ] {
            let plan = plan_preset_recall(method, &args, Some(&before)).unwrap();
            // QcUsbPlugin builds its timeout detail from this plan detail.
            assert_eq!(plan.detail, detail, "{method}");
            assert!(
                !quadcortex::device::request::detail_claims_verification(&plan.detail),
                "{method}"
            );
            let mut after = before.clone();
            after.preset_position = plan.position;
            after.position_revision = 15;
            after.full_preset_revision = 15;
            let action = verified_action(
                method,
                &serde_json::to_string(&plan.verification()).unwrap(),
                &after,
            );
            let outcome_detail = action["outcome"]["detail"].as_str().unwrap();
            assert!(
                !quadcortex::device::request::detail_claims_verification(outcome_detail),
                "{method}: {outcome_detail}"
            );
        }
    }

    #[test]
    fn a_partial_recall_observation_reaches_the_java_timeout_without_evidence() {
        // nativeGatewayVerificationAdvance returns {"kind":"timedOut"} with nothing else, so
        // QcUsbPlugin cannot know which condition was missing; QcTimedOutDetail therefore
        // names the required conditions without asserting one absent. These are the cases
        // that reach it, driven through the same shared runtime the JNI owner runs.
        let mut before = quadcortex::device::GatewaySnapshot {
            preset_name: "Preset A".into(),
            setlist_key: "/user/live".into(),
            preset_position: 8,
            position_revision: 14,
            full_preset_revision: 14,
            ..Default::default()
        };
        before.unobserved.retain(|field| *field != "dirty");
        let recall = plan_preset_recall(
            "device.recallPreset",
            &serde_json::json!({"setlistKey": "/user/live", "position": 9,
                "expectedPresetName": "Preset A", "expectedPosition": 8}),
            Some(&before),
        )
        .unwrap();
        let mut dirty = before.clone();
        dirty.dirty = true;
        let reload = plan_preset_recall(
            "device.reloadPreset",
            &serde_json::json!({"expectedPresetName": "Preset A", "expectedPosition": 8}),
            Some(&dirty),
        )
        .unwrap();
        let held =
            |position: u32, position_revision: u64, full_preset_revision: u64, is_dirty: bool| {
                let mut snapshot = before.clone();
                snapshot.preset_position = position;
                snapshot.position_revision = position_revision;
                snapshot.full_preset_revision = full_preset_revision;
                snapshot.dirty = is_dirty;
                snapshot
            };
        for (label, method, plan, evidence) in [
            (
                "(a) newer position, old body",
                "device.recallPreset",
                &recall,
                held(9, 15, 14, false),
            ),
            (
                "(b) newer body, no newer position",
                "device.recallPreset",
                &recall,
                held(8, 14, 15, false),
            ),
            (
                "(c) reload: position and body, clean flag missing",
                "device.reloadPreset",
                &reload,
                held(8, 15, 15, true),
            ),
            (
                "(d) nothing arrived",
                "device.recallPreset",
                &recall,
                held(8, 14, 14, false),
            ),
        ] {
            let policy = gateway_write_verification_policy(method);
            let mut runtime =
                GatewayVerificationRuntime::new(plan.verification(), 5, 1_000, &policy);
            assert!(
                matches!(
                    runtime.advance(Some(&evidence), None, 6, 1_001),
                    GatewayVerificationAction::Wait { .. }
                ),
                "{label}"
            );
            assert_eq!(
                runtime.advance(Some(&evidence), None, 6, 1_000 + policy.timeout_ms),
                GatewayVerificationAction::TimedOut,
                "{label}"
            );
            // The plan detail Java prefixes names the request only.
            assert!(
                !quadcortex::device::request::detail_claims_verification(&plan.detail),
                "{label}"
            );
        }
    }

    #[test]
    fn owner_advance_serializes_partial_recall_timeout_and_consumes_identity() {
        for (method, position, position_revision, full_revision, dirty) in [
            ("device.recallPreset", 9, 15, 14, false),
            ("device.recallPreset", 8, 14, 15, false),
            ("device.recallPreset", 8, 14, 14, false),
            ("device.reloadPreset", 8, 15, 15, true),
        ] {
            let mut owner = SessionRuntime::new(0);
            owner.opened(0).unwrap();
            owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
            let before = owner.state.snapshot_mut();
            before.has_preset = true;
            before.preset_name = "Preset A".into();
            before.setlist_key = "/user/live".into();
            before.preset_position = 8;
            before.position_revision = 14;
            before.full_preset_revision = 14;
            before.unobserved.retain(|field| *field != "dirty");
            before.dirty = dirty;
            let plan = plan_preset_recall(
                method,
                &serde_json::json!({
                    "setlistKey": "/user/live", "position": 9,
                    "expectedPresetName": "Preset A", "expectedPosition": 8
                }),
                Some(owner.state.snapshot()),
            )
            .unwrap();
            let predicate = serde_json::to_string(&plan.verification()).unwrap();
            let policy = gateway_write_verification_policy(method);
            let context = owner.session_context().unwrap();
            let id = owner
                .begin_gateway_verification(
                    &context,
                    GatewayVerificationRuntime::new(plan.verification(), 5, 1_000, &policy),
                )
                .unwrap();
            let snapshot = owner.state.snapshot_mut();
            snapshot.preset_position = position;
            snapshot.position_revision = position_revision;
            snapshot.full_preset_revision = full_revision;
            let wait: Value = serde_json::from_str(
                &advance_gateway_verification_json(&mut owner, id, 6, 1_001, method, &predicate)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(wait["kind"], "wait");
            assert!(wait["delayMs"].as_u64().unwrap() > 0);
            let wire = advance_gateway_verification_json(
                &mut owner,
                id,
                6,
                1_000 + policy.timeout_ms,
                method,
                &predicate,
            )
            .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&wire).unwrap(),
                serde_json::json!({"kind":"timedOut"})
            );
            assert!(advance_gateway_verification_json(
                &mut owner,
                id,
                6,
                1_000 + policy.timeout_ms,
                method,
                &predicate
            )
            .is_err());
        }
    }

    #[test]
    fn owner_advance_serializes_matched_recall_as_unverified_and_rejects_cancelled_identity() {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        let before = owner.state.snapshot_mut();
        before.has_preset = true;
        before.preset_name = "Preset A".into();
        before.setlist_key = "/user/live".into();
        before.preset_position = 8;
        before.position_revision = 14;
        before.full_preset_revision = 14;
        before.unobserved.retain(|field| *field != "dirty");
        let plan = plan_preset_recall(
            "device.recallPreset",
            &serde_json::json!({
                "setlistKey": "/user/live", "position": 9,
                "expectedPresetName": "Preset A", "expectedPosition": 8
            }),
            Some(owner.state.snapshot()),
        )
        .unwrap();
        let predicate = serde_json::to_string(&plan.verification()).unwrap();
        let policy = gateway_write_verification_policy("device.recallPreset");
        let context = owner.session_context().unwrap();
        let cancelled = owner
            .begin_gateway_verification(
                &context,
                GatewayVerificationRuntime::new(plan.verification(), 5, 1_000, &policy),
            )
            .unwrap();
        owner.cancel_gateway_verification(cancelled);
        assert!(advance_gateway_verification_json(
            &mut owner,
            cancelled,
            6,
            1_001,
            "device.recallPreset",
            &predicate
        )
        .is_err());
        let id = owner
            .begin_gateway_verification(
                &context,
                GatewayVerificationRuntime::new(plan.verification(), 5, 1_000, &policy),
            )
            .unwrap();
        let snapshot = owner.state.snapshot_mut();
        snapshot.preset_position = 9;
        snapshot.position_revision = 15;
        snapshot.full_preset_revision = 15;
        let action: Value = serde_json::from_str(
            &advance_gateway_verification_json(
                &mut owner,
                id,
                6,
                1_001,
                "device.recallPreset",
                &predicate,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(action["kind"], "verified");
        assert_eq!(action["outcome"]["verified"], false);
        assert_eq!(action["outcome"]["verification"], "accepted_unverified");
        assert!(action["outcome"].get("readback").is_none());
        assert!(action["outcome"].get("changed").is_none());
        let replaced = owner
            .begin_gateway_verification(
                &context,
                GatewayVerificationRuntime::new(plan.verification(), 5, 1_000, &policy),
            )
            .unwrap();
        owner.opened(1_002).unwrap();
        assert!(advance_gateway_verification_json(
            &mut owner,
            replaced,
            6,
            1_003,
            "device.recallPreset",
            &predicate
        )
        .is_err());
        assert!(advance_gateway_verification_json(
            &mut owner,
            0,
            6,
            1_003,
            "device.recallPreset",
            &predicate
        )
        .is_err());
    }

    #[test]
    fn every_jni_export_body_runs_inside_the_panic_guard() {
        let mut checked = 0;
        for source in [include_str!("lib.rs"), include_str!("preset_workflow.rs")] {
            let mut lines = source.lines();
            while let Some(line) = lines.next() {
                if !line.starts_with("pub extern \"system\" fn Java_") {
                    continue;
                }
                let name = line.to_string();
                for signature in lines.by_ref() {
                    if signature.starts_with(')') && signature.ends_with('{') {
                        break;
                    }
                }
                let first = lines.next().unwrap_or_default().trim();
                assert!(
                    first == "or_throw(" || first.starts_with("catch_panic("),
                    "{name} is not wrapped in the JNI panic guard"
                );
                checked += 1;
            }
        }
        assert!(checked >= 70, "only {checked} JNI exports found");
    }

    /// Both hosts reach the same owner: the reader pushes reports with
    /// `push_native_report` and the planner reads `owner.state` under the same
    /// lock. This drives the Android entry points (`complete_native_frame` for
    /// the frame, `plan_gateway_write_with_state` on the state as
    /// `nativePlanGatewayWrite` does).
    #[test]
    fn a_lost_delete_refuses_the_next_android_load_with_no_later_frame() {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        let epoch = owner.capture_report_epoch(connection).unwrap();
        let push = |owner: &mut SessionRuntime, message_type: u16, payload: &[u8], at: u64| {
            let mut completed = None;
            for mut report in framing::encode(message_type, payload) {
                report[0] = framing::IN_REPORT_ID;
                completed = owner
                    .push_native_report(connection, epoch, &report, at)
                    .unwrap();
            }
            completed
        };
        let load = |owner: &SessionRuntime| {
            plan_gateway_write_with_state(
                "device.loadGlobalEqModelPreset",
                &serde_json::json!({"value": "one", "isFactory": false, "hash": 4004}),
                &owner.state,
            )
        };
        // A complete frame first, then the device announces "one".
        push(&mut owner, 13, &[8, 1, 16, 42], 1).unwrap();
        let create = [
            0x08, 0x00, 0x1a, 0x0c, 0x0a, 0x0a, 0x0a, 0x03, b'o', b'n', b'e', 0x10, 0x00, 0x18,
            0xa4, 0x1f,
        ];
        let (frame, ticket) = push(&mut owner, 71, &create, 2).unwrap();
        complete_native_frame(&mut owner, frame, ticket, 2).unwrap();
        assert!(load(&owner).is_ok());

        // A later delete loses its first report: only orphaned continuations arrive, nothing
        // completes, and no frame follows.
        let lost_delete = vec![0_u8; 400];
        let reports = framing::encode(71, &lost_delete);
        assert!(reports.len() >= 3);
        for mut report in reports.into_iter().skip(1) {
            report[0] = framing::IN_REPORT_ID;
            let _ = owner.push_native_report(connection, epoch, &report, 3);
        }
        let refused = load(&owner).unwrap_err();
        assert!(refused.starts_with("STATE_UNAVAILABLE: "), "{refused}");
        assert!(refused.contains("inbound-loss"), "{refused}");
    }
}
