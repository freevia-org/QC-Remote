//! Small C ABI for iOS hosts that own USB I/O and use the shared QC runtime.
//!
//! The caller serializes access to each opaque session. No pointer is retained
//! across calls except the session returned by [`qc_ios_session_create`].

use quadcortex::{
    commands::{self, DeviceOperation},
    device::{
        backup::BackupAction,
        catalog::{CatalogVerificationAction, CatalogVerificationPolicy},
        delivery::{DeliveryStop, DeliveryTicket, ReportOutcome},
        owner::{
            DispatchContext, NativeDispatchTicket, OperationTicket, OwnedPresetPreparationAction,
            SessionRuntime,
        },
        preset_workflow::PresetWorkflowAction,
        read::{GatewayReadRuntime, ReadObservation},
        request::{
            finalize_device_backup, gateway_read_followup_method, gateway_write_budget_ms,
            gateway_write_is_realtime, gateway_write_verification_policy, plan_gateway_read,
            plan_gateway_write_with_state, plan_preset_recall, validate_backup_name,
            GatewayVerification, GatewayVerificationAction, GatewayVerificationRuntime,
            PlannedWrite,
        },
        session::{DispatchOutcome, SessionDriver},
        transport::{IngressTime, ReportLayout},
    },
    framing,
};
use serde_json::{json, Value};
use std::{ffi::c_char, panic::AssertUnwindSafe, ptr, slice};

/// Opaque per-connection state owned by the Swift host.
pub struct QcIosSession {
    runtime: SessionRuntime,
    pending_output: Option<Vec<u8>>,
    backup_operation: Option<OperationTicket>,
    backup_name: Option<String>,
}

fn ios_lifecycle_status(lifecycle: &SessionDriver) -> Value {
    json!({
        "phase": lifecycle.phase().as_str(),
        "sessionValidated": lifecycle.session_validated(),
        "error": lifecycle.failure().map(|failure| format!("{failure:?}")),
        "synchronized": lifecycle.synchronization() == Some(true),
    })
}

fn ios_workflow_write_action(
    session: &mut QcIosSession,
    operation: OperationTicket,
    write: PlannedWrite,
    preparation: bool,
    read: bool,
    now_ms: u64,
) -> Result<Value, String> {
    let interval = write.inter_message_interval_ms();
    let messages = match write {
        PlannedWrite::HidCommand(command) => {
            vec![command.try_encode().map_err(|e| e.to_string())?]
        }
        PlannedWrite::HidOperation(operation) => {
            operation.try_encode().map_err(|e| e.to_string())?
        }
        PlannedWrite::MidiControlChange { .. } => {
            return Err("Preset workflow requires USB HID".into())
        }
    };
    let dispatch = session
        .runtime
        .prepare_dispatch(Some(&operation), None, now_ms)?;
    let messages = messages
        .into_iter()
        .map(|message| {
            json!({
                "messageType": message.message_type,
                "payload": message.payload,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "kind": "write", "operationTicket": operation, "dispatchTicket": dispatch,
        "messages": messages, "interMessageIntervalMs": interval,
        "preparation": preparation, "read": read, "delayMs": 10,
    }))
}

fn ios_workflow_read_action(
    session: &mut QcIosSession,
    method: &str,
    params: &Value,
    context: &DispatchContext,
    remaining_ms: u64,
    now_ms: u64,
    preparation: bool,
) -> Result<Value, String> {
    let scope = session
        .runtime
        .begin_gateway_read_scope_for(context, now_ms, remaining_ms)?;
    let remaining = session
        .runtime
        .gateway_read_scope_remaining(&scope, now_ms)?;
    let operation = if method == "device.listPresetFolders" {
        DeviceOperation::ListPresetFolders
    } else {
        let lifecycle = session
            .runtime
            .lifecycle
            .as_mut()
            .filter(|lifecycle| lifecycle.is_connected())
            .ok_or("No connected QC session for preset workflow read")?;
        plan_gateway_read(method, params, lifecycle.reserve_request_id())?.operation
    };
    let ticket = session
        .runtime
        .plan_operation_in_context(scope.context(), now_ms, remaining)?;
    ios_workflow_write_action(
        session,
        ticket,
        PlannedWrite::HidOperation(operation),
        preparation,
        true,
        now_ms,
    )
}

fn ios_advance_preset_workflow(
    session: &mut QcIosSession,
    preparation: u64,
    now_ms: u64,
) -> Result<Value, String> {
    let workflow = match session
        .runtime
        .advance_preset_preparation(preparation, now_ms)?
    {
        OwnedPresetPreparationAction::Read {
            method,
            params,
            context,
            remaining_ms,
        } => {
            return ios_workflow_read_action(
                session,
                &method,
                &params,
                &context,
                remaining_ms,
                now_ms,
                true,
            );
        }
        OwnedPresetPreparationAction::Wait { until_ms } => {
            return Ok(json!({"kind":"wait", "delayMs": until_ms.saturating_sub(now_ms).min(10)}));
        }
        OwnedPresetPreparationAction::Ready { workflow_id } => workflow_id,
    };
    let step = session.runtime.advance_preset_workflow(workflow, now_ms)?;
    if let Some(catalog) = step.catalog {
        if catalog.action == CatalogVerificationAction::Request {
            return ios_workflow_read_action(
                session,
                "device.listPresetFolders",
                &Value::Null,
                &catalog.context,
                catalog.remaining_ms,
                now_ms,
                false,
            );
        }
    }
    match step.action {
        PresetWorkflowAction::Dispatch { write, .. } => ios_workflow_write_action(
            session,
            step.operation
                .ok_or("Preset workflow action has no operation ticket")?,
            write,
            false,
            false,
            now_ms,
        ),
        PresetWorkflowAction::Finished(_) => {
            let result = session
                .runtime
                .preset_workflow_result(workflow)?
                .ok_or("Finished preset workflow has no retained result")?
                .gateway_result();
            Ok(match result {
                Ok(value) => json!({"kind":"finished", "result":value}),
                Err(error) => json!({"kind":"finished", "error":error}),
            })
        }
        PresetWorkflowAction::Wait { until_ms } => {
            Ok(json!({"kind":"wait", "delayMs": until_ms.saturating_sub(now_ms).min(10)}))
        }
        PresetWorkflowAction::AwaitReceipt { .. }
        | PresetWorkflowAction::Verify { .. }
        | PresetWorkflowAction::VerifyCatalog { .. } => Ok(json!({"kind":"wait", "delayMs":10})),
    }
}

fn protect<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], String> {
    if len == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err("null byte buffer".into());
    }
    Ok(slice::from_raw_parts(ptr, len))
}

