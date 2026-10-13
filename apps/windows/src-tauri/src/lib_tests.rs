#![cfg(test)]

use super::*;

#[test]
fn macos_main_window_close_keeps_it_only_when_background_mode_is_enabled() {
    assert_eq!(
        macos_main_window_close_action(true),
        MacosMainWindowCloseAction::Hide
    );
    assert_eq!(
        macos_main_window_close_action(false),
        MacosMainWindowCloseAction::Exit
    );
}

fn write_broker_descriptor_fixture(
    file: &mut std::fs::File,
    descriptor: &device_broker_descriptor::DeviceBrokerDescriptor,
) {
    let wire = json!({
        "port": descriptor.port,
        "bearerToken": descriptor.bearer_token.as_str(),
        "instanceId": descriptor.instance_id.as_str(),
        "processId": descriptor.process_id,
        "processCreatedAt": descriptor.process_created_at,
        "publishedAtUnixMs": descriptor.published_at_unix_ms,
        "gatewayApiVersion": descriptor.gateway_api_version,
    });
    std::io::Write::write_all(file, &serde_json::to_vec(&wire).unwrap()).unwrap();
}

#[test]
fn gateway_domain_error_details_survive_the_production_device_adapter_mapping() {
    let response = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "error": {
            "code": -32013,
            "message": "DEVICE_SUSPENDED: BACKUP in progress",
            "data": {
                "code": "DEVICE_SUSPENDED",
                "retryable": false,
                "requestSent": false,
                "privatePayload": "must not cross the boundary"
            }
        }
    });
    let failure = decode_gateway_response(response, 7).expect_err("gateway refusal");
    let device_error = device_error_from_gateway_failure(failure);

    assert_eq!(device_error.code, "-32013");
    assert_eq!(
        device_error.details.as_deref(),
        Some(&json!({
            "code": "DEVICE_SUSPENDED",
            "retryable": false,
            "requestSent": false
        }))
    );
    assert!(!device_error
        .details
        .as_ref()
        .unwrap()
        .to_string()
        .contains("privatePayload"));
}

#[test]
fn only_a_debug_build_runs_a_broker_named_by_the_environment() {
    let configured = Some(std::ffi::OsString::from(r"C:\elsewhere\broker.exe"));
    assert_eq!(
        broker_executable_override(true, configured.clone()),
        configured
    );
    assert_eq!(broker_executable_override(false, configured), None);
    assert_eq!(broker_executable_override(true, None), None);
    assert_eq!(
        broker_executable_override(true, Some(std::ffi::OsString::new())),
        None
    );
}

#[test]
fn broker_notifications_reach_the_webview_and_responses_do_not() {
    assert_eq!(
        device_event_name(&json!({"jsonrpc": "2.0", "method": "device.stateFrame", "params": {}})),
        Some("qc-state-frame")
    );
    assert_eq!(
        device_event_name(
            &json!({"jsonrpc": "2.0", "method": "device.phase", "params": {"phase": "handshaking"}})
        ),
        Some("qc-device-phase")
    );
    assert_eq!(
        device_event_name(&json!({"jsonrpc": "2.0", "id": 4, "result": {"phase": "ready"}})),
        None,
        "an RPC response must stay with the request that is waiting for it"
    );
    assert_eq!(
        device_event_name(&json!({"jsonrpc": "2.0", "method": "device.unknown", "params": {}})),
        None
    );
}

#[test]
fn relay_pairing_browser_url_must_stay_on_the_secure_relay_origin() {
    assert!(relay_pair_url_allowed(
        "https://qcrc.freevia.org",
        "https://qcrc.freevia.org/pair?hostSession=opaque"
    ));
    assert!(!relay_pair_url_allowed(
        "https://qcrc.freevia.org",
        "https://evil.example/pair?hostSession=opaque"
    ));
    assert!(!relay_pair_url_allowed(
        "https://qcrc.freevia.org",
        "http://qcrc.freevia.org/pair?hostSession=opaque"
    ));
}

#[test]
fn workspace_validation_requires_version_and_snapshot() {
    assert!(validate_workspace(&json!({ "version": 1, "snapshot": {} })).is_ok());
    assert!(validate_workspace(&json!({ "version": 2, "snapshot": {} })).is_err());
    assert!(validate_workspace(&json!({ "version": 1 })).is_err());
}

#[test]
fn workspace_file_name_removes_windows_path_characters() {
    assert_eq!(safe_workspace_name("6B ICFTF: #22?"), "6B ICFTF_ #22_");
    assert_eq!(safe_workspace_name("..."), "QC Workspace");
}

#[test]
fn native_backup_validation_requires_the_qc_container_fields() {
    let valid = json!({
        "type": "backup",
        "creator": "quad",
        "payload": "AA==",
        "payload_hash": "0".repeat(64)
    });
    assert!(validate_native_backup(&valid).is_ok());
    assert!(validate_native_backup(&json!({ "type": "backup" })).is_err());
}

#[test]
fn a_backup_name_is_held_to_the_shared_rule_before_the_transfer_runs() {
    // `safe_backup_name` truncated at 80 while the shared boundary
    // (`finalize_device_backup` -> `validate_visible_text`) hard-rejects over
    // 64, so a 65-80 character name opened the save dialog, ran the whole
    // device transfer, and only then failed. Refuse it here instead.
    let limit = quadcortex::device::request::BACKUP_NAME_MAX_CHARACTERS;
    assert_eq!(limit, 64);
    let at_limit = "n".repeat(limit);
    assert_eq!(validate_backup_name(&at_limit).as_deref(), Ok(&*at_limit));
    let over = "n".repeat(limit + 1);
    let error = validate_backup_name(&over).expect_err("65 characters must be refused");
    assert!(error.contains("1-64"), "{error}");
    assert!(
        validate_backup_name(&"n".repeat(80)).is_err(),
        "the old 80-character ceiling must no longer pass"
    );

    // The name itself is never reshaped: the workflow doc forbids truncating
    // or silently deleting characters, and the document keeps what the user
    // typed.
    assert_eq!(
        validate_backup_name("  Band: Friday?  ").as_deref(),
        Ok("Band: Friday?")
    );
    assert!(
        validate_backup_name("Band\u{7}Friday").is_err(),
        "a control character is refused, not deleted"
    );

    // Only the dialog's suggested filename is sanitised, because Windows
    // cannot save those characters.
    assert_eq!(backup_file_stem("Band: Friday?"), "Band_ Friday_");
    assert_eq!(backup_file_stem("///"), "___");
    // A stem that reduces to nothing a filesystem would accept still needs
    // some default for the dialog.
    assert_eq!(backup_file_stem("  ..  "), "QC Device Backup");
}

#[test]
fn workspace_round_trips_on_disk() {
    let path = std::env::temp_dir().join(format!(
        "qc-workspace-test-{}-{}.qcw",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    let document = json!({ "version": 1, "snapshot": { "presetName": "Test" } });
    let result = write_workspace(&path, &document).expect("workspace write");
    assert_eq!(result["cancelled"], false);
    let loaded: Value =
        serde_json::from_slice(&fs::read(&path).expect("workspace read")).expect("workspace JSON");
    assert_eq!(loaded, document);
    fs::remove_file(&path).expect("workspace cleanup");
}

#[test]
fn diagnostics_are_allowlisted_and_redacted() {
    let report = json!({
        "generatedAt": "2026-08-30T17:00:00Z",
        "appVersion": "0.1.0",
        "runtime": { "platform": "Python gateway", "gatewayAvailable": true, "username": "secret" },
        "connection": { "phase": "ready", "demo": false, "detail": "C:\\Users\\secret" },
        "device": { "presetLocation": "6B", "presetPosition": 41, "mode": "STOMP", "activeScene": 0, "tempo": 45, "dirty": false, "blockCount": 18, "presetName": "private" },
        "events": [{ "at": "now", "event": "connected", "message": "private" }],
        "privatePayload": "private"
    });
    let safe = redacted_diagnostics(&report);
    let text = serde_json::to_string(&safe).expect("diagnostic JSON");
    assert!(!text.contains("secret"));
    assert!(!text.contains("private"));
    assert_eq!(safe["device"]["tempo"], 45);
    assert_eq!(safe["events"][0]["event"], "connected");
    assert_eq!(
        safe["connection"]["source"], "unknown",
        "an older report without the field is not guessed at"
    );
}

#[test]
fn diagnostics_keep_a_demo_session_distinguishable_from_hardware() {
    let report = |source: &str| json!({ "connection": { "phase": "ready", "demo": false, "source": source } });
    assert_eq!(
        redacted_diagnostics(&report("virtual"))["connection"]["source"],
        "virtual"
    );
    assert_eq!(
        redacted_diagnostics(&report("device"))["connection"]["source"],
        "device"
    );
    assert_eq!(
        redacted_diagnostics(&report("C:\\Users\\secret"))["connection"]["source"],
        "unknown"
    );
}

#[test]
fn a_contract_violation_is_reported_without_dropping_the_session() {
    // Regression: this was classified as a transport fault, so the host
    // killed and respawned a broker that was alive, answering, and framing
    // correctly - turning one malformed field into a dropped QC session on
    // every preset change. Only the transport arm may clear the process.
    let malformed = json!({ "detail": "Preset recalled", "snapshot": {} });
    let failure = validate_gateway_result("device.recallPreset", &malformed)
        .expect_err("a result without verification semantics must be rejected");
    match failure {
        GatewayRequestFailure::Remote {
            code, retryable, ..
        } => {
            assert_eq!(code, GATEWAY_CONTRACT_ERROR_CODE);
            assert!(!retryable, "replaying a contract violation cannot help");
        }
        GatewayRequestFailure::Transport(message) => {
            panic!("a contract violation must not be a transport fault: {message}")
        }
    }

    // A conforming result still passes.
    let valid = json!({
        "accepted": true,
        "verified": false,
        "verification": "accepted_unverified",
        "detail": "Preset change sent; the host observed a position update for /user/live position 9, which does not prove this request caused it, and no preset body can be bound to that address, so the loaded preset's contents are not verified",
        "observation": {"kind": "preset_address_observed", "setlistKey": "/user/live", "position": 9,
            "address": "observed", "body": "unbound", "causallyCorrelated": false},
        "snapshot": {"presetName": "Test", "blocks": []}
    });
    assert!(validate_gateway_result("device.recallPreset", &valid).is_ok());
}

/// How long the fake broker's backup holds its queue. Long enough that a
/// status read waiting for the gateway could not pass for a prompt one.
const FAKE_BACKUP_DURATION: Duration = Duration::from_secs(3);

/// One broker connection served the way the device broker serves it:
/// `system.status` answered as it arrives, every other request in order
/// on one queue. `device.createBackup` suspends the device as the unit
/// does and holds the queue for `FAKE_BACKUP_DURATION`; the suspension
/// then lasts until the test clears `suspended`, and a request reaching
/// the queue while it lasts is refused as the broker refuses it.
struct ConcurrentFakeBroker {
    suspended: std::sync::Arc<std::sync::atomic::AtomicBool>,
    executed: std::sync::Arc<Mutex<Vec<String>>>,
    backup_started: mpsc::Receiver<()>,
}

fn serve_concurrent_fake_broker(peer: TcpStream) -> ConcurrentFakeBroker {
    use std::sync::atomic::Ordering;
    let suspended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let executed = std::sync::Arc::new(Mutex::new(Vec::new()));
    let (started_tx, backup_started) = mpsc::channel();
    let output = std::sync::Arc::new(Mutex::new(peer.try_clone().unwrap()));
    let reply = |output: &Mutex<TcpStream>, response: Value| {
        let _ = write_frame(
            &mut *output.lock().unwrap(),
            &serde_json::to_vec(&response).unwrap(),
        );
    };
    let (queue_tx, queue) = mpsc::channel::<Value>();
    {
        let (output, suspended, executed) = (output.clone(), suspended.clone(), executed.clone());
        std::thread::spawn(move || {
            for request in queue {
                let method = request["method"].as_str().unwrap_or_default().to_owned();
                executed.lock().unwrap().push(method.clone());
                let id = request["id"].clone();
                if method == rpc::CREATE_DEVICE_BACKUP {
                    suspended.store(true, Ordering::Release);
                    let _ = started_tx.send(());
                    std::thread::sleep(FAKE_BACKUP_DURATION);
                    reply(
                        &output,
                        json!({"jsonrpc":"2.0","id":id,"result":{"name":"acceptance"}}),
                    );
                } else if suspended.load(Ordering::Acquire) {
                    reply(
                        &output,
                        json!({"jsonrpc":"2.0","id":id,"error":{
                            "code": -32013,
                            "message": "DEVICE_SUSPENDED: BACKUP in progress",
                            "data": {"code":"DEVICE_SUSPENDED","retryable":false,"requestSent":false}
                        }}),
                    );
                } else {
                    reply(&output, json!({"jsonrpc":"2.0","id":id,"result":{}}));
                }
            }
        });
    }
    {
        let suspended = suspended.clone();
        let mut input = peer;
        std::thread::spawn(move || {
            while let Ok(request) = read_frame(&mut input) {
                let method = request["method"].as_str().unwrap_or_default();
                if quadcortex::device::answers_beside_running_request(method) {
                    let now = suspended.load(Ordering::Acquire);
                    let id = request["id"].clone();
                    reply(
                        &output,
                        json!({"jsonrpc":"2.0","id":id,"result":{
                            "gatewayApiVersion": generated_gateway::API_VERSION,
                            "usbDiagnostics": {
                                "connected": true,
                                "phase": "ready",
                                "synchronized": true,
                                "deviceSuspended": now,
                                "suspension": {"active": if now {
                                    json!({"reason":"BACKUP"})
                                } else {
                                    Value::Null
                                }}
                            }
                        }}),
                    );
                } else if queue_tx.send(request).is_err() {
                    return;
                }
            }
        });
    }
    ConcurrentFakeBroker {
        suspended,
        executed,
        backup_started,
    }
}

/// A gateway attached to `serve_concurrent_fake_broker` exactly as
/// `GatewayProcess::start_shared` attaches to the real one.
fn gateway_with_concurrent_fake_broker() -> (
    std::sync::Arc<Mutex<Gateway>>,
    LockFreeReadSlot,
    ConcurrentFakeBroker,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let broker = serve_concurrent_fake_broker(peer);
    let intentional_release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reads = std::sync::Arc::new(LockFreeReads::new(
        stream.try_clone().unwrap(),
        intentional_release.clone(),
    ));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader_with(
        stream.try_clone().unwrap(),
        None,
        intentional_release.clone(),
        Some(reads.clone()),
    )
    .unwrap();
    let gateway = Gateway {
        process: Some(GatewayProcess {
            transport: GatewayTransport::SharedBroker {
                stream,
                hosted_child: None,
                instance_id: "concurrent-fake-broker".into(),
                release_marker_path: PathBuf::new(),
            },
            responses,
            reader_completed: completed,
            next_id: 1,
            broker_pid: 4242,
            intentional_release,
            participant_announced: true,
            lock_free_reads: Some(reads),
        }),
        ..Gateway::default()
    };
    gateway.publish_lock_free_reads();
    let slot = gateway.lock_free_read_slot();
    (std::sync::Arc::new(Mutex::new(gateway)), slot, broker)
}

fn request_through(
    gateway: &Mutex<Gateway>,
    slot: &LockFreeReadSlot,
    method: &'static str,
    params: Value,
) -> Result<Value, GatewayRequestFailure> {
    request_gateway(
        gateway,
        slot,
        method,
        params,
        WhenGatewayBusy::Wait,
        |gateway, params| gateway.request_detailed(method, params),
    )
}

/// Found on hardware: during a 28-second backup through the
/// HTTP gateway every concurrent `system.status` waited for the backup,
/// so no client ever saw `deviceSuspended: true`. A status read must now
/// answer while the backup holds the gateway, with the broker's own
/// account of the suspension. Writes keep their exact old behaviour:
/// one at a time behind the backup, then refused by the broker while the
/// device stays suspended.
#[test]
fn status_answers_during_a_long_request_while_writes_stay_serialized() {
    let (gateway, slot, broker) = gateway_with_concurrent_fake_broker();
    let backup = {
        let (gateway, slot) = (gateway.clone(), slot.clone());
        std::thread::spawn(move || {
            request_through(
                &gateway,
                &slot,
                rpc::CREATE_DEVICE_BACKUP,
                json!({"name":"acceptance"}),
            )
        })
    };
    broker
        .backup_started
        .recv_timeout(Duration::from_secs(2))
        .expect("the backup reached the broker");

    let started = Instant::now();
    let status = request_through(&gateway, &slot, rpc::RUNTIME_STATUS, json!({}))
        .unwrap_or_else(|_| panic!("system.status failed during the backup"));
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(1),
        "system.status waited {waited:?} behind the backup"
    );
    assert_eq!(
        status["usbDiagnostics"]["deviceSuspended"], true,
        "{status}"
    );
    assert_eq!(
        status["usbDiagnostics"]["suspension"]["active"]["reason"],
        "BACKUP"
    );

    // A write still queues for the gateway behind the backup: it has not
    // even been sent while the backup runs.
    let write = {
        let (gateway, slot) = (gateway.clone(), slot.clone());
        std::thread::spawn(move || {
            request_through(
                &gateway,
                &slot,
                rpc::SELECT_SCENE,
                json!({"scene":1,"expectedPresetIdentity":"x"}),
            )
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(!write.is_finished(), "a write overtook the running backup");
    assert_eq!(
        *broker.executed.lock().unwrap(),
        vec![rpc::CREATE_DEVICE_BACKUP.to_string()]
    );

    assert!(backup.join().unwrap().is_ok());
    match write.join().unwrap() {
        Err(GatewayRequestFailure::Remote { code, message, .. }) => {
            assert_eq!(code, -32013);
            assert!(message.starts_with("DEVICE_SUSPENDED"), "{message}");
        }
        Err(GatewayRequestFailure::Transport(message)) => {
            panic!("the write's refusal arrived as a transport failure: {message}")
        }
        Ok(result) => panic!("a write during the suspension was accepted: {result}"),
    }
    assert_eq!(
        *broker.executed.lock().unwrap(),
        vec![
            rpc::CREATE_DEVICE_BACKUP.to_string(),
            rpc::SELECT_SCENE.to_string()
        ]
    );
    // Once the gateway is free again a status read takes it as before.
    broker
        .suspended
        .store(false, std::sync::atomic::Ordering::Release);
    let status = request_through(&gateway, &slot, rpc::RUNTIME_STATUS, json!({}))
        .unwrap_or_else(|_| panic!("system.status failed on a free gateway"));
    assert_eq!(status["usbDiagnostics"]["deviceSuspended"], false);
}

/// A reply in the lock-free id range that nobody waits for any more - the
/// read gave up on a broker that answers in order - is dropped, and never
/// handed to the request that holds the gateway.
#[test]
fn a_late_lock_free_reply_never_reaches_the_request_in_flight() {
    let mut frames = Vec::new();
    for response in [
        json!({"jsonrpc":"2.0","id":LOCK_FREE_READ_ID_BASE + 7,"result":{"late":true}}),
        json!({"jsonrpc":"2.0","id":1,"result":{"name":"acceptance"}}),
    ] {
        write_frame(&mut frames, &serde_json::to_vec(&response).unwrap()).unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reads = std::sync::Arc::new(LockFreeReads::new(
        stream.try_clone().unwrap(),
        flag.clone(),
    ));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader_with(
        std::io::Cursor::new(frames),
        None,
        flag.clone(),
        Some(reads.clone()),
    )
    .unwrap();
    let mut process = GatewayProcess {
        transport: GatewayTransport::SharedBroker {
            stream,
            hosted_child: None,
            instance_id: "late-lock-free-reply".into(),
            release_marker_path: PathBuf::new(),
        },
        responses,
        reader_completed: completed,
        next_id: 1,
        broker_pid: 4242,
        intentional_release: flag,
        participant_announced: true,
        lock_free_reads: Some(reads),
    };
    assert_eq!(
        process
            .request_unchecked(rpc::CREATE_DEVICE_BACKUP, json!({"name":"acceptance"}))
            .unwrap_or_else(|_| panic!("the backup's own reply was displaced")),
        json!({"name":"acceptance"})
    );
}

#[test]
fn only_lock_free_reads_travel_beside_a_running_request() {
    let stream = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        TcpStream::connect(listener.local_addr().unwrap()).unwrap()
    };
    let reads = LockFreeReads::new(
        stream,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    for method in [
        rpc::SELECT_SCENE,
        rpc::CREATE_DEVICE_BACKUP,
        "device.status",
        rpc::CURRENT_PRESET,
        // Lock-free in the broker, but able to start a USB reopen.
        rpc::CURRENT_STATE_EVENTS,
        rpc::CURRENT_TEMPO_CLOCK,
        rpc::CURRENT_MASTER_VOLUME,
    ] {
        assert!(
            matches!(
                reads.request(method, json!({})),
                Err(GatewayRequestFailure::Transport(_))
            ),
            "{method} must never bypass the gateway"
        );
    }
    assert!(reads.pending().is_empty());
}

fn gateway_with_fake_broker_result(result: Value) -> (Gateway, TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let mut frames = Vec::new();
    write_frame(
        &mut frames,
        &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"result":result})).unwrap(),
    )
    .unwrap();
    let intentional_release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader(
        std::io::Cursor::new(frames),
        None,
        intentional_release.clone(),
    )
    .unwrap();
    let gateway = Gateway {
        process: Some(GatewayProcess {
            transport: GatewayTransport::SharedBroker {
                stream,
                hosted_child: None,
                instance_id: "broker-control-result-test".into(),
                release_marker_path: PathBuf::new(),
            },
            responses,
            reader_completed: completed,
            next_id: 1,
            broker_pid: 4242,
            intentional_release,
            participant_announced: false,
            lock_free_reads: None,
        }),
        ..Gateway::default()
    };
    (gateway, peer)
}

#[test]
fn broker_control_results_are_typed_and_malformed_replies_keep_the_healthy_connection() {
    let devices = json!({"devices":[
            {"id":"device-1","name":"Quad Cortex One","connected":true,"phase":"ready"},
            {"id":"device-2","name":"Quad Cortex Two","connected":false,"phase":"disconnected"}
        ],"selectedDeviceId":"device-2"});
    let (mut gateway, _peer) = gateway_with_fake_broker_result(devices.clone());
    assert_eq!(
        gateway.request("broker.devices", json!({})).unwrap(),
        devices
    );
    assert_eq!(gateway.selected_device_id.as_deref(), Some("device-2"));
    assert_eq!(gateway.process.as_ref().unwrap().broker_pid, 4242);

    let empty_devices = json!({"devices":[],"selectedDeviceId":null});
    let (mut gateway, _peer) = gateway_with_fake_broker_result(empty_devices.clone());
    assert_eq!(
        gateway.request("broker.devices", json!({})).unwrap(),
        empty_devices
    );
    assert_eq!(gateway.selected_device_id, None);
    assert_eq!(gateway.process.as_ref().unwrap().broker_pid, 4242);

    let selected = json!({"selectedDeviceId":"device-1"});
    let (mut gateway, _peer) = gateway_with_fake_broker_result(selected.clone());
    assert_eq!(
        gateway
            .request("broker.selectDevice", json!({"deviceId":"device-1"}))
            .unwrap(),
        selected
    );
    assert_eq!(gateway.process.as_ref().unwrap().broker_pid, 4242);

    for (method, params, malformed) in [
        (
            "broker.devices",
            json!({}),
            json!({"devices":[{"id":"device-1","name":"Quad Cortex","connected":"yes","phase":"ready"}],"selectedDeviceId":null}),
        ),
        (
            "broker.selectDevice",
            json!({"deviceId":"device-1"}),
            json!({"selectedDeviceId":false}),
        ),
    ] {
        let (mut gateway, _peer) = gateway_with_fake_broker_result(malformed);
        let error = gateway
            .request(method, params)
            .expect_err("malformed broker result must be rejected");
        assert!(error.contains("malformed broker-control result"), "{error}");
        assert_eq!(gateway.process.as_ref().unwrap().broker_pid, 4242,
                "a malformed result is a remote contract failure, not a reason to restart the healthy broker");
        assert!(gateway.retiring_process.is_none());
    }
}

#[test]
fn a_reservation_timeout_is_classified_by_code_not_by_guessing_at_text() {
    // Older brokers emitted this code after their retired retry path.
    // Retain compatibility without teaching current scoped timeouts to
    // clear readiness or trigger transport recovery.
    assert!(remote_failure_marks_disconnected(
        RESERVE_REQUEST_ID_TIMEOUT_ERROR_CODE,
        "Native QC request-id reservation timed out"
    ));
    // A contract violation is a fault in an otherwise-healthy broker and
    // must not affect readiness (see
    // `a_contract_violation_is_reported_without_dropping_the_session`).
    assert!(!remote_failure_marks_disconnected(
        GATEWAY_CONTRACT_ERROR_CODE,
        "gateway contract mismatch"
    ));
    // An ordinary, unrelated remote error is not readiness evidence
    // either - only its code or the one known session-loss phrase are.
    assert!(!remote_failure_marks_disconnected(
        -32010,
        "Scene must be between 0 and 7"
    ));
    for message in [
        "QC request budget expired before request ID reservation",
        "QC request budget expired while reserving a request ID",
        "QC logical read budget expired",
        "QC command budget expired before native admission",
    ] {
        assert!(!remote_failure_marks_disconnected(-32010, message));
    }
    // The known session-loss phrasing still works regardless of code.
    assert!(remote_failure_marks_disconnected(
        -32010,
        "No Quad Cortex session is open"
    ));
}

#[test]
fn a_stale_brokers_api_version_is_a_loud_specific_mismatch_not_a_downstream_device_error() {
    // Regression: tauri.conf.json and a running installed broker were
    // observed at different versions with nothing comparing them.
    // gatewayApiVersion has always been in every system.status reply;
    // nothing ever read it. The failure this prevents is a stale
    // broker's older argument contract rejecting a newer schema and
    // surfacing as an incomprehensible device error several calls later
    // (the `add_block` deadlock was this shape) instead of an immediate,
    // specific one at startup.
    assert!(check_gateway_api_version(
        &json!({
            "gatewayApiVersion": generated_gateway::API_VERSION
        }),
        4242
    )
    .is_ok());

    let mismatch = check_gateway_api_version(&json!({ "gatewayApiVersion": 3 }), 4242)
        .expect_err("an older broker version must be rejected");
    assert!(mismatch.contains(&generated_gateway::API_VERSION.to_string()));
    assert!(mismatch.contains("reports version 3"));
    // A shared broker can outlive the upgrade that made it stale, kept
    // alive by any other client; the message must say which process it is.
    assert!(mismatch.contains("process 4242"), "{mismatch}");

    let missing = check_gateway_api_version(&json!({ "platform": "Rust device gateway" }), 4242)
        .expect_err("a reply with no version at all must be rejected, not assumed compatible");
    assert!(missing.contains("did not report a gatewayApiVersion"));
    assert!(missing.contains("process 4242"), "{missing}");
}

#[test]
fn runtime_health_contains_only_operational_metadata() {
    let gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    let health = runtime_health_document(
        &gateway,
        "device.snapshot",
        "remote-error",
        Some("[-32020] gateway contract mismatch"),
        &[],
    );
    let keys = health
        .as_object()
        .expect("health object")
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        [
            "appPid",
            "brokerStderrLog",
            "connected",
            "deviceObservedAtUnix",
            "devicePhase",
            "deviceSynchronized",
            "deviceWriteQuarantineReason",
            "deviceWritesQuarantined",
            "gatewayPid",
            "lastDetail",
            "lastMethod",
            "lastRequestAtUnix",
            "lastStatus",
            "version",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
    assert_eq!(health["connected"], true);
    // Present and null until a reply carrying `usbDiagnostics` is seen, so
    // the key set stays closed and a reader never has to guess whether a
    // missing key means "not observed" or "old build".
    assert_eq!(health["devicePhase"], Value::Null);
    assert_eq!(health["deviceSynchronized"], Value::Null);
    assert_eq!(health["deviceObservedAtUnix"], Value::Null);
    assert_eq!(health["lastMethod"], "device.snapshot");
    assert_eq!(health["lastStatus"], "remote-error");
    assert_eq!(health["lastDetail"], "[-32020] gateway contract mismatch");
    assert_eq!(
        health["brokerStderrLog"],
        broker_stderr_path().to_string_lossy().as_ref()
    );
}

#[test]
fn runtime_health_witnesses_a_device_session_stuck_mid_sync() {
    // `connected` is a host-side transport latch, so it cannot by itself
    // prove that device synchronization completed. Runtime health must
    // preserve the authoritative phase and synchronization fields.
    let mut gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    gateway.observe_device_session(&json!({
        "platform": "Rust device gateway",
        "usbDiagnostics": {
            "phase": "syncing",
            "synchronized": false,
            "connected": true,
            "detail": "Waiting for the active preset",
        }
    }));
    let health = runtime_health_document(&gateway, "system.status", "ok", None, &[]);

    // The latch still says connected - that is the whole point. A reader
    // must be able to separate "the last device call succeeded" from "the
    // session actually reached a usable state".
    assert_eq!(health["connected"], true);
    assert_eq!(health["devicePhase"], "syncing");
    assert_eq!(health["deviceSynchronized"], false);
    assert!(
        health["deviceObservedAtUnix"]
            .as_u64()
            .is_some_and(|at| at > 0),
        "device truth without an observation time is another latch"
    );

    // A healthy session reads differently.
    gateway.observe_device_session(&json!({
        "usbDiagnostics": { "phase": "ready", "synchronized": true, "connected": true }
    }));
    let healthy = runtime_health_document(&gateway, "system.status", "ok", None, &[]);
    assert_eq!(healthy["devicePhase"], "ready");
    assert_eq!(healthy["deviceSynchronized"], true);
}

/// The broker's `usbDiagnostics.writesQuarantined`/`writeQuarantineReason`
/// (system.status, via `controller.status()`) previously had no reader on
/// this side at all: `observe_device_session` did not parse them, so a
/// window polling `system.status` every 250ms carried the field the whole
/// time without ever surfacing it - connected, synchronized, and silently
/// rejecting every write.
#[test]
fn runtime_health_witnesses_a_quarantined_broker() {
    let mut gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    gateway.observe_device_session(&json!({
        "usbDiagnostics": {
            "phase": "ready",
            "synchronized": true,
            "connected": true,
            "writesQuarantined": true,
            "writeQuarantineReason": "native delivery was not confirmed"
        }
    }));
    let health = runtime_health_document(&gateway, "system.status", "ok", None, &[]);
    assert_eq!(health["deviceWritesQuarantined"], true);
    assert_eq!(
        health["deviceWriteQuarantineReason"],
        "native delivery was not confirmed"
    );

    // Clearing (a reply that omits the field, as every non-system.status
    // usbDiagnostics reply does today) must not be read as "still
    // quarantined" - `observe_device_session` replaces `self.device`
    // wholesale on every call that has enough to act on, so this follows
    // the same "device truth outranks the last-known value" rule the
    // phase/synchronized fields already have.
    gateway.observe_device_session(&json!({
        "usbDiagnostics": { "phase": "ready", "synchronized": true, "connected": true }
    }));
    let cleared = runtime_health_document(&gateway, "system.status", "ok", None, &[]);
    assert_eq!(cleared["deviceWritesQuarantined"], json!(null));
}

#[test]
fn unplugging_the_device_clears_readiness_from_the_status_poll_alone() {
    // Regression: `connected` was a latch only `device.*` calls could move.
    // An idle app runs nothing but the 1 Hz `system.status` poll, so a
    // Quad Cortex unplugged while idle left `/health` answering 200 /
    // `deviceReady: true` indefinitely - to every local MCP client.
    let mut gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    gateway.observe_device_session(&json!({
        "platform": "Rust device gateway",
        "usbDiagnostics": {
            "phase": "disconnected",
            "synchronized": false,
            "connected": false,
            "detail": "No Quad Cortex detected",
        }
    }));
    assert!(
        !gateway.connected,
        "the poll carries the disconnection in its payload"
    );

    // The witnessed disconnection is kept, not wiped: it is this reply's
    // own truth, and it is what says why the session ended.
    gateway.forget_device_session_if_disconnected();
    let health = runtime_health_document(&gateway, "system.status", "ok", None, &[]);
    assert_eq!(health["connected"], false);
    assert_eq!(health["devicePhase"], "disconnected");
    assert_eq!(health["deviceSynchronized"], false);

    // And it comes back the same way, without waiting for a device call.
    gateway.observe_device_session(&json!({
        "usbDiagnostics": { "phase": "ready", "synchronized": true, "connected": true }
    }));
    assert!(gateway.connected);
    assert!(gateway_readiness(&Mutex::new(gateway)));
}

#[test]
fn a_reply_that_does_not_report_the_session_leaves_the_latch_alone() {
    // Only `system.status` carries `usbDiagnostics`. A reply that omits
    // `connected` must not be read as a disconnection.
    let mut gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    gateway.observe_device_session(&json!({
        "usbDiagnostics": { "phase": "syncing", "synchronized": false }
    }));
    assert!(gateway.connected);
    gateway.observe_device_session(&json!({ "phase": "ready", "detail": "handshake complete" }));
    assert!(gateway.connected);
}

#[test]
fn each_help_window_kind_opens_its_own_resizable_window() {
    let guide = help_window_spec("guide").expect("guide window");
    let feedback = help_window_spec("feedback").expect("feedback window");
    let about = help_window_spec("about").expect("about window");
    assert_eq!(
        [guide.label, feedback.label, about.label],
        ["help-guide", "help-feedback", "help-about"]
    );
    assert_eq!(guide.title, "QC Remote — User Guide");
    assert_eq!(feedback.title, "QC Remote — Feedback");
    assert_eq!(about.title, "QC Remote — About");
    for spec in [&guide, &feedback] {
        assert_eq!((spec.width, spec.height), (760.0, 680.0));
        assert_eq!((spec.min_width, spec.min_height), (560.0, 480.0));
    }
    assert_eq!((about.width, about.height), (860.0, 760.0));
    assert_eq!((about.min_width, about.min_height), (640.0, 520.0));
    assert_eq!(help_window_spec("main"), None);
    assert_eq!(help_window_spec(""), None);
}

#[test]
fn opening_a_help_window_is_an_async_command() {
    // A synchronous command runs on the main thread, where building a
    // WebView2 window deadlocks on Windows. This only compiles while
    // `open_help_window` returns a future, so making it synchronous again
    // fails the build rather than freezing the app on F1.
    fn returns_a_future<Command, Pending>(_: Command)
    where
        Command: Fn(AppHandle, String, Option<String>) -> Pending,
        Pending: std::future::Future<Output = Result<(), String>> + Send,
    {
    }
    returns_a_future(open_help_window);
}

#[test]
fn each_app_window_kind_opens_its_own_resizable_window() {
    let settings = app_window_spec("settings").expect("settings window");
    let device_info = app_window_spec("device-info").expect("device info window");
    let workspace = app_window_spec("workspace").expect("workspace window");
    assert_eq!(
        [settings.label, device_info.label, workspace.label],
        ["app-settings", "app-device-info", "app-workspace"]
    );
    assert_eq!(settings.title, "QC Remote — Settings");
    assert_eq!(device_info.title, "QC Remote — Device Info");
    assert_eq!(workspace.title, "QC Remote — Workspace");
    assert_eq!((settings.width, settings.height), (680.0, 620.0));
    assert_eq!((settings.min_width, settings.min_height), (520.0, 460.0));
    for spec in [&device_info, &workspace] {
        assert_eq!((spec.width, spec.height), (640.0, 560.0));
        assert_eq!((spec.min_width, spec.min_height), (480.0, 420.0));
    }
    // The two families never share a kind, so neither command can open
    // the other's window.
    for kind in ["guide", "feedback", "about", "main", ""] {
        assert_eq!(app_window_spec(kind), None, "{kind}");
    }
    for kind in ["settings", "device-info", "workspace"] {
        assert_eq!(help_window_spec(kind), None, "{kind}");
    }
    assert_eq!(
        window_page("device-info", None),
        "index.html?window=device-info"
    );
}

#[test]
fn opening_an_app_window_is_an_async_command() {
    fn returns_a_future<Command, Pending>(_: Command)
    where
        Command: Fn(AppHandle, String) -> Pending,
        Pending: std::future::Future<Output = Result<(), String>> + Send,
    {
    }
    returns_a_future(open_app_window);
}

#[test]
fn a_feedback_window_opens_on_the_kind_of_report_its_menu_entry_names() {
    assert_eq!(
        feedback_issue_kind("feedback", Some("bug")),
        Ok(Some("bug"))
    );
    assert_eq!(
        feedback_issue_kind("feedback", Some("feature")),
        Ok(Some("feature"))
    );
    assert_eq!(feedback_issue_kind("feedback", None), Ok(None));
    assert_eq!(feedback_issue_kind("guide", None), Ok(None));
    // Only the two kinds the page understands reach its query, and only
    // on the Feedback window.
    assert!(feedback_issue_kind("feedback", Some("bug&window=about")).is_err());
    assert!(feedback_issue_kind("feedback", Some("")).is_err());
    assert!(feedback_issue_kind("about", Some("bug")).is_err());
    assert_eq!(
        window_page("feedback", Some("feature")),
        "index.html?window=feedback&issue=feature"
    );
    assert_eq!(window_page("guide", None), "index.html?window=guide");
    assert_eq!(FEEDBACK_ISSUE_KIND_EVENT, "feedback-issue-kind");
}

#[test]
fn only_the_main_window_going_away_closes_the_help_windows() {
    assert!(main_window_is_gone("main", &tauri::WindowEvent::Destroyed));
    assert!(!main_window_is_gone(
        "help-guide",
        &tauri::WindowEvent::Destroyed
    ));
    assert!(!main_window_is_gone(
        "main",
        &tauri::WindowEvent::Focused(false)
    ));
}

#[test]
fn the_exit_handler_fires_on_the_event_tauri_delivers_before_process_exit() {
    // `tauri::App::run` ends in `std::process::exit`, so `Drop` never runs
    // and the loopback descriptor outlived the app. This is the event that
    // does reach us: tauri's `make_run_event_loop_callback` invokes the
    // callback for `RuntimeRunEvent::Exit` before `cleanup_before_exit`.
    assert!(is_process_exit_event(&tauri::RunEvent::Exit));
    assert!(!is_process_exit_event(&tauri::RunEvent::Ready));
    assert!(!is_process_exit_event(&tauri::RunEvent::Resumed));
    assert!(!is_process_exit_event(&tauri::RunEvent::MainEventsCleared));
}

#[test]
fn a_lost_session_does_not_leave_a_ready_phase_behind() {
    // Carrying the device truth forward after the session is gone would
    // recreate the same defect one field over: a stale "ready" read as
    // current.
    let mut gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    gateway.observe_device_session(&json!({
        "usbDiagnostics": { "phase": "ready", "synchronized": true }
    }));
    assert_eq!(
        runtime_health_document(&gateway, "system.status", "ok", None, &[])["devicePhase"],
        "ready"
    );

    gateway.connected = false;
    gateway.forget_device_session_if_disconnected();
    let health = runtime_health_document(&gateway, "device.snapshot", "transport-error", None, &[]);
    assert_eq!(health["connected"], false);
    assert_eq!(health["devicePhase"], Value::Null);
    assert_eq!(health["deviceSynchronized"], Value::Null);
    assert_eq!(health["deviceObservedAtUnix"], Value::Null);
}

/// Deterministic reproduction of the flake in the test below.
///
/// `Gateway::request_detailed` publishes readiness after every RPC, so the
/// dozen tests that call `.request(rpc::RUNTIME_STATUS, ...)` all write the
/// process-wide cell - and none of them hold `READINESS_PUBLICATION`, whose
/// stated contract only covers tests that *observe* it. One of those
/// writes landing between a probe's publication and its contended read
/// answers the probe for somebody else's gateway.
///
/// The single `publish_gateway_readiness(false)` below stands in for that
/// write, at the exact instant the scheduler would otherwise have to
/// supply. Before the fix this failed every run; the race was only ever
/// timing deciding whether it happened.
#[test]
fn a_contended_probe_is_not_answered_by_another_gateways_publication() {
    let gateway = Mutex::new(Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    });
    let published = std::sync::atomic::AtomicBool::new(false);
    assert!(
        gateway_readiness_with(&gateway, &published),
        "an idle, connected gateway is ready"
    );

    // Another gateway, on another thread, reports itself gone.
    publish_gateway_readiness(false);

    assert!(
        readiness_while_another_thread_holds_the_lock(&gateway, &published),
        "a contended probe must answer for its own gateway, not for whichever one published last"
    );
}

#[test]
fn a_busy_gateway_is_not_reported_as_an_unready_device() {
    // Regression: readiness used a non-blocking lock read, so transient
    // contention could be reported as "device not ready" even while the
    // live connection remained healthy. Both the loopback health endpoint
    // and relay readiness must instead use the last authoritative state.
    let gateway = Mutex::new(Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    });
    // This gateway's own published cell, not the process-wide one. See
    // `gateway_readiness_with`: the shared cell is what made this test
    // flake, because a dozen other tests publish into it by making a
    // request, without ever mentioning readiness.
    let published = std::sync::atomic::AtomicBool::new(false);
    assert!(
        gateway_readiness_with(&gateway, &published),
        "an idle, connected gateway is ready"
    );
    assert!(
        readiness_while_another_thread_holds_the_lock(&gateway, &published),
        "a readiness probe must report the device, not whether a call happens to be in flight"
    );

    // The fallback reports the last published state - it is not a blanket
    // "yes". A gateway that is genuinely disconnected stays unready while
    // busy, which is the property that makes the signal worth anything.
    gateway.lock().expect("gateway lock").connected = false;
    assert!(
        !gateway_readiness_with(&gateway, &published),
        "a disconnected gateway is not ready"
    );
    assert!(
        !readiness_while_another_thread_holds_the_lock(&gateway, &published),
        "a busy gateway must not be reported ready when the device is gone"
    );
}

/// A standalone `--local-mcp-bridge` host has no broker to ask, so a busy
/// gateway answers from the published cell. A detached gateway must record
/// "not ready" there, as `gateway_readiness` always did, or a "ready" from
/// before the detach would answer the next busy read.
#[test]
fn a_detached_gateway_records_not_ready_for_a_later_busy_read() {
    let gateway = Mutex::new(Gateway {
        process: None,
        connected: true,
        ..Gateway::default()
    });
    let published = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(
        gateway_attachment_with(&gateway, &published),
        GatewayAttachment::Detached
    );
    assert!(!published.load(std::sync::atomic::Ordering::Acquire));
    let (busy_tx, busy_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let observed = std::thread::scope(|scope| {
        let gateway = &gateway;
        scope.spawn(move || {
            let _held = gateway.lock().expect("gateway lock");
            busy_tx.send(()).expect("held");
            let _ = release_rx.recv();
        });
        busy_rx.recv().expect("the lock is held");
        let observed = gateway_attachment_with(gateway, &published);
        release_tx.send(()).expect("release");
        observed
    });
    assert_eq!(observed, GatewayAttachment::Busy(false));
}

/// Probes readiness at the one moment that matters: while the gateway mutex
/// is genuinely held by another thread, as it is during every device RPC.
/// A mutex used to stand here, taken by "every test that observes
/// `PUBLISHED_READINESS`", so that those tests took turns over the one
/// atomic the whole process shares.
///
/// It could not work, and removing it is part of the fix rather than
/// tidying after it. The writes it needed to exclude do not come from
/// tests that observe readiness - they come from the dozen that call
/// `.request(rpc::RUNTIME_STATUS, ...)`, because `Gateway::request_detailed`
/// publishes after every RPC. Those tests never mention readiness, so they
/// never took the guard, and no contract written on the observing side
/// could make them. Widening it to "every test that publishes" would also
/// have deadlocked: an observing test holds it across its whole body and
/// then calls the very function that would take it again, and
/// `std::sync::Mutex` is not reentrant.
///
/// `gateway_readiness_with` removes the shared cell instead, so there is
/// nothing left to take turns over. If a future test needs the fallback,
/// give it its own `AtomicBool` - do not reintroduce a guard here.
fn readiness_while_another_thread_holds_the_lock(
    gateway: &Mutex<Gateway>,
    published: &std::sync::atomic::AtomicBool,
) -> bool {
    let (busy_tx, busy_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let _held = gateway.lock().expect("gateway lock");
            busy_tx.send(()).expect("signal that the lock is held");
            release_rx.recv().expect("wait for the readiness probe");
        });
        busy_rx.recv().expect("the lock is held");
        assert!(
            gateway.try_lock().is_err(),
            "the scenario under test requires a genuinely contended lock"
        );
        let observed = gateway_readiness_with(gateway, published);
        release_tx.send(()).expect("release the lock");
        observed
    })
}

