//! A dedicated authenticated reverse connection. It never holds the app's
//! Gateway mutex or claims USB. Only an explicit Demo click may start a broker;
//! provider recovery never replays a device operation.
use crate::{broker_attach, device_broker_descriptor, DEVICE_BROKER_DESCRIPTOR_PATH};
use serde_json::{json, Value};
use std::net::{Shutdown, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

const MAX_FRAME: usize = quadcortex::domain::IPC_MAX_FRAME_BYTES;
const REQUEST_EVENT: &str = "qc-demo-provider-request";
const STATUS_EVENT: &str = "qc-demo-provider-status";
static PROVIDER_THREADS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(8);

fn explicit_demo_registration(registration: &Value) -> bool {
    registration
        .get("explicitSelection")
        .and_then(Value::as_bool)
        == Some(true)
        && registration
            .get("target")
            .and_then(|target| target.get("source"))
            .and_then(Value::as_str)
            == Some("demo")
}

enum Command {
    Admit {
        target: Value,
        sequence: u64,
        answer: mpsc::SyncSender<Result<Value, String>>,
    },
    Reply {
        target: Value,
        sequence: u64,
        response: Value,
    },
}

struct Worker {
    provider_id: String,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    pending: Arc<Mutex<Option<(Value, u64)>>>,
    commands: mpsc::SyncSender<Command>,
    original: Option<qc_native_platform::thread_owner::Owned>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(socket) = self.socket.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let Some(original) = self.original.as_mut() {
            // Owned retains the exact native original and its reservation if
            // TLS teardown cannot acknowledge within this bounded wait.
            let _ = original.wait_within(Duration::from_secs(2));
        }
    }
}

#[derive(Clone)]
struct SelectionLease {
    instance: String,
    token: String,
    operation: u64,
    descriptor: device_broker_descriptor::DeviceBrokerDescriptor,
    acknowledged: bool,
}

#[allow(
    deprecated,
    reason = "Keep compatibility with stable toolchains predating Atomic::try_update"
)]
fn next_operation(generation: &AtomicU64) -> Result<u64, String> {
    generation
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            value.checked_add(1)
        })
        .map(|value| value + 1)
        .map_err(|_| "Demo operation generation exhausted".into())
}

fn prepare_demo_start(
    user_action: bool,
    current: impl Fn() -> bool,
    start: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !current() {
        return Err("Demo selection superseded".into());
    }
    if user_action {
        start()?;
    }
    if !current() {
        return Err("Demo selection superseded".into());
    }
    Ok(())
}

fn record_selection_ack(
    selection: &Mutex<Option<SelectionLease>>,
    lease: SelectionLease,
    current: impl FnOnce() -> bool,
) -> bool {
    let mut held = selection.lock().unwrap_or_else(|p| p.into_inner());
    if held
        .as_ref()
        .is_some_and(|held| held.operation > lease.operation)
    {
        return false;
    }
    let current = current();
    *held = Some(lease);
    current
}

fn take_matching_provider(
    workers: &Mutex<Option<Worker>>,
    expected: Option<&str>,
) -> Option<Worker> {
    let mut current = workers.lock().unwrap_or_else(|p| p.into_inner());
    if current.as_ref().map(|worker| worker.provider_id.as_str()) == expected {
        current.take()
    } else {
        None
    }
}

#[derive(Default)]
pub(crate) struct DemoProviderBridge(
    Arc<Mutex<Option<Worker>>>,
    Arc<Mutex<Option<SelectionLease>>>,
    Arc<AtomicU64>,
    Mutex<()>,
);

impl DemoProviderBridge {
    fn replace(
        &self,
        operation: u64,
        create: impl FnOnce() -> Result<Worker, String>,
    ) -> Result<(), String> {
        // Reserve the slot before retiring/spawning. A concurrent start/stop
        // cannot create a worker that is absent from the ownership table.
        let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if self.2.load(Ordering::Acquire) != operation {
            return Err("Demo selection superseded".into());
        }
        let old = slot.take();
        drop(slot);
        drop(old);
        // Serialize only the local spawn factory, never a join or network call.
        let creating = self.3.lock().unwrap_or_else(|p| p.into_inner());
        if self.2.load(Ordering::Acquire) != operation {
            return Err("Demo selection superseded".into());
        }
        let worker = create()?;
        drop(creating);
        let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if self.2.load(Ordering::Acquire) != operation {
            drop(slot);
            drop(worker);
            return Err("Demo selection superseded".into());
        }
        *slot = Some(worker);
        Ok(())
    }