fn write_json(value: Value, output: *mut c_char, capacity: usize) -> isize {
    let Ok(encoded) = serde_json::to_vec(&value) else {
        return -1;
    };
    if output.is_null() || capacity < encoded.len() + 1 {
        return -(encoded.len() as isize) - 1;
    }
    unsafe {
        ptr::copy_nonoverlapping(encoded.as_ptr(), output.cast::<u8>(), encoded.len());
        *output.add(encoded.len()) = 0;
    }
    encoded.len() as isize
}

/// Creates a closed session. The host must serialize calls for this handle.
#[no_mangle]
pub extern "C" fn qc_ios_session_create() -> *mut QcIosSession {
    protect(ptr::null_mut(), || {
        Box::into_raw(Box::new(QcIosSession {
            runtime: SessionRuntime::new(0),
            pending_output: None,
            backup_operation: None,
            backup_name: None,
        }))
    })
}

fn write_session_json(
    session: &mut QcIosSession,
    value: Value,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    let Ok(encoded) = serde_json::to_vec(&value) else {
        return -1;
    };
    if output.is_null() || capacity < encoded.len() + 1 {
        let required = encoded.len();
        session.pending_output = Some(encoded);
        return -(required as isize) - 1;
    }
    unsafe {
        ptr::copy_nonoverlapping(encoded.as_ptr(), output.cast::<u8>(), encoded.len());
        *output.add(encoded.len()) = 0;
    }
    encoded.len() as isize
}

fn flush_session_output(
    session: &mut QcIosSession,
    output: *mut c_char,
    capacity: usize,
) -> Option<isize> {
    let encoded = session.pending_output.take()?;
    if output.is_null() || capacity < encoded.len() + 1 {
        let required = encoded.len();
        session.pending_output = Some(encoded);
        return Some(-(required as isize) - 1);
    }
    unsafe {
        ptr::copy_nonoverlapping(encoded.as_ptr(), output.cast::<u8>(), encoded.len());
        *output.add(encoded.len()) = 0;
    }
    Some(encoded.len() as isize)
}

/// Destroys a session returned by [`qc_ios_session_create`].
///
/// # Safety
/// `session` must be null or a live handle returned by this library exactly once.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_session_destroy(session: *mut QcIosSession) {
    if !session.is_null() {
        drop(Box::from_raw(session));
    }
}

/// Opens a new native connection and returns its monotonically increasing identity.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_session_opened(session: *mut QcIosSession, now_ms: u64) -> u64 {
    protect(0, || {
        let Some(session) = session.as_mut() else {
            return 0;
        };
        session.runtime.opened(now_ms).unwrap_or(0)
    })
}

/// Returns the current report epoch, or zero when the connection is stale.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_report_epoch(session: *mut QcIosSession, connection: u64) -> u64 {
    protect(0, || {
        let Some(session) = session.as_mut() else {
            return 0;
        };
        session
            .runtime
            .capture_report_epoch(connection)
            .unwrap_or(0)
    })
}

/// Pushes one HID input report and writes a JSON event when it completes a QC frame.
///
/// Returns `0` when a report is incomplete or belongs to an inactive/stale
/// reader, a positive JSON byte count when an event was written, and a negative
/// required capacity minus one on short output. Invalid input returns `-1`.
/// If the buffer is short, the event is retained and returned by the next
/// stateful JSON call when given enough space.
///
/// # Safety
/// All pointers must reference their declared lengths; session access must be serialized.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_push_report(
    session: *mut QcIosSession,
    connection: u64,
    epoch: u64,
    report: *const u8,
    report_len: usize,
    received_at_ms: u64,
    output: *mut c_char,
    output_capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, output_capacity) {
            return result;
        }
        let Ok(report) = bytes(report, report_len) else {
            return -1;
        };
        let (frame, ticket) =
            match session
                .runtime
                .push_native_report(connection, epoch, report, received_at_ms)
            {
                Ok(Some(value)) => value,
                Ok(None) => return 0,
                Err(_) => return -1,
            };

        let (states, tempo_clock) =
            if frame.message_type == quadcortex::profile::MESSAGE_TYPE_MODEL_REPO {
                let catalog = quadcortex::state::parse_model_repo(&frame.payload)
                    .map_err(|error| error.to_string());
                match session
                    .runtime
                    .install_model_catalog(&ticket, catalog, received_at_ms)
                {
                    Ok(updates) => (updates.unwrap_or_default(), None),
                    Err(_) => return -1,
                }
            } else {
                session.runtime.observe_lifecycle(
                    &ticket,
                    frame.message_type,
                    &frame.payload,
                    received_at_ms,
                );
                match session
                    .runtime
                    .observe_state(&ticket, frame.message_type, &frame.payload)
                {
                    Ok(Some(value)) => (value.states, value.tempo_clock),
                    Ok(None) => (Vec::new(), None),
                    Err(_) => return -1,
                }
            };
        let backup_progress =
            if frame.message_type == quadcortex::profile::MESSAGE_TYPE_LOCAL_BACKUP {
                let operation = session.backup_operation.clone();
                match (operation, session.backup_name.as_deref()) {
                    (Some(operation), Some(name)) => match session.runtime.consume_backup(
                        &operation,
                        &ticket,
                        &frame.payload,
                        received_at_ms,
                    ) {
                        Ok(Some(progress)) => {
                            let document = progress
                                .document
                                .map(|raw| finalize_device_backup(&raw, name))
                                .transpose();
                            match document {
                                Ok(document) => Some(json!({
                                    "complete": document.is_some(), "document": document,
                                    "started": progress.started, "chunks": progress.chunks,
                                    "ignoredPrefixChunks": progress.ignored_prefix_chunks,
                                    "ignoredPrefixTerminators": progress.ignored_prefix_terminators,
                                })),
                                Err(_) => return -1,
                            }
                        }
                        Ok(None) => Some(json!({"complete": false, "ignored": true})),
                        Err(_) => return -1,
                    },
                    _ => None,
                }
            } else {
                None
            };
        let (status, synchronization) = {
            let Some(lifecycle) = session.runtime.lifecycle.as_mut() else {
                return -1;
            };
            lifecycle.tick(received_at_ms);
            let failed = lifecycle.failure().is_some();
            let synchronization = lifecycle.take_synchronization_change().filter(|_| !failed);
            (ios_lifecycle_status(lifecycle), synchronization)
        };
        if let Some(synchronized) = synchronization {
            session
                .runtime
                .transport
                .synchronization_completed(received_at_ms, synchronized);
        }
        write_session_json(
            session,
            json!({
                "messageType": frame.message_type,
                "payload": frame.payload,
                "reportCount": frame.report_count,
                "observation": ticket,
                "states": states,
                "tempoClock": tempo_clock,
                "backupProgress": backup_progress,
                "lifecycle": status,
            }),
            output,
            output_capacity,
        )
    })
}