/// A scratch AppData root of this test's own, and the guard that removes it
/// when the test ends or unwinds.
fn temp_app_data_dir(name: &str) -> (RemoveDirOnDrop, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "qc-appdata-test-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    fs::create_dir_all(&path).expect("temp AppData root");
    (RemoveDirOnDrop::new(path.clone()), path)
}

#[test]
fn a_single_broker_installation_is_not_reported_as_a_duplicate() {
    let (_cleanup, root) = temp_app_data_dir("single");
    fs::create_dir_all(root.join("QC Remote")).unwrap();
    fs::write(root.join("QC Remote").join("qc-device-broker.exe"), b"exe").unwrap();

    // One installation is found (it exists), but it is not itself a
    // duplicate - only the health document's ">1" gate below matters for
    // the ordinary, healthy case.
    assert_eq!(detect_duplicate_broker_installations(&root).len(), 1);

    let gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    let health = runtime_health_document(
        &gateway,
        "device.snapshot",
        "ok",
        None,
        &detect_duplicate_broker_installations(&root),
    );
    assert!(health.get("duplicateBrokerInstallations").is_none());
}

#[test]
fn a_leftover_qc_control_broker_alongside_the_current_one_is_detected_and_reported() {
    // The default AppData directory changed from `QC Control` to
    // `QC Remote`, leaving the old one as a read
    // fallback for the flight recorder. A machine that still has its own
    // `qc-device-broker.exe` under the legacy directory has two
    // independently launchable brokers - the same hazard as a stale
    // broker process - and nothing surfaced that fact.
    let (_cleanup, root) = temp_app_data_dir("duplicate");
    fs::create_dir_all(root.join("QC Remote")).unwrap();
    fs::create_dir_all(root.join("QC Control")).unwrap();
    fs::write(
        root.join("QC Remote").join("qc-device-broker.exe"),
        b"current",
    )
    .unwrap();
    fs::write(
        root.join("QC Control").join("qc-device-broker.exe"),
        b"legacy",
    )
    .unwrap();

    let found = detect_duplicate_broker_installations(&root);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found
        .iter()
        .any(|entry| entry.path.contains("QC Remote") && entry.modified_unix_ms.is_some()));
    assert!(found
        .iter()
        .any(|entry| entry.path.contains("QC Control") && entry.modified_unix_ms.is_some()));

    let gateway = Gateway {
        process: None,
        started_once: false,
        connected: true,
        device: DeviceSession::default(),
        event_tx: None,
        ..Gateway::default()
    };
    let health = runtime_health_document(&gateway, "device.snapshot", "ok", None, &found);
    let reported = health["duplicateBrokerInstallations"]
        .as_array()
        .expect("duplicate installations are reported");
    assert_eq!(reported.len(), 2);
    // Nothing is deleted or moved - detection is read-only.
    assert!(root
        .join("QC Remote")
        .join("qc-device-broker.exe")
        .is_file());
    assert!(root
        .join("QC Control")
        .join("qc-device-broker.exe")
        .is_file());
}

/// Hold an OS file lock for the entire real-broker fixture lifetime.
/// The file remains after release; no timestamp or PID grants unlink authority.
const REAL_BROKER_TEST_LOCK_WAIT_BUDGET: Duration = Duration::from_secs(900);

pub(crate) struct RealBrokerTestLock {
    _file: fs::File,
}

fn try_real_broker_test_lock(path: &Path) -> Result<Option<RealBrokerTestLock>, std::io::Error> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Read/write sharing, but no DELETE sharing: an old mtime sweeper
        // cannot unlink the lock file while a current owner has it open.
        options.share_mode(0x1 | 0x2);
    }
    let file = options.open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(RealBrokerTestLock { _file: file })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

pub(crate) fn serialize_real_broker_test() -> RealBrokerTestLock {
    let path = std::env::temp_dir().join("qc-shared-broker-test.lock");
    let started = Instant::now();
    let deadline = started + REAL_BROKER_TEST_LOCK_WAIT_BUDGET;
    let mut announced = false;
    loop {
        match try_real_broker_test_lock(&path).expect("open/lock broker test owner") {
            Some(owner) => return owner,
            None => {
                assert!(
                    Instant::now() < deadline,
                    "real broker fixture lock timed out"
                );
                if !announced && started.elapsed() >= Duration::from_secs(5) {
                    eprintln!(
                        "waiting on {}: another real-broker fixture owns the OS lock",
                        path.display()
                    );
                    announced = true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[test]
fn real_broker_test_lock_excludes_another_handle_and_releases_on_unwind() {
    with_isolation_fixture("lock-control", |directory| {
        let path = directory.join("owner.lock");
        let owner = try_real_broker_test_lock(&path)
            .unwrap()
            .expect("first owner");
        assert!(try_real_broker_test_lock(&path).unwrap().is_none());
        drop(owner);
        let panicked = std::panic::catch_unwind(|| {
            let _owner = try_real_broker_test_lock(&path)
                .unwrap()
                .expect("after release");
            assert!(try_real_broker_test_lock(&path).unwrap().is_none());
            panic!("fixture unwind");
        });
        assert!(panicked.is_err());
        let final_owner = try_real_broker_test_lock(&path)
            .unwrap()
            .expect("after unwind");
        assert!(path.is_file(), "release never unlinks a shared lock name");
        drop(final_owner);
    });
}

/// Allocates a new directory without deleting anything from an earlier run.
pub(crate) fn unique_fixture_directory(name: &str) -> PathBuf {
    let parent =
        fs::canonicalize(std::env::temp_dir()).expect("canonicalize system temporary directory");
    unique_fixture_directory_in(&parent, name)
}

fn unique_fixture_directory_in(parent: &Path, name: &str) -> PathBuf {
    unique_fixture_directory_with_suffix(parent, name, rand::random::<u128>)
}

fn unique_fixture_directory_with_suffix(
    parent: &Path,
    name: &str,
    mut suffix: impl FnMut() -> u128,
) -> PathBuf {
    assert!(!name.is_empty());
    assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    for ancestor in parent.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).expect("fixture ancestor exists");
        assert!(
            !metadata.file_type().is_symlink(),
            "fixture symlink ancestor"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            assert_eq!(
                metadata.file_attributes() & 0x400,
                0,
                "fixture reparse ancestor"
            );
        }
    }
    for _ in 0..16 {
        let directory = parent.join(format!("qc-owned-{name}-{:032x}", suffix()));
        match create_private_fixture_directory(&directory) {
            Ok(()) => return directory,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create owned fixture: {error}"),
        }
    }
    panic!("unique fixture allocation exhausted");
}

#[cfg(unix)]
fn create_private_fixture_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    // Apply the owner-only mode during creation so no process can observe a
    // less restrictive intermediate directory.
    builder.mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_private_fixture_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)
}

fn test_descriptor_path(name: &str) -> PathBuf {
    device_broker_descriptor::device_broker_descriptor_path(&unique_fixture_directory(name))
}

// Only new regression fixtures use this bounded owner. Existing real-broker
// teardown remains a separate responsibility; allocation never deletes a path.
fn cleanup_isolation_fixture(
    path: &Path,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), String> {
    if depth > 4 || *remaining == 0 {
        return Err("fixture cleanup bound exceeded".into());
    }
    *remaining -= 1;
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("fixture cleanup refuses symlink".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("fixture cleanup refuses reparse point".into());
        }
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            cleanup_isolation_fixture(
                &entry.map_err(|e| e.to_string())?.path(),
                depth + 1,
                remaining,
            )?;
        }
        fs::remove_dir(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

fn with_isolation_fixture(name: &str, body: impl FnOnce(&Path)) {
    // Successful create-new allocation establishes ownership before body runs.
    let directory = unique_fixture_directory(name);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&directory)));
    let cleanup = cleanup_isolation_fixture(&directory, 0, &mut 64);
    if let Err(primary) = result {
        if let Err(error) = cleanup {
            eprintln!(
                "owned fixture cleanup also failed for {}: {error}",
                directory.display()
            );
        }
        std::panic::resume_unwind(primary);
    }
    cleanup.unwrap_or_else(|error| {
        panic!(
            "owned fixture cleanup failed for {}: {error}",
            directory.display()
        )
    });
}

#[test]
fn fixture_allocation_preserves_prior_bytes_and_never_reuses_a_name() {
    with_isolation_fixture("allocation-control", |parent| {
        let retained = parent.join(format!("qc-shared-broker-test-same-{}", std::process::id()));
        fs::create_dir(&retained).unwrap();
        fs::write(retained.join("sentinel"), b"previous owned evidence").unwrap();
        let collision = parent.join(format!("qc-owned-same-{:032x}", 1_u128));
        fs::create_dir(&collision).unwrap();
        fs::write(collision.join("sentinel"), b"collision evidence").unwrap();
        let mut suffixes = [1_u128, 2_u128].into_iter();
        let first =
            unique_fixture_directory_with_suffix(parent, "same", || suffixes.next().unwrap());
        let second = unique_fixture_directory_in(parent, "same");
        assert_eq!(first, parent.join(format!("qc-owned-same-{:032x}", 2_u128)));
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert_eq!(
            fs::read(retained.join("sentinel")).unwrap(),
            b"previous owned evidence"
        );
        assert_eq!(
            fs::read(collision.join("sentinel")).unwrap(),
            b"collision evidence"
        );
    });
}

#[cfg(unix)]
#[test]
fn owned_linux_broker_fixture_directories_match_production_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = unique_fixture_directory("owner-mode");
    let _cleanup = RemoveDirOnDrop::new(directory.clone());
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700,
        "broker test descriptors are rejected unless their parent is owner-only"
    );
}

#[test]
fn isolation_fixture_cleanup_preserves_primary_panic_and_removes_owned_bytes() {
    let mut owned = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_isolation_fixture("unwind-control", |directory| {
            owned = Some(directory.to_path_buf());
            fs::write(directory.join("owned"), b"temporary").unwrap();
            panic!("primary fixture assertion");
        });
    }));
    let error = result.expect_err("injected assertion must fail");
    assert_eq!(
        error.downcast_ref::<&str>(),
        Some(&"primary fixture assertion")
    );
    assert!(!owned.expect("body executed").exists());
}

#[cfg(windows)]
#[test]
#[ignore = "requires the dedicated owned-junction release control; run --exact --ignored"]
fn fixture_allocation_rejects_reparse_ancestor() {
    use sha2::{Digest, Sha256};
    use std::os::windows::fs::MetadataExt;
    let path = PathBuf::from(
        std::env::var("QC_ISOLATION_REPARSE_MANIFEST").expect("owned manifest required"),
    );
    let bytes = fs::read(&path).expect("read owned manifest");
    let expected = std::env::var("QC_ISOLATION_REPARSE_SHA256").expect("manifest digest required");
    assert_eq!(format!("{:X}", Sha256::digest(&bytes)), expected);
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(manifest["schema"], "owned-reparse-control/1");
    assert_eq!(manifest["token"].as_str().unwrap().len(), 32);
    let root = PathBuf::from(manifest["root"].as_str().unwrap());
    let target = PathBuf::from(manifest["target"].as_str().unwrap());
    let link = PathBuf::from(manifest["link"].as_str().unwrap());
    assert_eq!(path, root.join("manifest.json"));
    assert_eq!(target, root.join("target"));
    assert_eq!(link, root.join("link"));
    assert_ne!(
        fs::symlink_metadata(&link).unwrap().file_attributes() & 0x400,
        0
    );
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(&target).unwrap()
    );
    fn entries(path: &Path) -> Vec<String> {
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        entries
    }
    assert_eq!(entries(&target), vec!["nested", "sentinel"]);
    assert!(entries(&target.join("nested")).is_empty());
    assert_eq!(
        fs::read(target.join("sentinel")).unwrap(),
        b"owned-junction-sentinel"
    );
    // The leaf parent exists normally; its ancestor is the actual junction.
    let result = std::panic::catch_unwind(|| {
        unique_fixture_directory_in(&link.join("nested"), "reparse-control")
    });
    let error = result.expect_err("reparse ancestor must refuse allocation");
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("fixture symlink ancestor")
            || message.contains("fixture reparse ancestor"),
        "wrong refusal: {message}"
    );
    assert_eq!(entries(&target), vec!["nested", "sentinel"]);
    assert!(entries(&target.join("nested")).is_empty());
    assert_eq!(
        fs::read(target.join("sentinel")).unwrap(),
        b"owned-junction-sentinel"
    );
    with_isolation_fixture("reparse-positive", |normal| {
        let created = unique_fixture_directory_in(normal, "allowed");
        assert!(created.is_dir());
        assert_eq!(created.parent(), Some(normal));
    });
}

/// Asks the OS, not our own bookkeeping, whether a process still exists.
fn process_is_alive(pid: u32) -> bool {
    os_process_is_alive(pid)
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    condition()
}

/// Wait for an owned broker child to exit and reap it through its `Child`
/// handle. On timeout, stop only this test's broker before returning failure.
fn wait_for_broker_child_exit(child: &mut std::process::Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

#[test]
fn a_second_gateway_process_attaches_to_the_first_instead_of_spawning_a_new_broker() {
    let _serialize = serialize_real_broker_test();
    let descriptor_path = test_descriptor_path("second-attaches");
    let mut first = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("the first process becomes the broker");
    let first_pid = first
        .hosted_broker_pid()
        .expect("the first process hosts the broker");

    let second = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("the second process attaches");
    assert!(
        second.hosted_broker_pid().is_none(),
        "the second process must not spawn its own broker"
    );
    let published = device_broker_descriptor::read_descriptor(
        &descriptor_path,
        device_broker_descriptor::unix_time_ms(),
    )
    .expect("the hosted broker is published");
    assert_eq!(
        published.process_id, first_pid,
        "the published broker is the one the first process spawned"
    );
    // The broker binary's own record of its creation time is the value
    // this app re-derives for that pid - the two crates agree on it.
    assert_eq!(
        published.process_created_at,
        os_process_created_at(first_pid),
        "the broker publishes the creation time its pid reports"
    );
    assert!(published_broker_process_is_alive(&published));

    // both can independently round-trip a request against the SAME broker process
    let mut first = first;
    let mut second = second;
    assert!(first.request(rpc::RUNTIME_STATUS, json!({})).is_ok());
    assert!(second.request(rpc::RUNTIME_STATUS, json!({})).is_ok());

    drop(second);
    // the broker process (owned by `first`) must still be alive and answering
    // after the attached-only client drops
    assert!(first.request(rpc::RUNTIME_STATUS, json!({})).is_ok());

    let mut broker_child = first
        .take_hosted_broker_child()
        .expect("the first gateway owns the broker child");
    drop(first);
    // Nobody kills the broker, not even the process that spawned it: with
    // its last client gone it exits on its own after its 5 s grace.
    let exited = wait_for_broker_child_exit(&mut broker_child, Duration::from_secs(12));
    assert!(
        exited,
        "with every client gone, the broker ends through its own grace period"
    );
    // Only now is the broker known gone; a failure above leaves the
    // directory, which it may still be using.
    let _ = remove_dir_with_retries(descriptor_path.parent().unwrap());
}

#[test]
fn dropping_an_attached_only_client_does_not_kill_the_hosting_process() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("attach-drop-safety");
    let first = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path).unwrap();
    let broker_pid = first.hosted_broker_pid().unwrap();

    let second = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path).unwrap();
    assert!(second.hosted_broker_pid().is_none());
    drop(second);

    // Give a wrongful kill every chance to have landed before looking.
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        process_is_alive(broker_pid),
        "the broker process must still be running after an attached-only client dropped"
    );
    drop(first);
}

/// The broker here is spawned by the test itself, so no `GatewayProcess`
/// hosts it and the only thing that can end it is its own zero-client
/// grace timer. That makes both halves of the attached-client drop
/// contract observable: the broker is still alive right after the drop
/// (it was not killed), and it then exits on its own (the drop really
/// closed the connection, including the reader thread's cloned handle).
#[test]
fn an_attached_client_drop_closes_its_connection_without_killing_the_broker() {
    let _serialize = serialize_real_broker_test();
    let descriptor_path = test_descriptor_path("attach-drop-closes");
    let mut broker = native_broker_command()
        .expect("a broker executable is available to the test")
        .arg("--broker-serve")
        .arg("--descriptor-path")
        .arg(&descriptor_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the broker starts");
    let broker_pid = broker.id();
    assert!(
        wait_until(Duration::from_secs(5), || {
            device_broker_descriptor::read_descriptor(
                &descriptor_path,
                device_broker_descriptor::unix_time_ms(),
            )
            .is_some()
        }),
        "the independently started broker publishes itself"
    );

    let mut attached = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("attaches to the running broker");
    assert!(attached.hosted_broker_pid().is_none());
    assert!(attached.request(rpc::RUNTIME_STATUS, json!({})).is_ok());
    drop(attached);

    std::thread::sleep(Duration::from_millis(500));
    let alive_after_drop = process_is_alive(broker_pid);
    // The broker's grace period is 5 s after its last client leaves.
    let exited_on_its_own = wait_until(Duration::from_secs(12), || {
        matches!(broker.try_wait(), Ok(Some(_)))
    });
    if !exited_on_its_own {
        let _ = broker.kill();
        let _ = broker.wait();
    }
    // Armed only once the broker is reaped: before that, a failure leaves
    // the directory to the broker that may still be using it.
    let _directory_guard = RemoveDirOnDrop::new(descriptor_path.parent().unwrap());
    assert!(
        alive_after_drop,
        "dropping an attached-only client must not kill the broker"
    );
    assert!(
        exited_on_its_own,
        "the broker must see the dropped client leave and reach its zero-client exit"
    );
    assert!(
        !descriptor_path.exists(),
        "the broker released its descriptor on the way out"
    );
}

#[test]
fn a_descriptor_left_by_a_dead_broker_is_replaced_by_a_new_host() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("dead-descriptor");
    // A fresh descriptor naming a port nothing listens on: the shape a
    // crashed or killed broker leaves behind for up to its staleness window.
    let unused_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut slot = device_broker_descriptor::try_claim_descriptor_slot(&descriptor_path).unwrap();
    write_broker_descriptor_fixture(
        &mut slot,
        &device_broker_descriptor::DeviceBrokerDescriptor {
            port: unused_port,
            bearer_token: "gone".into(),
            instance_id: "gone".into(),
            process_id: 1,
            process_created_at: None,
            published_at_unix_ms: device_broker_descriptor::unix_time_ms(),
            gateway_api_version: None,
        },
    );
    drop(slot);

    let mut process = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("a dead broker's descriptor does not block the election");
    assert!(
        process.hosted_broker_pid().is_some(),
        "with no live broker, this process hosts one"
    );
    assert!(process.request(rpc::RUNTIME_STATUS, json!({})).is_ok());
}

/// The CLI's `residentNotVisible` predicate, at the gateway: a session marker
/// with no attachable descriptor in this directory refuses new hosting, and is
/// rechecked at spawn admission after the election. No marker, or a live
/// descriptor here, hosts or attaches as before.
#[test]
fn a_session_marker_without_an_attachable_descriptor_refuses_new_hosting() {
    // A fresh random name, created exclusively: the guard only ever removes a
    // directory this test made, and removes it on a failed assertion too.
    let root = std::env::temp_dir().join(format!(
        "qc-hidden-resident-{:032x}",
        rand::random::<u128>()
    ));
    fs::create_dir(&root).expect("a scratch directory of this test's own");
    let _root_guard = RemoveDirOnDrop::new(root.clone());
    let descriptor_path = root.join("device-broker.json");
    let now = device_broker_descriptor::unix_time_ms();
    let live = |_: &device_broker_descriptor::DeviceBrokerDescriptor| true;
    assert_eq!(
        hidden_resident_refusal(&descriptor_path, false, now, &live),
        None,
        "no marker: hosting is allowed"
    );
    let refusal = hidden_resident_refusal(&descriptor_path, true, now, &live)
        .expect("a marker without a descriptor here refuses new hosting");
    assert!(
        refusal.starts_with("No new broker was started."),
        "{refusal}"
    );
    assert!(refusal.contains("Quit QC Remote"), "{refusal}");
    write_broker_descriptor_fixture(
        &mut fs::File::create(&descriptor_path).unwrap(),
        &device_broker_descriptor::DeviceBrokerDescriptor {
            port: 1,
            bearer_token: "visible-broker-token".into(),
            instance_id: "visible".into(),
            process_id: 4242,
            process_created_at: Some(7),
            published_at_unix_ms: now,
            gateway_api_version: None,
        },
    );
    assert_eq!(
        hidden_resident_refusal(&descriptor_path, true, now, &live),
        None,
        "a marker and a live descriptor here: attach as before"
    );
    assert!(
        hidden_resident_refusal(&descriptor_path, true, now, &|_| false).is_some(),
        "a dead broker's descriptor is not attachable"
    );
    fs::remove_file(&descriptor_path).unwrap();
    // Absent at the check before the election, present by spawn admission.
    assert!(!session_marker_present(&descriptor_path));
    PINNED_SESSION_MARKERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((descriptor_path.clone(), true));
    let _unpin = RunOnDrop::new(|| {
        PINNED_SESSION_MARKERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(path, _)| path != &descriptor_path)
    });
    let _serialize = serialize_real_broker_test();
    match host_device_broker(&descriptor_path, false) {
        Err(error) => assert_eq!(error, refusal),
        Ok(_) => panic!("a broker was admitted while the session marker is held"),
    }
    assert!(!descriptor_path.exists(), "nothing was published");
}

/// A user's Quit, recorded by the broker that was quit, refuses
/// new hosting (election and spawn admission) until a user action clears it.
/// The record is bound to the quit broker's instance, never to a time: the
/// quit generation's own descriptor is not attachable, a live broker under
/// another instance id is (attaching starts nothing), a dead one is not. An
/// unreadable record stands. No process is started: both refusals come
/// before any spawn, and the serialisation guards the real-broker path as in
/// the session-marker test above.
#[test]
fn a_recorded_users_quit_refuses_new_hosting_until_a_user_action() {
    let root = std::env::temp_dir().join(format!("qc-user-quit-{:032x}", rand::random::<u128>()));
    fs::create_dir(&root).expect("a scratch directory of this test's own");
    let _root_guard = RemoveDirOnDrop::new(root.clone());
    let descriptor_path = root.join("device-broker.json");
    let record_path = device_broker_descriptor::user_quit_path(&descriptor_path);
    let now = device_broker_descriptor::unix_time_ms();
    let live = |_: &device_broker_descriptor::DeviceBrokerDescriptor| true;
    let dead = |_: &device_broker_descriptor::DeviceBrokerDescriptor| false;
    let publish = |instance: &str| {
        write_broker_descriptor_fixture(
            &mut fs::File::create(&descriptor_path).unwrap(),
            &device_broker_descriptor::DeviceBrokerDescriptor {
                port: 1,
                bearer_token: "user-quit-token".into(),
                instance_id: instance.into(),
                process_id: 4242,
                process_created_at: Some(7),
                published_at_unix_ms: now,
                gateway_api_version: None,
            },
        )
    };

    assert_eq!(
        user_quit_refusal_at(&descriptor_path, now, &live),
        None,
        "first use: no Quit recorded, hosting is allowed"
    );
    fs::write(
        &record_path,
        br#"{"instanceId":"quit-generation","source":"tray"}"#,
    )
    .unwrap();
    let refusal = user_quit_refusal_at(&descriptor_path, now, &live)
        .expect("a recorded Quit with nothing attachable refuses new hosting");
    assert_eq!(refusal, local_stdio::POST_QUIT_ADVICE);
    assert!(refusal.starts_with("QC Remote was quit."), "{refusal}");
    assert!(!refusal.contains("from its icon"), "{refusal}");

    publish("quit-generation");
    assert_eq!(
        user_quit_refusal_at(&descriptor_path, now, &live).as_deref(),
        Some(local_stdio::POST_QUIT_ADVICE),
        "the quit generation on its way out is not attachable"
    );
    publish("newer-generation");
    assert_eq!(
        user_quit_refusal_at(&descriptor_path, now, &live),
        None,
        "a live broker under another instance id is attached; attaching starts nothing"
    );
    assert!(
        user_quit_refusal_at(&descriptor_path, now, &dead).is_some(),
        "a dead broker's descriptor is not attachable"
    );
    fs::remove_file(&descriptor_path).unwrap();

    fs::write(&record_path, b"not a record").unwrap();
    assert!(
        user_quit_refusal_at(&descriptor_path, now, &live).is_some(),
        "an unreadable record stands"
    );
    fs::write(
        &record_path,
        br#"{"instanceId":"quit-generation","source":"control"}"#,
    )
    .unwrap();

    // Automatic demand meets it at the election and at spawn admission.
    let _serialize = serialize_real_broker_test();
    match attach_or_host_device_broker(&descriptor_path, false) {
        Err(error) => assert_eq!(error, refusal),
        Ok(_) => panic!("a broker was elected while the user's Quit stands"),
    }
    match host_device_broker(&descriptor_path, false) {
        Err(error) => assert_eq!(error, refusal),
        Ok(_) => panic!("a broker was admitted while the user's Quit stands"),
    }
    assert!(!descriptor_path.exists(), "nothing was published");
    assert!(record_path.exists(), "automatic demand never clears it");

    // The user's own Connect, USB or device click (and an app launch)
    // supersedes it.
    assert_eq!(acknowledge_user_connect_in(&descriptor_path), Ok(true));
    assert!(!record_path.exists());
    assert_eq!(user_quit_refusal_at(&descriptor_path, now, &live), None);
    assert_eq!(acknowledge_user_connect_in(&descriptor_path), Ok(false));
}