    pub(crate) fn stop(&self) {
        {
            let _selection = self.1.lock().unwrap_or_else(|p| p.into_inner());
            let _ = next_operation(&self.2);
        }
        let old = self.0.lock().unwrap_or_else(|p| p.into_inner()).take();
        drop(old);
    }
}

fn main_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Only the main editor can own its demo provider".into())
    }
}

struct Connection {
    stream: TcpStream,
    next_id: u64,
}

fn clear_observed_demo(
    connection: &mut Connection,
    current: impl Fn() -> bool,
    allow_legacy: bool,
) -> Result<(), String> {
    if !current() {
        return Err("Device selection superseded".into());
    }
    let observed = connection.call("broker.demoSelection", json!({}), Duration::from_secs(3));
    if !current() {
        return Err("Device selection superseded".into());
    }
    let observed = match observed {
        // This exact authenticated protocol rejection is the pre-selection
        // provider surface; network/authentication/parse failures stay errors.
        Err(message)
            if allow_legacy
                && message == "This method is not available to a demoProvider connection" =>
        {
            return Ok(())
        }
        other => other?,
    };
    let revision = observed
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or("Missing Demo selection revision")?;
    if !current() {
        return Err("Device selection superseded".into());
    }
    let result = connection.call(
        "broker.demoClearSelection",
        json!({"expectedRevision":revision}),
        Duration::from_secs(3),
    )?;
    if !current() {
        return Err("Device selection superseded".into());
    }
    if result.get("cleared").and_then(Value::as_bool) != Some(true) {
        return Err("Demo selection changed; select the device again".into());
    }
    Ok(())
}

impl Connection {
    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Demo provider sequence exhausted")?;
        let payload = serde_json::to_vec(
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )
        .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        qc_broker_framing::write_tcp_frame_before_deadline(
            &mut self.stream,
            &payload,
            MAX_FRAME,
            deadline,
        )
        .map_err(|e| e.to_string())?;
        let bytes = qc_broker_framing::read_tcp_frame_before_deadline(
            &mut self.stream,
            MAX_FRAME,
            deadline,
        )
        .map_err(|e| e.to_string())?;
        let reply: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if reply.get("id").and_then(Value::as_u64) != Some(id) {
            return Err("Unsolicited demo provider response".into());
        }
        if reply.get("error").is_some() {
            return Err(reply["error"]["message"]
                .as_str()
                .unwrap_or("The broker refused the demo request")
                .into());
        }
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| "Malformed demo provider response".into())
    }
}

struct ProviderStart<'a> {
    selection: &'a Mutex<Option<SelectionLease>>,
    user_action: bool,
    operation: u64,
    generation: &'a AtomicU64,
}

fn registration_params(
    provider_id: &str,
    user_action: bool,
    previous: Option<&SelectionLease>,
    instance: &str,
    revision: u64,
) -> Result<Value, String> {
    let mut params = json!({"sessionId":provider_id,"explicitSelection":true});
    if user_action {
        params["recoverRevision"] = json!(revision);
    } else {
        let previous = previous
            .filter(|lease| lease.acknowledged && lease.instance == instance)
            .ok_or("Select Demo explicitly to recover its broker selection")?;
        params["selectionToken"] = json!(previous.token);
    }
    Ok(params)
}

fn retire_local_selection(selection: &Mutex<Option<SelectionLease>>, lease: &SelectionLease) {
    let mut current = selection.lock().unwrap_or_else(|p| p.into_inner());
    if current.as_ref().is_some_and(|v| {
        v.operation == lease.operation && v.token == lease.token && v.instance == lease.instance
    }) {
        current.take();
    }
}