/// Selects the next reset-comms handshake write as JSON, or writes `null` when none is due.
///
/// The result contains the session ID and pre-framed 129-byte reports so the
/// host can transmit them without reimplementing QC framing.
///
/// # Safety
/// `session` must be a live handle and output must be writable for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_next_handshake(
    session: *mut QcIosSession,
    now_ms: u64,
    session_id: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if session_id.is_null() {
            return -1;
        }
        let Ok(id) = std::ffi::CStr::from_ptr(session_id).to_str() else {
            return -1;
        };
        let Some(write) = session.runtime.next_handshake_write(
            now_ms,
            id.to_owned(),
            Some(ReportLayout::ReportIdPrefixed),
        ) else {
            return write_session_json(session, Value::Null, output, capacity);
        };
        let reports: Vec<Vec<u8>> =
            framing::encode_message(write.message.message_type.into(), &write.message.payload)
                .into_iter()
                .map(|report| report.to_vec())
                .collect();
        write_session_json(
            session,
            json!({
                "attempt": write.attempt,
                "requestId": write.request_id(),
                "sessionId": write.session_id(),
                "reports": reports,
            }),
            output,
            capacity,
        )
    })
}

/// Returns the next queued startup batch as JSON with pre-framed HID reports.
/// The host must call `qc_ios_lifecycle_dispatched` after its ordered writer
/// accepts or rejects the batch.
///
/// # Safety
/// `session` must be live and output must be writable for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_next_lifecycle_batch(
    session: *mut QcIosSession,
    now_ms: u64,
    unix_time_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Some(lifecycle) = session.runtime.lifecycle.as_mut() else {
            return write_session_json(session, Value::Null, output, capacity);
        };
        let Some(batch) = lifecycle.next_batch(now_ms, unix_time_ms) else {
            lifecycle.tick(now_ms);
            return write_session_json(session, Value::Null, output, capacity);
        };
        let messages: Vec<Value> = batch
            .messages
            .into_iter()
            .map(|message| {
                let reports: Vec<Vec<u8>> = framing::encode(message.message_type, &message.payload)
                    .into_iter()
                    .map(|report| report.to_vec())
                    .collect();
                json!({ "messageType": message.message_type, "reports": reports })
            })
            .collect();
        write_session_json(
            session,
            json!({ "ticket": batch.ticket, "messages": messages }),
            output,
            capacity,
        )
    })
}

/// Completes one queued startup batch after the host ordered writer returns.
/// Returns the serialized lifecycle status; stale tickets are safely ignored.
///
/// # Safety
/// `session` is live, `ticket_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_lifecycle_dispatched(
    session: *mut QcIosSession,
    ticket_json: *const c_char,
    accepted: bool,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if ticket_json.is_null() {
            return -1;
        }
        let Ok(ticket_text) = std::ffi::CStr::from_ptr(ticket_json).to_str() else {
            return -1;
        };
        let Ok(ticket) =
            serde_json::from_str::<quadcortex::device::session::LifecycleTicket>(ticket_text)
        else {
            return -1;
        };
        let Some(lifecycle) = session.runtime.lifecycle.as_mut() else {
            return -1;
        };
        if lifecycle.is_current(&ticket) {
            lifecycle.dispatched(
                &ticket,
                if accepted {
                    DispatchOutcome::Accepted
                } else {
                    DispatchOutcome::Failed
                },
            );
        }
        lifecycle.tick(now_ms);
        let failed = lifecycle.failure().is_some();
        let synchronization = lifecycle.take_synchronization_change().filter(|_| !failed);
        let status = ios_lifecycle_status(lifecycle);
        if let Some(synchronized) = synchronization {
            session
                .runtime
                .transport
                .synchronization_completed(now_ms, synchronized);
        }
        write_session_json(session, status, output, capacity)
    })
}