/// An unreadable record names no generation, so it cannot tell a
/// newer broker from the quit one; it refuses even while a live, attachable
/// descriptor is published, and only a user action clears it.
#[test]
fn an_unreadable_user_quit_refuses_even_with_a_live_descriptor() {
    let root = std::env::temp_dir().join(format!("qc-user-quit-{:032x}", rand::random::<u128>()));
    fs::create_dir(&root).expect("a scratch directory of this test's own");
    let _root_guard = RemoveDirOnDrop::new(root.clone());
    let descriptor_path = root.join("device-broker.json");
    let now = device_broker_descriptor::unix_time_ms();
    let live = |_: &device_broker_descriptor::DeviceBrokerDescriptor| true;
    write_broker_descriptor_fixture(
        &mut fs::File::create(&descriptor_path).unwrap(),
        &device_broker_descriptor::DeviceBrokerDescriptor {
            port: 1,
            bearer_token: "user-quit-token".into(),
            instance_id: "live-generation".into(),
            process_id: 4242,
            process_created_at: Some(7),
            published_at_unix_ms: now,
            gateway_api_version: None,
        },
    );
    let record_path = device_broker_descriptor::user_quit_path(&descriptor_path);
    fs::write(&record_path, b"not a record").unwrap();
    assert_eq!(
        user_quit_refusal_at(&descriptor_path, now, &live).as_deref(),
        Some(local_stdio::POST_QUIT_ADVICE),
        "an unreadable record refuses although a live descriptor is attachable"
    );
    fs::write(
        &record_path,
        br#"{"instanceId":"older-quit","source":"tray"}"#,
    )
    .unwrap();
    assert_eq!(
        user_quit_refusal_at(&descriptor_path, now, &live),
        None,
        "a readable record of another generation does not block the live one"
    );
    fs::write(&record_path, b"not a record").unwrap();
    assert_eq!(acknowledge_user_connect_in(&descriptor_path), Ok(true));
    assert_eq!(user_quit_refusal_at(&descriptor_path, now, &live), None);
}

/// A user action's clear holds the election lock, as the broker's
/// record writer and every election do. While another process holds it, the
/// clear waits its bound and fails explicitly, the record standing; with the
/// lock free it clears and leaves no lock behind.
#[test]
fn a_users_clear_holds_the_election_lock_and_fails_explicitly_without_it() {
    let root = std::env::temp_dir().join(format!("qc-user-quit-{:032x}", rand::random::<u128>()));
    fs::create_dir(&root).expect("a scratch directory of this test's own");
    let _root_guard = RemoveDirOnDrop::new(root.clone());
    let descriptor_path = root.join("device-broker.json");
    let record_path = device_broker_descriptor::user_quit_path(&descriptor_path);
    fs::write(&record_path, br#"{"instanceId":"quit","source":"tray"}"#).unwrap();
    let election_path = device_broker_election_path(&descriptor_path);
    let election = device_broker_descriptor::try_claim_descriptor_slot(&election_path)
        .expect("this test holds the election");
    let error =
        acknowledge_user_connect_within(&descriptor_path, Duration::from_millis(60)).unwrap_err();
    assert!(error.contains("election lock"), "{error}");
    assert!(record_path.exists(), "the record stands");
    drop(election);
    device_broker_descriptor::release_descriptor_slot(&election_path);
    assert_eq!(acknowledge_user_connect_in(&descriptor_path), Ok(true));
    assert!(!record_path.exists());
    assert!(
        !election_path.exists(),
        "the clear released the election lock"
    );
}

/// One scripted run of `attach_or_elect_until`, on a fake clock that
/// only the injected sleeper advances.
struct ScriptedAttachRun {
    result: Result<&'static str, String>,
    /// Fake-clock offset at each attach attempt.
    attach_times: Vec<Duration>,
    elections: usize,
    sleeps: Vec<Duration>,
    elapsed: Duration,
}

/// Once the script runs out every further attach answers `Unresponsive`,
/// which never elects, so a run ends only by attaching, by electing or at
/// the deadline.
const SCRIPT_EXHAUSTED: &str = "scripted broker still unresponsive";

fn run_scripted_attach(
    script: Vec<Option<Result<&'static str, BrokerAttachFailure>>>,
    budget: Duration,
) -> ScriptedAttachRun {
    let base = Instant::now();
    let clock = std::cell::Cell::new(Duration::ZERO);
    let script = std::cell::RefCell::new(std::collections::VecDeque::from(script));
    let mut attach_times = Vec::new();
    let mut elections = 0;
    let mut sleeps = Vec::new();
    let result = attach_or_elect_until(
        base + budget,
        true,
        || {
            attach_times.push(clock.get());
            script.borrow_mut().pop_front().unwrap_or_else(|| {
                Some(Err(BrokerAttachFailure::Unresponsive(
                    SCRIPT_EXHAUSTED.into(),
                )))
            })
        },
        |_last_error| {
            elections += 1;
            Ok(Some("hosted"))
        },
        || base + clock.get(),
        |duration| {
            sleeps.push(duration);
            clock.set(clock.get() + duration);
        },
    );
    ScriptedAttachRun {
        result,
        attach_times,
        elections,
        sleeps,
        elapsed: clock.get(),
    }
}

fn gone() -> Option<Result<&'static str, BrokerAttachFailure>> {
    Some(Err(BrokerAttachFailure::Gone("connection refused".into())))
}

#[test]
fn a_gone_verdict_that_recovers_on_recheck_attaches_without_electing() {
    let run = run_scripted_attach(vec![gone(), Some(Ok("attached"))], Duration::from_secs(3));
    assert_eq!(run.result, Ok("attached"));
    assert_eq!(run.elections, 0);
    assert_eq!(run.attach_times.len(), 2);
}

#[test]
fn a_gone_verdict_confirmed_on_recheck_elects_exactly_once() {
    let run = run_scripted_attach(vec![gone(), gone()], Duration::from_secs(3));
    assert_eq!(run.result, Ok("hosted"));
    assert_eq!(run.elections, 1);
    assert_eq!(run.attach_times.len(), 2);
}

#[test]
fn a_gone_verdict_that_turns_unresponsive_keeps_polling_without_electing() {
    let budget = Duration::from_secs(1);
    let run = run_scripted_attach(
        vec![
            gone(),
            Some(Err(BrokerAttachFailure::Unresponsive(
                "handshake timed out".into(),
            ))),
        ],
        budget,
    );
    assert_eq!(run.elections, 0);
    let error = run.result.unwrap_err();
    assert!(error.contains(SCRIPT_EXHAUSTED), "{error}");
    // It went back to polling at the poll interval until the budget ran
    // out, and stopped there.
    assert!(run.attach_times.len() > 2);
    assert!(run.sleeps[1..]
        .iter()
        .all(|sleep| *sleep == DEVICE_BROKER_POLL_INTERVAL));
    assert!(run.elapsed >= budget);
    assert!(run.elapsed < budget + DEVICE_BROKER_POLL_INTERVAL);
}

#[test]
fn a_gone_verdict_whose_descriptor_vanishes_on_recheck_does_not_elect() {
    let run = run_scripted_attach(vec![gone(), None], Duration::from_secs(1));
    assert_eq!(run.elections, 0);
    assert!(run.result.is_err());
    // The pass after the re-check polled again instead of electing.
    assert!(run.attach_times.len() > 2);
    assert_eq!(
        run.attach_times[2] - run.attach_times[1],
        DEVICE_BROKER_POLL_INTERVAL
    );
}

#[test]
fn the_gone_recheck_waits_at_least_the_confirmation_delay() {
    let run = run_scripted_attach(vec![gone(), Some(Ok("attached"))], Duration::from_secs(3));
    assert_eq!(run.sleeps, vec![DEVICE_BROKER_GONE_CONFIRMATION_DELAY]);
    assert!(run.attach_times[1] - run.attach_times[0] >= DEVICE_BROKER_GONE_CONFIRMATION_DELAY);
}

/// A live broker's listener can, in principle, refuse a single connection
/// under genuine, severe machine load without the process actually being
/// dead. This proves the end-to-end recovery path: a `Gone` verdict
/// against a broker that only refused once, then answered normally
/// moments later, ends in a clean attach, not a replacement - simulated
/// by closing the listener the descriptor names and reopening a working
/// one on the identical port shortly after, so the first attempt is a
/// genuine `ConnectionRefused` and a later one reaches a real, answering
/// broker.
///
/// This is defense in depth, not a standalone regression test for the
/// `DEVICE_BROKER_GONE_CONFIRMATION_DELAY` re-check alone. The hosting
/// path already retries an occupied-looking descriptor before deletion;
/// the confirmation delay adds an independent margin before replacement.
///
/// On Windows the first connect usually times out rather than being
/// refused (a closed loopback port is retried for about two seconds), and
/// a live published pid turns that timeout into `Unresponsive`, so this
/// test rarely reaches the `Gone` confirmation branch there.
#[test]
fn a_broker_refusing_one_connection_transiently_is_not_replaced() {
    let _serialize = serialize_real_broker_test();
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("transient-refusal");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    // The listener above is already dropped: nothing listens on `port`
    // yet, so a connect attempt right now is a genuine ConnectionRefused.
    let instance_id = "recovers";
    let bearer_token = "recovers-token";
    let mut slot = device_broker_descriptor::try_claim_descriptor_slot(&descriptor_path).unwrap();
    write_broker_descriptor_fixture(
        &mut slot,
        &device_broker_descriptor::DeviceBrokerDescriptor {
            port,
            bearer_token: bearer_token.into(),
            instance_id: instance_id.into(),
            process_id: std::process::id(),
            process_created_at: Some(own_process_created_at()),
            published_at_unix_ms: device_broker_descriptor::unix_time_ms(),
            gateway_api_version: None,
        },
    );
    drop(slot);

    let recovered = std::thread::spawn(move || {
        // Long enough that the first attach attempt's connect has
        // already failed, short enough to land well inside the
        // confirming re-check's own connect timeout.
        std::thread::sleep(Duration::from_millis(150));
        let listener =
            std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
        // On Windows a connect to a closed loopback port keeps retrying
        // its SYN for about two seconds, so the client's first attempt
        // can be completed by this listener at the instant it gives up
        // at its own connect timeout. That connection is accepted here
        // but already closed by the client. Keep accepting until one
        // connection actually delivers the handshake, within a bound.
        listener.set_nonblocking(true).unwrap();
        let accept_deadline = Instant::now() + Duration::from_secs(10);
        let (mut stream, request) = loop {
            assert!(
                Instant::now() < accept_deadline,
                "no connection delivered the authenticate frame in time"
            );
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("the recovered listener failed to accept: {error}"),
            };
            // An accepted socket inherits the listener's non-blocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            // A connection the client already abandoned ends (EOF or
            // reset) before a full frame arrives; skip it.
            if let Ok(request) = read_frame(&mut stream) {
                break (stream, request);
            }
        };
        assert_eq!(
            request.get("method").and_then(Value::as_str),
            Some(DEVICE_BROKER_AUTHENTICATE_METHOD)
        );
        let response = serde_json::to_vec(&json!({
            "id": 0,
            "result": {"instanceId": instance_id}
        }))
        .unwrap();
        write_frame(&mut stream, &response).unwrap();
        // A successful attach immediately follows with a real
        // system.status round trip (GatewayProcess::verify_gateway_api_version),
        // which must be answered too or the client reads this as a
        // transport failure right after attaching.
        let status_request = read_frame(&mut stream).unwrap();
        let status_response = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": status_request.get("id").cloned().unwrap_or(json!(1)),
            "result": {"gatewayApiVersion": generated_gateway::API_VERSION}
        }))
        .unwrap();
        write_frame(&mut stream, &status_response).unwrap();
        // Held so the attached client's connection stays open for the
        // rest of the test instead of being read as a transport failure.
        std::thread::sleep(Duration::from_secs(2));
    });

    let process = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("the confirming re-check reaches the recovered broker");
    assert!(
        process.hosted_broker_pid().is_none(),
        "a broker that only refused once, then answered, must not be replaced"
    );
    drop(process);
    recovered.join().unwrap();
}

#[test]
fn processes_started_together_converge_on_one_broker() {
    let _serialize = serialize_real_broker_test();
    // `default_broker_executable` compiles the test broker on first use,
    // behind a process-wide `OnceLock`. Left implicit, whichever of the
    // four contenders below happens to win the election pays that cold
    // compile inside its own election budget. A cold build can exceed the
    // election timeout, letting the winner lose to a rival's shorter wait
    // before it can publish. Forcing the build here,
    // before the timed race starts, means the race measures only the
    // real runtime behaviour this test exists to check.
    default_broker_executable().expect("the test broker builds");
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("converge");
    let processes: Vec<GatewayProcess> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
                        .expect("every contender ends up attached")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let hosts = processes
        .iter()
        .filter(|process| process.hosted_broker_pid().is_some())
        .count();
    assert_eq!(hosts, 1, "exactly one contender hosts the broker");
}

/// Close this process's own connection to the broker, the way a socket
/// error or a timed-out call leaves it, without touching the broker.
fn break_own_connection(process: &GatewayProcess) {
    let GatewayTransport::SharedBroker { stream, .. } = &process.transport else {
        panic!("a shared-broker process");
    };
    stream.shutdown(Shutdown::Both).unwrap();
}

/// The recovery path `Gateway::request_detailed` takes on any
/// `GatewayRequestFailure::Transport`, and `Gateway::restart` takes on
/// every session reset: drop the process, start a new one. In the window
/// that hosts the broker, that drop must not take the broker - and every
/// other window's device session - with it.
#[test]
fn a_hosting_process_dropped_after_a_transport_failure_leaves_the_shared_broker_running() {
    let _serialize = serialize_real_broker_test();
    let descriptor_path = test_descriptor_path("host-transport-failure");
    let mut host = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("the first process hosts the broker");
    let broker_pid = host
        .hosted_broker_pid()
        .expect("the first process hosts the broker");
    let mut broker_child = host
        .take_hosted_broker_child()
        .expect("the hosting gateway owns the broker child");
    let mut other = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("another window attaches");
    assert!(other.hosted_broker_pid().is_none());
    assert!(other.request(rpc::RUNTIME_STATUS, json!({})).is_ok());

    // The hosting window's own connection fails, which says nothing about
    // the broker...
    break_own_connection(&host);
    assert!(matches!(
        host.request(rpc::RUNTIME_STATUS, json!({})),
        Err(GatewayRequestFailure::Transport(_))
    ));
    // ...and the recovery path drops its process (`self.process = None`).
    drop(host);

    // Give a wrongful kill every chance to have landed before looking.
    std::thread::sleep(Duration::from_millis(500));
    let alive_after_drop = process_is_alive(broker_pid);
    let other_still_served = other.request(rpc::RUNTIME_STATUS, json!({})).is_ok();
    // The recovery's next start re-attaches to the very same broker.
    let recovered = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path);
    let recovered_attached_to = recovered
        .as_ref()
        .ok()
        .filter(|process| process.hosted_broker_pid().is_none())
        .map(|process| process.broker_pid);
    drop(recovered);
    drop(other);
    let exited_on_its_own = wait_for_broker_child_exit(&mut broker_child, Duration::from_secs(12));
    // Armed only once the broker is seen gone: otherwise the directory is
    // left to the broker that may still be using it.
    let _directory_guard =
        exited_on_its_own.then(|| RemoveDirOnDrop::new(descriptor_path.parent().unwrap()));

    assert!(
        alive_after_drop,
        "dropping the hosting process on a transport failure must not kill the broker"
    );
    assert!(
        other_still_served,
        "a window attached to the same broker keeps being served"
    );
    assert_eq!(
        recovered_attached_to,
        Some(broker_pid),
        "the recovering window re-attaches to the broker it spawned, not a new one"
    );
    assert!(
        exited_on_its_own,
        "with every client gone, the broker ends through its own grace period"
    );
}

/// A published descriptor for a broker that accepts the connection and
/// never answers the handshake, naming `process_id` and the creation time
/// recorded for it.
fn publish_silent_broker(
    descriptor_path: &Path,
    process_id: u32,
    process_created_at: Option<u64>,
    published_at_unix_ms: u64,
) -> std::net::TcpListener {
    // Never accepted from: the OS completes the connect, the handshake
    // frame sits in the buffer, and no answer ever comes.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut slot = device_broker_descriptor::try_claim_descriptor_slot(descriptor_path).unwrap();
    write_broker_descriptor_fixture(
        &mut slot,
        &device_broker_descriptor::DeviceBrokerDescriptor {
            port: listener.local_addr().unwrap().port(),
            bearer_token: "silent".into(),
            instance_id: "silent".into(),
            process_id,
            process_created_at,
            published_at_unix_ms,
            gateway_api_version: None,
        },
    );
    listener
}

#[test]
fn an_idle_reopen_failure_reads_as_not_connected_wherever_text_is_matched() {
    // services/device-broker/src/rpc.rs idle_reopen_error: the stable
    // signal is data.category/retryable; the text says it too.
    for message in [
        "The Quad Cortex is not connected: device unplugged",
        "The Quad Cortex is not connected: its session, closed for idleness, did not reopen within 35 seconds; retry",
    ] {
        let error = relay_device_error(message.into());
        assert_eq!(error.code, "NOT_CONNECTED", "{message}");
        assert!(error.retryable, "{message}");
    }
}

#[test]
fn a_missing_sidecar_fails_closed_and_a_same_name_fallback_is_never_chosen_unverified() {
    let root =
        std::env::temp_dir().join(format!("qc-locate-broker-{:032x}", rand::random::<u128>()));
    // On a failing run the checked removal at the end never runs, so this
    // removes the directory; on a passing run that removal goes first and
    // this finds the directory already gone.
    let _cleanup = RemoveDirOnDrop::new(root.clone());
    let installed = root.join("install");
    let fallback_dir = root.join("checkout");
    fs::create_dir_all(&installed).unwrap();
    fs::create_dir_all(&fallback_dir).unwrap();
    let broker_name = native_broker_filename();
    let fallback = fallback_dir.join(broker_name);
    // A broker-only or stale build of the same name: no marker.
    fs::write(&fallback, b"MZ broker-only build").unwrap();
    let fallbacks = [fallback.clone()];
    // Release: no sidecar, no fallback at all, whatever is there.
    assert_eq!(
        locate_native_broker_in(Some(&installed), &fallbacks, false),
        None
    );
    fs::write(
        &fallback,
        format!("MZ {} ", resident_host_build_marker()).as_bytes(),
    )
    .unwrap();
    assert_eq!(
        locate_native_broker_in(Some(&installed), &fallbacks, false),
        None,
        "even a genuine build is not used by a release"
    );
    // Debug: only a fallback that embeds this version's marker.
    assert_eq!(
        locate_native_broker_in(Some(&installed), &fallbacks, true),
        Some(fallback.clone())
    );
    for invalid in [
        b"MZ broker-only build".to_vec(),
        b"MZ qc-remote-resident-host-build:v1:0.0.1:end".to_vec(),
        Vec::new(),
    ] {
        fs::write(&fallback, invalid).unwrap();
        assert_eq!(
            locate_native_broker_in(Some(&installed), &fallbacks, true),
            None
        );
    }
    // The installed sidecar always wins, and is the only release choice.
    fs::write(installed.join(broker_name), b"installed").unwrap();
    assert_eq!(
        locate_native_broker_in(Some(&installed), &fallbacks, false),
        Some(installed.join(broker_name))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_brokers_own_process_never_elects_deletes_or_spawns_a_broker() {
    // qc-device-broker.exe hosts the MCP host in-process. Whether its
    // broker has not published yet or is exiting, the host's gateway only
    // waits for that descriptor: a second broker would be a second USB
    // owner and a second process.
    let directory =
        std::env::temp_dir().join(format!("qc-attach-only-{:032x}", rand::random::<u128>()));
    // On a failing run the checked removal at the end never runs, so this
    // removes the directory; on a passing run that removal goes first and
    // this finds the directory already gone.
    let _cleanup = RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).unwrap();
    let descriptor_path = directory.join("device-broker.json");
    let started = Instant::now();
    let error = match attach_or_host_device_broker_within(
        &descriptor_path,
        true,
        Duration::from_millis(400),
        false,
    ) {
        Ok((_, pid)) => panic!("attached to or spawned broker {pid}"),
        Err(error) => error,
    };
    assert!(started.elapsed() >= Duration::from_millis(400));
    assert!(error.contains("has not published"), "{error}");
    assert!(
        !descriptor_path.exists(),
        "nothing was spawned or published"
    );
    assert!(!device_broker_election_path(&descriptor_path).exists());

    // A dead broker's descriptor (pid 0 never names a live process) is
    // not this process's to delete either.
    fs::write(
        &descriptor_path,
        br#"{"port":1,"bearerToken":"x","instanceId":"x","processId":0,"publishedAtUnixMs":0}"#,
    )
    .unwrap();
    let before = fs::read(&descriptor_path).unwrap();
    assert!(attach_or_host_device_broker_within(
        &descriptor_path,
        true,
        Duration::from_millis(300),
        false
    )
    .is_err());
    assert_eq!(fs::read(&descriptor_path).unwrap(), before);
    assert!(!device_broker_election_path(&descriptor_path).exists());
    fs::remove_dir_all(directory).unwrap();
}

fn assert_live_silent_broker_is_never_replaced(descriptor_path: &Path) {
    let before = fs::read(descriptor_path).unwrap();
    let started = Instant::now();
    let result =
        attach_or_host_device_broker_within(descriptor_path, false, Duration::from_secs(3), true);
    let error = match result {
        Ok((_, pid)) => panic!("attached to broker {pid}, but nothing could answer"),
        Err(error) => error,
    };
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "a live broker is waited for across the whole budget, not given up on: {error}"
    );
    assert_eq!(
        fs::read(descriptor_path).expect("the descriptor must not be deleted"),
        before,
        "a live broker's descriptor is neither deleted nor replaced by a second broker's"
    );
    assert!(
        !device_broker_election_path(descriptor_path).exists(),
        "the election lock is not left behind"
    );
}

#[test]
fn a_handshake_timeout_against_a_live_broker_never_replaces_it() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("live-silent-broker");
    // Backed by a live process, this test process, recorded exactly as a
    // real broker records itself.
    let _listener = publish_silent_broker(
        &descriptor_path,
        std::process::id(),
        Some(own_process_created_at()),
        device_broker_descriptor::unix_time_ms(),
    );
    assert_live_silent_broker_is_never_replaced(&descriptor_path);
}

/// An expired descriptor reads as absent to the attach loop, so this one
/// reaches the election - and must still be left alone by the winner,
/// because its process is running and its port does not refuse.
#[test]
fn an_expired_descriptor_for_a_live_unanswering_broker_is_never_replaced() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("live-silent-expired");
    let expired = device_broker_descriptor::unix_time_ms()
        - device_broker_descriptor::DEVICE_BROKER_DESCRIPTOR_STALE_AFTER.as_millis() as u64
        - 60_000;
    let _listener = publish_silent_broker(
        &descriptor_path,
        std::process::id(),
        Some(own_process_created_at()),
        expired,
    );
    assert!(
        device_broker_descriptor::read_descriptor(
            &descriptor_path,
            device_broker_descriptor::unix_time_ms()
        )
        .is_none(),
        "the descriptor is expired, so the attach loop enters the election"
    );
    assert_live_silent_broker_is_never_replaced(&descriptor_path);
}

/// The control for the two tests above: the same silent port, but its
/// process is gone, so the election winner may replace it.
#[test]
fn a_handshake_timeout_against_a_dead_process_is_replaced() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("dead-silent-broker");
    let dead_pid = exited_process_id();
    let _listener = publish_silent_broker(
        &descriptor_path,
        dead_pid,
        None,
        device_broker_descriptor::unix_time_ms(),
    );
    let mut process = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("a broker whose process is gone does not block the election");
    assert!(
        process.hosted_broker_pid().is_some(),
        "with its process gone, the silent broker is replaced by a new host"
    );
    assert!(process.request(rpc::RUNTIME_STATUS, json!({})).is_ok());
}

/// A broker that died without releasing its descriptor, whose pid Windows
/// has since handed to an unrelated live process. Stood in for by this
/// test process's own pid (alive) with a recorded creation time one tick
/// earlier than its real one - exactly what a recycled pid looks like:
/// the pid is live, but its process was created after the broker's.
#[test]
fn a_descriptor_whose_pid_was_recycled_by_another_process_is_replaced() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("recycled-pid-broker");
    let recycled_pid = std::process::id();
    let broker_created_at = own_process_created_at() - 1;
    let _listener = publish_silent_broker(
        &descriptor_path,
        recycled_pid,
        Some(broker_created_at),
        device_broker_descriptor::unix_time_ms(),
    );
    let descriptor = device_broker_descriptor::read_descriptor(
        &descriptor_path,
        device_broker_descriptor::unix_time_ms(),
    )
    .unwrap();
    assert!(
        os_process_is_alive(recycled_pid),
        "the pid itself is live, so a pid-only check would call the broker alive"
    );
    assert!(
        !published_broker_process_is_alive(&descriptor),
        "a live pid created after the recorded time is not the broker"
    );
    let mut process = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("a broker whose pid was recycled does not block the election");
    assert!(
        process.hosted_broker_pid().is_some(),
        "the recycled pid's descriptor is replaced by a new host"
    );
    assert!(process.request(rpc::RUNTIME_STATUS, json!({})).is_ok());
}

#[test]
fn a_creation_time_that_cannot_be_read_never_proves_a_broker_gone() {
    let alive = |_: u32| true;
    let unreadable = |_: u32| None;
    assert!(
        published_process_is_alive_with(42, Some(7), alive, unreadable),
        "an unreadable creation time falls back to the pid check, which says alive"
    );
    assert!(
        published_process_is_alive_with(42, None, alive, |_| Some(8)),
        "a descriptor that recorded no creation time falls back to the pid check"
    );
    assert!(
        published_process_is_alive_with(42, Some(7), alive, |_| Some(7)),
        "a matching creation time is the same process"
    );
    assert!(
        !published_process_is_alive_with(42, Some(7), alive, |_| Some(8)),
        "a live pid created at another time is a recycled pid: gone"
    );
    assert!(
        !published_process_is_alive_with(42, Some(7), |_| false, unreadable),
        "a pid that is gone is gone whatever the creation time"
    );
    // And against the real OS: pid 0 (the idle process) can never be
    // opened, so its creation time is unreadable - which alone is not
    // "gone" either.
    assert_eq!(os_process_created_at(0), None);
    assert!(published_process_is_alive_with(
        0,
        Some(7),
        alive,
        os_process_created_at
    ));
}

fn own_process_created_at() -> u64 {
    os_process_created_at(std::process::id()).expect("this process can read its own times")
}

#[cfg(windows)]
fn exited_process_id() -> u32 {
    let mut child = Command::new("cmd")
        .args(["/C", "exit 0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    // `child` is dropped here, closing the last handle to it.
    pid
}

#[cfg(unix)]
fn exited_process_id() -> u32 {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a short-lived child for the dead-process fixture");
    let pid = child.id();
    child.wait().unwrap();
    pid
}

// `os_process_is_alive`/`os_process_created_at` themselves are tested in
// `qc-windows-process`, where they're now defined; `own_process_created_at`/
// `exited_process_id` above stay here as helpers the broker-liveness
// tests below still need.

/// What keeps `GatewayProcess::start_owned_child` unreachable in
/// production: this must fall back to a real directory (never leave the
/// descriptor path unset) so `Gateway::shared_descriptor_path` always has
/// something to return. See `start_owned_child`'s own doc comment.
#[test]
fn a_missing_descriptor_directory_falls_back_to_temp_dir() {
    let resolved = resolve_descriptor_directory(
        Err("simulated app_local_data_dir failure"),
        "org.freevia.qcremote",
    );
    assert_eq!(resolved, std::env::temp_dir().join("org.freevia.qcremote"));
}

#[test]
fn a_resolved_descriptor_directory_is_used_as_is() {
    let resolved = resolve_descriptor_directory(
        Ok::<_, &str>(PathBuf::from(
            "C:/Users/test/AppData/Local/org.freevia.qcremote",
        )),
        "org.freevia.qcremote",
    );
    assert_eq!(
        resolved,
        PathBuf::from("C:/Users/test/AppData/Local/org.freevia.qcremote")
    );
}

fn shared_gateway(descriptor_path: &Path) -> Gateway {
    Gateway {
        descriptor_path: Some(descriptor_path.to_path_buf()),
        ..Gateway::default()
    }
}

#[test]
fn forced_tray_release_survives_socket_close_and_blocks_automatic_restart() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor) = armed_broker_descriptor_path("intentional-tray-release");
    let mut window = shared_gateway(&descriptor);
    window.request(rpc::RUNTIME_STATUS, json!({})).unwrap();
    let mut tray = GatewayProcess::start_with_descriptor_path(None, false, &descriptor).unwrap();
    assert!(tray
        .request_unchecked("broker.hello", json!({"role":"observer"}))
        .is_ok());
    assert!(tray
        .request_unchecked("broker.shutdown", json!({"force":true}))
        .is_ok());
    assert!(wait_until(Duration::from_secs(2), || {
        window
            .process
            .as_ref()
            .unwrap()
            .intentional_release
            .load(std::sync::atomic::Ordering::Acquire)
    }));
    for _ in 0..3 {
        assert!(matches!(
            window.request_detailed(rpc::RUNTIME_STATUS, json!({})),
            Err(GatewayRequestFailure::Remote {
                code: -32002,
                retryable: false,
                ..
            })
        ));
        assert!(window.process.is_none());
    }
    assert!(wait_until(Duration::from_secs(2), || !descriptor.exists()));
    assert!(
        window.restart(rpc::RESET_SESSION).is_err(),
        "a reset is not explicit reconnect"
    );
    assert!(window.process.is_none());
    // Exercise the production gate without initiating any physical USB
    // operation: only reconnect may admit a fresh broker start.
    assert!(matches!(
        window.respect_intentional_release(rpc::RECONNECT),
        Ok(true)
    ));
    assert!(!window.intentional_release);
}

#[test]
fn participant_names_are_announced_only_when_the_tray_is_enabled() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor) = armed_broker_descriptor_path("participant-announcement");
    let directory = descriptor.parent().unwrap();
    broker_tray_settings::set_tray_enabled_in(directory, false).unwrap();
    let mut window = shared_gateway(&descriptor);
    window.request(rpc::RUNTIME_STATUS, json!({})).unwrap();
    let process = window.process.as_mut().unwrap();
    assert!(!process.participant_announced);
    let before = process
        .request_unchecked("broker.clients", json!({}))
        .ok()
        .unwrap();
    assert_eq!(before["clients"][0]["provider"], "");
    let pid = process.broker_pid;
    broker_tray_settings::set_tray_enabled_in(directory, true).unwrap();
    window.request(rpc::RUNTIME_STATUS, json!({})).unwrap();
    let process = window.process.as_mut().unwrap();
    assert!(process.participant_announced);
    assert_eq!(
        process.broker_pid, pid,
        "enable must not reconnect the device"
    );
    let after = process
        .request_unchecked("broker.clients", json!({}))
        .ok()
        .unwrap();
    assert_eq!(after["clients"][0]["provider"], "QC Remote window");
    assert_eq!(after["clients"][0]["role"], "participant");
    break_own_connection(process);
    assert!(window
        .request_detailed(rpc::RUNTIME_STATUS, json!({}))
        .is_err());
    assert!(
        !window.intentional_release,
        "ordinary transport failure is still recoverable"
    );
    let recovered = window.request(rpc::RUNTIME_STATUS, json!({}));
    assert!(recovered.is_ok(), "ordinary recovery failed: {recovered:?}");
    assert_eq!(window.process.as_ref().unwrap().broker_pid, pid);
}

#[test]
fn intentional_release_notification_is_consumed_before_eof_without_a_pending_request() {
    let mut frames = Vec::new();
    write_frame(
        &mut frames,
        &serde_json::to_vec(&json!({
            "jsonrpc":"2.0","method":"broker.shuttingDown","params":{"forced":true}
        }))
        .unwrap(),
    )
    .unwrap();
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = spawn_gateway_reader(std::io::Cursor::new(frames), None, flag.clone()).unwrap();
    assert!(matches!(
        reader.responses.recv_timeout(Duration::from_secs(1)),
        Ok(GatewayReaderMessage::Failed(_))
    ));
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn an_older_brokers_unknown_hello_keeps_the_same_participant_transport_usable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut frames = Vec::new();
    for response in [
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Unknown method"}}),
        json!({"jsonrpc":"2.0","id":2,"result":{"detail":"Legacy broker still usable"}}),
    ] {
        write_frame(&mut frames, &serde_json::to_vec(&response).unwrap()).unwrap();
    }
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader(std::io::Cursor::new(frames), None, flag.clone()).unwrap();
    let mut process = GatewayProcess {
        transport: GatewayTransport::SharedBroker {
            stream,
            hosted_child: None,
            instance_id: "legacy-test".into(),
            release_marker_path: PathBuf::new(),
        },
        responses,
        reader_completed: completed,
        next_id: 1,
        broker_pid: std::process::id(),
        intentional_release: flag,
        participant_announced: false,
        lock_free_reads: None,
    };
    assert!(process.announce_participant("QC Remote window").is_ok());
    assert!(process.participant_announced);
    assert!(process.announce_participant("QC Remote window").is_ok());
    assert_eq!(
        process.next_id, 2,
        "unsupported hello is attempted only once per transport"
    );
    let response = process
        .request_unchecked("legacy.test", json!({}))
        .ok()
        .unwrap();
    assert_eq!(response["detail"], "Legacy broker still usable");
}