fn run(
    app: &AppHandle,
    provider_id: &str,
    stop: &AtomicBool,
    socket: &Mutex<Option<TcpStream>>,
    pending: &Mutex<Option<(Value, u64)>>,
    commands: mpsc::Receiver<Command>,
    start: ProviderStart<'_>,
) -> Result<(), String> {
    let ProviderStart {
        selection,
        user_action,
        operation,
        generation,
    } = start;
    let path = DEVICE_BROKER_DESCRIPTOR_PATH
        .get()
        .ok_or("The shared broker directory is unavailable")?;
    prepare_demo_start(
        user_action,
        || !stop.load(Ordering::Acquire) && generation.load(Ordering::Acquire) == operation,
        || {
            // Election/ownership cleanup is the same supported path as Connect.
            // Dropping this temporary attachment never kills an adopted broker.
            crate::acknowledge_user_connect_in(path)?;
            let attachment = crate::attach_or_host_device_broker(path, false)?;
            drop(attachment);
            Ok(())
        },
    )?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as u64;
    let descriptor = device_broker_descriptor::read_descriptor(path, now)
        .ok_or("Start the shared broker to expose the demo to MCP")?;
    // Reuse exact-process/kernel-listener checks before transmitting the token.
    let stream = broker_attach::attach_demo_provider(&descriptor)
        .map_err(|_| "The shared broker could not authenticate a demo provider")?;
    *socket.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(stream.try_clone().map_err(|e| e.to_string())?);
    if stop.load(Ordering::Acquire) {
        return Err("The demo provider stopped".into());
    }
    let mut connection = Connection { stream, next_id: 1 };
    let previous = selection.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let selection_state =
        connection.call("broker.demoSelection", json!({}), Duration::from_secs(3))?;
    let revision = selection_state
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or("The broker does not support recoverable Demo selection; update the broker")?;
    let mut registration_params = registration_params(
        provider_id,
        user_action,
        previous.as_ref(),
        &descriptor.instance_id,
        revision,
    )?;
    // Record the attempted broker BEFORE any registration can commit. A lost
    // acknowledgement must not look like "nothing was selected" to Connect.
    {
        let mut held = selection.lock().unwrap_or_else(|p| p.into_inner());
        if stop.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != operation {
            return Err("Demo selection superseded".into());
        }
        *held = Some(SelectionLease {
            instance: descriptor.instance_id.clone(),
            descriptor: descriptor.clone(),
            token: String::new(),
            operation,
            acknowledged: false,
        });
    }
    // Closing the old socket and joining its native client does not mean the
    // broker's reader has already retired its ownership row. Retry ONLY this
    // lifecycle registration, within one finite local deadline; never replay
    // a device request and never replace another live owner's registration.
    let registration_deadline = Instant::now() + Duration::from_secs(3);
    let registration = loop {
        if stop.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != operation {
            return Err("The demo provider stopped".into());
        }
        let remaining = registration_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("A live UI already owns the demo provider".into());
        }
        match connection.call(
            "broker.demoRegister",
            // Older brokers reject this additive field rather than silently
            // serving physical MCP while the editor displays Demo.
            registration_params.clone(),
            remaining,
        ) {
            Ok(value) => break value,
            Err(message) if message == "A live UI already owns the demo provider" => {
                std::thread::sleep(Duration::from_millis(20).min(remaining));
            }
            Err(message)
                if message == "Demo selection changed; select Demo again"
                    && generation.load(Ordering::Acquire) == operation =>
            {
                let held = selection.lock().unwrap_or_else(|p| p.into_inner()).clone();
                if let Some(held) = held.filter(|held| {
                    held.instance == descriptor.instance_id && held.operation < operation
                }) {
                    registration_params
                        .as_object_mut()
                        .unwrap()
                        .remove("recoverRevision");
                    registration_params["selectionToken"] = json!(held.token);
                } else {
                    return Err(message);
                }
            }
            Err(message) => return Err(message),
        }
    };
    if !explicit_demo_registration(&registration) {
        return Err("The broker did not acknowledge the editor's explicit Demo selection".into());
    }
    let token = registration
        .get("selectionToken")
        .and_then(Value::as_str)
        .ok_or("The broker did not return a Demo selection lease")?
        .to_owned();
    // Even a late ack records its exact lease for causal physical cleanup,
    // but never clears intent or overwrites a newer acknowledged operation.
    if !record_selection_ack(
        selection,
        SelectionLease {
            instance: descriptor.instance_id.clone(),
            token,
            operation,
            descriptor: descriptor.clone(),
            acknowledged: true,
        },
        || !stop.load(Ordering::Acquire) && generation.load(Ordering::Acquire) == operation,
    ) {
        return Err("The demo selection was cancelled".into());
    }
    app.emit_to(
        "main",
        STATUS_EVENT,
        json!({"providerId":provider_id,"ready":true,"brokerInstanceId":descriptor.instance_id}),
    )
    .map_err(|e| e.to_string())?;
    while !stop.load(Ordering::Acquire) {
        let request = connection.call("broker.demoNext", json!({}), Duration::from_secs(25))?;
        if request.is_null() {
            continue;
        }
        let target = request
            .get("target")
            .cloned()
            .ok_or("A demo request has no captured target")?;
        let sequence = request["sequence"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("Invalid demo request sequence")?;
        *pending.lock().unwrap_or_else(|p| p.into_inner()) = Some((target.clone(), sequence));
        app.emit_to(
            "main",
            REQUEST_EVENT,
            json!({"providerId":provider_id,"request":request}),
        )
        .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let response = loop {
            if stop.load(Ordering::Acquire) {
                return Err("The demo provider stopped".into());
            }
            let command = commands
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| "The demo editor did not finish its request before the deadline")?;
            match command {
                Command::Admit {
                    target: named,
                    sequence: named_sequence,
                    answer,
                } => {
                    let result = if named == target && named_sequence == sequence {
                        connection.call(
                            "broker.demoAdmit",
                            json!({"target":target,"sequence":sequence}),
                            Duration::from_secs(5),
                        )
                    } else {
                        Err("The demo request was superseded".into())
                    };
                    let result = result.and_then(|value| {
                        if value.get("admitted").and_then(Value::as_bool) == Some(true) {
                            Ok(value)
                        } else {
                            Err("The demo request was superseded before effect admission".into())
                        }
                    });
                    let _ = answer.try_send(result);
                }
                Command::Reply {
                    target: named,
                    sequence: named_sequence,
                    response,
                } => {
                    if named != target || named_sequence != sequence {
                        return Err("Foreign or late demo response".into());
                    }
                    break response;
                }
            }
        };
        pending.lock().unwrap_or_else(|p| p.into_inner()).take();
        let mut reply = response
            .as_object()
            .cloned()
            .ok_or("Malformed demo reply")?;
        if reply.contains_key("result") == reply.contains_key("error") || reply.len() != 1 {
            return Err("A demo reply must contain exactly one result or error".into());
        }
        reply.insert("target".into(), target);
        reply.insert("sequence".into(), json!(sequence));
        connection.call(
            "broker.demoReply",
            Value::Object(reply),
            Duration::from_secs(5),
        )?;
    }
    // A stopped/replaced transport does NOT release selection intent. Only the
    // explicit clear command below may do so with its exact lease.
    Ok(())
}