/// Plans one direct-device gateway write against the session's current decoded
/// state and returns the same guarded write contract used by the Android host.
/// The returned dispatch ticket must be used to create delivery receipts before
/// any report is sent; a plan alone is not authorization to write.
///
/// The JSON result contains `messages`, pacing, verification policy, and the
/// serialized native dispatch ticket. Preset recall/navigation/reload use the
/// shared preset-recall planner; all other writes use the complete shared state
/// planner, including expected-state guards.
///
/// # Safety
/// `session` is live, strings are NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_plan_gateway_write(
    session: *mut QcIosSession,
    method: *const c_char,
    params_json: *const c_char,
    started_at_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if method.is_null() || params_json.is_null() {
            return -1;
        }
        let Ok(method) = std::ffi::CStr::from_ptr(method).to_str() else {
            return -1;
        };
        let Ok(params_text) = std::ffi::CStr::from_ptr(params_json).to_str() else {
            return -1;
        };
        let Ok(params) = serde_json::from_str::<Value>(params_text) else {
            return -1;
        };

        let (detail, verification, write, no_op) = if matches!(
            method,
            "device.recallPreset" | "device.navigateBank" | "device.reloadPreset"
        ) {
            let Ok(plan) = plan_preset_recall(method, &params, session.runtime.current_preset())
            else {
                return -1;
            };
            (
                plan.detail.clone(),
                plan.verification(),
                PlannedWrite::HidCommand(plan.command),
                false,
            )
        } else {
            let Ok(plan) = plan_gateway_write_with_state(method, &params, &session.runtime.state)
            else {
                return -1;
            };
            let no_op = plan.is_noop();
            (plan.detail, plan.verification, plan.write, no_op)
        };

        let interval_ms = write.inter_message_interval_ms();
        let messages = match &write {
            PlannedWrite::HidCommand(command) => match command.clone().try_encode() {
                Ok(message) => vec![message],
                Err(_) => return -1,
            },
            PlannedWrite::HidOperation(operation) => match operation.clone().try_encode() {
                Ok(messages) => messages,
                Err(_) => return -1,
            },
            PlannedWrite::MidiControlChange { .. } => Vec::new(),
        };
        let (lane, midi_controller, midi_value) = match &write {
            PlannedWrite::MidiControlChange { controller, value } => ("midi", *controller, *value),
            _ => ("hid", 0, 0),
        };
        let messages: Vec<Value> = messages
            .into_iter()
            .map(|message| {
                json!({
                    "messageType": message.message_type,
                    "payload": message.payload,
                })
            })
            .collect();
        let policy = gateway_write_verification_policy(method);
        let catalog_policy = (method == "device.deleteSetlist")
            .then(CatalogVerificationPolicy::persistent_file_mutation);
        let observed_state_kinds = verification.observed_state_kinds();
        let mut result = json!({
            "method": method,
            "startedAtMs": started_at_ms,
            "detail": detail,
            "verification": verification,
            "observedStateKinds": observed_state_kinds,
            "lane": lane,
            "controller": midi_controller,
            "value": midi_value,
            "messages": messages,
            "interMessageIntervalMs": interval_ms,
            "realtime": gateway_write_is_realtime(method),
            "confirmationTimeoutMs": policy.timeout_ms,
            "preflightMethod": policy.preflight_method,
            "preflightObservationSource": policy.preflight_observation_source,
            "readbackMethod": policy.readback_method,
            "catalogPolicy": catalog_policy,
            "noOp": no_op,
        });
        if !no_op && !matches!(&write, PlannedWrite::MidiControlChange { .. }) {
            let Ok(context) =
                session
                    .runtime
                    .context_for_gateway_write(method, &params, &write, started_at_ms)
            else {
                return -1;
            };
            let Ok(timeout_ms) = gateway_write_budget_ms(method, &write) else {
                return -1;
            };
            let Ok(operation) =
                session
                    .runtime
                    .plan_operation_in_context(&context, started_at_ms, timeout_ms)
            else {
                return -1;
            };
            let Ok(dispatch) =
                session
                    .runtime
                    .prepare_dispatch(Some(&operation), None, started_at_ms)
            else {
                return -1;
            };
            result["verificationContext"] = match serde_json::to_value(&context) {
                Ok(value) => value,
                Err(_) => return -1,
            };
            result["operationTicket"] = match serde_json::to_value(operation) {
                Ok(value) => value,
                Err(_) => return -1,
            };
            result["dispatchTicket"] = match serde_json::to_value(dispatch) {
                Ok(value) => value,
                Err(_) => return -1,
            };
        }
        write_session_json(session, result, output, capacity)
    })
}

/// Starts shared post-write verification for a session-bound write context.
///
/// # Safety
/// `session` and string pointers are live and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_begin_gateway_verification(
    session: *mut QcIosSession,
    context_json: *const c_char,
    verification_json: *const c_char,
    method: *const c_char,
    after_sequence: u64,
    started_at_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if context_json.is_null() || verification_json.is_null() || method.is_null() {
            return -1;
        }
        let (Ok(context_text), Ok(verification_text), Ok(method)) = (
            std::ffi::CStr::from_ptr(context_json).to_str(),
            std::ffi::CStr::from_ptr(verification_json).to_str(),
            std::ffi::CStr::from_ptr(method).to_str(),
        ) else {
            return -1;
        };
        let (Ok(context), Ok(verification)) = (
            serde_json::from_str::<DispatchContext>(context_text),
            serde_json::from_str::<GatewayVerification>(verification_text),
        ) else {
            return -1;
        };
        let policy = gateway_write_verification_policy(method);
        let runtime = GatewayVerificationRuntime::new(
            verification,
            u128::from(after_sequence),
            started_at_ms,
            &policy,
        );
        let Ok(id) = session
            .runtime
            .begin_gateway_verification(&context, runtime)
        else {
            return -1;
        };
        write_session_json(session, json!({"verificationId": id}), output, capacity)
    })
}