struct GatedReleaseReader {
    frames: std::io::Cursor<Vec<u8>>,
    entered: Option<mpsc::Sender<()>>,
    release: mpsc::Receiver<()>,
}

#[test]
fn announcement_settings_skip_hot_reads_and_refresh_disabled_policy_within_one_second() {
    let now = Instant::now();
    let reads = std::cell::Cell::new(0);
    let mut cache = AnnouncementSettingsCache::default();
    let read_disabled = || {
        reads.set(reads.get() + 1);
        false
    };
    assert!(!cache.should_announce(false, 1, now, read_disabled));
    for milliseconds in [0, 1, 250, 999] {
        assert!(!cache.should_announce(
            false,
            1,
            now + Duration::from_millis(milliseconds),
            read_disabled
        ));
    }
    assert_eq!(
        reads.get(),
        1,
        "a disabled connection reads the file at most once per second"
    );
    assert!(
        cache.should_announce(false, 1, now + Duration::from_secs(1), || {
            reads.set(reads.get() + 1);
            true
        })
    );
    assert_eq!(
        reads.get(),
        2,
        "another process's enable is seen at the next bounded refresh"
    );
    for seconds in [1, 2, 60] {
        assert!(
            !cache.should_announce(true, 99, now + Duration::from_secs(seconds), || {
                reads.set(reads.get() + 1);
                true
            })
        );
    }
    assert_eq!(
        reads.get(),
        2,
        "announced and legacy-fallback connections perform zero further file reads"
    );
    let mut locally_changed = AnnouncementSettingsCache::default();
    assert!(!locally_changed.should_announce(false, 1, now, || false));
    assert!(
        locally_changed.should_announce(false, 2, now, || true),
        "a successful local setter invalidates even within the same instant"
    );
}

#[test]
fn failed_generation_marker_recovers_a_missed_notice_but_ignores_other_exits() {
    for (index, contents, expected_release) in [
        (
            0,
            Some(r#"{"instanceId":"failed-test-generation","forced":true}"#),
            true,
        ),
        (
            1,
            Some(r#"{"instanceId":"older-generation","forced":true}"#),
            false,
        ),
        (
            2,
            Some(r#"{"instanceId":"failed-test-generation","forced":false}"#),
            false,
        ),
        (3, Some("malformed"), false),
        (4, None, false),
    ] {
        let path = std::env::temp_dir().join(format!(
            "qc-release-marker-{}-{index}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        if let Some(contents) = contents {
            fs::write(&path, contents).unwrap();
        }
        let (mut gateway, _peer, release_reader, flag) =
            gateway_with_a_failed_writer_and_gated_release_reader(false);
        if let GatewayTransport::SharedBroker {
            release_marker_path,
            ..
        } = &mut gateway.process.as_mut().unwrap().transport
        {
            *release_marker_path = path.clone();
        }
        assert!(gateway
            .request_detailed(rpc::RUNTIME_STATUS, json!({}))
            .is_err());
        release_reader.send(()).unwrap();
        gateway
            .retiring_process
            .as_ref()
            .unwrap()
            .reader_completed
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(
            !flag.load(std::sync::atomic::Ordering::Acquire),
            "this case has EOF without a shutdown notification"
        );
        let outcome = gateway.respect_intentional_release(rpc::RUNTIME_STATUS);
        if expected_release {
            assert!(matches!(
                outcome,
                Err(GatewayRequestFailure::Remote { code: -32002, .. })
            ));
            assert!(gateway.intentional_release);
            assert!(matches!(
                gateway.respect_intentional_release(rpc::RECONNECT),
                Ok(true)
            ));
            assert!(matches!(gateway.respect_intentional_release(rpc::RUNTIME_STATUS),Ok(false)), "explicit reconnect supersedes the matching marker without deleting another generation's evidence");
        } else {
            assert!(matches!(outcome,Ok(false)), "stale, malformed, non-forced and absent markers must preserve normal crash recovery");
            assert!(!gateway.intentional_release);
        }
        if contents.is_some() {
            fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn later_forced_generations_preserve_a_delayed_participants_release_record() {
    use generated_broker_surface::BrokerReleaseMarker;
    let descriptor = std::env::temp_dir().join(format!(
        "qc-release-generations-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let old_path = BrokerReleaseMarker::path_for(&descriptor, "older-generation");
    let new_path = BrokerReleaseMarker::path_for(&descriptor, "newer-generation");
    assert_ne!(old_path, new_path);
    for generation in ["older-generation", "newer-generation"] {
        fs::write(
            BrokerReleaseMarker::path_for(&descriptor, generation),
            serde_json::to_vec(&BrokerReleaseMarker {
                instance_id: generation.into(),
                forced: true,
            })
            .unwrap(),
        )
        .unwrap();
    }
    for generation in ["older-generation", "newer-generation", "fresh-generation"] {
        let (mut gateway, _peer, release_reader, flag) =
            gateway_with_a_failed_writer_and_gated_release_reader(false);
        if let GatewayTransport::SharedBroker {
            instance_id,
            release_marker_path,
            ..
        } = &mut gateway.process.as_mut().unwrap().transport
        {
            *instance_id = generation.into();
            *release_marker_path = BrokerReleaseMarker::path_for(&descriptor, generation);
        }
        assert!(gateway
            .request_detailed(rpc::RUNTIME_STATUS, json!({}))
            .is_err());
        release_reader.send(()).unwrap();
        gateway
            .retiring_process
            .as_ref()
            .unwrap()
            .reader_completed
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
        let result = gateway.respect_intentional_release(rpc::RUNTIME_STATUS);
        if generation == "fresh-generation" {
            assert!(
                matches!(result, Ok(false)),
                "unreleased new generations keep ordinary crash recovery"
            );
        } else {
            assert!(matches!(result,Err(GatewayRequestFailure::Remote { code:-32002,.. })),"both released generations remain blocked even when their socket notices were missed");
        }
    }
    assert!(old_path.exists() && new_path.exists());
    fs::remove_file(old_path).unwrap();
    fs::remove_file(new_path).unwrap();
}
impl Read for GatedReleaseReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            self.release.recv().unwrap();
        }
        self.frames.read(bytes)
    }
}

fn gateway_with_a_failed_writer_and_gated_release_reader(
    notify_release: bool,
) -> (
    Gateway,
    TcpStream,
    mpsc::Sender<()>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    // A guaranteed local write error, independent of TCP reset timing.
    writer.shutdown(Shutdown::Write).unwrap();
    let mut frames = Vec::new();
    if notify_release {
        write_frame(
            &mut frames,
            &serde_json::to_vec(&json!({
                "jsonrpc":"2.0", "method":"broker.shuttingDown", "params":{"forced":true}
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader(
        GatedReleaseReader {
            frames: std::io::Cursor::new(frames),
            entered: Some(entered_tx),
            release: release_rx,
        },
        None,
        flag.clone(),
    )
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let gateway = Gateway {
        process: Some(GatewayProcess {
            transport: GatewayTransport::SharedBroker {
                stream: writer,
                hosted_child: None,
                instance_id: "failed-test-generation".into(),
                release_marker_path: PathBuf::new(),
            },
            responses,
            reader_completed: completed,
            next_id: 1,
            broker_pid: std::process::id(),
            intentional_release: flag.clone(),
            participant_announced: false,
            lock_free_reads: None,
        }),
        ..Gateway::default()
    };
    (gateway, peer, release_tx, flag)
}

#[test]
fn a_write_failure_waits_for_the_delayed_release_reader_before_automatic_recovery() {
    let (mut gateway, _peer, release_reader, flag) =
        gateway_with_a_failed_writer_and_gated_release_reader(true);
    assert!(matches!(
        gateway.request_detailed(rpc::RUNTIME_STATUS, json!({})),
        Err(GatewayRequestFailure::Transport(_))
    ));
    assert!(gateway.process.is_none());
    assert!(gateway.retiring_process.is_some());
    assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
    // The reader is still gated, so elapsed time can never stand in for
    // proof that recovery is safe. This is the production pre-start gate.
    assert!(
        matches!(gateway.respect_intentional_release(rpc::RUNTIME_STATUS), Err(GatewayRequestFailure::Transport(message)) if message.contains("still draining"))
    );
    assert!(gateway.process.is_none());
    release_reader.send(()).unwrap();
    gateway
        .retiring_process
        .as_ref()
        .unwrap()
        .reader_completed
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));
    assert!(matches!(
        gateway.respect_intentional_release(rpc::RUNTIME_STATUS),
        Err(GatewayRequestFailure::Remote {
            code: -32002,
            retryable: false,
            ..
        })
    ));
    assert!(gateway.retiring_process.is_none());
    assert!(gateway.intentional_release);
}

#[test]
fn an_explicit_reconnect_supersedes_a_delayed_old_reader_generation() {
    let (mut gateway, _peer, release_reader, flag) =
        gateway_with_a_failed_writer_and_gated_release_reader(true);
    assert!(gateway
        .request_detailed(rpc::RUNTIME_STATUS, json!({}))
        .is_err());
    // Retain the completion receiver only to coordinate this test after
    // the gateway deliberately abandons the old generation.
    let (_, placeholder) = mpsc::channel();
    let completed = std::mem::replace(
        &mut gateway.retiring_process.as_mut().unwrap().reader_completed,
        placeholder,
    );
    assert!(matches!(
        gateway.respect_intentional_release(rpc::RECONNECT),
        Ok(true)
    ));
    release_reader.send(()).unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        flag.load(std::sync::atomic::Ordering::Acquire),
        "old reader really did observe release late"
    );
    assert!(
        matches!(
            gateway.respect_intentional_release(rpc::RUNTIME_STATUS),
            Ok(false)
        ),
        "old generation must not latch the explicit replacement"
    );
}

/// I2: one window switching to the demo device (or pressing Disconnect)
/// used to end the USB session under every other window and MCP client
/// attached to the same broker. While anyone else is attached, the
/// disconnect stays on the caller's side.
#[test]
fn a_disconnect_leaves_a_shared_session_open_while_others_use_it() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("disconnect-shared");
    let mut window = shared_gateway(&descriptor_path);
    let mut other = shared_gateway(&descriptor_path);
    window.request(rpc::RUNTIME_STATUS, json!({})).unwrap();
    other.request(rpc::RUNTIME_STATUS, json!({})).unwrap();

    let left_open = window
        .request(rpc::DISCONNECT, json!({}))
        .expect("the disconnect succeeds for the caller");
    assert_eq!(left_open["detail"], json!(SHARED_SESSION_LEFT_OPEN_DETAIL));
    assert_eq!(left_open["phase"], json!("disconnected"));
    assert!(
        validate_gateway_result(rpc::DISCONNECT, &left_open).is_ok(),
        "the answer honours the disconnect contract the webview validates"
    );
    assert!(
        !window.connected,
        "the caller's own latch reads disconnected"
    );

    // Once it is the only client left, a disconnect reaches the broker.
    drop(other);
    assert!(
        wait_until(Duration::from_secs(5), || {
            window
                .process
                .as_mut()
                .and_then(GatewayProcess::shared_broker_client_count)
                == Some(1)
        }),
        "the broker notices the other client has gone"
    );
    let closed = window.request(rpc::DISCONNECT, json!({})).unwrap();
    assert_eq!(closed["detail"], json!("Quad Cortex session closed"));
}

/// I5: a headless MCP host has no mount-time connect, so the first broker
/// it spawns must open the session itself; a window's must not, or it
/// would race the window's own connect.
#[test]
fn only_a_gateway_asked_to_opens_the_session_from_its_first_broker() {
    let window = Gateway::default();
    assert!(!window.broker_auto_connect());
    let headless = Gateway {
        auto_connect_first_broker: true,
        ..Gateway::default()
    };
    assert!(headless.broker_auto_connect());
    // A replacement always reconnects, whoever asks.
    let replacing = Gateway {
        started_once: true,
        ..Gateway::default()
    };
    assert!(replacing.broker_auto_connect());
}

/// A gateway given its own descriptor path uses it, which is how a
/// headless bridge - which never runs Tauri setup - reaches the shared
/// broker instead of a private one.
#[test]
fn a_gateway_with_a_descriptor_path_meets_the_shared_broker() {
    let _serialize = serialize_real_broker_test();
    let (_cleanup, descriptor_path) = armed_broker_descriptor_path("gateway-own-path");
    let mut gateway = shared_gateway(&descriptor_path);
    gateway.request(rpc::RUNTIME_STATUS, json!({})).unwrap();
    assert!(gateway
        .process
        .as_ref()
        .is_some_and(GatewayProcess::uses_shared_broker));
    assert!(descriptor_path.is_file(), "the broker published beside it");
}

/// Standalone-process harness for
/// `apps/windows/src-tauri/tests/shared_broker_integration.rs`.
///
/// That file is a separate integration-test crate and, like any external
/// crate, cannot reach `GatewayProcess` or `start_with_descriptor_path` -
/// both are private to this module. This test doubles as the access point
/// instead: run alone, selected by name, from a process the integration
/// test spawns directly (this crate's own compiled `--lib` test binary,
/// located the same way `default_broker_executable` above locates the
/// broker binary - `cargo test --no-run --message-format=json`, parsed for
/// the `compiler-artifact` naming this target), it drives the real
/// attach-or-elect path exactly as a `GatewayProcess`-holding window
/// would, but from a genuinely separate OS process, so the integration
/// test observes real cross-process convergence rather than exercising
/// this same logic twice from inside one process.
///
/// A `#[test]` function takes no arguments, so every parameter comes from
/// the environment instead:
/// - `QC_HARNESS_DESCRIPTOR_PATH` (required to activate this harness at
///   all; unset, this test does nothing and returns immediately, so an
///   ordinary `cargo test --lib` run is unaffected by its existence).
/// - `QC_HARNESS_STATUS_PATH`: written once, right after
///   `start_with_descriptor_path` returns, with this harness's own pid,
///   whether it won the election and is hosting the broker, and the
///   broker's pid either way - the same `broker_pid` field
///   `a_hosting_process_dropped_after_a_transport_failure_leaves_the_shared_broker_running`
///   above already reads directly, here written out for a parent process
///   to read instead.
/// - `QC_HARNESS_CONTROL_DIR`: polled forever (until this process is
///   killed - it never exits on its own) for `*.cmd` trigger files. Each
///   one is answered with one real `rpc::RUNTIME_STATUS` round trip
///   against the shared broker; the outcome goes to the sibling `.result`
///   file and the trigger is removed, so the parent can drive any number
///   of request/response rounds across the process boundary just by
///   dropping files and waiting for the answer.
#[test]
fn shared_broker_integration_harness() {
    let Ok(descriptor_path) = std::env::var("QC_HARNESS_DESCRIPTOR_PATH") else {
        return;
    };
    let status_path = std::env::var("QC_HARNESS_STATUS_PATH")
        .expect("QC_HARNESS_STATUS_PATH must be set alongside QC_HARNESS_DESCRIPTOR_PATH");
    let control_dir = std::env::var("QC_HARNESS_CONTROL_DIR")
        .expect("QC_HARNESS_CONTROL_DIR must be set alongside QC_HARNESS_DESCRIPTOR_PATH");
    let descriptor_path = PathBuf::from(descriptor_path);
    let control_dir = PathBuf::from(control_dir);

    let mut process = GatewayProcess::start_with_descriptor_path(None, false, &descriptor_path)
        .expect("harness process could not attach to or host the shared broker");
    if std::env::var_os("QC_HARNESS_OBSERVER").is_some() {
        assert!(process
                .request_unchecked(
                    "broker.hello",
                    json!({"role":"observer", "provider":"Integration test observer", "idlePolicyMinutes":15}),
                )
                .is_ok(), "harness observer announcement");
    }
    let staged_status = PathBuf::from(&status_path).with_extension("tmp");
    fs::write(
        &staged_status,
        json!({
            "harnessPid": std::process::id(),
            "hosting": process.hosted_broker_pid().is_some(),
            "brokerPid": process.broker_pid,
        })
        .to_string(),
    )
    .expect("harness could not write its status file");
    fs::rename(&staged_status, &status_path).expect("harness publishes complete status");

    loop {
        if let Ok(entries) = fs::read_dir(&control_dir) {
            let mut triggers: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("cmd"))
                .collect();
            triggers.sort();
            for trigger in triggers {
                let command = fs::read(&trigger).expect("harness command is readable");
                let query: Option<Value> = if command.is_empty() {
                    None
                } else {
                    Some(serde_json::from_slice(&command).expect("harness command JSON"))
                };
                let response = match &query {
                    Some(query) => process.request_unchecked(
                        query["method"].as_str().expect("harness method"),
                        query["params"].clone(),
                    ),
                    None => process.request(rpc::RUNTIME_STATUS, json!({})),
                };
                let outcome = match response {
                    Ok(value) if query.is_some() => value.to_string(),
                    Ok(_) => "ok".to_string(),
                    Err(GatewayRequestFailure::Transport(message)) => {
                        format!("err:transport:{message}")
                    }
                    Err(GatewayRequestFailure::Remote { code, message, .. }) => {
                        format!("err:remote:{code}:{message}")
                    }
                };
                let staged_result = trigger.with_extension("tmp");
                fs::write(&staged_result, outcome).expect("harness stages response");
                fs::rename(staged_result, trigger.with_extension("result"))
                    .expect("harness publishes complete response");
                let _ = fs::remove_file(&trigger);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The safety net on every wait below. Nothing here passes by waiting;
/// a timeout fails the test by name.
const RESOURCE_ORIGIN_SAFETY: Duration = Duration::from_secs(60);

/// One broker connection served from a script: every request's method is
/// reported on `received` as it arrives; `system.status` reports the device
/// session open or closed; the request named `hold` gets no answer until the
/// test sends on `fail`, which then closes the connection the way a broker
/// that died or a socket that broke does. Anything else answers `{}`.
struct ScriptedBroker {
    received: mpsc::Receiver<String>,
    fail: mpsc::Sender<()>,
}

/// A gateway attached to a [`ScriptedBroker`] exactly as
/// `GatewayProcess::start_shared` attaches to the real one, meeting further
/// brokers (if it ever looks for one) at `descriptor_path`.
fn gateway_with_scripted_broker(
    descriptor_path: &Path,
    session_open: bool,
    hold: Option<&'static str>,
) -> (Gateway, ScriptedBroker) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let (received_tx, received) = mpsc::channel();
    let (fail, fail_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut input = peer.try_clone().unwrap();
        let mut output = peer;
        while let Ok(request) = read_frame(&mut input) {
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let id = request["id"].clone();
            let _ = received_tx.send(method.clone());
            let response = if Some(method.as_str()) == hold {
                let _ = fail_rx.recv_timeout(RESOURCE_ORIGIN_SAFETY);
                let _ = output.shutdown(Shutdown::Both);
                return;
            } else if method == rpc::RUNTIME_STATUS {
                json!({"jsonrpc":"2.0","id":id,"result":{
                    "gatewayApiVersion": generated_gateway::API_VERSION,
                    "capabilities": [BROKER_RESOURCE_READ_ORIGIN_CAPABILITY],
                    "usbDiagnostics": {
                        "connected": session_open,
                        "phase": if session_open { "ready" } else { "disconnected" },
                        "synchronized": session_open
                    }
                }})
            } else {
                json!({"jsonrpc":"2.0","id":id,"result":{}})
            };
            if write_frame(&mut output, &serde_json::to_vec(&response).unwrap()).is_err() {
                return;
            }
        }
    });
    let intentional_release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reads = std::sync::Arc::new(LockFreeReads::new(
        stream.try_clone().unwrap(),
        intentional_release.clone(),
    ));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader_with(
        stream.try_clone().unwrap(),
        None,
        intentional_release.clone(),
        Some(reads.clone()),
    )
    .unwrap();
    let gateway = Gateway {
        process: Some(GatewayProcess {
            transport: GatewayTransport::SharedBroker {
                stream,
                hosted_child: None,
                instance_id: "scripted-broker".into(),
                release_marker_path: PathBuf::new(),
            },
            responses,
            reader_completed: completed,
            next_id: 1,
            broker_pid: 4242,
            intentional_release,
            participant_announced: true,
            lock_free_reads: Some(reads),
        }),
        descriptor_path: Some(descriptor_path.to_path_buf()),
        ..Gateway::default()
    };
    gateway.publish_lock_free_reads();
    (gateway, ScriptedBroker { received, fail })
}

/// The production path a host's MCP and `/rpc` surfaces answer from:
/// `LocalBackend` over a headless host's device adapter, at full access.
fn local_backend_over(adapter: &local_stdio::HeadlessDeviceAdapter) -> local_access::LocalBackend {
    local_access::LocalBackend {
        adapter: std::sync::Arc::new(adapter.clone()),
        access: tokio::sync::watch::channel(qc_relay_client::AccessMode::Full).1,
    }
}

fn local_mcp_route() -> qc_remote_mcp::PrincipalRoute {
    qc_remote_mcp::PrincipalRoute::new("qc-remote-local-user", "qc-remote-windows")
}

fn resource_origin_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn scene_params() -> serde_json::Map<String, Value> {
    json!({"scene":1,"expectedPresetIdentity":"x"})
        .as_object()
        .cloned()
        .unwrap()
}

/// Every method the scripted broker has received so far.
fn received_so_far(broker: &ScriptedBroker) -> Vec<String> {
    broker.received.try_iter().collect()
}

/// A tool call holds the gateway and loses its broker
/// connection while a resource read waits for the gateway. The read must
/// meet the connection as the tool call left it - gone - and answer offline,
/// never attach, elect, spawn or recover a broker. A check made before the
/// lock (v1's `gateway_attachment` snapshot) saw `Busy` and let it through.
///
/// Deterministic without sleeps: the broker receives the tool call only while
/// the tool call holds the gateway lock, and fails it only when told. The
/// read is started inside that window, and can take the lock only after the
/// tool call has retired its connection, however the threads are scheduled.
#[test]
fn a_resource_read_racing_a_failing_tool_call_never_brings_a_broker_up() {
    use qc_remote_mcp::QcBackend as _;
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("resource-read-race");
    let (gateway, broker) =
        gateway_with_scripted_broker(&descriptor_path, true, Some(rpc::SELECT_SCENE));
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let tool_call = runtime.spawn({
        let backend = backend.clone();
        async move {
            backend
                .request(&local_mcp_route(), rpc::SELECT_SCENE, scene_params())
                .await
        }
    });
    assert_eq!(
        broker
            .received
            .recv_timeout(RESOURCE_ORIGIN_SAFETY)
            .unwrap(),
        rpc::SELECT_SCENE,
        "the tool call holds the gateway and its request reached the broker"
    );
    let read = runtime.spawn({
        let backend = backend.clone();
        async move {
            qc_remote_mcp::as_resource_read(backend.request(
                &local_mcp_route(),
                rpc::LIST_MODELS,
                serde_json::Map::new(),
            ))
            .await
        }
    });
    broker
        .fail
        .send(())
        .expect("fail the tool call's connection");

    let tool_call = runtime.block_on(tool_call).expect("tool call task");
    assert!(tool_call.is_err(), "the tool call's connection failed");
    let read = runtime
        .block_on(read)
        .expect("read task")
        .expect_err("nothing may serve the read");
    assert_eq!(read.code, "device_unavailable", "{}", read.message);
    assert_eq!(read.message, local_stdio::OFFLINE_ADVICE);
    assert!(read.retryable);
    assert!(
        !adapter.gateway_attached(),
        "the read attached to no broker"
    );
    assert!(
        !descriptor_path.exists(),
        "no broker was spawned or published for the read"
    );
    assert!(
        !device_broker_election_path(&descriptor_path).exists(),
        "and no election was held"
    );
    assert!(received_so_far(&broker).is_empty(), "nothing else was sent");
}

/// Control: the same race with a tool call in the read's
/// place still recovers, as every request did before: it starts a broker
/// where the gateway meets the shared one. The broker opens no USB session
/// (`auto_connect_first_broker` is off and nothing was started before), and
/// ends through its own grace period once the adapter lets go.
#[test]
fn a_tool_call_racing_a_failing_tool_call_still_recovers_the_broker() {
    use qc_remote_mcp::QcBackend as _;
    let _serialize = serialize_real_broker_test();
    let descriptor_path = test_descriptor_path("tool-call-race");
    let (gateway, broker) =
        gateway_with_scripted_broker(&descriptor_path, true, Some(rpc::SELECT_SCENE));
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let first = runtime.spawn({
        let backend = backend.clone();
        async move {
            backend
                .request(&local_mcp_route(), rpc::SELECT_SCENE, scene_params())
                .await
        }
    });
    assert_eq!(
        broker
            .received
            .recv_timeout(RESOURCE_ORIGIN_SAFETY)
            .unwrap(),
        rpc::SELECT_SCENE
    );
    let second = runtime.spawn({
        let backend = backend.clone();
        async move {
            backend
                .request(&local_mcp_route(), rpc::LIST_MODELS, serde_json::Map::new())
                .await
        }
    });
    broker
        .fail
        .send(())
        .expect("fail the first tool call's connection");
    assert!(runtime.block_on(first).expect("first task").is_err());
    // Its outcome depends on the device; only the recovery matters here.
    let _ = runtime.block_on(second).expect("second task");

    assert!(
        adapter.gateway_attached(),
        "a tool call still brings the broker connection back"
    );
    device_broker_descriptor::read_descriptor(
        &descriptor_path,
        device_broker_descriptor::unix_time_ms(),
    )
    .expect("the recovered broker is published where the gateway meets it");
    let mut broker_child = adapter
        .take_hosted_broker_child()
        .expect("the adapter owns the recovered broker child");
    adapter.stop();
    let exited = wait_for_broker_child_exit(&mut broker_child, Duration::from_secs(12));
    assert!(
        exited,
        "with every client gone, the broker ends through its own grace period"
    );
    // Only now is the broker known gone; a failure above leaves the
    // directory, which it may still be using.
    let _ = remove_dir_with_retries(descriptor_path.parent().unwrap());
}

/// A resource read uses a healthy connection the gateway holds. The
/// device session is open, so it is served, after one status read that is
/// never device demand.
#[test]
fn a_resource_read_uses_the_open_session_on_the_connection_the_gateway_holds() {
    use qc_remote_mcp::QcBackend as _;
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("resource-read-open-session");
    let (gateway, broker) = gateway_with_scripted_broker(&descriptor_path, true, None);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let answered = runtime.block_on(qc_remote_mcp::as_resource_read(backend.request(
        &local_mcp_route(),
        rpc::LIST_MODELS,
        serde_json::Map::new(),
    )));
    assert!(
        answered.is_ok(),
        "{:?}",
        answered.err().map(|error| error.message)
    );
    assert_eq!(
        received_so_far(&broker),
        vec![
            rpc::RUNTIME_STATUS.to_string(),
            rpc::LIST_MODELS.to_string()
        ]
    );
    assert!(adapter.gateway_attached());
    assert!(!descriptor_path.exists(), "no other broker was looked for");
}

/// On a connection whose device session is closed (an idle release, or
/// a `--resident` start), device demand would make the broker reopen USB. A
/// resource read sends none and answers offline; the connection itself is
/// kept. The same request as a tool call still sends the demand.
#[test]
fn a_resource_read_sends_no_device_demand_on_a_closed_session() {
    use qc_remote_mcp::QcBackend as _;
    let (_directory_guard, descriptor_path) =
        guarded_descriptor_path("resource-read-closed-session");
    let (gateway, broker) = gateway_with_scripted_broker(&descriptor_path, false, None);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let read = runtime
        .block_on(qc_remote_mcp::as_resource_read(backend.request(
            &local_mcp_route(),
            rpc::LIST_MODELS,
            serde_json::Map::new(),
        )))
        .expect_err("a closed session is not reopened for a resource read");
    assert_eq!(read.code, "device_unavailable", "{}", read.message);
    assert_eq!(read.message, local_stdio::OFFLINE_ADVICE);
    assert_eq!(
        received_so_far(&broker),
        vec![rpc::RUNTIME_STATUS.to_string()],
        "only the status read reached the broker"
    );
    assert!(
        adapter.gateway_attached(),
        "the connection is kept, not torn down"
    );

    let tool_call = runtime.block_on(backend.request(
        &local_mcp_route(),
        rpc::LIST_MODELS,
        serde_json::Map::new(),
    ));
    assert!(
        tool_call.is_ok(),
        "{:?}",
        tool_call.err().map(|error| error.message)
    );
    assert_eq!(
        received_so_far(&broker),
        vec![rpc::LIST_MODELS.to_string()],
        "a tool call sends its demand, with no status read first"
    );
}

/// A broker connection that admits requests the way the device broker does,
/// for the resource-read tests. `system.status` reports the session open and,
/// when `narrows`, lists the resource-read capability. After answering its
/// first status read it reports on `status_answered` and waits for `close`,
/// which closes the device session - exactly the moment between the
/// gateway's status read and its device read. Device demand on a closed
/// session counts as a USB reopen in `reopened`, as the broker's idle reopen
/// would, unless the request carries the resource-read origin to a broker
/// that `narrows`: that one is refused as not connected. Every device request
/// is reported on `demands` with its origin.
struct AdmittingBroker {
    status_answered: mpsc::Receiver<()>,
    close: mpsc::Sender<()>,
    demands: mpsc::Receiver<(String, Option<String>)>,
    reopened: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

fn gateway_with_admitting_broker(
    descriptor_path: &Path,
    narrows: bool,
) -> (Gateway, AdmittingBroker) {
    use std::sync::atomic::Ordering;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let (answered_tx, status_answered) = mpsc::channel();
    let (close, close_rx) = mpsc::channel::<()>();
    let (demands_tx, demands) = mpsc::channel();
    let reopened = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = reopened.clone();
    std::thread::spawn(move || {
        let mut input = peer.try_clone().unwrap();
        let mut output = peer;
        let mut session_open = true;
        let mut paused = false;
        while let Ok(request) = read_frame(&mut input) {
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let origin = request["origin"].as_str().map(str::to_owned);
            let id = request["id"].clone();
            let response = if method == rpc::RUNTIME_STATUS {
                let mut capabilities = vec!["nativeGateway"];
                if narrows {
                    capabilities.push(BROKER_RESOURCE_READ_ORIGIN_CAPABILITY);
                }
                json!({"jsonrpc":"2.0","id":id,"result":{
                    "gatewayApiVersion": generated_gateway::API_VERSION,
                    "capabilities": capabilities,
                    "usbDiagnostics": {"connected": session_open, "phase": "ready"}
                }})
            } else {
                let _ = demands_tx.send((method.clone(), origin.clone()));
                let narrowed = narrows && origin.as_deref() == Some(BROKER_RESOURCE_READ_ORIGIN);
                if narrowed && !session_open {
                    json!({"jsonrpc":"2.0","id":id,"error":{
                        "code": -32002,
                        "message": "The Quad Cortex is not connected: a resource read never opens or reopens its session",
                        "data": {"category": "notConnected", "retryable": true}
                    }})
                } else {
                    if !session_open {
                        counted.fetch_add(1, Ordering::SeqCst);
                        session_open = true;
                    }
                    json!({"jsonrpc":"2.0","id":id,"result":{}})
                }
            };
            if write_frame(&mut output, &serde_json::to_vec(&response).unwrap()).is_err() {
                return;
            }
            if method == rpc::RUNTIME_STATUS && !paused {
                paused = true;
                let _ = answered_tx.send(());
                if close_rx.recv_timeout(RESOURCE_ORIGIN_SAFETY).is_ok() {
                    session_open = false;
                }
            }
        }
    });
    let intentional_release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reads = std::sync::Arc::new(LockFreeReads::new(
        stream.try_clone().unwrap(),
        intentional_release.clone(),
    ));
    let GatewayReader {
        responses,
        completed,
    } = spawn_gateway_reader_with(
        stream.try_clone().unwrap(),
        None,
        intentional_release.clone(),
        Some(reads.clone()),
    )
    .unwrap();
    let gateway = Gateway {
        process: Some(GatewayProcess {
            transport: GatewayTransport::SharedBroker {
                stream,
                hosted_child: None,
                instance_id: "admitting-broker".into(),
                release_marker_path: PathBuf::new(),
            },
            responses,
            reader_completed: completed,
            next_id: 1,
            broker_pid: 4242,
            intentional_release,
            participant_announced: true,
            lock_free_reads: Some(reads),
        }),
        descriptor_path: Some(descriptor_path.to_path_buf()),
        ..Gateway::default()
    };
    gateway.publish_lock_free_reads();
    (
        gateway,
        AdmittingBroker {
            status_answered,
            close,
            demands,
            reopened,
        },
    )
}

/// Drives one resource read through the production path while the broker
/// closes the device session between the gateway's status read and its
/// device read. Returns the read's outcome.
fn resource_read_across_a_close(
    broker: &AdmittingBroker,
    backend: &local_access::LocalBackend,
) -> Result<Value, qc_remote_mcp::BackendError> {
    use qc_remote_mcp::QcBackend as _;
    let runtime = resource_origin_runtime();
    let read = runtime.spawn({
        let backend = backend.clone();
        async move {
            qc_remote_mcp::as_resource_read(backend.request(
                &local_mcp_route(),
                rpc::LIST_MODELS,
                serde_json::Map::new(),
            ))
            .await
        }
    });
    broker
        .status_answered
        .recv_timeout(RESOURCE_ORIGIN_SAFETY)
        .expect("the gateway read the session status first");
    broker.close.send(()).expect("close the session");
    runtime.block_on(read).expect("read task")
}

/// The session closes after the gateway's
/// status read reported it open and before the device read. A broker that
/// honours the narrowing is told the read's origin and refuses it under its
/// own lock: no reopen, and the structured offline answer.
#[test]
fn a_session_closed_between_the_status_and_the_read_is_never_reopened() {
    let (_directory_guard, descriptor_path) =
        guarded_descriptor_path("resource-read-close-between");
    let (gateway, broker) = gateway_with_admitting_broker(&descriptor_path, true);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);

    let read = resource_read_across_a_close(&broker, &backend)
        .expect_err("a closed session is not reopened for a resource read");
    assert_eq!(read.code, "device_unavailable", "{}", read.message);
    assert_eq!(read.message, local_stdio::OFFLINE_ADVICE);
    assert!(read.retryable);
    assert_eq!(
        broker.demands.try_iter().collect::<Vec<_>>(),
        vec![(
            rpc::LIST_MODELS.to_string(),
            Some(BROKER_RESOURCE_READ_ORIGIN.to_string())
        )],
        "the read reached the broker marked as a resource read"
    );
    assert_eq!(
        broker.reopened.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing reopened the session"
    );
    assert!(!descriptor_path.exists(), "no other broker was looked for");
}

/// A broker that does not list the capability cannot
/// be told a read's origin (it rejects unknown envelope fields) and cannot
/// close the gap between the gateway's status read and its device read. A
/// device-backed resource read therefore fails closed against it: the offline
/// shape, a message naming the too-old broker, and no device request at all,
/// so that broker never gets the chance to reopen the session.
#[test]
fn an_older_broker_fails_a_resource_read_closed_and_is_sent_no_device_request() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("resource-read-older-broker");
    let (gateway, broker) = gateway_with_admitting_broker(&descriptor_path, false);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);

    let read = resource_read_across_a_close(&broker, &backend)
        .expect_err("an older broker cannot serve a resource read safely");
    assert_eq!(read.code, "device_unavailable", "{}", read.message);
    assert_eq!(read.message, BROKER_TOO_OLD_FOR_RESOURCE_READS);
    assert!(read.retryable);
    assert_eq!(
        broker.demands.try_iter().collect::<Vec<_>>(),
        Vec::<(String, Option<String>)>::new(),
        "no device request reached the older broker"
    );
    assert_eq!(
        broker.reopened.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing reopened the session"
    );
}

/// Control: a tool call to the same older broker is
/// unchanged. It is never marked, sends no status read first, and is served.
#[test]
fn a_tool_call_to_an_older_broker_still_goes_through() {
    use qc_remote_mcp::QcBackend as _;
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("tool-call-older-broker");
    let (gateway, broker) = gateway_with_admitting_broker(&descriptor_path, false);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let answered = runtime.block_on(backend.request(
        &local_mcp_route(),
        rpc::LIST_MODELS,
        serde_json::Map::new(),
    ));
    assert!(
        answered.is_ok(),
        "{:?}",
        answered.err().map(|error| error.message)
    );
    assert_eq!(
        broker.demands.try_iter().collect::<Vec<_>>(),
        vec![(rpc::LIST_MODELS.to_string(), None)]
    );
    assert!(
        broker.status_answered.try_recv().is_err(),
        "no status read came first"
    );
}

/// Control: a tool call is never marked, sends no status
/// read first, and is served on the open session as before.
#[test]
fn a_tool_call_is_never_marked_as_a_resource_read() {
    use qc_remote_mcp::QcBackend as _;
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("tool-call-unmarked");
    let (gateway, broker) = gateway_with_admitting_broker(&descriptor_path, true);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let answered = runtime.block_on(backend.request(
        &local_mcp_route(),
        rpc::LIST_MODELS,
        serde_json::Map::new(),
    ));
    assert!(
        answered.is_ok(),
        "{:?}",
        answered.err().map(|error| error.message)
    );
    assert_eq!(
        broker.demands.try_iter().collect::<Vec<_>>(),
        vec![(rpc::LIST_MODELS.to_string(), None)]
    );
    assert!(
        broker.status_answered.try_recv().is_err(),
        "no status read came first"
    );
}

/// Removes one test's scratch directory when dropped: after its assertions
/// pass, or while a failed one unwinds. Declare it before anything that holds
/// files in the directory, so it drops last. A directory already gone, or one
/// Windows still holds open, is left quietly.
pub(crate) struct RemoveDirOnDrop(PathBuf);

impl RemoveDirOnDrop {
    pub(crate) fn new(directory: impl Into<PathBuf>) -> Self {
        Self(directory.into())
    }
}

impl Drop for RemoveDirOnDrop {
    /// Best effort, and never a panic: a panic here, during an unwind, would
    /// abort the test binary. A failure is retried (Windows sharing
    /// violations clear in milliseconds) and then reported on stderr. Tests
    /// that must fail on a leak remove their directory through
    /// `CleanupReport::remove_dir` instead, which this then finds gone.
    fn drop(&mut self) {
        if let Err(problem) = remove_dir_with_retries(&self.0) {
            eprintln!("test cleanup: {problem}");
        }
    }
}

/// `fs::remove_dir_all`, retried a few times with a short, bounded delay. A
/// directory already gone is success. Any other final failure names the
/// directory and the bytes left in it.
fn remove_dir_with_retries(directory: &Path) -> Result<(), String> {
    const ATTEMPTS: u32 = 5;
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match fs::remove_dir_all(directory) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => last = Some(error),
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Err(format!(
        "could not remove {} after {ATTEMPTS} attempts ({}); {} bytes left",
        directory.display(),
        last.map_or_else(String::new, |error| error.to_string()),
        bytes_under(directory)
    ))
}

/// Bytes in every file under `path`, for a cleanup report.
fn bytes_under(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| bytes_under(&entry.path()))
                    .sum()
            })
            .unwrap_or(0),
        Ok(metadata) => metadata.len(),
        Err(_) => 0,
    }
}

/// What a fixture's cleanup could not do. Empty is clean.
#[derive(Debug, Default)]
pub(crate) struct CleanupReport {
    pub(crate) problems: Vec<String>,
    /// Set by any step that could not acknowledge its resource stopped (or
    /// that panicked). Directory removal then leaves the bytes in place.
    pub(crate) blocked: bool,
    /// Every owned child terminated through its retained handle, by the pid
    /// recorded for the report.
    pub(crate) terminated: Vec<u32>,
}

impl std::fmt::Display for CleanupReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.problems.is_empty() {
            return write!(formatter, "clean");
        }
        write!(formatter, "{}", self.problems.join("; "))
    }
}