#[tauri::command]
pub(crate) fn start_demo_provider(
    app: AppHandle,
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
    session_id: String,
    user_action: Option<bool>,
) -> Result<(), String> {
    main_window(&window)?;
    if session_id.is_empty() || session_id.len() > 128 {
        return Err("Invalid demo provider session".into());
    }
    let operation = {
        let _selection = bridge.1.lock().unwrap_or_else(|p| p.into_inner());
        next_operation(&bridge.2)?
    };
    bridge.replace(operation, || {
        let stop = Arc::new(AtomicBool::new(false));
        let socket = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(None));
        let (send, receive) = mpsc::sync_channel(8);
        let worker_id = session_id.clone();
        let worker_stop = stop.clone();
        let worker_socket = socket.clone();
        let worker_pending = pending.clone();
        let selection = bridge.1.clone();
        let generation = bridge.2.clone();
        let original = qc_native_platform::thread_owner::Owned::spawn(
            "qc-demo-provider",
            &PROVIDER_THREADS,
            (stop.clone(), socket.clone(), pending.clone()),
            move || {
                let result = run(
                    &app,
                    &session_id,
                    &stop,
                    &socket,
                    &pending,
                    receive,
                    ProviderStart {
                        selection: &selection,
                        user_action: user_action.unwrap_or(false),
                        operation,
                        generation: &generation,
                    },
                );
                pending.lock().unwrap_or_else(|p| p.into_inner()).take();
                socket.lock().unwrap_or_else(|p| p.into_inner()).take();
                let _ = app.emit_to(
                    "main",
                    STATUS_EVENT,
                    json!({"providerId":session_id,"ready":false,"message":result.err()}),
                );
            },
        )?;
        Ok(Worker {
            provider_id: worker_id,
            stop: worker_stop,
            socket: worker_socket,
            pending: worker_pending,
            commands: send,
            original: Some(original),
        })
    })
}