/// Advances the shared verifier against the current decoded state snapshot.
///
/// # Safety
/// `session` and string pointers are live and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_advance_gateway_verification(
    session: *mut QcIosSession,
    verification_id: u64,
    observation_sequence: u64,
    now_ms: u64,
    method: *const c_char,
    verification_json: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if method.is_null() || verification_json.is_null() || verification_id == 0 {
            return -1;
        }
        let (Ok(method), Ok(verification_text)) = (
            std::ffi::CStr::from_ptr(method).to_str(),
            std::ffi::CStr::from_ptr(verification_json).to_str(),
        ) else {
            return -1;
        };
        let verification: GatewayVerification = match serde_json::from_str(verification_text) {
            Ok(value) => value,
            Err(_) => return -1,
        };
        let Ok((action, _context, _remaining)) = session.runtime.advance_gateway_verification(
            verification_id,
            u128::from(observation_sequence),
            now_ms,
        ) else {
            return -1;
        };
        let value = match action {
            GatewayVerificationAction::Wait { delay_ms } => {
                json!({"kind":"wait", "delayMs":delay_ms})
            }
            GatewayVerificationAction::TimedOut => json!({"kind":"timedOut"}),
            GatewayVerificationAction::Verified => {
                let snapshot = session.runtime.state.snapshot();
                let plugin_readback = quadcortex::device::request::verified_plugin_block_readback(
                    method,
                    &verification,
                    snapshot,
                )
                .and_then(|value| serde_json::to_value(value).ok());
                let preset_outcome = if matches!(
                    method,
                    "device.recallPreset" | "device.navigateBank" | "device.reloadPreset"
                ) {
                    quadcortex::device::request::preset_address_observed_result(
                        &verification,
                        snapshot,
                    )
                    .and_then(|value| serde_json::to_value(value).ok())
                } else {
                    None
                };
                let mut result = json!({"kind":"verified"});
                if let Some(value) = plugin_readback {
                    result["pluginBlockReadback"] = value;
                }
                if let Some(value) = preset_outcome {
                    result["outcome"] = value;
                }
                result
            }
        };
        write_session_json(session, value, output, capacity)
    })
}

/// Starts the shared guarded preset/setlist preparation workflow.
///
/// A direct device backup is an uncorrelated, one-shot export. Its operation
/// ticket and assembler remain pinned to this session until explicit recovery.
/// # Safety
/// Session is live and name/output follow the C ABI buffer contract.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_begin_backup(
    session: *mut QcIosSession,
    name: *const c_char,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if name.is_null() || session.backup_operation.is_some() {
            return -1;
        }
        let Ok(name) = std::ffi::CStr::from_ptr(name).to_str() else {
            return -1;
        };
        let Ok(name) = validate_backup_name(name) else {
            return -1;
        };
        let Ok(mut operation) = session
            .runtime
            .plan_operation(now_ms, quadcortex::profile::BACKUP_TOTAL_TIMEOUT_MS)
        else {
            return -1;
        };
        if session
            .runtime
            .begin_backup(&mut operation, now_ms)
            .is_err()
        {
            return -1;
        }
        let Ok(dispatch) = session
            .runtime
            .prepare_dispatch(Some(&operation), None, now_ms)
        else {
            return -1;
        };
        let message = commands::create_local_backup();
        session.backup_operation = Some(operation.clone());
        session.backup_name = Some(name);
        write_session_json(
            session,
            json!({
                "kind":"write", "operationTicket":operation, "dispatchTicket":dispatch,
                "messages":[{"messageType":message.message_type,"payload":message.payload}],
                "interMessageIntervalMs":0,
            }),
            output,
            capacity,
        )
    })
}

/// Advances the one active backup, returning a keepalive write only when due.
/// # Safety
/// Session is live and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_advance_backup(
    session: *mut QcIosSession,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Some(operation) = session.backup_operation.clone() else {
            return -1;
        };
        let Ok((action, started)) = session.runtime.advance_backup(&operation, now_ms) else {
            return -1;
        };
        let value = match action {
            BackupAction::Wait => json!({"action":"wait", "started":started}),
            BackupAction::Failed(error) => {
                json!({"action":"failed", "error":error, "started":started})
            }
            BackupAction::Keepalive => {
                let Ok(dispatch) = session
                    .runtime
                    .prepare_dispatch(Some(&operation), None, now_ms)
                else {
                    return -1;
                };
                let message = commands::keepalive();
                json!({
                    "action":"write", "started":started,
                    "operationTicket":operation, "dispatchTicket":dispatch,
                    "messages":[{"messageType":message.message_type,"payload":message.payload}],
                    "interMessageIntervalMs":0,
                })
            }
        };
        write_session_json(session, value, output, capacity)
    })
}

/// Cancels the active backup locally; it never replays the export request.
/// # Safety
/// Session is live.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_cancel_backup(session: *mut QcIosSession) {
    protect((), || {
        if let Some(session) = session.as_mut() {
            if let Some(operation) = session.backup_operation.take() {
                session.runtime.cancel_backup(&operation);
            }
            session.backup_name = None;
        }
    })
}

/// Claims the completed backup at the host's serialized recovery boundary.
/// # Safety
/// Session is live.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_finish_backup(session: *mut QcIosSession) -> bool {
    protect(false, || {
        let Some(session) = session.as_mut() else {
            return false;
        };
        let Some(operation) = session.backup_operation.as_ref() else {
            return false;
        };
        if session.runtime.claim_backup_recovery(operation).is_err() {
            return false;
        }
        session.backup_operation = None;
        session.backup_name = None;
        true
    })
}

/// Starts the shared guarded preset/setlist preparation workflow.
/// # Safety
/// Session is live and strings/output follow the C ABI buffer contract.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_begin_preset_workflow(
    session: *mut QcIosSession,
    method: *const c_char,
    params_json: *const c_char,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if method.is_null() || params_json.is_null() {
            return -1;
        }
        let Ok(method) = std::ffi::CStr::from_ptr(method).to_str() else {
            return -1;
        };
        let Ok(params) = std::ffi::CStr::from_ptr(params_json).to_str() else {
            return -1;
        };
        let Ok(params) = serde_json::from_str::<Value>(params) else {
            return -1;
        };
        let Ok(preparation_id) = session
            .runtime
            .begin_preset_preparation(method, params, now_ms)
        else {
            return -1;
        };
        write_session_json(
            session,
            json!({"preparationId":preparation_id}),
            output,
            capacity,
        )
    })
}

/// Advances a retained preset workflow by one owner-defined step.
/// # Safety
/// Session is live and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_advance_preset_workflow(
    session: *mut QcIosSession,
    preparation_id: u64,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Ok(action) = ios_advance_preset_workflow(session, preparation_id, now_ms) else {
            return -1;
        };
        write_session_json(session, action, output, capacity)
    })
}