impl CleanupReport {
    pub(crate) fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }

    /// A resource that could not be acknowledged stopped: reported, and the
    /// directory it may still write to is not removed.
    pub(crate) fn unacknowledged(&mut self, problem: String) {
        self.problems.push(problem);
        self.blocked = true;
    }

    /// Removes a fixture directory only when every step before it
    /// acknowledged its resource stopped; otherwise leaves it, with its bytes,
    /// in the report.
    pub(crate) fn remove_dir_unless_blocked(&mut self, directory: &Path) {
        if self.blocked {
            self.problems.push(format!(
                "termination not acknowledged; directory left in place: {} (bytes: {})",
                directory.display(),
                bytes_under(directory)
            ));
        } else if let Err(problem) = remove_dir_with_retries(directory) {
            self.problems.push(problem);
        }
    }
}

/// One teardown step: it owns (or shares an `Arc` of) what it stops.
type CleanupStep = Box<dyn FnOnce(&mut CleanupReport) + Send>;

/// Teardown armed as each resource is acquired, run in reverse on every exit:
/// after the assertions, after a startup failure or panic, and - best effort,
/// never panicking - from `Drop` if nobody ran it.
///
/// Each step runs under its own `catch_unwind`, so a step that panics is
/// reported and every later step still runs. `AssertUnwindSafe` is sound per
/// step: a step is consumed by its own call and never seen again, and the
/// only state shared across the catch is the `CleanupReport`, which a step
/// changes only by pushing whole strings and setting `blocked`; after a
/// panic the report is additionally marked blocked, so nothing is deleted on
/// the word of a step that did not finish.
#[derive(Default)]
pub(crate) struct CleanupStack {
    steps: Vec<(String, CleanupStep)>,
}

impl CleanupStack {
    pub(crate) fn arm(
        &mut self,
        what: impl Into<String>,
        step: impl FnOnce(&mut CleanupReport) + Send + 'static,
    ) {
        self.steps.push((what.into(), Box::new(step)));
    }

    /// Runs every armed step, last armed first, and returns what they could
    /// not do.
    pub(crate) fn run(mut self) -> CleanupReport {
        self.run_steps()
    }

    fn run_steps(&mut self) -> CleanupReport {
        let mut report = CleanupReport::default();
        while let Some((what, step)) = self.steps.pop() {
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step(&mut report)));
            if let Err(payload) = ran {
                report.unacknowledged(format!(
                    "cleanup step `{what}` panicked: {}",
                    panic_text(&*payload)
                ));
            }
        }
        report
    }
}

impl Drop for CleanupStack {
    /// Only when a test never reached `run`: best effort, and never a panic.
    fn drop(&mut self) {
        if !self.steps.is_empty() {
            let report = self.run_steps();
            if !report.is_clean() {
                eprintln!("test cleanup from Drop: {report}");
            }
        }
    }
}

/// Runs a test - startup included - and then its cleanup, whatever happened.
/// `body` arms `cleanup` as it acquires each resource, so a startup failure
/// or panic before any identity is captured still stops what was acquired.
///
/// `AssertUnwindSafe` wraps `body` only. After a panic in it, the only state
/// still used is the `CleanupStack`, whose armed steps are complete values
/// (pushing a step is the last thing arming does), so a half-run body leaves
/// it consistent. No resource is inside the catch except through a step.
pub(crate) fn run_with_cleanup<T>(
    body: impl FnOnce(&mut CleanupStack) -> T,
) -> (std::thread::Result<T>, CleanupReport) {
    let mut cleanup = CleanupStack::default();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&mut cleanup)));
    (outcome, cleanup.run())
}

/// The text of a panic payload, for a failure message that must keep it.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a non-text panic payload".to_owned())
}

/// Ends a test after `run_with_cleanup`. Any cleanup problem fails the test,
/// and the failure keeps both pieces of evidence:
/// - a primary failure with a clean cleanup is rethrown unchanged;
/// - a primary failure with an unclean cleanup fails with the primary text
///   followed by the cleanup report;
/// - no primary failure but an unclean cleanup fails with the report.
pub(crate) fn finish_or_rethrow<T>(outcome: std::thread::Result<T>, report: CleanupReport) -> T {
    match (outcome, report.is_clean()) {
        (Ok(value), true) => value,
        (Ok(_), false) => panic!("the test passed but its cleanup failed: {report}"),
        (Err(primary), true) => std::panic::resume_unwind(primary),
        (Err(primary), false) => panic!(
            "{}\n-- and its cleanup also failed: {report}",
            panic_text(&*primary)
        ),
    }
}

/// A child process the test holds the handle of. Termination goes through
/// that handle only (`Child::kill` is `TerminateProcess` on it, and `try_wait`
/// reads that same process object), so a pid that the OS has since reused
/// for another process can never be reached. `recorded_pid` is for the
/// report alone and is never used to find or kill anything.
/// A replacement for `try_wait`, injected by tests.
type ObserveWith = Box<dyn FnMut() -> std::io::Result<Option<std::process::ExitStatus>> + Send>;

pub(crate) struct OwnedChild {
    child: std::process::Child,
    recorded_pid: u32,
    /// Test-only injection: replaces `try_wait`, to prove that a
    /// wait error is never taken for an exit.
    observe_with: Option<ObserveWith>,
}

/// What a held process handle says about the process behind it. Only
/// `Exited` - `try_wait` answering `Ok(Some(status))`, or the handle being
/// signalled - acknowledges a termination. An error is `Unknown`, never gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitObservation {
    Exited,
    Running,
    Unknown(String),
}

/// A process the test holds a handle to: observed and terminated through
/// that handle only.
pub(crate) trait ProcessControl {
    fn recorded_pid(&self) -> u32;
    fn observe(&mut self) -> ExitObservation;
    fn terminate(&mut self) -> std::io::Result<()>;
    /// Drops any injected observation, so the real handle is read (used
    /// only by the test's own final reap).
    fn clear_injection(&mut self) {}
    /// A duplicate of the process handle, made while the owner holds the
    /// original. `None` where there is no OS handle (fakes).
    #[cfg(windows)]
    fn duplicate_handle(&self) -> Option<std::io::Result<std::os::windows::io::OwnedHandle>> {
        None
    }
}

impl OwnedChild {
    pub(crate) fn new(child: std::process::Child) -> Self {
        let recorded_pid = child.id();
        Self {
            child,
            recorded_pid,
            observe_with: None,
        }
    }

    /// `try_wait` on the held handle, or the injected replacement.
    pub(crate) fn observe(&mut self) -> ExitObservation {
        let observed = match &mut self.observe_with {
            Some(observe) => observe(),
            None => self.child.try_wait(),
        };
        match observed {
            Ok(Some(_)) => ExitObservation::Exited,
            Ok(None) => ExitObservation::Running,
            Err(error) => {
                ExitObservation::Unknown(format!("waiting on its handle failed: {error}"))
            }
        }
    }

    /// Waits up to `bound` for it to exit on its own, then terminates it
    /// through the handle and waits up to `acknowledgement` for the exit to
    /// be observed on that same handle (`stop_held`).
    pub(crate) fn stop(
        &mut self,
        what: &str,
        bound: Duration,
        acknowledgement: Duration,
        report: &mut CleanupReport,
        kill: impl FnOnce(&mut std::process::Child) -> std::io::Result<()>,
    ) {
        struct WithKill<'a, K> {
            owned: &'a mut OwnedChild,
            kill: Option<K>,
        }
        impl<K: FnOnce(&mut std::process::Child) -> std::io::Result<()>> ProcessControl
            for WithKill<'_, K>
        {
            fn recorded_pid(&self) -> u32 {
                self.owned.recorded_pid
            }
            fn observe(&mut self) -> ExitObservation {
                self.owned.observe()
            }
            fn terminate(&mut self) -> std::io::Result<()> {
                match self.kill.take() {
                    Some(kill) => kill(&mut self.owned.child),
                    None => Ok(()),
                }
            }
        }
        if stop_held(
            &mut WithKill {
                owned: self,
                kill: Some(kill),
            },
            what,
            bound,
            acknowledgement,
            report,
        ) {
            let _ = self.child.wait();
        }
    }
}

/// The one stop sequence for every held process. Returns whether the exit
/// was acknowledged - observed as `Exited` on the held handle.
///
/// - Exited within `bound`: acknowledged, nothing else done.
/// - Still running, or status unknown: reported, then terminated through the
///   handle, then observed again for up to `acknowledgement`.
/// - A terminate error is benign only when the next observation is `Exited`
///   (the process had already gone); otherwise it is reported.
/// - Anything but `Exited` at the end is "termination not acknowledged",
///   naming the wait error when there was one, and blocks removal.
pub(crate) fn stop_held(
    process: &mut dyn ProcessControl,
    what: &str,
    bound: Duration,
    acknowledgement: Duration,
    report: &mut CleanupReport,
) -> bool {
    let pid = process.recorded_pid();
    let mut last = process.observe();
    if last == ExitObservation::Running {
        wait_until(bound, || {
            last = process.observe();
            last != ExitObservation::Running
        });
    }
    match &last {
        ExitObservation::Exited => return true,
        ExitObservation::Running => report.problems.push(format!(
            "{what} (pid {pid}) still ran after {} ms",
            bound.as_millis()
        )),
        ExitObservation::Unknown(error) => report
            .problems
            .push(format!("{what} (pid {pid}): exit status unknown: {error}")),
    }
    let terminated = process.terminate();
    wait_until(acknowledgement, || {
        last = process.observe();
        last == ExitObservation::Exited
    });
    match (&terminated, &last) {
        (Ok(()), _) => report.terminated.push(pid),
        // Already gone: the terminate error was the benign one.
        (Err(_), ExitObservation::Exited) => {}
        (Err(error), _) => report.problems.push(format!(
            "{what} (pid {pid}): terminating through its handle failed: {error}"
        )),
    }
    match last {
        ExitObservation::Exited => true,
        ExitObservation::Running => {
            report.unacknowledged(format!(
                "{what} (pid {pid}): termination not acknowledged within {} ms",
                acknowledgement.as_millis()
            ));
            false
        }
        ExitObservation::Unknown(error) => {
            report.unacknowledged(format!(
                "{what} (pid {pid}): termination not acknowledged: exit status unknown: {error}"
            ));
            false
        }
    }
}

/// A broker process handle received at the instant of spawn, through the
/// test-only spawn sink in `host_device_broker` (`BrokerSpawnSink`): a
/// duplicate of the spawned `Child`'s own handle, so it names exactly that
/// process object whatever happens to its pid.
#[cfg(windows)]
pub(crate) struct HeldProcess {
    handle: std::os::windows::io::OwnedHandle,
    recorded_pid: u32,
}

#[cfg(windows)]
mod held_process_ffi {
    use std::os::windows::raw::HANDLE;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn WaitForSingleObject(handle: HANDLE, milliseconds: u32) -> u32;
        pub fn TerminateProcess(handle: HANDLE, exit_code: u32) -> i32;
        pub fn GetProcessId(handle: HANDLE) -> u32;
    }
    pub const WAIT_OBJECT_0: u32 = 0;
    pub const WAIT_TIMEOUT: u32 = 0x102;
}

#[cfg(windows)]
impl HeldProcess {
    pub(crate) fn new(handle: std::os::windows::io::OwnedHandle) -> Self {
        use std::os::windows::io::AsRawHandle as _;
        // SAFETY: a valid process handle owned by `handle`.
        let recorded_pid = unsafe { held_process_ffi::GetProcessId(handle.as_raw_handle()) };
        Self {
            handle,
            recorded_pid,
        }
    }
}

#[cfg(windows)]
impl ProcessControl for HeldProcess {
    fn recorded_pid(&self) -> u32 {
        self.recorded_pid
    }

    fn observe(&mut self) -> ExitObservation {
        use std::os::windows::io::AsRawHandle as _;
        // SAFETY: a valid process handle owned by `self.handle`; a zero wait
        // only reads whether the process object is signalled.
        match unsafe { held_process_ffi::WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            held_process_ffi::WAIT_OBJECT_0 => ExitObservation::Exited,
            held_process_ffi::WAIT_TIMEOUT => ExitObservation::Running,
            _ => ExitObservation::Unknown(format!(
                "waiting on its handle failed: {}",
                std::io::Error::last_os_error()
            )),
        }
    }

    fn terminate(&mut self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;
        // SAFETY: a valid process handle owned by `self.handle`.
        if unsafe { held_process_ffi::TerminateProcess(self.handle.as_raw_handle(), 1) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// How long a terminated child gets to be observed gone on its handle.
const TERMINATION_ACK_BOUND: Duration = Duration::from_secs(5);

/// A shared slot an owned child is put into the moment it is acquired, armed
/// before the acquisition can happen.
type ChildSlot = std::sync::Arc<Mutex<Option<OwnedChild>>>;

/// A process the test process owns.
type TestOwned = Box<dyn ProcessControl + Send>;

/// One reserved domain of a `TestOwner`.
struct TestDomain {
    domain: String,
    /// Capacity reserved for this domain, in processes.
    units: usize,
    /// Whether the reservation's `TestPermit` is still alive.
    permit_live: bool,
    /// Processes owned here, each kept until its exit is observed.
    processes: Vec<TestOwned>,
    /// Processes taken out one at a time for a callback; each comes back
    /// unless its exit was observed (`InFlight`).
    in_flight: usize,
}

/// The test process's owner for processes a fixture acquires.
///
/// - Admission: a fixture reserves its stable domain and capacity
///   (`reserve`) BEFORE it acquires anything. A domain still held, or
///   capacity exhausted, is refused before any spawn.
/// - Ownership: an acquired process is handed to the permit (`own`) and is
///   held here, in static storage reachable outside every test's stack,
///   through body unwind, cleanup-stack destruction and test return.
/// - Borrow-or-return: cleanup processes one entry at a time; the entry in
///   hand is returned on unwind (`InFlight`), the others never leave.
/// - An entry leaves only when its exit is observed through its own handle.
///   Nothing is dropped on refusal, overflow or panic; no final test hook.
struct TestOwner {
    domains: Mutex<Vec<TestDomain>>,
    units: usize,
}

/// The fixtures' owner: 16 process units.
static TEST_OWNER: TestOwner = TestOwner::new(16);

/// A reservation of one domain's capacity. Dropped with nothing owned, it
/// releases the domain; dropped with owned processes, the domain stays
/// reserved, and refuses re-admission, until every one is reaped.
#[must_use = "a permit owns its domain only while it is held or holds processes"]
struct TestPermit {
    owner: &'static TestOwner,
    domain: String,
}

impl TestOwner {
    const fn new(units: usize) -> Self {
        Self {
            domains: Mutex::new(Vec::new()),
            units,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TestDomain>> {
        self.domains
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reserves `units` for `domain`, or refuses - before anything is
    /// acquired - if the domain is held or the capacity is exhausted.
    fn reserve(&'static self, domain: &str, units: usize) -> Result<TestPermit, String> {
        let mut domains = self.lock();
        if let Some(held) = domains.iter().find(|held| held.domain == domain) {
            return Err(format!(
                "test domain {domain} not admitted: it still holds {} processes{}",
                held.processes.len() + held.in_flight,
                if held.permit_live {
                    " and a live permit"
                } else {
                    ""
                }
            ));
        }
        let reserved: usize = domains.iter().map(|held| held.units).sum();
        if reserved + units > self.units {
            return Err(format!(
                "test domain {domain} not admitted: {reserved} of {} units reserved",
                self.units
            ));
        }
        domains.push(TestDomain {
            domain: domain.to_owned(),
            units,
            permit_live: true,
            processes: Vec::new(),
            in_flight: 0,
        });
        Ok(TestPermit {
            owner: self,
            domain: domain.to_owned(),
        })
    }

    /// The recorded pids `domain` holds now, in hand ones excluded.
    fn pids(&self, domain: &str) -> Vec<u32> {
        self.lock()
            .iter()
            .filter(|held| held.domain == domain)
            .flat_map(|held| held.processes.iter().map(|process| process.recorded_pid()))
            .collect()
    }

    /// Whether `domain` is reserved at all (held processes or a live permit).
    fn is_reserved(&self, domain: &str) -> bool {
        self.lock().iter().any(|held| held.domain == domain)
    }

    /// Takes the next of `domain`'s processes into an `InFlight` that returns
    /// it on unwind or on an unobserved exit.
    fn take_one(&'static self, domain: &str) -> Option<InFlight> {
        let mut domains = self.lock();
        let held = domains.iter_mut().find(|held| held.domain == domain)?;
        if held.processes.is_empty() {
            return None;
        }
        let process = held.processes.remove(0);
        held.in_flight += 1;
        Some(InFlight {
            owner: self,
            domain: domain.to_owned(),
            process: Some(process),
        })
    }

    /// Puts a process back (or, with `None`, records its observed exit), and
    /// releases the domain when nothing is left and no permit is live.
    fn settle(&self, domain: &str, process: Option<TestOwned>) {
        let mut domains = self.lock();
        if let Some(held) = domains.iter_mut().find(|held| held.domain == domain) {
            held.in_flight = held.in_flight.saturating_sub(1);
            if let Some(process) = process {
                held.processes.push(process);
            }
        }
        domains.retain(|held| held.permit_live || held.in_flight > 0 || !held.processes.is_empty());
    }

    /// Stops `domain`'s processes one at a time, each through its own
    /// handle (`stop_held`): every entry is visited once; an acknowledged
    /// one is released, any other stays owned. A panic inside a callback
    /// unwinds with only the entry in hand, which `InFlight` returns; the
    /// others never left. `real` first clears any injected behaviour, so
    /// the real handle is used (the test's own final reap).
    fn stop_domain(
        &'static self,
        domain: &str,
        bound: Duration,
        acknowledgement: Duration,
        real: bool,
        report: &mut CleanupReport,
    ) {
        let count = self
            .lock()
            .iter()
            .find(|held| held.domain == domain)
            .map_or(0, |held| held.processes.len());
        for _ in 0..count {
            let Some(mut in_flight) = self.take_one(domain) else {
                break;
            };
            let process = in_flight.process.as_mut().expect("in hand");
            if real {
                process.clear_injection();
            }
            if stop_held(&mut **process, domain, bound, acknowledgement, report) {
                in_flight.process = None;
            }
        }
    }

    /// The test's own final reap of `domain`, with every injected behaviour
    /// cleared first. Returns the failures, empty when every process was
    /// observed exited.
    fn reap(&'static self, domain: &str, bound: Duration) -> Vec<String> {
        self.reap_with(domain, bound, true)
    }

    /// Terminates `domain`'s processes one at a time through their own
    /// handles and waits up to `bound` for each exit to be observed. Each
    /// entry is visited once and is in hand alone (`InFlight`): one not
    /// observed exited, or one whose callback panics, stays owned.
    fn reap_with(&'static self, domain: &str, bound: Duration, real: bool) -> Vec<String> {
        let count = self
            .lock()
            .iter()
            .find(|held| held.domain == domain)
            .map_or(0, |held| held.processes.len());
        let mut failures = Vec::new();
        for _ in 0..count {
            let Some(mut in_flight) = self.take_one(domain) else {
                break;
            };
            let process = in_flight.process.as_mut().expect("in hand");
            if real {
                process.clear_injection();
            }
            let pid = process.recorded_pid();
            let terminated = process.terminate();
            if wait_until(bound, || process.observe() == ExitObservation::Exited) {
                in_flight.process = None;
            } else {
                failures.push(format!(
                    "pid {pid}: not observed exited after terminate ({}); still owned under \
                     {domain}",
                    terminated.map_or_else(|error| error.to_string(), |()| "ok".into())
                ));
            }
        }
        failures
    }
}

/// One process in hand for a callback. Dropped - normally or while
/// unwinding - it settles back into its domain unless its exit was observed.
struct InFlight {
    owner: &'static TestOwner,
    domain: String,
    process: Option<TestOwned>,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.owner.settle(&self.domain, self.process.take());
    }
}

impl TestPermit {
    fn domain(&self) -> &str {
        &self.domain
    }

    /// Hands an acquired process to the owner at once. A process beyond the
    /// reserved units is still owned - never dropped - and reported as an
    /// overflow, which the caller must treat as a failure.
    fn own(&self, process: TestOwned) -> Result<(), String> {
        let mut domains = self.owner.lock();
        let held = domains
            .iter_mut()
            .find(|held| held.domain == self.domain)
            .expect("a live permit's domain is reserved");
        held.processes.push(process);
        let owned = held.processes.len() + held.in_flight;
        if owned > held.units {
            Err(format!(
                "test domain {}: {owned} processes owned, {} reserved",
                self.domain, held.units
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for TestPermit {
    fn drop(&mut self) {
        let mut domains = self.owner.lock();
        if let Some(held) = domains.iter_mut().find(|held| held.domain == self.domain) {
            held.permit_live = false;
        }
        domains.retain(|held| held.permit_live || held.in_flight > 0 || !held.processes.is_empty());
    }
}

/// A unique, stable domain for a fixture instance without a scratch path:
/// a counter, never a pid.
fn fixture_domain(what: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{what}@{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Reserves a domain, or fails the fixture before it acquires anything.
fn admit(domain: &str, units: usize) -> TestPermit {
    TEST_OWNER
        .reserve(domain, units)
        .unwrap_or_else(|refusal| panic!("{refusal}"))
}

impl ProcessControl for OwnedChild {
    fn recorded_pid(&self) -> u32 {
        self.recorded_pid
    }
    fn observe(&mut self) -> ExitObservation {
        OwnedChild::observe(self)
    }
    fn terminate(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
    fn clear_injection(&mut self) {
        self.observe_with = None;
    }
    #[cfg(windows)]
    fn duplicate_handle(&self) -> Option<std::io::Result<std::os::windows::io::OwnedHandle>> {
        use std::os::windows::io::AsHandle as _;
        Some(self.child.as_handle().try_clone_to_owned())
    }
}

impl TestOwner {
    /// Takes the entry recorded as `pid` in `domain` into an `InFlight`.
    fn take_pid(&'static self, domain: &str, pid: u32) -> Option<InFlight> {
        let mut domains = self.lock();
        let held = domains.iter_mut().find(|held| held.domain == domain)?;
        let index = held
            .processes
            .iter()
            .position(|process| process.recorded_pid() == pid)?;
        let process = held.processes.remove(index);
        held.in_flight += 1;
        Some(InFlight {
            owner: self,
            domain: domain.to_owned(),
            process: Some(process),
        })
    }

    /// Observes one owned process through its own handle, borrowed: it stays
    /// owned whatever the answer, and on unwind.
    fn observe_owned(&'static self, domain: &str, pid: u32) -> Option<ExitObservation> {
        let mut in_flight = self.take_pid(domain, pid)?;
        let observed = in_flight.process.as_mut().map(|process| process.observe());
        observed
    }

    /// A duplicate of one owned process's handle, made while the owner keeps
    /// the original.
    #[cfg(windows)]
    fn duplicate_owned(
        &'static self,
        domain: &str,
        pid: u32,
    ) -> Option<std::io::Result<std::os::windows::io::OwnedHandle>> {
        let in_flight = self.take_pid(domain, pid)?;
        let duplicate = in_flight
            .process
            .as_ref()
            .and_then(|process| process.duplicate_handle());
        duplicate
    }
}

/// Spawns a sleeper whose ORIGINAL `Child` goes into `permit`'s domain on
/// the very next statement - before any duplicate, assertion or `expect` -
/// and returns its pid. The spawn itself either fails (no
/// process) or yields a `Child` that is owned at once.
fn spawn_owned_sleeper(permit: &TestPermit) -> u32 {
    let owned = OwnedChild::new(spawn_sleeper());
    let pid = owned.recorded_pid;
    let overflow = permit.own(Box::new(owned));
    // The process is owned either way; an overflow fails the test afterwards.
    if let Err(overflow) = overflow {
        panic!("{overflow}");
    }
    pid
}

/// The same with an injected one-shot terminator.
fn spawn_owned_sleeper_with_kill<K>(permit: &TestPermit, kill: K) -> u32
where
    K: FnOnce(&mut std::process::Child) -> std::io::Result<()> + Send + 'static,
{
    let owned = KillWith {
        owned: OwnedChild::new(spawn_sleeper()),
        kill: Some(kill),
    };
    let pid = owned.owned.recorded_pid;
    if let Err(overflow) = permit.own(Box::new(owned)) {
        panic!("{overflow}");
    }
    pid
}

/// Puts a freshly acquired child into an armed slot without any fallible
/// step in between (a poisoned slot lock is recovered, never a panic).
fn fill_slot(slot: &ChildSlot, child: OwnedChild) {
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child);
}

/// An owned child with a one-shot injected terminator (`arm_owned_child_with`).
struct KillWith<K> {
    owned: OwnedChild,
    kill: Option<K>,
}

impl<K: FnOnce(&mut std::process::Child) -> std::io::Result<()> + Send> ProcessControl
    for KillWith<K>
{
    fn recorded_pid(&self) -> u32 {
        self.owned.recorded_pid
    }
    fn observe(&mut self) -> ExitObservation {
        self.owned.observe()
    }
    fn terminate(&mut self) -> std::io::Result<()> {
        match self.kill.take() {
            Some(kill) => kill(&mut self.owned.child),
            None => self.owned.child.kill(),
        }
    }
    fn clear_injection(&mut self) {
        self.kill = None;
        self.owned.observe_with = None;
    }
    #[cfg(windows)]
    fn duplicate_handle(&self) -> Option<std::io::Result<std::os::windows::io::OwnedHandle>> {
        self.owned.duplicate_handle()
    }
}

/// Arms the teardown of whatever child lands in `slot`, reserving its domain
/// first: call it before the child is acquired. Returns the
/// domain, under which an unacknowledged child stays owned.
fn arm_owned_child(
    cleanup: &mut CleanupStack,
    what: &'static str,
    slot: &ChildSlot,
    descriptor_path: Option<PathBuf>,
    bound: Duration,
) -> String {
    arm_owned_child_with(
        cleanup,
        what,
        slot,
        descriptor_path,
        bound,
        TERMINATION_ACK_BOUND,
        |child| child.kill(),
    )
}

/// `arm_owned_child` with the acknowledgement bound and the terminator
/// given. The permit is reserved here, before the child exists; at cleanup
/// the child moves from the slot straight into the owner and is stopped
/// there, borrowed, so neither a failed stop nor a panic can drop it.
fn arm_owned_child_with(
    cleanup: &mut CleanupStack,
    what: &'static str,
    slot: &ChildSlot,
    descriptor_path: Option<PathBuf>,
    bound: Duration,
    acknowledgement: Duration,
    kill: impl FnOnce(&mut std::process::Child) -> std::io::Result<()> + Send + 'static,
) -> String {
    let domain = fixture_domain(what);
    let permit = admit(&domain, 1);
    let slot = slot.clone();
    cleanup.arm(what, move |report| {
        let held = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match held {
            Some(child) => {
                if let Err(overflow) = permit.own(Box::new(KillWith {
                    owned: child,
                    kill: Some(kill),
                })) {
                    report.unacknowledged(overflow);
                }
                TEST_OWNER.stop_domain(permit.domain(), bound, acknowledgement, false, report);
                let left = TEST_OWNER.pids(permit.domain());
                if !left.is_empty() {
                    report.unacknowledged(format!(
                        "{what}: pids {left:?} still owned under {}",
                        permit.domain()
                    ));
                }
            }
            None => {
                let published = descriptor_path.as_deref().and_then(|path| {
                    device_broker_descriptor::read_descriptor(
                        path,
                        device_broker_descriptor::unix_time_ms(),
                    )
                });
                if let Some(published) = published {
                    report.unacknowledged(format!(
                        "{what}: descriptor names pid {}; no owned handle: not terminated",
                        published.process_id
                    ));
                }
            }
        }
    });
    domain
}

/// A held broker handle with an injectable terminator.
#[cfg(windows)]
struct Terminating {
    held: HeldProcess,
    terminate: fn(&mut HeldProcess) -> std::io::Result<()>,
}

#[cfg(windows)]
impl ProcessControl for Terminating {
    fn recorded_pid(&self) -> u32 {
        self.held.recorded_pid()
    }
    fn observe(&mut self) -> ExitObservation {
        self.held.observe()
    }
    fn terminate(&mut self) -> std::io::Result<()> {
        (self.terminate)(&mut self.held)
    }
    fn clear_injection(&mut self) {
        self.terminate = HeldProcess::terminate;
    }
}

/// Moves every process entry of a spawn sink, one at a time, into `permit`'s
/// domain; reports the non-process entries. Nothing is put back into the
/// sink.
#[cfg(windows)]
fn sink_into_owner(
    sink: &BrokerSpawnSink,
    permit: &TestPermit,
    terminate: fn(&mut HeldProcess) -> std::io::Result<()>,
    report: &mut CleanupReport,
) {
    loop {
        let entry = {
            let mut entries = sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if entries.is_empty() {
                break;
            }
            entries.remove(0)
        };
        let process: TestOwned = match entry {
            SpawnedBroker::Unowned(why) => {
                report.unacknowledged(format!(
                    "broker: spawned, but no owned handle: not terminated ({why})"
                ));
                continue;
            }
            SpawnedBroker::ReapedInPlace(why) => {
                report.problems.push(format!("broker: {why}"));
                continue;
            }
            SpawnedBroker::Retained { handle, why } => {
                report.unacknowledged(format!("broker: {why}; handle retained"));
                Box::new(Terminating {
                    held: HeldProcess::new(handle),
                    terminate,
                })
            }
            SpawnedBroker::Owned(handle) => Box::new(Terminating {
                held: HeldProcess::new(handle),
                terminate,
            }),
            SpawnedBroker::OriginalChild(child) => Box::new(OwnedChild::new(child)),
        };
        if let Err(overflow) = permit.own(process) {
            report.unacknowledged(overflow);
        }
    }
}

/// How many processes one broker fixture domain may own: a spawned broker
/// can appear as a duplicate and as its original `Child`.
#[cfg(windows)]
const BROKER_FIXTURE_UNITS: usize = 4;

/// Arms the teardown of every broker `host_device_broker` spawns for
/// `descriptor_path`, reserving the fixture domain (the descriptor path)
/// first - before the sink is installed, so before any spawn - and
/// installing the spawn sink. At cleanup every entry moves, one at a time,
/// into the owner and is stopped there, borrowed. Returns the
/// guard (keep it alive while the broker may spawn) and the sink.
#[cfg(windows)]
fn arm_spawned_broker(
    cleanup: &mut CleanupStack,
    descriptor_path: &Path,
    bound: Duration,
    acknowledgement: Duration,
    injection: SpawnInjection,
    terminate: fn(&mut HeldProcess) -> std::io::Result<()>,
) -> (BrokerSpawnSinkGuard, BrokerSpawnSink) {
    let permit = admit(&descriptor_path.display().to_string(), BROKER_FIXTURE_UNITS);
    let (guard, sink) = BrokerSpawnSinkGuard::install(descriptor_path, injection);
    let held = sink.clone();
    cleanup.arm("broker", move |report| {
        sink_into_owner(&held, &permit, terminate, report);
        TEST_OWNER.stop_domain(permit.domain(), bound, acknowledgement, false, report);
        let left = TEST_OWNER.pids(permit.domain());
        if !left.is_empty() {
            report.unacknowledged(format!(
                "broker: pids {left:?} still owned under {}",
                permit.domain()
            ));
        }
    });
    (guard, sink)
}

/// Arms the fixture's take-over of whatever the production start slot of
/// `descriptor_path` still holds at cleanup, reserving its own domain first.
#[cfg(windows)]
fn arm_start_slot(cleanup: &mut CleanupStack, descriptor_path: &Path, bound: Duration) {
    let path = descriptor_path.to_path_buf();
    let permit = admit(&format!("{}#start-slot", path.display()), 1);
    cleanup.arm("start slot", move |report| {
        let Some(child) = take_start_slot_child(&path) else {
            return;
        };
        if let Err(overflow) = permit.own(Box::new(OwnedChild::new(child))) {
            report.unacknowledged(overflow);
        }
        TEST_OWNER.stop_domain(permit.domain(), bound, TERMINATION_ACK_BOUND, false, report);
        let left = TEST_OWNER.pids(permit.domain());
        if !left.is_empty() {
            report.unacknowledged(format!(
                "start slot: pids {left:?} still owned under {}",
                permit.domain()
            ));
        }
    });
}

/// The test's own last reaping of whatever is left in a spawn sink: every
/// process entry moves, one at a time, into `permit`'s domain, which is then
/// stopped through each handle with `terminate`. One not observed exited
/// stays owned there. Returns the failures.
#[cfg(windows)]
fn reap_spawn_sink(sink: &BrokerSpawnSink, permit: &TestPermit) -> Vec<String> {
    reap_spawn_sink_with(sink, permit, Duration::from_secs(5), HeldProcess::terminate)
}

/// `reap_spawn_sink`, with the bound and the terminator given.
#[cfg(windows)]
fn reap_spawn_sink_with(
    sink: &BrokerSpawnSink,
    permit: &TestPermit,
    bound: Duration,
    terminate: fn(&mut HeldProcess) -> std::io::Result<()>,
) -> Vec<String> {
    let mut report = CleanupReport::default();
    sink_into_owner(sink, permit, terminate, &mut report);
    let mut failures = report.problems;
    failures.extend(TEST_OWNER.reap_with(permit.domain(), bound, false));
    failures
}

/// The owned entries of a spawn sink, as fresh duplicates, for a test that
/// wants to observe them without taking them out of cleanup's hands. Step
/// 2p correction: entries whose duplicate cannot be made, `Unowned`,
/// `ReapedInPlace` and `OriginalChild` entries are not in the list, so an
/// empty list is not evidence that nothing runs.
#[cfg(windows)]
fn owned_in_sink(sink: &BrokerSpawnSink) -> Vec<HeldProcess> {
    sink.lock()
        .unwrap()
        .iter()
        .filter_map(|entry| match entry {
            SpawnedBroker::Owned(handle) | SpawnedBroker::Retained { handle, .. } => {
                handle.try_clone().ok().map(HeldProcess::new)
            }
            _ => None,
        })
        .collect()
}

/// A stand-in process: a sleeper this test spawns and holds the handle of.
/// Never a broker.
fn spawn_sleeper() -> std::process::Child {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("ping");
        command.args(["-n", "60", "127.0.0.1"]);
        command
    };
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the stand-in sleeper")
}

/// A scratch directory under the temp dir, armed for removal as it is made.
fn armed_scratch_directory(cleanup: &mut CleanupStack, name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("qc-{name}-{}", std::process::id()));
    let armed = directory.clone();
    cleanup.arm("directory", move |report| {
        report.remove_dir_unless_blocked(&armed)
    });
    fs::create_dir_all(&directory).unwrap();
    directory
}

/// A stand-in is terminated through the handle the test holds,
/// never through a pid. Its recorded pid is deliberately replaced by the
/// pid of a second, live process - the recycled-pid case - and that second
/// process (a bystander) is untouched; the stand-in is gone. A held file then
/// makes removal fail, and the report names both.
#[cfg(windows)]
#[test]
fn an_owned_child_is_terminated_only_through_its_handle() {
    use std::os::windows::fs::OpenOptionsExt as _;
    // The bystander is owned outside the behaviour under test,
    // reserved before it is spawned and handed to the owner at once.
    let bystanders = admit(&fixture_domain("handle-only-bystander"), 1);
    let bystander_pid = spawn_owned_sleeper(&bystanders);
    let slot: ChildSlot = Default::default();
    let held = Mutex::new(None);

    let (outcome, report) = run_with_cleanup(|cleanup| {
        let directory = armed_scratch_directory(cleanup, "handle-only-termination");
        arm_owned_child(cleanup, "stand-in", &slot, None, Duration::from_millis(50));
        let mut stand_in = OwnedChild::new(spawn_sleeper());
        // The recycled-pid simulation: the record now names another live
        // process. Only the handle is ever used, so that process survives.
        stand_in.recorded_pid = bystander_pid;
        fill_slot(&slot, stand_in);
        *held.lock().unwrap() = Some(
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .share_mode(0)
                .open(directory.join("held"))
                .unwrap(),
        );
        directory
    });
    let directory = outcome.expect("startup succeeded");
    let bystander_state = TEST_OWNER.observe_owned(bystanders.domain(), bystander_pid);
    drop(held.lock().unwrap().take());
    let bystander_reaped = TEST_OWNER
        .reap(bystanders.domain(), Duration::from_secs(5))
        .is_empty();

    assert_eq!(
        report.terminated,
        vec![bystander_pid],
        "one termination, by handle"
    );
    assert!(
        !report
            .problems
            .iter()
            .any(|problem| problem.contains("not acknowledged")),
        "the stand-in was observed gone on its own handle: {report}"
    );
    assert_eq!(
        bystander_state,
        Some(ExitObservation::Running),
        "the process the stale record names was not touched"
    );
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("could not remove")),
        "{report}"
    );
    assert!(
        bystander_reaped,
        "the bystander was ended through its own handle"
    );
    assert_eq!(remove_dir_with_retries(&directory), Ok(()));
}

/// A terminate that does not take effect is not acknowledged,
/// and the directory - which the still-running process may write to - is
/// left in place with its bytes reported.
#[test]
fn an_unacknowledged_termination_leaves_the_directory_in_place() {
    // Reserved first; the stand-in's original `Child` is owned on
    // the statement after its spawn, with a terminator that does not take
    // effect.
    let permit = admit(&fixture_domain("unacknowledged-termination"), 1);
    let mut cleanup = CleanupStack::default();
    let directory = armed_scratch_directory(&mut cleanup, "unacknowledged-termination");
    fs::write(
        directory.join("written-by-the-owned-process"),
        b"0123456789",
    )
    .unwrap();
    let pid = spawn_owned_sleeper_with_kill(&permit, |_| Ok(()));
    let mut report = CleanupReport::default();
    TEST_OWNER.stop_domain(
        permit.domain(),
        Duration::from_millis(20),
        Duration::from_millis(20),
        false,
        &mut report,
    );
    report.remove_dir_unless_blocked(&directory);
    let kept = directory.exists();
    let still_owned = TEST_OWNER.pids(permit.domain());

    // The test's own reap, through the same handle; then the armed directory
    // step removes what is left.
    let reaped = TEST_OWNER
        .reap(permit.domain(), Duration::from_secs(5))
        .is_empty();
    drop(cleanup);

    assert!(report.blocked);
    assert!(kept, "the directory was not removed");
    assert_eq!(still_owned, vec![pid], "still owned after the failed stop");
    assert!(reaped);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("termination not acknowledged within")),
        "{report}"
    );
    assert!(
        report.problems.iter().any(|problem| {
            problem.contains("termination not acknowledged; directory left in place")
                && problem.contains("(bytes: 10)")
        }),
        "{report}"
    );
}

/// A broker descriptor that this test holds no handle of is
/// reported and never terminated, however recent it is: here it names a
/// live sleeper the test spawned for the purpose.
#[test]
fn a_descriptor_only_process_is_reported_and_never_terminated() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("descriptor-only");
    let slot: ChildSlot = Default::default();
    let mut cleanup = CleanupStack::default();
    let directory = descriptor_path.parent().unwrap().to_path_buf();
    let armed = directory.clone();
    cleanup.arm("directory", move |report| {
        report.remove_dir_unless_blocked(&armed)
    });
    // Everything is reserved before the process exists - the
    // armed step under test and the separate owner of the descriptor's
    // process, which is outside the behaviour under test.
    arm_owned_child(
        &mut cleanup,
        "broker",
        &slot,
        Some(descriptor_path.clone()),
        Duration::from_millis(10),
    );
    let unowned_by_fixture = admit(&fixture_domain("descriptor-only-process"), 1);
    let pid = spawn_owned_sleeper(&unowned_by_fixture);
    let _listener = publish_silent_broker(
        &descriptor_path,
        pid,
        os_process_created_at(pid),
        device_broker_descriptor::unix_time_ms(),
    );
    let report = cleanup.run();
    let state = TEST_OWNER.observe_owned(unowned_by_fixture.domain(), pid);
    let reaped = TEST_OWNER
        .reap(unowned_by_fixture.domain(), Duration::from_secs(5))
        .is_empty();

    assert_eq!(
        state,
        Some(ExitObservation::Running),
        "the descriptor's process was not touched"
    );
    assert!(report.terminated.is_empty());
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains(&format!(
                "descriptor names pid {pid}; no owned handle: not terminated"
            ))),
        "{report}"
    );
    assert!(directory.exists(), "removal was blocked");
    assert!(reaped);
}

/// A cleanup step that panics is reported, and every step armed
/// before it still runs.
#[test]
fn a_panicking_cleanup_step_does_not_stop_the_later_steps() {
    let ran = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut cleanup = CleanupStack::default();
    for name in ["first armed", "second armed"] {
        let ran = ran.clone();
        cleanup.arm(name, move |_| ran.lock().unwrap().push(name));
    }
    cleanup.arm("panics", |_| panic!("a cleanup step failed"));
    let report = cleanup.run();
    assert_eq!(*ran.lock().unwrap(), vec!["second armed", "first armed"]);
    assert!(report.blocked, "a panicked step never authorises removal");
    assert_eq!(
        report.problems,
        vec!["cleanup step `panics` panicked: a cleanup step failed".to_string()]
    );
}

/// A startup that fails after acquiring a directory and a child,
/// before anything else, still stops the child through its handle and
/// removes the directory; the failure and a clean report both come back.
#[test]
fn a_startup_failure_still_releases_what_was_acquired() {
    let slot: ChildSlot = Default::default();
    let mut directory = None;
    let (outcome, report) = run_with_cleanup(|cleanup| -> Result<(), String> {
        let made = armed_scratch_directory(cleanup, "startup-failure");
        directory = Some(made);
        arm_owned_child(cleanup, "stand-in", &slot, None, Duration::from_millis(50));
        fill_slot(&slot, OwnedChild::new(spawn_sleeper()));
        Err("injected startup failure".into())
    });
    assert_eq!(
        outcome.expect("no panic"),
        Err("injected startup failure".into())
    );
    assert_eq!(report.terminated.len(), 1, "{report}");
    assert!(!directory.unwrap().exists(), "the directory was removed");
    assert!(slot.lock().unwrap().is_none());
}

/// The same with a startup panic: the panic comes back as the
/// primary failure, after the child was stopped and the directory removed.
#[test]
fn a_startup_panic_still_releases_what_was_acquired() {
    let slot: ChildSlot = Default::default();
    let directory = Mutex::new(None);
    let (outcome, report) = run_with_cleanup(|cleanup| {
        *directory.lock().unwrap() = Some(armed_scratch_directory(cleanup, "startup-panic"));
        arm_owned_child(cleanup, "stand-in", &slot, None, Duration::from_millis(50));
        fill_slot(&slot, OwnedChild::new(spawn_sleeper()));
        panic!("injected startup panic");
    });
    let primary = outcome.expect_err("the startup panic is caught");
    assert_eq!(panic_text(&*primary), "injected startup panic");
    assert_eq!(report.terminated.len(), 1, "{report}");
    assert!(!directory.lock().unwrap().take().unwrap().exists());
}

/// Every combination of primary failure and cleanup failure
/// ends the test failed with both pieces of evidence, or passed when both are
/// clean.
#[test]
fn the_primary_failure_and_the_cleanup_evidence_both_reach_the_failure() {
    let dirty = |cleanup: &mut CleanupStack| {
        cleanup.arm("teardown", |report| {
            report.problems.push("teardown: left something".into());
        });
    };
    let (outcome, report) = run_with_cleanup(|_| -> () { panic!("primary") });
    let rethrown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        finish_or_rethrow::<()>(outcome, report)
    }))
    .expect_err("the primary failure is rethrown");
    assert_eq!(rethrown.downcast_ref::<&str>().copied(), Some("primary"));