fn sender(
    bridge: &DemoProviderBridge,
    provider_id: &str,
    target: &Value,
    sequence: u64,
) -> Result<mpsc::SyncSender<Command>, String> {
    let current = bridge.0.lock().unwrap_or_else(|p| p.into_inner());
    let worker = current
        .as_ref()
        .filter(|w| w.provider_id == provider_id)
        .ok_or("The demo provider no longer owns this registration")?;
    if worker
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        != Some(&(target.clone(), sequence))
    {
        return Err("The demo request is no longer pending".into());
    }
    Ok(worker.commands.clone())
}

#[tauri::command]
pub(crate) async fn admit_demo_provider_request(
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
    provider_id: String,
    target: Value,
    sequence: u64,
) -> Result<Value, String> {
    main_window(&window)?;
    let sender = sender(&bridge, &provider_id, &target, sequence)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (answer, receive) = mpsc::sync_channel(1);
        sender
            .try_send(Command::Admit {
                target,
                sequence,
                answer,
            })
            .map_err(|_| "The demo provider queue is unavailable")?;
        receive
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| "Demo admission expired")?
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) fn complete_demo_provider_request(
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
    provider_id: String,
    target: Value,
    sequence: u64,
    response: Value,
) -> Result<(), String> {
    main_window(&window)?;
    let sender = sender(&bridge, &provider_id, &target, sequence)?;
    sender
        .try_send(Command::Reply {
            target,
            sequence,
            response,
        })
        .map_err(|_| "The demo provider queue is unavailable".into())
}