/// Cancels a retained preset preparation before any later action is issued.
/// # Safety
/// Session is live.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_cancel_preset_workflow(
    session: *mut QcIosSession,
    preparation_id: u64,
) {
    protect((), || {
        if let Some(session) = session.as_mut() {
            session.runtime.cancel_preset_preparation(preparation_id);
        }
    })
}

/// Plans a correlated gateway read using the current connected session and
/// returns encoded QC messages, reply type, timeout, and shared projection.
///
/// # Safety
/// `session` is live, strings are NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_plan_gateway_read(
    session: *mut QcIosSession,
    method: *const c_char,
    params_json: *const c_char,
    started_at_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if method.is_null() || params_json.is_null() {
            return -1;
        }
        let Ok(method) = std::ffi::CStr::from_ptr(method).to_str() else {
            return -1;
        };
        let Ok(params_text) = std::ffi::CStr::from_ptr(params_json).to_str() else {
            return -1;
        };
        let Ok(params) = serde_json::from_str::<Value>(params_text) else {
            return -1;
        };
        let Some(lifecycle) = session
            .runtime
            .lifecycle
            .as_mut()
            .filter(|lifecycle| lifecycle.is_connected())
        else {
            return -1;
        };
        let request_id = lifecycle.reserve_request_id();
        let epoch = lifecycle.epoch();
        let Ok(plan) = plan_gateway_read(method, &params, request_id) else {
            return -1;
        };
        let timeout_ms = plan.timeout_ms;
        let response_type = plan.response_type;
        let projection = plan.projection;
        let Ok(messages) = plan.operation.try_encode() else {
            return -1;
        };
        let messages: Vec<Value> = messages
            .into_iter()
            .map(|message| json!({ "messageType": message.message_type, "payload": message.payload }))
            .collect();
        let result = json!({
            "method": method,
            "startedAtMs": started_at_ms,
            "responseType": response_type,
            "timeoutMs": timeout_ms,
            "projection": projection,
            "followupMethod": gateway_read_followup_method(method),
            "sessionEpoch": epoch,
            "messages": messages,
        });
        write_session_json(session, result, output, capacity)
    })
}

/// Correlates and decodes one response against a shared gateway read plan.
/// Returns a serialized ReadObservation (unrelated, expired, rejected, matched).
///
/// # Safety
/// `projection_json` is NUL-terminated UTF-8; `payload` and output reference
/// their declared lengths. Output may be null to query the required capacity.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_observe_gateway_read(
    projection_json: *const c_char,
    expected_type: u16,
    actual_type: u16,
    payload: *const u8,
    payload_len: usize,
    started_at_ms: u64,
    timeout_ms: u64,
    ingress_first_ms: u64,
    ingress_last_ms: u64,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        if projection_json.is_null() {
            return -1;
        }
        let Ok(projection_text) = std::ffi::CStr::from_ptr(projection_json).to_str() else {
            return -1;
        };
        let Ok(projection) = serde_json::from_str(projection_text) else {
            return -1;
        };
        let Ok(payload) = bytes(payload, payload_len) else {
            return -1;
        };
        let read = GatewayReadRuntime::new(expected_type, projection, started_at_ms, timeout_ms);
        let ingress = IngressTime {
            first_report_at_ms: ingress_first_ms,
            last_report_at_ms: ingress_last_ms,
        };
        let observation = if !read.accepts_ingress(Some(ingress), now_ms) {
            ReadObservation::Unrelated
        } else {
            read.observe_with_clock(actual_type, payload, || now_ms)
        };
        let value = serde_json::to_value(observation).unwrap_or(Value::Null);
        write_json(value, output, capacity)
    })
}

/// Composes the global and preset tempo readbacks into the public gateway
/// shape using the shared Rust projection logic.
///
/// # Safety
/// Both input strings are NUL-terminated UTF-8 JSON objects; output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_compose_global_tempo_settings(
    global_json: *const c_char,
    preset_json: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        if global_json.is_null() || preset_json.is_null() {
            return -1;
        }
        let Ok(global_text) = std::ffi::CStr::from_ptr(global_json).to_str() else {
            return -1;
        };
        let Ok(preset_text) = std::ffi::CStr::from_ptr(preset_json).to_str() else {
            return -1;
        };
        let (Ok(global), Ok(preset)) = (
            serde_json::from_str::<Value>(global_text),
            serde_json::from_str::<Value>(preset_text),
        ) else {
            return -1;
        };
        let Ok(composed) =
            quadcortex::device::request::compose_global_tempo_settings(&global, &preset)
        else {
            return -1;
        };
        write_json(composed, output, capacity)
    })
}

/// Registers one encoded QC message before the host submits its reports.
/// Returns the opaque delivery ticket as JSON; the dispatch ticket must be the
/// one returned by `qc_ios_plan_gateway_write`.
///
/// # Safety
/// The session is live, `dispatch_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_prepare_delivery(
    session: *mut QcIosSession,
    dispatch_json: *const c_char,
    message_type: u16,
    report_count: usize,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if dispatch_json.is_null() {
            return -1;
        }
        let Ok(text) = std::ffi::CStr::from_ptr(dispatch_json).to_str() else {
            return -1;
        };
        let Ok(dispatch) = serde_json::from_str::<NativeDispatchTicket>(text) else {
            return -1;
        };
        let Ok(ticket) =
            session
                .runtime
                .prepare_delivery(&dispatch, message_type, report_count, now_ms)
        else {
            return -1;
        };
        write_session_json(session, json!(ticket), output, capacity)
    })
}

/// Marks a prepared report batch as admitted to the host's ordered writer.
///
/// # Safety
/// The session is live, `ticket_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_delivery_admitted(
    session: *mut QcIosSession,
    ticket_json: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Some(ticket) = parse_delivery_ticket(ticket_json) else {
            return -1;
        };
        if session.runtime.delivery_admitted(&ticket).is_err() {
            return -1;
        }
        let Ok(record) = session.runtime.delivery_record(&ticket) else {
            return -1;
        };
        write_session_json(session, json!(record), output, capacity)
    })
}