    let (outcome, report) = run_with_cleanup(|cleanup| -> () {
        dirty(cleanup);
        panic!("primary")
    });
    let both = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        finish_or_rethrow::<()>(outcome, report)
    }))
    .expect_err("a primary failure with a dirty cleanup fails");
    let message = panic_text(&*both);
    assert!(message.starts_with("primary"), "{message}");
    assert!(message.contains("teardown: left something"), "{message}");

    let (outcome, report) = run_with_cleanup(|_| 7);
    assert_eq!(finish_or_rethrow(outcome, report), 7);

    let (outcome, report) = run_with_cleanup(|cleanup| {
        dirty(cleanup);
        7
    });
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        finish_or_rethrow(outcome, report)
    }))
    .expect_err("a dirty cleanup fails a passing test");
    assert!(panic_text(&*failed).contains("teardown: left something"));
}

/// Runs one teardown step when dropped: after the test's assertions pass, or
/// while a failed one unwinds. Used to stop a hosting bridge or a real broker
/// before the directory guard removes their files, so declare it after the
/// `RemoveDirOnDrop` and after everything its step borrows; locals drop in
/// reverse order. The step must not panic itself: a panic while already
/// unwinding aborts the test binary.
pub(crate) struct RunOnDrop<F: FnMut()>(Option<F>);

impl<F: FnMut()> RunOnDrop<F> {
    pub(crate) fn new(step: F) -> Self {
        Self(Some(step))
    }
}

impl<F: FnMut()> Drop for RunOnDrop<F> {
    fn drop(&mut self) {
        if let Some(mut step) = self.0.take() {
            step();
        }
    }
}

/// The teardown order the hosting and real-broker fixtures rely
/// on, while a test panics. A stand-in teardown step (a counter, not a real
/// bridge: see README-step2c) runs once, before the directory is removed, and
/// the panic itself still reaches the caller. No `AssertUnwindSafe` is needed:
/// the closure captures only an `Arc` of atomics and a `PathBuf`.
#[test]
fn a_panicking_fixture_runs_its_teardown_before_its_directory_is_removed() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let directory =
        std::env::temp_dir().join(format!("qc-run-on-drop-order-{}", std::process::id()));
    let stops = std::sync::Arc::new(AtomicUsize::new(0));
    let directory_present_at_stop = std::sync::Arc::new(AtomicBool::new(false));
    let unwound = std::panic::catch_unwind({
        let (directory, stops, present) = (
            directory.clone(),
            stops.clone(),
            directory_present_at_stop.clone(),
        );
        move || {
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("held-by-the-fixture"), b"bytes").unwrap();
            let _directory_guard = RemoveDirOnDrop::new(directory.clone());
            let _stop_hosting = RunOnDrop::new(|| {
                present.store(directory.exists(), Ordering::SeqCst);
                stops.fetch_add(1, Ordering::SeqCst);
            });
            panic!("a failing assertion while the fixture is hosting");
        }
    });
    let panic = unwound.expect_err("the test's own panic is not swallowed");
    assert_eq!(
        panic.downcast_ref::<&str>().copied(),
        Some("a failing assertion while the fixture is hosting")
    );
    assert_eq!(stops.load(Ordering::SeqCst), 1, "the teardown ran once");
    assert!(
        directory_present_at_stop.load(Ordering::SeqCst),
        "it ran while the files were still there, before the directory guard"
    );
    assert!(!directory.exists(), "and the directory guard ran after it");
}

/// `test_descriptor_path`, with its directory removed by the returned guard.
fn guarded_descriptor_path(name: &str) -> (RemoveDirOnDrop, PathBuf) {
    let descriptor_path = test_descriptor_path(name);
    let guard = RemoveDirOnDrop::new(descriptor_path.parent().unwrap());
    (guard, descriptor_path)
}

/// `test_descriptor_path` for a test that starts a real broker there, with
/// its teardown armed before anything can spawn: every broker spawned for the
/// path is owned through its handle from the spawn (`arm_spawned_broker`),
/// given the 12 s it needs to end through its own grace period once its
/// clients are gone, and terminated through that handle only after that.
/// The directory goes last, and only once every exit was acknowledged.
///
/// Bind the stack after the real-broker lock and before every client: the
/// clients drop first, so the broker can reach its own zero-client exit, and
/// the lock is still held while it does. Teardown runs from the stack's
/// `Drop` on success and on a panic alike, and only reports, never fails.
fn armed_broker_descriptor_path(name: &str) -> (CleanupStack, PathBuf) {
    let descriptor_path = test_descriptor_path(name);
    let cleanup = armed_broker_teardown(&descriptor_path);
    (cleanup, descriptor_path)
}

/// The teardown `armed_broker_descriptor_path` arms, for a test that builds
/// its descriptor path itself (`local_stdio`'s headless hosts). The directory
/// holding `descriptor_path` must be the test's own: it is removed whole.
/// Bind the stack the same way, after the real-broker lock and before every
/// client; the lock also keeps the process units it reserves from adding to
/// another real-broker fixture's.
pub(crate) fn armed_broker_teardown(descriptor_path: &Path) -> CleanupStack {
    // The spawn sink is Windows-only, so elsewhere nothing is armed and the
    // directory stays, as it always has.
    #[cfg(not(windows))]
    let _ = descriptor_path;
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cleanup = CleanupStack::default();
    #[cfg(windows)]
    {
        let directory = descriptor_path.parent().unwrap().to_path_buf();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&directory)
        });
        let (spawns, _sink) = arm_spawned_broker(
            &mut cleanup,
            descriptor_path,
            Duration::from_secs(12),
            TERMINATION_ACK_BOUND,
            SpawnInjection::default(),
            HeldProcess::terminate,
        );
        // Dropped first at teardown; the broker step keeps its own sink.
        cleanup.arm("spawn sink", move |_| drop(spawns));
    }
    cleanup
}

/// The guard cleans up on a panic, not only on success, and
/// tolerates a directory that is already gone.
#[test]
fn a_scratch_directory_is_removed_when_its_test_panics() {
    let directory =
        std::env::temp_dir().join(format!("qc-remove-dir-on-drop-{}", std::process::id()));
    let inside = directory.clone();
    let unwound = std::panic::catch_unwind(move || {
        fs::create_dir_all(inside.join("nested")).unwrap();
        fs::write(inside.join("nested").join("file"), b"bytes").unwrap();
        let _guard = RemoveDirOnDrop::new(inside.clone());
        panic!("a failing assertion while the guard is alive");
    });
    assert!(unwound.is_err(), "the closure panicked");
    assert!(
        !directory.exists(),
        "the guard removed the directory while unwinding"
    );
    drop(RemoveDirOnDrop::new(directory.clone()));
    assert!(!directory.exists());
}

/// Every file in `directory`, with its bytes: what a broker start, attach,
/// election or descriptor cleanup would change.
fn directory_snapshot(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .map(|path| {
                    let bytes = fs::read(&path).unwrap_or_default();
                    (path, bytes)
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

/// The relay adapter's resource-read entry, called
/// as the relay client calls it - directly, from a task that carries no MCP
/// resource-read scope of its own - with no broker connection held and no
/// descriptor published. It is refused offline, and nothing in the
/// descriptor directory moves: no broker was started, attached to, elected
/// or published. Supersedes step2-03's
/// `a_relay_marked_resource_read_never_brings_a_broker_up`.
#[test]
fn a_relay_marked_read_with_no_broker_is_refused_and_starts_nothing() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("relay-read-no-broker");
    let directory = descriptor_path.parent().unwrap().to_path_buf();
    let before = directory_snapshot(&directory);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
        descriptor_path: Some(descriptor_path.clone()),
        ..Gateway::default()
    });
    let runtime = resource_origin_runtime();

    let refused = runtime
        .block_on(adapter.invoke_resource_read(rpc::LIST_MODELS, json!({})))
        .expect_err("nothing may serve a relay-marked resource read");
    assert_eq!(refused.code, "DEVICE_TRANSPORT");
    assert_eq!(refused.message, local_stdio::OFFLINE_ADVICE);
    assert!(refused.retryable);
    assert!(!adapter.gateway_attached(), "no broker connection was made");
    assert_eq!(
        directory_snapshot(&directory),
        before,
        "nothing was started or published"
    );
    assert!(!descriptor_path.exists());
    assert!(!device_broker_election_path(&descriptor_path).exists());
}

/// The same with a stale descriptor on disk, which
/// a connecting request would read, reap and replace. The resource read
/// leaves it byte for byte as it was.
#[test]
fn a_relay_marked_read_with_a_stale_descriptor_is_refused_and_touches_nothing() {
    let (_directory_guard, descriptor_path) =
        guarded_descriptor_path("relay-read-stale-descriptor");
    let directory = descriptor_path.parent().unwrap().to_path_buf();
    fs::write(
        &descriptor_path,
        br#"{"processId":4294967294,"port":1,"instanceId":"stale"}"#,
    )
    .unwrap();
    let before = directory_snapshot(&directory);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
        descriptor_path: Some(descriptor_path.clone()),
        ..Gateway::default()
    });
    let runtime = resource_origin_runtime();

    let refused = runtime
        .block_on(adapter.invoke_resource_read(rpc::CURRENT_PRESET, json!({})))
        .expect_err("a stale descriptor is not a broker to read from");
    assert_eq!(refused.code, "DEVICE_TRANSPORT");
    assert_eq!(refused.message, local_stdio::OFFLINE_ADVICE);
    assert!(refused.retryable);
    assert!(!adapter.gateway_attached());
    assert_eq!(
        directory_snapshot(&directory),
        before,
        "the stale descriptor was not touched"
    );
    assert!(!device_broker_election_path(&descriptor_path).exists());
}

/// Control: an unmarked call through the same
/// adapter entry (`invoke`, a relayed tool call) still brings the broker up
/// as today. The broker opens no USB session (`auto_connect_first_broker` is
/// off and nothing was started before), and ends through its own grace
/// period once the adapter lets go.
///
/// Every resource is armed for cleanup before it is acquired. The
/// broker this call spawns is owned by cleanup from the instant of the spawn,
/// through a duplicate of its process handle delivered by the test-only spawn
/// sink in `host_device_broker`; it is waited for, and if need be terminated,
/// through that handle alone.
#[cfg(windows)]
#[test]
fn an_unmarked_relay_call_through_the_same_adapter_still_brings_the_broker_up() {
    let _serialize = serialize_real_broker_test();
    let (outcome, report) = run_with_cleanup(|cleanup| {
        let descriptor_path = test_descriptor_path("relay-tool-call-brings-broker");
        let directory = descriptor_path.parent().unwrap().to_path_buf();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&directory)
        });
        // Cleanup owns the broker's handle from the instant it is
        // spawned (`arm_spawned_broker`), not from after `invoke` returns.
        let (_spawns, sink) = arm_spawned_broker(
            cleanup,
            &descriptor_path,
            Duration::from_secs(12),
            TERMINATION_ACK_BOUND,
            SpawnInjection::default(),
            HeldProcess::terminate,
        );
        let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
            descriptor_path: Some(descriptor_path.clone()),
            ..Gateway::default()
        });
        let stopping = adapter.clone();
        cleanup.arm("adapter", move |_| stopping.stop());
        let runtime = resource_origin_runtime();

        let answered = runtime.block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})));
        assert!(
            answered.is_ok(),
            "{:?}",
            answered.as_ref().err().map(|error| &error.message)
        );
        assert!(
            adapter.gateway_attached(),
            "a tool call still brings the broker up"
        );
        assert_eq!(
            owned_in_sink(&sink).len(),
            1,
            "the broker this call spawned is owned through its handle"
        );
        adapter.stop();
        assert!(
            wait_until(Duration::from_secs(12), || {
                // A plain loop: `observe` needs `&mut` on each held handle.
                let mut owned = owned_in_sink(&sink);
                let mut exited = 0;
                for held in &mut owned {
                    if held.observe() == ExitObservation::Exited {
                        exited += 1;
                    }
                }
                !owned.is_empty() && exited == owned.len()
            }),
            "with every client gone, the broker ends through its own grace period"
        );
    });
    finish_or_rethrow(outcome, report);
}

/// On the real broker fixture: the broker the call
/// spawned, owned through its retained `Child`, is made not to acknowledge
/// its termination by a bounded test-only injection (a 20 ms bound and a
/// terminator that does not take effect). Then:
/// - the directory and its bytes are preserved, and the report says
///   termination was not acknowledged;
/// - the primary failure is retained in the failure output;
/// - a bystander process is never touched;
/// - the broker is still reaped by the end of the test, through the same
///   handle, which cleanup returned to its slot.
///
/// The broker is owned through the spawn sink from its spawn.
#[cfg(windows)]
#[test]
fn a_real_broker_whose_termination_is_not_acknowledged_keeps_its_bytes_and_is_still_reaped() {
    let _serialize = serialize_real_broker_test();
    // The bystander is reserved first and owned at once.
    let bystanders = admit(&fixture_domain("broker-unacknowledged-bystander"), 1);
    let bystander_pid = spawn_owned_sleeper(&bystanders);
    let broker: Mutex<Option<BrokerSpawnSink>> = Mutex::new(None);
    let directory: std::sync::Arc<Mutex<Option<PathBuf>>> = Default::default();
    let (outcome, report) = run_with_cleanup(|cleanup| {
        let descriptor_path = test_descriptor_path("broker-unacknowledged");
        let parent = descriptor_path.parent().unwrap().to_path_buf();
        *directory.lock().unwrap() = Some(parent.clone());
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&parent)
        });
        // Owned from the spawn on; the terminator is made not to take effect.
        let (_spawns, sink) = arm_spawned_broker(
            cleanup,
            &descriptor_path,
            Duration::from_millis(20),
            Duration::from_millis(20),
            SpawnInjection::default(),
            |_| Ok(()),
        );
        *broker.lock().unwrap() = Some(sink.clone());
        let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
            descriptor_path: Some(descriptor_path.clone()),
            ..Gateway::default()
        });
        let stopping = adapter.clone();
        cleanup.arm("adapter", move |_| stopping.stop());
        let runtime = resource_origin_runtime();
        let answered = runtime.block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})));
        assert!(answered.is_ok(), "the broker came up");
        assert_eq!(owned_in_sink(&sink).len(), 1, "owned through its handle");
        fs::write(
            descriptor_path.with_file_name("bytes-to-keep"),
            b"0123456789",
        )
        .unwrap();
        panic!("injected failure");
    });
    let directory = directory.lock().unwrap().clone().expect("a directory");
    let kept = directory.exists() && directory.join("bytes-to-keep").exists();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_or_rethrow(outcome, report)
    }))
    .expect_err("the test fails with both pieces of evidence");
    let message = panic_text(&*failure);
    let bystander_untouched = TEST_OWNER.observe_owned(bystanders.domain(), bystander_pid)
        == Some(ExitObservation::Running);

    // The test's own reaping, before any assertion can fail: the broker
    // through the handle cleanup handed to the test owner, the
    // bystander through its own.
    let label = device_broker_descriptor::device_broker_descriptor_path(&directory)
        .display()
        .to_string();
    // Cleanup moved every sink entry into the owner; the sink is
    // empty, and the broker is owned under the fixture domain.
    let sink_empty = broker
        .lock()
        .unwrap()
        .take()
        .is_some_and(|sink| sink.lock().unwrap().is_empty());
    let owned_after_cleanup = TEST_OWNER.pids(&label);
    let reap_failures = TEST_OWNER.reap(&label, Duration::from_secs(5));
    let reaped = sink_empty
        && !owned_after_cleanup.is_empty()
        && reap_failures.is_empty()
        && !TEST_OWNER.is_reserved(&label);
    let bystander_reaped = TEST_OWNER
        .reap(bystanders.domain(), Duration::from_secs(5))
        .is_empty();
    let removed = remove_dir_with_retries(&directory);

    assert!(kept, "the directory and its bytes were preserved");
    assert!(message.starts_with("injected failure"), "{message}");
    assert!(
        message.contains("termination not acknowledged; directory left in place"),
        "{message}"
    );
    assert!(message.contains("(bytes: "), "{message}");
    assert!(bystander_untouched, "no foreign process was touched");
    assert!(
        bystander_reaped,
        "the bystander was ended through its own handle"
    );
    assert!(reaped, "the broker was still reaped through its handle");
    assert_eq!(removed, Ok(()));
}

/// A wait error on the held handle is an unknown
/// status, never an exit. The stand-in's `try_wait` is replaced by one that
/// always fails; cleanup terminates through the handle, cannot observe the
/// exit, reports "exit status unknown" naming the io error, and leaves the
/// directory and its bytes in place; the primary failure is retained. The
/// test then reaps its child through the real handle.
#[test]
fn a_wait_error_is_an_unknown_status_and_never_authorises_removal() {
    let slot: ChildSlot = Default::default();
    let pid: Mutex<Option<u32>> = Mutex::new(None);
    let domain: Mutex<Option<String>> = Mutex::new(None);
    let directory: Mutex<Option<PathBuf>> = Mutex::new(None);
    let (outcome, report) = run_with_cleanup(|cleanup| {
        let made = armed_scratch_directory(cleanup, "wait-error-unknown");
        *directory.lock().unwrap() = Some(made.clone());
        fs::write(made.join("bytes-to-keep"), b"0123456789").unwrap();
        *domain.lock().unwrap() = Some(arm_owned_child(
            cleanup,
            "stand-in",
            &slot,
            None,
            Duration::from_millis(20),
        ));
        let mut stand_in = OwnedChild::new(spawn_sleeper());
        stand_in.observe_with = Some(Box::new(|| {
            Err(std::io::Error::other("injected wait failure"))
        }));
        let stand_in_pid = stand_in.recorded_pid;
        fill_slot(&slot, stand_in);
        *pid.lock().unwrap() = Some(stand_in_pid);
        panic!("injected failure");
    });
    let directory = directory.lock().unwrap().clone().expect("a directory");
    let kept = directory.join("bytes-to-keep").exists();
    let blocked = report.blocked;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_or_rethrow(outcome, report)
    }))
    .expect_err("the test fails with both pieces of evidence");
    let message = panic_text(&*failure);
    // After cleanup returned, the stand-in is held by the test owner under
    // its domain - the same process - and the test reaps it through that
    // handle (the reap clears the injected observation first).
    let pid = pid.lock().unwrap().expect("a stand-in");
    let label = domain.lock().unwrap().clone().expect("a domain");
    let owned_after_cleanup = TEST_OWNER.pids(&label);
    let reaped = TEST_OWNER.reap(&label, Duration::from_secs(5)).is_empty()
        && !TEST_OWNER.is_reserved(&label);
    let removed = remove_dir_with_retries(&directory);

    assert!(blocked, "an unknown status blocks removal");
    assert!(kept, "the bytes were preserved");
    assert!(message.starts_with("injected failure"), "{message}");
    assert!(message.contains("exit status unknown"), "{message}");
    assert!(message.contains("injected wait failure"), "{message}");
    assert!(
        message.contains("termination not acknowledged; directory left in place"),
        "{message}"
    );
    assert_eq!(
        owned_after_cleanup,
        vec![pid],
        "owned after cleanup returned"
    );
    assert!(reaped, "the stand-in, still owned, was reaped by the test");
    assert_eq!(removed, Ok(()));
}

