//! A dedicated authenticated reverse connection. It never holds the app's
//! Gateway mutex, claims USB, starts a broker, or retries an operation.
use crate::{broker_attach, device_broker_descriptor, DEVICE_BROKER_DESCRIPTOR_PATH};
use serde_json::{json, Value};
use std::net::{Shutdown, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

const MAX_FRAME: usize = quadcortex::domain::IPC_MAX_FRAME_BYTES;
const REQUEST_EVENT: &str = "qc-demo-provider-request";
const STATUS_EVENT: &str = "qc-demo-provider-status";

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
    completed: Option<mpsc::Receiver<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(socket) = self.socket.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let Some(completed) = self.completed.take() {
            let _ = completed.recv_timeout(Duration::from_secs(2));
        }
    }
}

#[derive(Default)]
pub(crate) struct DemoProviderBridge(Mutex<Option<Worker>>);

impl DemoProviderBridge {
    pub(crate) fn stop(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).take();
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

fn run(
    app: &AppHandle,
    provider_id: &str,
    stop: &AtomicBool,
    socket: &Mutex<Option<TcpStream>>,
    pending: &Mutex<Option<(Value, u64)>>,
    commands: mpsc::Receiver<Command>,
) -> Result<(), String> {
    let path = DEVICE_BROKER_DESCRIPTOR_PATH
        .get()
        .ok_or("The shared broker directory is unavailable")?;
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
    let registration = connection.call(
        "broker.demoRegister",
        json!({"sessionId":provider_id}),
        Duration::from_secs(5),
    )?;
    app.emit_to(
        "main",
        STATUS_EVENT,
        json!({"providerId":provider_id,"ready":true,"registration":registration}),
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
    let _ = connection.call("broker.demoWithdraw", json!({}), Duration::from_secs(2));
    Ok(())
}

#[tauri::command]
pub(crate) fn start_demo_provider(
    app: AppHandle,
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
    session_id: String,
) -> Result<(), String> {
    main_window(&window)?;
    if session_id.is_empty() || session_id.len() > 128 {
        return Err("Invalid demo provider session".into());
    }
    bridge.stop();
    let stop = Arc::new(AtomicBool::new(false));
    let socket = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(None));
    let (send, receive) = mpsc::sync_channel(8);
    let (completed, completion) = mpsc::sync_channel(1);
    let worker = Worker {
        provider_id: session_id.clone(),
        stop: stop.clone(),
        socket: socket.clone(),
        pending: pending.clone(),
        commands: send,
        completed: Some(completion),
    };
    std::thread::Builder::new()
        .name("qc-demo-provider".into())
        .spawn(move || {
            // Dropping this sender acknowledges exit even during unwinding.
            let _completed = completed;
            let result = run(&app, &session_id, &stop, &socket, &pending, receive);
            pending.lock().unwrap_or_else(|p| p.into_inner()).take();
            socket.lock().unwrap_or_else(|p| p.into_inner()).take();
            let _ = app.emit_to(
                "main",
                STATUS_EVENT,
                json!({"providerId":session_id,"ready":false,"message":result.err()}),
            );
        })
        .map_err(|e| e.to_string())?;
    *bridge.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(worker);
    Ok(())
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
pub(crate) fn stop_demo_provider(
    window: WebviewWindow,
    bridge: State<'_, DemoProviderBridge>,
    provider_id: String,
) -> Result<(), String> {
    main_window(&window)?;
    let mut current = bridge.0.lock().unwrap_or_else(|p| p.into_inner());
    if current
        .as_ref()
        .is_some_and(|worker| worker.provider_id == provider_id)
    {
        current.take();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

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
        let bridge = DemoProviderBridge(Mutex::new(Some(Worker {
            provider_id: "ui-1".into(),
            stop: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(Some(stream))),
            pending: pending.clone(),
            commands,
            completed: None,
        })));
        assert!(sender(&bridge, "foreign-ui", &target, 1).is_err());
        assert!(sender(&bridge, "ui-1", &target, 2).is_err());
        assert!(sender(&bridge, "ui-1", &target, 1).is_ok());
        pending.lock().unwrap().take();
        assert!(sender(&bridge, "ui-1", &target, 1).is_err());
        bridge.stop();
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }
}