/// Marks one report as starting immediately before the native USB write.
///
/// # Safety
/// The session is live, `ticket_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_begin_report(
    session: *mut QcIosSession,
    ticket_json: *const c_char,
    index: usize,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Some(ticket) = parse_delivery_ticket(ticket_json) else {
            return -1;
        };
        if session
            .runtime
            .begin_native_report(&ticket, index, now_ms)
            .is_err()
        {
            return -1;
        }
        let Ok(record) = session.runtime.delivery_record(&ticket) else {
            return -1;
        };
        write_session_json(session, json!(record), output, capacity)
    })
}

/// Finishes one report transfer. `outcome` is 0 delivered, 1 expected QC
/// status stall, or 2 transport failure.
///
/// # Safety
/// The session is live, `ticket_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_finish_report(
    session: *mut QcIosSession,
    ticket_json: *const c_char,
    index: usize,
    outcome: u8,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Some(ticket) = parse_delivery_ticket(ticket_json) else {
            return -1;
        };
        let outcome = match outcome {
            0 => ReportOutcome::Delivered,
            1 => ReportOutcome::ExpectedStatusStall,
            2 => ReportOutcome::Failed,
            _ => return -1,
        };
        if session
            .runtime
            .finish_native_report(&ticket, index, outcome, now_ms)
            .is_err()
        {
            return -1;
        }
        let Ok(record) = session.runtime.delivery_record(&ticket) else {
            return -1;
        };
        write_session_json(session, json!(record), output, capacity)
    })
}

/// Stops one pending delivery. `reason`: 0 queue rejected, 1 session changed,
/// 2 admission revoked, 3 transport failure, 4 host stopped.
///
/// # Safety
/// The session is live, `ticket_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_stop_delivery(
    session: *mut QcIosSession,
    ticket_json: *const c_char,
    reason: u8,
    now_ms: u64,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        let Some(ticket) = parse_delivery_ticket(ticket_json) else {
            return -1;
        };
        let reason = match reason {
            0 => DeliveryStop::QueueRejected,
            1 => DeliveryStop::SessionChanged,
            2 => DeliveryStop::AdmissionRevoked,
            3 => DeliveryStop::TransportFailure,
            4 => DeliveryStop::Stopped,
            _ => return -1,
        };
        if session
            .runtime
            .stop_delivery(&ticket, reason, now_ms)
            .is_err()
        {
            return -1;
        }
        let Ok(record) = session.runtime.delivery_record(&ticket) else {
            return -1;
        };
        write_session_json(session, json!(record), output, capacity)
    })
}

/// Reports whether every planned message for an operation has a completed
/// native delivery receipt. This says nothing about device-side confirmation.
///
/// # Safety
/// The session is live, `operation_json` is NUL-terminated UTF-8, and output is writable.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_operation_delivery_complete(
    session: *mut QcIosSession,
    operation_json: *const c_char,
    frame_count: usize,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_mut() else {
            return -1;
        };
        if let Some(result) = flush_session_output(session, output, capacity) {
            return result;
        }
        if operation_json.is_null() {
            return -1;
        }
        let Ok(text) = std::ffi::CStr::from_ptr(operation_json).to_str() else {
            return -1;
        };
        let Ok(operation) = serde_json::from_str::<OperationTicket>(text) else {
            return -1;
        };
        let Ok(complete) = session
            .runtime
            .operation_delivery_complete(&operation, frame_count)
        else {
            return -1;
        };
        write_session_json(session, json!({ "complete": complete }), output, capacity)
    })
}

unsafe fn parse_delivery_ticket(ticket_json: *const c_char) -> Option<DeliveryTicket> {
    if ticket_json.is_null() {
        return None;
    }
    let text = std::ffi::CStr::from_ptr(ticket_json).to_str().ok()?;
    serde_json::from_str(text).ok()
}

/// Encodes one arbitrary QC message into the concatenated 129-byte output reports.
///
/// Returns the report count, or a negative required byte capacity minus one.
///
/// # Safety
/// Input/output pointers must be valid for their declared lengths.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_encode_message(
    message_type: u16,
    payload: *const u8,
    payload_len: usize,
    output: *mut u8,
    output_capacity: usize,
) -> isize {
    protect(-1, || {
        let Ok(payload) = bytes(payload, payload_len) else {
            return -1;
        };
        let reports = framing::encode(message_type, payload);
        let required = reports.len() * framing::REPORT_SIZE;
        if output.is_null() || output_capacity < required {
            return -(required as isize) - 1;
        }
        let target = slice::from_raw_parts_mut(output, required);
        for (index, report) in reports.iter().enumerate() {
            target[index * framing::REPORT_SIZE..(index + 1) * framing::REPORT_SIZE]
                .copy_from_slice(report);
        }
        reports.len() as isize
    })
}

/// Number of bytes in every complete QC HID report.
#[no_mangle]
pub extern "C" fn qc_ios_report_size() -> usize {
    framing::REPORT_SIZE
}

/// Returns a shared-runtime transport phase as JSON for diagnostics.
///
/// # Safety
/// `session` must be live and output must be writable for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_transport_status(
    session: *const QcIosSession,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_ref() else {
            return -1;
        };
        let phase = match session.runtime.transport.phase() {
            quadcortex::device::transport::SessionPhase::Disconnected => "disconnected",
            quadcortex::device::transport::SessionPhase::Searching => "searching",
            quadcortex::device::transport::SessionPhase::Handshaking => "handshaking",
            quadcortex::device::transport::SessionPhase::Syncing => "syncing",
            quadcortex::device::transport::SessionPhase::Ready => "ready",
        };
        write_json(json!({ "phase": phase }), output, capacity)
    })
}