/// Runs a real broker fixture whose body fails at `failure`, with cleanup
/// owning the broker's handle from its spawn. Returns the cleanup report,
/// the primary failure's text, the fixture directory and whether the sink
/// still held a live broker afterwards.
#[cfg(windows)]
fn real_broker_failing_after_spawn(
    name: &str,
    injection: SpawnInjection,
) -> (CleanupReport, String, PathBuf, bool, Option<String>) {
    let directory: Mutex<Option<PathBuf>> = Mutex::new(None);
    let gateway_domain: Mutex<Option<String>> = Mutex::new(None);
    let invoke_error: Mutex<Option<String>> = Mutex::new(None);
    let sink_after: Mutex<Option<BrokerSpawnSink>> = Mutex::new(None);
    let (outcome, report) = run_with_cleanup(|cleanup| {
        let descriptor_path = test_descriptor_path(name);
        let parent = descriptor_path.parent().unwrap().to_path_buf();
        *directory.lock().unwrap() = Some(parent.clone());
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&parent)
        });
        let (_spawns, sink) = arm_spawned_broker(
            cleanup,
            &descriptor_path,
            Duration::from_millis(200),
            TERMINATION_ACK_BOUND,
            injection,
            HeldProcess::terminate,
        );
        *sink_after.lock().unwrap() = Some(sink);
        // The gateway's own `Child` is the last owner when the test
        // holds no handle (both duplicates failed). It is armed here, before
        // the spawn, and taken out of the gateway the moment `invoke` returns,
        // so cleanup can reap the broker through it.
        let gateway_child: ChildSlot = Default::default();
        *gateway_domain.lock().unwrap() = Some(arm_owned_child(
            cleanup,
            "gateway's broker child",
            &gateway_child,
            None,
            Duration::from_millis(200),
        ));
        // The production start slot owns the broker from its spawn
        // until it is handed on; whatever it still holds at cleanup (a
        // retained unpublished broker, or one held across a panic) is taken
        // by this fixture, which owns the scratch path, and stopped through
        // the original `Child`.
        arm_start_slot(cleanup, &descriptor_path, Duration::from_millis(200));
        let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
            descriptor_path: Some(descriptor_path.clone()),
            ..Gateway::default()
        });
        let stopping = adapter.clone();
        cleanup.arm("adapter", move |_| stopping.stop());
        let runtime = resource_origin_runtime();
        let answered = runtime.block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})));
        if let Some(child) = adapter.take_hosted_broker_child() {
            *gateway_child.lock().unwrap() = Some(OwnedChild::new(child));
        }
        *invoke_error.lock().unwrap() = answered.as_ref().err().map(|error| error.message.clone());
        // The failure lands here, before the test does anything with the
        // broker: either `GatewayProcess::start` already panicked right after
        // the spawn (and `invoke` reports the dead blocking task), or `invoke`
        // just returned.
        panic!(
            "injected failure after spawn: invoke answered {}",
            answered.is_ok()
        );
    });
    let primary = panic_text(&*outcome.expect_err("the injected failure"));
    let directory = directory.lock().unwrap().clone().expect("a directory");
    // Whatever cleanup could not acknowledge is held by the test
    // owner under one of this fixture's three domains (broker, gateway
    // child, start slot); the final reap goes through the same handles.
    // Cleanup drained the sink into the owner, so a non-empty sink is itself
    // a failure. `live` means "the final reap reported a failure", not an
    // OS-wide liveness measurement.
    let broker_domain = device_broker_descriptor::device_broker_descriptor_path(&directory)
        .display()
        .to_string();
    let mut failures = Vec::new();
    if sink_after
        .lock()
        .unwrap()
        .take()
        .is_some_and(|sink| !sink.lock().unwrap().is_empty())
    {
        failures.push("the spawn sink was not drained into the owner".to_owned());
    }
    let start_slot_domain = format!("{broker_domain}#start-slot");
    let domains = [
        Some(broker_domain),
        gateway_domain.lock().unwrap().take(),
        Some(start_slot_domain),
    ];
    for domain in domains.into_iter().flatten() {
        failures.extend(TEST_OWNER.reap(&domain, Duration::from_secs(5)));
    }
    let live = !failures.is_empty();
    let invoke_error = invoke_error.lock().unwrap().take();
    (report, primary, directory, live, invoke_error)
}

/// A panic inside `GatewayProcess::start` right after
/// the broker is spawned - before the attach, before `invoke` returns,
/// before the test has seen the broker at all - still leaves the broker
/// owned by cleanup, which terminates it through its handle and observes its
/// exit; the directory is then removed.
#[cfg(windows)]
#[test]
fn a_panic_inside_start_right_after_the_spawn_still_reaps_the_broker() {
    let _serialize = serialize_real_broker_test();
    let (report, primary, directory, live, _) = real_broker_failing_after_spawn(
        "broker-panic-inside-start",
        SpawnInjection {
            panic_after_owned: true,
            ..SpawnInjection::default()
        },
    );
    assert_eq!(
        primary,
        "injected failure after spawn: invoke answered false"
    );
    assert_eq!(
        report.terminated.len(),
        1,
        "terminated through its handle: {report}"
    );
    assert!(!report.blocked, "the exit was acknowledged: {report}");
    assert!(!live, "nothing left running");
    assert!(!directory.exists(), "the directory was removed");
}

/// The same failure the instant `invoke` returns,
/// before the test touches the broker.
#[cfg(windows)]
#[test]
fn a_failure_right_after_invoke_returns_still_reaps_the_broker() {
    let _serialize = serialize_real_broker_test();
    let (report, primary, directory, live, _) =
        real_broker_failing_after_spawn("broker-failure-after-invoke", SpawnInjection::default());
    assert_eq!(
        primary,
        "injected failure after spawn: invoke answered true"
    );
    assert!(!report.blocked, "the exit was acknowledged: {report}");
    assert!(!live, "nothing left running");
    assert!(!directory.exists(), "the directory was removed");
}

/// The spawned broker's handle cannot be duplicated
/// (injected). The broker is killed in place, and its exit observed, through the
/// original `Child` at the spawn point; the failure is reported; no panic is
/// injected because the broker was never owned through a duplicate; nothing
/// is left running and the directory is removed (the in-place reap is an
/// acknowledged exit).
#[cfg(windows)]
#[test]
fn a_failed_handle_duplicate_still_reaps_the_broker_in_place() {
    let _serialize = serialize_real_broker_test();
    let (report, primary, directory, live, invoke_error) = real_broker_failing_after_spawn(
        "broker-clone-fails",
        SpawnInjection {
            fail_clone: true,
            // Requested, and must not fire: the broker is not owned.
            panic_after_owned: true,
            ..SpawnInjection::default()
        },
    );
    assert_eq!(
        primary,
        "injected failure after spawn: invoke answered false"
    );
    let invoke_error = invoke_error.expect("the start failed: its broker was reaped");
    assert!(
        !invoke_error.contains("panicked"),
        "no panic before ownership: {invoke_error}"
    );
    assert!(
        report.problems.iter().any(|problem| problem
            .contains("duplicating its handle failed (injected handle duplication failure)")
            && problem.contains("killed in place (kill: ok) and its exit observed")),
        "{report}"
    );
    assert!(
        !report.blocked,
        "the in-place reap is an acknowledged exit: {report}"
    );
    assert!(report.terminated.is_empty(), "nothing else was terminated");
    assert!(!live, "nothing left running");
    assert!(!directory.exists(), "the directory was removed");
}

/// A sleeper and a duplicate of its handle for a spawn sink entry.
#[cfg(windows)]
fn sleeper_in_sink(originals: &TestPermit) -> (u32, SpawnedBroker) {
    // The ORIGINAL `Child` is owned (in `originals`' domain) before
    // the duplicate is attempted, so a failed duplicate cannot drop it.
    let pid = spawn_owned_sleeper(originals);
    let handle = TEST_OWNER
        .duplicate_owned(originals.domain(), pid)
        .expect("the original is owned")
        .expect("duplicate the sleeper's handle");
    (pid, SpawnedBroker::Owned(handle))
}

/// Whether the owned original `pid` in `domain` is observed exited, read
/// through its own handle for up to five seconds.
#[cfg(windows)]
fn owned_exited(domain: &str, pid: u32) -> bool {
    wait_until(Duration::from_secs(5), || {
        TEST_OWNER.observe_owned(domain, pid) == Some(ExitObservation::Exited)
    })
}

/// An entry that cannot be reaped never stops the
/// entries after it. `[Unowned, live]`: the live sleeper after the failed
/// entry is still terminated through its handle, and the failure is
/// returned.
#[cfg(windows)]
#[test]
fn reaping_a_spawn_sink_visits_the_live_entry_after_a_failed_one() {
    // Both domains are reserved before anything is spawned.
    let permit = admit(&fixture_domain("reap-visits-live-after-failed"), 1);
    let originals = admit(
        &fixture_domain("reap-visits-live-after-failed-originals"),
        1,
    );
    let (pid, entry) = sleeper_in_sink(&originals);
    let sink = BrokerSpawnSink::default();
    sink.lock()
        .unwrap()
        .extend([SpawnedBroker::Unowned("injected: no handle".into()), entry]);
    let failures = reap_spawn_sink(&sink, &permit);
    let live_exited = owned_exited(originals.domain(), pid);
    let originals_reaped = TEST_OWNER
        .reap(originals.domain(), Duration::from_secs(5))
        .is_empty();
    assert!(
        live_exited,
        "the live entry after the failed one was reaped"
    );
    assert_eq!(
        failures,
        vec![
            "broker: spawned, but no owned handle: not terminated (injected: no handle)"
                .to_string()
        ]
    );
    assert!(sink.lock().unwrap().is_empty());
    assert!(TEST_OWNER.pids(permit.domain()).is_empty());
    assert!(originals_reaped);
}

/// Mixed outcomes - two live sleepers, one already
/// exited, one reaped in place and one unowned - are all visited; every
/// live one is reaped and only the unowned one is a failure.
#[cfg(windows)]
#[test]
fn reaping_a_spawn_sink_with_mixed_outcomes_visits_every_entry() {
    // Both domains are reserved before anything is spawned.
    let permit = admit(&fixture_domain("reap-mixed-outcomes"), 3);
    let originals = admit(&fixture_domain("reap-mixed-outcomes-originals"), 3);
    let (first, first_entry) = sleeper_in_sink(&originals);
    let (gone, gone_entry) = sleeper_in_sink(&originals);
    // The "already exited" entry: ended through its owned original first.
    let gone_ended = TEST_OWNER
        .take_pid(originals.domain(), gone)
        .is_some_and(|mut in_flight| {
            let process = in_flight.process.as_mut().expect("in hand");
            let _ = process.terminate();
            let exited = wait_until(Duration::from_secs(5), || {
                process.observe() == ExitObservation::Exited
            });
            if exited {
                in_flight.process = None;
            }
            exited
        });
    let (second, second_entry) = sleeper_in_sink(&originals);
    let sink = BrokerSpawnSink::default();
    sink.lock().unwrap().extend([
        first_entry,
        SpawnedBroker::ReapedInPlace("injected: reaped in place".into()),
        gone_entry,
        SpawnedBroker::Unowned("injected: no handle".into()),
        second_entry,
    ]);
    let failures = reap_spawn_sink(&sink, &permit);
    let first_exited = owned_exited(originals.domain(), first);
    let second_exited = owned_exited(originals.domain(), second);
    let originals_reaped = TEST_OWNER
        .reap(originals.domain(), Duration::from_secs(5))
        .is_empty();
    assert!(
        gone_ended,
        "the already-exited entry was ended through its own handle"
    );
    assert!(first_exited && second_exited, "every live entry was reaped");
    assert_eq!(
        failures,
        vec![
            "broker: injected: reaped in place".to_string(),
            "broker: spawned, but no owned handle: not terminated (injected: no handle)"
                .to_string(),
        ]
    );
    assert!(TEST_OWNER.pids(permit.domain()).is_empty());
    assert!(originals_reaped);
}

/// All live - every one is reaped and nothing fails.
#[cfg(windows)]
#[test]
fn reaping_a_spawn_sink_of_live_entries_reaps_them_all() {
    // Both domains are reserved before anything is spawned.
    let permit = admit(&fixture_domain("reap-all-live"), 3);
    let originals = admit(&fixture_domain("reap-all-live-originals"), 3);
    let sink = BrokerSpawnSink::default();
    let mut pids = Vec::new();
    for _ in 0..3 {
        let (pid, entry) = sleeper_in_sink(&originals);
        pids.push(pid);
        sink.lock().unwrap().push(entry);
    }
    let failures = reap_spawn_sink(&sink, &permit);
    let mut all_exited = true;
    for pid in &pids {
        all_exited &= owned_exited(originals.domain(), *pid);
    }
    let originals_reaped = TEST_OWNER
        .reap(originals.domain(), Duration::from_secs(5))
        .is_empty();
    assert!(all_exited, "every entry was reaped");
    assert!(failures.is_empty(), "{failures:?}");
    assert!(originals_reaped);
}

/// The handle cannot be duplicated and the in-place
/// kill fails (both injected) while the real broker keeps running. The reap
/// at the spawn point is bounded (`SPAWN_REAP_BOUND`), so nothing hangs;
/// ownership is kept as a retained handle; cleanup reports the
/// unacknowledged reap and blocks removal, so the directory is left in
/// place; and the broker is still reaped at the test end through the
/// retained handle.
#[cfg(windows)]
#[test]
fn an_unacknowledged_in_place_reap_keeps_ownership_and_never_hangs() {
    let _serialize = serialize_real_broker_test();
    let started = Instant::now();
    let (report, primary, directory, live, _) = real_broker_failing_after_spawn(
        "broker-in-place-reap-unacknowledged",
        SpawnInjection {
            fail_clone: true,
            fail_kill: true,
            panic_after_owned: true,
            ..SpawnInjection::default()
        },
    );
    let kept = directory.exists();
    let removed = remove_dir_with_retries(&directory);

    assert!(
        started.elapsed() < Duration::from_secs(60),
        "bounded: {:?}",
        started.elapsed()
    );
    assert!(
        primary.starts_with("injected failure after spawn"),
        "{primary}"
    );
    assert!(
        report.blocked,
        "an unacknowledged reap blocks removal: {report}"
    );
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("in-place reap not acknowledged")
                && problem.contains("injected kill failure")
                && problem.contains("handle retained")),
        "{report}"
    );
    assert!(kept, "the directory was left in place");
    assert!(!live, "the retained broker was reaped at the test end");
    assert_eq!(removed, Ok(()));
}

/// The pid whose terminate `stubborn_terminate` makes a no-op (a
/// plain `fn`, so it can live in an owned entry).
#[cfg(windows)]
static STUBBORN_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
fn stubborn_terminate(held: &mut HeldProcess) -> std::io::Result<()> {
    if held.recorded_pid() == STUBBORN_PID.load(std::sync::atomic::Ordering::SeqCst) {
        Ok(())
    } else {
        held.terminate()
    }
}

/// The case `[Owned that does not exit within the bound, live]`.
/// The first entry's terminate does not take effect (injected); it is
/// reported and retained in the sink, owned, not dropped; the later live
/// entry is still reaped; the failure blocks removal so the bytes are kept;
/// and the retained one is reaped afterwards.
#[cfg(windows)]
#[test]
fn a_handle_not_observed_exited_is_retained_and_later_entries_are_still_reaped() {
    // Both domains are reserved before anything is acquired, and
    // each original is owned before its duplicate is made.
    let permit = admit(&fixture_domain("retained-handle-later-entries"), 2);
    let originals = admit(&fixture_domain("retained-handle-originals"), 2);
    let (stubborn_pid, stubborn_entry) = sleeper_in_sink(&originals);
    let (later_pid, later_entry) = sleeper_in_sink(&originals);
    STUBBORN_PID.store(stubborn_pid, std::sync::atomic::Ordering::SeqCst);
    let sink = BrokerSpawnSink::default();
    sink.lock().unwrap().extend([stubborn_entry, later_entry]);
    let mut cleanup = CleanupStack::default();
    let directory = armed_scratch_directory(&mut cleanup, "retained-handle");
    fs::write(directory.join("bytes-to-keep"), b"0123456789").unwrap();

    let label = permit.domain().to_owned();
    let failures = reap_spawn_sink_with(
        &sink,
        &permit,
        Duration::from_millis(100),
        stubborn_terminate,
    );
    let later_exited = owned_exited(originals.domain(), later_pid);
    let stubborn_running = TEST_OWNER.observe_owned(originals.domain(), stubborn_pid)
        == Some(ExitObservation::Running);
    // The survivor is held by the test owner, not put back.
    let retained = TEST_OWNER.pids(&label);
    let sink_empty = sink.lock().unwrap().is_empty();
    let mut report = CleanupReport::default();
    for failure in &failures {
        report.unacknowledged(failure.clone());
    }
    report.remove_dir_unless_blocked(&directory);
    let kept = directory.join("bytes-to-keep").exists();

    // The test end: the retained handle is reaped through the test owner.
    let afterwards = TEST_OWNER.reap(&label, Duration::from_secs(5));
    let stubborn_exited = owned_exited(originals.domain(), stubborn_pid);
    let originals_reaped = TEST_OWNER
        .reap(originals.domain(), Duration::from_secs(5))
        .is_empty();
    drop(cleanup);

    assert!(later_exited, "the later live entry was reaped");
    assert!(
        stubborn_running,
        "the first entry's terminate did not take effect"
    );
    assert_eq!(
        retained,
        vec![stubborn_pid],
        "the first entry is owned by the test owner"
    );
    assert!(sink_empty, "nothing was put back into the sink");
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains(&format!("pid {stubborn_pid}")),
        "{failures:?}"
    );
    assert!(failures[0].contains("still owned under"), "{failures:?}");
    assert!(kept, "the failure blocked removal; the bytes were kept");
    assert!(afterwards.is_empty(), "{afterwards:?}");
    assert!(
        stubborn_exited,
        "the retained handle was reaped at the test end"
    );
    assert!(originals_reaped);
}

/// End to end: both duplicates fail and the in-place kill does
/// not take effect, so the spawn sink holds `Unowned`. Cleanup cannot prove
/// ownership and reports the unowned spawn, keeping the fixture directory.
/// The fixture's final owner cleanup reaps the process after that report.
#[cfg(windows)]
#[test]
fn both_duplicates_failing_reports_unowned_spawn_and_blocks_cleanup() {
    let _serialize = serialize_real_broker_test();
    let started = Instant::now();
    let (report, _primary, directory, live, _) = real_broker_failing_after_spawn(
        "broker-double-duplicate-failure",
        SpawnInjection {
            fail_clone: true,
            fail_kill: true,
            fail_retain_clone: true,
            ..SpawnInjection::default()
        },
    );
    let kept = directory.exists();
    let removed = remove_dir_with_retries(&directory);

    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    assert!(
        report.problems.iter().any(|problem| problem
            .contains("no handle could be retained (injected second duplication failure)")),
        "{report}"
    );
    assert!(
        report.blocked,
        "the unowned spawn diagnostic remains visible: {report}"
    );
    assert!(!live, "nothing left in the sink");
    assert!(kept, "the unowned entry blocked removal");
    assert_eq!(removed, Ok(()));
}

/// The Published-failure arm: `host_device_broker` returns
/// `Occupied` and drops its local `Child` without killing it. The test's
/// duplicate, taken at the spawn, still owns the broker, and cleanup reaps
/// it through that handle.
#[cfg(windows)]
#[test]
fn the_published_failure_arm_leaves_the_broker_owned_by_the_spawn_handle() {
    let _serialize = serialize_real_broker_test();
    let (report, _primary, directory, live, _) = real_broker_failing_after_spawn(
        "broker-published-failure-arm",
        SpawnInjection {
            attach_outcome: AttachOverride::PublishedFailure,
            ..SpawnInjection::default()
        },
    );
    assert!(!report.blocked, "the exit was acknowledged: {report}");
    assert!(!live, "nothing left running");
    assert!(!directory.exists(), "the directory was removed");
}

/// The Unpublished arm when kill fails and wait status is unknown:
/// the real broker remains owned through the error and final cleanup reaps it.
/// The fake-process test separately proves the bounded still-running poll
/// without racing this broker's normal idle shutdown.
#[cfg(windows)]
#[test]
fn the_unpublished_arm_reports_unknown_wait_and_preserves_owned_cleanup() {
    let _serialize = serialize_real_broker_test();
    let started = Instant::now();
    let (report, primary, directory, live, invoke_error) = real_broker_failing_after_spawn(
        "broker-unpublished-arm",
        SpawnInjection {
            attach_outcome: AttachOverride::Unpublished,
            fail_kill: true,
            // Keep this real-child integration deterministic: the broker's
            // normal idle shutdown shares the five-second reap bound. The
            // fake-process test below covers the bounded still-running case.
            fail_wait: true,
            ..SpawnInjection::default()
        },
    );
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        primary,
        "injected failure after spawn: invoke answered false"
    );
    let invoke_error = invoke_error.expect("the start failed");
    assert!(
        invoke_error.contains("injected: the broker did not publish"),
        "{invoke_error}"
    );
    assert!(
        invoke_error.contains("kill failed: injected kill failure"),
        "{invoke_error}"
    );
    assert!(
        invoke_error.contains("exit status unknown: injected wait failure"),
        "{invoke_error}"
    );
    assert!(!report.blocked, "{report}");
    assert!(!live, "nothing left running");
    assert!(!directory.exists(), "the directory was removed");
}

/// What `unpublished_retained_across_unwind` saw.
#[cfg(windows)]
struct RetainedRun {
    invoke_error: Option<String>,
    slot_in_body: Option<(Option<u32>, Option<String>)>,
    slot_after: Option<(Option<u32>, Option<String>)>,
    report: CleanupReport,
    primary: String,
    descriptor_path: PathBuf,
}

/// Runs a real broker start whose attach is forced Unpublished, with no
/// auxiliary spawn duplicate (`no_sink_duplicate`): only the production
/// start slot can own the child. The body panics after `invoke`, the
/// adapter (its `Gateway`) is dropped, and the cleanup stack runs and is
/// destroyed; the fixture does NOT take the slot. The slot is read in the
/// body and again after all of that.
#[cfg(windows)]
fn unpublished_retained_across_unwind(name: &str, injection: SpawnInjection) -> RetainedRun {
    let descriptor_path = test_descriptor_path(name);
    let invoke_error: Mutex<Option<String>> = Mutex::new(None);
    let slot_in_body: Mutex<Option<(Option<u32>, Option<String>)>> = Mutex::new(None);
    let (outcome, report) = run_with_cleanup(|cleanup| {
        let directory = descriptor_path.parent().unwrap().to_path_buf();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&directory)
        });
        let watched = descriptor_path.clone();
        cleanup.arm("start slot still held", move |report| {
            if let Some((Some(pid), why)) = start_slot_state(&watched) {
                report.unacknowledged(format!(
                    "the production start slot still holds pid {pid} ({})",
                    why.unwrap_or_default()
                ));
            }
        });
        let (_spawns, _sink) = BrokerSpawnSinkGuard::install(
            &descriptor_path,
            SpawnInjection {
                no_sink_duplicate: true,
                ..injection
            },
        );
        let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
            descriptor_path: Some(descriptor_path.clone()),
            ..Gateway::default()
        });
        let runtime = resource_origin_runtime();
        let answered = runtime.block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})));
        *invoke_error.lock().unwrap() = answered.err().map(|error| error.message);
        *slot_in_body.lock().unwrap() = start_slot_state(&descriptor_path);
        adapter.stop();
        drop(adapter);
        panic!("injected failure");
    });
    RetainedRun {
        invoke_error: invoke_error.into_inner().unwrap(),
        slot_in_body: slot_in_body.into_inner().unwrap(),
        slot_after: start_slot_state(&descriptor_path),
        report,
        primary: outcome
            .err()
            .map(|payload| panic_text(&*payload))
            .unwrap_or_default(),
        descriptor_path,
    }
}

/// The test's own last step for a retained start slot: take the original
/// child out of it and end it through that same `Child`. Returns whether its
/// exit was observed within the bound.
#[cfg(windows)]
fn reap_start_slot(descriptor_path: &Path, owner: &TestPermit) -> bool {
    // The original moves from the production slot straight into the
    // test owner (`owner`, reserved by the caller before the start), and is
    // ended there; one not observed exited stays owned, never dropped.
    let Some(child) = take_start_slot_child(descriptor_path) else {
        return false;
    };
    let _ = owner.own(Box::new(OwnedChild::new(child)));
    TEST_OWNER
        .reap(owner.domain(), Duration::from_secs(5))
        .is_empty()
}

/// The fixture directories of a test whose broker child lives in the shared
/// production start slot. Dropped - after the assertions pass, or while a
/// failed one unwinds - it removes each directory only once its slot is
/// confirmed released: a child still in the slot is first ended through that
/// same `Child` (`reap_start_slot`). A directory whose child was not observed
/// exited, or whose slot is still reserved, is left in place and reported.
///
/// Bind it after the slot owner and before anything that starts a broker, so
/// it drops after the test's `Gateway` and before the permit it reaps into.
/// It reports and never fails or panics: a panic while already unwinding
/// aborts the test binary.
#[cfg(windows)]
struct StartSlotDirectories<'a> {
    owner: &'a TestPermit,
    descriptor_paths: Vec<PathBuf>,
    /// Descriptor paths whose slot child was taken and not observed exited.
    unreleased: Vec<PathBuf>,
}

#[cfg(windows)]
impl<'a> StartSlotDirectories<'a> {
    fn new(owner: &'a TestPermit, descriptor_paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            owner,
            descriptor_paths: descriptor_paths.into_iter().collect(),
            unreleased: Vec::new(),
        }
    }

    /// One more descriptor path whose directory this removes.
    fn watch(&mut self, descriptor_path: &Path) {
        self.descriptor_paths.push(descriptor_path.to_path_buf());
    }

    /// `reap_start_slot`, remembered: a child taken from the slot and not
    /// observed exited keeps its directory.
    fn reap(&mut self, descriptor_path: &Path) -> bool {
        let held = matches!(start_slot_state(descriptor_path), Some((Some(_), _)));
        let reaped = reap_start_slot(descriptor_path, self.owner);
        if held && !reaped {
            self.unreleased.push(descriptor_path.to_path_buf());
        }
        reaped
    }

    /// Whether the slot of `descriptor_path` is confirmed released, ending a
    /// child still in it first.
    fn release(&mut self, descriptor_path: &Path) -> bool {
        match start_slot_state(descriptor_path) {
            Some((Some(_), _)) => self.reap(descriptor_path),
            // Reserved with no child: a start may still be filling it.
            Some((None, _)) => false,
            None => !self.unreleased.iter().any(|path| path == descriptor_path),
        }
    }
}

#[cfg(windows)]
impl Drop for StartSlotDirectories<'_> {
    fn drop(&mut self) {
        for descriptor_path in std::mem::take(&mut self.descriptor_paths) {
            let released = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.release(&descriptor_path)
            }));
            let Some(directory) = descriptor_path.parent() else {
                continue;
            };
            match released {
                Ok(true) => {
                    if let Err(problem) = remove_dir_with_retries(directory) {
                        eprintln!("test cleanup: {problem}");
                    }
                }
                Ok(false) => eprintln!(
                    "test cleanup: the start slot of {} is not confirmed released; {} is left \
                     in place",
                    descriptor_path.display(),
                    directory.display()
                ),
                Err(payload) => eprintln!(
                    "test cleanup: releasing the start slot of {} panicked ({}); {} is left in \
                     place",
                    descriptor_path.display(),
                    panic_text(&*payload),
                    directory.display()
                ),
            }
        }
    }
}

/// A kill error while the broker keeps running, with no later request: the
/// production Unpublished arm's kill fails and the
/// broker keeps running. The returned failure carries the primary attach
/// failure, the kill error, the status and elapsed/bound; the SAME original
/// child stays in the production start slot after the request, the
/// `Gateway` and the whole test stack are gone - nothing polled it and
/// nothing claims cleanup succeeded. Removal is blocked. The test then ends
/// the broker through that same `Child`.
#[cfg(windows)]
#[test]
fn an_unpublished_kill_failure_is_retained_by_the_production_slot_across_unwind() {
    let _serialize = serialize_real_broker_test();
    let slot_owner = admit(&fixture_domain("slot-retained-kill-failure-owner"), 1);
    let run = unpublished_retained_across_unwind(
        "slot-retained-kill-failure",
        SpawnInjection {
            attach_outcome: AttachOverride::Unpublished,
            fail_kill: true,
            ..SpawnInjection::default()
        },
    );
    let reaped = reap_start_slot(&run.descriptor_path, &slot_owner);
    let directory = run.descriptor_path.parent().unwrap().to_path_buf();
    let kept = directory.exists();
    let removed = remove_dir_with_retries(&directory);

    let error = run.invoke_error.expect("the start failed");
    assert!(
        error.contains("injected: the broker did not publish"),
        "{error}"
    );
    assert!(
        error.contains("kill failed: injected kill failure"),
        "{error}"
    );
    assert!(error.contains("still running after"), "{error}");
    assert!(error.contains("of 5000 ms"), "{error}");
    assert!(error.contains("retained in its start slot"), "{error}");
    let (pid, why) = run
        .slot_in_body
        .expect("the slot held the child in the body");
    assert!(pid.is_some(), "the original child is in the slot");
    assert_eq!(
        run.slot_after,
        Some((pid, why)),
        "the same child, unchanged, after request, Gateway and test stack"
    );
    assert_eq!(run.primary, "injected failure");
    assert!(run.report.blocked, "{}", run.report);
    assert!(kept, "removal was blocked while the slot held the broker");
    assert!(reaped, "ended through the same Child at the test end");
    assert_eq!(removed, Ok(()));
}

/// A `try_wait` error: the kill succeeds but every
/// status read fails, so the exit is unknown; the original child stays in
/// its slot, unchanged, after the whole stack is gone, and the failure names
/// the wait error.
#[cfg(windows)]
#[test]
fn an_unpublished_wait_failure_is_retained_by_the_production_slot_across_unwind() {
    let _serialize = serialize_real_broker_test();
    let slot_owner = admit(&fixture_domain("slot-retained-wait-failure-owner"), 1);
    let run = unpublished_retained_across_unwind(
        "slot-retained-wait-failure",
        SpawnInjection {
            attach_outcome: AttachOverride::Unpublished,
            fail_wait: true,
            ..SpawnInjection::default()
        },
    );
    let reaped = reap_start_slot(&run.descriptor_path, &slot_owner);
    let directory = run.descriptor_path.parent().unwrap().to_path_buf();
    let removed = remove_dir_with_retries(&directory);

    let error = run.invoke_error.expect("the start failed");
    assert!(
        error.contains("injected: the broker did not publish"),
        "{error}"
    );
    assert!(error.contains("kill: ok"), "{error}");
    assert!(
        error.contains("exit status unknown: injected wait failure"),
        "{error}"
    );
    assert!(error.contains("retained in its start slot"), "{error}");
    let (pid, why) = run
        .slot_in_body
        .expect("the slot held the child in the body");
    assert!(pid.is_some());
    assert_eq!(run.slot_after, Some((pid, why)));
    assert!(run.report.blocked, "{}", run.report);
    assert!(reaped, "its exit is observed through the same Child");
    assert_eq!(removed, Ok(()));
}

/// A slot occupant whose lifetime only the test controls: `cmd`
/// reading commands from a pipe that stays open, so it never exits on its
/// own - unlike `ping -n 60`, which ends after about a minute.
#[cfg(windows)]
fn spawn_controlled_occupant() -> std::process::Child {
    Command::new("cmd")
        .args(["/Q", "/K"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a controlled slot occupant")
}

/// Real-process counterpart: every
/// start slot is held by a controlled occupant. A start for a new directory
/// spawns nothing and fails naming the held slots; no occupant is evicted or
/// replaced. The test then ends the first occupant through a duplicate of
/// its handle - made only after the slot owns the original - observes the
/// exit, and one reservation is admitted.
///
/// Ownership order: each occupant moves into its production slot on the
/// statement after its spawn; a refused occupation hands it back and it goes
/// straight to the test owner. Duplicates come only afterwards.
///
/// HOLD: real processes and a real gateway start. It is `#[ignore]`d until
/// the runner's containment (job breakaway, descendants) is settled. The
/// deterministic proof of the admission logic is synthetic
/// (`the_test_owner_reserves_before_acquisition_and_refuses_when_full`,
/// `two_racing_reservations_for_the_last_unit_admit_exactly_one`); a
/// synthetic proof of the production `START_SLOTS` table itself would need a
/// production change.
#[cfg(windows)]
#[test]
#[ignore = "HOLD: real processes; run only once runner containment is settled"]
fn a_full_start_slot_table_refuses_to_spawn_and_evicts_nothing() {
    let _serialize = serialize_real_broker_test();
    let refused_occupants = admit(&fixture_domain("slot-capacity-refused-occupants"), 4);
    let slot_owner = admit(&fixture_domain("slot-capacity-owner"), 4);
    let paths: Vec<PathBuf> = (0..MAX_START_SLOTS)
        .map(|index| test_descriptor_path(&format!("slot-capacity-{index}")))
        .collect();
    let mut directories = StartSlotDirectories::new(&slot_owner, paths.iter().cloned());
    let mut occupied = Vec::new();
    for path in &paths {
        let child = spawn_controlled_occupant();
        match occupy_start_slot_for_test(path, child) {
            Ok(()) => occupied.push(true),
            Err((_refusal, child)) => {
                // Owned at once, never dropped.
                let _ = refused_occupants.own(Box::new(OwnedChild::new(child)));
                occupied.push(false);
            }
        }
    }
    let first_duplicate = duplicate_start_slot_child(&paths[0]);
    let held_before: Vec<_> = paths.iter().map(|path| start_slot_state(path)).collect();
    let new_path = test_descriptor_path("slot-capacity-new");
    directories.watch(&new_path);
    let (spawns, sink) = BrokerSpawnSinkGuard::install(&new_path, SpawnInjection::default());
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
        descriptor_path: Some(new_path.clone()),
        ..Gateway::default()
    });
    let refused = resource_origin_runtime()
        .block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})))
        .err()
        .map(|error| error.message)
        .unwrap_or_default();
    adapter.stop();
    let spawned = sink.lock().unwrap().len();
    drop(spawns);
    let held_after: Vec<_> = paths.iter().map(|path| start_slot_state(path)).collect();
    // Only now is the first occupant ended, by the test, through its own
    // handle; its exit is observed on that handle before the next reserve.
    let ended = match first_duplicate {
        Some(Ok(handle)) => {
            let mut first = HeldProcess::new(handle);
            first.terminate().is_ok()
                && wait_until(Duration::from_secs(5), || {
                    first.observe() == ExitObservation::Exited
                })
        }
        _ => false,
    };
    let admitted = reserve_start_slot_for_test(&new_path).is_ok();
    let first_released = start_slot_state(&paths[0]).is_none();
    let others_held: Vec<_> = paths[1..]
        .iter()
        .map(|path| start_slot_state(path))
        .collect();
    let reaped: Vec<bool> = paths[1..]
        .iter()
        .map(|path| directories.reap(path))
        .collect();
    let refused_reaped = TEST_OWNER
        .reap(refused_occupants.domain(), Duration::from_secs(5))
        .is_empty();
    // The directories go when `directories` drops, here or on a failed
    // assertion, each only once its slot is confirmed released.

    assert!(occupied.iter().all(|occupied| *occupied), "{occupied:?}");
    assert!(refused.contains("start slots are held"), "{refused}");
    assert_eq!(spawned, 0, "no broker was spawned");
    assert_eq!(held_after, held_before, "nothing was evicted or replaced");
    assert!(
        ended,
        "the test ended the first occupant through its handle"
    );
    assert!(
        admitted,
        "its observed exit released the slot; one start was permitted"
    );
    assert!(first_released);
    assert_eq!(others_held, held_before[1..].to_vec());
    assert!(reaped.iter().all(|reaped| *reaped), "{reaped:?}");
    assert!(refused_reaped);
}