#[tauri::command]
pub(crate) async fn clear_demo_selection(
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
) -> Result<(), String> {
    main_window(&window)?;
    let selection = bridge.1.clone();
    let workers = bridge.0.clone();
    let (lease, expected_worker, clear_operation) = {
        let held = selection.lock().unwrap_or_else(|p| p.into_inner());
        let current = workers.lock().unwrap_or_else(|p| p.into_inner());
        (
            held.clone(),
            current.as_ref().map(|worker| worker.provider_id.clone()),
            next_operation(&bridge.2)?,
        )
    };
    let generation = bridge.2.clone();
    // Retire the provider first; late replies cannot mutate the chosen target.
    tauri::async_runtime::spawn_blocking(move || {
        let old = take_matching_provider(&workers, expected_worker.as_deref());
        if let Some(mut old) = old {
            old.stop.store(true, Ordering::Release);
            if let Some(socket) = old.socket.lock().unwrap_or_else(|p| p.into_inner()).take() {
                let _ = socket.shutdown(Shutdown::Both);
            }
            if let Some(original) = old.original.as_mut() {
                if !original.wait_within(Duration::from_secs(3)) {
                    return Err("Demo startup is still retiring; choose Connect again".into());
                }
            }
            drop(old);
        }
        if generation.load(Ordering::Acquire) != clear_operation {
            return Err("Device selection superseded".into());
        }
        let latest = {
            selection
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .filter(|lease| lease.operation < clear_operation)
        };
        let lease = latest.or(lease);
        let path = DEVICE_BROKER_DESCRIPTOR_PATH
            .get()
            .ok_or("The shared broker directory is unavailable")?;
        let Some(lease) = lease else {
            if let Some(descriptor) = device_broker_descriptor::read_descriptor(
                path,
                device_broker_descriptor::unix_time_ms(),
            ) {
                let stream = broker_attach::attach_demo_provider(&descriptor)
                    .map_err(|_| "Could not authenticate shared selection recovery")?;
                let mut connection = Connection { stream, next_id: 1 };
                clear_observed_demo(
                    &mut connection,
                    || generation.load(Ordering::Acquire) == clear_operation,
                    true,
                )?;
            }
            return Ok(());
        };
        let descriptor = device_broker_descriptor::read_descriptor(
            path,
            device_broker_descriptor::unix_time_ms(),
        );
        if let Some(descriptor) = descriptor {
            let stream = broker_attach::attach_demo_provider(&descriptor)
                .map_err(|_| "Could not authenticate Demo selection withdrawal")?;
            if descriptor.instance_id == lease.instance {
                let mut connection = Connection { stream, next_id: 1 };
                if !lease.acknowledged {
                    clear_observed_demo(
                        &mut connection,
                        || generation.load(Ordering::Acquire) == clear_operation,
                        false,
                    )?;
                } else {
                    let params = json!({"selectionToken":lease.token});
                    if generation.load(Ordering::Acquire) != clear_operation {
                        return Err("Device selection superseded".into());
                    }
                    let result = connection.call(
                        "broker.demoClearSelection",
                        params,
                        Duration::from_secs(3),
                    )?;
                    if result.get("cleared").and_then(Value::as_bool) != Some(true) {
                        return Err("Demo selection changed; select the device again".into());
                    }
                }
            } else {
                let mut connection = Connection { stream, next_id: 1 };
                clear_observed_demo(
                    &mut connection,
                    || generation.load(Ordering::Acquire) == clear_operation,
                    true,
                )?;
            }
        } else if broker_attach::published_broker_process_is_alive(&lease.descriptor) {
            // Missing/stale/unreadable metadata does not prove the old epoch
            // ended. Do not erase Quit or start a broker just to clear a token.
            return Err(
                "The selected Demo broker is temporarily unreachable; retry the selection".into(),
            );
        }
        retire_local_selection(&selection, &lease);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) fn stop_demo_provider(
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
    provider_id: String,
) -> Result<(), String> {
    main_window(&window)?;
    let mut current = bridge.0.lock().unwrap_or_else(|p| p.into_inner());
    let old = if current
        .as_ref()
        .is_some_and(|worker| worker.provider_id == provider_id)
    {
        current.take()
    } else {
        None
    };
    drop(current);
    drop(old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn selection_fixture(operation: u64) -> SelectionLease {
        SelectionLease {
            instance: "test-broker".into(),
            token: format!("test-only-lease-{operation}"),
            operation,
            acknowledged: true,
            descriptor: device_broker_descriptor::DeviceBrokerDescriptor {
                port: 1,
                bearer_token: "test-only-bearer".into(),
                instance_id: "test-broker".into(),
                process_id: 0,
                process_created_at: None,
                published_at_unix_ms: 0,
                gateway_api_version: None,
            },
        }
    }

    #[test]
    fn automatic_recovery_requires_an_acknowledged_same_epoch_lease() {
        let old = selection_fixture(1);
        assert!(registration_params("demo", false, Some(&old), "replacement-broker", 4).is_err());
        let retained = registration_params("demo", false, Some(&old), "test-broker", 4).unwrap();
        assert_eq!(retained["selectionToken"], old.token);
        assert!(retained.get("recoverRevision").is_none());
        let explicit =
            registration_params("demo", true, Some(&old), "replacement-broker", 4).unwrap();
        assert_eq!(explicit["recoverRevision"], 4);
        assert!(explicit.get("selectionToken").is_none());
        let mut pending = old;
        pending.acknowledged = false;
        assert!(registration_params("demo", false, Some(&pending), "test-broker", 4).is_err());
    }

    #[test]
    fn stale_pending_clear_cannot_erase_a_newer_empty_token_lease() {
        let mut old = selection_fixture(1);
        old.token.clear();
        old.acknowledged = false;
        let mut newer = old.clone();
        newer.operation = 2;
        let held = Mutex::new(Some(newer));
        retire_local_selection(&held, &old);
        assert_eq!(held.lock().unwrap().as_ref().unwrap().operation, 2);
    }

    #[test]
    fn explicit_demo_starts_a_missing_broker_but_rebind_and_cancel_never_start_one() {
        use std::cell::Cell;
        let starts = Cell::new(0);
        for (explicit, current) in [(false, true), (true, false)] {
            let result = prepare_demo_start(
                explicit,
                || current,
                || {
                    starts.set(starts.get() + 1);
                    Ok(())
                },
            );
            assert_eq!(result.is_ok(), current);
        }
        assert_eq!(starts.get(), 0);
        prepare_demo_start(
            true,
            || true,
            || {
                starts.set(starts.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(starts.get(), 1);
        // A user cancels while the supported start returns. The shared broker
        // is left alive; only this selection is refused, never an adopted host killed.
        let current = Cell::new(true);
        assert!(prepare_demo_start(
            true,
            || current.get(),
            || {
                current.set(false);
                starts.set(starts.get() + 1);
                Ok(())
            }
        )
        .is_err());
        assert_eq!(starts.get(), 2);
    }

    #[test]
    fn delayed_registration_ack_preserves_unavailable_intent_without_overwriting_newer_demo() {
        let held = Mutex::new(None);
        assert!(!record_selection_ack(&held, selection_fixture(1), || false));
        assert_eq!(held.lock().unwrap().as_ref().unwrap().operation, 1);
        assert!(record_selection_ack(&held, selection_fixture(2), || true));
        assert!(!record_selection_ack(&held, selection_fixture(1), || false));
        assert_eq!(held.lock().unwrap().as_ref().unwrap().operation, 2);
    }

    #[test]
    fn stale_clear_never_retires_a_replacement_provider() {
        let (commands, _) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let workers = Mutex::new(Some(Worker {
            provider_id: "new-provider".into(),
            stop: stop.clone(),
            socket: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(None)),
            commands,
            original: None,
        }));
        assert!(take_matching_provider(&workers, Some("old-provider")).is_none());
        assert!(!stop.load(Ordering::Acquire));
        assert_eq!(
            workers.lock().unwrap().as_ref().unwrap().provider_id,
            "new-provider"
        );
        drop(take_matching_provider(&workers, Some("new-provider")));
        assert!(stop.load(Ordering::Acquire));
    }

    #[test]
    fn fresh_window_clears_retained_intent_by_observed_revision_without_a_local_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let query: Value = serde_json::from_slice(
                &qc_broker_framing::read_frame(&mut socket, MAX_FRAME)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(query["method"], "broker.demoSelection");
            qc_broker_framing::write_frame(
                &mut socket,
                &serde_json::to_vec(
                    &json!({"jsonrpc":"2.0","id":query["id"],"result":{"revision":7}}),
                )
                .unwrap(),
                MAX_FRAME,
            )
            .unwrap();
            let clear: Value = serde_json::from_slice(
                &qc_broker_framing::read_frame(&mut socket, MAX_FRAME)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(clear["method"], "broker.demoClearSelection");
            assert_eq!(clear["params"], json!({"expectedRevision":7}));
            qc_broker_framing::write_frame(
                &mut socket,
                &serde_json::to_vec(
                    &json!({"jsonrpc":"2.0","id":clear["id"],"result":{"cleared":true}}),
                )
                .unwrap(),
                MAX_FRAME,
            )
            .unwrap();
        });
        let mut connection = Connection { stream, next_id: 1 };
        let result = clear_observed_demo(&mut connection, || true, true);
        drop(connection);
        peer.join().unwrap();
        result.unwrap();
    }

    #[test]
    fn fresh_window_legacy_clear_allows_only_the_exact_unsupported_protocol_reply() {
        for (message, legacy, allowed) in [
            (
                "This method is not available to a demoProvider connection",
                true,
                true,
            ),
            (
                "This method is not available to a demoProvider connection",
                false,
                false,
            ),
            ("Authentication failed", true, false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let peer = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let query: Value = serde_json::from_slice(
                    &qc_broker_framing::read_frame(&mut socket, MAX_FRAME)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                qc_broker_framing::write_frame(&mut socket, &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":query["id"],"error":{"code":-32002,"message":message}})).unwrap(), MAX_FRAME).unwrap();
            });
            let mut connection = Connection { stream, next_id: 1 };
            let result = clear_observed_demo(&mut connection, || true, legacy);
            drop(connection);
            peer.join().unwrap();
            assert_eq!(result.is_ok(), allowed);
        }
    }

    #[test]
    fn explicit_demo_requires_broker_selection_acknowledgement_not_only_registration() {
        assert!(explicit_demo_registration(
            &json!({"explicitSelection":true,"target":{"source":"demo"}})
        ));
        for response in [
            json!({"target":{"source":"demo"}}),
            json!({"explicitSelection":false,"target":{"source":"demo"}}),
            json!({"explicitSelection":true,"target":{"source":"physical"}}),
            json!({"explicitSelection":true,"target":null}),
        ] {
            assert!(!explicit_demo_registration(&response));
        }
    }

    #[test]
    fn concurrent_start_fences_the_superseded_spawn_and_retires_its_exact_worker() {
        fn worker(id: &str, stop: Arc<AtomicBool>) -> Worker {
            let (commands, _receive) = mpsc::sync_channel(8);
            Worker {
                provider_id: id.into(),
                stop,
                socket: Arc::new(Mutex::new(None)),
                pending: Arc::new(Mutex::new(None)),
                commands,
                original: None,
            }
        }
        let bridge = Arc::new(DemoProviderBridge::default());
        bridge.2.store(1, Ordering::Release);
        let first_stop = Arc::new(AtomicBool::new(false));
        let second_stop = Arc::new(AtomicBool::new(false));
        let second_entered = Arc::new(AtomicBool::new(false));
        let (first_inside, inside) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let first = {
            let bridge = bridge.clone();
            let stop = first_stop.clone();
            std::thread::spawn(move || {
                bridge.replace(1, || {
                    first_inside.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(worker("first", stop))
                })
            })
        };
        inside.recv_timeout(Duration::from_secs(2)).unwrap();
        let (attempting, attempted) = mpsc::sync_channel(1);
        let second = {
            let bridge = bridge.clone();
            let stop = second_stop.clone();
            let entered = second_entered.clone();
            std::thread::spawn(move || {
                bridge.2.store(2, Ordering::Release);
                attempting.send(()).unwrap();
                bridge
                    .replace(2, || {
                        entered.store(true, Ordering::Release);
                        Ok(worker("second", stop))
                    })
                    .unwrap();
            })
        };
        attempted.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!second_entered.load(Ordering::Acquire));
        release.send(()).unwrap();
        assert!(first.join().unwrap().is_err());
        second.join().unwrap();
        assert!(first_stop.load(Ordering::Acquire));
        assert!(!second_stop.load(Ordering::Acquire));
        assert_eq!(
            bridge.0.lock().unwrap().as_ref().unwrap().provider_id,
            "second"
        );
        bridge.stop();
        assert!(second_stop.load(Ordering::Acquire));
    }

    #[test]
    fn reverse_client_refuses_unsolicited_reply_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            qc_broker_framing::read_frame(&mut socket, MAX_FRAME).unwrap();
            qc_broker_framing::write_frame(
                &mut socket,
                br#"{"jsonrpc":"2.0","id":99,"result":null}"#,
                MAX_FRAME,
            )
            .unwrap();
        });
        let mut connection = Connection { stream, next_id: 1 };
        assert!(connection
            .call("broker.demoNext", json!({}), Duration::from_secs(2))
            .unwrap_err()
            .contains("Unsolicited"));
        peer.join().unwrap();
    }

    #[test]
    fn withdrawal_closes_only_its_owned_socket_and_rejects_late_or_foreign_replies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (commands, _receive) = mpsc::sync_channel(8);
        let target = json!({"source":"demo","sessionId":"broker-1:demo-1","generation":1});
        let pending = Arc::new(Mutex::new(Some((target.clone(), 1))));
        let bridge = DemoProviderBridge::default();
        *bridge.0.lock().unwrap() = Some(Worker {
            provider_id: "ui-1".into(),
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(Some(stream))),
            pending: pending.clone(),
            commands,
            original: None,
        });
        assert!(sender(&bridge, "foreign-ui", &target, 1).is_err());
        assert!(sender(&bridge, "ui-1", &target, 2).is_err());
        assert!(sender(&bridge, "ui-1", &target, 1).is_ok());
        pending.lock().unwrap().take();
        assert!(sender(&bridge, "ui-1", &target, 1).is_err());
        bridge.stop();
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }
}