/// Returns the shared session's current QC snapshot, or JSON null before a
/// preset has been observed.
///
/// # Safety
/// `session` must be live and output must be writable for `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_current_preset(
    session: *const QcIosSession,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        let Some(session) = session.as_ref() else {
            return -1;
        };
        write_json(json!(session.runtime.current_preset()), output, capacity)
    })
}

/// Builds a documented low-level QC command and returns its message type and protobuf bytes.
///
/// This intentionally accepts only a bounded action set. Higher level writes
/// must use the shared gateway planner before they are admitted to the device.
/// Actions: `versionHello`, `readScene`, `readTuner`, `showGigView`, `keepalive`.
/// The return value is JSON: `{ "messageType": number, "payload": [byte, ...] }`.
///
/// # Safety
/// `action` is a valid NUL-terminated UTF-8 string; output is writable for capacity.
#[no_mangle]
pub unsafe extern "C" fn qc_ios_build_control(
    action: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    protect(-1, || {
        if action.is_null() {
            return -1;
        }
        let Ok(action) = std::ffi::CStr::from_ptr(action).to_str() else {
            return -1;
        };
        let message = match action {
            "versionHello" => commands::version_hello(),
            "readScene" => commands::read(quadcortex::profile::MESSAGE_TYPE_SCENE),
            "readTuner" => commands::read_tuner(),
            "showGigView" => commands::show_gig_view(true),
            "keepalive" => commands::keepalive(),
            _ => return -1,
        };
        write_json(
            json!({ "messageType": message.message_type, "payload": message.payload }),
            output,
            capacity,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    #[test]
    fn exported_control_builder_returns_allowlisted_qc_message() {
        let action = CString::new("versionHello").unwrap();
        let mut output = vec![0_i8; 4096];
        let written =
            unsafe { qc_ios_build_control(action.as_ptr(), output.as_mut_ptr(), output.len()) };

        assert!(written > 0);
        let json = unsafe { CStr::from_ptr(output.as_ptr()) }.to_str().unwrap();
        let value: Value = serde_json::from_str(json).unwrap();
        assert!(value["messageType"].as_u64().is_some());
        assert!(value["payload"]
            .as_array()
            .is_some_and(|payload| !payload.is_empty()));

        let unsupported = CString::new("rawUsbWrite").unwrap();
        assert_eq!(
            unsafe {
                qc_ios_build_control(unsupported.as_ptr(), output.as_mut_ptr(), output.len())
            },
            -1
        );
    }

    #[test]
    fn exported_encoder_reports_required_capacity_and_emits_complete_hid_report() {
        let payload = b"ios-ffi-test";
        let required = unsafe {
            qc_ios_encode_message(1, payload.as_ptr(), payload.len(), ptr::null_mut(), 0)
        };
        assert!(required < -1);

        let mut reports = vec![0_u8; (-required - 1) as usize];
        let written = unsafe {
            qc_ios_encode_message(
                1,
                payload.as_ptr(),
                payload.len(),
                reports.as_mut_ptr(),
                reports.len(),
            )
        };
        assert_eq!(written, 1);
        assert_eq!(reports.len(), qc_ios_report_size());
        assert_eq!(reports[0], quadcortex::framing::OUT_REPORT_ID);
    }

    #[test]
    fn session_exports_connection_and_report_epoch() {
        let session = qc_ios_session_create();
        assert!(!session.is_null());

        let connection = unsafe { qc_ios_session_opened(session, 100) };
        assert_ne!(connection, 0);
        assert_eq!(unsafe { qc_ios_report_epoch(session, connection) }, 0);

        let session_id = CString::new("ios-ffi-test-session").unwrap();
        let mut handshake_output = vec![0_i8; 4096];
        let handshake_bytes = unsafe {
            qc_ios_next_handshake(
                session,
                101,
                session_id.as_ptr(),
                handshake_output.as_mut_ptr(),
                handshake_output.len(),
            )
        };
        assert!(handshake_bytes > 0);
        let handshake_json = unsafe { CStr::from_ptr(handshake_output.as_ptr()) }
            .to_str()
            .unwrap();
        let handshake: Value = serde_json::from_str(handshake_json).unwrap();
        assert!(handshake["reports"]
            .as_array()
            .is_some_and(|reports| !reports.is_empty()));

        let epoch = unsafe { qc_ios_report_epoch(session, connection) };
        assert_ne!(epoch, 0);
        let stale_report = vec![0_u8; qc_ios_report_size()];
        let mut report_output = vec![0_i8; 256];
        assert_eq!(
            unsafe {
                qc_ios_push_report(
                    session,
                    connection,
                    0,
                    stale_report.as_ptr(),
                    stale_report.len(),
                    102,
                    report_output.as_mut_ptr(),
                    report_output.len(),
                )
            },
            0,
            "an inactive reader epoch must be ignored without killing the reader"
        );

        let mut output = vec![0_i8; 256];
        let written =
            unsafe { qc_ios_transport_status(session, output.as_mut_ptr(), output.len()) };
        assert!(written > 0);
        let json = unsafe { CStr::from_ptr(output.as_ptr()) }.to_str().unwrap();
        let status: Value = serde_json::from_str(json).unwrap();
        assert!(status["phase"].as_str().is_some());

        unsafe { qc_ios_session_destroy(session) };
    }

    #[test]
    fn ios_driver_readiness_uses_the_shared_connected_phase_contract() {
        use quadcortex::device::initialization::DeviceStartupPhase;

        // qc_ios_push_report exports this enum's `as_str()` value as
        // lifecycle.phase; keep it aligned with the Swift DriverKit gate.
        assert_eq!(DeviceStartupPhase::Connected.as_str(), "connected");
        assert_ne!(DeviceStartupPhase::Invalid.as_str(), "connected");
        assert_ne!(DeviceStartupPhase::Failed.as_str(), "connected");

        let lifecycle = SessionDriver::start(1, "ios-readiness-contract".into(), 0);
        let status = ios_lifecycle_status(&lifecycle);
        assert_eq!(status["synchronized"], false);
        assert_eq!(status["phase"], "sessionValidating");
    }
}