/// Both duplicates fail and the attach takes
/// the Published-failure arm. Cleanup reports the unowned spawn and keeps the
/// fixture directory; the fixture's final owner cleanup reaps its broker.
/// No shared production broker is involved.
#[cfg(windows)]
#[test]
fn double_duplicate_failure_with_a_published_failure_blocks_cleanup() {
    let _serialize = serialize_real_broker_test();
    let (report, _primary, directory, live, _) = real_broker_failing_after_spawn(
        "broker-double-duplicate-published",
        SpawnInjection {
            fail_clone: true,
            fail_kill: true,
            fail_retain_clone: true,
            attach_outcome: AttachOverride::PublishedFailure,
            ..SpawnInjection::default()
        },
    );
    let kept = directory.exists();
    let removed = remove_dir_with_retries(&directory);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("no handle could be retained")),
        "{report}"
    );
    assert!(
        report.blocked,
        "the unowned spawn keeps cleanup conservative: {report}"
    );
    assert!(kept, "the blocked cleanup kept its fixture directory");
    assert!(!live, "the final reap reported no failure");
    assert_eq!(removed, Ok(()));
}

/// A panic right after acquisition, and during
/// cleanup: the start panics the instant the child is in its slot; a
/// cleanup step then panics too. The production slot still holds the same
/// child after both, and the test ends it through that same `Child`. No pid
/// lookup is involved.
#[cfg(windows)]
#[test]
fn a_panic_right_after_spawn_and_in_cleanup_leaves_the_child_in_its_slot() {
    let _serialize = serialize_real_broker_test();
    let slot_owner = admit(&fixture_domain("slot-panic-after-spawn-owner"), 1);
    let descriptor_path = test_descriptor_path("slot-panic-after-spawn");
    let mut directories = StartSlotDirectories::new(&slot_owner, [descriptor_path.clone()]);
    let slot_in_body: Mutex<Option<(Option<u32>, Option<String>)>> = Mutex::new(None);
    let (outcome, report) = run_with_cleanup(|cleanup| {
        cleanup.arm("panics during cleanup", |_| {
            panic!("injected cleanup panic")
        });
        let (_spawns, _sink) = BrokerSpawnSinkGuard::install(
            &descriptor_path,
            SpawnInjection {
                no_sink_duplicate: true,
                panic_after_spawn: true,
                ..SpawnInjection::default()
            },
        );
        let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
            descriptor_path: Some(descriptor_path.clone()),
            ..Gateway::default()
        });
        let _ = resource_origin_runtime().block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})));
        *slot_in_body.lock().unwrap() = start_slot_state(&descriptor_path);
        adapter.stop();
        panic!("injected failure");
    });
    let slot_after = start_slot_state(&descriptor_path);
    let reaped = directories.reap(&descriptor_path);

    assert!(outcome.is_err());
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("panicked: injected cleanup panic")),
        "{report}"
    );
    let in_body = slot_in_body
        .into_inner()
        .unwrap()
        .expect("held in the body");
    assert!(in_body.0.is_some(), "the original child is in the slot");
    assert_eq!(
        slot_after,
        Some(in_body),
        "the same child after both panics"
    );
    assert!(reaped, "ended through the same Child");
}

/// A successful production Unpublished cleanup,
/// proved with no auxiliary duplicate. The kill takes effect, the exit is
/// observed through the slot's own `Child`, the slot is released, and the
/// failure returned names the primary failure and the acknowledged exit.
#[cfg(windows)]
#[test]
fn a_successful_unpublished_cleanup_needs_no_duplicate() {
    let _serialize = serialize_real_broker_test();
    let slot_owner = admit(&fixture_domain("slot-unpublished-success-owner"), 1);
    let descriptor_path = test_descriptor_path("slot-unpublished-success");
    let mut directories = StartSlotDirectories::new(&slot_owner, [descriptor_path.clone()]);
    let (_spawns, sink) = BrokerSpawnSinkGuard::install(
        &descriptor_path,
        SpawnInjection {
            no_sink_duplicate: true,
            attach_outcome: AttachOverride::Unpublished,
            ..SpawnInjection::default()
        },
    );
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
        descriptor_path: Some(descriptor_path.clone()),
        ..Gateway::default()
    });
    let error = resource_origin_runtime()
        .block_on(adapter.invoke(rpc::RUNTIME_STATUS, json!({})))
        .err()
        .map(|error| error.message)
        .unwrap_or_default();
    adapter.stop();
    let slot = start_slot_state(&descriptor_path);
    let recorded = sink.lock().unwrap().len();
    let left = directories.reap(&descriptor_path);

    assert!(
        error.contains("injected: the broker did not publish"),
        "{error}"
    );
    assert!(error.contains("cleanup: kill: ok; exited"), "{error}");
    assert_eq!(slot, None, "the slot was released after the observed exit");
    assert_eq!(recorded, 0, "no auxiliary duplicate existed");
    assert!(!left, "nothing was left in the slot");
}

/// Runs `body` with a cleanup stack holding a sink entry, a stand-in that
/// does not exit when told (no-op terminator, 20 ms bounds), then checks
/// ownership after the stack is gone. `panics` makes the body panic.
#[cfg(windows)]
fn sink_entry_owned_after_cleanup(name: &str, panics: bool) -> (Vec<u32>, u32, bool, bool) {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path(name);
    let label = descriptor_path.display().to_string();
    let pid: Mutex<Option<u32>> = Mutex::new(None);
    // The stand-in's original is owned in its own domain, reserved
    // before anything is spawned; the fixture under test gets a duplicate.
    let originals = admit(&fixture_domain(&format!("{name}-originals")), 1);
    let (_outcome, _report) = run_with_cleanup(|cleanup| {
        let (spawns, sink) = arm_spawned_broker(
            cleanup,
            &descriptor_path,
            Duration::from_millis(20),
            Duration::from_millis(20),
            SpawnInjection::default(),
            |_| Ok(()),
        );
        let (stand_in, entry) = sleeper_in_sink(&originals);
        sink.lock().unwrap().push(entry);
        *pid.lock().unwrap() = Some(stand_in);
        drop(sink);
        drop(spawns);
        if panics {
            panic!("injected failure");
        }
    });
    let pid = pid.into_inner().unwrap().expect("a stand-in");
    let owned = TEST_OWNER.pids(&label);
    let reaped = TEST_OWNER.reap(&label, Duration::from_secs(5)).is_empty()
        && !TEST_OWNER.is_reserved(&label);
    let exited = owned_exited(originals.domain(), pid);
    let originals_reaped = TEST_OWNER
        .reap(originals.domain(), Duration::from_secs(5))
        .is_empty();
    (owned, pid, reaped && originals_reaped, exited)
}

/// After the cleanup stack - the last holder of the
/// sink - has returned and been destroyed, the unacknowledged process is
/// still owned (by the test owner), and is reaped through the same handle.
#[cfg(windows)]
#[test]
fn a_sink_entry_whose_stop_fails_is_owned_after_cleanup_returns() {
    let (owned, pid, reaped, exited) = sink_entry_owned_after_cleanup("owned-after-return", false);
    assert_eq!(owned, vec![pid]);
    assert!(reaped && exited);
}

/// The same, with the test body panicking.
#[cfg(windows)]
#[test]
fn a_sink_entry_whose_stop_fails_is_owned_after_a_panicking_cleanup() {
    let (owned, pid, reaped, exited) = sink_entry_owned_after_cleanup("owned-after-panic", true);
    assert_eq!(owned, vec![pid]);
    assert!(reaped && exited);
}

/// A `ChildSlot` stand-in whose kill does not take effect, with
/// the slot's local `Arc` dropped in the body. After the cleanup stack is
/// gone, the stand-in is owned by the test owner and reaped through it.
#[cfg(windows)]
fn slot_child_owned_after_cleanup(what: &'static str, panics: bool) -> (Vec<u32>, u32, bool) {
    let pid: Mutex<Option<u32>> = Mutex::new(None);
    let domain: Mutex<Option<String>> = Mutex::new(None);
    let (_outcome, _report) = run_with_cleanup(|cleanup| {
        let slot: ChildSlot = Default::default();
        // Admitted (inside `arm_owned_child_with`) before the child
        // is spawned.
        *domain.lock().unwrap() = Some(arm_owned_child_with(
            cleanup,
            what,
            &slot,
            None,
            Duration::from_millis(20),
            Duration::from_millis(20),
            |_| Ok(()),
        ));
        let child = OwnedChild::new(spawn_sleeper());
        let child_pid = child.recorded_pid;
        fill_slot(&slot, child);
        *pid.lock().unwrap() = Some(child_pid);
        drop(slot);
        if panics {
            panic!("injected failure");
        }
    });
    let pid = pid.into_inner().unwrap().expect("a child");
    let label = domain.into_inner().unwrap().expect("a domain");
    let owned = TEST_OWNER.pids(&label);
    let reaped = TEST_OWNER.reap(&label, Duration::from_secs(5)).is_empty()
        && !TEST_OWNER.is_reserved(&label);
    (owned, pid, reaped)
}

/// After cleanup returns.
#[cfg(windows)]
#[test]
fn a_slot_child_whose_stop_fails_is_owned_after_cleanup_returns() {
    let (owned, pid, reaped) = slot_child_owned_after_cleanup("slot-after-return", false);
    assert_eq!(owned, vec![pid]);
    assert!(reaped);
}

/// With a panicking cleanup body.
#[cfg(windows)]
#[test]
fn a_slot_child_whose_stop_fails_is_owned_after_a_panicking_cleanup() {
    let (owned, pid, reaped) = slot_child_owned_after_cleanup("slot-after-panic", true);
    assert_eq!(owned, vec![pid]);
    assert!(reaped);
}

/// The production cleanup through the `SpawnedProcess` seam with a fake
/// process - no real process at all. A kill error and an always-running
/// status come back in the report with the bound; the borrowed process is
/// still the caller's afterwards (the helper cannot take it).
#[test]
fn reap_unpublished_reports_kill_and_status_and_keeps_the_borrow() {
    struct Fake {
        kills: u32,
        polls: u32,
        wait_fails: bool,
    }
    impl SpawnedProcess for Fake {
        fn kill(&mut self) -> std::io::Result<()> {
            self.kills += 1;
            Err(std::io::Error::other("fake kill failure"))
        }
        fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
            self.polls += 1;
            if self.wait_fails {
                Err(std::io::Error::other("fake wait failure"))
            } else {
                Ok(None)
            }
        }
    }
    let mut running = Fake {
        kills: 0,
        polls: 0,
        wait_fails: false,
    };
    let reap = reap_unpublished(&mut running, Duration::from_millis(120));
    let described = reap.describe(Duration::from_millis(120));
    assert!(!reap.exited);
    assert!(
        described.contains("kill failed: fake kill failure"),
        "{described}"
    );
    assert!(described.contains("still running"), "{described}");
    assert!(described.contains("of 120 ms"), "{described}");
    assert_eq!(running.kills, 1, "one kill, no retry");
    assert!(running.polls >= 2, "polled until the bound");

    let mut unknown = Fake {
        kills: 0,
        polls: 0,
        wait_fails: true,
    };
    let reap = reap_unpublished(&mut unknown, Duration::from_millis(120));
    assert!(!reap.exited);
    assert!(reap
        .describe(Duration::from_millis(120))
        .contains("exit status unknown: fake wait failure"));
    assert_eq!(unknown.polls, 1, "an unknown status ends the poll at once");
}

/// Where a `FakeProcess` panics, once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FakePanic {
    Never,
    Clear,
    Terminate,
    Observe,
}

/// A process stand-in with no OS process at all: its "exit" is a
/// shared flag only `terminate` sets, so ownership and release can be
/// asserted deterministically. It panics once at `panic_at`.
struct FakeProcess {
    pid: u32,
    exited: std::sync::Arc<std::sync::atomic::AtomicBool>,
    panic_at: FakePanic,
    terminate_works: bool,
}

impl FakeProcess {
    fn new(pid: u32) -> (Self, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        (
            Self {
                pid,
                exited: exited.clone(),
                panic_at: FakePanic::Never,
                terminate_works: true,
            },
            exited,
        )
    }

    fn panics_at(mut self, point: FakePanic) -> Self {
        self.panic_at = point;
        self
    }

    fn fire(&mut self, point: FakePanic) {
        if self.panic_at == point {
            self.panic_at = FakePanic::Never;
            panic!(
                "injected panic in {} of fake pid {}",
                fake_point(point),
                self.pid
            );
        }
    }
}

fn fake_point(point: FakePanic) -> &'static str {
    match point {
        FakePanic::Never => "nothing",
        FakePanic::Clear => "clear_injection",
        FakePanic::Terminate => "terminate",
        FakePanic::Observe => "observe",
    }
}

impl ProcessControl for FakeProcess {
    fn recorded_pid(&self) -> u32 {
        self.pid
    }
    fn observe(&mut self) -> ExitObservation {
        self.fire(FakePanic::Observe);
        if self.exited.load(std::sync::atomic::Ordering::SeqCst) {
            ExitObservation::Exited
        } else {
            ExitObservation::Running
        }
    }
    fn terminate(&mut self) -> std::io::Result<()> {
        self.fire(FakePanic::Terminate);
        if self.terminate_works {
            self.exited.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
    fn clear_injection(&mut self) {
        self.fire(FakePanic::Clear);
        self.terminate_works = true;
    }
}

/// A private owner for one test, so the shared fixture owner's other users
/// cannot disturb its capacity arithmetic.
fn private_owner(units: usize) -> &'static TestOwner {
    Box::leak(Box::new(TestOwner::new(units)))
}

/// Admission happens before acquisition. A held
/// domain and an exhausted capacity are refused, and the acquisition step
/// (counted) never runs for a refused reservation. A permit dropped with
/// nothing owned releases its units; one dropped while owning keeps its
/// domain reserved, refusing re-admission, until a reap observes the exit.
/// No process is involved.
#[test]
fn the_test_owner_reserves_before_acquisition_and_refuses_when_full() {
    let owner = private_owner(2);
    let acquired = std::sync::atomic::AtomicUsize::new(0);
    let admit_then_acquire = |domain: &str| {
        owner.reserve(domain, 1).inspect(|_| {
            acquired.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
    };

    let first = admit_then_acquire("a").expect("a is admitted");
    assert!(admit_then_acquire("a").is_err(), "a held domain is refused");
    let second = admit_then_acquire("b").expect("b is admitted");
    let full = match admit_then_acquire("c") {
        Ok(_) => panic!("capacity is exhausted"),
        Err(error) => error,
    };
    assert!(full.contains("2 of 2 units reserved"), "{full}");
    assert_eq!(
        acquired.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "nothing was acquired for a refused reservation"
    );

    drop(second);
    assert!(
        !owner.is_reserved("b"),
        "an empty permit releases its domain"
    );
    let third = admit_then_acquire("c").expect("the released units admit c");

    let (fake, exited) = FakeProcess::new(41);
    first.own(Box::new(fake)).expect("within the reservation");
    drop(first);
    assert!(owner.is_reserved("a"), "an owning domain stays reserved");
    assert_eq!(owner.pids("a"), vec![41]);
    assert!(admit_then_acquire("a").is_err(), "and refuses re-admission");
    assert!(
        owner.reserve("d", 1).is_err(),
        "and still counts against capacity"
    );

    assert!(owner.reap("a", Duration::from_secs(1)).is_empty());
    assert!(exited.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        !owner.is_reserved("a"),
        "the observed exit released the domain"
    );
    drop(third);
    assert!(owner.reserve("a", 2).is_ok(), "all units are free again");
}

/// A process beyond the reserved units is still
/// owned - never dropped - and the overflow is reported.
#[test]
fn an_overflowing_permit_still_owns_every_process() {
    let owner = private_owner(4);
    let permit = owner.reserve("overflow", 1).expect("admitted");
    let (one, _) = FakeProcess::new(1);
    let (two, _) = FakeProcess::new(2);
    assert!(permit.own(Box::new(one)).is_ok());
    let overflow = permit
        .own(Box::new(two))
        .expect_err("beyond the reservation");
    assert!(
        overflow.contains("2 processes owned, 1 reserved"),
        "{overflow}"
    );
    assert_eq!(owner.pids("overflow"), vec![1, 2], "both are still owned");
    assert!(owner.reap("overflow", Duration::from_secs(1)).is_empty());
}

/// Race control: two threads reserve the last unit
/// at the same moment (a barrier releases them together); exactly one is
/// admitted.
#[test]
fn two_racing_reservations_for_the_last_unit_admit_exactly_one() {
    let owner = private_owner(1);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let racers: Vec<_> = ["left", "right"]
        .into_iter()
        .map(|domain| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                owner.reserve(domain, 1).map(|permit| {
                    let domain = permit.domain().to_owned();
                    std::mem::forget(permit);
                    domain
                })
            })
        })
        .collect();
    let outcomes: Vec<_> = racers
        .into_iter()
        .map(|racer| racer.join().expect("racer thread"))
        .collect();
    let admitted: Vec<_> = outcomes.iter().filter(|outcome| outcome.is_ok()).collect();
    assert_eq!(admitted.len(), 1, "{outcomes:?}");
}

/// Owns three fakes in one domain, the middle one panicking once at
/// `point`, and runs `stop` under `catch_unwind`. Returns the owner, the
/// domain, the pids still owned right after the panic, and the exit flags.
fn panic_inside_a_callback(
    point: FakePanic,
    stop: impl FnOnce(&'static TestOwner),
) -> (
    &'static TestOwner,
    Vec<u32>,
    Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) {
    let owner = private_owner(3);
    let permit = owner.reserve("panics", 3).expect("admitted");
    let mut flags = Vec::new();
    for pid in [1, 2, 3] {
        let (fake, exited) = FakeProcess::new(pid);
        let fake = if pid == 2 {
            fake.panics_at(point)
        } else {
            fake
        };
        permit.own(Box::new(fake)).expect("within the reservation");
        flags.push(exited);
    }
    drop(permit);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stop(owner)));
    assert!(panicked.is_err(), "the injected panic fired");
    let mut owned = owner.pids("panics");
    owned.sort_unstable();
    (owner, owned, flags)
}

/// A panic inside `terminate` during the
/// test's own reap. Entry 1 was reaped before it; entry 2 (in hand) is
/// returned by `InFlight`; entry 3 never left. Both are still owned after
/// the panic, and a second reap ends them through the same objects.
#[test]
fn a_panic_in_terminate_during_a_reap_keeps_every_other_entry_owned() {
    let (owner, owned, flags) = panic_inside_a_callback(FakePanic::Terminate, |owner| {
        owner.reap("panics", Duration::from_millis(50));
    });
    assert_eq!(owned, vec![2, 3], "the entry in hand and the unvisited one");
    assert!(
        flags[0].load(std::sync::atomic::Ordering::SeqCst),
        "entry 1 was reaped"
    );
    assert!(owner.reap("panics", Duration::from_secs(1)).is_empty());
    assert!(flags
        .iter()
        .all(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)));
    assert!(!owner.is_reserved("panics"));
}

/// The same with a panic inside `observe`, during a cleanup's
/// `stop_domain` (the path every fixture step uses).
#[test]
fn a_panic_in_observe_during_a_cleanup_stop_keeps_every_other_entry_owned() {
    let (owner, owned, flags) = panic_inside_a_callback(FakePanic::Observe, |owner| {
        let mut report = CleanupReport::default();
        owner.stop_domain(
            "panics",
            Duration::ZERO,
            Duration::from_millis(50),
            false,
            &mut report,
        );
    });
    assert_eq!(owned, vec![2, 3]);
    assert!(flags[0].load(std::sync::atomic::Ordering::SeqCst));
    assert!(owner.reap("panics", Duration::from_secs(1)).is_empty());
    assert!(flags
        .iter()
        .all(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)));
}

/// The same with a panic inside `clear_injection`, at the start of the
/// test's own reap of that entry.
#[test]
fn a_panic_in_clear_injection_during_a_reap_keeps_every_other_entry_owned() {
    let (owner, owned, flags) = panic_inside_a_callback(FakePanic::Clear, |owner| {
        owner.reap("panics", Duration::from_millis(50));
    });
    assert_eq!(owned, vec![2, 3]);
    assert!(flags[0].load(std::sync::atomic::Ordering::SeqCst));
    assert!(owner.reap("panics", Duration::from_secs(1)).is_empty());
    assert!(flags
        .iter()
        .all(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)));
}

/// The pid whose terminate panics in `panicking_terminate`.
#[cfg(windows)]
static PANICKING_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(windows)]
fn panicking_terminate(held: &mut HeldProcess) -> std::io::Result<()> {
    if held.recorded_pid() == PANICKING_PID.load(std::sync::atomic::Ordering::SeqCst) {
        panic!("injected panic in terminate of pid {}", held.recorded_pid());
    }
    held.terminate()
}

/// Through a real cleanup stack: three
/// sleepers' handles in one broker fixture sink; the terminate of the
/// second panics inside the `broker` cleanup step. The step's panic is
/// reported; after the cleanup stack is destroyed, the second sleeper is
/// still owned under the fixture domain (the first and third were stopped);
/// the test's own reap then ends it through the same handle. This test spawns
/// real sleepers.
#[cfg(windows)]
#[test]
fn a_panic_in_a_sink_entrys_terminate_keeps_it_owned_after_the_cleanup_stack() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("sink-terminate-panics");
    let label = descriptor_path.display().to_string();
    // The three originals are owned in their own domain, reserved
    // before anything is spawned; the broker fixture gets duplicates.
    let originals = admit(&fixture_domain("sink-terminate-panics-originals"), 3);
    let pids: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    let (_outcome, report) = run_with_cleanup(|cleanup| {
        let (spawns, sink) = arm_spawned_broker(
            cleanup,
            &descriptor_path,
            Duration::ZERO,
            Duration::from_secs(5),
            SpawnInjection::default(),
            panicking_terminate,
        );
        for index in 0..3 {
            let (pid, entry) = sleeper_in_sink(&originals);
            if index == 1 {
                PANICKING_PID.store(pid, std::sync::atomic::Ordering::SeqCst);
            }
            sink.lock().unwrap().push(entry);
            pids.lock().unwrap().push(pid);
        }
        drop(sink);
        drop(spawns);
    });
    let pids = pids.into_inner().unwrap();
    let (first, second, third) = (pids[0], pids[1], pids[2]);
    // `stop_domain` visits the entries one at a time and does not catch a
    // callback's panic: the first is stopped, the second's terminate panics
    // (its `InFlight` puts it back), and the loop unwinds before the third
    // is visited. Both the second and the third stay owned.
    let mut owned_after = TEST_OWNER.pids(&label);
    owned_after.sort_unstable();
    let mut expected = vec![second, third];
    expected.sort_unstable();
    let first_exited = owned_exited(originals.domain(), first);
    let second_running =
        TEST_OWNER.observe_owned(originals.domain(), second) == Some(ExitObservation::Running);
    let third_running =
        TEST_OWNER.observe_owned(originals.domain(), third) == Some(ExitObservation::Running);
    let reaped = TEST_OWNER.reap(&label, Duration::from_secs(5)).is_empty()
        && !TEST_OWNER.is_reserved(&label);
    let second_exited = owned_exited(originals.domain(), second);
    let third_exited = owned_exited(originals.domain(), third);
    let originals_reaped = TEST_OWNER
        .reap(originals.domain(), Duration::from_secs(5))
        .is_empty();

    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("cleanup step `broker` panicked")),
        "{report}"
    );
    assert_eq!(
        owned_after, expected,
        "the panicking entry and the unvisited one stay owned"
    );
    assert!(first_exited, "the first entry was stopped before the panic");
    assert!(
        second_running,
        "the panicking entry was not ended by the panic"
    );
    assert!(third_running, "the unvisited entry was never stopped");
    assert!(
        reaped,
        "both were reaped through their own handles afterwards"
    );
    assert!(second_exited && third_exited);
    assert!(originals_reaped);
}

/// A relay-marked read on a healthy connection
/// with an open session is served, and reaches the broker marked.
#[test]
fn a_relay_marked_read_on_a_healthy_session_is_served() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("relay-read-healthy");
    let (gateway, broker) = gateway_with_admitting_broker(&descriptor_path, true);
    let AdmittingBroker {
        close,
        demands,
        reopened,
        ..
    } = broker;
    // Nothing closes the session: the broker's pause ends at once.
    drop(close);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(gateway);
    let runtime = resource_origin_runtime();

    let served = runtime.block_on(adapter.invoke_resource_read(rpc::LIST_MODELS, json!({})));
    assert!(
        served.is_ok(),
        "{:?}",
        served.err().map(|error| error.message)
    );
    assert_eq!(
        demands.try_iter().collect::<Vec<_>>(),
        vec![(
            rpc::LIST_MODELS.to_string(),
            Some(BROKER_RESOURCE_READ_ORIGIN.to_string())
        )]
    );
    assert_eq!(reopened.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// A headless adapter over a gateway that holds no broker connection and
/// meets its broker at `descriptor_path`, with `GatewayProcess::start`
/// replaced by a counting starter that launches nothing.
fn adapter_over_counted_start_seam(
    descriptor_path: &Path,
) -> (local_stdio::HeadlessDeviceAdapter, GatewayStartSeam) {
    let seam = GatewayStartSeam::install(descriptor_path);
    let adapter = local_stdio::HeadlessDeviceAdapter::over_gateway(Gateway {
        descriptor_path: Some(descriptor_path.to_path_buf()),
        ..Gateway::default()
    });
    (adapter, seam)
}

/// A relay-marked resource read with no broker never reaches
/// `GatewayProcess::start` - the one place an attach, an election, a shared
/// spawn or a private `--stdio` child begins - and is refused offline.
#[test]
fn a_relay_marked_read_with_no_broker_never_reaches_the_start_seam() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("seam-relay-read-no-broker");
    let (adapter, seam) = adapter_over_counted_start_seam(&descriptor_path);
    let runtime = resource_origin_runtime();

    let refused = runtime
        .block_on(adapter.invoke_resource_read(rpc::LIST_MODELS, json!({})))
        .expect_err("nothing may serve a relay-marked resource read");
    assert_eq!(refused.code, "DEVICE_TRANSPORT");
    assert_eq!(refused.message, local_stdio::OFFLINE_ADVICE);
    assert!(refused.retryable);
    assert_eq!(seam.starts(), 0, "no start was attempted");
    assert!(!adapter.gateway_attached());
}

/// The same with a stale descriptor on disk.
#[test]
fn a_relay_marked_read_with_a_stale_descriptor_never_reaches_the_start_seam() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("seam-relay-read-stale");
    fs::write(
        &descriptor_path,
        br#"{"processId":4294967294,"port":1,"instanceId":"stale"}"#,
    )
    .unwrap();
    let (adapter, seam) = adapter_over_counted_start_seam(&descriptor_path);
    let runtime = resource_origin_runtime();

    let refused = runtime
        .block_on(adapter.invoke_resource_read(rpc::CURRENT_PRESET, json!({})))
        .expect_err("a stale descriptor is not a broker to read from");
    assert_eq!(refused.code, "DEVICE_TRANSPORT");
    assert_eq!(refused.message, local_stdio::OFFLINE_ADVICE);
    assert!(refused.retryable);
    assert_eq!(seam.starts(), 0, "no start was attempted");
}

/// Control: an unmarked call through the same adapter entry
/// reaches the start seam exactly once, as a tool call does today. The
/// counting starter refuses, so no process exists to leak; the real-broker
/// control `an_unmarked_relay_call_through_the_same_adapter_still_brings_the_broker_up`
/// stays as the end-to-end check.
#[test]
fn an_unmarked_relay_call_reaches_the_start_seam_once() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("seam-relay-tool-call");
    let (adapter, seam) = adapter_over_counted_start_seam(&descriptor_path);
    let runtime = resource_origin_runtime();

    let failed = runtime
        .block_on(adapter.invoke(rpc::LIST_MODELS, json!({})))
        .expect_err("the counting starter starts nothing");
    assert!(
        failed.message.contains(GatewayStartSeam::REFUSAL),
        "{}",
        failed.message
    );
    assert_eq!(seam.starts(), 1, "a tool call still asks for a broker");
    assert!(!adapter.gateway_attached());
}

/// The MCP-scoped path (the local `/mcp` surfaces and a bridge
/// that hosts, through `LocalBackend`) never reaches the start seam for a
/// resource read either.
#[test]
fn an_mcp_resource_read_never_reaches_the_start_seam() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("seam-mcp-resource-read");
    let (adapter, seam) = adapter_over_counted_start_seam(&descriptor_path);
    let backend = local_backend_over(&adapter);
    let runtime = resource_origin_runtime();

    let refused = runtime.block_on(qc_remote_mcp::as_resource_read({
        use qc_remote_mcp::QcBackend as _;
        backend.request(&local_mcp_route(), rpc::LIST_MODELS, serde_json::Map::new())
    }));
    assert!(refused.is_err(), "{:?}", refused.as_ref().ok());
    assert_eq!(seam.starts(), 0, "no start was attempted");
}

/// A gateway with no descriptor path - the private `--stdio`
/// child's branch of `GatewayProcess::start` - never reaches its start for an
/// `ExistingOnly` read. The counting starter belongs to this `Gateway` value
/// (`Gateway::count_starts_refusing`), not to a registry, so the `None` key
/// cannot collide with any other test running in parallel.
#[test]
fn an_existing_only_read_with_no_descriptor_never_starts_the_private_stdio_child() {
    let mut gateway = Gateway::default();
    let starts = gateway.count_starts_refusing();
    assert_eq!(
        gateway.shared_descriptor_path(),
        None,
        "unit tests never run the Tauri setup that resolves a descriptor"
    );

    let refused =
        gateway.request_detailed_as(rpc::LIST_MODELS, json!({}), ConnectionUse::ExistingOnly);
    assert!(
        matches!(
            &refused,
            Err(GatewayRequestFailure::Transport(message)) if message == local_stdio::OFFLINE_ADVICE
        ),
        "{:?}",
        refused.as_ref().ok()
    );
    assert!(starts.lock().unwrap().is_empty(), "no start was attempted");
    assert!(gateway.process.is_none());
}

/// Control: an ordinary call on the same kind of gateway
/// reaches the private `--stdio` start exactly once (the counting starter
/// launches nothing).
#[test]
fn an_ordinary_call_with_no_descriptor_reaches_the_private_stdio_start_once() {
    let mut gateway = Gateway::default();
    let starts = gateway.count_starts_refusing();

    let failed = gateway.request_detailed(rpc::LIST_MODELS, json!({}));
    assert!(
        matches!(
            &failed,
            Err(GatewayRequestFailure::Transport(message)) if message == GatewayStartSeam::REFUSAL
        ),
        "{:?}",
        failed.as_ref().ok()
    );
    assert_eq!(
        *starts.lock().unwrap(),
        vec![None],
        "one start, on the private --stdio branch"
    );
}

/// The gateway, lock-free read slot and start record a relay adapter hands
/// `relay_gateway_request`, with this gateway's start counted and refused.
type RelayHelperFixture = (
    Mutex<Gateway>,
    LockFreeReadSlot,
    std::sync::Arc<Mutex<Vec<Option<PathBuf>>>>,
);

fn relay_helper_fixture(descriptor_path: Option<PathBuf>) -> RelayHelperFixture {
    let mut gateway = Gateway {
        descriptor_path,
        ..Gateway::default()
    };
    let starts = gateway.count_starts_refusing();
    let reads = gateway.lock_free_reads.clone();
    (Mutex::new(gateway), reads, starts)
}

/// `relay_gateway_request`, the call
/// `WindowsRelayAdapter::invoke_as` makes (with `stopped: None`) and
/// `HeadlessDeviceAdapter::invoke_gateway` makes (with its stop flag), never
/// reaches a broker start for an `ExistingOnly` read - with a shared
/// descriptor path or none - and answers offline.
#[test]
fn the_relay_helper_never_starts_a_broker_for_an_existing_only_read() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("relay-helper-existing-only");
    let running = std::sync::atomic::AtomicBool::new(false);
    for descriptor in [None, Some(descriptor_path.clone())] {
        for stopped in [None, Some(&running)] {
            let (gateway, reads, starts) = relay_helper_fixture(descriptor.clone());
            let refused = relay_gateway_request(
                &gateway,
                &reads,
                rpc::CURRENT_PRESET,
                json!({}),
                ConnectionUse::ExistingOnly,
                stopped,
            );
            assert!(
                matches!(
                    &refused,
                    Err(GatewayRequestFailure::Transport(message))
                        if message == local_stdio::OFFLINE_ADVICE
                ),
                "{descriptor:?}: {:?}",
                refused.as_ref().ok()
            );
            assert!(
                starts.lock().unwrap().is_empty(),
                "{descriptor:?}: no start"
            );
        }
    }
}

/// Control: the same helper with an ordinary call reaches
/// the broker start exactly once, on the descriptor path the gateway was
/// given (`None`: the private `--stdio` child).
#[test]
fn the_relay_helper_starts_a_broker_once_for_an_ordinary_call() {
    let (_directory_guard, descriptor_path) = guarded_descriptor_path("relay-helper-ordinary");
    for descriptor in [None, Some(descriptor_path.clone())] {
        let (gateway, reads, starts) = relay_helper_fixture(descriptor.clone());
        let failed = relay_gateway_request(
            &gateway,
            &reads,
            rpc::LIST_MODELS,
            json!({}),
            ConnectionUse::MayConnect,
            None,
        );
        assert!(
            matches!(
                &failed,
                Err(GatewayRequestFailure::Transport(message))
                    if message == GatewayStartSeam::REFUSAL
            ),
            "{descriptor:?}: {:?}",
            failed.as_ref().ok()
        );
        assert_eq!(*starts.lock().unwrap(), vec![descriptor.clone()]);
    }
}
