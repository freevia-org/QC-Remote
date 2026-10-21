use crate::generated_broker_surface::{BrokerDescriptor, BrokerReleaseMarker, ConnectionRole};
use crate::platform::midi::{PerformanceMidi, PerformanceMidiReceipt};
use crate::tray_settings::IdlePolicy;
use crate::worker::{BrokerStatus, DeviceController, PresetCatalogUnpublished, ReconnectOutcome};
use crate::write_quarantine::{self, WriteQuarantine};
use base64::Engine as _;
use quadcortex::device::owner::DEVICE_SUSPENDED_MARKER;
use quadcortex::device::{
    generated_gateway,
    owner::{DispatchContext, GatewayReadScope, OperationTicket},
    request::{
        self as runtime_request, finalize_device_backup, GatewayResponseProjection,
        GatewayVerification, GatewayVerificationAction, GatewayWritePlan, PlannedWrite,
    },
};
use quadcortex::state::BlockDetails;
use quadcortex::sync::RecoverPoison;
use quadcortex::{domain, profile};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq as _;

mod background;
mod background_policy;
#[cfg(windows)]
mod stdio_pipe;
mod wire;
use wire::{
    error, read_frame_bytes_limited, read_request, write_observer_action_response_with,
    write_response, write_tcp_frame_before_deadline, write_tcp_response, Request,
    RESOURCE_READ_ORIGIN, RESOURCE_READ_ORIGIN_CAPABILITY,
};
#[cfg(test)]
use wire::{read_frame_bytes, write_observer_action_response};

/// How often the phase publisher samples the worker's status.
const PHASE_EVENT_POLL: Duration = Duration::from_millis(100);

/// The `device.phase` notification for one observed worker status.
///
/// Only the fields a host needs to tell "still looking" from "a Quad Cortex
/// answered" travel here; the full record stays behind `system.status`.
fn phase_notification(status: &BrokerStatus) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "device.phase",
        "params": {
            "phase": status.phase,
            "detail": status.detail,
            "connected": status.connected,
            "synchronized": status.synchronized
        }
    })
}

/// Publish every change of the worker's phase until `stop` is raised.
///
/// A `device.reconnect` holds the request loop for as long as the handshake
/// runs - up to the ready-wait timeout, and the boot directory read alone is
/// about eleven seconds - so a host cannot ask `system.status` in the meantime.
/// This is the only way it can learn, while that call is still pending,
/// whether a Quad Cortex has actually answered or nothing is attached.
#[cfg(any(not(windows), test))]
fn publish_phase_changes<W: Write>(
    controller: &DeviceController,
    output: &Mutex<W>,
    stop: &AtomicBool,
) {
    let mut published: Option<(String, String, bool, bool)> = None;
    while !stop.load(Ordering::Acquire) {
        let status = controller.status();
        let observed = (
            status.phase.clone(),
            status.detail.clone(),
            status.connected,
            status.synchronized,
        );
        if published.as_ref() != Some(&observed) {
            let Ok(mut output) = output.lock() else {
                return;
            };
            if write_response(&mut *output, &phase_notification(&status)).is_err() {
                return;
            }
            published = Some(observed);
        }
        thread::sleep(PHASE_EVENT_POLL);
    }
}

/// One request read off the `--stdio` pipe.
///
/// The only writer is the host that spawned this process, over its own pipes
/// (`GatewayProcess::start_owned_child` in `apps/windows/src-tauri/src/lib.rs`).
/// This `handle` also lists [`RESOURCE_READ_ORIGIN_CAPABILITY`] in
/// `system.status`, so such a host sends the resource-read origin here too,
/// and it is enforced, never dropped: a resource read reaches only a device
/// read ([`resource_read_may_reach`]), and only while the session is
/// connected. There is no idle release on stdio and requests run strictly in
/// order, so nothing on this pipe can close the session between the check and
/// the read. Anything without the origin is handled exactly as before.
fn serve_stdio_request(
    controller: &DeviceController,
    performance_midi: &PerformanceMidiWorkers,
    request: Request,
) -> Value {
    if request.origin.as_deref() != Some(RESOURCE_READ_ORIGIN) {
        return handle(controller, performance_midi, request);
    }
    if !resource_read_may_reach(&request.method) {
        return idle_reopen_error(request.id, RESOURCE_READ_NOT_OPEN);
    }
    if request.method != generated_gateway::rpc::RUNTIME_STATUS && !controller.status().connected {
        return idle_reopen_error(request.id, RESOURCE_READ_NOT_OPEN);
    }
    handle(controller, performance_midi, request)
}

#[cfg(windows)]
pub fn serve_stdio(controller: DeviceController, phase_events: bool) -> Result<(), String> {
    let controller = Arc::new(controller);
    let clock = Arc::new(crate::orphan_guard::RequestClock::started_now());
    stdio_pipe::serve(controller, phase_events, clock)
}

#[cfg(not(windows))]
pub(crate) fn serve_stdio(
    controller: DeviceController,
    phase_events: bool,
    #[cfg(target_os = "linux")] signals: crate::platform::shutdown::Pending,
) -> Result<(), String> {
    let request_clock = Arc::new(crate::orphan_guard::RequestClock::started_now());
    let performance_midi = Arc::new(PerformanceMidiWorkers::default());
    #[cfg(target_os = "linux")]
    let owner = StdioControllerOwner {
        controller: Some(controller),
        performance_midi: Arc::clone(&performance_midi),
    };
    #[cfg(target_os = "linux")]
    let controller = owner
        .controller
        .as_ref()
        .expect("stdio controller is owned");
    #[cfg(not(target_os = "linux"))]
    let controller = &controller;
    // The guard exits with `process::exit`, which skips the controller's Drop;
    // it stops the worker itself so the device still sees the disconnect.
    let worker_shutdown = controller.shutdown_handle();
    #[cfg(target_os = "linux")]
    let _signal_watch = signals.watch({
        let midi = Arc::clone(&performance_midi);
        let shutdown = worker_shutdown.clone();
        move || {
            retire_then_stop(
                || midi.retire_all_devices(),
                || {
                    let _ = shutdown.stop_and_join(crate::worker::WORKER_EXIT_BUDGET);
                },
            );
            std::process::exit(0);
        }
    })?;
    crate::orphan_guard::watch(
        crate::orphan_guard::parent_process_id(),
        Arc::clone(&request_clock),
        {
            let midi = Arc::clone(&performance_midi);
            move || {
                #[cfg(target_os = "linux")]
                if !midi.retire_all_devices() {
                    return;
                }
                #[cfg(not(target_os = "linux"))]
                let _ = &midi;
                let _ = worker_shutdown.stop_and_join(crate::worker::WORKER_EXIT_BUDGET);
            }
        },
    );
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let output = Arc::new(Mutex::new(io::stdout()));
    // All MIDI opens, sends, and closes run on per-device detached workers;
    // request handling only queues and waits for a bounded reply.
    let event_output = Arc::clone(&output);
    let events = controller.subscribe_state_events();
    thread::Builder::new()
        .name("qc-native-events".into())
        .spawn(move || {
            while let Ok(frame) = events.recv() {
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "device.stateFrame",
                    "params": frame
                });
                let Ok(mut output) = event_output.lock() else {
                    return;
                };
                if write_response(&mut *output, &notification).is_err() {
                    return;
                }
            }
        })
        .map_err(|error| format!("Could not start native event stream: {error}"))?;
    let stop_phase_events = AtomicBool::new(false);
    thread::scope(|scope| {
        if phase_events {
            let (controller, output, stop) = (controller, &*output, &stop_phase_events);
            thread::Builder::new()
                .name("qc-native-phase".into())
                .spawn_scoped(scope, move || {
                    publish_phase_changes(controller, output, stop)
                })
                .map_err(|error| format!("Could not start native phase stream: {error}"))?;
        }
        let served = (|| {
            while let Some(request) = read_request(&mut input)? {
                if !request_clock.start_request() {
                    return Err("STDIO orphan retirement committed before request dispatch".into());
                }
                let response = serve_stdio_request(controller, &performance_midi, request);
                request_clock.finish_request();
                let mut output = output
                    .lock()
                    .map_err(|_| "Native broker output lock was poisoned".to_string())?;
                write_response(&mut *output, &response)?;
            }
            Ok(())
        })();
        stop_phase_events.store(true, Ordering::Release);
        served
    })
}

#[cfg(target_os = "linux")]
struct StdioControllerOwner {
    controller: Option<DeviceController>,
    performance_midi: Arc<PerformanceMidiWorkers>,
}

#[cfg(target_os = "linux")]
fn retire_then_stop(retire: impl FnOnce() -> bool, stop: impl FnOnce()) -> bool {
    if !retire() {
        return false;
    }
    stop();
    true
}

#[cfg(all(test, target_os = "linux"))]
mod stdio_shutdown_tests {
    use super::{
        retire_then_stop, PerformanceMidiEndpoint, PerformanceMidiReceipt, PerformanceMidiWorkers,
        PERFORMANCE_MIDI_RETIRE_BUDGET,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn stdio_midi_registry_orders_retirement_closes_admission_and_exits_worker() {
        struct TrackingEndpoint {
            events: Arc<Mutex<Vec<&'static str>>>,
            dropped: Arc<AtomicBool>,
        }
        impl PerformanceMidiEndpoint for TrackingEndpoint {
            fn send(
                &mut self,
                _device_id: &str,
                _controller: u8,
                _value: u8,
            ) -> Result<PerformanceMidiReceipt, String> {
                self.events.lock().unwrap().push("send");
                Ok(PerformanceMidiReceipt {
                    endpoint: "fake".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }

            fn retire(&mut self) {
                self.events.lock().unwrap().push("retire");
            }
        }
        impl Drop for TrackingEndpoint {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(AtomicBool::new(false));
        let factory_events = Arc::clone(&events);
        let factory_dropped = Arc::clone(&dropped);
        let workers = PerformanceMidiWorkers::with_factory(move || {
            Box::new(TrackingEndpoint {
                events: Arc::clone(&factory_events),
                dropped: Arc::clone(&factory_dropped),
            })
        });
        workers.send("stdio-fixture", 1, 2, || {}).unwrap();
        assert!(retire_then_stop(
            || workers.retire_all_devices(),
            || events.lock().unwrap().push("hid-stopped"),
        ));
        assert!(workers
            .send("stdio-fixture", 3, 4, || {})
            .err()
            .expect("send after retirement must be rejected")
            .contains("shutting down"));
        assert_eq!(*events.lock().unwrap(), ["send", "retire", "hid-stopped"]);
        drop(workers);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !dropped.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            dropped.load(Ordering::Acquire),
            "MIDI worker did not exit after registry drop"
        );
    }

    #[test]
    fn stdio_retirement_timeout_keeps_worker_quarantined_without_native_leak() {
        struct BlockingEndpoint {
            gate: Arc<(Mutex<bool>, Condvar)>,
            dropped: Arc<AtomicBool>,
        }
        struct ReleaseGateOnDrop(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for ReleaseGateOnDrop {
            fn drop(&mut self) {
                let (released, changed) = &*self.0;
                *released.lock().unwrap() = true;
                changed.notify_all();
            }
        }
        impl PerformanceMidiEndpoint for BlockingEndpoint {
            fn send(
                &mut self,
                _device_id: &str,
                _controller: u8,
                _value: u8,
            ) -> Result<PerformanceMidiReceipt, String> {
                Ok(PerformanceMidiReceipt {
                    endpoint: "fake".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }

            fn retire(&mut self) {
                let (released, changed) = &*self.gate;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = changed.wait(released).unwrap();
                }
            }
        }
        impl Drop for BlockingEndpoint {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = ReleaseGateOnDrop(Arc::clone(&gate));
        let dropped = Arc::new(AtomicBool::new(false));
        let factory_gate = Arc::clone(&gate);
        let factory_dropped = Arc::clone(&dropped);
        let workers = PerformanceMidiWorkers::with_factory(move || {
            Box::new(BlockingEndpoint {
                gate: Arc::clone(&factory_gate),
                dropped: Arc::clone(&factory_dropped),
            })
        });
        workers.send("stdio-timeout-fixture", 1, 2, || {}).unwrap();
        let started = Instant::now();
        let hid_stopped = AtomicBool::new(false);
        assert!(!retire_then_stop(
            || workers.retire_all_devices(),
            || hid_stopped.store(true, Ordering::Release),
        ));
        assert!(!hid_stopped.load(Ordering::Acquire));
        assert!(started.elapsed() < PERFORMANCE_MIDI_RETIRE_BUDGET + Duration::from_millis(250));
        assert!(workers
            .send("stdio-timeout-fixture", 3, 4, || {})
            .err()
            .expect("send after retirement timeout must be rejected")
            .contains("shutting down"));

        let (released, changed) = &*gate;
        *released.lock().unwrap() = true;
        changed.notify_all();
        assert!(workers.retire_device("stdio-timeout-fixture"));
        drop(workers);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !dropped.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            dropped.load(Ordering::Acquire),
            "timed-out MIDI worker did not exit after gate release and registry drop"
        );
    }

    #[test]
    fn full_queue_retirement_closes_on_owner_before_cancelled_send_and_recovers() {
        use super::{MidiCommand, MidiSendCommand, MIDI_CANCELLED};
        struct Endpoint {
            retired: Arc<AtomicBool>,
            sends: Arc<std::sync::atomic::AtomicUsize>,
            dropped: Arc<AtomicBool>,
        }
        impl PerformanceMidiEndpoint for Endpoint {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                assert!(self.retired.load(Ordering::Acquire));
                self.sends.fetch_add(1, Ordering::AcqRel);
                Ok(PerformanceMidiReceipt {
                    endpoint: "fixture".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }
            fn retire(&mut self) {
                self.retired.store(true, Ordering::Release);
            }
        }
        impl Drop for Endpoint {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }
        struct Release(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for Release {
            fn drop(&mut self) {
                let (open, changed) = &*self.0;
                *open.lock().unwrap() = true;
                changed.notify_all();
            }
        }
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let release = Release(Arc::clone(&gate));
        let retired = Arc::new(AtomicBool::new(false));
        let sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let factory_gate = Arc::clone(&gate);
        let factory_retired = Arc::clone(&retired);
        let factory_sends = Arc::clone(&sends);
        let factory_dropped = Arc::clone(&dropped);
        let workers = PerformanceMidiWorkers::with_factory(move || {
            let (open, changed) = &*factory_gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = changed.wait(open).unwrap();
            }
            Box::new(Endpoint {
                retired: Arc::clone(&factory_retired),
                sends: Arc::clone(&factory_sends),
                dropped: Arc::clone(&factory_dropped),
            })
        });
        let (sender, _) = workers.get_or_start("full-queue-fixture").unwrap();
        let (reply, _response) = std::sync::mpsc::channel();
        let (_ack, acknowledged) = std::sync::mpsc::channel();
        sender
            .try_send(MidiCommand::Send(MidiSendCommand {
                device_id: "full-queue-fixture".into(),
                controller: 1,
                value: 2,
                validate: Box::new(|| panic!("cancelled fixture must not validate")),
                state: Arc::new(std::sync::atomic::AtomicU8::new(MIDI_CANCELLED)),
                reply,
                acknowledged,
            }))
            .unwrap_or_else(|_| panic!("fixture queue must accept its first send"));
        assert!(
            !workers.retire_device("full-queue-fixture"),
            "full queue cannot admit the Retire marker"
        );
        assert!(workers.workers.lock().unwrap()["full-queue-fixture"]
            .retirement_pending
            .load(Ordering::Acquire));
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while workers.workers.lock().unwrap()["full-queue-fixture"]
            .retirement_pending
            .load(Ordering::Acquire)
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(retired.load(Ordering::Acquire));
        assert_eq!(sends.load(Ordering::Acquire), 0);
        workers.send("full-queue-fixture", 3, 4, || {}).unwrap();
        assert_eq!(sends.load(Ordering::Acquire), 1);
        assert_eq!(
            workers.workers.lock().unwrap().len(),
            1,
            "same owner recovered; no replacement"
        );
        drop(sender);
        drop(workers);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dropped.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            dropped.load(Ordering::Acquire),
            "fixture endpoint retired on its owner"
        );
    }
}

#[cfg(target_os = "linux")]
impl Drop for StdioControllerOwner {
    fn drop(&mut self) {
        if let Some(controller) = self.controller.take() {
            if self.performance_midi.retire_all_devices() {
                drop(controller);
            } else {
                // This function is the terminal stdio CLI path. Do not close
                // HID before MIDI; process teardown retains final ownership.
                eprintln!(
                    "Retaining stdio HID owner until process teardown: MIDI close unacknowledged"
                );
                std::mem::forget(controller);
            }
        }
    }
}

fn runtime_status_payload(status: Option<BrokerStatus>, broker_transport: bool) -> Value {
    let mut capabilities = vec!["nativeGateway"];
    capabilities.extend_from_slice(generated_gateway::CAPABILITIES);
    capabilities.extend([
        "hostMidiPerformance",
        "multiDeviceSelection",
        RESOURCE_READ_ORIGIN_CAPABILITY,
    ]);
    if broker_transport {
        capabilities.extend(["physicalTargetOrigin", "expectedBrokerTarget"]);
    }
    let mut result = json!({
        "platform": "Rust device gateway",
        "gatewayAvailable": true,
        "gatewayApiVersion": generated_gateway::API_VERSION,
        "capabilities": capabilities,
        "message": "Shared Rust QC engine active"
    });
    if let Some(status) = status {
        result["usbDiagnostics"] = json!(status);
    }
    result
}

fn handle(
    controller: &DeviceController,
    performance_midi: &PerformanceMidiWorkers,
    request: Request,
) -> Value {
    handle_with_disconnect_admission(controller, performance_midi, request, None, false)
}

fn rpc_reconnect(
    controller: &DeviceController,
    queue_after_idle_close: bool,
) -> Result<ReconnectOutcome, String> {
    #[cfg(test)]
    if queue_after_idle_close {
        if let Some(hook) = test_hook(
            &PENDING_CLOSE_RECONNECT_HOOKS,
            controller as *const DeviceController as usize,
        ) {
            hook()?;
            return Ok(ReconnectOutcome::Restarted);
        }
    }
    #[cfg(test)]
    // In unit tests this intentionally intercepts every reconnect for this
    // fixture controller, including explicit RPCs. Production builds contain
    // no hook; tests install it only around the race they exercise.
    if let Some(hook) = test_hook(
        &IDLE_RECONNECT_HOOKS,
        controller as *const DeviceController as usize,
    ) {
        hook()?;
        return Ok(ReconnectOutcome::Restarted);
    }
    if queue_after_idle_close {
        controller.reconnect_after_pending_close()
    } else {
        controller.reconnect()
    }
}

fn handle_with_disconnect_admission(
    controller: &DeviceController,
    performance_midi: &PerformanceMidiWorkers,
    request: Request,
    disconnect_admission: Option<Box<dyn Send>>,
    queue_reconnect_after_idle_close: bool,
) -> Value {
    let id = request.id.clone();
    if request.jsonrpc != "2.0" || id.as_u64().is_none_or(|id| id == 0) {
        return error(id, -32600, "Invalid JSON-RPC request");
    }
    if !request.params.is_object() {
        return error(id, -32602, "JSON-RPC params must be an object");
    }
    if generated_gateway::broker_dispatch(&request.method).is_some() {
        if let Err(message) = generated_gateway::validate_params(&request.method, &request.params) {
            return error(id, -32602, &message);
        }
    }
    let mutation = is_device_mutation(&request.method);
    // A suspended device is refused before anything is planned. Session control
    // stays open as the escape hatch. Reads that would reach the device are
    // refused by the session owner itself and reported the same way below.
    if mutation && !is_quarantine_recovery(&request.method) {
        if let Some(refusal) = controller.suspension_refusal() {
            controller.record_flight_event(format!(
                "rpc-suspended-{}-{}",
                id.as_u64().unwrap_or_default(),
                flight_method_name(&request.method)
            ));
            return suspension_refusal(id, controller, &refusal, Some(false));
        }
    }
    if mutation && !is_quarantine_recovery(&request.method) {
        if let Some(quarantine) = controller.write_quarantine() {
            if write_quarantine::is_stale(&quarantine, controller.session_epoch()) {
                // Set in an earlier session epoch: the new session already
                // cleared what it described. No read is taken to decide that.
                controller.clear_stale_write_quarantine(quarantine.since_unix_ms);
            } else {
                let active_setlist = controller
                    .gateway_snapshot()
                    .map(|snapshot| snapshot.setlist_key);
                if write_quarantine::gates(
                    &quarantine,
                    &request.method,
                    &request.params,
                    active_setlist.as_deref(),
                ) {
                    // Refused, with exactly why and how it clears. The broker
                    // sends nothing to find out: see `write_quarantine`.
                    return quarantine_refusal(id, &quarantine);
                }
            }
        }
    }
    let flight_method = flight_method_name(&request.method);
    controller.record_flight_event(format!(
        "rpc-start-{}-{}",
        id.as_u64().unwrap_or_default(),
        flight_method
    ));
    let result = match generated_gateway::broker_dispatch(&request.method) {
        Some(generated_gateway::BrokerDispatch::SystemStatus) => {
            Ok(runtime_status_payload(Some(controller.status()), false))
        }
        Some(generated_gateway::BrokerDispatch::Reconnect) => {
            rpc_reconnect(controller, queue_reconnect_after_idle_close).map(|outcome| {
                reconnect_state(controller, outcome, "Quad Cortex handshake complete")
            })
        }
        Some(generated_gateway::BrokerDispatch::ResetSession) => controller
            .reset_session()
            .map(|outcome| reconnect_state(controller, outcome, "Communication session reset")),
        Some(generated_gateway::BrokerDispatch::Disconnect) => performance_midi
            .retire_controller(controller)
            .and_then(|()| controller.disconnect_with_admission(disconnect_admission))
            .map(|_| connection_state(controller, "Quad Cortex session closed")),
        // The native broker implements the generated gateway contract without
        // exposing raw HID/protobuf details to either client.
        Some(generated_gateway::BrokerDispatch::StateEvents) => {
            gateway_state_events(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::Snapshot) => gateway_cached_snapshot(controller),
        Some(generated_gateway::BrokerDispatch::CurrentPreset) => {
            gateway_current_preset(controller)
        }
        Some(generated_gateway::BrokerDispatch::ListModels) => gateway_list_models(controller),
        Some(generated_gateway::BrokerDispatch::Identity) => gateway_identity(controller),
        Some(generated_gateway::BrokerDispatch::LibraryRead) => {
            gateway_library_read(controller, &request.method, &request.params)
        }
        // Like type 3, the QC publishes type 9 during boot; that it ignores an
        // on-demand read is BELIEVED (recorded without a date or artifact).
        // Device settings therefore come from the latest
        // synchronized session publication.
        Some(generated_gateway::BrokerDispatch::GeneralSettings) => {
            gateway_general_settings(controller)
        }
        // The QC publishes type 3 during boot but ignores an on-demand type-3
        // read. Serve the synchronized session publication instead of waiting
        // through two replies the hardware never sends.
        Some(generated_gateway::BrokerDispatch::IoSettings) => gateway_io_settings(controller),
        Some(generated_gateway::BrokerDispatch::GatewayRead) => {
            execute_gateway_read(controller, &request.method, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::DeleteSetlist) => {
            gateway_delete_setlist(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::GatewayOperation) => {
            gateway_operation(controller, &request.params, &request.method)
        }
        Some(generated_gateway::BrokerDispatch::SetDeviceName) => {
            gateway_set_device_name(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::Undo) => {
            gateway_history(controller, true, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::Redo) => {
            gateway_history(controller, false, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::InhibitedModules) => {
            gateway_inhibited_modules(controller)
        }
        Some(generated_gateway::BrokerDispatch::PresetScreenshot) => {
            gateway_preset_screenshot(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::CaptureScreen) => {
            gateway_capture_screen(controller)
        }
        Some(generated_gateway::BrokerDispatch::TapScreen) => {
            gateway_screen_gesture(controller, &request.method, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::TempoClock) => gateway_tempo_clock(controller),
        Some(generated_gateway::BrokerDispatch::SelectScene) => {
            gateway_select_scene(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::ToggleBypass) => {
            gateway_toggle_bypass(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::BlockDetails) => {
            gateway_block_details(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::LaneControlDetails) => {
            gateway_lane_control_details(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::PreviewParameter) => {
            gateway_parameter(controller, &request.params, true)
        }
        Some(generated_gateway::BrokerDispatch::SetParameter) => {
            gateway_parameter(controller, &request.params, false)
        }
        Some(generated_gateway::BrokerDispatch::PreviewLaneControlParameter) => {
            gateway_lane_control_parameter(controller, &request.params, true)
        }
        Some(generated_gateway::BrokerDispatch::SetLaneControlParameter) => {
            gateway_lane_control_parameter(controller, &request.params, false)
        }
        Some(generated_gateway::BrokerDispatch::SetLaneControlSceneMode) => {
            gateway_lane_control_scene_mode(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::SetLaneControlExpression) => {
            gateway_lane_control_expression(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::ParameterAssignment) => {
            gateway_parameter_assignment(controller, &request.params, &request.method)
        }
        Some(generated_gateway::BrokerDispatch::SetTempo) => {
            gateway_set_tempo(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::SetMasterVolume) => {
            gateway_set_master_volume(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::MasterVolume) => gateway_master_volume(controller),
        Some(generated_gateway::BrokerDispatch::PerformanceMidi) => gateway_performance_midi(
            controller,
            performance_midi,
            &request.method,
            &request.params,
        ),
        Some(generated_gateway::BrokerDispatch::NavigateBank) => {
            gateway_navigate_bank(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::RecallPreset) => {
            gateway_recall_preset(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::ReloadPreset) => {
            gateway_reload_preset(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::ListPresetFolders) => {
            gateway_list_preset_folders(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::ListPresets) => {
            gateway_list_presets(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::ListPresetSlots) => {
            gateway_list_preset_slots(controller)
        }
        Some(generated_gateway::BrokerDispatch::SavePresetAs) => {
            gateway_save_preset_as(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::RenameCurrentPreset) => {
            gateway_rename_current_preset(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::CreateBackup) => {
            gateway_create_backup(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::CreateSetlist) => {
            gateway_create_setlist(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::CopyPreset) => {
            gateway_copy_preset(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::DuplicateSetlist) => {
            gateway_duplicate_setlist(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::DeletePreset) => {
            gateway_delete_preset(controller, &request.params)
        }
        Some(generated_gateway::BrokerDispatch::MovePreset) => {
            gateway_move_preset(controller, &request.params)
        }
        None if request.method == "broker.backupTarget" => controller
            .backup_context()
            .and_then(|context| serde_json::to_value(context).map_err(|error| error.to_string())),
        None if request.method == "broker.createDeviceBackupBound" => {
            gateway_create_backup_with(&request.params, || {
                let context: DispatchContext = serde_json::from_value(
                    request
                        .params
                        .get("context")
                        .cloned()
                        .ok_or("Backup session context is required")?,
                )
                .map_err(|_| "Invalid backup session context".to_string())?;
                controller.create_backup_in_context(
                    Duration::from_millis(profile::BACKUP_TOTAL_TIMEOUT_MS),
                    Some(&context),
                )
            })
        }
        None if request.method == "device.status" => {
            serde_json::to_value(controller.status()).map_err(|error| error.to_string())
        }
        None if request.method == "device.state.events" => {
            state_events(controller, &request.params)
        }
        None if request.method == "device.state.blockDetails" => {
            state_block_details(controller, &request.params)
        }
        None => return error(id, -32601, &format!("Method not found: {}", request.method)),
    };
    match result {
        Ok(result) => {
            controller.record_flight_event(format!(
                "rpc-ok-{}-{}",
                id.as_u64().unwrap_or_default(),
                flight_method
            ));
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        Err(message) => {
            controller.record_flight_event(format!(
                "rpc-error-{}-{}",
                id.as_u64().unwrap_or_default(),
                flight_method
            ));
            failure_response(controller, id, &request.method, mutation, &message)
        }
    }
}

/// Turn a handler failure into its response, and quarantine only when the
/// failure leaves a transmitted write genuinely unresolved.
fn failure_response(
    controller: &DeviceController,
    id: Value,
    method: &str,
    mutation: bool,
    message: &str,
) -> Value {
    if let Some(detail) = message.strip_prefix(STATE_UNAVAILABLE_PREFIX) {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": HANDLER_ERROR_CODE, "message": detail,
                "data": { "code": "STATE_UNAVAILABLE", "retryable": false } }
        });
    }
    if let Some(detail) =
        message.strip_prefix(runtime_request::UNSAVED_STATE_NOT_YET_OBSERVED_PREFIX)
    {
        // The planner refused a preset change before sending anything, because the unit has not
        // yet reported its dirty flag. The flag arrives by itself, so a retry can succeed.
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": HANDLER_ERROR_CODE, "message": detail,
                "data": { "code": "UNSAVED_STATE_NOT_YET_OBSERVED", "retryable": true,
                    "userActionRequired": false } }
        });
    }
    if let Some((message, catalog)) = split_catalog_diagnostics(message) {
        // A retained-catalog read found nothing. The JSON-RPC code stays the
        // handler code callers already match; `data.code` says which case,
        // `data.setlistKey` echoes the key it looked up, and `data.catalog`
        // says why. Only counts are reported, never folder names.
        let (code, _) = classify_handler_error(message);
        let unknown = catalog["setlistKeyUnknown"].as_bool() == Some(true);
        return json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": code, "message": message,
                "data": {
                    "code": if unknown {
                        PRESET_SETLIST_UNKNOWN_CODE
                    } else {
                        PRESET_CATALOG_UNPUBLISHED_CODE
                    },
                    "retryable": false,
                    "setlistKey": catalog["setlistKey"].clone(),
                    "knownSetlists": catalog["knownSetlists"].clone(),
                    "catalog": catalog
                } }
        });
    }
    let flight_method = flight_method_name(method);
    let ambiguous = mutation && error_has_ambiguous_write_outcome(message);
    if ambiguous {
        controller.quarantine_writes_for(
            flight_method,
            format!("{method} failed after transmitting to the device"),
        );
    }
    if let Some(start) = message.find(DEVICE_SUSPENDED_MARKER).filter(|_| !ambiguous) {
        // The owner refused a request at its wire boundary. Only a refusal
        // that is the whole failure (at most behind the transport's admission
        // wrapper) proves nothing was sent. A refusal composed into a larger
        // failure -- a multi-step workflow whose earlier steps ran -- keeps
        // its full text and does not claim the request never left.
        let prefix = &message[..start];
        if prefix.is_empty() || prefix == ADMISSION_REFUSAL_PREFIX {
            return suspension_refusal(id, controller, &message[start..], Some(false));
        }
        return suspension_refusal(id, controller, message, None);
    }
    let (code, retryable) = classify_handler_error(message);
    if ambiguous {
        let quarantine = controller.write_quarantine();
        let clears = quarantine
            .as_ref()
            .map(|quarantine| quarantine.clears_by.clone())
            .unwrap_or_default();
        json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": code,
                "message": format!(
                    "{message}. Conflicting device writes are now quarantined; it clears {clears}."
                ),
                "data": {
                    "code": "WRITE_OUTCOME_AMBIGUOUS",
                    "retryable": false,
                    "writesQuarantined": true,
                    "quarantine": quarantine
                } }
        })
    } else if retryable {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message, "data": { "retryable": true } }
        })
    } else {
        error(id, code, message)
    }
}

/// Default handler failure. Scoped request deadlines use this ordinary error
/// path; exhausting a caller budget does not establish session loss.
const HANDLER_ERROR_CODE: i64 = -32010;
/// Retained compatibility mapping for the retired reservation retry path.
/// Current callers reserve once inside their original scope and do not emit
/// this legacy message. The desktop bridge still recognizes older brokers.
const RESERVE_REQUEST_ID_TIMEOUT_MESSAGE: &str = "Native QC request-id reservation timed out";
const RESERVE_REQUEST_ID_TIMEOUT_ERROR_CODE: i64 = -32011;
const WRITE_QUARANTINED_ERROR_CODE: i64 = -32012;
/// The device holds the session suspended (a backup or a reset is running on
/// the unit) and the request was not sent. `data.code` is `DEVICE_SUSPENDED`.
const DEVICE_SUSPENDED_ERROR_CODE: i64 = -32013;
const STATE_UNAVAILABLE_PREFIX: &str = "STATE_UNAVAILABLE: ";
/// What the USB writer puts in front of an owner refusal (`UsbError::Admission`).
const ADMISSION_REFUSAL_PREFIX: &str = "USB command was not admitted: ";
fn is_device_mutation(method: &str) -> bool {
    method == "broker.createDeviceBackupBound" || generated_gateway::is_mutation_method(method)
}

fn is_quarantine_recovery(method: &str) -> bool {
    matches!(
        generated_gateway::broker_dispatch(method),
        Some(
            generated_gateway::BrokerDispatch::Reconnect
                | generated_gateway::BrokerDispatch::ResetSession
                | generated_gateway::BrokerDispatch::Disconnect
        )
    )
}

fn flight_method_name(method: &str) -> &str {
    if method == "broker.createDeviceBackupBound" {
        return generated_gateway::rpc::CREATE_DEVICE_BACKUP;
    }
    if generated_gateway::METHODS.contains(&method)
        || matches!(
            method,
            "device.status" | "device.state.events" | "device.state.blockDetails"
        )
    {
        method
    } else {
        "unknown-method"
    }
}

/// Handler errors distinguish preflight refusal from a write whose outcome is
/// unknown in their stable diagnostics. Do not infer this from the global USB
/// send counter: keepalives and read preflights also increment it and would
/// quarantine an unrelated client even though no mutation was attempted.
fn error_has_ambiguous_write_outcome(message: &str) -> bool {
    message.contains(" was sent, but")
        || message.starts_with(
            "Persistent USB reports were delivered, but the device outcome was not verified.",
        )
        || message.starts_with(
            "A persistent USB write was attempted; its device outcome remains unconfirmed.",
        )
        || message.contains("mutation was sent exactly once")
        || message.contains("DELETE was sent exactly once")
        || message.contains("USB link lost")
        || message.contains("Device session restarted")
        // `DeviceController::wait_for_native_write`'s three timeout messages
        // all end this way. That path admits and dispatches the write before
        // it can time out waiting for delivery confirmation, so - unlike a
        // preflight refusal - the bytes may already be on the wire. Every raw
        // and legacy command handler (`command_scene`, `raw_send`, ...)
        // propagates this error unwrapped, so this is the only place that
        // catches it for them; do not narrow this match without giving those
        // handlers their own explicit wrapping first.
        || message.contains("device effect may be uncertain")
}

fn classify_handler_error(message: &str) -> (i64, bool) {
    if message == RESERVE_REQUEST_ID_TIMEOUT_MESSAGE {
        (RESERVE_REQUEST_ID_TIMEOUT_ERROR_CODE, true)
    } else {
        (HANDLER_ERROR_CODE, false)
    }
}

fn connection_state(controller: &DeviceController, detail: &str) -> Value {
    let status = controller.status();
    json!({
        "phase": status.phase,
        "detail": detail,
        "lastSync": status.connected_at_unix_ms,
        "demo": false
    })
}

/// The caller-facing refusal for a gated write. Keeps the dedicated -32012
/// code and the "writesQuarantined" flag the hosts already key on, and adds the
/// structured record so a caller can see why, since when, and how it clears.
fn quarantine_refusal(id: Value, quarantine: &WriteQuarantine) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": WRITE_QUARANTINED_ERROR_CODE,
            "message": write_quarantine::refusal_message(quarantine, unix_ms()),
            "data": {
                "retryable": false,
                "writesQuarantined": true,
                "quarantine": quarantine,
            }
        }
    })
}

/// The caller-facing refusal for a request the suspended device was not sent.
/// Not retryable by any automatic layer: the suspension lasts as long as the
/// operation on the unit, so a replay would only be refused again. The
/// structured record says why and since when.
/// `request_sent` is `Some(false)` when the refusal proves nothing left, and
/// `None` (serialized as null) when part of a larger operation may have.
fn suspension_refusal(
    id: Value,
    controller: &DeviceController,
    message: &str,
    request_sent: Option<bool>,
) -> Value {
    let suspension = controller.status().suspension;
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": DEVICE_SUSPENDED_ERROR_CODE,
            "message": message,
            "data": {
                "code": "DEVICE_SUSPENDED",
                "retryable": false,
                "deviceSuspended": true,
                "requestSent": request_sent,
                "suspension": suspension.active,
            }
        }
    })
}

fn requested_device_is_present(
    selected_device_id: Option<&str>,
    devices: &[quadcortex_hid::usb::QcUsbDevice],
) -> bool {
    devices
        .iter()
        .any(|device| selected_device_id.is_none_or(|selected| selected == device.id))
}

fn await_ready_status(controller: &DeviceController) -> BrokerStatus {
    // A reconnect request arms the worker's background search. When HID
    // enumeration already proves that the selected QC (or any QC, when no
    // selection exists) is absent, there is no handshake to wait for. Return
    // the searching state immediately; the worker remains armed and its phase
    // stream will announce a device plugged in later.
    let selected_device_id = controller.device_id();
    if controller.status().phase == "searching" {
        if let Ok(devices) = controller.connected_devices() {
            let selected_is_present =
                requested_device_is_present(selected_device_id.as_deref(), &devices);
            if !selected_is_present {
                return controller.status();
            }
        }
    }
    let status = controller.wait_for_ready(Duration::from_millis(profile::READY_WAIT_TIMEOUT_MS));
    if status.phase == "ready" {
        // USB synchronization and state decoding run on separate workers.
        // A successful reconnect must not return until the first decoded
        // preset is queryable by the very next gateway call.
        let _ = controller.wait_for_gateway_snapshot(
            Duration::from_millis(profile::COMMAND_CONFIRMATION_TIMEOUT_MS),
            |snapshot| !snapshot.setlist_key.is_empty() && !snapshot.preset_name.is_empty(),
        );
    }
    status
}

fn connection_state_json(status: &BrokerStatus, detail: &str) -> Value {
    json!({
        "phase": status.phase,
        "detail": if status.phase == "ready" { detail } else { &status.detail },
        "lastSync": status.connected_at_unix_ms,
        "demo": false
    })
}

/// The detail line and `restarted` flag a reconnect result must carry.
///
/// An unforced reconnect is short-circuited while the session is healthy, and
/// that is deliberate. What is not defensible is reporting it as a completed
/// handshake: `reconnect_device` is contracted `risky-write` and spends a
/// `confirm_risky_operation`, so the caller is entitled to know that the
/// answer was "nothing to do" and where the forcing path is.
fn reconnect_report(
    outcome: ReconnectOutcome,
    restarted_detail: &'static str,
) -> (&'static str, bool) {
    match outcome {
        ReconnectOutcome::Restarted => (restarted_detail, true),
        ReconnectOutcome::AlreadyConnected => (
            "Already connected: the transport was not restarted. Use reset_device_session to force \
             a teardown and a fresh handshake.",
            false,
        ),
    }
}

fn reconnect_state(
    controller: &DeviceController,
    outcome: ReconnectOutcome,
    restarted_detail: &'static str,
) -> Value {
    reconnect_state_json(&await_ready_status(controller), outcome, restarted_detail)
}

fn reconnect_state_json(
    status: &BrokerStatus,
    outcome: ReconnectOutcome,
    restarted_detail: &'static str,
) -> Value {
    let (detail, restarted) = reconnect_report(outcome, restarted_detail);
    let mut state = connection_state_json(status, detail);
    state["restarted"] = json!(restarted);
    state
}

fn gateway_state_events(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let after = optional_u64(params, "afterSequence")?.unwrap_or(0);
    let limit = optional_bounded_u64(params, "limit", 1, domain::STATE_EVENT_MAXIMUM_LIMIT as u64)?
        .unwrap_or(domain::STATE_EVENT_DEFAULT_LIMIT as u64) as usize;
    serde_json::to_value(controller.state_event_page(after, limit))
        .map_err(|error| error.to_string())
}

fn gateway_list_models(controller: &DeviceController) -> Result<Value, String> {
    serde_json::to_value(controller.list_models()?).map_err(|error| error.to_string())
}

fn execute_single_gateway_read(
    controller: &DeviceController,
    scope: &GatewayReadScope,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    controller.gateway_read_in_scope(scope, method, params)
}

/// One logical read: the first attempt and, on a timeout-class error (not for
/// `device.diagnostics` or `device.looperStatus`), ONE same-session re-send
/// of the read, then a refusal that does not reset the session
/// ([`unresponsive_read_session_refusal`]). It never rebuilds the session and
/// never replays through a new one.
///
/// Host policy (not a Cortex Control frame; Cortex Control re-sends nothing).
/// The re-send shares the first attempt's scope budget, so for a single-frame
/// read the budget is normally spent and the re-send is practically
/// unreachable: BELIEVED, not verified by any test.
fn execute_gateway_read(
    controller: &DeviceController,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let scope = controller.begin_gateway_read_scope(method, params)?;
    match execute_gateway_read_on_current_session(controller, &scope, method, params) {
        Ok(value) => Ok(value),
        Err(error) if gateway_read_timeout_permits_same_session_retry(method, &error) => {
            // A follow-up may exhaust its own shorter wait while the complete
            // logical read still has time left. Never create another scope.
            if controller.gateway_read_scope_remaining(&scope).is_err() {
                return Err(error);
            }
            execute_gateway_read_on_current_session(controller, &scope, method, params).map_err(
                |retry_error| {
                    if gateway_read_timeout_permits_same_session_retry(method, &retry_error) {
                        unresponsive_read_session_refusal(&retry_error)
                    } else {
                        retry_error
                    }
                },
            )
        }
        Err(error) => Err(error),
    }
}

/// What a read says when the session will not answer it twice.
///
/// It does *not* reset the session. `device.resetSession` is contracted
/// `risky-write` requiring `confirm_risky_operation` because a teardown fails
/// every in-flight request, voids every `expected_*` value the caller is
/// holding, and resets the unit's screen in front of the player. A read cannot
/// buy that on the caller's behalf, and disclosing it afterwards would not help:
/// by the time any field could report it, the in-flight requests have already
/// failed and the expected values are already void. The consent has to come
/// first, so the read names the operation that asks for it and stops.
fn unresponsive_read_session_refusal(error: &str) -> String {
    format!(
        "{error} The read was retried once on the same session and timed out again, so the USB \
         session looks unresponsive. It was left running: recovering it restarts the session, \
         which fails every in-flight request, voids every expected value you are holding, and \
         resets the unit's screen in front of whoever is playing. Call reset_device_session with \
         confirm_risky_operation when that is acceptable."
    )
}

fn execute_gateway_read_on_current_session(
    controller: &DeviceController,
    scope: &GatewayReadScope,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let primary = execute_single_gateway_read(controller, scope, method, params)?;
    let Some(followup_method) = runtime_request::gateway_read_followup_method(method) else {
        return Ok(primary);
    };
    let followup = execute_single_gateway_read(controller, scope, followup_method, &Value::Null)?;
    let value = runtime_request::compose_global_tempo_settings(&primary, &followup)?;
    controller.gateway_read_scope_remaining(scope)?;
    Ok(value)
}

fn gateway_read_timeout_permits_same_session_retry(method: &str, error: &str) -> bool {
    !matches!(method, "device.diagnostics" | "device.looperStatus")
        && ((error.starts_with("The Quad Cortex did not return a valid ")
            && error.contains(" reply within "))
            || (error.starts_with("No correlated QC message type ")
                && error.contains(" response for request ")
                && error.ends_with(" before timeout")))
}

fn gateway_identity(controller: &DeviceController) -> Result<Value, String> {
    execute_gateway_read(controller, "device.identity", &Value::Null)
}

/// Send the single Version UPDATE frame that carries the custom name, then
/// stop. It is a device extension (Cortex Control has no sender for this
/// field) and no host reads the name back: a type-10 READ ladder used to follow
/// it and, when the unit answered late or not at all, reported a plain failure
/// for a write that may have applied. The caller gets `accepted_unverified`
/// once the frame is delivered; `device.identity` stays available as an
/// ordinary read. Hardware verification of the name change is pending.
fn gateway_set_device_name(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let (plan, plan_fence) = plan_gateway_write(controller, "device.setDeviceName", params)?;
    execute_gateway_write(controller, &plan, &plan_fence).map_err(|error| {
        // Only an error that already says the delivery outcome is unknown gets
        // the extra sentence; a refusal before anything was written stays plain.
        if error_has_ambiguous_write_outcome(&error) {
            format!("The device-name frame may have been sent: {error}")
        } else {
            error
        }
    })?;
    Ok(accepted_unverified(plan.detail))
}

/// Send the one UndoRedo UPDATE frame (`08 01 28 01` or `08 01 30 01`) that
/// Cortex Control sends, then stop. No read follows it: Cortex Control issues
/// none, and whether the QC pushes state after an undo is UNKNOWN, so the host
/// does not refresh the cached preset on its own (hardware verification
/// pending). The expected-preset-identity guard is host policy, checked while
/// planning. History is non-idempotent and is never replayed.
fn gateway_history(
    controller: &DeviceController,
    undo: bool,
    params: &Value,
) -> Result<Value, String> {
    let method = if undo { "device.undo" } else { "device.redo" };
    let (plan, plan_fence) = plan_gateway_write(controller, method, params)?;
    execute_gateway_write(controller, &plan, &plan_fence)?;
    Ok(accepted_unverified(plan.detail))
}

fn gateway_inhibited_modules(controller: &DeviceController) -> Result<Value, String> {
    execute_gateway_read(controller, "device.inhibitedModules", &Value::Null)
}

fn gateway_preset_screenshot(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    execute_gateway_read(controller, "device.presetScreenshot", params)
}

fn gateway_capture_screen(controller: &DeviceController) -> Result<Value, String> {
    execute_gateway_read(controller, "device.captureScreen", &Value::Null)
}

fn gateway_screen_gesture(
    controller: &DeviceController,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    gateway_operation(controller, params, method)
}

fn gateway_tempo_clock(controller: &DeviceController) -> Result<Value, String> {
    let Some(frame) = controller.latest_tempo_clock() else {
        return Ok(json!({"available": false}));
    };
    let Some(status) = frame.tempo_clock else {
        return Ok(json!({"available": false}));
    };
    Ok(json!({
        "available": true,
        "sequence": frame.sequence,
        "receivedAtUnixMs": frame.observed_at,
        "enabled": status.enabled,
        "prerollEnabled": status.preroll_enabled,
        "currentBeat": status.current_beat,
        "currentBar": status.current_bar,
        "currentTick": status.current_tick,
        "countInBeatsRemaining": status.count_in_beats_remaining,
        "countInBarsRemaining": status.count_in_bars_remaining
    }))
}

fn assert_expected_preset(controller: &DeviceController, params: &Value) -> Result<(), String> {
    let snapshot = controller.gateway_snapshot();
    runtime_request::assert_expected_preset(snapshot.as_ref(), params)
}

fn plan_gateway_write(
    controller: &DeviceController,
    method: &str,
    params: &Value,
) -> Result<(GatewayWritePlan, DispatchContext), String> {
    controller.plan_gateway_write(method, params)
}

fn execute_gateway_write(
    controller: &DeviceController,
    plan: &GatewayWritePlan,
    fence: &DispatchContext,
) -> Result<(), String> {
    execute_planned_write(controller, &plan.write, fence)
}

fn execute_planned_write(
    controller: &DeviceController,
    write: &PlannedWrite,
    fence: &DispatchContext,
) -> Result<(), String> {
    match write {
        PlannedWrite::HidCommand(command) => {
            let message = command
                .clone()
                .try_encode()
                .map_err(|error| error.to_string())?;
            controller.send_command_for_plan(message, fence)
        }
        PlannedWrite::HidOperation(operation) => {
            controller.send_operation_for_plan(operation.clone(), fence)
        }
        PlannedWrite::MidiControlChange { .. } => {
            Err("Host MIDI must be executed by the native application transport".into())
        }
    }
}

fn execute_realtime_planned_write(
    controller: &DeviceController,
    write: &PlannedWrite,
    fence: &DispatchContext,
) -> Result<(), String> {
    match write {
        PlannedWrite::HidCommand(command) => controller.send_realtime_command_for_plan(
            command
                .clone()
                .try_encode()
                .map_err(|error| error.to_string())?,
            fence,
        ),
        PlannedWrite::HidOperation(operation) => {
            controller.queue_operation_for_plan(operation.clone(), fence)
        }
        PlannedWrite::MidiControlChange { .. } => {
            Err("Host MIDI must be executed by the native application transport".into())
        }
    }
}

/// Whether a write result is `verified: true`. Post-write block projections and
/// comparisons are attached only then: an unverified result carries no readback, and a
/// stale mirror compared against the request would manufacture a failure.
fn result_is_verified(result: &Value) -> bool {
    result.get("verified").and_then(Value::as_bool) == Some(true)
}

fn accepted_unverified(detail: impl Into<String>) -> Value {
    json!({
        "accepted": true,
        "verified": false,
        "verification": "accepted_unverified",
        "detail": detail.into()
    })
}

fn matching_settings_observation_result(detail: impl Into<String>) -> Value {
    let mut result = accepted_unverified(format!(
        "{}; a matching settings publication was observed after native admission, but it has no request correlation and does not prove this write caused the value",
        detail.into()
    ));
    result["observation"] = json!({
        "kind": "matching_publication_after_native_admission",
        "causallyCorrelated": false
    });
    result
}

/// One verified write result, with the authoritative observation it was decided
/// against attached under `body_key` (`"snapshot"` or `"readback"`).
///
/// `changed` answers the question `verified: true` alone cannot: whether this
/// call actually moved device state. The verification predicate is absolute -
/// state == desired - so a write of a value the device already holds satisfies
/// it without proving the device applied anything. `Some(false)` says exactly
/// that; `Some(true)` says an authoritative pre-write observation did not
/// satisfy the predicate and the post-write one does. `None` is for the case
/// where no prior observation existed, which the contract's
/// `x-verification-outcomes` marks optional rather than forcing a guess.
fn verified_readback_result(
    detail: impl Into<String>,
    changed: Option<bool>,
    body_key: &str,
    body: Value,
) -> Value {
    let mut result = json!({
        "accepted": true,
        "verified": true,
        "verification": "authoritative_readback",
        "detail": detail.into(),
    });
    if let Some(object) = result.as_object_mut() {
        if let Some(changed) = changed {
            object.insert("changed".into(), json!(changed));
        }
        object.insert(body_key.into(), body);
    }
    result
}

/// An authoritative read taken *before* the write showed the requested setting
/// already matched, so no mutation was sent. The contract pins `changed` to
/// false here, because nothing was sent that could have changed anything.
fn authoritative_preflight_result(readback: Value) -> Value {
    json!({
        "accepted": true,
        "verified": true,
        "verification": "authoritative_preflight",
        "changed": false,
        "requestSent": false,
        "detail": "The requested setting already matched authoritative device state; no mutation was sent.",
        "readback": readback
    })
}

/// Bound the time a request holds `request_lock` while awaiting its worker.
/// Once native work starts, expiration is an unknown outcome and quarantines
/// writes; it is never reported as a safe retry.
const PERFORMANCE_MIDI_LOCK_BUDGET: Duration = Duration::from_secs(3);
const PERFORMANCE_MIDI_QUEUE_CAPACITY: usize = 1;
const PERFORMANCE_MIDI_MAX_DEVICE_WORKERS: usize = 16;
#[cfg(target_os = "linux")]
const PERFORMANCE_MIDI_RETIRE_BUDGET: Duration = Duration::from_secs(3);
// Software headroom across successive instances, not a firmware device count.
// Native sends and endpoint close cannot be forcibly cancelled.
static PERFORMANCE_MIDI_ORIGINALS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(PERFORMANCE_MIDI_MAX_DEVICE_WORKERS);

trait PerformanceMidiEndpoint: Send + 'static {
    fn send(
        &mut self,
        device_id: &str,
        controller: u8,
        value: u8,
    ) -> Result<PerformanceMidiReceipt, String>;

    #[cfg(target_os = "linux")]
    fn retire(&mut self) {}
}

impl PerformanceMidiEndpoint for PerformanceMidi {
    fn send(
        &mut self,
        device_id: &str,
        controller: u8,
        value: u8,
    ) -> Result<PerformanceMidiReceipt, String> {
        self.send_for_device(device_id, controller, value)
    }

    #[cfg(target_os = "linux")]
    fn retire(&mut self) {
        self.retire_endpoint();
    }
}

const MIDI_QUEUED: u8 = 0;
const MIDI_VALIDATING: u8 = 1;
const MIDI_RUNNING: u8 = 2;
const MIDI_CANCELLED: u8 = 3;

struct MidiSendCommand {
    device_id: String,
    controller: u8,
    value: u8,
    validate: Box<dyn FnOnce() -> Result<(), String> + Send>,
    state: Arc<std::sync::atomic::AtomicU8>,
    reply: mpsc::Sender<Result<PerformanceMidiReceipt, String>>,
    acknowledged: mpsc::Receiver<()>,
}

enum MidiCommand {
    Send(MidiSendCommand),
    #[cfg(target_os = "linux")]
    Retire(mpsc::Sender<bool>),
}

struct MidiWorker {
    // Fields drop in declaration order: disconnect queue before retirement.
    sender: mpsc::SyncSender<MidiCommand>,
    poisoned: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    retirement_pending: Arc<AtomicBool>,
    _original: qc_native_platform::thread_owner::Owned,
}

/// Per-device MIDI owners. Worker records are retained for the broker lifetime,
/// including poisoned workers, so a timed-out WinMM call is never replaced.
/// The map is capped to prevent arbitrary device identities growing threads.
/// Dropping the registry disconnects queues and transfers unfinished originals
/// to the shared bounded owner. Global permits survive send, endpoint close and
/// native TLS teardown until the exact original is signalled and joined.
struct PerformanceMidiWorkers {
    workers: Mutex<HashMap<String, MidiWorker>>,
    #[cfg(target_os = "linux")]
    admission: Mutex<()>,
    #[cfg(target_os = "linux")]
    closing: AtomicBool,
    factory: Arc<dyn Fn() -> Box<dyn PerformanceMidiEndpoint> + Send + Sync>,
    worker_budget: &'static qc_native_platform::thread_owner::Budget,
    #[cfg(test)]
    budget: Duration,
}

impl Default for PerformanceMidiWorkers {
    fn default() -> Self {
        Self::with_factory(|| Box::new(PerformanceMidi::default()))
    }
}

impl PerformanceMidiWorkers {
    fn with_factory(
        factory: impl Fn() -> Box<dyn PerformanceMidiEndpoint> + Send + Sync + 'static,
    ) -> Self {
        Self {
            workers: Mutex::new(HashMap::new()),
            #[cfg(target_os = "linux")]
            admission: Mutex::new(()),
            #[cfg(target_os = "linux")]
            closing: AtomicBool::new(false),
            factory: Arc::new(factory),
            worker_budget: &PERFORMANCE_MIDI_ORIGINALS,
            #[cfg(test)]
            budget: PERFORMANCE_MIDI_LOCK_BUDGET,
        }
    }
    fn get_or_start(
        &self,
        device_id: &str,
    ) -> Result<(mpsc::SyncSender<MidiCommand>, Arc<AtomicBool>), String> {
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| "Performance MIDI worker registry was poisoned".to_string())?;
        if let Some(worker) = workers.get(device_id) {
            return Ok((worker.sender.clone(), Arc::clone(&worker.poisoned)));
        }
        if workers.len() >= PERFORMANCE_MIDI_MAX_DEVICE_WORKERS {
            return Err("Performance MIDI worker limit reached; no worker was started".into());
        }
        let (sender, receiver) = mpsc::sync_channel::<MidiCommand>(PERFORMANCE_MIDI_QUEUE_CAPACITY);
        let poisoned = Arc::new(AtomicBool::new(false));
        let worker_poisoned = Arc::clone(&poisoned);
        #[cfg(target_os = "linux")]
        let retirement_pending = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        let worker_retirement_pending = Arc::clone(&retirement_pending);
        let factory = Arc::clone(&self.factory);
        let original = qc_native_platform::thread_owner::Owned::spawn(
            "qc-performance-midi",
            self.worker_budget,
            (),
            move || {
                let mut endpoint = factory();
                while let Ok(command) = receiver.recv() {
                    #[cfg(target_os = "linux")]
                    let command = match command {
                        MidiCommand::Send(command) => command,
                        MidiCommand::Retire(reply) => {
                            endpoint.retire();
                            worker_retirement_pending.store(false, Ordering::Release);
                            let _ = reply.send(true);
                            continue;
                        }
                    };
                    #[cfg(not(target_os = "linux"))]
                    let MidiCommand::Send(command) = command;
                    #[cfg(target_os = "linux")]
                    if worker_retirement_pending.load(Ordering::Acquire) {
                        // A full queue may have prevented the Retire marker
                        // from entering. The same owner must still close after
                        // its prior native call, even for a cancelled send.
                        endpoint.retire();
                        worker_retirement_pending.store(false, Ordering::Release);
                        let _ = command.reply.send(Err(
                            "Performance MIDI endpoint retired before this queued send".into(),
                        ));
                        continue;
                    }
                    if command
                        .state
                        .compare_exchange(
                            MIDI_QUEUED,
                            MIDI_VALIDATING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        continue;
                    }
                    if worker_poisoned.load(Ordering::Acquire) {
                        let _ = command.reply.send(Err(
                            "Performance MIDI worker is poisoned after an ambiguous timeout".into(),
                        ));
                        continue;
                    }
                    if let Err(error) = (command.validate)() {
                        let _ = command.reply.send(Err(error));
                        continue;
                    }
                    if command
                        .state
                        .compare_exchange(
                            MIDI_VALIDATING,
                            MIDI_RUNNING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        // The caller cancelled while the fence was being
                        // checked. Native work has not started and is known
                        // not to have been sent.
                        continue;
                    }
                    let result =
                        endpoint.send(&command.device_id, command.controller, command.value);
                    if command.reply.send(result).is_err() || command.acknowledged.recv().is_err() {
                        // A caller that timed out after dispatch dropped its
                        // acknowledgement. Retire this owner before accepting
                        // later work, even if native work returned late.
                        worker_poisoned.store(true, Ordering::Release);
                    }
                }
                // Endpoint drop stays here; the original permit is retained
                // past this body and native TLS teardown.
            },
        )
        .map_err(|error| format!("Could not start Performance MIDI worker: {error}"))?;
        workers.insert(
            device_id.to_owned(),
            MidiWorker {
                sender: sender.clone(),
                poisoned: Arc::clone(&poisoned),
                #[cfg(target_os = "linux")]
                retirement_pending,
                _original: original,
            },
        );
        Ok((sender, poisoned))
    }

    fn retire_controller(&self, controller: &DeviceController) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if let Some(device_id) = controller.device_id() {
            if !self.retire_device(&device_id) {
                return Err(format!("Performance MIDI endpoint for {device_id} is still draining; its QC session remains open"));
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = controller;
        Ok(())
    }

    /// Ordered endpoint close without replacing or detaching the native owner.
    /// Request callers hold request_lock across this barrier and HID release;
    /// terminal callers close admission first. The worker clears pending only
    /// after close, so that caller fence must prevent reopening before release.
    #[cfg(target_os = "linux")]
    fn retire_device(&self, device_id: &str) -> bool {
        let (sender, pending) = {
            let workers = self.workers.lock_recover();
            let Some(worker) = workers.get(device_id) else {
                return true;
            };
            (
                worker.sender.clone(),
                Arc::clone(&worker.retirement_pending),
            )
        };
        let deadline = Instant::now() + PERFORMANCE_MIDI_RETIRE_BUDGET;
        pending.store(true, Ordering::Release);
        let (reply, retired) = mpsc::channel();
        let mut command = MidiCommand::Retire(reply);
        loop {
            match sender.try_send(command) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
                Err(mpsc::TrySendError::Full(queued)) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    command = queued;
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
        retired
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    fn retire_all_devices(&self) -> bool {
        let admission = self.admission.lock_recover();
        self.closing.store(true, Ordering::Release);
        let devices: Vec<_> = self.workers.lock_recover().keys().cloned().collect();
        drop(admission);
        let mut retired = true;
        for device in devices {
            retired &= self.retire_device(&device);
        }
        retired
    }

    #[cfg(test)]
    fn send(
        &self,
        device_id: &str,
        controller: u8,
        value: u8,
        on_ambiguous_timeout: impl FnOnce(),
    ) -> Result<PerformanceMidiReceipt, String> {
        self.send_with_budget_and_validator(
            device_id,
            controller,
            value,
            self.budget,
            || Ok(()),
            on_ambiguous_timeout,
        )
    }

    #[cfg(test)]
    fn send_with_budget(
        &self,
        device_id: &str,
        controller: u8,
        value: u8,
        budget: Duration,
        on_ambiguous_timeout: impl FnOnce(),
    ) -> Result<PerformanceMidiReceipt, String> {
        self.send_with_budget_and_validator(
            device_id,
            controller,
            value,
            budget,
            || Ok(()),
            on_ambiguous_timeout,
        )
    }

    fn send_with_budget_and_validator(
        &self,
        device_id: &str,
        controller: u8,
        value: u8,
        budget: Duration,
        validate: impl FnOnce() -> Result<(), String> + Send + 'static,
        on_ambiguous_timeout: impl FnOnce(),
    ) -> Result<PerformanceMidiReceipt, String> {
        self.send_with_budget_and_validator_notified(
            device_id,
            controller,
            value,
            budget,
            validate,
            || {},
            on_ambiguous_timeout,
        )
    }

    // `on_enqueued` makes the queue/session race deterministic in the fake
    // endpoint test; production passes a no-op.
    #[allow(clippy::too_many_arguments)]
    fn send_with_budget_and_validator_notified(
        &self,
        device_id: &str,
        controller: u8,
        value: u8,
        budget: Duration,
        validate: impl FnOnce() -> Result<(), String> + Send + 'static,
        on_enqueued: impl FnOnce(),
        on_ambiguous_timeout: impl FnOnce(),
    ) -> Result<PerformanceMidiReceipt, String> {
        #[cfg(target_os = "linux")]
        let admission = self.admission.lock_recover();
        #[cfg(target_os = "linux")]
        if self.closing.load(Ordering::Acquire) {
            return Err("Performance MIDI workers are shutting down; request was not sent".into());
        }
        let (sender, poisoned) = self.get_or_start(device_id)?;
        #[cfg(target_os = "linux")]
        if self.workers.lock_recover()[device_id]
            .retirement_pending
            .load(Ordering::Acquire)
        {
            return Err(
                "Performance MIDI endpoint is still draining after a device release".into(),
            );
        }
        if poisoned.load(Ordering::Acquire) {
            return Err("Performance MIDI worker is poisoned after an ambiguous timeout".into());
        }
        let (reply, response) = mpsc::channel();
        let (acknowledge, acknowledged) = mpsc::channel();
        let state = Arc::new(std::sync::atomic::AtomicU8::new(MIDI_QUEUED));
        let command = MidiCommand::Send(MidiSendCommand {
            device_id: device_id.to_owned(),
            controller,
            value,
            validate: Box::new(validate),
            state: Arc::clone(&state),
            reply,
            acknowledged,
        });
        match sender.try_send(command) {
            Ok(()) => on_enqueued(),
            Err(mpsc::TrySendError::Full(_)) => {
                return Err(
                    "Performance MIDI queue is full; request was not sent. Retry the request."
                        .into(),
                )
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("Performance MIDI worker has stopped; request was not sent.".into())
            }
        }
        #[cfg(target_os = "linux")]
        drop(admission);
        match response.recv_timeout(budget) {
            Ok(result) => {
                let _ = acknowledge.send(());
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel_before_native(&state) {
                    return Err("Performance MIDI request timed out before dispatch; request was not sent. Retry the request.".into());
                }
                poisoned.store(true, Ordering::Release);
                on_ambiguous_timeout();
                Err("Performance MIDI dispatch timed out after native work began; outcome is unknown and writes are quarantined.".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if cancel_before_native(&state) {
                    return Err("Performance MIDI worker stopped before native dispatch; request was not sent. Retry the request.".into());
                }
                poisoned.store(true, Ordering::Release);
                on_ambiguous_timeout();
                Err("Performance MIDI worker stopped after dispatch began; outcome is unknown and writes are quarantined.".into())
            }
        }
    }
}

fn cancel_before_native(state: &std::sync::atomic::AtomicU8) -> bool {
    for prior in [MIDI_QUEUED, MIDI_VALIDATING] {
        if state
            .compare_exchange(prior, MIDI_CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
struct GatedMidiEndpoint {
    gate: Arc<(Mutex<bool>, Condvar)>,
    started: Arc<AtomicBool>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl PerformanceMidiEndpoint for GatedMidiEndpoint {
    fn send(
        &mut self,
        _device_id: &str,
        _controller: u8,
        _value: u8,
    ) -> Result<PerformanceMidiReceipt, String> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.started.store(true, Ordering::Release);
        let (released, changed) = &*self.gate;
        let mut released = released.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
        Ok(PerformanceMidiReceipt {
            endpoint: "offline fake".into(),
            dispatch_latency_ms: 0.0,
            throttle_delay_ms: 0.0,
        })
    }
}

fn gateway_performance_midi(
    controller: &DeviceController,
    performance_midi: &PerformanceMidiWorkers,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    if !runtime_request::gateway_write_is_realtime(method) {
        return Err(format!(
            "{method} is not classified as a realtime gateway write"
        ));
    }
    let device_id = controller
        .device_id()
        .ok_or("No Quad Cortex device is selected")?;
    let (plan, plan_fence) = plan_gateway_write(controller, method, params)?;
    let PlannedWrite::MidiControlChange {
        controller: midi_controller,
        value,
    } = plan.write
    else {
        return Err(format!("{method} did not produce a host MIDI write"));
    };
    controller.validate_gateway_write_fence(&plan_fence)?;
    let host_started_at_unix_ms = unix_ms();
    let validate_fence = controller.gateway_write_fence_validator(plan_fence);
    let receipt = performance_midi.send_with_budget_and_validator(
        &device_id,
        midi_controller,
        value,
        PERFORMANCE_MIDI_LOCK_BUDGET,
        validate_fence,
        || {
            controller.quarantine_writes_for(
                method,
                "Performance MIDI dispatch timed out after native work began; outcome is unknown",
            );
        },
    )?;
    let mut result = accepted_unverified(format!(
        "{} immediately through {}; live USB state will reconcile the result.",
        plan.detail, receipt.endpoint
    ));
    if let Some(object) = result.as_object_mut() {
        object.insert("immediate".into(), json!(true));
        object.insert("transport".into(), json!(receipt.endpoint));
        object.insert(
            "dispatchLatencyMs".into(),
            json!(receipt.dispatch_latency_ms),
        );
        object.insert("hostStartedAtUnixMs".into(), json!(host_started_at_unix_ms));
        object.insert("throttleDelayMs".into(), json!(receipt.throttle_delay_ms));
    }
    Ok(result)
}

fn unix_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

/// Host policy (not a Cortex Control frame): watches for pushed evidence until
/// the policy's deadline and sends NOTHING while it waits (the type-15
/// verification READs this used to schedule are removed; Cortex Control reads
/// nothing after a write). `Ok(None)` means the window expired without evidence:
/// the caller reports `accepted_unverified`, which is not an error.
fn verify_gateway_write_on_schedule(
    controller: &DeviceController,
    events: &crate::worker::StateSubscription,
    verification: GatewayVerification,
    context: &DispatchContext,
    after_sequence: u64,
    policy: &runtime_request::GatewayWriteVerificationPolicy,
) -> Result<Option<quadcortex::device::GatewaySnapshot>, String> {
    // Which decoded state kinds can actually carry evidence for this predicate.
    // Captured before the verification moves into the runtime.
    let observed_kinds = verification.observed_state_kinds();
    let wait =
        controller.begin_gateway_verification(context, verification, after_sequence, policy)?;
    let mut observation_sequence = after_sequence;
    loop {
        let step = wait.advance(observation_sequence)?;
        match step.action {
            GatewayVerificationAction::Verified => return Ok(step.snapshot),
            GatewayVerificationAction::TimedOut => return Ok(None),
            GatewayVerificationAction::Wait { delay_ms } => {
                match events.recv_timeout(Duration::from_millis(delay_ms)) {
                    Ok(frame) => {
                        // A newer frame sequence is not freshness. `worker.rs`
                        // drops a decoded frame only when its states *and* its
                        // tempo clock are both absent, so a tempo-clock-only
                        // frame - `states: []`, four of them inside one
                        // millisecond on an idle device - reaches this same
                        // subscriber list. Advancing the observation token on
                        // one made an unrelated tempo tick the proof that a
                        // write landed, because the predicate is absolute
                        // against the cached snapshot. Only a frame carrying a
                        // state kind this predicate reads is an observation.
                        if frame
                            .states
                            .iter()
                            .any(|state| observed_kinds.contains(&state.kind.as_str()))
                        {
                            observation_sequence = frame.sequence;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(
                            "QC state event stream disconnected during write verification"
                                .to_string(),
                        );
                    }
                }
            }
        }
    }
}

fn observed_gateway_parameter(
    controller: &DeviceController,
    (row, column, parameter_index): (u32, u32, u32),
) -> Option<quadcortex::state::BlockParameter> {
    let details = match column {
        10 => controller.lane_control_details(row, "inputGate"),
        11 => controller.lane_control_details(row, "laneOutput"),
        _ => controller.block_details(row, column),
    }
    .ok()
    .flatten()?;
    details
        .parameters
        .into_iter()
        .find(|parameter| parameter.index == parameter_index)
}

fn gateway_select_scene(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    gateway_operation(controller, params, "device.selectScene")
}

fn gateway_toggle_bypass(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    gateway_operation(controller, params, "device.toggleBypass")
}

/// Served from the pushed state mirror, exactly like `get_current_preset`: no frame is sent.
///
/// Host policy (not a Cortex Control frame). Cortex Control sends a type-15 READ only at boot
/// and renders everything else from pushed state, so this no longer forces a type-15 READ
/// with a host request id (which the QC may ignore for up to 15 s while an editor is open:
/// BELIEVED, code comment). The caller's preset identity is checked against the mirror in
/// the same lock that reads the details. How quickly the mirror reflects an edit made on
/// the unit is UNKNOWN (hardware verification pending, experiment 11); stale-state guards
/// that matter (`expected_*` arguments) are still compared with this same mirror.
fn gateway_block_details(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let details = state_block_details(controller, params)?;
    if details.is_null() {
        Err("No block exists at that grid position".into())
    } else {
        Ok(details)
    }
}

fn lane_control_target(params: &Value) -> Result<(u32, &str), String> {
    let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
    let control = params
        .get("control")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "inputGate" | "laneOutput"))
        .ok_or_else(|| "control must be inputGate or laneOutput".to_string())?;
    Ok((row, control))
}

fn gateway_lane_control_details(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    // Served from the pushed state mirror; see `gateway_block_details`. The identity
    // guard and the read happen under one lock.
    let (row, control) = lane_control_target(params)?;
    controller
        .guarded_lane_control_details(row, control, params)?
        .map(|details| serde_json::to_value(details).map_err(|error| error.to_string()))
        .unwrap_or_else(|| {
            Err(format!(
                "No {control} control is synchronized for row {}",
                row + 1
            ))
        })
}

/// The decoded state a preflight decision inspects. Host policy (not a Cortex Control frame):
/// no preflight sends a READ. Cortex Control reads nothing before a write, so every guard
/// compares the caller's expected values with state the session already folded from what the
/// QC pushed, and fails closed - sending nothing - when that state has not been published.
///
/// Each guard reads its own projection: the scene guard the observed preset, the tempo guards
/// the folded type-33 preconditions, the favorite and pinned guards the retained
/// type-20 / type-54 lists. They are not interchangeable - GatewaySnapshot.mode is the
/// footswitch mode, whose domain also contains "PRESET".
fn preflight_observation(controller: &DeviceController, method: &str) -> Result<Value, String> {
    controller
        .gateway_preflight_observation(method)
        .ok_or_else(|| runtime_request::gateway_preflight_unavailable_message(method))
}

fn gateway_cached_snapshot(controller: &DeviceController) -> Result<Value, String> {
    let snapshot = controller
        .gateway_snapshot()
        .filter(|snapshot| !snapshot.setlist_key.is_empty() && !snapshot.preset_name.is_empty())
        .ok_or_else(|| "No Quad Cortex preset has been synchronized yet".to_owned())?;
    serialize_gateway_snapshot(snapshot)
}

fn serialize_gateway_snapshot(
    snapshot: quadcortex::device::GatewaySnapshot,
) -> Result<Value, String> {
    serde_json::to_value(snapshot).map_err(|error| error.to_string())
}

fn gateway_current_preset(controller: &DeviceController) -> Result<Value, String> {
    let snapshot = controller
        .gateway_snapshot()
        .filter(|snapshot| !snapshot.setlist_key.is_empty() && !snapshot.preset_name.is_empty())
        .ok_or_else(|| "No Quad Cortex preset has been synchronized yet".to_owned())?;
    if snapshot.grid_layout_complete {
        return serde_json::to_value(snapshot).map_err(|error| error.to_string());
    }

    // A sparse structural Grid echo can invalidate the block list without
    // carrying enough model identity to merge the new block safely. For an
    // explicit current-preset request only, issue one bare type-15 READ and
    // require a newer full publication from this same state session. The
    // complete fast path above remains no-frame; post-write verification never
    // comes through this endpoint. Do not use execute_gateway_read here: its
    // timeout policy may resend the read.
    let prior_revision = snapshot.full_preset_revision;
    let prior_session = snapshot
        .state_version
        .as_ref()
        .map(|version| version.session.clone())
        .ok_or_else(|| format!("{STATE_UNAVAILABLE_PREFIX}The current preset has no session version for a safe refresh"))?;
    let unavailable = current_preset_refresh_unavailable;
    let scope = controller
        .begin_gateway_read_scope("device.currentPreset", &Value::Null)
        .map_err(unavailable)?;
    controller
        .gateway_read_in_scope(&scope, "device.currentPreset", &Value::Null)
        .map_err(unavailable)?;
    let remaining = controller
        .gateway_read_scope_remaining(&scope)
        .map_err(unavailable)?;
    let refreshed = controller
        .wait_for_gateway_snapshot(Duration::from_millis(remaining), |candidate| {
            current_preset_refresh_matches(candidate, prior_revision, &prior_session)
        })
        .ok_or_else(|| {
            format!("{STATE_UNAVAILABLE_PREFIX}The Quad Cortex did not publish a complete current preset after the refresh; the Grid layout remains incomplete")
        })?;
    controller
        .gateway_read_scope_remaining(&scope)
        .map_err(unavailable)?;
    serde_json::to_value(refreshed).map_err(|error| error.to_string())
}

/// How a failed current-preset refresh read is reported. A suspension refusal
/// stays unwrapped so `failure_response` types it as DEVICE_SUSPENDED, as it
/// does for every other read the session owner refuses.
fn current_preset_refresh_unavailable(error: String) -> String {
    if error.contains(DEVICE_SUSPENDED_MARKER) {
        return error;
    }
    format!("{STATE_UNAVAILABLE_PREFIX}{error}")
}

fn current_preset_refresh_matches(
    candidate: &quadcortex::device::GatewaySnapshot,
    prior_full_preset_revision: u64,
    prior_session: &str,
) -> bool {
    candidate.grid_layout_complete
        && candidate.full_preset_revision > prior_full_preset_revision
        && candidate
            .state_version
            .as_ref()
            .is_some_and(|version| version.session == prior_session)
}

fn gateway_lane_control_parameter(
    controller: &DeviceController,
    params: &Value,
    preview: bool,
) -> Result<Value, String> {
    assert_expected_preset(controller, params)?;
    let (row, control) = lane_control_target(params)?;
    let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
    let before = controller
        .lane_control_details(row, control)?
        .ok_or_else(|| format!("No {control} control is synchronized for row {}", row + 1))?;
    let observed = before
        .parameters
        .iter()
        .find(|parameter| parameter.index == parameter_index);
    let actual = observed.and_then(|parameter| parameter.normalized_value);
    // An option list is stored as index / (count - 1), so the guard has to
    // compare the selected option rather than the raw float. The caller supplies
    // no earlier count, so the current one stands for both halves of the pair;
    // that alone absorbs the float32 storage error a flat 1e-5 rejects, and the
    // rescaling is ready for a host that records the count it read.
    let option_counts = observed
        .map(|parameter| parameter.options.len())
        .filter(|count| *count >= 2)
        .map(|count| (count, count));
    runtime_request::assert_expected_parameter_with_options(actual, params, option_counts)?;
    let method = if preview {
        "device.previewLaneControlParameter"
    } else {
        "device.setLaneControlParameter"
    };
    let (plan, plan_fence) = plan_gateway_write(controller, method, params)?;
    if preview {
        execute_gateway_write(controller, &plan, &plan_fence)?;
        let mut result = accepted_unverified(plan.detail);
        if let Some(object) = result.as_object_mut() {
            object.insert(
                "acceptedValue".into(),
                params.get("value").cloned().unwrap_or_else(|| json!(0.0)),
            );
        }
        return Ok(result);
    }
    let mut result = gateway_operation(controller, params, method)?;
    if !result_is_verified(&result) {
        return Ok(result);
    }
    let Some(details) = controller.verified_write_details(&result, row, None, Some(control))?
    else {
        return Ok(result);
    };
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "block".into(),
            serde_json::to_value(details).map_err(|error| error.to_string())?,
        );
    }
    Ok(result)
}

fn gateway_lane_control_scene_mode(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    assert_expected_preset(controller, params)?;
    let (row, control) = lane_control_target(params)?;
    let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
    let mut result = gateway_operation(controller, params, "device.setLaneControlSceneMode")?;
    if !result_is_verified(&result) {
        return Ok(result);
    }
    let Some(details) = controller.verified_write_details(&result, row, None, Some(control))?
    else {
        return Ok(result);
    };
    let parameter = details
        .parameters
        .iter()
        .find(|item| item.index == parameter_index)
        .ok_or_else(|| {
            "The write completed, but its lane parameter is absent from readback".to_string()
        })?;
    let expected = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| "enabled must be true or false".to_string())?;
    if parameter.scene_mode != expected {
        return Err(format!(
            "Lane parameter scene behavior readback was {}, expected {expected}",
            parameter.scene_mode
        ));
    }
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "block".into(),
            serde_json::to_value(details).map_err(|error| error.to_string())?,
        );
    }
    Ok(result)
}

fn gateway_lane_control_expression(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    assert_expected_preset(controller, params)?;
    let (row, control) = lane_control_target(params)?;
    let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
    let before = controller
        .lane_control_details(row, control)?
        .ok_or_else(|| "The selected lane control is absent from readback".to_string())?;
    let parameter = before
        .parameters
        .iter()
        .find(|item| item.index == parameter_index)
        .ok_or_else(|| "The selected lane parameter is absent from readback".to_string())?;
    if !parameter.writable || !parameter.expression_assignable {
        return Err("The selected lane parameter cannot be assigned to an expression pedal".into());
    }
    let mut result = gateway_operation(controller, params, "device.setLaneControlExpression")?;
    if !result_is_verified(&result) {
        return Ok(result);
    }
    let Some(details) = controller.verified_write_details(&result, row, None, Some(control))?
    else {
        return Ok(result);
    };
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "block".into(),
            serde_json::to_value(details).map_err(|error| error.to_string())?,
        );
    }
    Ok(result)
}

fn gateway_parameter(
    controller: &DeviceController,
    params: &Value,
    preview: bool,
) -> Result<Value, String> {
    let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
    let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
    let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
    let before = controller
        .block_details(row, column)?
        .ok_or_else(|| "No block exists at that grid position".to_string())?;
    let observed = before
        .parameters
        .iter()
        .find(|parameter| parameter.index == parameter_index);
    let actual = observed.and_then(|parameter| parameter.normalized_value);
    // An option list is stored as index / (count - 1), so the guard has to
    // compare the selected option rather than the raw float. The caller supplies
    // no earlier count, so the current one stands for both halves of the pair;
    // that alone absorbs the float32 storage error a flat 1e-5 rejects, and the
    // rescaling is ready for a host that records the count it read.
    let option_counts = observed
        .map(|parameter| parameter.options.len())
        .filter(|count| *count >= 2)
        .map(|count| (count, count));
    runtime_request::assert_expected_parameter_with_options(actual, params, option_counts)?;
    let method = if preview {
        "device.previewParameter"
    } else {
        "device.setParameter"
    };
    let (plan, plan_fence) = plan_gateway_write(controller, method, params)?;
    let value = params
        .get("value")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    if preview {
        execute_gateway_write(controller, &plan, &plan_fence)?;
        let mut result = accepted_unverified(plan.detail);
        if let Some(object) = result.as_object_mut() {
            object.insert("acceptedValue".into(), json!(value));
        }
        Ok(result)
    } else {
        // device.setParameter is not a realtime write, so gateway_operation has
        // already blocked on an authoritative readback by the time it returns.
        // Reporting the pre-write block here only patched normalizedValue and
        // left displayValue showing the previous value, so a readback described
        // two different states at once and looked like a failed write. Read the
        // block back after the verified write, exactly as the sibling
        // parameter-assignment path does. Low-latency editing is served by
        // device.previewParameter above, which returns accepted-unverified
        // without fabricating an authoritative parameter snapshot.
        let mut result = gateway_operation(controller, params, method)?;
        if !result_is_verified(&result) {
            return Ok(result);
        }
        let Some(after) = controller.verified_write_details(&result, row, Some(column), None)?
        else {
            return Ok(result);
        };
        attach_parameter_block(&mut result, after)?;
        Ok(result)
    }
}

/// Attach the post-write block to a parameter result.
///
/// The readback is reported verbatim. Patching the accepted value back over a
/// single field is exactly what left `normalizedValue` updated while
/// `displayValue` still showed the previous value, so one response described
/// two different device states and a landed write looked like it had failed.
fn attach_parameter_block(result: &mut Value, after: BlockDetails) -> Result<(), String> {
    let block = serde_json::to_value(after).map_err(|error| error.to_string())?;
    result
        .as_object_mut()
        .ok_or_else(|| "a parameter write produced a malformed result".to_string())?
        .insert("block".into(), block);
    Ok(())
}

fn gateway_parameter_assignment(
    controller: &DeviceController,
    params: &Value,
    method: &str,
) -> Result<Value, String> {
    let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
    let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
    let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
    let mut result = gateway_operation(controller, params, method)?;
    if !result_is_verified(&result) {
        return Ok(result);
    }
    let Some(block) = controller.verified_write_details(&result, row, Some(column), None)? else {
        return Ok(result);
    };
    let parameter = block
        .parameters
        .iter()
        .find(|parameter| parameter.index == parameter_index)
        .ok_or_else(|| {
            "The write completed, but its parameter is absent from readback".to_string()
        })?;

    match method {
        "device.setParameterSceneMode" => {
            let expected = params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| "enabled must be true or false".to_string())?;
            if parameter.scene_mode != expected {
                return Err(format!(
                    "Parameter scene behavior readback was {}, expected {expected}",
                    parameter.scene_mode
                ));
            }
        }
        "device.setParameterExpression" => {
            let expected_pedal = bounded_u32(params, "pedal", 2)? as i32;
            let actual_pedal = parameter.expression.unwrap_or(0);
            if actual_pedal != expected_pedal {
                return Err(format!(
                    "Parameter expression readback was pedal {actual_pedal}, expected {expected_pedal}"
                ));
            }
            if expected_pedal != 0 {
                let expected_minimum = normalized_f64(params, "minimum")?;
                let expected_maximum = normalized_f64(params, "maximum")?;
                let actual_minimum = parameter.expression_minimum.map(f64::from);
                let actual_maximum = parameter.expression_maximum.map(f64::from);
                let close = |actual: Option<f64>, expected: f64| {
                    actual.is_some_and(|value| (value - expected).abs() <= 0.001)
                };
                if !close(actual_minimum, expected_minimum)
                    || !close(actual_maximum, expected_maximum)
                {
                    return Err(format!(
                        "Parameter expression range readback was {:?}-{:?}, expected {expected_minimum}-{expected_maximum}",
                        actual_minimum, actual_maximum
                    ));
                }
            }
        }
        _ => return Err(format!("Unsupported parameter assignment method: {method}")),
    }

    if let Some(object) = result.as_object_mut() {
        object.insert(
            "block".into(),
            serde_json::to_value(block).map_err(|error| error.to_string())?,
        );
    }
    Ok(result)
}

fn gateway_set_tempo(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    gateway_operation(controller, params, "device.setTempo")
}

fn gateway_master_volume(controller: &DeviceController) -> Result<Value, String> {
    let status = controller.status();
    if !status.connected {
        return Err("Quad Cortex USB is not connected".to_string());
    }
    if !status.synchronized {
        return Err("Quad Cortex USB is still synchronizing its authoritative state".to_string());
    }
    let frame = controller
        .latest_master_volume()
        .ok_or_else(|| "The Quad Cortex has not reported Master Volume yet".to_string())?;
    let value = frame
        .states
        .iter()
        .rev()
        .find_map(|state| state.master_volume)
        .ok_or_else(|| "The Quad Cortex Master Volume cache is invalid".to_string())?;
    // `value` is the percentage this API has always reported. `normalized` is
    // the wire float (0.0 to 1.0) the unit sent, to six decimals, so a caller
    // can see the number the unit reported rather than only our rescaling of it.
    // What the wire float corresponds to acoustically is not established.
    Ok(json!({
        "value": (value * 100.0).round() as u32,
        "normalized": (f64::from(value) * 1_000_000.0).round() / 1_000_000.0,
        "observedAt": frame.observed_at
    }))
}

fn gateway_set_master_volume(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    gateway_operation(controller, params, "device.setMasterVolume")
}

/// Reconcile a published settings write from the frame the QC pushes on its
/// own. CorOS answers a type-3 / type-9 write with a sparse settings echo; that
/// it never answers an on-demand READ of either type is BELIEVED (recorded
/// without a date or artifact). Host policy (not a desktop-application frame):
/// the publication cache
/// cannot move until that echo lands. Polling it at 0 ms was a guaranteed
/// miss for any write that changes
/// a value, and a *hit* at 0 ms was a report about the pre-write publication,
/// not about this write - there is no freshness check and no preflight here.
///
/// `ingest_incoming` merges the echo into the publication cache before it fans
/// the frame out to raw subscribers, so a frame observed here is already
/// readable. No echo inside the budget is not proof the write failed - Cortex
/// Control waits for nothing at all - so the caller gets accepted-unverified.
fn await_published_readback(
    controller: &DeviceController,
    context: &DispatchContext,
    method: &str,
    params: &Value,
    read_method: &str,
    echoes: &crate::worker::RawSubscription,
    operation: &OperationTicket,
) -> Result<Option<Value>, String> {
    let (message_type, projection) = published_settings(read_method)
        .ok_or_else(|| format!("{read_method} is not a published settings readback"))?;
    let scope = controller.begin_gateway_read_scope_for(
        context,
        runtime_request::gateway_write_verification_policy(method).timeout_ms,
    )?;
    loop {
        let remaining_ms = controller.gateway_read_scope_remaining(&scope)?;
        match echoes.recv_timeout(Duration::from_millis(remaining_ms)) {
            // Compare original native ingress and observation identity with the
            // actual begin-report boundary. RPC subscription/queue time is too
            // early: a matching pre-write publication can still be buffered.
            Ok(frame)
                if frame.message_type == message_type
                    && controller
                        .operation_observation_is_fresh(operation, frame.observation.as_ref()) =>
            {
                if !runtime_request::gateway_write_echo_matches(
                    method,
                    params,
                    frame.message_type,
                    &frame.payload,
                ) {
                    continue;
                }
                let response = controller.published_gateway_read_in_scope(
                    &scope,
                    message_type,
                    projection.clone(),
                )?;
                if runtime_request::gateway_write_readback_matches(method, params, &response) {
                    controller.gateway_read_scope_remaining(&scope)?;
                    return Ok(Some(response));
                }
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("QC device event stream disconnected during write readback".into())
            }
        }
    }
}

fn published_settings(read_method: &str) -> Option<(u16, GatewayResponseProjection)> {
    match read_method {
        "device.generalSettings" => Some((
            profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            GatewayResponseProjection::GeneralSettings,
        )),
        "device.ioSettings" => Some((
            profile::MESSAGE_TYPE_IO_SETTINGS,
            GatewayResponseProjection::IoSettings,
        )),
        _ => None,
    }
}

fn gateway_operation(
    controller: &DeviceController,
    params: &Value,
    method: &str,
) -> Result<Value, String> {
    let verification_policy = runtime_request::gateway_write_verification_policy(method);
    let (plan, mut plan_fence) = plan_gateway_write(controller, method, params)?;
    if plan.is_noop() {
        let snapshot = controller.gateway_snapshot_for_plan(&plan_fence)?;
        let result = authoritative_preflight_result(
            serde_json::to_value(snapshot).map_err(|error| error.to_string())?,
        );
        return Ok(result);
    }
    let mut preflight_proved_a_difference = false;
    if verification_policy.preflight_method.is_some() {
        let response = preflight_observation(controller, method)?;
        let (bound, decision) =
            controller.bind_gateway_preflight(&plan_fence, method, params, &response)?;
        plan_fence = bound;
        match decision {
            // The decoded state said the requested state was *not* already
            // present, so a write that then verifies did move device state.
            runtime_request::GatewayWritePreflightDecision::Proceed => {
                preflight_proved_a_difference = true;
            }
            // Cortex Control skips these frames too (scene selection and tempo
            // mode only); every other write is always sent.
            runtime_request::GatewayWritePreflightDecision::AlreadyMatched => {
                return Ok(authoritative_preflight_result(response));
            }
        }
    }
    if runtime_request::gateway_write_is_realtime(method) {
        execute_realtime_planned_write(controller, &plan.write, &plan_fence)?;
        return Ok(accepted_unverified(plan.detail));
    }
    // Did the requested state already satisfy this write's own predicate before
    // anything was sent? Answered with the same predicate against the same kind
    // of decoded observation the verification below will certify with, so
    // `changed` is held to exactly the standard `verified` is - no weaker, no
    // stronger. `None` where there is no synchronized snapshot to compare.
    let unmoved_before_write = plan
        .verification
        .requires_authoritative_readback()
        .then(|| {
            controller.gateway_snapshot().map(|snapshot| {
                let parameter = plan
                    .verification
                    .parameter_target()
                    .and_then(|target| observed_gateway_parameter(controller, target));
                plan.verification.matches(&snapshot, parameter.as_ref())
            })
        })
        .flatten();
    let events = controller.subscribe_state_events();
    let after_sequence = controller.latest_state_sequence();
    // Opened before the write: a subscription created afterwards can miss the
    // settings echo entirely, which is the only evidence these writes get.
    let published_echoes = verification_policy
        .readback_method
        .filter(|read_method| runtime_request::gateway_readback_is_published(read_method))
        .and_then(|read_method| {
            let (message_type, _) = published_settings(read_method)?;
            Some((controller.subscribe_raw_events(), message_type))
        });
    record_move_block_frame(controller, method, &plan);
    let settings_operation = if published_echoes.is_some() {
        let (messages, interval) = match &plan.write {
            PlannedWrite::HidCommand(command) => (
                vec![command
                    .clone()
                    .try_encode()
                    .map_err(|error| error.to_string())?],
                Duration::ZERO,
            ),
            PlannedWrite::HidOperation(operation) => (
                operation
                    .clone()
                    .try_encode()
                    .map_err(|error| error.to_string())?,
                Duration::from_millis(runtime_request::operation_inter_message_interval_ms(
                    operation,
                )),
            ),
            PlannedWrite::MidiControlChange { .. } => {
                return Err("Host MIDI cannot use settings readback".into())
            }
        };
        Some(controller.send_settings_for_plan(messages, interval, &plan_fence)?)
    } else {
        execute_gateway_write(controller, &plan, &plan_fence)?;
        None
    };
    let changed = if preflight_proved_a_difference {
        Some(true)
    } else {
        unmoved_before_write.map(|unmoved| !unmoved)
    };
    if !plan.verification.requires_authoritative_readback() {
        let (Some(read_method), Some((echoes, _)), Some(operation)) = (
            verification_policy.readback_method,
            published_echoes,
            settings_operation,
        ) else {
            // The frames were delivered and nothing can confirm them: report
            // exactly that. No read follows (Cortex Control reads nothing back).
            return Ok(accepted_unverified(plan.detail));
        };
        return Ok(
            match await_published_readback(
                controller,
                &plan_fence,
                method,
                params,
                read_method,
                &echoes,
                &operation,
            )
            .map_err(|error| format!("{} was sent, but {error}", plan.detail))?
            {
                Some(_) => matching_settings_observation_result(plan.detail),
                None => accepted_unverified(plan.detail),
            },
        );
    }
    let snapshot = verify_gateway_write_on_schedule(
        controller,
        &events,
        plan.verification.clone(),
        &plan_fence,
        after_sequence,
        &verification_policy,
    )?;
    let mut result = match snapshot {
        // The owner checked the predicate and copied this snapshot under one lock.
        // Resampling a parameter here could mix it with a later session's state.
        Some(snapshot) => verified_readback_result(
            plan.detail.clone(),
            changed,
            "snapshot",
            serde_json::to_value(snapshot).map_err(|error| error.to_string())?,
        ),
        // The window expired without pushed evidence. The frames were delivered,
        // so this is neither a failure nor an ambiguity: nothing is read, nothing
        // is quarantined, and the result says exactly what is known.
        None => accepted_unverified(format!(
            "{}; the QC pushed no confirming state within {} ms, so the effect is not verified",
            plan.detail, verification_policy.timeout_ms
        )),
    };
    // A plugin block the device placed is reported as read back; ownership is
    // the device's to enforce, so nothing is refused or removed here.
    runtime_request::annotate_plugin_block_readback(&plan.write, &mut result);
    Ok(result)
}

/// Record exactly what a move frame carried, so a device that ignores a move
/// leaves the evidence behind. Content-free: coordinates, model ids and the
/// retained chain layout (see `StateDecoder::move_grid_layout_diagnostic`).
fn record_move_block_frame(controller: &DeviceController, method: &str, plan: &GatewayWritePlan) {
    if method != "device.moveBlock" {
        return;
    }
    let PlannedWrite::HidOperation(operation) = &plan.write else {
        return;
    };
    let frames = match operation.clone().try_encode() {
        Ok(messages) => messages
            .iter()
            .map(|message| {
                let bytes = message
                    .payload
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                format!(
                    "type={} len={} hex={bytes}",
                    message.message_type,
                    message.payload.len()
                )
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Err(error) => format!("unencodable: {error}"),
    };
    controller.record_flight_event(format!(
        "move-block-frame {frames} grid[{}]",
        controller.move_grid_layout_diagnostic()
    ));
}

/// The one type-2 SetlistPosition UPDATE a recall, bank navigation or reload
/// sends: the desktop application's `updateSetlist` frame, with no request id.
/// Completion is observed as a newer position revision, never from an echo, and
/// that observation names the address only, not the preset body.
fn preset_recall_message(
    plan: &runtime_request::PresetRecallPlan,
) -> Result<quadcortex::commands::OutboundMessage, String> {
    plan.command
        .clone()
        .try_encode()
        .map_err(|error| error.to_string())
}

fn execute_preset_recall(
    controller: &DeviceController,
    (plan, fence): (runtime_request::PresetRecallPlan, DispatchContext),
) -> Result<Value, String> {
    let recall_message = preset_recall_message(&plan)?;
    let device_events = controller.subscribe_state_events();
    let after_sequence = controller.latest_state_sequence();
    let verification = plan.verification();
    let verification_policy =
        runtime_request::gateway_write_verification_policy("device.recallPreset");
    controller.send_command_for_plan(recall_message, &fence)?;
    let Some(after) = verify_gateway_write_on_schedule(
        controller,
        &device_events,
        verification,
        &fence,
        after_sequence,
        &verification_policy,
    )?
    else {
        // The SetlistPosition frame was delivered and the QC pushed no
        // confirming position within the window. Not a failure: nothing is
        // read and nothing is quarantined.
        // The mirror's snapshot rides along (labelled by the verification kind) so a
        // caller that reads `snapshot` keeps getting what the host currently holds.
        let mut result = preset_recall_timeout_result(&plan, verification_policy.timeout_ms);
        if let (Some(object), Some(snapshot)) =
            (result.as_object_mut(), controller.gateway_snapshot())
        {
            object.insert(
                "snapshot".into(),
                serde_json::to_value(snapshot).map_err(|error| error.to_string())?,
            );
        }
        return Ok(result);
    };
    if !plan.matches_recovered(&after) {
        return Err(format!(
            "Preset recall targeted slot {}, but live device readback remained on slot {}.",
            plan.position, after.preset_position
        ));
    }
    preset_recall_address_result(&plan, &after)
}

/// The result of a recall, bank navigation or reload whose predicate matched `after`.
///
/// recallPreset, navigateBank and reloadPreset are contracted as
/// DeviceActionResult, so they must carry verification semantics like every
/// other device action; omitting them made the host reject a correct result and
/// tear down the session on every preset change. The checks above observe a
/// position update naming the requested ADDRESS; it carries no request id, so
/// it is an observation, not causal proof. The call still returns Err when the
/// observed position is another slot. They cannot bind the preset body: a type-15
/// body carries no address and a bare read no request id, so a late body of the
/// previous preset passes them. The result is therefore accepted_unverified with
/// an address observation (runtime_request::preset_address_observed_result),
/// never authoritative_readback. `snapshot` stays attached as the mirror the
/// caller navigates by; it is not a readback of the loaded preset.
fn preset_recall_address_result(
    plan: &runtime_request::PresetRecallPlan,
    after: &quadcortex::device::GatewaySnapshot,
) -> Result<Value, String> {
    let mut result = plan
        .address_observed_result(after)
        .ok_or_else(|| "A preset recall plan must carry a preset address predicate".to_string())?;
    result["snapshot"] = serde_json::to_value(after).map_err(|error| error.to_string())?;
    Ok(result)
}

/// The result of a recall, bank navigation or reload whose predicate did not match within
/// `timeout_ms`: the frame was sent and nothing confirming was observed, so the detail says
/// exactly that and claims no verification (runtime_request::PresetRecallPlan::timeout_detail).
fn preset_recall_timeout_result(
    plan: &runtime_request::PresetRecallPlan,
    timeout_ms: u64,
) -> Value {
    accepted_unverified(plan.timeout_detail(timeout_ms))
}

fn gateway_recall_preset(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let plan = controller.plan_preset_recall("device.recallPreset", params)?;
    execute_preset_recall(controller, plan)
}

fn gateway_navigate_bank(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let plan = controller.plan_preset_recall("device.navigateBank", params)?;
    execute_preset_recall(controller, plan)
}

fn gateway_reload_preset(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let plan = controller.plan_preset_recall("device.reloadPreset", params)?;
    execute_preset_recall(controller, plan)
}

/// The QC publishes its complete directory at the start of each USB session,
/// inside the Booting burst's `read_startup_files(false)` (586 type-4 frames
/// over ~12.6 s), and pushes File updates afterwards. The unit DOES answer a
/// later bare `readFolders(false)` (ESTABLISHED, CorOS 4.1.0, direct USB,
/// 2026-10-07); two earlier recorded sessions that saw no File traffic after
/// one are superseded. An explicit refresh is simply not implemented, so the
/// refusal below is host policy (not a desktop-application frame) and a
/// missing feature, not a device limit.
///
/// A new USB session is the only thing that produces a new directory, which is
/// exactly what Android's `relayCatalogRead` does for `refresh` before it
/// reads. A read must not take that side effect on itself, so it says so
/// immediately instead of spending 15 or 45 seconds rediscovering it.
use runtime_request::PRESET_CATALOG_REFRESH_UNAVAILABLE;

const PRESET_CATALOG_UNPUBLISHED: &str = "This Quad Cortex USB session has not published a preset-folder catalog. Reconnect the device session to pull the directory again.";

/// Separates a catalog refusal's public message from its diagnostics while
/// both travel through the handler's `Result<_, String>`. Only the three
/// catalog routes append it, and `failure_response` strips it before anything
/// reaches a client, so the message a caller sees is unchanged.
const CATALOG_DIAGNOSTICS_MARKER: &str = "\u{1f}qc-catalog-diagnostics:";

/// `data.code` of a catalog read that waited out its budget for a directory
/// this session has not published (or not finished publishing).
const PRESET_CATALOG_UNPUBLISHED_CODE: &str = "PRESET_CATALOG_UNPUBLISHED";

/// `data.code` of a catalog read whose setlist key names no folder of the
/// session's complete directory. It fails at once: the directory is published
/// once per USB session, so waiting cannot make the key appear, and the
/// unpublished message's advice to reconnect would be wrong.
const PRESET_SETLIST_UNKNOWN_CODE: &str = "PRESET_SETLIST_UNKNOWN";

/// The public message for an unknown setlist key. The same wording as the
/// virtual gateway's `listPresets` refusal, so both hosts read alike.
fn preset_setlist_unknown_message(key: &str) -> String {
    format!(
        "The setlist {} does not exist on this device.",
        serde_json::to_string(key).unwrap_or_else(|_| format!("\"{key}\""))
    )
}

/// `message`, carrying why the catalog read found nothing. A read that
/// settled on an unknown setlist key says that instead of `message`.
fn catalog_unpublished_error(message: &str, diagnostics: &PresetCatalogUnpublished) -> String {
    let message = match diagnostics.setlist_key.as_deref() {
        Some(key) if diagnostics.setlist_key_unknown => preset_setlist_unknown_message(key),
        _ => message.to_owned(),
    };
    match serde_json::to_string(diagnostics) {
        Ok(json) => format!("{message}{CATALOG_DIAGNOSTICS_MARKER}{json}"),
        Err(_) => message,
    }
}

/// The public message and the diagnostics of a [`catalog_unpublished_error`],
/// or `None` for any other handler failure.
fn split_catalog_diagnostics(message: &str) -> Option<(&str, Value)> {
    let (public, json) = message.split_once(CATALOG_DIAGNOSTICS_MARKER)?;
    Some((public, serde_json::from_str(json).unwrap_or(Value::Null)))
}

/// `device.captures` planned a `ReadLibraryFiles` with no file type and no
/// request ID, which `read_library_files_query` degrades to the same
/// uncorrelated `readFolders(false)` the preset catalog reads were waiting on,
/// and then waited 30 s for a type-4 reply.
///
/// Such a read can only be "answered" by the device's own spontaneous
/// in-session directory republication (`syncing` -> `ready` state rebuilds),
/// never by a reply correlated to the request itself. On a quiet session with
/// no such rebuild in progress, the read has nothing to catch and can only
/// reach its timeout.
///
/// The capture listing is already part of the boot publication, so it is served
/// from the session's own directory cache instead.
const CAPTURE_CATALOG_UNPUBLISHED: &str = "This Quad Cortex USB session has not published a capture catalog. Reconnect the device session to pull the directory again.";

/// Type 9 is treated as session state, not a request/reply endpoint (BELIEVED:
/// an on-demand read is recorded as unanswered, without a date or artifact), so
/// `device.generalSettings` uses the single boot publication instead of sending
/// a new read. Host policy (not a desktop-application frame).
const GENERAL_SETTINGS_UNPUBLISHED: &str = "This Quad Cortex USB session has not published General Settings. Reconnect the device session to pull them again.";

fn gateway_general_settings(controller: &DeviceController) -> Result<Value, String> {
    gateway_general_settings_with(controller, |payload| {
        GatewayResponseProjection::GeneralSettings.decode(payload)
    })
}

pub(crate) fn gateway_general_settings_with(
    controller: &DeviceController,
    decode: impl FnOnce(&[u8]) -> Result<Value, String>,
) -> Result<Value, String> {
    let scope = controller.begin_gateway_read_scope("device.generalSettings", &Value::Null)?;
    if controller
        .latest_message(profile::MESSAGE_TYPE_GENERAL_SETTINGS)
        .is_none()
    {
        return Err(GENERAL_SETTINGS_UNPUBLISHED.to_string());
    }
    controller.published_gateway_read_in_scope_with(
        &scope,
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        decode,
    )
}

/// Type 3 is treated as session state, not a request/reply endpoint (BELIEVED:
/// an on-demand read is recorded as unanswered, without a date or artifact), so
/// `device.ioSettings` uses the single boot publication instead of sending a
/// new read. Host policy (not a desktop-application frame).
const IO_SETTINGS_UNPUBLISHED: &str = "This Quad Cortex USB session has not published I/O settings. Reconnect the device session to pull them again.";

fn gateway_io_settings(controller: &DeviceController) -> Result<Value, String> {
    gateway_io_settings_with(controller, |payload| {
        GatewayResponseProjection::IoSettings.decode(payload)
    })
}

pub(crate) fn gateway_io_settings_with(
    controller: &DeviceController,
    decode: impl FnOnce(&[u8]) -> Result<Value, String>,
) -> Result<Value, String> {
    let scope = controller.begin_gateway_read_scope("device.ioSettings", &Value::Null)?;
    let payload = controller
        .retained_io_settings_in_scope(&scope)?
        .ok_or_else(|| IO_SETTINGS_UNPUBLISHED.to_string())?;
    let value = decode(&payload)?;
    // A fresh owner's healthy state must never certify this old read.
    controller.validate_io_settings_read(&scope)?;
    Ok(value)
}

fn gateway_library_read(
    controller: &DeviceController,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let target = runtime_request::plan_retained_library_read(method, params)?;
    let entries = controller
        .wait_for_library_folder(
            target.kind,
            &target.key,
            Duration::from_millis(profile::PRESET_SYNC_TIMEOUT_MS),
        )?
        .ok_or_else(|| {
            if method == "device.captures" {
                CAPTURE_CATALOG_UNPUBLISHED.to_string()
            } else {
                "This Quad Cortex USB session has not published the requested IR catalog."
                    .to_string()
            }
        })?;
    serde_json::to_value(entries).map_err(|error| error.to_string())
}

fn preset_catalog_refresh_requested(params: &Value) -> bool {
    params
        .get("refresh")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn gateway_list_preset_folders(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    if preset_catalog_refresh_requested(params) {
        return Err(PRESET_CATALOG_REFRESH_UNAVAILABLE.into());
    }
    let mut value = controller
        .wait_for_preset_catalog(
            "device.listPresetFolders",
            None,
            Duration::from_millis(profile::PRESET_SYNC_TIMEOUT_MS),
        )?
        .map_err(|why| catalog_unpublished_error(PRESET_CATALOG_UNPUBLISHED, &why))?;
    value["loading"] = Value::Bool(false);
    Ok(value)
}

fn gateway_list_presets(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    if preset_catalog_refresh_requested(params) {
        return Err(PRESET_CATALOG_REFRESH_UNAVAILABLE.into());
    }
    let mut value = controller
        .wait_for_preset_catalog(
            "device.listPresets",
            params.get("setlistKey").and_then(Value::as_str),
            Duration::from_millis(profile::PRESET_SYNC_TIMEOUT_MS),
        )?
        .map_err(|why| catalog_unpublished_error(PRESET_CATALOG_UNPUBLISHED, &why))?;
    value["loading"] = Value::Bool(false);
    Ok(value)
}

fn gateway_list_preset_slots(controller: &DeviceController) -> Result<Value, String> {
    controller
        .wait_for_preset_catalog(
            "device.listPresetSlots",
            None,
            Duration::from_millis(profile::PRESET_SYNC_TIMEOUT_MS),
        )?
        .map_err(|why| catalog_unpublished_error(PRESET_CATALOG_UNPUBLISHED, &why))
}

fn gateway_save_preset_as(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    controller.execute_preset_workflow("device.savePresetAs", params)
}

fn gateway_rename_current_preset(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    controller.execute_preset_workflow("device.renameCurrentPreset", params)
}

fn gateway_create_backup(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    gateway_create_backup_with(params, || {
        controller.create_backup(Duration::from_millis(profile::BACKUP_TOTAL_TIMEOUT_MS))
    })
}

fn gateway_create_backup_with(
    params: &Value,
    create_backup: impl FnOnce() -> Result<String, String>,
) -> Result<Value, String> {
    let name = required_text(params, "name")?;
    let name = runtime_request::validate_backup_name(&name)?;
    let raw = create_backup()?;
    finalize_device_backup(&raw, &name)
}

fn gateway_copy_preset(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    controller.execute_preset_workflow("device.copyPreset", params)
}

fn gateway_duplicate_setlist(
    controller: &DeviceController,
    params: &Value,
) -> Result<Value, String> {
    controller.execute_preset_workflow("device.duplicateSetlist", params)
}

fn gateway_create_setlist(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    controller.execute_preset_workflow("device.createSetlist", params)
}

fn gateway_delete_setlist(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    controller.execute_preset_workflow("device.deleteSetlist", params)
}

fn gateway_delete_preset(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    controller.execute_preset_workflow("device.deletePreset", params)
}

fn gateway_move_preset(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    controller.execute_preset_workflow("device.movePreset", params)
}

fn state_events(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let after = optional_u64(params, "afterSequence")?.unwrap_or(0);
    let limit = optional_bounded_u64(params, "limit", 1, domain::STATE_EVENT_MAXIMUM_LIMIT as u64)?
        .unwrap_or(domain::STATE_EVENT_DEFAULT_LIMIT as u64) as usize;
    serde_json::to_value(controller.state_events_since(after, limit))
        .map_err(|error| error.to_string())
}

fn state_block_details(controller: &DeviceController, params: &Value) -> Result<Value, String> {
    let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
    let column = bounded_u32(params, "column", 9)?;
    serde_json::to_value(controller.guarded_block_details(row, column, params)?)
        .map_err(|error| error.to_string())
}

fn bounded_u32(params: &Value, field: &str, maximum: u32) -> Result<u32, String> {
    runtime_request::bounded_u32(params, field, maximum)
}

fn optional_u64(params: &Value, field: &str) -> Result<Option<u64>, String> {
    match params.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{field} must be a non-negative integer")),
    }
}

fn optional_bounded_u64(
    params: &Value,
    field: &str,
    minimum: u64,
    maximum: u64,
) -> Result<Option<u64>, String> {
    let Some(value) = optional_u64(params, field)? else {
        return Ok(None);
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{field} must be an integer from {minimum} through {maximum}"
        ));
    }
    Ok(Some(value))
}

fn normalized_f64(params: &Value, field: &str) -> Result<f64, String> {
    params
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .ok_or_else(|| format!("{field} must be a number from 0 through 1"))
}

fn required_text(params: &Value, field: &str) -> Result<String, String> {
    runtime_request::required_text(params, field)
}

// --- Shared TCP multi-client broker (`--broker-serve`) ----------------------
//
// A private-per-process `--stdio` broker owns exactly one input/output pair
// for its whole life. `--broker-serve` instead binds a loopback TCP listener
// and fans the same JSON-RPC + `device.stateFrame` protocol out to however
// many clients are attached at once, so several hosts on the same machine
// can share one USB owner instead of racing each other for the device.

/// This process's native creation identity (Windows FILETIME, Linux boot/start hash); see
/// `BrokerDescriptor::process_created_at`.
fn own_process_created_at() -> Option<u64> {
    crate::platform::process::os_process_created_at(std::process::id())
}

/// Mirrors `DEVICE_BROKER_DESCRIPTOR_REFRESH_INTERVAL` in
/// `apps/windows/src-tauri/src/device_broker_descriptor.rs`.
const DESCRIPTOR_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// How long the accept loop pauses after a failed `accept`: from 10 ms,
/// doubling per consecutive failure, to at most 1 s.
const ACCEPT_RETRY_FLOOR: Duration = Duration::from_millis(10);
const ACCEPT_RETRY_CEILING: Duration = Duration::from_secs(1);

/// Paces the accept loop through a run of failed `accept` calls and decides
/// which of them are logged: the first of a run, then every hundredth.
#[derive(Default)]
struct AcceptBackoff {
    consecutive_failures: u32,
}

impl AcceptBackoff {
    fn accepted(&mut self) {
        if self.consecutive_failures > 0 {
            eprintln!(
                "The device broker accepted a connection again after {} failed accept(s)",
                self.consecutive_failures
            );
        }
        self.consecutive_failures = 0;
    }

    fn failed(&mut self, error: &std::io::Error) -> Option<String> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let failures = self.consecutive_failures;
        (failures == 1 || failures.is_multiple_of(100)).then(|| {
            format!(
                "The device broker could not accept a connection (failure {failures} in a row): {error}"
            )
        })
    }

    fn delay(&self) -> Duration {
        let doublings = self.consecutive_failures.saturating_sub(1).min(16);
        ACCEPT_RETRY_FLOOR
            .saturating_mul(1 << doublings)
            .min(ACCEPT_RETRY_CEILING)
    }
}

/// The first retry after a failed descriptor refresh. Each further failure
/// doubles it, up to `DESCRIPTOR_REFRESH_INTERVAL`, so a reader holding the
/// file for a moment costs one second and a persistent fault is retried well
/// inside the 150 s after which clients treat the descriptor as stale.
const DESCRIPTOR_REFRESH_RETRY_FLOOR: Duration = Duration::from_secs(1);

/// Keep the published descriptor fresh for as long as this broker holds its
/// claim. A failed write is logged and retried with backoff, never the end of
/// the heartbeat: any process opening the file without delete sharing for
/// longer than `publish_atomically`'s rename budget fails one write, and a
/// heartbeat that stopped there made every client treat this healthy broker
/// as stale about two and a half minutes later. Releasing the claim or the
/// owning server's stop token ends the loop.
fn refresh_descriptor_while_claimed_until<T>(
    claim: &Mutex<Option<T>>,
    interval: Duration,
    retry_floor: Duration,
    stop: &AtomicBool,
    mut write: impl FnMut(&mut T) -> Result<(), String>,
) {
    let mut delay = interval;
    let mut failures: u32 = 0;
    loop {
        let deadline = Instant::now() + delay;
        while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            );
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
        let outcome = {
            let mut claim = claim.lock_recover();
            let Some(file) = claim.as_mut() else {
                return;
            };
            write(file)
        };
        match outcome {
            Ok(()) => {
                if failures > 0 {
                    eprintln!(
                        "The device broker descriptor refresh succeeded again after {failures} failed attempt(s)"
                    );
                }
                failures = 0;
                delay = interval;
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                delay = if failures == 1 {
                    retry_floor
                } else {
                    delay.saturating_mul(2)
                }
                .min(interval);
                eprintln!(
                    "Could not refresh the device broker descriptor (attempt {failures}); retrying in {} ms: {error}",
                    delay.as_millis()
                );
            }
        }
    }
}

/// How long the broker waits after its last client disconnects before it
/// actually gives up the descriptor slot and exits, in case a new client
/// (e.g. a host doing its own reconnect) arrives in the meantime. Host policy
/// (not a Cortex Control frame); BELIEVED, no measurement in the repository.
const IDLE_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const RESIDENT_IDLE_RETRY_DELAY: Duration = Duration::from_secs(30);

/// How long a request waits for the device session a resident broker closed
/// for idleness to come back: the established ready-wait budget, the same one
/// a client already gives a freshly started broker's handshake.
#[cfg(not(test))]
const IDLE_REOPEN_WAIT: Duration =
    Duration::from_millis(quadcortex::profile::READY_WAIT_TIMEOUT_MS);
/// Short under test, so the timeout path is exercised without a 35 s wait.
#[cfg(test)]
const IDLE_REOPEN_WAIT: Duration = Duration::from_millis(1500);

/// Where a device closed by a resident idle release stands, as tests and
/// waiters read it. Stored as an [`IdleReopenEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum IdleReopen {
    /// Closed, and nobody has asked for it since.
    Released,
    /// A request asked for it; its reopen is running.
    Reopening,
    /// The last reopen did not reach a ready session. The next request
    /// starts another.
    Failed(String),
}

/// One device a resident idle release (or the icon's Disconnect) closed.
struct IdleReopenEntry {
    controller: Arc<DeviceController>,
    /// An explicit Disconnect is not idleness: only Connect may reopen it.
    manually_released: bool,
    /// The accepted HID close has not been acknowledged. The cached worker
    /// phase can still look ready until this command is handled, so reconnect
    /// must enter the same FIFO queue as a barrier behind the close.
    close_unacknowledged: bool,
    reopening: bool,
    /// The number of the current, or last, reopen attempt.
    attempt: u64,
    /// The last attempt that failed, and why. A waiter that joined attempt
    /// `n` reports the reason of any failure numbered `n` or later, even if a
    /// newer request has already started the next attempt.
    last_failure: Option<(u64, String)>,
}

impl IdleReopenEntry {
    fn released(controller: Arc<DeviceController>) -> Self {
        Self::released_with_pending_close(controller, false)
    }

    fn released_with_pending_close(
        controller: Arc<DeviceController>,
        close_unacknowledged: bool,
    ) -> Self {
        Self {
            controller,
            manually_released: false,
            close_unacknowledged,
            reopening: false,
            attempt: 0,
            last_failure: None,
        }
    }

    fn view(&self) -> IdleReopen {
        match (&self.last_failure, self.reopening) {
            (_, true) => IdleReopen::Reopening,
            (Some((_, reason)), false) => IdleReopen::Failed(reason.clone()),
            (None, false) => IdleReopen::Released,
        }
    }
}

/// The JSON-RPC error a request gets when the device it needs could not be
/// reopened. `data.category` is the stable signal; the message also says
/// "not connected" for clients that still read text, and `data.retryable`
/// tells relay and MCP clients that trying again is right.
fn idle_reopen_error(id: Value, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{
        "code": -32002,
        "message": message,
        "data": {"category": "notConnected", "retryable": true}
    }})
}

/// The provider name the in-process background MCP host presents in its
/// `broker.hello`. The icon does not list it (the host is this process's own
/// machinery; see [`BrokerHandle::participants_without_host`]), but
/// `broker.clients` still reports it. Display only: the host is recognised by
/// the per-process token it presents in `broker.hello` (see
/// [`crate::resident::host_token`]), never by this name, which any client
/// could send.
pub const IN_PROCESS_HOST_PROVIDER: &str = "QC Remote background host";

/// Test hooks, each keyed by the address of the fixture object it applies to,
/// so tests running in parallel never see each other's hooks.
#[cfg(test)]
type TestHook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
/// The wait stage of an idle reopen, per controller. Without one, a test
/// reopen fails: a real wait reaches for hardware.
#[cfg(test)]
static IDLE_REOPEN_HOOKS: Mutex<Vec<(usize, TestHook)>> = Mutex::new(Vec::new());
/// The reconnect stage of an idle reopen (run under `request_lock`), per
/// controller. Without one it succeeds without touching the worker.
#[cfg(test)]
static IDLE_RECONNECT_HOOKS: Mutex<Vec<(usize, TestHook)>> = Mutex::new(Vec::new());
/// Reconnects whose accepted close is still unacknowledged must use the
/// queued worker barrier. Separate hook makes that decision observable.
#[cfg(test)]
static PENDING_CLOSE_RECONNECT_HOOKS: Mutex<Vec<(usize, TestHook)>> = Mutex::new(Vec::new());
/// Named pause points inside the broker, per `BrokerShared` (keyed by its
/// random instance id, which no later fixture can reuse).
#[cfg(test)]
static TEST_POINTS: Mutex<Vec<(String, &'static str, TestHook)>> = Mutex::new(Vec::new());
/// A shorter shutdown grace, per `BrokerShared`.
#[cfg(test)]
static TEST_GRACE: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());
/// A shorter idle-policy minute, per `BrokerShared`.
#[cfg(test)]
static TEST_MINUTE: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());
/// A shorter native-teardown retry delay, per `BrokerShared`.
#[cfg(test)]
static TEST_IDLE_RETRY_DELAY: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());

#[cfg(test)]
fn test_hook(registry: &Mutex<Vec<(usize, TestHook)>>, key: usize) -> Option<TestHook> {
    registry
        .lock_recover()
        .iter()
        .find(|(registered, _)| *registered == key)
        .map(|(_, hook)| Arc::clone(hook))
}
/// Fallback for an observer that declares no idle policy. A declared policy
/// instead gets its own period plus five minutes, including short periods.
const OBSERVER_IDLE_BACKSTOP: Duration = Duration::from_secs(60 * 60);

/// JSON-RPC error code for a TCP connection that never authenticated. Local
/// to this protocol - not part of `generated_gateway`'s device error codes.
const BROKER_AUTH_ERROR_CODE: i64 = -32001;

/// Total time a freshly accepted socket has to deliver its complete
/// `broker.authenticate` frame. It is one deadline for the whole frame, not a
/// per-read idle timeout, so a client dripping one byte at a time cannot hold
/// a connection slot open indefinitely.
const BROKER_AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// The largest frame accepted before authentication. The handshake carries
/// one short bearer token; the 16 MiB `IPC_MAX_FRAME_BYTES` limit applies
/// only once the connection has authenticated.
const BROKER_AUTH_MAX_FRAME_BYTES: usize = 4 * 1024;

/// How many accepted sockets may be authenticating at once. Further
/// connections are closed on accept, before any thread is spawned for them.
const BROKER_MAX_PENDING_AUTHENTICATIONS: usize = 16;
/// Software owner ceiling, not a firmware limit. The actual retained
/// connection job keeps capacity until its original thread finishes/joins.
const BROKER_MAX_CONNECTION_OWNERS: usize = 256;
const BROKER_PROVIDER_MAX_UTF8_BYTES: usize = 4 * 1024;

/// How many observer connections (trays) one broker will carry. There is one
/// tray per machine; a growing list means connections are being abandoned
/// faster than the broker notices, so further observers are refused with a
/// reason rather than accumulated without limit.
const MAX_OBSERVER_CONNECTIONS: usize = 8;

/// Why this broker could not claim the descriptor slot, naming the broker that
/// holds it when its descriptor can be read.
/// Software document ceiling shared with the descriptor consumer; not a
/// firmware limit. Diagnostics never justify unbounded allocation or a
/// separate permission check followed by reopening another pathname occupant.
const BROKER_DESCRIPTOR_DIAGNOSTIC_MAX_BYTES: usize = 128 * 1024;
fn descriptor_claim_refusal(path: &Path) -> String {
    let holder =
        qc_owner_only_file::read_owner_only_bounded(path, BROKER_DESCRIPTOR_DIAGNOSTIC_MAX_BYTES)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|descriptor| descriptor["processId"].as_u64())
            .map(|pid| format!(" (device broker process {pid})"))
            .unwrap_or_default();
    format!(
        "Could not claim the device broker descriptor slot at {}; a broker{holder} is already \
         published there and keeps the Quad Cortex. This broker exits and the app attaches to \
         that one instead.",
        path.display()
    )
}

/// A broker-level method, answered by the TCP connection loop itself and never
/// by `handle`: how many clients this broker currently serves. A client uses it
/// to tell whether a session-ending call would reach anyone but itself (see
/// `Gateway::disconnect_shared` in the Windows app). It is not part of the
/// generated gateway contract, exactly like `broker.authenticate`.
const BROKER_CLIENT_COUNT_METHOD: &str = "broker.clientCount";
/// Exports are fenced by their owner admission and must never reopen a
/// deliberately released device session.
const IDLE_RELEASE_EXPORT_METHODS: &[&str] =
    &["broker.backupTarget", "broker.createDeviceBackupBound"];
/// An explicit person-facing Disconnect action. Unlike `device.disconnect`,
/// which is safe for one participant to use while other apps share the
/// broker, this deliberately closes the selected device for every attached
/// client. It is kept out of the public device action contract and is called
/// only by the desktop connection panel.
const BROKER_DISCONNECT_DEVICE_METHOD: &str = "broker.disconnectDevice";

/// Whether a request may run without the broker-wide request lock.
///
/// `handle` used to be reached by one caller at a time: `--stdio` has a single
/// request loop, and each app process serialised its own calls. With several
/// processes attached to one broker, their requests arrive on separate
/// threads, and multi-step flows - a planned write and its verification, the
/// quarantine check and the write it guards, a backup - could interleave with
/// another client's. So every request runs under one lock, except these four:
/// once served, each only copies out state the workers already cached, and
/// each is what a client polls several times a second (`system.status` is
/// every window's 1 Hz health poll). Letting them wait behind a three-minute
/// backup would report a healthy device as unresponsive.
///
/// Skipping this lock is not the same as never touching USB. Only
/// `system.status` also skips `prepare_idle_released_target`; a participant's
/// `device.stateEvents`, `device.tempoClock` or `device.masterVolume` is device
/// demand and, after a resident idle release, starts or joins a USB reopen
/// and waits for it - and that reopen reconnects under this very lock. That is
/// why only `system.status` is answered straight from a connection's reader
/// (`quadcortex::device::answers_beside_running_request`), and the other three
/// keep their place in the connection's queue.
fn runs_without_request_lock(method: &str) -> bool {
    quadcortex::device::runs_without_request_lock(method)
}

/// The one method a connection's reader answers itself, beside a request that
/// is still running on the same connection.
fn answers_beside_running_request(method: &str) -> bool {
    quadcortex::device::answers_beside_running_request(method)
}

/// How many requests may wait on one connection behind the one it is
/// running. Every host sends a request and waits for its reply, and sends
/// only `system.status` beside it, so a well-behaved connection never queues
/// even one; four leaves room for a pipelining client without letting a
/// misbehaving one hold more than four frames (at most
/// `quadcortex::domain::IPC_MAX_FRAME_BYTES`, 16 MiB, each) while a backup
/// runs.
const CONNECTION_QUEUE_DEPTH: usize = 4;
/// A request arrived while its connection's queue already held
/// `CONNECTION_QUEUE_DEPTH` requests. It was neither queued nor sent to the
/// device; `data.code` is `CONNECTION_QUEUE_FULL`, and it may be retried once
/// earlier requests on the connection have been answered.
const CONNECTION_QUEUE_FULL_ERROR_CODE: i64 = -32014;

fn connection_queue_full(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": CONNECTION_QUEUE_FULL_ERROR_CODE,
            "message": format!(
                "This connection already has {CONNECTION_QUEUE_DEPTH} requests waiting behind \
                 the one that is running; this request was not queued or sent. Retry once \
                 they have been answered."
            ),
            "data": {
                "code": "CONNECTION_QUEUE_FULL",
                "retryable": true,
                "requestSent": false,
                "queueDepth": CONNECTION_QUEUE_DEPTH
            }
        }
    })
}

fn unix_time_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

fn generate_token(byte_len: usize) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use rand::RngExt as _;
    let mut bytes = vec![0_u8; byte_len];
    rand::rng().fill(bytes.as_mut_slice());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn try_claim_descriptor_slot(path: &Path) -> Result<qc_owner_only_file::ClaimedFile, ()> {
    qc_owner_only_file::ClaimedFile::try_claim(path).map_err(|_| ())
}

fn write_descriptor(
    file: &mut qc_owner_only_file::ClaimedFile,
    descriptor: &BrokerDescriptor,
) -> Result<(), String> {
    let json = serde_json::to_vec(descriptor)
        .map_err(|error| format!("Failed to serialize descriptor: {error}"))?;
    file.write(&json)
}

/// Requires the first frame a TCP client sends to be a `broker.authenticate`
/// request carrying the correct bearer token.
///
/// This is not a separate out-of-band handshake step: it is an ordinary
/// length-prefixed JSON frame using the exact same framing as every other
/// request (`read_frame_bytes` / `write_response`), just sent before any
/// other RPC traffic is accepted on the connection:
/// `{"method": "broker.authenticate", "params": {"token": "<bearer_token>"}, "id": 0}`.
/// It cannot reuse `read_request`/`Request`, because that struct requires a
/// `"jsonrpc"` field the handshake frame omits and rejects `id: 0` outright
/// (`handle` treats id 0 as invalid) - both true of every ordinary RPC
/// frame here on purpose, so a handshake frame can never be mistaken for
/// one. A missing/wrong token, a malformed frame, or any first message
/// that isn't `broker.authenticate` all get a normal error-shaped response
/// frame and the connection is closed without ever reaching `handle`.
///
/// Until it succeeds the socket is untrusted: the whole frame must arrive
/// within `BROKER_AUTH_TIMEOUT` and fit in `BROKER_AUTH_MAX_FRAME_BYTES`, and
/// the token is compared in constant time. On success both socket timeouts
/// are cleared again, because the authenticated serve loop relies on
/// blocking reads and manages its own write deadlines.
///
/// `on_accepted` runs once the token has been verified and before the success
/// reply is written: a client must never be told it is authenticated while the
/// broker still counts it as an unauthenticated socket, or a disconnect from
/// another participant processed in that gap would not see it attached.
/// When `on_accepted` refuses (a shutdown was committed while this socket was
/// still authenticating), its message is sent as an error instead of the
/// success reply and the connection is closed.
fn authenticate_tcp_client_then(
    stream: &mut TcpStream,
    bearer_token: &str,
    instance_id: &str,
    on_accepted: impl FnOnce(AuthenticatedAs) -> Result<(), String>,
) -> Result<(), ()> {
    authenticate_tcp_client_within_then(
        stream,
        bearer_token,
        instance_id,
        BROKER_AUTH_TIMEOUT,
        on_accepted,
    )
}

#[cfg(test)]
fn authenticate_tcp_client(
    stream: &mut TcpStream,
    bearer_token: &str,
    instance_id: &str,
) -> Result<(), ()> {
    authenticate_tcp_client_within(stream, bearer_token, instance_id, BROKER_AUTH_TIMEOUT)
}

#[cfg(test)]
fn authenticate_tcp_client_within(
    stream: &mut TcpStream,
    bearer_token: &str,
    instance_id: &str,
    timeout: Duration,
) -> Result<(), ()> {
    authenticate_tcp_client_within_then(stream, bearer_token, instance_id, timeout, |_| Ok(()))
}

fn authenticate_tcp_client_within_then(
    stream: &mut TcpStream,
    bearer_token: &str,
    instance_id: &str,
    timeout: Duration,
    on_accepted: impl FnOnce(AuthenticatedAs) -> Result<(), String>,
) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    stream.set_write_timeout(Some(timeout)).map_err(|_| ())?;
    let outcome =
        authenticate_tcp_client_before(stream, bearer_token, instance_id, deadline, on_accepted);
    if outcome.is_ok() {
        stream.set_read_timeout(None).map_err(|_| ())?;
        stream.set_write_timeout(None).map_err(|_| ())?;
    }
    outcome
}

/// Reads from a socket under one absolute deadline: each read gets only the
/// time remaining, so slow partial writes cannot each earn a fresh timeout.
struct DeadlineReader<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "broker authentication deadline elapsed",
            ));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

fn authenticate_tcp_client_before(
    stream: &mut TcpStream,
    bearer_token: &str,
    instance_id: &str,
    deadline: Instant,
    on_accepted: impl FnOnce(AuthenticatedAs) -> Result<(), String>,
) -> Result<(), ()> {
    let mut reader = DeadlineReader {
        stream: &*stream,
        deadline,
    };
    let frame = match read_frame_bytes_limited(&mut reader, BROKER_AUTH_MAX_FRAME_BYTES) {
        Ok(Some(body)) => serde_json::from_slice::<Value>(&body).ok(),
        _ => None,
    };
    let Some(frame) = frame else {
        let _ = write_response(
            stream,
            &error(
                json!(0),
                BROKER_AUTH_ERROR_CODE,
                "Expected broker.authenticate as the first frame",
            ),
        );
        return Err(());
    };
    let id = frame.get("id").cloned().unwrap_or(json!(0));
    let method = frame.get("method").and_then(Value::as_str);
    let token = frame
        .get("params")
        .and_then(|params| params.get("token"))
        .and_then(Value::as_str);
    let token_matches =
        token.is_some_and(|token| bool::from(token.as_bytes().ct_eq(bearer_token.as_bytes())));
    if method != Some("broker.authenticate") || !token_matches {
        let _ = write_response(
            stream,
            &error(id, BROKER_AUTH_ERROR_CODE, "Authentication required"),
        );
        return Err(());
    }
    // The role is declared in the handshake itself, so a control connection
    // is never, even for an instant, counted as a participant. Absent is the
    // participant every existing client has always been.
    let role = match frame.get("params").and_then(|params| params.get("role")) {
        None => AuthenticatedAs::Participant,
        Some(Value::String(role)) if role == CONTROL_ROLE => AuthenticatedAs::Control,
        Some(Value::String(role)) if role == DEMO_PROVIDER_ROLE => AuthenticatedAs::DemoProvider,
        Some(_) => {
            let _ = write_response(
                stream,
                &error(id, BROKER_AUTH_ERROR_CODE, "Unsupported connection role"),
            );
            return Err(());
        }
    };
    if let Err(message) = on_accepted(role) {
        let _ = write_response(stream, &error(id, -32002, &message));
        return Err(());
    }
    let result = match role {
        AuthenticatedAs::Participant => json!({"instanceId": instance_id}),
        AuthenticatedAs::Control => json!({"instanceId": instance_id, "role": CONTROL_ROLE}),
        AuthenticatedAs::DemoProvider => {
            json!({"instanceId": instance_id, "role": DEMO_PROVIDER_ROLE})
        }
    };
    if write_response(
        stream,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}),
    )
    .is_err()
    {
        return Err(());
    }
    Ok(())
}

/// What a socket authenticated as. See [`CONTROL_ROLE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthenticatedAs {
    Participant,
    Control,
    DemoProvider,
}

/// The role `qc-device-broker.exe --control` declares in `broker.authenticate`
/// (`{"token": ..., "role": "control"}`). A control connection is never a
/// participant or an observer: it is not device demand, never renews an idle
/// clock, is not listed to observers, and may call only the
/// `CONTROL_METHODS`. See `crate::control`.
pub(crate) const CONTROL_ROLE: &str = "control";
const DEMO_PROVIDER_ROLE: &str = "demoProvider";
/// Read-only: the broker's own accounting, from caches. No USB, no clock.
pub(crate) const BROKER_CONTROL_STATUS_METHOD: &str = "broker.controlStatus";
/// The icon's Disconnect without its exit: see [`BrokerHandle::release_devices_staying`].
pub(crate) const BROKER_CONTROL_RELEASE_METHOD: &str = "broker.controlRelease";
/// The icon's Connect: [`BrokerHandle::reconnect`].
pub(crate) const BROKER_CONTROL_CONNECT_METHOD: &str = "broker.controlConnect";
/// The running binary's own version (`cli::Binary::version`, the app's
/// version in the shipped build), set once at startup.
static BINARY_VERSION: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// Record the running binary's version for `broker.controlStatus`.
pub fn set_binary_version(version: &'static str) {
    let _ = BINARY_VERSION.set(version);
}

/// `brokerVersion` only when the binary told us; never a stand-in.
pub(crate) fn with_broker_version(status: &mut Value, version: Option<&str>) {
    if let Some(version) = version {
        status["brokerVersion"] = json!(version);
    }
}

/// Legacy control-release refusal, retained for clients talking to older brokers.
pub(crate) const CONTROL_PARTICIPANTS_ATTACHED: &str = "PARTICIPANTS_ATTACHED";
/// Everything a control connection may ask. `broker.shutdown` only with
/// `force: true`, the icon's forced Release and exit.
const CONTROL_METHODS: [&str; 6] = [
    BROKER_CONTROL_STATUS_METHOD,
    "broker.trayMenu",
    "broker.trayAction",
    BROKER_CONTROL_RELEASE_METHOD,
    BROKER_CONTROL_CONNECT_METHOD,
    "broker.shutdown",
];

/// One request from one TCP client: the broker-level methods answered here,
/// everything else through `handle`, under the broker-wide request lock
/// unless `runs_without_request_lock` exempts it.
fn dispatch_demo_provider_request(
    shared: &BrokerShared,
    connection: u64,
    request: Request,
) -> Value {
    let result = (|| -> Result<Value, String> {
        if shared
            .clients
            .lock_recover()
            .connections
            .get(&connection)
            .is_none_or(|row| row.demo_provider_closed)
        {
            return Err("Demo provider connection has ended".into());
        }
        if request.jsonrpc != "2.0" || request.id.as_u64().is_none_or(|id| id == 0) {
            return Err("Invalid JSON-RPC request".into());
        }
        let params = request
            .params
            .as_object()
            .ok_or("Demo params must be an object")?;
        let readiness = shared.selected_target();
        match request.method.as_str() {
            "broker.demoRegister" => {
                if params.len() != 1 {
                    return Err("Register takes exactly sessionId".into());
                }
                let id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or("Missing sessionId")?;
                if id.trim().is_empty() || id.len() > 512 {
                    return Err("Invalid sessionId".into());
                }
                // A registration is never identified by a persisted/UI-chosen
                // id: even re-registration on the same socket has a new lease.
                let session = format!("{}:demo:{}", shared.instance_id, generate_token(16));
                shared.demo.register(connection, session.clone())?;
                shared.refresh_targets();
                Ok(json!({"target":shared.targets.identity(),"providerSessionId":session}))
            }
            "broker.demoNext" => {
                if !params.is_empty() {
                    return Err("Next takes no params".into());
                }
                serde_json::to_value(
                    shared
                        .demo
                        .next(connection, crate::demo_provider::NEXT_TIMEOUT)?,
                )
                .map_err(|e| e.to_string())
            }
            "broker.demoWithdraw" => {
                if !params.is_empty() {
                    return Err("Withdraw takes no params".into());
                }
                let withdrawn = shared.demo.withdraw(connection);
                shared.refresh_targets();
                Ok(json!({"withdrawn":withdrawn}))
            }
            "broker.demoAdmit" | "broker.demoReply" => {
                let target = wire::parse_target_identity(
                    params.get("target").cloned().ok_or("Missing target")?,
                )?;
                let sequence = params
                    .get("sequence")
                    .and_then(Value::as_u64)
                    .ok_or("Missing sequence")?;
                if let Err(message) = readiness {
                    shared
                        .demo
                        .reject(connection, &target, sequence, message.message())?;
                    return Ok(if request.method == "broker.demoAdmit" {
                        json!({"admitted":false,"reason":"readinessUnavailable"})
                    } else {
                        json!({"accepted":false,"reason":"readinessUnavailable"})
                    });
                }
                if request.method == "broker.demoAdmit" {
                    if params.len() != 2 {
                        return Err("Admit takes target and sequence".into());
                    }
                    let admitted = shared.demo.admit(connection, &target, sequence)?;
                    Ok(
                        json!({"admitted":admitted,"reason":if admitted { "current" } else { "superseded" }}),
                    )
                } else {
                    if params.len() != 3 {
                        return Err("Reply takes target, sequence and result or error".into());
                    }
                    let response = match (params.get("result"), params.get("error")) {
                        (Some(value), None) => json!({"result":value}),
                        (None, Some(value))
                            if value.get("code").and_then(Value::as_i64).is_some()
                                && value.get("message").and_then(Value::as_str).is_some() =>
                        {
                            json!({"error":value})
                        }
                        _ => {
                            return Err("Reply requires exactly one result or JSON-RPC error".into())
                        }
                    };
                    let accepted =
                        shared
                            .demo
                            .reply(connection, &target, sequence, Ok(response))?;
                    Ok(
                        json!({"accepted":accepted,"reason":if accepted { "current" } else { "superseded" }}),
                    )
                }
            }
            _ => Err("This method is not available to a demoProvider connection".into()),
        }
    })();
    match result {
        Ok(value) => json!({"jsonrpc":"2.0","id":request.id,"result":value}),
        Err(message) => error(request.id, -32002, &message),
    }
}

#[cfg(test)]
fn dispatch_broker_request(shared: &BrokerShared, request: Request) -> Value {
    dispatch_broker_client_request(shared, None, request)
}

/// API liveness is independent of device readiness. A busy cache must not make
/// initial host attachment fail, and must not fabricate a usable target either.
fn dispatch_cached_status(
    shared: &BrokerShared,
    connection_id: Option<u64>,
    request: Request,
) -> Value {
    if request.jsonrpc != "2.0" || request.id.as_u64().is_none_or(|id| id == 0) {
        return error(request.id, -32600, "Invalid JSON-RPC request");
    }
    if !request.params.is_object() {
        return error(request.id, -32602, "JSON-RPC params must be an object");
    }
    if let Err(message) = generated_gateway::validate_params(&request.method, &request.params) {
        return error(request.id, -32602, &message);
    }
    let canonical = shared.selected_target();
    let controller = match &canonical {
        Ok(canonical) => match shared.request_controller(
            connection_id,
            request.physical_only,
            canonical.as_ref(),
        ) {
            Ok(controller) => controller,
            Err(message) => return error(request.id, -32002, message),
        },
        Err(_) => shared.controller_for(connection_id),
    };
    let observed = shared.targets.identity();
    if request
        .expected_broker_target
        .as_ref()
        .is_some_and(|expected| {
            (request.physical_only && expected.source != crate::target::TargetSource::Physical)
                || canonical.is_err()
                || observed.as_ref() != Some(expected)
        })
    {
        return error(
            request.id,
            -32002,
            "Broker target changed or is unavailable; request was not sent",
        );
    }
    let mut result = runtime_status_payload(controller.try_cached_status(), true);
    if canonical.is_ok() {
        let target = observed.clone().filter(|target| {
            !request.physical_only
                || (target.source == crate::target::TargetSource::Physical
                    && canonical
                        .as_ref()
                        .ok()
                        .and_then(Option::as_ref)
                        .is_some_and(|chosen| Arc::ptr_eq(chosen, &controller)))
        });
        // A second try-only observation fences metadata, not API liveness.
        // Never turn transient uncertainty into targetReady=false or a stale ID.
        if shared.selected_target().is_ok() && shared.targets.identity() == observed {
            result["targetReady"] = json!(target.is_some());
            if let Some(target) = target {
                result["brokerTarget"] = json!(target);
            }
        } else if request.expected_broker_target.is_some() {
            return error(
                request.id,
                -32002,
                "Broker target changed or is unavailable; request was not sent",
            );
        }
    } else if matches!(canonical, Err(TargetSelectionError::Ambiguous)) {
        // Unlike cache contention, verified ambiguity is an authoritative
        // unavailable observation. Clear prior demo publication explicitly.
        result["targetReady"] = json!(false);
    }
    let mut response = json!({"jsonrpc":"2.0", "id": request.id, "result": result});
    if let Some(target) = response["result"].get("brokerTarget").cloned() {
        response["brokerTarget"] = target;
    }
    response
}

fn dispatch_broker_client_request(
    shared: &BrokerShared,
    connection_id: Option<u64>,
    mut request: Request,
) -> Value {
    if let Some(id) = connection_id.filter(|id| shared.clients.lock_recover().is_demo_provider(*id))
    {
        return dispatch_demo_provider_request(shared, id, request);
    }
    if request.method.starts_with("broker.demo") {
        return error(
            request.id,
            -32601,
            "A dedicated demoProvider connection is required",
        );
    }
    // A control connection is served by `dispatch_control_request` alone.
    // Anything that reaches this from one (the reader answers `system.status`
    // itself) is refused: it is not a participant, so no request of its may
    // count as device demand.
    if connection_id.is_some_and(|id| shared.clients.lock_recover().is_control(id)) {
        return error(
            request.id,
            -32601,
            "This method is not available to a control connection",
        );
    }
    if request.method == "system.status" {
        return dispatch_cached_status(shared, connection_id, request);
    }
    // Demo calls remain pinned to the UI provider; physical-only GUI calls
    // never enter this route, including during physical discovery.
    let canonical = match shared.selected_target() {
        Ok(controller) => controller,
        Err(message)
            if !request.physical_only
                && generated_gateway::broker_dispatch(&request.method).is_some() =>
        {
            return error(request.id, -32002, message.message());
        }
        Err(_) => None,
    };
    let selected_controller = if generated_gateway::broker_dispatch(&request.method).is_some() {
        match shared.request_controller(connection_id, request.physical_only, canonical.as_ref()) {
            Ok(controller) => controller,
            Err(message) => return error(request.id, -32002, message),
        }
    } else {
        shared.controller_for(connection_id)
    };
    if request.physical_only
        && request
            .expected_broker_target
            .as_ref()
            .is_some_and(|target| target.source != crate::target::TargetSource::Physical)
    {
        return error(
            request.id,
            -32002,
            "A physical-only request cannot use a demo identity",
        );
    }
    if request
        .expected_broker_target
        .as_ref()
        .is_some_and(|expected| shared.targets.identity().as_ref() != Some(expected))
    {
        return error(
            request.id,
            -32002,
            "Broker target changed; request was not sent",
        );
    }
    if !request.physical_only
        && generated_gateway::broker_dispatch(&request.method).is_some()
        && shared
            .targets
            .identity()
            .is_some_and(|target| target.source == crate::target::TargetSource::Demo)
    {
        if request.origin.as_deref() == Some(RESOURCE_READ_ORIGIN)
            && !resource_read_may_reach(&request.method)
        {
            return error(request.id, -32601, "This method is not a resource read");
        }
        let result = shared
            .targets
            .admit(request.expected_broker_target.as_ref())
            .map_err(|_| "Broker target changed; request was not sent".to_string())
            .and_then(|lease| {
                let target = lease.identity().clone();
                shared
                    .demo
                    .invoke(lease, &request.method, request.params)
                    .map(|result| (target, result))
            });
        shared.refresh_targets();
        return match result {
            Ok((target, mut envelope)) => {
                envelope["brokerTarget"] = json!(target);
                if let Some(result) = envelope.get_mut("result").filter(|value| value.is_object()) {
                    result["brokerTarget"] = json!(target);
                    result["targetReady"] = json!(true);
                }
                envelope["jsonrpc"] = json!("2.0");
                envelope["id"] = request.id;
                shared.fence_result(Some(&target), envelope)
            }
            Err(message) => error(request.id, -32002, &message),
        };
    }
    // Before every other branch: a resource read reaches none of them, so no
    // broker-level method (a `broker.selectDevice` with `autoConnect`, say)
    // can be reached through the narrowing.
    if request.origin.as_deref() == Some(RESOURCE_READ_ORIGIN) {
        return dispatch_resource_read(shared, connection_id, selected_controller, request);
    }
    if matches!(
        request.method.as_str(),
        "broker.hello"
            | "broker.clients"
            | BROKER_CLIENT_COUNT_METHOD
            | "broker.shutdown"
            | "broker.devices"
            | "broker.selectDevice"
            | BROKER_DISCONNECT_DEVICE_METHOD
    ) && (request.jsonrpc != "2.0" || request.id.as_u64().is_none_or(|id| id == 0))
    {
        return error(request.id, -32600, "Invalid JSON-RPC request");
    }
    if request.method == "broker.devices" {
        // Read the caller's selection (`clients`) before taking `devices`:
        // never hold `devices` while acquiring `clients` (see BrokerShared).
        let selected_device_id = shared.controller_for(connection_id).device_id();
        return match quadcortex_hid::usb::connected_devices() {
            Ok(devices) => {
                let owners = shared.devices.lock_recover();
                let rows: Vec<_> = devices.into_iter().map(|device| {
                    let owner = if shared.controller.device_id().as_deref() == Some(&device.id) {
                        Some(&shared.controller)
                    } else { owners.iter().find(|owner| owner.device_id().as_deref() == Some(&device.id)) };
                    let status = owner.map(|owner| owner.status());
                    json!({"id":device.id,"name":device.name,"connected":status.as_ref().is_some_and(|status| status.connected),
                        "phase":status.map(|status| status.phase).unwrap_or_else(|| "disconnected".into())})
                }).collect();
                drop(owners);
                json!({"jsonrpc":"2.0","id":request.id,"result":{
                    "devices":rows,"selectedDeviceId":selected_device_id
                }})
            }
            Err(message) => error(request.id, -32002, &message.to_string()),
        };
    }
    if request.method == "broker.selectDevice" {
        let Some(id) = request
            .params
            .get("deviceId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 8192)
        else {
            return error(
                request.id,
                -32602,
                "deviceId must identify an available Quad Cortex",
            );
        };
        let Some(connection_id) = connection_id else {
            return error(request.id, -32600, "A live connection is required");
        };
        let auto_connect = match request.params.get("autoConnect") {
            None | Some(Value::Bool(false)) => false,
            Some(Value::Bool(true)) => true,
            _ => return error(request.id, -32602, "autoConnect must be a boolean"),
        };
        return match shared.select_device(connection_id, id) {
            Ok(()) => {
                let controller = shared.controller_for(Some(connection_id));
                // A device a resident idle release closed comes back with the
                // first request routed to it, which waits for it; a second,
                // unawaited reconnect here would only race that one.
                let idle_released = {
                    let mut clients = shared.clients.lock_recover();
                    clients.idle_released = false;
                    clients.idle_reopen_state(&controller).is_some()
                };
                if auto_connect && !idle_released {
                    if let Err(message) =
                        background_policy::selected(shared, connection_id, controller)
                    {
                        return error(request.id, -32002, &message.to_string());
                    }
                }
                json!({"jsonrpc":"2.0","id":request.id,"result":{"selectedDeviceId":id}})
            }
            Err(message) => error(request.id, -32002, &message),
        };
    }
    if request.method == "broker.hello" {
        let mut row = match connection_hello(&request.params) {
            Ok(row) => row,
            Err(message) => return error(request.id, -32602, message),
        };
        let mut clients = shared.clients.lock_recover();
        if clients.shutting_down {
            return error(request.id, -32002, "Broker shutdown is already committed");
        }
        if matches!(row.role, ConnectionRole::Observer { .. })
            && clients.observers_other_than(connection_id) >= MAX_OBSERVER_CONNECTIONS
        {
            return error(
                request.id,
                -32002,
                "Too many observer connections are attached to this broker",
            );
        }
        let Some(existing) = connection_id.and_then(|id| clients.connections.get_mut(&id)) else {
            return error(request.id, -32600, "Connection is no longer attached");
        };
        let selected = existing
            .selected_device
            .as_ref()
            .unwrap_or(&shared.controller)
            .clone();
        if matches!(row.role, ConnectionRole::Participant) && clients.device_is_closing(&selected) {
            return error(
                request.id,
                -32002,
                "The selected device session is closing; attach again after it has closed",
            );
        }
        let existing = clients
            .connections
            .get_mut(&connection_id.expect("live row checked"))
            .expect("live row checked under the same accounting lock");
        let became_participant =
            matches!(row.role, ConnectionRole::Participant) && !existing.is_participant();
        row.selected_device = existing.selected_device.take();
        row.implicit_selection = existing.implicit_selection;
        row.output = existing.output.take();
        row.admission = Arc::clone(&existing.admission);
        *existing = row;
        if became_participant {
            clients.idle_release_retry_not_before = None;
            shared.observer_changed.notify_all();
        }
        return json!({"jsonrpc":"2.0", "id":request.id, "result":{"accepted":true}});
    }
    if request.method == "broker.shutdown" {
        return request_broker_shutdown_after_precheck(shared, request, connection_id, || {});
    }
    if request.method == "broker.clients" {
        let clients = shared.clients.lock_recover();
        let rows = clients.authenticated_rows();
        return json!({"jsonrpc":"2.0", "id":request.id, "result":{"clients":rows}});
    }
    if request.method == BROKER_CLIENT_COUNT_METHOD {
        return json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {"clients": shared.participant_count_for(connection_id)}
        });
    }
    let forced_disconnect = request.method == BROKER_DISCONNECT_DEVICE_METHOD;
    if forced_disconnect {
        request.method = generated_gateway::rpc::DISCONNECT.into();
        if let Err(message) = generated_gateway::validate_params(&request.method, &request.params) {
            return error(request.id, -32602, &message);
        }
    }
    let controller = selected_controller;
    // Dialog-bound exports must never reopen a session to make a stale fence
    // appear usable. Their owner admission below is authoritative.
    if !IDLE_RELEASE_EXPORT_METHODS.contains(&request.method.as_str()) {
        if let Err(message) =
            shared.prepare_idle_released_target(connection_id, &controller, &request.method)
        {
            return idle_reopen_error(request.id, &message);
        }
    }
    // `apps/windows/src-tauri`'s `Gateway::leave_shared_session_open` makes
    // this same "am I the sole client" check client-side, as an RPC-then-RPC
    // sequence: ask `broker.clientCount`, then - in a *separate* request -
    // either stop locally or forward `device.disconnect`. That is a real
    // TOCTOU: a second client can attach in the gap between the two calls,
    // and the first client's stale "I'm alone" answer still forwards a real
    // disconnect that tears down the session the new client just reached.
    // The broker is the only place that can decide this atomically, because
    // `clients` here is the same lock the accept loop and idle-shutdown
    // share - so it is authoritative here too: a `device.disconnect` that
    // arrives while more than one client is attached is answered as if the
    // session was left open, without ever calling `controller.disconnect()`,
    // regardless of what the caller believed when it sent the request. The
    // Windows app's own pre-check is left in place as a latency optimization
    // (it avoids the round trip in the common single-client case) but no
    // longer has to be correct for correctness to hold.
    //
    // The count is re-checked *twice*: once here (cheap, rejects the common
    // "someone else is clearly still attached" case immediately), and once
    // more right before `handle()` actually runs `controller.disconnect()`,
    // after `request_lock` is held (see below). Holding `clients` locked
    // across the wait for `request_lock` was considered and rejected: that
    // lock can be held for as long as another client's in-flight operation
    // takes (a backup, up to ~215s), and holding `clients` for that whole
    // stretch would block the accept loop from counting any new connection
    // for the duration - trading a narrow correctness race for a real
    // availability stall. Re-checking immediately before the effectful call
    // instead reserves closing admission under that lock after request_lock
    // is acquired; new participants cannot join until disconnect acknowledges.
    if !forced_disconnect
        && request.method == generated_gateway::rpc::DISCONNECT
        && shared.disconnect_blocked_for(connection_id, &controller)
    {
        return json!({
            "jsonrpc": "2.0", "id": request.id,
            "result": connection_state(&controller,
                "Another client is still attached; the shared session was left open")
        });
    }
    if runs_without_request_lock(&request.method) {
        let expected = request.expected_broker_target.clone();
        if !shared.target_matches(expected.as_ref()) {
            return error(
                request.id,
                -32002,
                "Broker target changed; request was not sent",
            );
        }
        return shared.fence_result(
            expected.as_ref(),
            handle(&controller, &shared.performance_midi, request),
        );
    }
    if request.method == generated_gateway::rpc::DISCONNECT {
        shared.test_point("disconnect:after_precheck");
    }
    shared.test_point("request:before-request-lock");
    // A poisoned lock only means another client's request panicked; the
    // exclusion it provides is still intact.
    let _exclusive = shared
        .request_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if shared.clients.lock_recover().shutting_down {
        return error(request.id, -32002, "Broker shutdown is already committed");
    }
    if forced_disconnect && shared.clients.lock_recover().reopens_in_flight != 0 {
        return error(
            request.id,
            -32002,
            "The Quad Cortex is reopening; choose Disconnect again in a moment",
        );
    }
    if connection_id.is_some_and(|id| !shared.clients.lock_recover().connections.contains_key(&id))
    {
        return error(
            request.id,
            -32002,
            "Connection is no longer attached; queued action was cancelled",
        );
    }
    // A request can pass the idle-release precheck just before a resident
    // close publishes its completed release record, then wait here on the
    // request lock that the close owns. Re-check after acquiring that lock so
    // this request joins/restarts the newly visible reopen instead of reaching
    // the closed controller. Drop the lock before redispatching because the
    // reopen itself must acquire it.
    let idle_release_published_while_waiting =
        idle_release_recheck(shared, connection_id, &controller, &request.method);
    if idle_release_published_while_waiting {
        drop(_exclusive);
        return dispatch_broker_client_request(shared, connection_id, request);
    }
    // Read this only after request_lock and the late marker recheck. An idle
    // release can publish an accepted but unacknowledged close while this
    // explicit session command waits for the lock.
    let preserve_idle_close_marker = {
        let clients = shared.clients.lock_recover();
        clients
            .idle_entry(&controller)
            .is_some_and(|entry| entry.close_unacknowledged || entry.manually_released)
            && matches!(
                request.method.as_str(),
                generated_gateway::rpc::DISCONNECT
                    | generated_gateway::rpc::RECONNECT
                    | generated_gateway::rpc::RESET_SESSION
            )
    };
    let queue_reconnect_after_idle_close = request.method == generated_gateway::rpc::RECONNECT
        && shared
            .clients
            .lock_recover()
            .idle_entry(&controller)
            .is_some_and(|entry| entry.close_unacknowledged);
    // The second, immediately-before-effect re-check: a second client could
    // have attached while this request queued behind `request_lock` (e.g.
    // behind another client's in-flight backup).
    let _disconnect_admission =
        if !forced_disconnect && request.method == generated_gateway::rpc::DISCONNECT {
            match shared.reserve_disconnect_for(connection_id, &controller) {
                Some(admission) => Some(admission),
                None => {
                    return json!({
                        "jsonrpc": "2.0", "id": request.id,
                        "result": connection_state(&controller,
                            "Another client is still attached; the shared session was left open")
                    })
                }
            }
        } else {
            None
        };
    if _disconnect_admission.is_some() {
        shared.test_point("disconnect:reserved");
    }
    if request.method == generated_gateway::rpc::DISCONNECT && !preserve_idle_close_marker {
        // The disconnect really runs now, under `request_lock`: the session is
        // the caller's to close, so a pending idle reopen of this device is
        // withdrawn rather than left to reconnect it afterwards.
        shared
            .clients
            .lock_recover()
            .forget_idle_release(&controller);
    }
    let expected = request.expected_broker_target.clone();
    if !shared.target_matches(expected.as_ref()) {
        return error(
            request.id,
            -32002,
            "Broker target changed; request was not sent",
        );
    }
    let changes_session = matches!(
        request.method.as_str(),
        generated_gateway::rpc::RECONNECT
            | generated_gateway::rpc::RESET_SESSION
            | generated_gateway::rpc::DISCONNECT
    );
    let lifecycle = changes_session
        .then(|| {
            expected
                .as_ref()
                .and_then(|from| shared.capture_lifecycle(&controller, &request.method, from))
        })
        .flatten();
    let disconnecting = request.method == generated_gateway::rpc::DISCONNECT;
    let response = if forced_disconnect {
        let all = shared.all_controllers();
        let report = shared.disconnect_controllers_report(vec![Arc::clone(&controller)]);
        let accepted = report.midi_retired && !report.queued.is_empty();
        let pending = !report.close_unacknowledged.is_empty();
        if report.midi_retired && !report.queued.is_empty() {
            let mut clients = shared.clients.lock_recover();
            clients.record_release(report.queued, report.close_unacknowledged, true);
            clients.idle_released = all
                .iter()
                .all(|device| clients.idle_entry(device).is_some());
        }
        if report.failures.is_empty() || accepted {
            let message = if pending {
                "Disconnect requested; native close verification is pending. Choose Connect to wait for retirement and reconnect."
            } else {
                "Disconnected. Choose Connect to use the Quad Cortex again."
            };
            json!({"jsonrpc":"2.0", "id":request.id,
                "result":connection_state(&controller, message)})
        } else {
            error(request.id, -32002, &report.failures.join("; "))
        }
    } else {
        handle_with_disconnect_admission(
            &controller,
            &shared.performance_midi,
            request,
            _disconnect_admission.map(|admission| Box::new(admission) as Box<dyn Send>),
            queue_reconnect_after_idle_close,
        )
    };
    if preserve_idle_close_marker && !disconnecting && response.get("error").is_none() {
        let mut clients = shared.clients.lock_recover();
        clients.idle_released = false;
        clients.idle_release_retry_not_before = None;
        clients.forget_idle_release(&controller);
        shared.observer_changed.notify_all();
    }
    if changes_session {
        shared.finish_lifecycle(lifecycle.as_ref(), expected.as_ref(), response)
    } else {
        shared.fence_result(expected.as_ref(), response)
    }
}

/// Reconcile an idle close published while a participant waited for
/// `request_lock`. Export commands intentionally remain fenced without
/// reopening; reconnect/reset own the session and withdraw a late marker.
fn idle_release_recheck(
    shared: &BrokerShared,
    connection_id: Option<u64>,
    controller: &Arc<DeviceController>,
    method: &str,
) -> bool {
    let mut clients = shared.clients.lock_recover();
    let (authenticated, participant) = connection_id.map_or((true, true), |id| {
        clients.connections.get(&id).map_or((false, false), |row| {
            (row.authenticated, row.is_participant())
        })
    });
    let session_command = matches!(
        method,
        generated_gateway::rpc::DISCONNECT
            | generated_gateway::rpc::RECONNECT
            | generated_gateway::rpc::RESET_SESSION
    );
    let export_command = IDLE_RELEASE_EXPORT_METHODS.contains(&method);
    let released = clients.idle_entry(controller).is_some();
    let close_unacknowledged = clients
        .idle_entry(controller)
        .is_some_and(|entry| entry.close_unacknowledged);
    if released
        && authenticated
        && matches!(
            method,
            generated_gateway::rpc::RECONNECT | generated_gateway::rpc::RESET_SESSION
        )
        && !close_unacknowledged
        && !clients
            .idle_entry(controller)
            .is_some_and(|entry| entry.manually_released)
    {
        // The earlier preflight may have cleared the old entry before it
        // queued behind the close. Clear a marker published by that close
        // while this explicit session command waited on request_lock.
        clients.idle_released = false;
        clients.forget_idle_release(controller);
        shared.observer_changed.notify_all();
    }
    participant && released && !session_command && !export_command
}

/// The answer to every refused resource read, on TCP and on `--stdio`.
const RESOURCE_READ_NOT_OPEN: &str =
    "The Quad Cortex is not connected: a resource read never opens or reopens its session";

/// Whether a resource read may reach `method` at all: a device read, never a
/// broker-level method, a session command or a mutation.
fn resource_read_may_reach(method: &str) -> bool {
    generated_gateway::broker_dispatch(method).is_some()
        && !is_device_mutation(method)
        && !matches!(
            method,
            generated_gateway::rpc::RECONNECT
                | generated_gateway::rpc::RESET_SESSION
                | generated_gateway::rpc::DISCONNECT
        )
}

/// One request marked as an MCP resource read ([`RESOURCE_READ_ORIGIN`]): the
/// client reading context, not anyone asking for the device. It is served
/// only from a device session that is open when it is admitted, and nothing
/// it does opens, reopens or reconnects one:
/// - it never reaches `prepare_idle_released_target`, so it neither starts
///   nor joins an idle reopen;
/// - only a device read is admitted: no broker-level method, no session
///   command, no mutation;
/// - apart from `system.status`, which reports the session as it is, the
///   session is checked when the read is admitted. A read that takes
///   `request_lock` today is admitted under it: an idle release
///   (`release_idle_device`), an icon Disconnect, another client's disconnect
///   and a committed shutdown all close the session under that same lock, so
///   none can land between the check and the read. The cached reads that run
///   without `request_lock` (`runs_without_request_lock`) stay lock-free, as
///   for any other caller: once past `prepare_idle_released_target`, which
///   this path never calls, they only copy state the workers already hold and
///   cannot reach USB, so a close after the check costs nothing.
///
/// No lock is added: this is the admission `dispatch_broker_client_request`
/// already performs, minus the reopen. Everything refused gets the
/// not-connected error an idle reopen's failure gets: retryable,
/// `data.category` `notConnected`.
fn dispatch_resource_read(
    shared: &BrokerShared,
    connection_id: Option<u64>,
    controller: Arc<DeviceController>,
    request: Request,
) -> Value {
    const NOT_OPEN: &str = RESOURCE_READ_NOT_OPEN;
    let method = request.method.as_str();
    if !resource_read_may_reach(method) {
        return idle_reopen_error(request.id, NOT_OPEN);
    }
    if method == generated_gateway::rpc::RUNTIME_STATUS {
        return handle(&controller, &shared.performance_midi, request);
    }
    // Tests pause here: the client's status read is answered, its device read
    // is not yet admitted.
    shared.test_point("resource_read:before_admission");
    let _exclusive =
        (!runs_without_request_lock(method)).then(|| shared.request_lock.lock_recover());
    shared.test_point("resource_read:admitted");
    {
        let clients = shared.clients.lock_recover();
        if clients.shutting_down
            || connection_id.is_some_and(|id| !clients.connections.contains_key(&id))
            || clients.idle_entry(&controller).is_some()
        {
            return idle_reopen_error(request.id, NOT_OPEN);
        }
    }
    if !controller.status().connected {
        return idle_reopen_error(request.id, NOT_OPEN);
    }
    let expected = request.expected_broker_target.clone();
    if !shared.target_matches(expected.as_ref()) {
        return error(
            request.id,
            -32002,
            "Broker target changed; request was not sent",
        );
    }
    shared.fence_result(
        expected.as_ref(),
        handle(&controller, &shared.performance_midi, request),
    )
}

/// Commit while holding the accounting lock after waiting for device work.
/// Like disconnect, the cheap pre-check is not authoritative: another client
/// can attach while this waits for request_lock. Never hold clients across
/// that wait. The connection loop flushes this response before invoking exit.
#[cfg(test)]
fn request_broker_shutdown(shared: &BrokerShared, request: Request) -> Value {
    request_broker_shutdown_after_precheck(shared, request, None, || {})
}

/// The injected boundary lets the race regression prove a participant arrives
/// after the preliminary check, rather than relying on thread scheduling.
fn request_broker_shutdown_after_precheck(
    shared: &BrokerShared,
    request: Request,
    connection_id: Option<u64>,
    after_precheck: impl FnOnce(),
) -> Value {
    request_broker_shutdown_recording(shared, request, connection_id, after_precheck, None)
}

/// `request_broker_shutdown_after_precheck`, recording a user's Quit when
/// `user_quit` names one: the icon's Quit or `--control quit`, never an idle
/// exit, an installer stop or another client's shutdown. The record is
/// published under the same locks and after the same refusals as the forced
/// release marker, so it exists exactly when the shutdown commits; a failed
/// write refuses the shutdown and leaves the device open.
fn request_broker_shutdown_recording(
    shared: &BrokerShared,
    request: Request,
    connection_id: Option<u64>,
    after_precheck: impl FnOnce(),
    user_quit: Option<crate::user_quit::UserQuitSource>,
) -> Value {
    let force = match request.params.get("force") {
        None if request.params.is_object() => false,
        Some(Value::Bool(force)) => *force,
        _ => return error(request.id, -32602, "force must be a boolean"),
    };
    let refusal = |id| {
        error(
            id,
            -32002,
            "A participant is still attached; the shared session was left open",
        )
    };
    // This process's own MCP host dies with it, so it never refuses a
    // shutdown; every other participant does, unless forced.
    if !force && shared.clients.lock_recover().participants_other_than_host() != 0 {
        return refusal(request.id);
    }
    after_precheck();
    let _exclusive = shared.request_lock.lock_recover();
    let mut clients = shared.clients.lock_recover();
    if connection_id.is_some_and(|id| !clients.connections.contains_key(&id)) {
        return error(
            request.id,
            -32002,
            "Connection is no longer attached; queued shutdown was cancelled",
        );
    }
    if !force && clients.participants_other_than_host() != 0 {
        return refusal(request.id);
    }
    if clients.shutting_down {
        return error(request.id, -32002, "Broker shutdown is already committed");
    }
    // A reopen is mid-handshake: an unforced exit would cut it off and strand
    // the device's session. A forced release still goes ahead, and its
    // disconnect interrupts the handshake.
    if !force && clients.reopens_in_flight != 0 {
        return error(
            request.id,
            -32002,
            "The Quad Cortex is reopening; try again in a moment",
        );
    }
    if let Some(source) = user_quit {
        if let Err(message) = shared.publish_user_quit(source) {
            return error(request.id, -32002, &message);
        }
    }
    if force {
        // A congested participant may miss the bounded socket notification.
        // Publish this exact generation's release before committing shutdown,
        // so recovery can distinguish it from a crash even after EOF.
        if let Err(message) = shared.publish_forced_release() {
            // Not committed, so not a Quit: the record this call wrote is
            // withdrawn (compare-and-remove: never another generation's). A
            // withdrawal that fails is said, not hidden: the record may still
            // stand and block new hosting until a user action clears it.
            let message = match user_quit
                .map(|_| crate::user_quit::withdraw(&shared.descriptor_path, &shared.instance_id))
            {
                Some(Err(withdrawal)) => format!(
                    "{message}. {withdrawal}; until it is cleared, only Connect in a QC Remote \
                     window, reopening QC Remote or --control start brings the broker back"
                ),
                Some(Ok(_)) | None => message,
            };
            return error(request.id, -32002, &message);
        }
    }
    clients.shutting_down = true;
    drop(clients);
    shared.observer_changed.notify_all();
    let delivery = shared.disconnect_devices();
    let detail = match delivery {
        Ok(()) => "Device sessions closed; the broker is shutting down".to_owned(),
        Err(message) => format!("Broker shutdown committed; {message}"),
    };
    json!({"jsonrpc":"2.0","id":request.id,"result":{"shutdown":true,"detail":detail}})
}

/// One request from a control connection (`qc-device-broker.exe --control`).
/// Each command is the icon's own action, so automation and the menu can
/// never behave differently:
/// - status: [`BrokerShared::control_status`], from caches only;
/// - release: the icon's Disconnect, [`BrokerHandle::release_devices_staying`].
///   The broker and its clients stay attached; only USB is released;
/// - connect: the icon's Connect, [`BrokerHandle::reconnect`];
/// - `broker.shutdown {"force": true}`: the icon's forced Release and exit,
///   through the same committed shutdown every client's forced shutdown
///   takes. Unforced is refused: the control client always forces.
fn dispatch_control_request(
    shared: &Arc<BrokerShared>,
    connection_id: u64,
    request: Request,
) -> Value {
    if request.jsonrpc != "2.0" || request.id.as_u64().is_none_or(|id| id == 0) {
        return error(request.id, -32600, "Invalid JSON-RPC request");
    }
    if !CONTROL_METHODS.contains(&request.method.as_str()) {
        return error(
            request.id,
            -32601,
            "This method is not available to a control connection",
        );
    }
    let handle = BrokerHandle(Arc::clone(shared));
    let id = request.id.clone();
    let outcome = |result: Result<Value, String>| match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(message) => error(id.clone(), -32002, &message),
    };
    match request.method.as_str() {
        BROKER_CONTROL_STATUS_METHOD => outcome(Ok(shared.control_status())),
        "broker.trayMenu" => outcome(crate::tray::menu_bridge::read()),
        "broker.trayAction" => outcome(crate::tray::menu_bridge::dispatch(request.params)),
        BROKER_CONTROL_CONNECT_METHOD => outcome(
            handle
                .reconnect()
                .map(|detail| json!({"connecting": true, "detail": detail})),
        ),
        BROKER_CONTROL_RELEASE_METHOD => match handle.release_devices_staying() {
            Ok(detail) => outcome(Ok(json!({"released": true, "detail": detail}))),
            Err(message) => outcome(Err(message)),
        },
        _ => {
            if request.params != json!({"force": true}) {
                return error(
                    request.id,
                    -32602,
                    "A control connection's broker.shutdown takes exactly {\"force\": true}",
                );
            }
            // Only `--control quit --confirm` sends it: a user's Quit, recorded
            // whether or not the tray switch is on.
            request_broker_shutdown_recording(
                shared,
                request,
                Some(connection_id),
                || {},
                Some(crate::user_quit::UserQuitSource::Control),
            )
        }
    }
}

/// One accepted socket's whole life: authenticate, serve, detach. The
/// connection is registered as authenticated before the client is told so,
/// and is refused instead if a shutdown was committed while it authenticated.
fn authenticate_and_serve_tcp_client(
    mut stream: TcpStream,
    shared: &Arc<BrokerShared>,
    connection_id: u64,
    bearer_token: &str,
    instance_id: &str,
    phase_events: bool,
) {
    let Some(_owner) = connection_owner(&shared.clients, connection_id) else {
        return;
    };
    // Detach on every exit, a panic included: a connection thread that
    // unwound past a plain trailing call left its count and participant row
    // behind for good, so the idle grace never fired and disconnect and
    // shutdown stayed "blocked by another participant".
    let _detach = DetachOnDrop {
        shared,
        connection_id,
    };
    let _ = stream.set_nodelay(true);
    // The writer is registered in the same step that counts the connection,
    // before the reply: a shutdown committed once the client is told it is
    // authenticated must find a writer for its `broker.shuttingDown` notice,
    // or that client reads a bare EOF. The writer stays locked until the
    // reply is written, so the notice can only follow it, never split it.
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let output = Arc::new(Mutex::new(writer));
    let mut replying = None;
    let authenticated =
        authenticate_tcp_client_then(&mut stream, bearer_token, instance_id, |role| {
            replying = Some(output.lock_recover());
            shared.mark_client_authenticated_as(connection_id, role, Some(Arc::clone(&output)))
        })
        .is_ok();
    drop(replying);
    if authenticated {
        // Tests pause here: the client has its reply, nothing serves it yet.
        shared.test_point("tcp:authenticated");
        serve_registered_tcp_connection(stream, output, shared, connection_id, phase_events);
    }
}

/// Runs `detach_client` for one accepted connection when dropped.
struct DetachOnDrop<'a> {
    shared: &'a Arc<BrokerShared>,
    connection_id: u64,
}

impl Drop for DetachOnDrop<'_> {
    fn drop(&mut self) {
        self.shared.detach_client(self.connection_id);
    }
}

/// Serves one already-authenticated TCP client: an `mpsc` push thread
/// draining that connection's own `device.stateFrame` subscription (and,
/// with `phase_events`, `publish_phase_changes` reused unmodified against
/// this connection's own output), plus a request-reading loop on the calling
/// thread that dispatches every frame into the same `handle` function
/// `--stdio` uses. Mirrors `serve_stdio`'s own shape as closely as a
/// per-connection body can: one push thread and one phase thread inside a
/// `thread::scope`, with the request loop as the scope's "main" task. Each
/// request goes through `dispatch_broker_request`.
#[cfg(test)]
fn serve_broker_tcp_connection(
    stream: TcpStream,
    shared: &Arc<BrokerShared>,
    connection_id: u64,
    phase_events: bool,
) {
    let Ok(output_stream) = stream.try_clone() else {
        return;
    };
    serve_broker_tcp_connection_with_output(
        stream,
        shared,
        connection_id,
        phase_events,
        Arc::new(Mutex::new(output_stream)),
    );
}

#[cfg(test)]
fn serve_broker_tcp_connection_with_output(
    stream: TcpStream,
    shared: &Arc<BrokerShared>,
    connection_id: u64,
    phase_events: bool,
    output: Arc<Mutex<TcpStream>>,
) {
    // This entrypoint is reached only after authentication succeeds. In
    // production `authenticate_and_serve_tcp_client` has already marked the
    // connection before replying, so this repeat is an idempotent no-op; it
    // exists for callers (tests) that authenticate and serve directly. Its
    // refusal is ignored: an already-marked connection stays marked, and a
    // committed shutdown refuses every dispatch on its own.
    let _ = shared.mark_client_authenticated(connection_id);
    let Ok(input) = stream.try_clone() else {
        return;
    };
    if let Some(row) = shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&connection_id)
    {
        row.output = Some(Arc::clone(&output));
    }
    serve_registered_tcp_connection(input, output, shared, connection_id, phase_events);
}

/// The body of `serve_broker_tcp_connection`, for a connection whose `output`
/// is already registered on its row: `input` reads requests, every write
/// goes through `output`.
fn serve_registered_tcp_connection(
    mut input: TcpStream,
    output: Arc<Mutex<TcpStream>>,
    shared: &Arc<BrokerShared>,
    connection_id: u64,
    phase_events: bool,
) {
    let Some(_owner) = connection_owner(&shared.clients, connection_id) else {
        return;
    };
    let Ok(shutdown_socket) = input.try_clone() else {
        return;
    };
    // A control connection is nobody's session: no state frames for it.
    let control = {
        let clients = shared.clients.lock_recover();
        clients.is_control(connection_id) || clients.is_demo_provider(connection_id)
    };
    let events = Mutex::new(
        (!control).then(|| Arc::new(ConnectionSubscription::of(Arc::clone(&shared.controller)))),
    );
    let stop = AtomicBool::new(false);
    let observer_action = Arc::new(AtomicBool::new(false));
    thread::scope(|scope| {
        // Drop before the scope joins, including on unwind. This socket clone
        // never needs the writer mutex to wake blocked readers and writers.
        let _teardown = ConnectionTeardown {
            stop: &stop,
            socket: shutdown_socket,
        };
        let push_output = &output;
        let push_stop = &stop;
        let push_events = &events;
        let push = thread::Builder::new()
            .name("qc-broker-tcp-push".into())
            .spawn_scoped(scope, move || {
                let mut last_clients = None;
                while !push_stop.load(Ordering::Acquire) {
                    // Never hold the subscription-selection lock across a blocking
                    // receive: hello must be able to replace it immediately.
                    let subscription = push_events.lock_recover().clone();
                    let received = subscription.as_ref().map(|events| {
                        events.frames.lock_recover().recv_timeout(Duration::from_millis(200))
                    });
                    let Some(received) = received else {
                        // An observer still needs ownership changes while its
                        // request thread waits on a slow connect/disconnect.
                        // Reuse the push thread it already has; legacy clients
                        // never enter this branch or receive this notification.
                        if let Ok(mut output) = push_output.try_lock() {
                            let snapshot = {
                                let clients = shared.clients.lock_recover();
                                clients
                                    .is_observer(connection_id)
                                    .then(|| clients.authenticated_rows())
                            };
                            if let Some(snapshot) = snapshot {
                                if last_clients.as_ref() != Some(&snapshot) {
                                    let notification = json!({"jsonrpc":"2.0","method":"broker.clientsChanged","params":{"clients":snapshot}});
                                    let body = serde_json::to_vec(&notification).expect("JSON values serialize");
                                    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
                                    frame.extend_from_slice(&body);
                                    if write_tcp_frame_before_deadline(&mut output, &frame, Instant::now() + Duration::from_millis(200)).is_err() {
                                        return;
                                    }
                                    last_clients = Some(snapshot);
                                }
                            }
                        }
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    };
                    match received {
                        Ok(frame) => {
                            let mut params = serde_json::to_value(&frame).expect("state frame serializes");
                            // Labelled by the device this subscription belongs to, not
                            // by the current selection: a selection commits before its
                            // subscription is swapped, and a frame of the old device
                            // written in between must not carry the new device's id.
                            params["deviceId"] = json!(subscription.as_ref().and_then(|events| events.device.device_id()));
                            let notification = json!({"jsonrpc":"2.0","method":"device.stateFrame","params":params});
                            let Ok(mut output) = push_output.lock() else {
                                return;
                            };
                            let current_subscription = push_events.lock_recover().clone();
                            if !current_subscription.as_ref().zip(subscription.as_ref())
                                .is_some_and(|(current, received)| Arc::ptr_eq(current, received))
                            {
                                continue;
                            }
                            // Serialize the role check with hello's response: once an
                            // observer receives its acknowledgement no queued frame
                            // may leak through from its old participant subscription.
                            if shared.clients.lock_recover().is_observer(connection_id) {
                                continue;
                            }
                            if write_tcp_response(&mut output, &notification).is_err() {
                                let _ = output.shutdown(std::net::Shutdown::Both);
                                return;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            ;
        if let Err(error) = push {
            // Nothing else runs yet: the connection ends, and the guard in
            // `authenticate_and_serve_tcp_client` detaches it.
            eprintln!("Could not start a broker TCP push thread: {error}");
            return;
        }
        if phase_events && !control {
            let (output, stop) = (&output, &stop);
            let phase = thread::Builder::new()
                .name("qc-broker-tcp-phase".into())
                .spawn_scoped(scope, move || {
                    let mut published = None;
                    while !stop.load(Ordering::Acquire) {
                        let Ok(mut writer) = output.lock() else {
                            return;
                        };
                        let selected = shared.controller_for(Some(connection_id));
                        let status = selected.status();
                        let observed = (
                            selected.device_id(),
                            status.phase.clone(),
                            status.detail.clone(),
                            status.connected,
                            status.synchronized,
                        );
                        if published.as_ref() != Some(&observed) {
                            let mut notification = phase_notification(&status);
                            notification["params"]["deviceId"] = json!(selected.device_id());
                            if write_tcp_response(&mut writer, &notification).is_err() {
                                let _ = writer.shutdown(std::net::Shutdown::Both);
                                return;
                            }
                            published = Some(observed);
                        }
                        drop(writer);
                        thread::sleep(PHASE_EVENT_POLL);
                    }
                });
            if let Err(error) = phase {
                // The push thread is running: stop it, or the scope never ends.
                eprintln!("Could not start a broker TCP phase thread: {error}");
                stop.store(true, Ordering::Release);
                return;
            }
        }
        // The socket is read on its own thread so that `system.status`
        // (`answers_beside_running_request`) is answered the moment it
        // arrives, even while this connection's own backup has been running
        // for half a minute. That is the only thing the reader serves itself:
        // it is the one method that can neither wait for the request lock nor
        // start a USB reopen. Every other request is admitted to a queue of at
        // most `CONNECTION_QUEUE_DEPTH`, and this thread runs the queue one
        // request at a time, in arrival order, exactly as the inline loop did:
        // the order of writes, their serialization under `request_lock` and
        // every refusal are unchanged. A client that waits for each reply
        // before sending the next one (every host here) finds the queue empty
        // whenever it asks, so for it nothing changes at all.
        //
        // A full queue is refused, not waited on: blocking the reader would
        // stop it answering `system.status` too, which is the point of it.
        let (queued_tx, queued) = mpsc::sync_channel::<Request>(CONNECTION_QUEUE_DEPTH);
        let reader_output = &output;
        let reader_events = &events;
        let reader = thread::Builder::new()
            .name("qc-broker-tcp-reader".into())
            .spawn_scoped(scope, move || {
                while let Ok(Some(mut request)) = read_request(&mut input) {
                    if shared.capture_request_target(Some(connection_id), &mut request) {
                        let _writer = reader_output.lock_recover();
                        *reader_events.lock_recover() = Some(Arc::new(ConnectionSubscription::of(
                            shared.controller_for(Some(connection_id)),
                        )));
                    }
                    let response = if answers_beside_running_request(&request.method) {
                        dispatch_broker_client_request(shared, Some(connection_id), request)
                    } else {
                        match queued_tx.try_send(request) {
                            Ok(()) => continue,
                            Err(mpsc::TrySendError::Full(request)) => {
                                connection_queue_full(request.id)
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        }
                    };
                    let mut writer = reader_output.lock_recover();
                    if write_tcp_response(&mut writer, &response).is_err() {
                        let _ = writer.shutdown(std::net::Shutdown::Both);
                        break;
                    }
                }
                {
                    let mut clients = shared.clients.lock_recover();
                    if let Some(row) = clients.connections.get_mut(&connection_id) {
                        row.demo_provider_closed = true;
                    }
                }
                if shared.demo.withdraw(connection_id) {
                    shared.refresh_targets();
                }
                // Dropping `queued_tx` here lets the queue finish what was
                // already admitted, then end, as an EOF always did.
            });
        if let Err(error) = reader {
            eprintln!("Could not start a broker TCP reader thread: {error}");
            stop.store(true, Ordering::Release);
            return;
        }
        // Set only when the reader has finished and the queue is drained.
        let mut reader_finished = false;
        loop {
            let Ok(request) = queued.recv() else {
                reader_finished = true;
                break;
            };
            if shared.clients.lock_recover().is_control(connection_id) {
                // One request at a time, inline: a control client sends one
                // command per connection and waits for its answer.
                let is_shutdown = request.method == "broker.shutdown";
                let response = dispatch_control_request(shared, connection_id, request);
                if is_shutdown && response.get("error").is_none() {
                    // Exactly a client's forced `broker.shutdown` below.
                    shared.notify_shutdown(connection_id, true);
                    shared.finish_shutdown_after_response(&output, &response);
                    break;
                }
                if write_tcp_response(&mut output.lock_recover(), &response).is_err() {
                    break;
                }
                continue;
            }
            let control_request = matches!(
                request.method.as_str(),
                "broker.hello"
                    | "broker.clients"
                    | "broker.devices"
                    | "broker.selectDevice"
                    | BROKER_CLIENT_COUNT_METHOD
            ) || runs_without_request_lock(&request.method);
            if !control_request
                && (observer_action.load(Ordering::Acquire)
                    || shared.clients.lock_recover().is_observer(connection_id))
            {
                // An observer must update its declared policy on this same
                // socket while reconnect waits for the device. Keep exactly
                // one operation in flight; legacy participants stay inline.
                if observer_action.swap(true, Ordering::AcqRel) {
                    let response = error(
                        request.id,
                        -32002,
                        "An observer device action is already in progress",
                    );
                    if write_tcp_response(&mut output.lock_recover(), &response).is_err() {
                        break;
                    }
                    continue;
                }
                let action_shared = Arc::clone(shared);
                let action_output = Arc::clone(&output);
                let action_pending = Arc::clone(&observer_action);
                let request_id = request.id.clone();
                let retired_output = Arc::clone(&output);
                let retired_pending = Arc::clone(&observer_action);
                let action_owner = Arc::clone(&_owner);
                let action_entered = Arc::new(AtomicBool::new(false));
                let action_answered = Arc::new(AtomicBool::new(false));
                let retired_entered = Arc::clone(&action_entered);
                let retired_answered = Arc::clone(&action_answered);
                let action_is_shutdown = request.method == "broker.shutdown";
                let action_forced = request.params.get("force") == Some(&Value::Bool(true));
                // EOF removes the visible observer immediately, but the bounded
                // owner retains its exact original and connection admission.
                // A separate instance of the existing manager avoids blocking
                // the policy/reopen pool with requests that wait on that pool.
                let spawned = shared.observer_actions.schedule(
                    [6, connection_id, 0],
                    Instant::now(),
                    background::Work::new(move |_| {
                        action_entered.store(true, Ordering::Release);
                        let response = dispatch_broker_client_request(
                            &action_shared,
                            Some(connection_id),
                            request,
                        );
                        if action_is_shutdown && response.get("error").is_none() {
                            action_shared.notify_shutdown(connection_id, action_forced);
                            action_shared.finish_shutdown_after_response(&action_output, &response);
                            let _ = action_output
                                .lock_recover()
                                .shutdown(std::net::Shutdown::Both);
                            action_pending.store(false, Ordering::Release);
                            action_answered.store(true, Ordering::Release);
                            return;
                        }
                        write_observer_action_response_with(
                            &action_output,
                            &action_pending,
                            &response,
                            write_tcp_response,
                            |writer| {
                                let _ = writer.shutdown(std::net::Shutdown::Both);
                            },
                        );
                        action_answered.store(true, Ordering::Release);
                    })
                    .on_retired(move |_joined_without_panic| {
                        if !retired_entered.load(Ordering::Acquire) {
                            let response = error(
                                request_id,
                                -32002,
                                "Observer device action was cancelled before dispatch",
                            );
                            write_observer_action_response_with(
                                &retired_output,
                                &retired_pending,
                                &response,
                                write_tcp_response,
                                |writer| {
                                    let _ = writer.shutdown(std::net::Shutdown::Both);
                                },
                            );
                        } else if !retired_answered.load(Ordering::Acquire) {
                            // An admitted request may already have affected the
                            // device. Transport loss is ambiguous, never a false
                            // pre-dispatch refusal or a second framed response.
                            let _ = retired_output
                                .lock_recover()
                                .shutdown(std::net::Shutdown::Both);
                            retired_pending.store(false, Ordering::Release);
                        }
                        // This callback runs after original signal/join, not
                        // merely when its request body or thread-local scope ends.
                        drop(action_owner);
                    }),
                );
                if let Err(spawn_error) = spawned {
                    // Failed admission also retires never-started Work and
                    // sends its one refusal through the callback above.
                    eprintln!("Could not admit observer device action: {spawn_error}");
                }
                continue;
            }
            // Only hello holds output while dispatching: it never waits for
            // `request_lock`. Device operations may take minutes and must not
            // starve phase/state notifications, and neither may a selection,
            // which waits for `request_lock` behind another client's backup:
            // holding output across that wait stalled this connection's own
            // `system.status` replies until the host declared the broker
            // unresponsive. A selection takes output once it has committed, and
            // swaps its subscription under it, so the reply still precedes every
            // frame of the newly selected device and follows every frame of the
            // old one.
            let is_selection = request.method == "broker.selectDevice";
            let swaps_subscription = request.method == "broker.hello" || is_selection;
            let is_shutdown = request.method == "broker.shutdown";
            let forced = request.params.get("force") == Some(&Value::Bool(true));
            let hello_output = if holds_output_while_dispatching(&request.method) {
                output.lock().ok()
            } else {
                None
            };
            let response = dispatch_broker_client_request(shared, Some(connection_id), request);
            let hello_output = if is_selection && response.get("error").is_none() {
                output.lock().ok()
            } else {
                hello_output
            };
            if swaps_subscription && response.get("error").is_none() {
                let mut events = events.lock_recover();
                if is_selection {
                    *events = None;
                }
                if shared.clients.lock_recover().is_observer(connection_id) {
                    *events = None;
                } else {
                    // Subscribe before acknowledging a promotion; the first
                    // frame after hello succeeds must already have a receiver.
                    events.get_or_insert_with(|| {
                        Arc::new(ConnectionSubscription::of(
                            shared.controller_for(Some(connection_id)),
                        ))
                    });
                }
                drop(events);
                shared.update_observer_backstop();
            }
            let shutdown_committed = is_shutdown && response.get("error").is_none();
            if shutdown_committed {
                shared.notify_shutdown(connection_id, forced);
                shared.finish_shutdown_after_response(&output, &response);
                break;
            }
            let mut output_guard = match hello_output {
                Some(output) => output,
                None => match output.lock() {
                    Ok(output) => output,
                    Err(_) => break,
                },
            };
            if write_tcp_response(&mut output_guard, &response).is_err() {
                break;
            }
        }
        // A committed shutdown or a failed write ended the queue while the
        // reader is still blocked on the socket. The inline loop simply
        // returned; here the scope cannot end until the reader does, so close
        // the socket without needing its writer mutex. The teardown guard also
        // retires the connection on EOF; admitted observer effects retain their
        // own operation ownership, but no longer retain a usable reply socket.
        if !reader_finished {
            let _ = _teardown.socket.shutdown(std::net::Shutdown::Both);
        }
        stop.store(true, Ordering::Release);
    });
}

struct ConnectionTeardown<'a> {
    stop: &'a AtomicBool,
    socket: TcpStream,
}

impl Drop for ConnectionTeardown<'_> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

/// Whether a connection holds its output lock across this request's dispatch.
/// Only `broker.hello`, which never waits for `request_lock`; see the request
/// loop in `serve_broker_tcp_connection`.
fn holds_output_while_dispatching(method: &str) -> bool {
    method == "broker.hello"
}

/// One connection's state-frame subscription together with the device it was
/// taken from, so every frame is labelled with the device that produced it.
struct ConnectionSubscription {
    device: Arc<DeviceController>,
    frames: Mutex<crate::worker::StateSubscription>,
}

impl ConnectionSubscription {
    fn of(device: Arc<DeviceController>) -> Self {
        let frames = Mutex::new(device.subscribe_state_events());
        Self { device, frames }
    }
}

#[derive(Default)]
struct ConnectionInfo {
    admission: Arc<()>,
    selected_device: Option<Arc<DeviceController>>,
    implicit_selection: bool,
    provider: String,
    role: ConnectionRole,
    authenticated: bool,
    /// A live Never observer renews this lease with hello; a hung tray still expires.
    keep_connected_since: Option<Instant>,
    output: Option<Arc<Mutex<TcpStream>>>,
    /// This connection presented this process's host token in its latest
    /// `broker.hello`: it is the in-process background MCP host. Every hello
    /// recomputes it, so a re-hello without the token loses it, and a
    /// provider name alone never grants it.
    in_process_host: bool,
    /// Authenticated with the [`CONTROL_ROLE`]: neither a participant nor an
    /// observer, for the whole life of the connection (a control connection
    /// cannot send `broker.hello`).
    control: bool,
    demo_provider: bool,
    demo_provider_closed: bool,
}

impl ConnectionInfo {
    /// An authenticated connection in the participant role. Every row starts
    /// with the default participant role on accept, so the role alone does
    /// not distinguish a participant from a socket still authenticating.
    /// A control connection is never one.
    fn is_participant(&self) -> bool {
        self.authenticated
            && !self.control
            && !self.demo_provider
            && matches!(self.role, ConnectionRole::Participant)
    }
}

fn connection_hello(params: &Value) -> Result<ConnectionInfo, &'static str> {
    if !params.is_object() {
        return Err("broker.hello params must be an object");
    }
    let provider = match params.get("provider") {
        None => "",
        Some(Value::String(value)) => value,
        _ => return Err("provider must be a string"),
    };
    if provider.len() > BROKER_PROVIDER_MAX_UTF8_BYTES {
        return Err("provider must not exceed 4096 UTF-8 bytes");
    }
    let provider = provider.to_string();
    let minutes = match params.get("idlePolicyMinutes") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|value| (1..=1440).contains(value))
                .ok_or("idlePolicyMinutes must be an integer between 1 and 1440, or null")?
                as u32,
        ),
    };
    let role = match params.get("role") {
        None => ConnectionRole::Participant,
        Some(Value::String(role)) => ConnectionRole::from_wire(role, minutes)
            .ok_or("role must be participant or observer")?,
        _ => return Err("role must be participant or observer"),
    };
    let keep_connected = match params.get("keepConnected") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        _ => return Err("keepConnected must be a boolean"),
    };
    if keep_connected
        && !matches!(
            role,
            ConnectionRole::Observer {
                idle_policy_minutes: None
            }
        )
    {
        return Err("keepConnected requires an observer without an idle period");
    }
    Ok(ConnectionInfo {
        admission: Arc::new(()),
        selected_device: None,
        implicit_selection: false,
        keep_connected_since: keep_connected.then(Instant::now),
        provider,
        role,
        // broker.hello is available only on an authenticated connection.
        authenticated: true,
        output: None,
        in_process_host: match params.get("hostToken") {
            Some(Value::String(token)) => crate::resident::is_host_token(token),
            _ => false,
        },
        // A control connection never reaches hello (`CONTROL_METHODS`).
        control: false,
        demo_provider: false,
        demo_provider_closed: false,
    })
}

/// `client_count` and the shutdown decision it gates used to be a bare
/// `AtomicUsize`: the accept loop's `fetch_add` and the grace thread's
/// load-then-act were two independent atomic operations with a real gap
/// between them, wide enough (a `release_descriptor_slot` filesystem call
/// sits inside it) that a client connecting at almost exactly the moment the
/// last client's grace period expired could be accepted and counted, and
/// then have the broker exit under it anyway - the grace thread's stale
/// "zero" read never saw the new arrival. Combining the count and a
/// shutting-down flag under one lock makes "accept and count a new client"
/// and "confirm zero and commit to shutting down" mutually exclusive: the
/// accept loop refuses a connection that arrives after shutdown has been
/// committed to, rather than the reverse.
#[derive(Default)]
struct ClientAccounting {
    /// Raw accepts, including connections still authenticating. Layer 1 must
    /// continue to use this count, never the participant count.
    count: usize,
    shutting_down: bool,
    /// New participants cannot join this device between the final sharing
    /// check and the worker's disconnect acknowledgment.
    closing_devices: Vec<Arc<DeviceController>>,
    next_connection_id: u64,
    connections: BTreeMap<u64, ConnectionInfo>,
    // Bounded weak inventory under this mutex; rows and original jobs own
    // the strong leases. Detached but unfinished jobs still consume slots.
    connection_owners: Vec<std::sync::Weak<()>>,
    observer_idle_since: Option<Instant>,
    /// Prevents an expired observer/tray deadline from retrying native
    /// teardown before its low-rate idle retry is due.
    idle_release_retry_not_before: Option<Instant>,
    observer_timer_running: bool,
    // Exact observer owner; retiring an old original cannot clear a queued
    // replacement's flag. This marker is bounded by manager/job ownership.
    observer_owner: Option<Arc<()>>,
    /// The broker's own icon hold: `Some` only while this process's
    /// notification icon is registered with the shell. `None` is the shipped
    /// broker, reaped by the five-second grace once no connection remains.
    tray_hold: Option<IdlePolicy>,
    /// When participants last reached zero under a tray hold (or when the icon
    /// appeared, if none were attached then).
    tray_idle_since: Option<Instant>,
    /// This process is the session's resident host: its tray switch is on and
    /// its icon thread is alive (see `crate::resident`). A resident broker
    /// answers a reap by closing the device session and staying published,
    /// because the same process also serves the background MCP host. A broker
    /// that is not resident reaps exactly as it always did: it exits.
    resident: bool,
    /// A resident reap closed every device session and nothing has reached a
    /// device since. While set there is nothing left to release, so no reap
    /// timer runs. Cleared by any request that reaches a device.
    idle_released: bool,
    /// The devices that resident reap closed, by controller. Each is reopened
    /// lazily, by the first request routed to it (its connection's selected
    /// device, else the default), and only then: a device nobody asks for
    /// again stays closed, and no request is ever routed to another device
    /// in its place.
    idle_released_devices: Vec<IdleReopenEntry>,
    /// Reopen threads still running a reconnect or waiting for its session.
    /// No grace, reap or installer stop commits a shutdown while one runs, so
    /// a handshake is never cut off by this process's own exit.
    reopens_in_flight: usize,
    /// A grace expired, or a non-control client left, while some connection
    /// was still counted. Ordinarily that connection's own departure re-arms
    /// the grace; a control connection's departure re-arms it only when this
    /// is set, so a status read can never start a grace (and with it an idle
    /// release or exit) that nothing else had pending.
    reap_deferred: bool,
}

impl ClientAccounting {
    fn device_is_closing(&self, device: &Arc<DeviceController>) -> bool {
        self.closing_devices
            .iter()
            .any(|closing| Arc::ptr_eq(closing, device))
    }
    fn is_control(&self, id: u64) -> bool {
        self.connections.get(&id).is_some_and(|row| row.control)
    }

    fn is_demo_provider(&self, id: u64) -> bool {
        self.connections
            .get(&id)
            .is_some_and(|row| row.authenticated && row.demo_provider)
    }

    fn is_observer(&self, id: u64) -> bool {
        self.connections
            .get(&id)
            .is_some_and(|row| matches!(row.role, ConnectionRole::Observer { .. }))
    }

    /// Observers other than `caller`, so a hello that renews an observer's
    /// own policy never counts against the cap.
    fn observers_other_than(&self, caller: Option<u64>) -> usize {
        self.connections
            .iter()
            .filter(|(id, row)| {
                Some(**id) != caller && matches!(row.role, ConnectionRole::Observer { .. })
            })
            .count()
    }

    fn observers(&self) -> usize {
        self.connections
            .values()
            .filter(|row| matches!(row.role, ConnectionRole::Observer { .. }))
            .count()
    }

    /// Authenticated participants only. A socket still authenticating is
    /// counted by `count` (so Layer 1 idle shutdown cannot race it) and by
    /// `pending_authentications`, but it is nobody's participant: it cannot
    /// block a disconnect or shutdown, and is not listed to observers.
    fn participants(&self) -> usize {
        self.connections
            .values()
            .filter(|row| row.is_participant())
            .count()
    }

    /// What `broker.clients` and `broker.clientsChanged` report: every
    /// authenticated connection, never a socket still authenticating, and
    /// never a control connection (it is nobody's client).
    fn authenticated_rows(&self) -> Vec<Value> {
        self.connections
            .values()
            .filter(|row| row.authenticated && !row.control && !row.demo_provider)
            .map(|row| json!({"provider":row.provider,"role":row.role.wire_name()}))
            .collect()
    }

    /// Accepted sockets that have not (yet) authenticated.
    fn pending_authentications(&self) -> usize {
        self.connections
            .values()
            .filter(|row| !row.authenticated)
            .count()
    }

    /// Participants other than this process's own background MCP host. The
    /// host is a participant only while it serves an AI client, and it lives
    /// and dies with this process, so the icon's Disconnect and an
    /// installer's stop decide on everyone else.
    fn participants_other_than_host(&self) -> usize {
        self.connections
            .values()
            .filter(|row| row.is_participant() && !row.in_process_host)
            .count()
    }

    fn idle_entry(&self, controller: &Arc<DeviceController>) -> Option<&IdleReopenEntry> {
        self.idle_released_devices
            .iter()
            .find(|entry| Arc::ptr_eq(&entry.controller, controller))
    }

    fn idle_entry_mut(
        &mut self,
        controller: &Arc<DeviceController>,
    ) -> Option<&mut IdleReopenEntry> {
        self.idle_released_devices
            .iter_mut()
            .find(|entry| Arc::ptr_eq(&entry.controller, controller))
    }

    fn idle_reopen_state(&self, controller: &Arc<DeviceController>) -> Option<IdleReopen> {
        self.idle_entry(controller).map(IdleReopenEntry::view)
    }

    fn forget_idle_release(&mut self, controller: &Arc<DeviceController>) {
        self.idle_released_devices
            .retain(|entry| !Arc::ptr_eq(&entry.controller, controller));
    }

    /// Record controllers whose HID close commands were accepted for idleness.
    /// Call with `request_lock` held after MIDI retirement and command
    /// admission, so no reopen can reconnect one in between (its reconnect
    /// also needs `request_lock`). A missing HID acknowledgement does not
    /// prove the queued close failed; the worker may finish it later.
    /// `idleReleased` therefore means the close command was accepted, not
    /// acknowledged.
    fn record_idle_release(
        &mut self,
        closed: Vec<Arc<DeviceController>>,
        close_unacknowledged: Vec<Arc<DeviceController>>,
    ) {
        self.record_release(closed, close_unacknowledged, false);
    }

    fn record_release(
        &mut self,
        closed: Vec<Arc<DeviceController>>,
        close_unacknowledged: Vec<Arc<DeviceController>>,
        manually_released: bool,
    ) {
        self.idle_released = true;
        let previous = std::mem::take(&mut self.idle_released_devices);
        // A partial later release must not erase an earlier controller's
        // accepted close marker. Keep entries not part of this attempt, then
        // refresh the controllers that were accepted now.
        let mut updated: Vec<_> = previous
            .iter()
            .filter(|entry| {
                // Keep a prior marker even when cached status says connected:
                // an accepted close may still be queued and finish later.
                // The next lazy reopen uses an unforced reconnect, which
                // leaves a healthy self-reconnected worker alone, then clears
                // the marker only after readiness is verified.
                !closed
                    .iter()
                    .any(|controller| Arc::ptr_eq(&entry.controller, controller))
            })
            .map(|entry| IdleReopenEntry {
                controller: Arc::clone(&entry.controller),
                manually_released: entry.manually_released,
                close_unacknowledged: entry.close_unacknowledged,
                reopening: entry.reopening,
                attempt: entry.attempt,
                last_failure: entry.last_failure.clone(),
            })
            .collect();
        updated.extend(closed.into_iter().map(|controller| {
            let close_unacknowledged = close_unacknowledged
                .iter()
                .any(|pending| Arc::ptr_eq(pending, &controller));
            // A device recorded again keeps its attempt number and its last
            // failure, so a waiter on an earlier attempt still gets it.
            match previous
                .iter()
                .find(|entry| Arc::ptr_eq(&entry.controller, &controller))
            {
                Some(entry) => IdleReopenEntry {
                    controller,
                    manually_released: manually_released || entry.manually_released,
                    close_unacknowledged,
                    reopening: false,
                    attempt: entry.attempt,
                    last_failure: entry.last_failure.clone(),
                },
                None => {
                    let mut entry = IdleReopenEntry::released_with_pending_close(
                        controller,
                        close_unacknowledged,
                    );
                    entry.manually_released = manually_released;
                    entry
                }
            }
        }));
        self.idle_released_devices = updated;
    }

    /// Whether the icon's policy keeps the device past the shipped grace.
    /// Immediately is the grace itself, so it holds nothing.
    fn tray_holds_past_grace(&self) -> bool {
        matches!(
            self.tray_hold,
            Some(IdlePolicy::Minutes(_) | IdlePolicy::Never)
        )
    }

    fn disconnect_blocked(&self, caller: Option<u64>) -> bool {
        // An observer must not disconnect even one participant. Participants
        // retain the existing protection for everyone other than themselves.
        let caller_is_participant = caller.is_none_or(|id| {
            self.connections
                .get(&id)
                .is_some_and(ConnectionInfo::is_participant)
        });
        self.participants() > usize::from(caller_is_participant)
    }
}

/// The accept loop's half of the mutual exclusion `ClientAccounting`
/// documents: locked, checked, and incremented as one step so this and a
/// grace thread's shutdown decision can never interleave. A connection
/// arriving after a grace thread has committed to shutting down is refused
/// here (returns `false`) rather than accepted and then torn down anyway
/// once the process exits. A successful accept (`true`) is counted before
/// the caller ever spawns the connection's own thread - a window after
/// `accept()` returns but before that thread starts must never let a grace
/// thread from a *previous* disconnect observe zero clients.
#[cfg(test)]
fn accept_or_refuse_client(clients: &Mutex<ClientAccounting>) -> bool {
    accept_client(clients).is_some()
}

/// Also refuses (returns `None`) while `BROKER_MAX_PENDING_AUTHENTICATIONS`
/// accepted sockets are still authenticating, so unauthenticated connections
/// cannot pin an unbounded number of threads; the caller closes the socket.
fn accept_client(clients: &Mutex<ClientAccounting>) -> Option<u64> {
    let mut clients = clients.lock_recover();
    clients
        .connection_owners
        .retain(|owner| owner.strong_count() != 0);
    if clients.shutting_down
        || clients.pending_authentications() >= BROKER_MAX_PENDING_AUTHENTICATIONS
        || clients.connection_owners.len() >= BROKER_MAX_CONNECTION_OWNERS
    {
        return None;
    }
    clients.count += 1;
    clients.next_connection_id += 1;
    let id = clients.next_connection_id;
    let row = ConnectionInfo::default();
    clients
        .connection_owners
        .push(Arc::downgrade(&row.admission));
    clients.connections.insert(id, row);
    Some(id)
}

fn connection_owner(clients: &Mutex<ClientAccounting>, id: u64) -> Option<Arc<()>> {
    clients
        .lock_recover()
        .connections
        .get(&id)
        .map(|row| Arc::clone(&row.admission))
}

/// What every connection thread of one broker shares.
/// Lock order, for every path in this file: `request_lock`, then `clients`.
/// `devices` is a leaf: it is taken alone, or under `request_lock`, and is
/// never held while `clients` is acquired (`broker.devices` reads the
/// caller's selection before it takes `devices`; the idle release and the
/// icon's Disconnect copy `devices` before they take `clients`). The per-device
/// worker locks are taken only by `DeviceController` methods, never while
/// `clients` is held. Nothing blocking (a handshake, a socket write without a
/// deadline) runs under `clients`; under `request_lock` only bounded work
/// runs.
struct BrokerShared {
    controller: Arc<DeviceController>,
    devices: Mutex<Vec<Arc<DeviceController>>>,
    performance_midi: Arc<PerformanceMidiWorkers>,
    /// Serialises `handle` across clients; see `runs_without_request_lock`.
    request_lock: Arc<Mutex<()>>,
    /// Connections accepted and not yet closed, authenticated or not.
    clients: Arc<Mutex<ClientAccounting>>,
    descriptor_path: PathBuf,
    instance_id: String,
    descriptor_claim: Arc<Mutex<Option<qc_owner_only_file::ClaimedFile>>>,
    on_idle_shutdown: Arc<dyn Fn() + Send + Sync>,
    observer_changed: Condvar,
    background: background::Manager,
    observer_actions: background::Manager,
    demo: crate::demo_provider::Hub,
    targets: crate::target::TargetRegistry,
    /// Duration of a policy minute, injected only by in-module tests.
    observer_minute: Duration,
}

struct DisconnectAdmission {
    clients: Arc<Mutex<ClientAccounting>>,
    device: Arc<DeviceController>,
}

/// Internal evidence only. Never serialize or log the physical key.
struct LifecycleProof {
    controller: Arc<DeviceController>,
    physical_key: String,
    from: crate::target::TargetIdentity,
    method: String,
}

impl Drop for DisconnectAdmission {
    fn drop(&mut self) {
        self.clients
            .lock_recover()
            .closing_devices
            .retain(|device| !Arc::ptr_eq(device, &self.device));
    }
}

struct DisconnectDevicesReport {
    midi_retired: bool,
    attempted: usize,
    queued: Vec<Arc<DeviceController>>,
    close_unacknowledged: Vec<Arc<DeviceController>>,
    failures: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetSelectionError {
    ReadinessBusy,
    InventoryBusy,
    Ambiguous,
}

impl TargetSelectionError {
    fn message(self) -> &'static str {
        match self {
            Self::ReadinessBusy => "Broker target readiness is temporarily unavailable",
            Self::InventoryBusy => "Broker target inventory is temporarily unavailable",
            Self::Ambiguous => {
                "Multiple usable Quad Cortex devices require an explicit canonical selection"
            }
        }
    }

    fn is_unknown(self) -> bool {
        matches!(self, Self::ReadinessBusy | Self::InventoryBusy)
    }
}

impl BrokerShared {
    fn capture_request_target(&self, connection_id: Option<u64>, request: &mut Request) -> bool {
        if request.physical_only
            || request.method == "system.status"
            || generated_gateway::broker_dispatch(&request.method).is_none()
        {
            return false;
        }
        let Ok(physical) = self.selected_target() else {
            return false;
        };
        if request
            .expected_broker_target
            .as_ref()
            .is_some_and(|expected| self.targets.identity().as_ref() != Some(expected))
        {
            return false;
        }
        let mut changed = false;
        if let (Some(connection), Some(physical)) = (connection_id, physical) {
            let mut clients = self.clients.lock_recover();
            if !clients.device_is_closing(&physical) {
                if let Some(row) = clients.connections.get_mut(&connection) {
                    if row.is_participant()
                        && (row.selected_device.is_none() || row.implicit_selection)
                    {
                        changed = !Arc::ptr_eq(
                            row.selected_device.as_ref().unwrap_or(&self.controller),
                            &physical,
                        );
                        row.selected_device = Some(physical);
                        row.implicit_selection = true;
                    }
                }
            }
        }
        if let Some(target) = self.targets.identity() {
            request.expected_broker_target = Some(target);
        } else {
            // Preserve existing physical open/reopen behavior while ensuring
            // that a demo window appearing later cannot take this queued call.
            request.physical_only = true;
        }
        changed
    }

    fn target_matches(&self, expected: Option<&crate::target::TargetIdentity>) -> bool {
        expected.is_none()
            || (self.selected_target().is_ok()
                && expected
                    .is_none_or(|expected| self.targets.identity().as_ref() == Some(expected)))
    }

    fn fence_result(
        &self,
        expected: Option<&crate::target::TargetIdentity>,
        mut response: Value,
    ) -> Value {
        if self.target_matches(expected) {
            if let Some(target) = expected.filter(|_| response.get("error").is_none()) {
                response["brokerTarget"] = json!(target);
                if let Some(result) = response
                    .get_mut("result")
                    .filter(|result| result.is_object())
                {
                    result["brokerTarget"] = json!(target);
                    result["targetReady"] = json!(true);
                }
            }
            return response;
        }
        json!({"jsonrpc":"2.0","id":response["id"],"error":{
            "code":-32002,"message":"Broker target changed while the request ran; result discarded. Do not replay this request on another target.",
            "data":{"code":"BROKER_TARGET_SUPERSEDED","requestSent":true,"retryable":false}
        }})
    }

    fn capture_lifecycle(
        &self,
        controller: &Arc<DeviceController>,
        method: &str,
        from: &crate::target::TargetIdentity,
    ) -> Option<LifecycleProof> {
        if !matches!(
            method,
            generated_gateway::rpc::RECONNECT
                | generated_gateway::rpc::RESET_SESSION
                | generated_gateway::rpc::DISCONNECT
        ) {
            return None;
        }
        if from.source != crate::target::TargetSource::Physical || !self.target_matches(Some(from))
        {
            return None;
        }
        let physical_key = controller
            .device_id()
            .filter(|key| !key.trim().is_empty())?;
        let canonical = self.selected_target().ok()??;
        if !Arc::ptr_eq(controller, &canonical) {
            return None;
        }
        Some(LifecycleProof {
            controller: controller.clone(),
            physical_key,
            from: from.clone(),
            method: method.into(),
        })
    }

    fn lifecycle_transition(&self, proof: &LifecycleProof) -> Option<Value> {
        if proof.controller.device_id().as_deref() != Some(proof.physical_key.as_str()) {
            return None;
        }
        let (connected, synchronized, _) = proof.controller.try_target_readiness()?;
        let canonical = self.selected_target().ok()?;
        let to = self.targets.identity();
        let verification = match proof.method.as_str() {
            generated_gateway::rpc::RECONNECT | generated_gateway::rpc::RESET_SESSION
                if connected
                    && synchronized
                    && canonical
                        .as_ref()
                        .is_some_and(|controller| Arc::ptr_eq(controller, &proof.controller))
                    && to.as_ref().is_some_and(|target| {
                        target.source == crate::target::TargetSource::Physical
                    }) =>
            {
                "same_native_controller"
            }
            generated_gateway::rpc::DISCONNECT
                if !connected
                    && !synchronized
                    && canonical.is_none()
                    && to.as_ref().is_none_or(|target| {
                        target.source == crate::target::TargetSource::Demo
                    }) =>
            {
                "closed_native_controller"
            }
            _ => return None,
        };
        Some(json!({"method":proof.method,"from":proof.from,"to":to,"verification":verification}))
    }

    fn finish_lifecycle(
        &self,
        proof: Option<&LifecycleProof>,
        expected: Option<&crate::target::TargetIdentity>,
        mut response: Value,
    ) -> Value {
        if response.get("error").is_some() || self.target_matches(expected) {
            return self.fence_result(expected, response);
        }
        if let Some(transition) = proof.and_then(|proof| self.lifecycle_transition(proof)) {
            if let Some(result) = response
                .get_mut("result")
                .filter(|result| result.is_object())
            {
                // The acknowledgment proves completion on the OLD controller;
                // it grants no permission to replay a request on the new one.
                result["brokerTargetTransition"] = transition;
                return response;
            }
        }
        self.fence_result(expected, response)
    }

    fn refresh_targets(&self) {
        let _ = self.selected_target();
    }

    fn canonical_physical(
        &self,
    ) -> Result<Option<(Arc<DeviceController>, u64)>, TargetSelectionError> {
        let (connected, synchronized, epoch) = self
            .controller
            .try_target_readiness()
            .ok_or(TargetSelectionError::ReadinessBusy)?;
        if connected && synchronized {
            return Ok(Some((self.controller.clone(), epoch)));
        }
        let known = match self.devices.try_lock() {
            Ok(known) => known.clone(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(TargetSelectionError::InventoryBusy)
            }
        };
        let mut ready = None;
        for controller in known {
            let (connected, synchronized, epoch) = controller
                .try_target_readiness()
                .ok_or(TargetSelectionError::ReadinessBusy)?;
            if connected && synchronized {
                if ready.is_some() {
                    return Err(TargetSelectionError::Ambiguous);
                }
                ready = Some((controller, epoch));
            }
        }
        Ok(ready)
    }

    fn selected_target(&self) -> Result<Option<Arc<DeviceController>>, TargetSelectionError> {
        let choice = self.canonical_physical();
        if matches!(choice, Err(TargetSelectionError::Ambiguous))
            && self.targets.update(None, None).unwrap_or(false)
        {
            self.demo.target_changed();
        }
        let choice = choice?;
        let physical = choice.as_ref().map(|(controller, epoch)| {
            format!(
                "{}:physical:{}:{}",
                self.instance_id,
                controller.target_stream_id(),
                epoch
            )
        });
        if self
            .targets
            .update(physical, self.demo.session())
            .unwrap_or(false)
        {
            self.demo.target_changed();
        }
        Ok(choice.map(|(controller, _)| controller))
    }

    fn request_controller(
        &self,
        connection: Option<u64>,
        physical_only: bool,
        canonical: Option<&Arc<DeviceController>>,
    ) -> Result<Arc<DeviceController>, &'static str> {
        let selected = self.controller_for(connection);
        if physical_only {
            return Ok(selected);
        }
        if let Some(canonical) = canonical {
            if connection.is_some_and(|id| {
                self.clients
                    .lock_recover()
                    .connections
                    .get(&id)
                    .is_some_and(|row| row.selected_device.is_some() && !row.implicit_selection)
            }) && !Arc::ptr_eq(&selected, canonical)
            {
                return Err("MCP access to a noncanonical Quad Cortex is not supported; select the canonical device or use the physical-only app connection");
            }
            return Ok(canonical.clone());
        }
        Ok(selected)
    }

    fn controller_for(&self, connection_id: Option<u64>) -> Arc<DeviceController> {
        connection_id
            .and_then(|id| {
                self.clients
                    .lock_recover()
                    .connections
                    .get(&id)
                    .and_then(|row| row.selected_device.clone())
            })
            .unwrap_or_else(|| Arc::clone(&self.controller))
    }

    fn participant_count_for(&self, caller: Option<u64>) -> usize {
        let selected = self.controller_for(caller);
        self.clients
            .lock_recover()
            .connections
            .values()
            .filter(|row| {
                row.is_participant()
                    && Arc::ptr_eq(
                        row.selected_device.as_ref().unwrap_or(&self.controller),
                        &selected,
                    )
            })
            .count()
    }

    /// What `broker.controlStatus` reports: the worker's cached status (no
    /// USB traffic), then one short look at the accounting, then the host's
    /// own status file. Changes nothing, takes no request lock, so it answers
    /// beside a running backup. No token, no provider name, no instance id.
    fn control_status(&self) -> Value {
        let status = self.controller.status();
        let snapshot = {
            let clients = self.clients.lock_recover();
            let tray_hold = match clients.tray_hold {
                None => Value::Null,
                Some(IdlePolicy::Immediately) => json!({"policy": "immediately"}),
                Some(IdlePolicy::Never) => json!({"policy": "never"}),
                Some(IdlePolicy::Minutes(minutes)) => {
                    json!({"policy": "minutes", "minutes": minutes})
                }
            };
            json!({
                "participants": clients.participants(),
                "participantsOtherThanHost": clients.participants_other_than_host(),
                "hostAttached": clients
                    .connections
                    .values()
                    .any(|row| row.is_participant() && row.in_process_host),
                "observers": clients.observers(),
                "pendingAuthentications": clients.pending_authentications(),
                "controlConnections": clients
                    .connections
                    .values()
                    .filter(|row| row.control)
                    .count(),
                "resident": clients.resident,
                "trayHold": tray_hold,
                "idleReleased": clients.idle_released,
                "reopensInFlight": clients.reopens_in_flight,
                "shuttingDown": clients.shutting_down,
            })
        };
        let host_clients = self
            .descriptor_path
            .parent()
            .and_then(crate::tray::host_status::read)
            .map(|presence| {
                let remote = presence
                    .clients
                    .iter()
                    .filter(|client| client.remote)
                    .count();
                json!({"local": presence.clients.len() - remote, "remote": remote})
            })
            .unwrap_or(Value::Null);
        let mut result = snapshot;
        result["phase"] = json!(status.phase);
        result["connected"] = json!(status.connected);
        result["hostClients"] = host_clients;
        result["gatewayApiVersion"] = json!(generated_gateway::API_VERSION);
        with_broker_version(&mut result, BINARY_VERSION.get().copied());
        result
    }

    /// Whether a participant other than `caller` holds `device`, the
    /// controller the disconnect will act on. The caller passes the
    /// controller it already resolved, never its selection re-read here: the
    /// same connection can run `broker.selectDevice` while its disconnect
    /// waits for `request_lock`, and a re-read would then count the new
    /// device's participants while the disconnect closes the old one. The
    /// caller's own row is left out only by identity, so a caller now
    /// participating on another device does not discount `device`'s holder.
    fn disconnect_blocked_for(&self, caller: Option<u64>, device: &Arc<DeviceController>) -> bool {
        let clients = self.clients.lock_recover();
        Self::disconnect_blocked_under(&clients, &self.controller, caller, device)
    }

    fn disconnect_blocked_under(
        clients: &ClientAccounting,
        default: &Arc<DeviceController>,
        caller: Option<u64>,
        device: &Arc<DeviceController>,
    ) -> bool {
        if clients.connections.is_empty() {
            return clients.disconnect_blocked(caller);
        }
        let others = clients
            .connections
            .iter()
            .filter(|(id, row)| {
                Some(**id) != caller
                    && row.is_participant()
                    && Arc::ptr_eq(row.selected_device.as_ref().unwrap_or(default), device)
            })
            .count();
        // A caller with no connection row (in-process) keeps its historical
        // allowance of one participant: itself.
        others > usize::from(caller.is_none())
    }

    /// Called only with request_lock already held: never hold clients while
    /// waiting behind another operation. Authentication and hello promotion
    /// share this accounting lock, so admission has a real linearization point.
    fn reserve_disconnect_for(
        &self,
        caller: Option<u64>,
        device: &Arc<DeviceController>,
    ) -> Option<DisconnectAdmission> {
        let mut clients = self.clients.lock_recover();
        if clients.device_is_closing(device)
            || Self::disconnect_blocked_under(&clients, &self.controller, caller, device)
        {
            return None;
        }
        clients.closing_devices.push(Arc::clone(device));
        Some(DisconnectAdmission {
            clients: Arc::clone(&self.clients),
            device: Arc::clone(device),
        })
    }

    fn select_device(&self, connection_id: u64, id: &str) -> Result<(), String> {
        let present =
            quadcortex_hid::usb::connected_devices().map_err(|error| error.to_string())?;
        if !present.iter().any(|device| device.id == id) {
            return Err("The selected Quad Cortex is no longer available".into());
        }
        self.select_available_device(connection_id, id)
    }

    fn select_available_device(&self, connection_id: u64, id: &str) -> Result<(), String> {
        let _exclusive = self.request_lock.lock_recover();
        {
            let clients = self.clients.lock_recover();
            if clients.shutting_down {
                return Err("Broker shutdown is already committed".into());
            }
            if !clients
                .connections
                .get(&connection_id)
                .is_some_and(ConnectionInfo::is_participant)
            {
                return Err("Only attached participants select a device".into());
            }
        }

        let selected = if self.controller.claim_device(id) {
            Arc::clone(&self.controller)
        } else {
            let mut devices = self.devices.lock_recover();
            if let Some(controller) = devices
                .iter()
                .find(|controller| controller.device_id().as_deref() == Some(id))
            {
                Arc::clone(controller)
            } else {
                let controller =
                    Arc::new(DeviceController::start_for_device(Some(id.into()), false)?);
                if let Some(directory) = self.descriptor_path.parent() {
                    controller.configure_updater_relay(directory.to_path_buf());
                }
                devices.push(Arc::clone(&controller));
                controller
            }
        };
        let mut clients = self.clients.lock_recover();
        if clients.shutting_down {
            return Err("Broker shutdown is already committed".into());
        }
        if clients.device_is_closing(&selected) {
            return Err(
                "The selected device session is closing; select it again after it has closed"
                    .into(),
            );
        }
        let row = clients
            .connections
            .get_mut(&connection_id)
            .ok_or("Connection is no longer attached")?;
        if !row.is_participant() {
            return Err("Only participants select a device".into());
        }
        let previous = row
            .selected_device
            .replace(selected)
            .unwrap_or_else(|| Arc::clone(&self.controller));
        row.implicit_selection = false;
        drop(clients);
        self.release_unused_device_after_grace(previous);
        Ok(())
    }

    fn release_unused_device_after_grace(&self, device: Arc<DeviceController>) {
        if let Err(error) = background_policy::unused(self, device) {
            eprintln!("Could not start the device release grace thread; the unused device stays open: {error}");
        }
    }

    /// Called with request_lock held, preserving the existing unused-target
    /// policy (ANY attached row, including observer/pending, keeps it open).
    fn reserve_unused_device(
        accounting: &Arc<Mutex<ClientAccounting>>,
        default: &Arc<DeviceController>,
        device: &Arc<DeviceController>,
    ) -> Option<DisconnectAdmission> {
        let mut clients = accounting.lock_recover();
        if clients.shutting_down
            || clients.device_is_closing(device)
            || clients
                .connections
                .values()
                .any(|row| Arc::ptr_eq(row.selected_device.as_ref().unwrap_or(default), device))
        {
            return None;
        }
        clients.closing_devices.push(Arc::clone(device));
        Some(DisconnectAdmission {
            clients: Arc::clone(accounting),
            device: Arc::clone(device),
        })
    }

    fn disconnect_devices_report(&self) -> DisconnectDevicesReport {
        self.disconnect_controllers_report(self.all_controllers())
    }

    fn disconnect_controllers_report(
        &self,
        devices: Vec<Arc<DeviceController>>,
    ) -> DisconnectDevicesReport {
        // A failed close must not release ANY HID session while raw MIDI still
        // owns the matching device. Request callers already hold admission.
        let attempted = devices.len();
        for device in &devices {
            if let Err(error) = self.performance_midi.retire_controller(device) {
                return DisconnectDevicesReport {
                    midi_retired: false,
                    attempted,
                    queued: Vec::new(),
                    close_unacknowledged: Vec::new(),
                    failures: vec![error],
                };
            }
        }
        // Snapshot before waiting. Every logical session still closes even
        // when another controller cannot verify its native teardown.
        let mut queued = Vec::new();
        let mut close_unacknowledged = Vec::new();
        let mut failures = Vec::new();
        for device in devices {
            let (accepted, result) = device.disconnect_with_admission_report(None);
            if accepted {
                queued.push(Arc::clone(&device));
                if result.is_err() {
                    close_unacknowledged.push(Arc::clone(&device));
                }
            }
            if let Err(message) = result {
                failures.push(format!(
                    "{}: {message}",
                    device
                        .device_id()
                        .unwrap_or_else(|| "default device".into())
                ));
            }
        }
        self.test_point("disconnect_devices:done");
        if !failures.is_empty() {
            eprintln!("Device disconnect not verified: {}", failures.join("; "));
        }
        DisconnectDevicesReport {
            midi_retired: true,
            attempted,
            queued,
            close_unacknowledged,
            failures,
        }
    }

    fn disconnect_devices(&self) -> Result<(), String> {
        let report = self.disconnect_devices_report();
        if report.failures.is_empty() {
            Ok(())
        } else {
            Err(report.failures.join("; "))
        }
    }

    /// Every controller this broker owns, copied out of `devices` before any
    /// caller takes `clients`.
    fn all_controllers(&self) -> Vec<Arc<DeviceController>> {
        let mut all = vec![Arc::clone(&self.controller)];
        all.extend(self.devices.lock_recover().iter().cloned());
        all
    }

    /// A named pause point for tests; nothing in production.
    #[cfg(not(test))]
    fn test_point(&self, _name: &'static str) {}

    #[cfg(test)]
    fn test_point(&self, name: &'static str) {
        let hook = TEST_POINTS
            .lock_recover()
            .iter()
            .find(|(registered, point, _)| *registered == self.instance_id && *point == name)
            .map(|(_, _, hook)| Arc::clone(hook));
        if let Some(hook) = hook {
            let _ = hook();
        }
    }

    /// The grace before an unattended broker exits or, if resident, releases
    /// the device. Tests shorten it per fixture.
    fn idle_grace(&self) -> Duration {
        #[cfg(test)]
        {
            if let Some((_, grace)) = TEST_GRACE
                .lock_recover()
                .iter()
                .find(|(k, _)| *k == self.instance_id)
            {
                return *grace;
            }
        }
        IDLE_SHUTDOWN_GRACE
    }

    fn idle_release_retry_delay(&self) -> Duration {
        #[cfg(test)]
        if let Some((_, delay)) = TEST_IDLE_RETRY_DELAY
            .lock_recover()
            .iter()
            .find(|(key, _)| *key == self.instance_id)
        {
            return *delay;
        }
        RESIDENT_IDLE_RETRY_DELAY
    }

    /// One minute of an idle policy. Tests shorten it per fixture after the
    /// broker is serving (in-module tests also inject `observer_minute`).
    fn policy_minute(&self) -> Duration {
        #[cfg(test)]
        {
            if let Some((_, minute)) = TEST_MINUTE
                .lock_recover()
                .iter()
                .find(|(key, _)| *key == self.instance_id)
            {
                return *minute;
            }
        }
        self.observer_minute
    }

    /// Tiny records are written only for explicit forced releases and are
    /// deliberately retained: a suspended participant may recover much later,
    /// so deleting an old generation could make it reclaim the device.
    fn publish_forced_release(&self) -> Result<(), String> {
        let marker = BrokerReleaseMarker {
            instance_id: self.instance_id.clone(),
            forced: true,
        };
        let bytes = serde_json::to_vec(&marker).expect("release marker serializes");
        qc_owner_only_file::publish_atomically(
            &BrokerReleaseMarker::path_for(&self.descriptor_path, &self.instance_id),
            &bytes,
        )
        .map_err(|error| {
            format!("Could not record the intentional release; device was left open: {error}")
        })
    }

    /// A user's Quit, recorded for this generation before its shutdown
    /// commits (see [`crate::user_quit`]): nothing autonomous starts a broker
    /// again until a user action clears it.
    fn publish_user_quit(&self, source: crate::user_quit::UserQuitSource) -> Result<(), String> {
        crate::user_quit::publish(
            &self.descriptor_path,
            &crate::user_quit::UserQuitRecord {
                instance_id: self.instance_id.clone(),
                source,
            },
        )
    }

    /// Warn healthy peers before deliberate exit so a host does not mistake
    /// release for a crash and immediately auto-connect a replacement broker.
    /// The entire broadcast has one budget, not one budget per peer. Snapshot
    /// writers before I/O, never block accept/accounting behind a slow reader.
    fn notify_shutdown(&self, caller: u64, forced: bool) {
        let mut pending: Vec<_> = self
            .clients
            .lock_recover()
            .connections
            .iter()
            .filter(|(id, _)| **id != caller)
            .filter_map(|(_, row)| row.output.clone())
            .collect();
        let notification = json!({"jsonrpc":"2.0","method":"broker.shuttingDown","params":{
            "forced":forced,"detail":"The device was deliberately released; reconnect explicitly to start a new session"
        }});
        let Ok(body) = serde_json::to_vec(&notification) else {
            return;
        };
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        let deadline = Instant::now() + Duration::from_millis(200);
        while !pending.is_empty() && Instant::now() < deadline {
            pending.retain(|writer| {
                let Ok(mut writer) = writer.try_lock() else {
                    return true;
                };
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let _ = write_tcp_frame_before_deadline(&mut writer, &frame, deadline);
                false
            });
            if !pending.is_empty() {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// A committed shutdown cannot depend on its caller continuing to read.
    /// Its push thread may already hold output while blocked on a full socket;
    /// bound both lock acquisition and response writes, then exit regardless.
    fn finish_shutdown_after_response(&self, output: &Mutex<TcpStream>, response: &Value) {
        let deadline = Instant::now() + Duration::from_millis(200);
        if let Ok(body) = serde_json::to_vec(response) {
            let mut frame = (body.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(&body);
            while Instant::now() < deadline {
                match output.try_lock() {
                    Ok(mut output) => {
                        let _ = write_tcp_frame_before_deadline(&mut output, &frame, deadline);
                        break;
                    }
                    Err(std::sync::TryLockError::Poisoned(_)) => break,
                    Err(std::sync::TryLockError::WouldBlock) => {
                        thread::sleep(Duration::from_millis(1))
                    }
                }
            }
        }
        self.finish_shutdown();
    }

    fn finish_shutdown(&self) -> crate::worker::WorkerShutdownReport {
        #[cfg(target_os = "linux")]
        if !self.performance_midi.retire_all_devices() {
            let report = crate::worker::WorkerShutdownReport {
                outcomes: self
                    .all_controllers()
                    .iter()
                    .map(|controller| (controller.device_id(), crate::worker::WorkerExit::TimedOut))
                    .collect(),
            };
            eprintln!("MIDI retirement unacknowledged; HID and descriptor claim remain owned until process teardown");
            (self.on_idle_shutdown)();
            return report;
        }
        let _ = self.disconnect_devices();
        // `on_idle_shutdown` exits the process. Let the USB workers return
        // first, so a teardown a disconnect left in flight (an ack that timed
        // out inside the HID writer join) reaches the device before the
        // descriptor slot frees for the next broker. One shared deadline for
        // every device's worker stop-and-join, not one budget each. That bounds
        // only this phase: the `disconnect_devices` waits above (and in the
        // shutdown request handler) are per device and outside it. `all_controllers` copies the
        // controllers out under `devices` and releases it; every caller has
        // dropped `clients`; the workers need no broker lock.
        let controllers = self.all_controllers();
        let report = crate::worker::stop_workers_within(
            controllers.iter().map(Arc::as_ref),
            crate::worker::WORKER_EXIT_BUDGET,
        );
        if !report.all_verified() {
            eprintln!("QC device broker shutdown: {}", report.summary());
        }
        drop(controllers);
        if report.all_verified() {
            if let Some(claim) = self.descriptor_claim.lock_recover().take() {
                if let Err(error) = claim.release() {
                    eprintln!(
                        "Could not release {}: {error}",
                        self.descriptor_path.display()
                    );
                }
            }
        }
        (self.on_idle_shutdown)();
        report
    }

    /// Refuses once shutdown is committed. Checked under the same lock that
    /// commits it, so a socket still authenticating when a non-forced
    /// `broker.shutdown` (which ignores such sockets) commits can never then
    /// be counted and told it is authenticated by a broker about to exit.
    #[cfg(test)]
    fn mark_client_authenticated(self: &Arc<Self>, connection_id: u64) -> Result<(), String> {
        self.mark_client_authenticated_as(connection_id, AuthenticatedAs::Participant, None)
    }

    /// As `mark_client_authenticated`, for the role the handshake
    /// declared. A control connection changes nothing but its own row: no
    /// idle origin is reset, no backstop recomputed, nothing is announced.
    /// `output`, when given, becomes the connection's writer under the same
    /// lock: once marked, a committed shutdown's `broker.shuttingDown`
    /// broadcast finds it.
    fn mark_client_authenticated_as(
        self: &Arc<Self>,
        connection_id: u64,
        role: AuthenticatedAs,
        output: Option<Arc<Mutex<TcpStream>>>,
    ) -> Result<(), String> {
        self.mark_client_authenticated_as_with_output(connection_id, role, output)
    }

    /// Publish the authenticated socket's writer in the same critical section
    /// as authentication. A process signal can arrive as soon as the client
    /// receives its success reply; shutdown notices must already be able to
    /// reach that client at that point.
    fn mark_client_authenticated_as_with_output(
        self: &Arc<Self>,
        connection_id: u64,
        role: AuthenticatedAs,
        output: Option<Arc<Mutex<TcpStream>>>,
    ) -> Result<(), String> {
        let has_observers = {
            let mut clients = self.clients.lock_recover();
            if clients.shutting_down {
                return Err("Broker shutdown is already committed".into());
            }
            if let Some(row) = clients.connections.get_mut(&connection_id) {
                if let Some(output) = output {
                    row.output = Some(output);
                }
            }
            if matches!(
                role,
                AuthenticatedAs::Control | AuthenticatedAs::DemoProvider
            ) || clients.is_control(connection_id)
                || clients.is_demo_provider(connection_id)
            {
                if let Some(row) = clients.connections.get_mut(&connection_id) {
                    row.authenticated = true;
                    row.control = role == AuthenticatedAs::Control || row.control;
                    row.demo_provider = role == AuthenticatedAs::DemoProvider || row.demo_provider;
                }
                // No clock moves, but the backstop watcher was woken by this
                // socket's accept and parked while it was authenticating
                // (`reap_deadline` is `None` then): wake it to recompute, or
                // an icon or observer idle release waits for an unrelated
                // event that may never come.
                self.observer_changed.notify_all();
                return Ok(());
            }
            if clients.connections.get(&connection_id).is_some_and(|row| {
                matches!(row.role, ConnectionRole::Participant)
                    && clients
                        .device_is_closing(row.selected_device.as_ref().unwrap_or(&self.controller))
            }) {
                return Err(
                    "The selected device session is closing; attach again after it has closed"
                        .into(),
                );
            }
            if let Some(row) = clients.connections.get_mut(&connection_id) {
                row.authenticated = true;
                if matches!(row.role, ConnectionRole::Participant) && !row.control {
                    clients.observer_idle_since = None;
                    clients.tray_idle_since = None;
                    clients.idle_release_retry_not_before = None;
                }
            }
            clients.observers() != 0 || clients.tray_hold.is_some()
        };
        if has_observers {
            self.update_observer_backstop();
        }
        Ok(())
    }

    /// No timer is created until an observer is attached. A single wakeable
    /// timer handles repeated hello updates; a settings edit neither detaches
    /// the observer nor leaks a sleeping thread for the old (possibly day-long)
    /// deadline. Policy changes use the original participant-idle instant.
    fn update_observer_backstop(self: &Arc<Self>) {
        let mut clients = self.clients.lock_recover();
        // The tray's display-only identity read is opt-in too: with no
        // observer attached and no icon shown, the broker must generate no
        // extra USB work.
        self.controller.set_identity_observation_enabled(
            clients.observers() != 0 || clients.tray_hold.is_some(),
        );
        if clients.observers() == 0 || clients.participants() != 0 {
            clients.observer_idle_since = None;
        } else {
            // A pending authentication blocks expiry through observer_deadline
            // but preserves this origin. Failed probes cannot extend it.
            clients.observer_idle_since.get_or_insert_with(Instant::now);
        }
        if clients.tray_hold.is_none() || clients.participants() != 0 {
            clients.tray_idle_since = None;
        } else {
            clients.tray_idle_since.get_or_insert_with(Instant::now);
        }
        self.observer_changed.notify_all();
        let tray_waiting = clients.tray_holds_past_grace() && clients.tray_idle_since.is_some();
        if clients.shutting_down
            || (clients.observer_idle_since.is_none() && !tray_waiting)
            || clients.observer_timer_running
        {
            return;
        }
        clients.observer_timer_running = true;
        let marker = Arc::new(());
        clients.observer_owner = Some(Arc::clone(&marker));
        drop(clients);
        if let Err(error) = background_policy::observer(self, marker) {
            eprintln!("Could not start the broker observer backstop: {error}");
        }
    }

    fn observer_deadline(&self, clients: &ClientAccounting) -> Option<Instant> {
        // A socket still authenticating is not a participant, but it may be
        // one a moment from now: like Layer 1's raw count, it defers reap
        // (bounded by BROKER_AUTH_TIMEOUT) without resetting the idle origin.
        if clients.participants() != 0
            || clients.pending_authentications() != 0
            || clients.observers() == 0
        {
            return None;
        }
        let idle_since = clients.observer_idle_since?;
        clients
            .connections
            .values()
            .filter_map(|row| match row.role {
                ConnectionRole::Observer {
                    idle_policy_minutes,
                } => {
                    let minutes = idle_policy_minutes
                        .map_or((OBSERVER_IDLE_BACKSTOP.as_secs() / 60) as u32, |minutes| {
                            minutes + 5
                        });
                    // Never is a renewable hold, not a disabled timer that releases
                    // immediately. Legacy null observers retain their old deadline.
                    Some(
                        row.keep_connected_since
                            .unwrap_or(idle_since)
                            .max(idle_since)
                            + self.policy_minute() * minutes,
                    )
                }
                ConnectionRole::Participant => None,
            })
            .max()
    }

    /// When the device is next released for idleness, given every holder: each
    /// legacy observer's backstop and the broker's own icon policy. The device
    /// stays held until every holder's deadline has passed, so the latest one
    /// wins and Never holds indefinitely. `None` means nothing to wait for.
    fn reap_deadline(&self, clients: &ClientAccounting) -> Option<Instant> {
        // A resident broker that already closed the device session has nothing
        // left to release until a participant reopens it.
        if clients.participants() != 0
            || clients.pending_authentications() != 0
            || (clients.idle_released && clients.resident)
        {
            return None;
        }
        // A reopen is finishing a handshake: look again shortly rather than
        // commit anything under it.
        if clients.reopens_in_flight != 0 {
            return Some(Instant::now() + Duration::from_millis(250));
        }
        let observers = self.observer_deadline(clients);
        let tray = match clients.tray_hold {
            Some(IdlePolicy::Never) => return None,
            Some(IdlePolicy::Minutes(minutes)) => clients
                .tray_idle_since
                .map(|since| since + self.policy_minute() * minutes),
            Some(IdlePolicy::Immediately) | None => None,
        };
        observers
            .into_iter()
            .chain(tray)
            .chain(clients.idle_release_retry_not_before)
            .max()
    }

    fn watch_observer_backstop(self: Arc<Self>, ticket: background::Ticket, marker: Arc<()>) {
        loop {
            // Queue before clients is the admission order. Never test a
            // ticket while retaining clients across a Condvar wake.
            if !ticket.current() {
                return;
            }
            let mut clients = self.clients.lock_recover();
            if !clients
                .observer_owner
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &marker))
            {
                return;
            }
            if clients.shutting_down
                || (clients.observers() == 0 && !clients.tray_holds_past_grace())
            {
                clients.observer_timer_running = false;
                return;
            }
            if let Some(deadline) = self.reap_deadline(&clients) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    if clients.resident {
                        // The process stays: it is also the session's MCP
                        // host. Only the device session is released.
                        clients.observer_timer_running = false;
                        drop(clients);
                        background_policy::release_idle(&self, Some(&ticket));
                        return;
                    }
                    // Accept, role changes, all counts, and shutdown commitment
                    // share this lock. A quiet participant always cancels reap.
                    clients.shutting_down = true;
                    clients.observer_timer_running = false;
                    drop(clients);
                    self.finish_shutdown();
                    return;
                }
                drop(
                    self.observer_changed
                        .wait_timeout(clients, remaining.min(Duration::from_millis(100)))
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
            } else {
                drop(
                    self.observer_changed
                        .wait_timeout(clients, Duration::from_millis(100))
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
            }
        }
    }

    fn detach_client(self: &Arc<Self>, connection_id: u64) {
        if self.demo.withdraw(connection_id) {
            self.refresh_targets();
        }
        let previous = self.controller_for(Some(connection_id));
        let (control, arm) = {
            let mut clients = self.clients.lock_recover();
            let control = clients
                .connections
                .remove(&connection_id)
                .is_some_and(|row| row.control || row.demo_provider);
            clients.count -= 1;
            let arm = if clients.count != 0 {
                if !control {
                    // Someone is still counted: the grace this departure would
                    // have armed waits for the last one out.
                    clients.reap_deferred = true;
                }
                false
            } else if control {
                // Only what was pending before (see `reap_deferred`).
                std::mem::take(&mut clients.reap_deferred)
            } else {
                clients.reap_deferred = false;
                true
            };
            (control, arm)
        };
        if control {
            // A control connection selected nothing, was never a participant
            // or an observer, and changed no clock. The backstop watcher
            // still has to look again: the accounting it waits on changed.
            self.observer_changed.notify_all();
            if arm {
                self.arm_idle_shutdown();
            }
            return;
        }
        if !Arc::ptr_eq(&previous, &self.controller) {
            self.release_unused_device_after_grace(previous);
        }
        self.update_observer_backstop();
        if arm {
            self.arm_idle_shutdown();
        }
    }

    /// The process became, or stopped being, the session's resident host.
    /// Losing residency with no connection left falls back to the shipped
    /// five-second grace, exactly like losing the icon: a broker that is not
    /// resident is reaped the way it always was.
    fn set_resident(self: &Arc<Self>, resident: bool) {
        let rearm = {
            let mut clients = self.clients.lock_recover();
            if clients.resident == resident {
                return;
            }
            clients.resident = resident;
            let wants_grace =
                !resident && !clients.shutting_down && !clients.tray_holds_past_grace();
            if wants_grace && clients.count != 0 {
                // The last connection out arms it (`detach_client`).
                clients.reap_deferred = true;
            }
            wants_grace && clients.count == 0
        };
        if rearm {
            self.arm_idle_shutdown();
        }
    }

    /// A resident reap: close every device session, stay published. Decided
    /// again under `request_lock` then `clients` (the lock order), so a
    /// participant that arrived after the timer fired keeps the session. The
    /// devices are closed while holding `request_lock`, then commands accepted
    /// by their workers are recorded as released before the lock is dropped.
    /// A request arriving in between waits, sees the record, and reopens.
    #[cfg(test)]
    fn release_idle_device(self: &Arc<Self>) {
        background_policy::release_idle(self, None);
    }

    /// Called with the controller a request is about to reach, before it is
    /// served. After a resident idle release this is where that one device,
    /// and only that one, comes back:
    /// - a reconnect, reset or disconnect from any authenticated connection
    ///   (an observer's included) manages the session itself, so the device
    ///   just leaves the released set, and a reconnect or reset ends the idle
    ///   release so the idle policy can close the session again;
    /// - `system.status` (the clients' 1 Hz poll) neither starts nor waits for
    ///   a reopen, and nothing from a connection that is not an authenticated
    ///   participant does: only device demand opens USB;
    /// - anything else starts or joins the reopen and waits for a ready
    ///   session, bounded by `IDLE_REOPEN_WAIT`, then is served on it. A
    ///   reopen that fails or runs out of time is a clear, retryable
    ///   "not connected" error that carries the reopen's own reason.
    ///
    /// Takes only the `clients` lock, briefly, never while waiting, and runs
    /// before `request_lock`. The reopen itself reconnects under
    /// `request_lock`, so it is ordered after any idle release or icon
    /// Disconnect already holding it, and it re-checks for a committed
    /// shutdown first.
    fn prepare_idle_released_target(
        &self,
        connection_id: Option<u64>,
        controller: &Arc<DeviceController>,
        method: &str,
    ) -> Result<(), String> {
        // A status poll is not device demand: it reports the session as it is.
        if method == generated_gateway::rpc::RUNTIME_STATUS {
            return Ok(());
        }
        let session_command = matches!(
            method,
            generated_gateway::rpc::RECONNECT
                | generated_gateway::rpc::RESET_SESSION
                | generated_gateway::rpc::DISCONNECT
        );
        let attempt = {
            let mut clients = self.clients.lock_recover();
            let (authenticated, participant) = match connection_id {
                // The in-module test entry point, which has no connection.
                None => (true, true),
                Some(id) => clients.connections.get(&id).map_or((false, false), |row| {
                    (row.authenticated, row.is_participant())
                }),
            };
            let close_unacknowledged = clients
                .idle_entry(controller)
                .is_some_and(|entry| entry.close_unacknowledged);
            if session_command && authenticated {
                if close_unacknowledged
                    || clients
                        .idle_entry(controller)
                        .is_some_and(|entry| entry.manually_released)
                {
                    // Keep this marker until the session command has crossed
                    // the worker FIFO barrier and succeeds. An unforced
                    // reconnect must not trust cached `ready` state here.
                    return Ok(());
                }
                // A disconnect may still be refused (another participant is
                // attached), so the released entry stays until it really runs:
                // `dispatch_broker_client_request` forgets it after its last
                // re-check, under `request_lock`. A reconnect or reset always
                // runs, and manages the session itself.
                if method != generated_gateway::rpc::DISCONNECT {
                    clients.idle_released = false;
                    clients.idle_release_retry_not_before = None;
                    self.observer_changed.notify_all();
                    clients.forget_idle_release(controller);
                }
                return Ok(());
            }
            // Only a participant's request is device demand. A socket still
            // authenticating, or an observer, never opens USB.
            if !participant {
                return Ok(());
            }
            let Some(entry) = clients.idle_entry(controller) else {
                // Nothing to reopen: lock-free reads keep working during a
                // committed shutdown exactly as they did before.
                return Ok(());
            };
            if entry.manually_released {
                return Err(
                    "The Quad Cortex is disconnected by the user. Choose Connect to use it again."
                        .into(),
                );
            }
            let joining = entry.reopening.then_some(entry.attempt);
            if clients.shutting_down {
                return Err(
                    "The Quad Cortex is not connected: broker shutdown is already committed".into(),
                );
            }
            // Something reaches a device: there may be a session to release
            // again later.
            clients.idle_released = false;
            match joining {
                Some(attempt) => attempt,
                None => {
                    let entry = clients
                        .idle_entry_mut(controller)
                        .expect("entry just found");
                    entry.attempt += 1;
                    entry.reopening = true;
                    let attempt = entry.attempt;
                    clients.reopens_in_flight += 1;
                    drop(clients);
                    self.spawn_idle_reopen(Arc::clone(controller), attempt, close_unacknowledged);
                    attempt
                }
            }
        };
        let deadline = Instant::now() + IDLE_REOPEN_WAIT;
        loop {
            {
                let clients = self.clients.lock_recover();
                let Some(entry) = clients.idle_entry(controller) else {
                    return Ok(());
                };
                if entry.manually_released {
                    return Err("The Quad Cortex is disconnected by the user. Choose Connect to use it again.".into());
                }
                if let Some((failed, reason)) = &entry.last_failure {
                    if *failed >= attempt {
                        return Err(format!("The Quad Cortex is not connected: {reason}"));
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "The Quad Cortex is not connected: its session, closed for idleness, did not reopen within {} seconds; retry",
                    IDLE_REOPEN_WAIT.as_secs()
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Reopen one idle-released device on its own thread:
    /// 1. under `request_lock`, re-check that no shutdown was committed and
    ///    this attempt is still wanted, then ask the worker to reconnect
    ///    (bounded: it returns once the worker acknowledges);
    /// 2. without any lock, wait for the session (bounded). A forced release
    ///    can interrupt this: its disconnect raises the worker's connect
    ///    interrupt;
    /// 3. record the outcome for every request waiting on this attempt.
    ///
    /// A guard records a failure and releases the in-flight count however the
    /// thread ends, a panic included.
    fn spawn_idle_reopen(
        &self,
        controller: Arc<DeviceController>,
        attempt: u64,
        close_unacknowledged: bool,
    ) {
        if let Err(error) =
            background_policy::reopen(self, controller, attempt, close_unacknowledged)
        {
            // The closure (and the guard inside it) was dropped unrun; the
            // guard already recorded a failure and released the count.
            eprintln!("Could not start reopening the Quad Cortex: {error}");
        }
    }

    /// The icon appeared (`Some`), changed policy, or disappeared (`None`).
    /// Appearing starts the idle origin now if nobody is attached; disappearing
    /// with no connection left falls back to the shipped five-second grace, so
    /// a broker without its icon never holds the device longer than it did
    /// before the icon existed.
    fn set_tray_hold(self: &Arc<Self>, hold: Option<IdlePolicy>) {
        let fall_back_to_grace = {
            let mut clients = self.clients.lock_recover();
            if clients.tray_hold == hold {
                return;
            }
            let was_holding = clients.tray_holds_past_grace();
            if clients.tray_hold.is_none() {
                clients.tray_idle_since = None;
            }
            clients.tray_hold = hold;
            let wants_grace = was_holding && !clients.tray_holds_past_grace();
            if wants_grace && clients.count != 0 {
                // The last connection out arms it (`detach_client`).
                clients.reap_deferred = true;
            }
            wants_grace && clients.count == 0
        };
        self.update_observer_backstop();
        if fall_back_to_grace {
            self.arm_idle_shutdown();
        }
    }

    /// Waits out the grace period and, if nobody is attached by then, gives up
    /// the descriptor slot and exits. Armed on every last disconnect, and once
    /// at publish: a broker whose spawner died before attaching, or whose
    /// first client failed its version check and never came back, would
    /// otherwise never see a disconnect and live forever - holding the USB
    /// device, when started with `--auto-connect`.
    fn arm_idle_shutdown(self: &Arc<Self>) {
        if let Err(error) = background_policy::idle(self) {
            // Without this thread an unattended broker never reaps itself.
            eprintln!("Could not start the device broker idle-shutdown grace thread: {error}");
        }
    }
}

/// What this process's own tray may do to the broker it lives in. Each call
/// takes only the short `clients` lock, or waits for `request_lock` exactly as
/// a client request would; none of them is ever made from the UI thread.
#[derive(Clone)]
pub struct BrokerHandle(Arc<BrokerShared>);

impl BrokerHandle {
    /// SIGTERM/SIGINT are process lifecycle requests, not a recorded user Quit.
    /// Commit shutdown before stopping workers so no concurrent client can
    /// reopen USB. Reuse the shared bounded notification and teardown path.
    #[cfg(unix)]
    pub(crate) fn shutdown_from_os(&self) {
        {
            let mut clients = self.0.clients.lock_recover();
            if clients.shutting_down {
                return;
            }
            clients.shutting_down = true;
        }
        self.0.observer_changed.notify_all();
        self.0.notify_shutdown(u64::MAX, true);
        self.0.finish_shutdown();
    }
    /// The directory the descriptor, the tray settings and the host status
    /// file share.
    pub fn directory(&self) -> PathBuf {
        self.0
            .descriptor_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// The worker's cached status: no USB traffic.
    pub fn status(&self) -> BrokerStatus {
        self.0.controller.status()
    }

    /// A non-blocking status projection for the in-process tray. A busy
    /// device-state lock is not a failed broker heartbeat.
    pub(crate) fn try_tray_status(&self) -> Option<BrokerStatus> {
        self.0.controller.try_tray_status()
    }

    /// Whether this broker's Quad Cortex session is open, for the in-process
    /// host's `/health` and relay readiness while that host holds no
    /// participant connection (it attaches only while MCP clients are
    /// present).
    ///
    /// Read from the worker's cached connection state under its short status
    /// lock. It is never a request: it opens nothing, never claims the idle
    /// reopen of a session an idle release or the icon closed, attaches no
    /// participant and counts as no device demand - the same answer a
    /// `system.status` poll carries (see `prepare_idle_released_target`). A
    /// session closed for idleness is therefore reported not ready, which is
    /// the truth until a client's device request reopens it.
    pub fn device_readiness(&self) -> crate::resident::DeviceReadiness {
        if self.shutting_down() {
            return crate::resident::DeviceReadiness::Unavailable(
                "the device broker is shutting down".into(),
            );
        }
        let physical = match self.0.selected_target() {
            Ok(physical) => physical,
            Err(reason) if reason.is_unknown() => return crate::resident::DeviceReadiness::Unknown,
            Err(reason) => {
                return crate::resident::DeviceReadiness::Unavailable(reason.message().into())
            }
        };
        let Some((connected, phase)) = self.0.controller.try_connection_readiness() else {
            return crate::resident::DeviceReadiness::Unknown;
        };
        if let Some(target) = self.0.targets.identity() {
            crate::resident::DeviceReadiness::ReadyTarget {
                usb_connected: physical.is_some() || connected,
                target,
            }
        } else {
            crate::resident::DeviceReadiness::NotReady { phase }
        }
    }

    /// Providers of the authenticated participants, in connection order.
    pub fn served_target(&self) -> Option<crate::target::TargetIdentity> {
        // Reuse the existing tray tick with only try-readiness facts. This
        // never acquires the retained device tree and adds no cloud timer.
        self.0.refresh_targets();
        self.0.targets.identity()
    }

    /// Providers of the authenticated participants, in connection order.
    pub fn participants(&self) -> Vec<String> {
        self.0
            .clients
            .lock_recover()
            .connections
            .values()
            .filter(|row| row.is_participant())
            .map(|row| row.provider.clone())
            .collect()
    }

    /// See [`BrokerShared::set_tray_hold`]: `Some` only while the icon shows.
    pub fn set_tray_hold(&self, hold: Option<IdlePolicy>) {
        self.0.set_tray_hold(hold);
    }

    /// Whether this process is the session's resident host. Set by the tray's
    /// state thread: true only while the user's switch is on and the icon
    /// thread is alive. See [`BrokerShared::set_resident`].
    pub fn set_resident(&self, resident: bool) {
        self.0.set_resident(resident);
    }

    pub fn is_resident(&self) -> bool {
        self.0.clients.lock_recover().resident
    }

    /// Shutdown is committed: the process is about to exit.
    pub fn shutting_down(&self) -> bool {
        self.0.clients.lock_recover().shutting_down
    }

    /// A broker that publishes with its default device closed and nobody to
    /// open it (a `--resident` start at sign-in) records that device exactly
    /// as an idle release would, so the first participant's device request
    /// opens it and waits for it.
    pub fn mark_started_disconnected(&self) {
        let mut clients = self.0.clients.lock_recover();
        if clients.idle_entry(&self.0.controller).is_none() {
            clients.idle_released = true;
            clients
                .idle_released_devices
                .push(IdleReopenEntry::released(Arc::clone(&self.0.controller)));
        }
    }

    /// Providers of the authenticated participants other than this process's
    /// own background MCP host, in connection order: the clients the icon
    /// lists. The host is the broker's own machinery; the AI apps it serves
    /// are listed from its published status instead.
    pub fn participants_without_host(&self) -> Vec<String> {
        self.0
            .clients
            .lock_recover()
            .connections
            .values()
            .filter(|row| row.is_participant() && !row.in_process_host)
            .map(|row| row.provider.clone())
            .collect()
    }

    /// Participants other than this process's own MCP host.
    pub fn participants_other_than_host(&self) -> usize {
        self.0.clients.lock_recover().participants_other_than_host()
    }

    /// The icon's Connect. It opens the default device itself, so that device
    /// leaves the idle-released set, and with something open again the idle
    /// policy may release it later.
    pub fn reconnect(&self) -> Result<String, String> {
        let _exclusive = self.0.request_lock.lock_recover();
        let close_unacknowledged = {
            let clients = self.0.clients.lock_recover();
            if clients.shutting_down {
                return Err("Broker shutdown is already committed".into());
            }
            if clients.reopens_in_flight != 0 {
                return Err(
                    "The Quad Cortex is already reopening; try Connect again in a moment".into(),
                );
            }
            clients
                .idle_entry(&self.0.controller)
                .is_some_and(|entry| entry.close_unacknowledged)
        };
        let outcome = icon_reconnect(&self.0.controller, close_unacknowledged);
        if outcome.is_ok() {
            {
                let mut clients = self.0.clients.lock_recover();
                clients.idle_released = false;
                clients.forget_idle_release(&self.0.controller);
                // Tray Disconnect applies to all controllers. A successful
                // Connect lifts that manual hold for all of them; secondary
                // devices reopen lazily, retaining their pending-close FIFO
                // barriers rather than trusting stale readiness.
                for entry in &mut clients.idle_released_devices {
                    entry.manually_released = false;
                }
            }
            self.0.update_observer_backstop();
        }
        outcome.map(|_| "Connecting to the Quad Cortex…".to_owned())
    }

    /// Explicit Disconnect releases USB without exiting or detaching clients.
    /// Ordinary client demand cannot undo it; Connect reopens the session.
    pub fn disconnect(&self) -> Result<String, String> {
        self.release_devices_staying()
    }

    /// The icon and control CLI share this release, regardless of participants.
    /// Preserve accepted-close barriers even when acknowledgment times out.
    pub(crate) fn release_devices_staying(&self) -> Result<String, String> {
        let _exclusive = self.0.request_lock.lock_recover();
        let clients = self.0.clients.lock_recover();
        if clients.shutting_down {
            return Err("Broker shutdown is already committed".into());
        }
        if clients.reopens_in_flight != 0 {
            return Err("The Quad Cortex is reopening; choose Disconnect again in a moment".into());
        }
        drop(clients);
        let report = self.0.disconnect_devices_report();
        if report.midi_retired && !report.queued.is_empty() {
            let queued_count = report.queued.len();
            let mut clients = self.0.clients.lock_recover();
            clients.record_release(report.queued, report.close_unacknowledged, true);
            clients.idle_released = queued_count == report.attempted;
        }
        if !report.failures.is_empty() {
            return Err(report.failures.join("; "));
        }
        Ok("Disconnected. Connect starts the device session again.".to_owned())
    }

    /// Release the device and exit through the same committed shutdown a
    /// client's `broker.shutdown` takes. `forced` records the release for any
    /// attached participant.
    pub fn release_and_exit(&self, forced: bool) -> Result<String, String> {
        self.release_and_exit_recording(forced, None)
    }

    /// The icon's Quit: [`Self::release_and_exit`], recording the user's Quit
    /// for this generation whether or not it is forced ([`crate::user_quit`]).
    /// When the record cannot be written, nothing is released, the broker
    /// keeps serving and the error says so; a Quit is never claimed without
    /// it.
    pub fn quit_by_user(&self, forced: bool) -> Result<String, String> {
        self.release_and_exit_recording(forced, Some(crate::user_quit::UserQuitSource::Tray))
    }

    fn release_and_exit_recording(
        &self,
        forced: bool,
        user_quit: Option<crate::user_quit::UserQuitSource>,
    ) -> Result<String, String> {
        let request = Request {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: "broker.shutdown".into(),
            params: json!({"force": forced}),
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        };
        let response = request_broker_shutdown_recording(&self.0, request, None, || {}, user_quit);
        if let Some(message) = response["error"]["message"].as_str() {
            return Err(message.to_owned());
        }
        self.0.notify_shutdown(u64::MAX, forced);
        let report = self.0.finish_shutdown();
        if report.all_verified() {
            Ok("Released".into())
        } else {
            // A worker that did not return is not a clean release; say so
            // rather than reporting success.
            Ok(format!("Broker shutdown committed; {}", report.summary()))
        }
    }

    /// An installer's stop request: honoured only with no participant and no
    /// socket still authenticating, decided under the accounting lock so a
    /// client arriving meanwhile is either refused by the accept loop or
    /// keeps the broker. Never records a forced release.
    pub fn release_for_install(&self) -> bool {
        {
            let mut clients = self.0.clients.lock_recover();
            // A running reopen counts as in use: the installer retries, and
            // never exits this process under a handshake.
            if clients.shutting_down
                || clients.participants() != 0
                || clients.pending_authentications() != 0
                || clients.reopens_in_flight != 0
            {
                return false;
            }
            clients.shutting_down = true;
        }
        self.0.observer_changed.notify_all();
        self.0.notify_shutdown(u64::MAX, false);
        self.0.finish_shutdown();
        true
    }
}

/// Constructs the worker registry without opening an unbound MIDI endpoint.
fn new_performance_midi_workers() -> Arc<PerformanceMidiWorkers> {
    Arc::new(PerformanceMidiWorkers::default())
}

/// Runs the shared multi-client broker.
///
/// Binds `127.0.0.1:0`, claims the descriptor slot at `descriptor_path`
/// (exiting defensively if that fails - under correct `--broker-serve`
/// launch logic this process should never have been started at all in that
/// case, see the broker election path in `cli.rs`), publishes the descriptor, and accepts TCP connections
/// for the rest of the process's life. Each connection authenticates with
/// `authenticate_tcp_client` and then is served by
/// `serve_broker_tcp_connection` on its own thread. Keyed MIDI workers are
/// created lazily only after a realtime request is admitted.
/// The USB auto-connect handshake (when `auto_connect` is set) is deferred
/// the same way, started only after publication via
/// `DeviceController::reconnect` on its own thread; `controller` itself is
/// always constructed disconnected by the caller (`cli.rs`'s
/// `--broker-serve` branch), so nothing here ever blocks on real hardware
/// before the descriptor is written.
///
/// `on_idle_shutdown` runs after the 5s grace period following the last
/// disconnect - or following publication, if no client ever attaches -
/// elapses with the client count still at zero (after
/// `release_descriptor_slot` has already run). Production passes a closure
/// that calls `std::process::exit(0)`; tests pass one that only flips a
/// flag, so the behavior under test - "does the broker decide to shut
/// down" - is observable without ending the test process.
pub fn serve_broker_tcp(
    controller: Arc<DeviceController>,
    phase_events: bool,
    descriptor_path: PathBuf,
    on_idle_shutdown: Arc<dyn Fn() + Send + Sync>,
    auto_connect: bool,
) -> Result<(), String> {
    serve_broker_tcp_with_published_hook(
        controller,
        phase_events,
        descriptor_path,
        on_idle_shutdown,
        auto_connect,
        |_| {},
    )
}

/// `serve_broker_tcp`, calling `on_published` once the descriptor slot is
/// claimed and published and before the first connection is accepted. The
/// broker's own tray starts there: a broker that lost the election exits
/// before it, so it never draws an icon.
pub fn serve_broker_tcp_with_published_hook(
    controller: Arc<DeviceController>,
    phase_events: bool,
    descriptor_path: PathBuf,
    on_idle_shutdown: Arc<dyn Fn() + Send + Sync>,
    auto_connect: bool,
    on_published: impl FnOnce(BrokerHandle),
) -> Result<(), String> {
    serve_broker_tcp_full(
        controller,
        phase_events,
        descriptor_path,
        on_idle_shutdown,
        auto_connect,
        |controller| {
            let _ = controller.reconnect();
        },
        on_published,
    )
}

struct ServingJob {
    thread: thread::JoinHandle<()>,
    socket: Option<TcpStream>,
    // Retained with the original handle until actual finish/join, including
    // unknown-retirement storage. Function-return alone frees no capacity.
    _connection_owner: Option<Arc<()>>,
}

struct ServingLifetime {
    stop: Arc<AtomicBool>,
    claim: Option<Arc<Mutex<Option<qc_owner_only_file::ClaimedFile>>>>,
    shared: Option<Arc<BrokerShared>>,
    jobs: Vec<ServingJob>,
    cleanup_started: bool,
}

fn retained_serving_lifetimes() -> &'static Mutex<Vec<ServingLifetime>> {
    static RETAINED: std::sync::OnceLock<Mutex<Vec<ServingLifetime>>> = std::sync::OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}

impl ServingLifetime {
    fn new(
        stop: Arc<AtomicBool>,
        claim: Arc<Mutex<Option<qc_owner_only_file::ClaimedFile>>>,
    ) -> Self {
        // Retirement callbacks may reenter admission. Extract original
        // lifetime owners before any join/callback/publication release.
        let retired = std::mem::take(&mut *retained_serving_lifetimes().lock_recover());
        let mut unfinished = Vec::new();
        let mut complete = Vec::new();
        for mut lifetime in retired {
            lifetime.reap_finished();
            let background_done = lifetime.shared.as_ref().is_none_or(|shared| {
                shared.background.acknowledged() && shared.observer_actions.acknowledged()
            });
            if lifetime.jobs.is_empty() && background_done {
                complete.push(lifetime);
            } else {
                unfinished.push(lifetime);
            }
        }
        retained_serving_lifetimes()
            .lock_recover()
            .extend(unfinished);
        drop(complete);
        Self {
            stop,
            claim: Some(claim),
            shared: None,
            jobs: Vec::new(),
            cleanup_started: false,
        }
    }

    fn reap_finished(&mut self) {
        let mut index = 0;
        while index < self.jobs.len() {
            if crate::platform::process::thread_signalled(&self.jobs[index].thread) {
                let job = self.jobs.swap_remove(index);
                if job.thread.join().is_err() {
                    eprintln!("An owned broker serving task panicked during retirement");
                }
            } else {
                index += 1;
            }
        }
    }
}

impl ServingLifetime {
    fn finish(&mut self) -> Result<(), String> {
        if self.cleanup_started {
            return Ok(());
        }
        self.cleanup_started = true;
        self.stop.store(true, Ordering::Release);
        if let Some(shared) = &self.shared {
            shared.clients.lock_recover().shutting_down = true;
            // No clients lock is retained across cancellation callbacks.
            shared.background.stop();
            shared.observer_actions.stop();
            shared.observer_changed.notify_all();
        }
        for job in &self.jobs {
            if let Some(socket) = &job.socket {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
        }
        let deadline = Instant::now() + crate::worker::WORKER_EXIT_BUDGET;
        loop {
            self.reap_finished();
            let background_done = self.shared.as_ref().is_none_or(|shared| {
                shared.background.acknowledged() && shared.observer_actions.acknowledged()
            });
            if (self.jobs.is_empty() && background_done) || Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let background_done = self.shared.as_ref().is_none_or(|shared| {
            shared.background.acknowledged() && shared.observer_actions.acknowledged()
        });
        if !self.jobs.is_empty() || !background_done {
            let error = format!("Broker serving cleanup has {} unacknowledged serving task(s), background acknowledged={background_done}; retaining exact handles, accounting and descriptor ownership", self.jobs.len());
            retained_serving_lifetimes().lock_recover().push(Self {
                stop: Arc::clone(&self.stop),
                claim: self.claim.take(),
                shared: self.shared.take(),
                jobs: std::mem::take(&mut self.jobs),
                cleanup_started: false,
            });
            return Err(error);
        }
        if let Some(claim) = self.claim.take() {
            if let Some(file) = claim.lock_recover().take() {
                if let Err(error) = file.release() {
                    return Err(format!(
                        "Could not retire the owned broker publication: {error}"
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Drop for ServingLifetime {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("{error}");
        }
    }
}

fn serve_broker_tcp_full(
    controller: Arc<DeviceController>,
    phase_events: bool,
    descriptor_path: PathBuf,
    on_idle_shutdown: Arc<dyn Fn() + Send + Sync>,
    auto_connect: bool,
    perform_auto_connect: impl FnOnce(&DeviceController) + Send + 'static,
    on_published: impl FnOnce(BrokerHandle),
) -> Result<(), String> {
    serve_broker_tcp_full_with_stop(
        controller,
        phase_events,
        descriptor_path,
        on_idle_shutdown,
        auto_connect,
        perform_auto_connect,
        on_published,
        Arc::new(AtomicBool::new(false)),
    )
}

/// The normal server and in-process fixtures share the same owned startup and
/// teardown path. The explicit token never changes authentication or dispatch.
#[allow(clippy::too_many_arguments)]
fn serve_broker_tcp_full_with_stop(
    controller: Arc<DeviceController>,
    phase_events: bool,
    descriptor_path: PathBuf,
    on_idle_shutdown: Arc<dyn Fn() + Send + Sync>,
    auto_connect: bool,
    perform_auto_connect: impl FnOnce(&DeviceController) + Send + 'static,
    on_published: impl FnOnce(BrokerHandle),
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Could not bind the device broker TCP listener: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not make the broker listener cancellable: {error}"))?;
    let local_port = listener
        .local_addr()
        .map_err(|error| format!("Could not read the device broker TCP listener address: {error}"))?
        .port();

    let bearer_token: Arc<str> = Arc::from(generate_token(32));
    let instance_id: Arc<str> = Arc::from(generate_token(16));

    // A refusal returns rather than exiting here, so `on_published` (which
    // owns a `--resident` start's election lock) is dropped and that lock
    // released before the caller exits 1. `process::exit` runs no
    // destructors: it left the lock for the app to wait out as stale.
    let descriptor_file = try_claim_descriptor_slot(&descriptor_path)
        .map_err(|()| descriptor_claim_refusal(&descriptor_path))?;
    let descriptor_claim = Arc::new(Mutex::new(Some(descriptor_file)));
    let mut serving = ServingLifetime::new(Arc::clone(&stop), Arc::clone(&descriptor_claim));

    // The empty registry is ready before publication; native endpoints are
    // created only by admitted per-device worker jobs.
    let performance_midi = new_performance_midi_workers();

    let mut descriptor = BrokerDescriptor {
        port: local_port,
        bearer_token: bearer_token.to_string(),
        instance_id: instance_id.to_string(),
        process_id: std::process::id(),
        process_created_at: own_process_created_at(),
        published_at_unix_ms: unix_time_ms(),
        gateway_api_version: generated_gateway::API_VERSION,
    };
    {
        let mut claim = descriptor_claim.lock_recover();
        write_descriptor(claim.as_mut().expect("claimed descriptor"), &descriptor)
            .map_err(|error| format!("Could not publish the device broker descriptor: {error}"))?;
    }
    let refresh_claim = Arc::clone(&descriptor_claim);
    let refresh_stop = Arc::clone(&stop);
    let refresh = thread::Builder::new()
        .name("qc-broker-descriptor-refresh".into())
        .spawn(move || {
            refresh_descriptor_while_claimed_until(
                &refresh_claim,
                DESCRIPTOR_REFRESH_INTERVAL,
                DESCRIPTOR_REFRESH_RETRY_FLOOR,
                &refresh_stop,
                |file| {
                    descriptor.published_at_unix_ms = unix_time_ms();
                    write_descriptor(file, &descriptor)
                },
            )
        })
        .map_err(|error| format!("Could not start the descriptor refresh thread: {error}"))?;
    serving.jobs.push(ServingJob {
        thread: refresh,
        socket: None,
        _connection_owner: None,
    });

    // The update relay's switch lives in the settings file beside the
    // descriptor; only a broker that has a descriptor can be told to turn it off.
    if let Some(directory) = descriptor_path.parent() {
        controller.configure_updater_relay(directory.to_path_buf());
    }
    let exit_stop = Arc::clone(&stop);
    let owned_exit: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        // Shutdown replies and native retirement happen before this callback.
        // Merely setting clients.shutting_down earlier must not close sockets
        // before the committed shutdown response has been delivered.
        exit_stop.store(true, Ordering::Release);
        on_idle_shutdown();
    });
    let shared = Arc::new(BrokerShared {
        controller,
        devices: Mutex::new(Vec::new()),
        performance_midi,
        request_lock: Arc::new(Mutex::new(())),
        clients: Arc::new(Mutex::new(ClientAccounting::default())),
        descriptor_path,
        instance_id: instance_id.to_string(),
        descriptor_claim,
        on_idle_shutdown: owned_exit,
        observer_changed: Condvar::new(),
        background: background::Manager::default(),
        observer_actions: background::Manager::default(),
        demo: crate::demo_provider::Hub::default(),
        targets: crate::target::TargetRegistry::default(),
        observer_minute: Duration::from_secs(60),
    });
    serving.shared = Some(Arc::clone(&shared));
    shared.arm_idle_shutdown();
    on_published(BrokerHandle(Arc::clone(&shared)));

    // Deferred until after publication: publication only promises that the
    // broker can authenticate a client, and authentication never touches the
    // device, so the USB auto-connect handshake - which can take upwards of
    // ten seconds against real hardware - must not run before publication,
    // or a spawning host can kill this process mid-handshake once its own
    // publish-wait budget (a few seconds, covering only bind + descriptor
    // claim + one file write) elapses. `DeviceController::reconnect` mirrors
    // the `device.reconnect` RPC below: it flips the worker's internal
    // auto-connect flag and returns once the worker has acknowledged the
    // request, not once a device has actually answered, so this thread's
    // own lifetime is short regardless of whether a Quad Cortex is present.
    // The accept loop below is unaffected either way - a client that
    // connects and authenticates before the device is ready reads that
    // state from `system.status`/`device.status`, exactly as it already
    // does for a `--stdio` broker started without `--auto-connect`.
    if auto_connect {
        let reconnect_controller = Arc::clone(&shared.controller);
        let reconnect = thread::Builder::new()
            .name("qc-broker-tcp-auto-connect".into())
            .spawn(move || {
                perform_auto_connect(&reconnect_controller);
            })
            .map_err(|error| {
                format!("Could not start the deferred auto-connect thread: {error}")
            })?;
        serving.jobs.push(ServingJob {
            thread: reconnect,
            socket: None,
            _connection_owner: None,
        });
    }

    let mut accept_backoff = AcceptBackoff::default();
    while !stop.load(Ordering::Acquire) {
        serving.reap_finished();
        let stream = match listener.accept() {
            Ok((stream, _)) => {
                // Winsock accepts inherit the listener's nonblocking mode.
                // Only accept is polled; framed authentication and connection
                // readers require blocking sockets with explicit deadlines.
                stream.set_nonblocking(false).map_err(|error| {
                    format!(
                        "Could not restore blocking mode for an accepted broker socket: {error}"
                    )
                })?;
                accept_backoff.accepted();
                stream
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            Err(error) => {
                // A persistent failure (handle exhaustion, WSAENOBUFS) used to
                // spin this loop at full CPU with nothing logged.
                if let Some(message) = accept_backoff.failed(&error) {
                    eprintln!("{message}");
                }
                thread::sleep(accept_backoff.delay());
                continue;
            }
        };
        let shutdown_socket = stream.try_clone().map_err(|error| {
            format!("Could not retain broker client shutdown ownership: {error}")
        })?;
        let Some(connection_id) = accept_client(&shared.clients) else {
            continue;
        };
        shared.observer_changed.notify_all();
        let bearer_token = Arc::clone(&bearer_token);
        let instance_id = Arc::clone(&instance_id);
        let connection_shared = Arc::clone(&shared);
        let connection_owner = connection_owner(&shared.clients, connection_id)
            .expect("newly accepted row is owned before worker admission");
        let thread_owner = Arc::clone(&connection_owner);
        let spawned = thread::Builder::new()
            .name("qc-broker-tcp-client".into())
            .spawn(move || {
                let _connection_owner = thread_owner;
                authenticate_and_serve_tcp_client(
                    stream,
                    &connection_shared,
                    connection_id,
                    &bearer_token,
                    &instance_id,
                    phase_events,
                );
            });
        match spawned {
            Ok(thread) => {
                serving.jobs.push(ServingJob {
                    thread,
                    socket: Some(shutdown_socket),
                    _connection_owner: Some(connection_owner),
                });
            }
            Err(error) => {
                eprintln!("Could not start a broker TCP client thread: {error}");
                // The closure that would decrement the count never ran.
                shared.detach_client(connection_id);
            }
        }
    }
    serving.finish()
}

#[cfg(test)]
#[path = "rpc_tray_hold_tests.rs"]
mod tray_hold_tests;

#[cfg(test)]
#[path = "rpc_resident_tests.rs"]
mod resident_tests;

/// Fixtures for tests outside this module (cli.rs): a broker handle with no
/// listener, no descriptor and a controller that never reaches hardware.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) struct RecoveryResource {
        pub(crate) resource: Arc<dyn Send + Sync>,
        pub(crate) armed: bool,
    }
    impl Drop for RecoveryResource {
        fn drop(&mut self) {
            if self.armed {
                static RETAINED: Mutex<Vec<Arc<dyn Send + Sync>>> = Mutex::new(Vec::new());
                RETAINED.lock_recover().push(Arc::clone(&self.resource));
                eprintln!("serving cleanup unproven; original fixture resource retained");
            }
        }
    }

    pub(crate) struct FixtureJob<T: Send + 'static> {
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<T>>,
        release_on_failure: Option<Arc<(Mutex<bool>, Condvar)>>,
        shutdown_on_failure: Option<TcpStream>,
    }
    impl<T: Send + 'static> FixtureJob<T> {
        pub(crate) fn is_finished(&self) -> bool {
            self.thread
                .as_ref()
                .is_none_or(crate::platform::process::thread_signalled)
        }
        pub(crate) fn spawn_plain(work: impl FnOnce() -> T + Send + 'static) -> Self {
            Self::spawn(move |_| work())
        }
        pub(crate) fn spawn_with_gate(
            gate: Arc<(Mutex<bool>, Condvar)>,
            work: impl FnOnce() -> T + Send + 'static,
        ) -> Self {
            let mut job = Self::spawn_plain(work);
            job.release_on_failure = Some(gate);
            job
        }
        pub(crate) fn spawn_with_socket(
            socket: TcpStream,
            work: impl FnOnce() -> T + Send + 'static,
        ) -> Self {
            let mut job = Self::spawn_plain(work);
            job.shutdown_on_failure = Some(socket);
            job
        }
        pub(crate) fn spawn(work: impl FnOnce(Arc<AtomicBool>) -> T + Send + 'static) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let owned_stop = Arc::clone(&stop);
            Self {
                stop,
                thread: Some(thread::spawn(move || work(owned_stop))),
                release_on_failure: None,
                shutdown_on_failure: None,
            }
        }
        pub(crate) fn join(self) -> thread::Result<T> {
            self.join_with_budget(Duration::from_secs(5))
        }
        pub(crate) fn join_with_budget(mut self, budget: Duration) -> thread::Result<T> {
            self.stop.store(true, Ordering::Release);
            let deadline = Instant::now() + budget;
            while !crate::platform::process::thread_signalled(self.thread.as_ref().unwrap()) {
                assert!(Instant::now() < deadline, "fixture job did not retire");
                thread::sleep(Duration::from_millis(5));
            }
            let result = self.thread.take().unwrap().join();
            if result.is_ok() {
                self.release_on_failure = None;
                self.shutdown_on_failure = None;
            }
            result
        }
    }
    impl<T: Send + 'static> Drop for FixtureJob<T> {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(socket) = self.shutdown_on_failure.take() {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            if let Some(gate) = self.release_on_failure.take() {
                let (released, changed) = &*gate;
                *released.lock_recover() = true;
                changed.notify_all();
            }
            let Some(job) = self.thread.take() else {
                return;
            };
            let deadline = Instant::now() + Duration::from_secs(6);
            while !crate::platform::process::thread_signalled(&job) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if crate::platform::process::thread_signalled(&job) {
                let _ = job.join();
            } else {
                static RETAINED: Mutex<Vec<Box<dyn std::any::Any + Send>>> = Mutex::new(Vec::new());
                RETAINED.lock_recover().push(Box::new(job));
                eprintln!(
                    "fixture job exit unacknowledged; exact handle and captured resources retained"
                );
            }
        }
    }

    pub(crate) fn handle() -> BrokerHandle {
        BrokerHandle(Arc::new(BrokerShared {
            controller: Arc::new(DeviceController::start_disconnected().expect("controller")),
            devices: Mutex::new(Vec::new()),
            performance_midi: Arc::new(PerformanceMidiWorkers::default()),
            request_lock: Arc::new(Mutex::new(())),
            clients: Arc::new(Mutex::new(ClientAccounting::default())),
            descriptor_path: std::env::temp_dir().join(format!(
                "qc-broker-support-unused-{}.json",
                generate_token(8)
            )),
            instance_id: generate_token(16),
            descriptor_claim: Arc::new(Mutex::new(None)),
            on_idle_shutdown: Arc::new(|| {}),
            observer_changed: Condvar::new(),
            background: background::Manager::default(),
            observer_actions: background::Manager::default(),
            demo: crate::demo_provider::Hub::default(),
            targets: crate::target::TargetRegistry::default(),
            observer_minute: Duration::from_secs(60),
        }))
    }

    /// An authenticated participant; `host` marks it as this process's host,
    /// exactly as a hello with the host token does.
    pub(crate) fn attach(handle: &BrokerHandle, host: bool) -> u64 {
        let id = accept_client(&handle.0.clients).expect("accepted");
        handle
            .0
            .mark_client_authenticated(id)
            .expect("not shutting down");
        handle
            .0
            .clients
            .lock_recover()
            .connections
            .get_mut(&id)
            .unwrap()
            .in_process_host = host;
        id
    }

    pub(crate) fn detach(handle: &BrokerHandle, id: u64) {
        let mut clients = handle.0.clients.lock_recover();
        clients.connections.remove(&id);
        clients.count -= 1;
    }

    pub(crate) fn idle_released(handle: &BrokerHandle) -> bool {
        handle.0.clients.lock_recover().idle_released
    }

    pub(crate) fn tray_hold(handle: &BrokerHandle) -> Option<IdlePolicy> {
        handle.0.clients.lock_recover().tray_hold
    }

    /// Run a tray regression assertion while retained device state is busy.
    pub(crate) fn with_session_lock(handle: &BrokerHandle, assertion: impl FnOnce()) {
        let _held = handle.0.controller.hold_session_lock_for_test();
        assertion();
    }

    /// Stop any timer the fixture armed from acting on it later.
    pub(crate) fn finish(handle: &BrokerHandle) {
        TEST_IDLE_RETRY_DELAY
            .lock_recover()
            .retain(|(key, _)| key != &handle.0.instance_id);
        handle.0.clients.lock_recover().shutting_down = true;
        handle.0.background.stop();
        handle.0.observer_actions.stop();
        handle.0.observer_changed.notify_all();
    }

    /// A real TCP broker on `descriptor_path`, in this process, on a
    /// disconnected controller that never reaches hardware, with lazy MIDI
    /// workers and no auto-connect. `configure` runs in the published hook,
    /// before the first connection can arrive. `exited` is set where the
    /// shipped broker would call `process::exit`.
    pub(crate) struct Served {
        pub(crate) handle: BrokerHandle,
        pub(crate) exited: Arc<AtomicBool>,
        job: Option<FixtureJob<()>>,
        resource: Arc<dyn Send + Sync>,
    }
    impl Drop for Served {
        fn drop(&mut self) {
            finish(&self.handle);
            let id = &self.handle.0.instance_id;
            TEST_POINTS.lock_recover().retain(|(key, _, _)| key != id);
            TEST_GRACE.lock_recover().retain(|(key, _)| key != id);
            TEST_MINUTE.lock_recover().retain(|(key, _)| key != id);
            drop(self.job.take());
            let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(2);
            let retired = || {
                Arc::strong_count(&self.handle.0) == 1
                    && Arc::strong_count(&self.handle.0.clients) == 1
                    && Arc::strong_count(&self.handle.0.request_lock) == 1
            };
            while !retired() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if !retired() {
                let _retain = RecoveryResource {
                    resource: Arc::clone(&self.resource),
                    armed: true,
                };
            }
        }
    }

    pub(crate) fn serve(
        descriptor_path: &Path,
        resource: Arc<dyn Send + Sync>,
        configure: impl FnOnce(&BrokerHandle) + Send + 'static,
    ) -> Served {
        let controller =
            Arc::new(DeviceController::start_disconnected().expect("controller starts"));
        let exited = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&exited);
        let on_exit: Arc<dyn Fn() + Send + Sync> =
            Arc::new(move || flag.store(true, Ordering::Release));
        let (published_tx, published) = mpsc::channel();
        let path = descriptor_path.to_path_buf();
        let owned_resource = Arc::clone(&resource);
        let job = FixtureJob::spawn(move |stop| {
            let mut recovery = RecoveryResource {
                resource: owned_resource,
                armed: true,
            };
            let result = serve_broker_tcp_full_with_stop(
                controller,
                false,
                path,
                on_exit,
                false,
                |_| {},
                move |handle| {
                    configure(&handle);
                    let _ = published_tx.send(handle);
                },
                stop,
            );
            if let Err(error) = result {
                panic!("the test broker did not serve: {error}");
            }
            recovery.armed = false;
        });
        let handle = published
            .recv_timeout(Duration::from_secs(8))
            .expect("the test broker publishes promptly");
        Served {
            handle,
            exited,
            job: Some(job),
            resource,
        }
    }

    /// A shorter grace for every timer this broker arms from now on.
    pub(crate) fn set_grace(handle: &BrokerHandle, grace: Duration) {
        {
            let mut clocks = TEST_GRACE.lock_recover();
            clocks.retain(|(id, _)| id != &handle.0.instance_id);
            clocks.push((handle.0.instance_id.clone(), grace));
        }
        // Real serving has already admitted the initial default-five-second
        // timer before its publication hook. Rebase only this fixture's still
        // queued clock, not production policy or an already-admitted callback.
        handle
            .0
            .background
            .rebase_pending_deadline_for_test([1, 0, 0], Instant::now() + grace);
    }

    /// A shorter idle-policy minute for this broker from now on.
    pub(crate) fn set_policy_minute(handle: &BrokerHandle, minute: Duration) {
        TEST_MINUTE
            .lock_recover()
            .push((handle.0.instance_id.clone(), minute));
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn set_idle_release_retry_delay(handle: &BrokerHandle, delay: Duration) {
        TEST_IDLE_RETRY_DELAY
            .lock_recover()
            .push((handle.0.instance_id.clone(), delay));
    }

    /// The idle origins a status read must never move.
    pub(crate) fn idle_origins(handle: &BrokerHandle) -> (Option<Instant>, Option<Instant>) {
        let clients = handle.0.clients.lock_recover();
        (clients.tray_idle_since, clients.observer_idle_since)
    }

    /// Raw accepted connections, control ones included.
    pub(crate) fn connection_count(handle: &BrokerHandle) -> usize {
        handle.0.clients.lock_recover().count
    }

    pub(crate) fn instance_id(handle: &BrokerHandle) -> String {
        handle.0.instance_id.clone()
    }

    pub(crate) fn own_created_at() -> Option<u64> {
        own_process_created_at()
    }
}

#[cfg(test)]
mod tcp_tests {
    use super::*;
    use crate::worker::DecodedStateFrame;
    use quadcortex::state::StateUpdate;

    fn demo_rpc(shared: &BrokerShared, owner: u64, method: &str, params: Value) -> Value {
        let mut request = rpc_request(method);
        request.params = params;
        dispatch_broker_client_request(shared, Some(owner), request)
    }

    fn demo_owner(shared: &Arc<BrokerShared>) -> u64 {
        let owner = accept_client(&shared.clients).unwrap();
        shared
            .mark_client_authenticated_as(owner, AuthenticatedAs::DemoProvider, None)
            .unwrap();
        owner
    }

    #[test]
    fn demo_provider_is_not_a_participant_or_observer_and_cannot_escalate() {
        let shared = Arc::new(broker_shared_for_test());
        let owner = demo_owner(&shared);
        {
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.participants(), 0);
            assert_eq!(clients.observers(), 0);
            assert!(clients.authenticated_rows().is_empty());
        }
        for method in [
            "device.snapshot",
            "broker.hello",
            "broker.controlRelease",
            "broker.shutdown",
        ] {
            assert!(demo_rpc(&shared, owner, method, json!({}))
                .get("error")
                .is_some());
        }
        let registered = demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"UI-instance"}),
        );
        assert_eq!(registered["result"]["target"]["source"], "demo");
        assert_ne!(registered["result"]["providerSessionId"], "UI-instance");
        let status = demo_rpc(&shared, owner, "system.status", json!({}));
        assert!(status.get("error").is_some());
        let ordinary_status = rpc_request("system.status");
        let status = dispatch_broker_request(&shared, ordinary_status);
        assert_eq!(status["result"]["targetReady"], true);
        assert_eq!(status["result"]["usbDiagnostics"]["connected"], false);
        let mut resource = rpc_request("system.status");
        resource.origin = Some(RESOURCE_READ_ORIGIN.into());
        let resource = dispatch_broker_request(&shared, resource);
        assert_eq!(
            resource["result"]["brokerTarget"],
            status["result"]["brokerTarget"]
        );
        assert_eq!(resource["result"]["targetReady"], true);
        assert_eq!(resource["result"]["usbDiagnostics"]["connected"], false);
        let mut gui = rpc_request("system.status");
        gui.physical_only = true;
        let gui = dispatch_broker_request(&shared, gui);
        assert_eq!(gui["result"]["targetReady"], false);
        assert!(gui["result"].get("brokerTarget").is_none());
        assert!(matches!(
            BrokerHandle(shared.clone()).device_readiness(),
            crate::resident::DeviceReadiness::ReadyTarget {
                usb_connected: false,
                ..
            }
        ));
        shared
            .clients
            .lock_recover()
            .connections
            .get_mut(&owner)
            .unwrap()
            .demo_provider_closed = true;
        assert!(demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"late"})
        )
        .get("error")
        .is_some());
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn demo_inflight_hotplug_returns_to_the_same_provider_without_replaying() {
        let shared = Arc::new(broker_shared_for_test());
        let owner = demo_owner(&shared);
        let registered = demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"UI-instance"}),
        );
        let session = registered["result"]["providerSessionId"].clone();
        let request_shared = shared.clone();
        let request = test_support::FixtureJob::spawn_plain(move || {
            dispatch_broker_request(&request_shared, rpc_request("device.snapshot"))
        });
        // Holding the normal broker request lock must not block the reverse
        // provider Next/Admit/Reply path needed to complete an MCP request.
        let held = shared.request_lock.lock_recover();
        let next = demo_rpc(&shared, owner, "broker.demoNext", json!({}))["result"].clone();
        let identity = json!({"target":next["target"],"sequence":next["sequence"]});
        assert_eq!(
            demo_rpc(&shared, owner, "broker.demoAdmit", identity.clone())["result"]["admitted"],
            true
        );
        {
            let mut status = shared.controller.hold_status_lock_for_test();
            status.connected = true;
            status.synchronized = true;
            status.phase = "ready".into();
        }
        let stale =
            json!({"target":next["target"],"sequence":next["sequence"],"result":{"name":"old"}});
        assert_eq!(
            demo_rpc(&shared, owner, "broker.demoReply", stale)["result"]["accepted"],
            false
        );
        assert!(request.join().unwrap().get("error").is_some());
        {
            let mut status = shared.controller.hold_status_lock_for_test();
            status.connected = false;
            status.synchronized = false;
            status.phase = "disconnected".into();
        }
        shared.refresh_targets();
        let target = shared.targets.identity().unwrap();
        assert_eq!(json!(target.session_id), session);
        assert_ne!(json!(target.generation), next["target"]["generation"]);
        let request_shared = shared.clone();
        let request = test_support::FixtureJob::spawn_plain(move || {
            dispatch_broker_request(&request_shared, rpc_request("device.snapshot"))
        });
        let next = demo_rpc(&shared, owner, "broker.demoNext", json!({}))["result"].clone();
        assert_eq!(
            demo_rpc(
                &shared,
                owner,
                "broker.demoAdmit",
                json!({"target":next["target"],"sequence":next["sequence"]})
            )["result"]["admitted"],
            true
        );
        assert_eq!(
            demo_rpc(
                &shared,
                owner,
                "broker.demoReply",
                json!({"target":next["target"],"sequence":next["sequence"],"result":{"name":"same UI store"}})
            )["result"]["accepted"],
            true
        );
        assert_eq!(request.join().unwrap()["result"]["name"], "same UI store");
        drop(held);
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn demo_requests_are_fenced_before_the_tcp_queue_and_on_provider_replacement() {
        let shared = Arc::new(broker_shared_for_test());
        let owner = demo_owner(&shared);
        let first = demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"same-UI"}),
        );
        let mut request = rpc_request("device.snapshot");
        shared.capture_request_target(None, &mut request);
        assert_eq!(
            request.expected_broker_target.as_ref().unwrap().source,
            crate::target::TargetSource::Demo
        );
        demo_rpc(&shared, owner, "broker.demoWithdraw", json!({}));
        let replacement = demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"same-UI"}),
        );
        assert_ne!(
            first["result"]["providerSessionId"],
            replacement["result"]["providerSessionId"]
        );
        let refused = dispatch_broker_request(&shared, request);
        assert!(refused.get("error").is_some());
        assert!(shared.demo.next(owner, Duration::ZERO).unwrap().is_none());
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    fn demo_ready_controller(
        key: &str,
        epoch: u64,
    ) -> (Arc<DeviceController>, crate::worker::TestCommands) {
        let (controller, commands) = DeviceController::live_session_for_test();
        let controller = Arc::new(controller);
        assert!(controller.claim_device(key));
        {
            let mut state = controller.hold_status_lock_for_test();
            state.connected = true;
            state.synchronized = true;
            state.phase = "ready".into();
            state.session_epoch = epoch;
            state.messages_received = epoch;
        }
        (controller, commands)
    }

    #[test]
    fn demo_default_offline_routes_to_unique_known_qc_and_keeps_identity_opaque() {
        let shared = Arc::new(broker_shared_for_test());
        let (physical, commands) = demo_ready_controller("linux:serial:PRIVATE_SERIAL", 37);
        shared.devices.lock_recover().push(physical.clone());
        let participant = accept_client(&shared.clients).unwrap();
        shared.mark_client_authenticated(participant).unwrap();
        let mut request = rpc_request("device.stateEvents");
        assert!(shared.capture_request_target(Some(participant), &mut request));
        assert!(Arc::ptr_eq(
            &shared.controller_for(Some(participant)),
            &physical
        ));
        assert_eq!(shared.participant_count_for(Some(participant)), 1);
        let expected = request.expected_broker_target.clone().unwrap();
        let response = dispatch_broker_client_request(&shared, Some(participant), request);
        // This is the actual device method's journal, not only its label or
        // a status projection. The old default-controller route returns a
        // different stream and fails this assertion.
        assert_eq!(response["result"]["streamId"], physical.target_stream_id());
        assert_ne!(
            response["result"]["streamId"],
            shared.controller.target_stream_id()
        );
        assert_eq!(shared.targets.identity().unwrap(), expected);
        shared.refresh_targets();
        assert_eq!(shared.targets.identity().unwrap(), expected);
        assert!(!serde_json::to_string(&expected)
            .unwrap()
            .contains("PRIVATE_SERIAL"));
        let status = demo_rpc(&shared, participant, "system.status", json!({}));
        assert_eq!(status["result"]["usbDiagnostics"]["messagesReceived"], 37);
        assert_eq!(status["result"]["brokerTarget"], json!(expected));
        assert!(!status.to_string().contains("PRIVATE_SERIAL"));
        assert!(
            matches!(BrokerHandle(shared.clone()).device_readiness(), crate::resident::DeviceReadiness::ReadyTarget { usb_connected: true, target } if target == expected)
        );
        assert_eq!(
            commands.device_work(),
            0,
            "cached reads must not create USB work"
        );
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn demo_two_ready_qcs_refuse_noncanonical_mcp_without_mislabeling_physical_gui() {
        let shared = Arc::new(broker_shared_for_test());
        {
            let mut state = shared.controller.hold_status_lock_for_test();
            state.connected = true;
            state.synchronized = true;
            state.phase = "ready".into();
            state.session_epoch = 11;
        }
        let (other, commands) = demo_ready_controller("other:serial:PRIVATE_SERIAL", 42);
        shared.devices.lock_recover().push(other.clone());
        let participant = accept_client(&shared.clients).unwrap();
        shared.mark_client_authenticated(participant).unwrap();
        shared
            .clients
            .lock_recover()
            .connections
            .get_mut(&participant)
            .unwrap()
            .selected_device = Some(other.clone());
        let refused = demo_rpc(&shared, participant, "device.stateEvents", json!({}));
        assert!(refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("noncanonical"));
        let mut gui = rpc_request("system.status");
        gui.physical_only = true;
        let status = dispatch_broker_client_request(&shared, Some(participant), gui);
        assert_eq!(status["result"]["usbDiagnostics"]["messagesReceived"], 42);
        assert!(
            status["result"].get("brokerTarget").is_none(),
            "must not label B with canonical A's identity"
        );
        assert_eq!(commands.device_work(), 0);
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn demo_is_not_selected_when_multiple_nondefault_qcs_are_ready() {
        let shared = Arc::new(broker_shared_for_test());
        let owner = demo_owner(&shared);
        demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"UI"}),
        );
        let (first, _first_commands) = demo_ready_controller("A", 1);
        let (second, _second_commands) = demo_ready_controller("B", 2);
        shared.devices.lock_recover().extend([first, second]);
        let refused = dispatch_broker_request(&shared, rpc_request("device.stateEvents"));
        assert!(refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Multiple usable"));
        assert!(shared.targets.identity().is_none());
        assert!(
            shared.demo.session().is_some(),
            "ambiguity must not withdraw the live provider"
        );
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    fn demo_mark_primary_ready(shared: &BrokerShared, epoch: u64) {
        let mut state = shared.controller.hold_status_lock_for_test();
        state.connected = true;
        state.synchronized = true;
        state.phase = "ready".into();
        state.session_epoch = epoch;
    }

    #[test]
    fn demo_target_readiness_recovers_poison_without_using_the_device_tree() {
        let shared = Arc::new(broker_shared_for_test());
        demo_mark_primary_ready(&shared, 17);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = shared.controller.hold_status_lock_for_test();
            panic!("injected status writer unwind");
        }));
        assert!(poisoned.is_err());
        assert_eq!(
            shared.controller.try_target_readiness(),
            Some((true, true, 17))
        );
        shared.refresh_targets();
        assert_eq!(
            shared.targets.identity().unwrap().source,
            crate::target::TargetSource::Physical
        );
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn demo_lifecycle_reset_proof_allows_only_the_same_native_controller_and_key() {
        let shared = Arc::new(broker_shared_for_test());
        assert!(shared.controller.claim_device("PRIVATE_SERIAL_A"));
        demo_mark_primary_ready(&shared, 1);
        shared.refresh_targets();
        let from = shared.targets.identity().unwrap();
        let proof = shared
            .capture_lifecycle(
                &shared.controller,
                generated_gateway::rpc::RESET_SESSION,
                &from,
            )
            .unwrap();
        assert!(shared
            .capture_lifecycle(&shared.controller, "device.setTempo", &from)
            .is_none());
        demo_mark_primary_ready(&shared, 2);
        let success = json!({"jsonrpc":"2.0","id":1,"result":{"phase":"ready","detail":"reset complete","demo":false,"source":"device"}});
        let result = shared.finish_lifecycle(Some(&proof), Some(&from), success.clone());
        let ack = &result["result"]["brokerTargetTransition"];
        assert_eq!(ack["method"], "device.resetSession");
        assert_eq!(ack["from"], json!(from));
        assert_eq!(ack["to"], json!(shared.targets.identity().unwrap()));
        assert_eq!(ack["verification"], "same_native_controller");
        assert!(!result.to_string().contains("PRIVATE_SERIAL"));
        assert!(shared
            .finish_lifecycle(None, Some(&from), success.clone())
            .get("error")
            .is_some());
        let (other, _commands) = demo_ready_controller("PRIVATE_SERIAL_B", 9);
        shared.devices.lock_recover().push(other);
        {
            let mut state = shared.controller.hold_status_lock_for_test();
            state.connected = false;
            state.synchronized = false;
        }
        let refused = shared.finish_lifecycle(Some(&proof), Some(&from), success);
        assert!(
            refused.get("error").is_some(),
            "must not acknowledge a reset on another controller"
        );
        assert!(refused["result"].get("brokerTargetTransition").is_none());
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn demo_lifecycle_disconnect_ack_requires_verified_close_and_never_another_physical() {
        for with_demo in [false, true] {
            let shared = Arc::new(broker_shared_for_test());
            assert!(shared.controller.claim_device("PRIVATE_SERIAL_A"));
            demo_mark_primary_ready(&shared, 1);
            if with_demo {
                let owner = demo_owner(&shared);
                demo_rpc(
                    &shared,
                    owner,
                    "broker.demoRegister",
                    json!({"sessionId":"UI"}),
                );
            }
            shared.refresh_targets();
            let from = shared.targets.identity().unwrap();
            let proof = shared
                .capture_lifecycle(
                    &shared.controller,
                    generated_gateway::rpc::DISCONNECT,
                    &from,
                )
                .unwrap();
            {
                let mut state = shared.controller.hold_status_lock_for_test();
                state.connected = false;
                state.synchronized = false;
                state.phase = "disconnected".into();
            }
            let success = json!({"jsonrpc":"2.0","id":1,"result":{"phase":"disconnected","detail":"closed","demo":true,"source":"device"}});
            let result = shared.finish_lifecycle(Some(&proof), Some(&from), success.clone());
            let ack = &result["result"]["brokerTargetTransition"];
            assert_eq!(ack["verification"], "closed_native_controller");
            assert_eq!(ack["from"], json!(from));
            if with_demo {
                assert_eq!(ack["to"]["source"], "demo");
            } else {
                assert!(ack["to"].is_null());
            }
            let (other, _commands) = demo_ready_controller("PRIVATE_SERIAL_B", 9);
            shared.devices.lock_recover().push(other);
            assert!(shared
                .finish_lifecycle(Some(&proof), Some(&from), success)
                .get("error")
                .is_some());
            crate::rpc::test_support::finish(&BrokerHandle(shared));
        }
    }

    struct ReleaseGate(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            let (released, changed) = &*self.0;
            *released.lock_recover() = true;
            changed.notify_all();
        }
    }
    struct ShutdownSocket(TcpStream);
    impl Drop for ShutdownSocket {
        fn drop(&mut self) {
            let _ = self.0.shutdown(std::net::Shutdown::Both);
        }
    }

    #[test]
    fn demo_publication_readiness_distinguishes_contention_from_unavailable() {
        use crate::resident::DeviceReadiness;
        let shared = Arc::new(broker_shared_for_test());
        let owner = demo_owner(&shared);
        let registered = demo_rpc(
            &shared,
            owner,
            "broker.demoRegister",
            json!({"sessionId":"visible-demo"}),
        );
        let target = registered["result"]["target"].clone();
        let handle = BrokerHandle(shared.clone());
        assert!(matches!(
            handle.device_readiness(),
            DeviceReadiness::ReadyTarget { .. }
        ));
        {
            let _held = shared.controller.hold_status_lock_for_test();
            let started = Instant::now();
            assert_eq!(handle.device_readiness(), DeviceReadiness::Unknown);
            assert!(started.elapsed() < Duration::from_millis(100));
            assert_eq!(json!(shared.targets.identity()), target);
            assert!(
                dispatch_broker_request(&shared, rpc_request("device.setTempo"))
                    .get("error")
                    .is_some()
            );
        }
        {
            let _held = shared.devices.lock_recover();
            let started = Instant::now();
            assert_eq!(handle.device_readiness(), DeviceReadiness::Unknown);
            assert!(started.elapsed() < Duration::from_millis(100));
            assert_eq!(json!(shared.targets.identity()), target);
        }
        let (first, first_commands) = demo_ready_controller("native-A", 11);
        let (second, second_commands) = demo_ready_controller("native-B", 12);
        shared
            .devices
            .lock_recover()
            .extend([first.clone(), second.clone()]);
        {
            let _held = first.hold_status_lock_for_test();
            assert_eq!(handle.device_readiness(), DeviceReadiness::Unknown);
            assert_eq!(json!(shared.targets.identity()), target);
        }
        let status = dispatch_broker_request(&shared, rpc_request("system.status"));
        assert_eq!(status["result"]["targetReady"], false);
        assert!(status["result"].get("brokerTarget").is_none());
        assert!(status.get("brokerTarget").is_none());
        assert!(matches!(
            handle.device_readiness(),
            DeviceReadiness::Unavailable(_)
        ));
        assert!(
            shared.targets.identity().is_none(),
            "ambiguity must not keep demo ready"
        );
        shared.demo.withdraw(owner);
        shared.devices.lock_recover().clear();
        shared
            .controller
            .set_connection_for_test(true, "handshaking");
        assert_eq!(
            handle.device_readiness(),
            DeviceReadiness::NotReady {
                phase: "handshaking".into()
            }
        );
        shared
            .controller
            .set_connection_for_test(false, "disconnected");
        assert_eq!(
            handle.device_readiness(),
            DeviceReadiness::NotReady {
                phase: "disconnected".into()
            }
        );
        shared.clients.lock_recover().shutting_down = true;
        {
            let _held = shared.controller.hold_status_lock_for_test();
            assert!(matches!(
                handle.device_readiness(),
                DeviceReadiness::Unavailable(_)
            ));
        }
        shared.clients.lock_recover().shutting_down = false;
        assert_eq!(first_commands.device_work(), 0);
        assert_eq!(second_commands.device_work(), 0);
        crate::rpc::test_support::finish(&handle);
    }

    #[test]
    fn demo_status_attach_survives_busy_readiness_without_claiming_a_target() {
        let shared = Arc::new(broker_shared_for_test());
        let expected = crate::target::TargetIdentity {
            source: crate::target::TargetSource::Physical,
            session_id: "unverified".into(),
            generation: 1,
        };
        let held = shared.controller.hold_status_lock_for_test();
        let started = Instant::now();
        let response = dispatch_broker_request(&shared, rpc_request("system.status"));
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(response["result"]["gatewayAvailable"], true);
        assert_eq!(
            response["result"]["gatewayApiVersion"],
            generated_gateway::API_VERSION
        );
        assert!(response["result"]["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("expectedBrokerTarget")));
        assert!(response["result"].get("usbDiagnostics").is_none());
        assert!(response["result"].get("targetReady").is_none());
        assert!(response["result"].get("brokerTarget").is_none());
        assert!(response.get("brokerTarget").is_none());
        let mut guarded = rpc_request("system.status");
        guarded.expected_broker_target = Some(expected);
        assert_eq!(
            dispatch_broker_request(&shared, guarded)["error"]["code"],
            -32002
        );
        let effect = dispatch_broker_request(&shared, rpc_request("device.setTempo"));
        assert_eq!(effect["error"]["code"], -32002);
        let mut malformed = rpc_request("system.status");
        malformed.id = json!(0);
        assert_eq!(
            dispatch_broker_request(&shared, malformed)["error"]["code"],
            -32600
        );
        drop(held);
        let retained = shared.controller.hold_session_lock_for_test();
        let mut resource = rpc_request("system.status");
        resource.origin = Some(RESOURCE_READ_ORIGIN.into());
        let response = dispatch_broker_request(&shared, resource);
        assert_eq!(response["result"]["gatewayAvailable"], true);
        assert!(response["result"].get("usbDiagnostics").is_none());
        drop(retained);
        crate::rpc::test_support::finish(&BrokerHandle(shared));
    }

    #[test]
    fn tray_status_never_waits_for_busy_device_state() {
        let handle = test_support::handle();
        {
            let mut status = handle.0.controller.hold_status_lock_for_test();
            status.phase = "ready".into();
            status.connected = true;
            status.synchronized = true;
            status.active_preset_name = Some("Cached preset".into());
        }

        let owner = handle.0.controller.hold_session_lock_for_test();
        let started = Instant::now();
        let status = handle
            .try_tray_status()
            .expect("the cached status lock is free");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "tray status waited behind the retained device state"
        );
        assert_eq!(status.phase, "ready");
        assert_eq!(status.active_preset_name.as_deref(), Some("Cached preset"));
        drop(owner);

        let status_lock = handle.0.controller.hold_status_lock_for_test();
        let started = Instant::now();
        assert!(handle.try_tray_status().is_none());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "tray status waited behind a worker status update"
        );
        drop(status_lock);
        test_support::finish(&handle);
    }

    #[test]
    fn tcp_phase_notifications_publish_progress_detail_without_a_phase_transition() {
        let shared = Arc::new(broker_shared_for_test());
        {
            let mut status = shared.controller.hold_status_lock_for_test();
            status.phase = "syncing".into();
            status.detail = "Opening session".into();
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let id = accept_client(&shared.clients).unwrap();
        let connection_shared = Arc::clone(&shared);
        let job = test_support::FixtureJob::spawn_plain(move || {
            serve_broker_tcp_connection(server, &connection_shared, id, true);
            connection_shared.detach_client(id);
        });
        let shutdown = ShutdownSocket(client.try_clone().unwrap());
        let first = read_notification(&mut client, "device.phase").expect("first progress");
        assert_eq!(first["params"]["phase"], "syncing");
        assert_eq!(first["params"]["detail"], "Opening session");
        shared.controller.hold_status_lock_for_test().detail = "Reading preset library".into();
        let second = read_notification(&mut client, "device.phase").expect("same-phase progress");
        assert_eq!(second["params"]["phase"], "syncing");
        assert_eq!(second["params"]["detail"], "Reading preset library");
        drop(shutdown);
        job.join().unwrap();
    }

    #[test]
    fn unread_tcp_response_is_bounded_and_socket_shutdown_retires_the_owned_writer_on_unwind() {
        fn bounded_pair(listener: &TcpListener) -> (TcpStream, TcpStream) {
            // Set receive capacity before the handshake, rather than racing
            // Windows loopback's delayed ACKs and auto-grown receive window.
            let socket =
                socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
            socket.set_recv_buffer_size(4096).unwrap();
            socket
                .connect(&listener.local_addr().unwrap().into())
                .unwrap();
            // BSD kernels may round up well beyond the requested 4KiB. The
            // effective buffer must still be far below our unchanged 8MiB frame.
            assert!(socket.recv_buffer_size().unwrap() <= 2 * 1024 * 1024);
            let client: TcpStream = socket.into();
            let (server, _) = listener.accept().unwrap();
            let server_socket = socket2::SockRef::from(&server);
            // Winsock can complete a blocking send by accepting its buffer
            // even when it exceeds SO_SNDBUF. Disable that test-only staging
            // so completion requires peer acknowledgement, not admission.
            // https://learn.microsoft.com/en-us/windows/win32/winsock/tcp-ip-specific-issues-2
            #[cfg(windows)]
            {
                server_socket.set_send_buffer_size(0).unwrap();
                assert_eq!(server_socket.send_buffer_size().unwrap(), 0);
            }
            #[cfg(not(windows))]
            server_socket.set_send_buffer_size(4096).unwrap();
            assert!(server_socket.send_buffer_size().unwrap() <= 2 * 1024 * 1024);
            (client, server)
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let (client, mut server) = bounded_pair(&listener);
        let response = json!({"id":1,"result":"x".repeat(8 * 1024 * 1024)});
        let writer = test_support::FixtureJob::spawn_plain(move || {
            let start = Instant::now();
            let result = write_tcp_response(&mut server, &response);
            (result, start.elapsed())
        });
        let shutdown = ShutdownSocket(client.try_clone().unwrap());
        let (result, elapsed) = writer.join_with_budget(Duration::from_secs(7)).unwrap();
        assert!(
            result.is_err(),
            "an unread multi-megabyte frame cannot complete"
        );
        assert!(elapsed < Duration::from_secs(6), "{elapsed:?}");
        drop(shutdown);
        drop(client);

        let (client, mut server) = bounded_pair(&listener);
        let finished = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&finished);
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let response = json!({"id":2,"result":"x".repeat(8 * 1024 * 1024)});
            let _writer = test_support::FixtureJob::spawn_plain(move || {
                started_tx.send(()).unwrap();
                let _ = write_tcp_response(&mut server, &response);
                done.store(true, Ordering::Release);
                let _ = finished_tx.send(());
            });
            let _shutdown = ShutdownSocket(client);
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(
                finished_rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout),
                "the owned writer must still be unfinished before socket shutdown"
            );
            panic!("injected reader assertion failure");
        }));
        let panic = unwind.expect_err("the injected reader failure must unwind");
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"injected reader assertion failure"),
            "an earlier fixture assertion must not masquerade as the injected unwind"
        );
        assert!(
            finished.load(Ordering::Acquire),
            "writer retired before unwind returned"
        );
    }

    #[test]
    fn a_nonblocking_listener_preserves_delayed_authentication_and_persistent_requests() {
        let path = scratch_descriptor_path("delayed-persistent-client");
        let serve = start_broker_serve_for_test(&path);
        let mut client = TcpStream::connect(("127.0.0.1", serve.port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let _shutdown = ShutdownSocket(client.try_clone().unwrap());
        // The accepted connection must remain blocking while no request byte
        // is present, even though the listener itself polls nonblocking.
        thread::sleep(Duration::from_millis(150));
        write_response(
            &mut client,
            &json!({"method":"broker.authenticate","params":{"token":serve.token},"id":0}),
        )
        .unwrap();
        assert!(read_response_within(&mut client, Duration::from_secs(3))
            .unwrap()
            .get("result")
            .is_some());
        for _ in 0..2 {
            thread::sleep(Duration::from_millis(150));
            let reply = send_request(&mut client, "broker.clientCount", json!({}));
            assert_eq!(
                reply["result"]["clients"], 1,
                "persistent participant remains attached"
            );
        }
    }

    #[test]
    fn tcp_fixture_unwind_stops_the_real_listener_before_retiring_its_exclusive_root() {
        let path = scratch_descriptor_path("serve-unwind");
        let directory = path.root.0.clone();
        let mut address = None;
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let serve = start_broker_serve_for_test(&path);
            address = Some(std::net::SocketAddr::from(([127, 0, 0, 1], serve.port)));
            panic!("injected fixture assertion failure");
        }));
        assert!(failure.is_err());
        assert!(TcpStream::connect_timeout(&address.unwrap(), Duration::from_millis(200)).is_err());
        drop(path);
        assert!(
            !directory.exists(),
            "retired server cannot retain fixture bytes"
        );
    }

    #[test]
    fn descriptor_reader_unwind_signals_and_joins_before_retiring_its_root() {
        let path = scratch_descriptor_path("reader-unwind");
        let directory = path.root.0.clone();
        let (entered, ready) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&finished);
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _reader = test_support::FixtureJob::spawn(move |stop| {
                let _path = path;
                entered.send(()).unwrap();
                while !stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                done.store(true, Ordering::Release);
            });
            ready.recv_timeout(Duration::from_secs(1)).unwrap();
            panic!("injected refresh assertion failure");
        }));
        assert!(failure.is_err());
        assert!(finished.load(Ordering::Acquire));
        assert!(!directory.exists());
    }

    #[test]
    fn published_callback_unwind_retires_the_common_server_before_releasing_its_claim() {
        let path = scratch_descriptor_path("published-callback-unwind");
        let directory = path.root.0.clone();
        let mut recovery = test_support::RecoveryResource {
            resource: path.root.clone(),
            armed: true,
        };
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = serve_broker_tcp_full_with_stop(
                Arc::new(DeviceController::start_disconnected().unwrap()),
                false,
                path.to_path_buf(),
                Arc::new(|| {}),
                false,
                |_| {},
                |_| panic!("injected publication callback failure"),
                Arc::new(AtomicBool::new(false)),
            );
        }));
        assert!(failure.is_err());
        // A live retained publisher must prevent this exact owner acquisition.
        let claim =
            try_claim_descriptor_slot(&path).expect("callback unwind retires publisher ownership");
        claim.release().unwrap();
        recovery.armed = false;
        drop(recovery);
        drop(path);
        assert!(!directory.exists());
    }

    fn descriptor_fixture() -> BrokerDescriptor {
        BrokerDescriptor {
            port: 54123,
            bearer_token: "credential".repeat(1024),
            instance_id: "atomic-publication".into(),
            process_id: std::process::id(),
            process_created_at: own_process_created_at(),
            published_at_unix_ms: unix_time_ms(),
            gateway_api_version: generated_gateway::API_VERSION,
        }
    }

    #[test]
    fn a_reader_never_observes_an_empty_or_partial_descriptor_during_a_refresh() {
        let (_scratch, path) = guarded_scratch_descriptor_path("atomic-refresh");
        let mut claim = try_claim_descriptor_slot(&path).expect("claim");
        let mut descriptor = descriptor_fixture();
        write_descriptor(&mut claim, &descriptor).unwrap();
        let reader_path = path.clone();
        let (ready, wait_ready) = mpsc::channel();
        let reader = test_support::FixtureJob::spawn(move |reader_stop| {
            let mut reads = 0;
            loop {
                let bytes = std::fs::read(&reader_path).expect("published document stays readable");
                let value: Value =
                    serde_json::from_slice(&bytes).expect("complete JSON on every read");
                assert_eq!(value["instanceId"], "atomic-publication");
                assert_eq!(value["bearerToken"], "credential".repeat(1024));
                assert_eq!(value["port"], 54123);
                reads += 1;
                if reads == 10 {
                    ready.send(()).unwrap();
                }
                if reader_stop.load(Ordering::Acquire) {
                    return reads;
                }
            }
        });
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        // Keep a reader open across replacements too. Windows permits rename
        // with read handles that share deletion, including std::fs::File.
        let mut old_snapshot = std::fs::File::open(&path).unwrap();
        for version in 1..=120 {
            descriptor.published_at_unix_ms = version;
            write_descriptor(&mut claim, &descriptor).unwrap();
            assert!(
                try_claim_descriptor_slot(&path).is_err(),
                "refresh retains election ownership"
            );
        }
        assert!(reader.join().unwrap() >= 10);
        let mut old_bytes = Vec::new();
        old_snapshot.read_to_end(&mut old_bytes).unwrap();
        let old: Value = serde_json::from_slice(&old_bytes).unwrap();
        assert!(old["publishedAtUnixMs"].as_u64().unwrap() > 120);
        drop(old_snapshot);
        let final_value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(final_value["publishedAtUnixMs"], 120);
        claim.release().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn device_broker_json_is_created_with_an_owner_only_dacl() {
        let (_scratch, path) = guarded_scratch_descriptor_path("owner-only-dacl");
        let mut claim = try_claim_descriptor_slot(&path).expect("claim");
        let mut descriptor = descriptor_fixture();
        write_descriptor(&mut claim, &descriptor).unwrap();
        assert!(qc_owner_only_file::is_owner_only(&path).unwrap());
        descriptor.published_at_unix_ms += 1;
        write_descriptor(&mut claim, &descriptor).unwrap();
        assert!(qc_owner_only_file::is_owner_only(&path).unwrap());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["bearerToken"],
            descriptor.bearer_token
        );
        claim.release().unwrap();
    }

    #[test]
    fn descriptor_shutdown_prevents_a_later_refresh_from_republishing() {
        let (_scratch, path) = guarded_scratch_descriptor_path("shutdown-refresh");
        let mut claim = try_claim_descriptor_slot(&path).expect("claim");
        write_descriptor(&mut claim, &descriptor_fixture()).unwrap();
        let shared = BrokerShared {
            descriptor_path: path.to_path_buf(),
            descriptor_claim: Arc::new(Mutex::new(Some(claim))),
            ..broker_shared_for_test()
        };
        shared.finish_shutdown();
        assert!(shared.descriptor_claim.lock_recover().is_none());
        assert!(!path.exists());
        try_claim_descriptor_slot(&path)
            .expect("shutdown frees election immediately")
            .release()
            .unwrap();
    }

    #[test]
    fn a_refused_claim_names_the_broker_that_holds_the_slot() {
        let (_scratch, path) = guarded_scratch_descriptor_path("refusal");
        let mut claim = try_claim_descriptor_slot(&path).expect("claim");
        write_descriptor(&mut claim, &descriptor_fixture()).unwrap();
        let message = descriptor_claim_refusal(&path);
        assert!(
            message.contains(&format!("process {}", std::process::id())),
            "{message}"
        );
        assert!(!message.contains("credential"), "the token never leaks");
        claim.release().unwrap();
        assert!(descriptor_claim_refusal(&path).contains("a broker is already"));
    }

    /// A `--resident` start that wins the election but finds the descriptor
    /// slot held refuses to serve. The refusal comes back as an error, so its
    /// publication hook, which owns the election lock, is dropped and that
    /// lock released before the process exits 1; `process::exit` there left
    /// the lock for the app to wait out as stale. The holder's claim stays.
    #[test]
    fn a_refused_claim_releases_the_resident_starts_own_election_lock() {
        let path = scratch_descriptor_path("refusal-election");
        let _scratch = OwnedScratch(path.clone());
        let election = crate::resident::election_path(&path);
        let crate::resident::Election::Serve(guard) =
            crate::resident::elect(&path, Duration::from_secs(1)).expect("election runs")
        else {
            panic!("no descriptor and an unheld election lock: this start serves");
        };
        assert!(election.exists(), "the start holds its election lock");
        // Another broker claims the slot between the election and the claim.
        let mut holder = try_claim_descriptor_slot(&path).expect("holder claims");
        write_descriptor(&mut holder, &descriptor_fixture()).unwrap();
        let published = Arc::new(AtomicBool::new(false));
        let reported = Arc::clone(&published);
        let result = serve_broker_tcp_full(
            Arc::new(DeviceController::start_disconnected().expect("controller")),
            false,
            path.to_path_buf(),
            Arc::new(|| {}),
            false,
            |_| {},
            move |_| {
                drop(guard);
                reported.store(true, Ordering::SeqCst);
            },
        );
        let error = result.expect_err("a held slot refuses this broker");
        assert!(error.contains("a broker"), "{error}");
        assert!(!published.load(Ordering::SeqCst), "never published");
        assert!(
            !election.exists(),
            "the refusing start released its own election lock"
        );
        assert!(
            try_claim_descriptor_slot(&path).is_err(),
            "the holder's claim is untouched"
        );
        holder.release().unwrap();
    }

    #[test]
    fn observer_connections_are_capped_and_a_renewal_does_not_count() {
        let mut accounting = ClientAccounting::default();
        for id in 1..=MAX_OBSERVER_CONNECTIONS as u64 {
            let row = ConnectionInfo {
                authenticated: true,
                role: ConnectionRole::Observer {
                    idle_policy_minutes: None,
                },
                ..Default::default()
            };
            accounting.connections.insert(id, row);
        }
        assert_eq!(
            accounting.observers_other_than(Some(1)),
            MAX_OBSERVER_CONNECTIONS - 1
        );
        assert!(accounting.observers_other_than(Some(1)) < MAX_OBSERVER_CONNECTIONS);
        assert!(accounting.observers_other_than(Some(99)) >= MAX_OBSERVER_CONNECTIONS);
    }

    /// A fresh directory per call. The PID alone is not unique: Windows
    /// reuses PIDs, and a predecessor's `.claim` sidecar outlives it (see
    /// `a_stale_claim_from_a_same_pid_predecessor_never_blocks_a_fixture`).
    #[derive(Clone)]
    struct ScratchDescriptor {
        path: PathBuf,
        root: Arc<crate::device_lease::testing::FixtureRoot>,
    }
    impl std::ops::Deref for ScratchDescriptor {
        type Target = PathBuf;
        fn deref(&self) -> &Self::Target {
            &self.path
        }
    }
    impl AsRef<Path> for ScratchDescriptor {
        fn as_ref(&self) -> &Path {
            &self.path
        }
    }
    fn scratch_descriptor_path(name: &str) -> ScratchDescriptor {
        let root = crate::device_lease::testing::fresh_root(&format!("rpc-{name}"));
        ScratchDescriptor {
            path: root.0.join("device-broker.json"),
            root,
        }
    }

    // Both names reuse the original exact-root ownership. Fixture workers and
    // TestServe keep this same Arc until real retirement, including unwind.
    fn guarded_scratch_descriptor_path(name: &str) -> (OwnedScratch, ScratchDescriptor) {
        let path = scratch_descriptor_path(name);
        (OwnedScratch(path.clone()), path)
    }

    fn broker_scratch_descriptor_path(name: &str) -> (OwnedScratch, ScratchDescriptor) {
        guarded_scratch_descriptor_path(name)
    }

    /// A broker test process that ends before its idle-shutdown grace
    /// elapses leaves its `.claim` sidecar behind, and Windows reuses PIDs.
    /// A later test process that derives the same scratch directory then
    /// fails the claim, and `serve_broker_tcp` refuses to serve (it once
    /// answered with `process::exit(1)`, and the run died silently mid-test;
    /// it now returns the refusal as an error). Dropping a claim
    /// without releasing it is exactly what process exit leaves on disk, and
    /// a second fixture call from the same process is the same-PID case.
    #[test]
    fn a_stale_claim_from_a_same_pid_predecessor_never_blocks_a_fixture() {
        let predecessor = scratch_descriptor_path("same-pid-stale-claim");
        let _predecessor_dir = OwnedScratch(predecessor.clone());
        drop(try_claim_descriptor_slot(&predecessor).expect("predecessor claims"));
        let path = scratch_descriptor_path("same-pid-stale-claim");
        let _successor_dir = OwnedScratch(path.clone());
        // The guards remove both directories even when this fails, so the
        // message alone has to carry what a diagnosis needs.
        let stale_claim_at_successor = qc_owner_only_file::claim_path(&path).exists();
        let claim = qc_owner_only_file::ClaimedFile::try_claim(&path).unwrap_or_else(|error| {
            panic!(
                "a fixture path must be free even after a same-PID predecessor died: \
                 claim at {} failed with {error} (kind {:?}); predecessor {}; \
                 same directory: {}; stale .claim present at the successor: {}",
                path.display(),
                error.kind(),
                predecessor.display(),
                predecessor.parent() == path.parent(),
                stale_claim_at_successor
            )
        });
        claim.release().unwrap();
    }

    /// Removes exactly what one fixture call created, even when the test
    /// panics: the descriptor, its `.claim` sidecar and their directory.
    /// `remove_dir` is not recursive, so nothing else is ever swept; on a
    /// passing test anything left over fails it instead.
    struct OwnedScratch(ScratchDescriptor);

    impl Drop for OwnedScratch {
        fn drop(&mut self) {
            // The exclusive root's final Arc owns retirement. Other admitted
            // readers/server workers retain the same root until they finish.
            if Arc::strong_count(&self.0.root) == 1 {
                let removals = [
                    std::fs::remove_file(&self.0.path),
                    std::fs::remove_file(qc_owner_only_file::claim_path(&self.0.path)),
                    std::fs::remove_dir(&self.0.root.0),
                ];
                for removal in removals {
                    if let Err(error) = removal {
                        if error.kind() != std::io::ErrorKind::NotFound && !thread::panicking() {
                            panic!("could not clean up {}: {error}", self.0.root.0.display());
                        }
                    }
                }
            }
        }
    }

    impl OwnedScratch {
        fn served_by(&mut self, _serve: &TestServe) {
            // TestServe already holds the exact root, original worker and broker
            // handle; its retained ownership, not an exit flag, gates cleanup.
        }
    }

    /// Starts `serve_broker_tcp` against a disconnected (no real USB)
    /// controller on a background thread and returns everything a test
    /// needs: the assigned port, the bearer token, the controller itself
    /// (so a test can inject a synthetic state frame), and a handle to ask
    /// whether the simulated process is still "alive" - i.e. whether the
    /// idle-shutdown grace period has fired yet.
    struct TestServe {
        port: u16,
        token: String,
        controller: Arc<DeviceController>,
        exited: Arc<AtomicBool>,
        job: Option<test_support::FixtureJob<()>>,
        handle: BrokerHandle,
        _root: Arc<crate::device_lease::testing::FixtureRoot>,
    }
    impl Drop for TestServe {
        fn drop(&mut self) {
            test_support::finish(&self.handle);
            drop(self.job.take());
            let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(2);
            let retired = || {
                Arc::strong_count(&self.handle.0) == 1
                    && Arc::strong_count(&self.handle.0.clients) == 1
                    && Arc::strong_count(&self.handle.0.request_lock) == 1
            };
            while !retired() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if !retired() {
                static RETAINED: Mutex<
                    Vec<(BrokerHandle, Arc<crate::device_lease::testing::FixtureRoot>)>,
                > = Mutex::new(Vec::new());
                RETAINED
                    .lock_recover()
                    .push((self.handle.clone(), Arc::clone(&self._root)));
                eprintln!("TCP fixture resources unacknowledged; exact handle and root retained");
            }
        }
    }
    struct RetainRootOnFailure {
        root: Arc<crate::device_lease::testing::FixtureRoot>,
        armed: bool,
    }
    impl Drop for RetainRootOnFailure {
        fn drop(&mut self) {
            if self.armed {
                static RETAINED: Mutex<Vec<Arc<crate::device_lease::testing::FixtureRoot>>> =
                    Mutex::new(Vec::new());
                RETAINED.lock_recover().push(Arc::clone(&self.root));
                eprintln!(
                    "serve failure cleanup unproven; exact root retained at {}",
                    self.root.0.display()
                );
            }
        }
    }

    impl TestServe {
        fn alive(&self) -> bool {
            !self.exited.load(Ordering::Acquire)
        }
    }

    fn start_broker_serve_for_test(descriptor_path: &ScratchDescriptor) -> TestServe {
        start_broker_serve_for_test_full(descriptor_path, false, |controller| {
            let _ = controller.reconnect();
        })
    }

    /// Like `start_broker_serve_for_test`, but with `--auto-connect` set and
    /// the deferred auto-connect action injected, so a test can hold the
    /// simulated handshake open and observe what the broker does meanwhile.
    fn start_broker_serve_for_test_with_auto_connect(
        descriptor_path: &ScratchDescriptor,
        perform_auto_connect: impl FnOnce(&DeviceController) + Send + 'static,
    ) -> TestServe {
        start_broker_serve_for_test_full(descriptor_path, true, perform_auto_connect)
    }

    fn start_broker_serve_for_test_full(
        descriptor_path: &ScratchDescriptor,
        auto_connect: bool,
        perform_auto_connect: impl FnOnce(&DeviceController) + Send + 'static,
    ) -> TestServe {
        // `serve_broker_tcp` takes `Arc<DeviceController>` (not an owned
        // `DeviceController`) precisely so a caller like this test can keep
        // its own clone of the Arc to inject synthetic state frames through
        // while the same controller instance is being served over TCP.
        let controller =
            Arc::new(DeviceController::start_disconnected().expect("controller starts"));
        let controller_for_serve = Arc::clone(&controller);
        let exited = Arc::new(AtomicBool::new(false));
        let on_idle_shutdown_exited = Arc::clone(&exited);
        let on_idle_shutdown: Arc<dyn Fn() + Send + Sync> =
            Arc::new(move || on_idle_shutdown_exited.store(true, Ordering::Release));
        let root = Arc::clone(&descriptor_path.root);
        let job_root = Arc::clone(&root);
        let descriptor_path = descriptor_path.to_path_buf();

        // The port is OS-assigned inside `serve_broker_tcp`, so read it back
        // out of the descriptor file it publishes rather than duplicating
        // its bind logic here.
        //
        // The empty MIDI worker registry and deferred auto-connect action are
        // ready before publication; publication never opens a device endpoint.
        let (ready_tx, ready_rx) = mpsc::channel();
        let path_for_serve = descriptor_path.clone();
        let job = test_support::FixtureJob::spawn(move |stop| {
            let mut root_guard = RetainRootOnFailure {
                root: job_root,
                armed: true,
            };
            let result = serve_broker_tcp_full_with_stop(
                controller_for_serve,
                false,
                path_for_serve,
                on_idle_shutdown,
                auto_connect,
                perform_auto_connect,
                move |handle| {
                    let _ = ready_tx.send(handle);
                },
                stop,
            );
            if let Err(error) = result {
                panic!("test broker failed: {error}");
            }
            root_guard.armed = false;
        });
        let handle = ready_rx
            .recv_timeout(Duration::from_secs(8))
            .expect("broker publishes its descriptor promptly");
        let bytes = std::fs::read(&descriptor_path).expect("descriptor file readable");
        let descriptor: Value = serde_json::from_slice(&bytes).expect("descriptor is JSON");
        let port = descriptor["port"].as_u64().expect("published port") as u16;
        let token = descriptor["bearerToken"]
            .as_str()
            .expect("descriptor carries a bearer token")
            .to_string();

        TestServe {
            port,
            token,
            controller,
            exited,
            job: Some(job),
            handle,
            _root: root,
        }
    }

    fn authenticated_tcp_client(port: u16, token: &str) -> TcpStream {
        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connects to broker");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read budget");
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("write budget");
        client.set_nodelay(true).expect("nodelay");
        write_response(
            &mut client,
            &json!({"method": "broker.authenticate", "params": {"token": token}, "id": 0}),
        )
        .expect("auth frame sent");
        let response = read_frame_value(&mut client)
            .expect("auth response readable")
            .expect("auth response present");
        assert!(
            response.get("result").is_some(),
            "authentication must succeed with the published token: {response:?}"
        );
        client
    }

    fn read_frame_value(stream: &mut TcpStream) -> Result<Option<Value>, String> {
        let Some(body) = read_frame_bytes(stream)? else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn send_request(client: &mut TcpStream, method: &str, params: Value) -> Value {
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("write budget");
        write_response(
            client,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        )
        .expect("request sent");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "matching response deadline expired");
            client
                .set_read_timeout(Some(remaining))
                .expect("read budget");
            let response = read_frame_value(client)
                .expect("response readable")
                .expect("response present");
            if response.get("id") == Some(&json!(1)) {
                return response;
            }
        }
    }

    /// Reads frames off `client` until one is the named notification or the
    /// short deadline expires - a request/response frame with a matching
    /// `id` may arrive first on a connection that also just sent a request.
    fn read_notification(client: &mut TcpStream, method: &str) -> Option<Value> {
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout set");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match read_frame_value(client) {
                Ok(Some(frame)) if frame.get("method") == Some(&json!(method)) => {
                    return Some(frame)
                }
                Ok(Some(_)) => continue,
                _ => return None,
            }
        }
        None
    }

    fn observer_hello(client: &mut TcpStream, minutes: Value) -> Value {
        send_request(
            client,
            "broker.hello",
            json!({"role": "observer", "provider": "QC tray", "idlePolicyMinutes": minutes}),
        )
    }

    fn assert_observer_policy_and_detachment_during_action(method: &str) {
        let shared = Arc::new(broker_shared_for_test());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let id = accept_client(&shared.clients).unwrap();
        let connection_shared = Arc::clone(&shared);
        let (ended, wait_ended) = mpsc::channel();
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &connection_shared, id, false);
                connection_shared.detach_client(id);
                ended.send(()).unwrap();
            });
        assert!(observer_hello(&mut client, json!(1))
            .get("result")
            .is_some());
        let idle_origin = shared.clients.lock_recover().observer_idle_since;
        // Hold the actual operation lock rather than relying on a slow device
        // or a scheduling delay. This unknown method cannot touch hardware.
        let operation = shared.request_lock.lock_recover();
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":11,"method":method,"params":{}}),
        )
        .unwrap();
        let updated = observer_hello(&mut client, json!(1440));
        assert!(updated.get("result").is_some(), "{updated}");
        {
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.count, 1);
            assert_eq!(clients.observers(), 1);
            assert_eq!(clients.observer_idle_since, idle_origin);
            assert_eq!(
                shared.observer_deadline(&clients).unwrap() - idle_origin.unwrap(),
                Duration::from_secs(1445 * 60)
            );
        }
        let busy = send_request(&mut client, "test.secondAction", json!({}));
        assert_eq!(busy["error"]["code"], -32002);
        assert!(busy["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already in progress"));
        drop(client);
        wait_ended
            .recv_timeout(Duration::from_secs(2))
            .expect("closing an observer must detach without waiting for its device action");
        assert_eq!(shared.clients.lock_recover().count, 0);
        drop(operation);
        connection.join().unwrap();
    }

    #[test]
    fn observer_policy_and_detachment_are_live_while_its_device_action_waits() {
        assert_observer_policy_and_detachment_during_action("test.blockedAction");
    }

    #[test]
    fn observer_action_retains_connection_admission_after_actual_socket_eof() {
        let shared = Arc::new(broker_shared_for_test());
        let (mut client, server) = socket_pair();
        let id = accept_client(&shared.clients).unwrap();
        let weak_owner = Arc::downgrade(&connection_owner(&shared.clients, id).unwrap());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered, ready) = mpsc::channel();
        let action_gate = Arc::clone(&gate);
        TEST_POINTS.lock_recover().push((
            shared.instance_id.clone(),
            "disconnect:after_precheck",
            Arc::new(move || {
                let _ = entered.send(());
                let (released, changed) = &*action_gate;
                let released = released.lock_recover();
                let (released, _) = changed
                    .wait_timeout_while(released, Duration::from_secs(5), |released| !*released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *released {
                    Ok(())
                } else {
                    Err("fixture action gate timed out".into())
                }
            }),
        ));
        struct ReleaseGate {
            gate: Arc<(Mutex<bool>, Condvar)>,
            instance: String,
        }
        impl Drop for ReleaseGate {
            fn drop(&mut self) {
                *self.gate.0.lock_recover() = true;
                self.gate.1.notify_all();
                TEST_POINTS
                    .lock_recover()
                    .retain(|(instance, _, _)| instance != &self.instance);
            }
        }
        // Armed before the connection/action starts, including assertion unwind.
        let release = ReleaseGate {
            gate,
            instance: shared.instance_id.clone(),
        };
        let serving = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &serving, id, false);
                serving.detach_client(id);
            });
        assert!(observer_hello(&mut client, json!(15))
            .get("result")
            .is_some());
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0", "id":17,
            "method":generated_gateway::rpc::DISCONNECT, "params":{}}),
        )
        .unwrap();
        ready
            .recv_timeout(Duration::from_secs(2))
            .expect("actual observer action entered before request lock");
        drop(client);
        connection.join().unwrap();
        assert_eq!(
            shared.clients.lock_recover().count,
            0,
            "EOF still detaches the visible observer immediately"
        );
        let admitted_while_action_alive = weak_owner.strong_count() > 0;
        // Release before asserting the regression so the pre-fix detached task
        // never remains blocked. Its final live-connection check prevents I/O.
        drop(release);
        shared.background.stop();
        shared.observer_actions.stop();
        let until = Instant::now() + Duration::from_secs(3);
        while Arc::strong_count(&shared) > 1 && Instant::now() < until {
            let _ = shared.background.acknowledged();
            let _ = shared.observer_actions.acknowledged();
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            Arc::strong_count(&shared),
            1,
            "controlled action body and timer captures retired"
        );
        assert!(admitted_while_action_alive, "a detached observer action must retain its exact connection admission until original retirement");
    }

    #[test]
    fn an_observer_action_latch_is_released_before_its_response_first_byte() {
        struct ObserveFirstWrite {
            action_pending: Arc<AtomicBool>,
            latch_was_clear: Arc<AtomicBool>,
            observed: bool,
        }

        impl Write for ObserveFirstWrite {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                assert!(!bytes.is_empty());
                if !self.observed {
                    self.latch_was_clear.store(
                        !self.action_pending.load(Ordering::Acquire),
                        Ordering::Release,
                    );
                    self.observed = true;
                }
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let action_pending = Arc::new(AtomicBool::new(true));
        let latch_was_clear = Arc::new(AtomicBool::new(false));
        let output = Mutex::new(ObserveFirstWrite {
            action_pending: Arc::clone(&action_pending),
            latch_was_clear: Arc::clone(&latch_was_clear),
            observed: false,
        });

        write_observer_action_response(
            &output,
            &action_pending,
            &json!({"jsonrpc":"2.0","id":1,"result":{}}),
            |_| {},
        );

        assert!(latch_was_clear.load(Ordering::Acquire));
        assert!(!action_pending.load(Ordering::Acquire));
    }

    #[test]
    fn an_entered_observer_action_failure_closes_transport_without_false_predispatch_refusal() {
        let shared = Arc::new(broker_shared_for_test());
        let (mut client, server) = socket_pair();
        let id = accept_client(&shared.clients).unwrap();
        TEST_POINTS.lock_recover().push((
            shared.instance_id.clone(),
            "disconnect:after_precheck",
            Arc::new(|| panic!("injected failure after observer dispatch entered")),
        ));
        struct HookOwner(String);
        impl Drop for HookOwner {
            fn drop(&mut self) {
                TEST_POINTS
                    .lock_recover()
                    .retain(|(instance, _, _)| instance != &self.0);
            }
        }
        let _hook = HookOwner(shared.instance_id.clone());
        let serving = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &serving, id, false);
                serving.detach_client(id);
            });
        assert!(observer_hello(&mut client, json!(15))
            .get("result")
            .is_some());
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0", "id":18,
            "method":generated_gateway::rpc::DISCONNECT, "params":{}}),
        )
        .unwrap();
        client.set_nonblocking(true).unwrap();
        let close_deadline = Instant::now() + Duration::from_secs(2);
        let mut byte = [0_u8; 1];
        loop {
            match client.peek(&mut byte) {
                Ok(0) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < close_deadline,
                        "a silent open socket is not acknowledged transport closure"
                    );
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(_) => {
                    // Drain only existing notifications. A request response,
                    // incomplete frame or timeout cannot masquerade as EOF.
                    client.set_nonblocking(false).unwrap();
                    let remaining = close_deadline.saturating_duration_since(Instant::now());
                    assert!(
                        !remaining.is_zero(),
                        "transport did not close before its fixture deadline"
                    );
                    client.set_read_timeout(Some(remaining)).unwrap();
                    let frame = read_frame_value(&mut client)
                        .expect("complete pending notification")
                        .expect("notification body");
                    assert!(
                        frame.get("id").is_none(),
                        "an admitted action failure must not claim a no-write refusal: {frame}"
                    );
                    client.set_nonblocking(true).unwrap();
                }
                Err(error) => panic!("unproven transport closure: {error}"),
            }
        }
        drop(client);
        connection.join().unwrap();
        shared.background.stop();
        shared.observer_actions.stop();
        let until = Instant::now() + Duration::from_secs(3);
        while !(shared.background.acknowledged() && shared.observer_actions.acknowledged()) {
            assert!(
                Instant::now() < until,
                "controlled original dispatchers must retire"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(shared.clients.lock_recover().count, 0);
    }

    #[test]
    fn a_never_started_observer_action_refuses_once_and_keeps_the_socket_usable() {
        let shared = Arc::new(broker_shared_for_test());
        shared.observer_actions.stop();
        let (mut client, server) = socket_pair();
        let id = accept_client(&shared.clients).unwrap();
        let weak_owner = Arc::downgrade(&connection_owner(&shared.clients, id).unwrap());
        let serving = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &serving, id, false);
                serving.detach_client(id);
            });
        assert!(observer_hello(&mut client, json!(15))
            .get("result")
            .is_some());
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0", "id":22,
            "method":generated_gateway::rpc::DISCONNECT, "params":{}}),
        )
        .unwrap();
        let refusal = read_response_within(&mut client, Duration::from_secs(2)).unwrap();
        assert_eq!(refusal["id"], 22);
        assert_eq!(refusal["error"]["code"], -32002);
        assert!(refusal["error"]["message"]
            .as_str()
            .unwrap()
            .contains("before dispatch"));
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0", "id":23, "method":"system.status", "params":{}}),
        )
        .unwrap();
        let next = read_response_within(&mut client, Duration::from_secs(2)).unwrap();
        assert_eq!(
            next["id"], 23,
            "never-started retirement sends no duplicate refusal"
        );
        assert!(next.get("result").is_some());
        drop(client);
        connection.join().unwrap();
        shared.background.stop();
        let until = Instant::now() + Duration::from_secs(3);
        while !shared.background.acknowledged() {
            assert!(
                Instant::now() < until,
                "controlled policy dispatcher must retire"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert!(shared.observer_actions.acknowledged());
        assert_eq!(
            weak_owner.strong_count(),
            0,
            "never-started admission is retired after socket exit"
        );
    }

    fn read_response_within(client: &mut TcpStream, within: Duration) -> Option<Value> {
        let deadline = Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            client.set_read_timeout(Some(remaining)).unwrap();
            match read_frame_value(client) {
                Ok(Some(frame)) if frame.get("id").is_some() => return Some(frame),
                Ok(Some(_notification)) => continue,
                _ => return None,
            }
        }
    }

    /// A client that floods its connection while a long request holds it can
    /// queue at most `CONNECTION_QUEUE_DEPTH` requests; the rest are refused
    /// on the spot, `system.status` still answers, and what was admitted runs
    /// afterwards in the order it arrived.
    #[test]
    fn a_flooded_connection_queues_a_bounded_backlog_and_keeps_answering_status() {
        const FLOOD: u64 = 12;
        let shared = Arc::new(broker_shared_for_test());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let id = accept_client(&shared.clients).unwrap();
        let connection_shared = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &connection_shared, id, false);
                connection_shared.detach_client(id);
            });
        let operation = shared.request_lock.lock_recover();
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":1,"method":"test.longOperation","params":{}}),
        )
        .unwrap();
        // The status reply is written only after the reader has handed the
        // long request to the queue; give the queue thread a moment to take
        // it, so every slot of the queue is free for the flood.
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":2,"method":"system.status","params":{}}),
        )
        .unwrap();
        assert_eq!(
            read_response_within(&mut client, Duration::from_secs(2)).unwrap()["id"],
            2
        );
        thread::sleep(Duration::from_millis(200));
        for flood in 0..FLOOD {
            write_response(
                &mut client,
                &json!({"jsonrpc":"2.0","id":100 + flood,"method":"test.flood","params":{}}),
            )
            .unwrap();
        }
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":3,"method":"system.status","params":{}}),
        )
        .unwrap();
        let started = Instant::now();
        let mut refused = Vec::new();
        loop {
            let response = read_response_within(&mut client, Duration::from_secs(2))
                .expect("the reader answers while the long request runs");
            if response["id"] == 3 {
                assert!(
                    response["result"]["usbDiagnostics"].is_object(),
                    "{response}"
                );
                break;
            }
            assert_eq!(
                response["error"]["code"], CONNECTION_QUEUE_FULL_ERROR_CODE,
                "{response}"
            );
            assert_eq!(response["error"]["data"]["code"], "CONNECTION_QUEUE_FULL");
            assert_eq!(response["error"]["data"]["requestSent"], false);
            refused.push(response["id"].as_u64().unwrap());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        // Exactly the overflow is refused, and it is the tail of the flood.
        let admitted = FLOOD as usize - refused.len();
        assert_eq!(admitted, CONNECTION_QUEUE_DEPTH, "refused {refused:?}");
        assert_eq!(
            refused,
            (100 + CONNECTION_QUEUE_DEPTH as u64..100 + FLOOD).collect::<Vec<_>>()
        );

        drop(operation);
        let mut answered = Vec::new();
        for _ in 0..=CONNECTION_QUEUE_DEPTH {
            let response = read_response_within(&mut client, Duration::from_secs(2)).unwrap();
            assert_ne!(
                response["error"]["code"], CONNECTION_QUEUE_FULL_ERROR_CODE,
                "{response}"
            );
            answered.push(response["id"].as_u64().unwrap());
        }
        let mut expected = vec![1];
        expected.extend(100..100 + CONNECTION_QUEUE_DEPTH as u64);
        assert_eq!(answered, expected, "admitted requests ran out of order");
        assert_eq!(
            read_response_within(&mut client, Duration::from_millis(300)),
            None
        );
        drop(client);
        connection.join().unwrap();
        assert_eq!(shared.clients.lock_recover().count, 0);
    }

    /// A connection thread that panics still detaches, so its
    /// count and row never outlive it.
    #[test]
    fn a_panicking_connection_still_detaches() {
        let shared = Arc::new(broker_shared_for_test());
        let id = accept_client(&shared.clients).unwrap();
        assert_eq!(shared.clients.lock_recover().count, 1);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _detach = DetachOnDrop {
                shared: &shared,
                connection_id: id,
            };
            panic!("a connection thread failed mid-serve");
        }));
        assert!(unwound.is_err());
        let clients = shared.clients.lock_recover();
        assert_eq!(clients.count, 0, "the panicked connection is still counted");
        assert!(!clients.connections.contains_key(&id));
    }

    /// Only `system.status` is answered beside a running request; the other
    /// lock-free reads may wait on a USB reopen and keep their place in line.
    #[test]
    fn only_status_is_answered_beside_a_running_request() {
        let shared = Arc::new(broker_shared_for_test());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let id = accept_client(&shared.clients).unwrap();
        let connection_shared = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &connection_shared, id, false);
                connection_shared.detach_client(id);
            });
        let operation = shared.request_lock.lock_recover();
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":1,"method":"test.longOperation","params":{}}),
        )
        .unwrap();
        for (id, method) in [
            (2, "device.stateEvents"),
            (3, "device.tempoClock"),
            (4, "device.masterVolume"),
        ] {
            assert!(runs_without_request_lock(method));
            assert!(!answers_beside_running_request(method));
            write_response(
                &mut client,
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}),
            )
            .unwrap();
        }
        assert_eq!(
            read_response_within(&mut client, Duration::from_millis(300)),
            None,
            "a cached read that can reopen USB overtook the running request"
        );
        drop(operation);
        let answered: Vec<_> = (0..4)
            .map(|_| {
                read_response_within(&mut client, Duration::from_secs(2)).unwrap()["id"].clone()
            })
            .collect();
        assert_eq!(answered, vec![json!(1), json!(2), json!(3), json!(4)]);
        drop(client);
        connection.join().unwrap();
    }

    /// A participant's own long request - a backup holds `request_lock` for
    /// about half a minute on hardware - must not hold back that same
    /// connection's `system.status`, or no client could see the
    /// unit's backup suspension at all, because every status poll queued
    /// behind the backup that caused it. Writes behind the backup must still
    /// wait their turn and then be refused exactly as before.
    #[test]
    fn a_participant_status_read_answers_while_its_own_long_request_runs() {
        let (controller, _commands) = DeviceController::live_session_for_test();
        // The unit's backup, as it reports it: screen lock, then suspension.
        controller.observe_device_frame_for_test(
            profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            &[0x08, 0x01, 0x30, 0x01],
        );
        controller.observe_device_frame_for_test(
            profile::MESSAGE_TYPE_SUSPEND_CONNECTION,
            &[0x10, 0x01, 0x18, 0x00],
        );
        let shared = Arc::new(BrokerShared {
            controller: Arc::new(controller),
            ..broker_shared_for_test()
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let id = accept_client(&shared.clients).unwrap();
        let connection_shared = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                serve_broker_tcp_connection(server, &connection_shared, id, false);
                connection_shared.detach_client(id);
            });
        // The long request: hold the operation lock it needs, as a running
        // backup does, so nothing here depends on timing or a device.
        let operation = shared.request_lock.lock_recover();
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":11,"method":"test.longOperation","params":{}}),
        )
        .unwrap();
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":12,"method":"system.status","params":{}}),
        )
        .unwrap();
        let started = Instant::now();
        let status = read_response_within(&mut client, Duration::from_secs(2))
            .expect("system.status must answer while the long request runs");
        assert!(started.elapsed() < Duration::from_secs(1), "{status}");
        assert_eq!(status["id"], 12, "{status}");
        let diagnostics = &status["result"]["usbDiagnostics"];
        assert_eq!(diagnostics["deviceSuspended"], true, "{status}");
        assert_eq!(diagnostics["suspension"]["active"]["reason"], "BACKUP");

        // A write sent behind the long request keeps its place in line.
        write_response(
            &mut client,
            &json!({"jsonrpc":"2.0","id":13,"method":"device.setTempo","params":{
                "bpm": 120, "expectedTempo": 100, "expectedPresetName": "Clean",
                "expectedPresetIdentity": "fixture-native-A"
            }}),
        )
        .unwrap();
        assert_eq!(
            read_response_within(&mut client, Duration::from_millis(300)),
            None,
            "a write must not overtake the request ahead of it"
        );
        drop(operation);
        let first = read_response_within(&mut client, Duration::from_secs(2)).unwrap();
        assert_eq!(first["id"], 11, "{first}");
        let write = read_response_within(&mut client, Duration::from_secs(2)).unwrap();
        assert_eq!(write["id"], 13, "{write}");
        assert_eq!(
            write["error"]["code"], DEVICE_SUSPENDED_ERROR_CODE,
            "{write}"
        );
        assert_eq!(write["error"]["data"]["code"], "DEVICE_SUSPENDED");
        assert_eq!(write["error"]["data"]["requestSent"], false);
        drop(client);
        connection.join().unwrap();
        assert_eq!(shared.clients.lock_recover().count, 0);
    }

    #[test]
    fn observer_policy_and_detachment_are_live_while_its_shutdown_waits() {
        assert_observer_policy_and_detachment_during_action("broker.shutdown");
    }

    #[test]
    fn a_detached_observer_never_gets_the_participants_disconnect_allowance() {
        let shared = Arc::new(broker_shared_for_test());
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(15));
        let held = shared.request_lock.lock_recover();
        let pending = Arc::clone(&shared);
        let request = rpc_request(generated_gateway::rpc::DISCONNECT);
        let worker = test_support::FixtureJob::spawn_plain(move || {
            dispatch_broker_client_request(&pending, Some(observer), request)
        });
        shared.detach_client(observer);
        let participant = accept_authenticated(&shared.clients);
        assert!(shared
            .clients
            .lock_recover()
            .disconnect_blocked(Some(observer)));
        assert!(!shared
            .clients
            .lock_recover()
            .disconnect_blocked(Some(participant)));
        drop(held);
        let response = worker.join().unwrap();
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|s| s.contains("no longer attached"))
                || response["result"]["detail"]
                    .as_str()
                    .is_some_and(|s| s.contains("left open")),
            "{response}"
        );
        assert_eq!(shared.clients.lock_recover().participants(), 1);
        shared.detach_client(participant);
    }

    #[test]
    fn a_queued_forced_shutdown_is_cancelled_when_its_observer_detaches() {
        let shared = Arc::new(broker_shared_for_test());
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(15));
        let held = shared.request_lock.lock_recover();
        let (started, ready) = mpsc::channel();
        let pending = Arc::clone(&shared);
        let mut request = rpc_request("broker.shutdown");
        request.params = json!({"force":true});
        let worker = test_support::FixtureJob::spawn_plain(move || {
            request_broker_shutdown_after_precheck(&pending, request, Some(observer), || {
                started.send(()).unwrap()
            })
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        shared.detach_client(observer);
        let participant = accept_authenticated(&shared.clients);
        drop(held);
        let response = worker.join().unwrap();
        assert_eq!(response["error"]["code"], -32002);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no longer attached"));
        assert!(!shared.clients.lock_recover().shutting_down);
        shared.detach_client(participant);
    }

    #[test]
    fn observers_receive_ownership_changes_without_polling_or_state_frames() {
        let (mut scratch, path) = broker_scratch_descriptor_path("client-changes");
        let broker = start_broker_serve_for_test(&path);
        scratch.served_by(&broker);
        let mut observer = authenticated_tcp_client(broker.port, &broker.token);
        observer_hello(&mut observer, Value::Null);
        let initial = read_notification(&mut observer, "broker.clientsChanged").unwrap();
        assert_eq!(initial["params"]["clients"].as_array().unwrap().len(), 1);
        // A socket that has not authenticated is nobody's participant: it is
        // not announced or listed, and it cannot hold the observer's
        // disconnect hostage. (It still defers idle shutdown; see
        // `failed_authentication_preserves_the_observer_idle_origin`.)
        let mut participant = TcpStream::connect(("127.0.0.1", broker.port)).unwrap();
        participant.set_nodelay(true).unwrap();
        observer
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        assert!(
            read_frame_value(&mut observer).is_err(),
            "an unauthenticated socket must not produce a clientsChanged notification"
        );
        observer
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let listed = send_request(&mut observer, "broker.clients", json!({}));
        assert_eq!(listed["result"]["clients"].as_array().unwrap().len(), 1);
        let allowed = send_request(&mut observer, "device.disconnect", json!({}));
        assert!(
            !allowed.to_string().contains("left open"),
            "an unauthenticated socket must not block a disconnect: {allowed}"
        );
        // The same socket becomes a participant the moment it authenticates.
        write_response(
            &mut participant,
            &json!({"method": "broker.authenticate", "params": {"token": broker.token}, "id": 0}),
        )
        .unwrap();
        assert!(read_frame_value(&mut participant)
            .unwrap()
            .unwrap()
            .get("result")
            .is_some());
        let attached = read_notification(&mut observer, "broker.clientsChanged").unwrap();
        assert!(attached["params"]["clients"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["role"] == "participant"));
        let refused = send_request(&mut observer, "device.disconnect", json!({}));
        assert!(refused["result"]["detail"]
            .as_str()
            .unwrap()
            .contains("left open"));
        drop(participant);
        let detached = read_notification(&mut observer, "broker.clientsChanged").unwrap();
        assert_eq!(detached["params"]["clients"].as_array().unwrap().len(), 1);
        assert_eq!(detached["params"]["clients"][0]["role"], "observer");
    }

    #[test]
    fn observers_are_named_but_do_not_count_as_participants_or_block_their_disconnect() {
        let (mut scratch, path) = broker_scratch_descriptor_path("observer-participant-count");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut participant = authenticated_tcp_client(serve.port, &serve.token);
        let mut observer = authenticated_tcp_client(serve.port, &serve.token);
        assert!(observer_hello(&mut observer, json!(15))
            .get("result")
            .is_some());
        assert_eq!(
            send_request(&mut participant, "broker.clientCount", json!({}))["result"]["clients"],
            1
        );
        let clients = send_request(&mut observer, "broker.clients", json!({}));
        assert!(clients["result"]["clients"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["provider"] == "QC tray" && row["role"] == "observer"));
        let refused = send_request(&mut observer, "device.disconnect", json!({}));
        assert!(refused["result"]["detail"]
            .as_str()
            .unwrap()
            .contains("left open"));
        let disconnected = send_request(&mut participant, "device.disconnect", json!({}));
        assert!(!disconnected["result"]["detail"]
            .as_str()
            .unwrap()
            .contains("left open"));
        let _second = authenticated_tcp_client(serve.port, &serve.token);
        let refused = send_request(&mut participant, "device.disconnect", json!({}));
        assert!(refused["result"]["detail"]
            .as_str()
            .unwrap()
            .contains("left open"));
    }

    #[test]
    fn explicit_broker_disconnect_closes_the_shared_device_for_all_participants() {
        let (mut scratch, path) = broker_scratch_descriptor_path("explicit-disconnect");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut first = authenticated_tcp_client(serve.port, &serve.token);
        let _second = authenticated_tcp_client(serve.port, &serve.token);
        let scoped = send_request(&mut first, "device.disconnect", json!({}));
        assert!(scoped["result"]["detail"]
            .as_str()
            .unwrap()
            .contains("left open"));
        let explicit = send_request(&mut first, BROKER_DISCONNECT_DEVICE_METHOD, json!({}));
        assert!(explicit.get("error").is_none(), "{explicit}");
        assert!(!explicit["result"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("left open"));
    }

    #[test]
    fn hello_rejects_invalid_values_without_changing_the_connection_role() {
        let (mut scratch, path) = broker_scratch_descriptor_path("observer-invalid-hello");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut client = authenticated_tcp_client(serve.port, &serve.token);
        for minutes in [json!(0), json!(1441), json!("15"), json!(1.5), json!(-1)] {
            let refused = observer_hello(&mut client, minutes);
            assert!(refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("1 and 1440"));
            assert_eq!(
                send_request(&mut client, "broker.clientCount", json!({}))["result"]["clients"],
                1
            );
        }
        for params in [json!({"role":"admin"}), json!({"provider":7})] {
            assert!(send_request(&mut client, "broker.hello", params)
                .get("error")
                .is_some());
        }
        for minutes in [json!(1), json!(1440), Value::Null] {
            assert!(observer_hello(&mut client, minutes).get("result").is_some());
        }
        assert!(send_request(&mut client, "broker.hello", json!({}))
            .get("result")
            .is_some());
        assert_eq!(
            send_request(&mut client, "broker.clientCount", json!({}))["result"]["clients"],
            1
        );
    }

    #[test]
    fn unknown_authenticate_fields_keep_the_legacy_participant_default() {
        let (mut scratch, path) = broker_scratch_descriptor_path("auth-extra-fields");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut client = TcpStream::connect(("127.0.0.1", serve.port)).unwrap();
        write_response(&mut client,&json!({"method":"broker.authenticate","id":0,"params":{"token":serve.token,"somethingNewer":true}})).unwrap();
        assert!(read_frame_value(&mut client)
            .unwrap()
            .unwrap()
            .get("result")
            .is_some());
        let _second = authenticated_tcp_client(serve.port, &serve.token);
        assert_eq!(
            send_request(&mut client, "broker.clientCount", json!({}))["result"]["clients"],
            2
        );
        assert!(
            send_request(&mut client, "device.disconnect", json!({}))["result"]["detail"]
                .as_str()
                .unwrap()
                .contains("left open")
        );
    }

    #[test]
    fn observers_stop_receiving_state_frames_and_can_become_participants_again() {
        let (mut scratch, path) = broker_scratch_descriptor_path("observer-state-frame");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut client = authenticated_tcp_client(serve.port, &serve.token);
        assert!(observer_hello(&mut client, json!(15))
            .get("result")
            .is_some());
        serve
            .controller
            .inject_state_frame_for_test(DecodedStateFrame {
                stream_id: "observer-test".into(),
                sequence: 1,
                observed_at: 0,
                states: vec![StateUpdate::empty("observer-test")],
                tempo_clock: None,
            });
        client
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            match read_frame_value(&mut client) {
                Ok(Some(frame)) => assert_eq!(
                    frame["method"], "broker.clientsChanged",
                    "an observer may receive ownership metadata but never device.stateFrame"
                ),
                Err(_) => break,
                Ok(None) => panic!("observer socket must remain attached"),
            }
        }
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(
            send_request(&mut client, "broker.hello", json!({"role":"participant"}))
                .get("result")
                .is_some()
        );
        serve
            .controller
            .inject_state_frame_for_test(DecodedStateFrame {
                stream_id: "observer-test".into(),
                sequence: 2,
                observed_at: 0,
                states: vec![StateUpdate::empty("observer-test")],
                tempo_clock: None,
            });
        assert!(read_notification(&mut client, "device.stateFrame").is_some());
    }

    #[test]
    fn shutdown_refuses_participants_then_forces_and_flushes_its_response() {
        let (mut scratch, path) = broker_scratch_descriptor_path("observer-shutdown");
        let serve = start_broker_serve_for_test(&path);
        let instance_id = serve.handle.0.instance_id.clone();
        let release_marker = BrokerReleaseMarker::path_for(&path, &instance_id);
        scratch.served_by(&serve);
        let mut observer = authenticated_tcp_client(serve.port, &serve.token);
        assert!(observer_hello(&mut observer, json!(15))
            .get("result")
            .is_some());
        let mut participant = authenticated_tcp_client(serve.port, &serve.token);
        assert!(send_request(&mut observer, "broker.shutdown", json!({}))
            .get("error")
            .is_some());
        assert!(serve.alive());
        assert!(
            send_request(&mut observer, "broker.shutdown", json!({"force":"true"}))
                .get("error")
                .is_some()
        );
        let response = send_request(&mut observer, "broker.shutdown", json!({"force":true}));
        assert_eq!(response["result"]["shutdown"], true, "{response}");
        let notice = read_notification(&mut participant, "broker.shuttingDown")
            .expect("participant notified before deliberate exit");
        assert_eq!(notice["params"]["forced"], true);
        assert!(notice["params"]["detail"]
            .as_str()
            .unwrap()
            .contains("deliberately released"));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while serve.alive() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!serve.alive());
        assert!(!path.exists());
        drop(observer);
        drop(participant);
        drop(serve); // Join the original serve/peer actors before fixture cleanup.
        let marker: BrokerReleaseMarker =
            serde_json::from_slice(&std::fs::read(&release_marker).unwrap()).unwrap();
        assert_eq!(marker.instance_id, instance_id);
        assert!(marker.forced);
        assert!(qc_owner_only_file::is_owner_only(&release_marker).unwrap());
        // Forced release is intentionally durable in production. Retire only
        // this fixture's verified generation marker, not arbitrary leftovers.
        std::fs::remove_file(release_marker).unwrap();
    }

    #[test]
    fn shutdown_succeeds_with_only_an_observer_and_no_force() {
        let (mut scratch, path) = broker_scratch_descriptor_path("observer-shutdown-alone");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut observer = authenticated_tcp_client(serve.port, &serve.token);
        assert!(observer_hello(&mut observer, Value::Null)
            .get("result")
            .is_some());
        assert_eq!(
            send_request(&mut observer, "broker.shutdown", json!({}))["result"]["shutdown"],
            true
        );
    }

    #[test]
    fn an_observer_holds_raw_liveness_until_its_socket_closes() {
        let (mut scratch, path) = broker_scratch_descriptor_path("observer-raw-liveness");
        let serve = start_broker_serve_for_test(&path);
        scratch.served_by(&serve);
        let mut observer = authenticated_tcp_client(serve.port, &serve.token);
        assert!(observer_hello(&mut observer, json!(15))
            .get("result")
            .is_some());
        thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(300));
        assert!(
            serve.alive(),
            "an observer still counts for layer-1 liveness"
        );
        assert_eq!(
            send_request(&mut observer, "broker.clientCount", json!({}))["result"]["clients"],
            0
        );
        drop(observer); // The same socket-close path covers a crashed tray.
        let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(3);
        while serve.alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !serve.alive(),
            "the last observer closes through the existing five-second grace"
        );
        assert!(!path.exists());
    }

    #[test]
    fn committed_shutdown_rejects_further_gateway_dispatch() {
        let shared = broker_shared_for_test();
        shared.clients.lock_recover().shutting_down = true;
        let response = dispatch_broker_request(&shared, rpc_request("device.snapshot"));
        assert_eq!(response["error"]["code"], -32002);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("shutdown is already committed"));
    }

    #[test]
    fn committed_shutdown_finishes_even_when_the_callers_output_lock_is_stalled() {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let shared = Arc::new(BrokerShared {
            on_idle_shutdown: Arc::new(move || {
                let _ = shutdown_tx.send(());
            }),
            ..broker_shared_for_test()
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let _reader = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (writer, _) = listener.accept().unwrap();
        let output = Arc::new(Mutex::new(writer));
        let stalled = output.lock().unwrap();
        let waiting = Arc::clone(&shared);
        let waiting_output = Arc::clone(&output);
        let worker = test_support::FixtureJob::spawn_plain(move || {
            let response = request_broker_shutdown(&waiting, rpc_request("broker.shutdown"));
            assert_eq!(response["result"]["shutdown"], true);
            waiting.finish_shutdown_after_response(&waiting_output, &response);
        });
        // The output lock remains held for the entire wait, modelling a push
        // that is stuck writing to a caller which has stopped reading.
        shutdown_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("a stalled caller must not prevent committed shutdown");
        assert!(shared.clients.lock_recover().shutting_down);
        drop(stalled);
        worker.join().unwrap();
    }

    fn observer_test_shared() -> (Arc<BrokerShared>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(BrokerShared {
            observer_minute: Duration::from_millis(2),
            on_idle_shutdown: Arc::new(move || {
                let _ = tx.send(());
            }),
            ..broker_shared_for_test()
        });
        (shared, rx)
    }

    /// An accepted connection that has completed authentication, as
    /// `serve_broker_tcp_connection` records it, without a real socket.
    fn accept_authenticated(clients: &Mutex<ClientAccounting>) -> u64 {
        let id = accept_client(clients).unwrap();
        clients
            .lock_recover()
            .connections
            .get_mut(&id)
            .unwrap()
            .authenticated = true;
        id
    }

    fn declare_observer(shared: &Arc<BrokerShared>, id: u64, minutes: Value) {
        let mut request = rpc_request("broker.hello");
        request.params = json!({"role":"observer","idlePolicyMinutes":minutes});
        assert!(dispatch_broker_client_request(shared, Some(id), request)
            .get("result")
            .is_some());
        shared.update_observer_backstop();
    }

    #[test]
    fn broker_never_hold_renews_from_hello_and_expires_when_renewal_stops() {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let shared = Arc::new(BrokerShared {
            observer_minute: Duration::from_millis(10),
            on_idle_shutdown: Arc::new(move || {
                let _ = shutdown_tx.send(());
            }),
            ..broker_shared_for_test()
        });
        let observer = accept_client(&shared.clients).unwrap();
        let hello = || {
            let mut request = rpc_request("broker.hello");
            request.params =
                json!({"role":"observer","idlePolicyMinutes":null,"keepConnected":true});
            dispatch_broker_client_request(&shared, Some(observer), request)
        };
        assert_eq!(hello()["result"]["accepted"], true);
        let old_origin = Instant::now() - Duration::from_secs(2);
        {
            let mut clients = shared.clients.lock_recover();
            clients.observer_idle_since = Some(old_origin);
            clients
                .connections
                .get_mut(&observer)
                .unwrap()
                .keep_connected_since = Some(old_origin);
            assert!(shared.observer_deadline(&clients).unwrap() < Instant::now());
        }
        let renewed_after = Instant::now();
        assert_eq!(hello()["result"]["accepted"], true);
        {
            let mut clients = shared.clients.lock_recover();
            let renewed = clients.connections[&observer].keep_connected_since.unwrap();
            assert!(renewed >= renewed_after);
            assert_eq!(clients.observer_idle_since, Some(old_origin));
            assert_eq!(
                shared.observer_deadline(&clients),
                Some(renewed + shared.observer_minute * 60)
            );
            assert!(!clients.shutting_down);
            // A participant departing after the latest hello starts a full new
            // idle grace; an older renewal must never shorten that grace.
            let later_idle_origin = renewed + Duration::from_secs(1);
            clients.observer_idle_since = Some(later_idle_origin);
            assert_eq!(
                shared.observer_deadline(&clients),
                Some(later_idle_origin + shared.observer_minute * 60)
            );
            // Advance the idle/renewal instants deterministically instead of
            // making this test wait a real hour for a hung Never observer.
            clients.observer_idle_since = Some(old_origin);
            clients
                .connections
                .get_mut(&observer)
                .unwrap()
                .keep_connected_since = Some(old_origin);
        }
        assert!(shutdown_rx.try_recv().is_err());
        shared.update_observer_backstop();
        shutdown_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("a hung Never observer must release its expired hold");
        assert!(shared.clients.lock_recover().shutting_down);
        assert!(!accept_or_refuse_client(&shared.clients));
    }

    #[test]
    fn broker_never_hold_does_not_change_legacy_null_or_finite_deadlines() {
        let shared = broker_shared_for_test();
        let observer = accept_client(&shared.clients).unwrap();
        let origin = Instant::now() - Duration::from_secs(120);
        shared.clients.lock_recover().observer_idle_since = Some(origin);
        for (params, backstop_minutes) in [
            (json!({"role":"observer"}), 60),
            (json!({"role":"observer","idlePolicyMinutes":null}), 60),
            (
                json!({"role":"observer","idlePolicyMinutes":null,"keepConnected":false}),
                60,
            ),
            (
                json!({"role":"observer","idlePolicyMinutes":15,"keepConnected":false}),
                20,
            ),
        ] {
            let mut enable = rpc_request("broker.hello");
            enable.params = json!({"role":"observer","keepConnected":true});
            assert_eq!(
                dispatch_broker_client_request(&shared, Some(observer), enable)["result"]
                    ["accepted"],
                true
            );
            assert!(shared.clients.lock_recover().connections[&observer]
                .keep_connected_since
                .is_some());
            let mut request = rpc_request("broker.hello");
            request.params = params;
            assert_eq!(
                dispatch_broker_client_request(&shared, Some(observer), request)["result"]
                    ["accepted"],
                true
            );
            let clients = shared.clients.lock_recover();
            assert!(clients.connections[&observer]
                .keep_connected_since
                .is_none());
            assert_eq!(clients.observer_idle_since, Some(origin));
            assert_eq!(
                shared.observer_deadline(&clients),
                Some(origin + Duration::from_secs(backstop_minutes * 60))
            );
        }
    }

    #[test]
    fn broker_never_hold_refuses_invalid_flags_without_changing_the_connection() {
        let shared = broker_shared_for_test();
        let observer = accept_client(&shared.clients).unwrap();
        let mut initial = rpc_request("broker.hello");
        initial.params = json!({"role":"observer","provider":"tray","keepConnected":true});
        assert_eq!(
            dispatch_broker_client_request(&shared, Some(observer), initial)["result"]["accepted"],
            true
        );
        let lease = shared.clients.lock_recover().connections[&observer].keep_connected_since;
        assert!(lease.is_some());
        for params in [
            json!({"role":"participant","keepConnected":true}),
            json!({"keepConnected":true}),
            json!({"role":"observer","idlePolicyMinutes":1,"keepConnected":true}),
            json!({"role":"observer","idlePolicyMinutes":1440,"keepConnected":true}),
            json!({"role":"observer","idlePolicyMinutes":0,"keepConnected":true}),
            json!({"role":"observer","keepConnected":"true"}),
            json!({"role":"observer","keepConnected":1}),
            json!({"role":"observer","keepConnected":null}),
        ] {
            let mut request = rpc_request("broker.hello");
            request.params = params;
            let response = dispatch_broker_client_request(&shared, Some(observer), request);
            assert_eq!(response["error"]["code"], -32602);
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.count, 1);
            assert_eq!(clients.observers(), 1);
            assert_eq!(clients.connections[&observer].provider, "tray");
            assert_eq!(clients.connections[&observer].keep_connected_since, lease);
            assert!(matches!(
                clients.connections[&observer].role,
                ConnectionRole::Observer {
                    idle_policy_minutes: None
                }
            ));
        }
        let participant =
            connection_hello(&json!({"role":"participant","keepConnected":false})).unwrap();
        assert_eq!(participant.role, ConnectionRole::Participant);
        assert!(participant.keep_connected_since.is_none());
    }

    #[test]
    fn failed_authentication_preserves_the_observer_idle_origin() {
        let shared = Arc::new(broker_shared_for_test());
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(15));
        let original = Instant::now() - Duration::from_secs(300);
        shared.clients.lock_recover().observer_idle_since = Some(original);

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let probe = accept_client(&shared.clients).unwrap();
        shared.update_observer_backstop();
        {
            let clients = shared.clients.lock_recover();
            // Layer 1 still sees the raw accept, and a pending authentication
            // still defers the observer backstop...
            assert_eq!(clients.count, 2);
            assert_eq!(clients.pending_authentications(), 1);
            assert_eq!(clients.observer_idle_since, Some(original));
            assert!(shared.observer_deadline(&clients).is_none());
            // ...but it is no participant: it cannot hold off the observer's
            // disconnect and is not listed to observers.
            assert_eq!(clients.participants(), 0);
            assert!(!clients.disconnect_blocked(Some(observer)));
            assert_eq!(clients.authenticated_rows().len(), 1);
        }
        assert!(
            !shared.disconnect_blocked_for(Some(observer), &shared.controller_for(Some(observer)))
        );
        write_response(
            &mut client,
            &json!({"id":0,"method":"broker.authenticate","params":{"token":"wrong"}}),
        )
        .unwrap();
        assert!(authenticate_tcp_client(&mut server, "expected", "instance").is_err());
        shared.detach_client(probe);
        {
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.observer_idle_since, Some(original));
            assert_eq!(
                shared.observer_deadline(&clients),
                Some(original + Duration::from_secs(20 * 60))
            );
        }
        shared.detach_client(observer);
    }

    /// A connected (client, server) pair on loopback.
    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_nodelay(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    fn auth_frame(token: &str) -> Value {
        json!({"method":"broker.authenticate","params":{"token":token},"id":0})
    }

    #[test]
    fn authentication_has_one_deadline_and_clears_it_once_authenticated() {
        // Silent after a partial header.
        let (mut client, mut server) = socket_pair();
        client.write_all(&[0, 0]).unwrap();
        let started = Instant::now();
        assert!(authenticate_tcp_client_within(
            &mut server,
            "expected",
            "instance",
            Duration::from_millis(300)
        )
        .is_err());
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(250), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");

        // Dripping a valid frame one byte per 100 ms: every individual read
        // succeeds well inside any per-read timeout, so only a whole-frame
        // deadline can end it.
        let (client, mut server) = socket_pair();
        let body = serde_json::to_vec(&auth_frame("expected")).unwrap();
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        let drip = test_support::FixtureJob::spawn_plain(move || {
            let mut client = client;
            for byte in frame {
                if client.write_all(&[byte]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        let started = Instant::now();
        assert!(authenticate_tcp_client_within(
            &mut server,
            "expected",
            "instance",
            Duration::from_millis(600)
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(server);
        drip.join().unwrap();

        // A prompt, valid handshake succeeds and leaves a blocking socket for
        // the authenticated serve loop.
        let (mut client, mut server) = socket_pair();
        write_response(&mut client, &auth_frame("expected")).unwrap();
        assert!(authenticate_tcp_client_within(
            &mut server,
            "expected",
            "instance",
            Duration::from_secs(2)
        )
        .is_ok());
        assert_eq!(server.read_timeout().unwrap(), None);
        assert_eq!(server.write_timeout().unwrap(), None);
        assert!(
            read_frame_value(&mut client).unwrap().unwrap()["result"]["instanceId"].is_string()
        );
    }

    #[test]
    fn the_authentication_frame_is_capped_at_four_kib_before_any_body_is_read() {
        for declared in [BROKER_AUTH_MAX_FRAME_BYTES + 1, domain::IPC_MAX_FRAME_BYTES] {
            let (mut client, mut server) = socket_pair();
            // Only the header is sent: an uncapped reader would wait for the
            // body until the authentication deadline.
            client.write_all(&(declared as u32).to_be_bytes()).unwrap();
            let started = Instant::now();
            assert!(authenticate_tcp_client(&mut server, "expected", "instance").is_err());
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "a {declared}-byte declaration was not refused from its header"
            );
            let refusal = read_frame_value(&mut client).unwrap().unwrap();
            assert_eq!(refusal["error"]["code"], BROKER_AUTH_ERROR_CODE);
        }

        // Exactly at the cap still authenticates.
        let (mut client, mut server) = socket_pair();
        let mut frame = auth_frame("expected");
        let unpadded = serde_json::to_vec(&frame).unwrap().len();
        frame["pad"] = json!("x".repeat(BROKER_AUTH_MAX_FRAME_BYTES - unpadded - 9));
        assert_eq!(
            serde_json::to_vec(&frame).unwrap().len(),
            BROKER_AUTH_MAX_FRAME_BYTES
        );
        write_response(&mut client, &frame).unwrap();
        assert!(authenticate_tcp_client(&mut server, "expected", "instance").is_ok());

        // After authentication the ordinary IPC limit applies again.
        let large = vec![b' '; 64 * 1024];
        let mut wire = (large.len() as u32).to_be_bytes().to_vec();
        wire.extend_from_slice(&large);
        assert_eq!(
            read_frame_bytes(&mut io::Cursor::new(wire))
                .unwrap()
                .unwrap(),
            large
        );
    }

    #[test]
    fn token_comparison_accepts_only_the_exact_token() {
        for wrong in ["", "expecte", "expectee", "expectedd", "EXPECTED"] {
            let (mut client, mut server) = socket_pair();
            write_response(&mut client, &auth_frame(wrong)).unwrap();
            assert!(
                authenticate_tcp_client(&mut server, "expected", "instance").is_err(),
                "{wrong:?} must not authenticate"
            );
            let refusal = read_frame_value(&mut client).unwrap().unwrap();
            assert_eq!(refusal["error"]["code"], BROKER_AUTH_ERROR_CODE);
        }
        let (mut client, mut server) = socket_pair();
        write_response(&mut client, &auth_frame("expected")).unwrap();
        assert!(authenticate_tcp_client(&mut server, "expected", "instance").is_ok());
    }

    #[test]
    fn pending_authentication_budget_is_distinct_from_the_total_owner_budget() {
        let shared = Arc::new(broker_shared_for_test());
        let pending: Vec<u64> = (0..BROKER_MAX_PENDING_AUTHENTICATIONS)
            .map(|_| accept_client(&shared.clients).unwrap())
            .collect();
        assert!(accept_client(&shared.clients).is_none());
        assert_eq!(
            shared.clients.lock_recover().count,
            BROKER_MAX_PENDING_AUTHENTICATIONS,
            "a refused socket is never counted"
        );
        // Authenticating one frees its slot; authenticated clients never
        // count against the cap.
        shared.mark_client_authenticated(pending[0]).unwrap();
        let next = accept_client(&shared.clients).unwrap();
        assert!(accept_client(&shared.clients).is_none());
        shared.detach_client(pending[1]);
        let after_close = accept_client(&shared.clients).unwrap();
        assert!(accept_client(&shared.clients).is_none());
        assert_eq!(shared.clients.lock_recover().participants(), 1);
        for id in pending
            .iter()
            .copied()
            .filter(|&id| id != pending[1])
            .chain([next, after_close])
        {
            shared.detach_client(id);
        }
        assert_eq!(shared.clients.lock_recover().count, 0);
    }

    #[test]
    fn excess_unauthenticated_sockets_are_closed_on_accept() {
        let (mut scratch, path) = broker_scratch_descriptor_path("pending-cap");
        let broker = start_broker_serve_for_test(&path);
        scratch.served_by(&broker);
        let mut raw: Vec<TcpStream> = (0..BROKER_MAX_PENDING_AUTHENTICATIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", broker.port)).unwrap())
            .collect();
        let mut excess = TcpStream::connect(("127.0.0.1", broker.port)).unwrap();
        excess
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let started = Instant::now();
        let mut byte = [0_u8; 1];
        match excess.read(&mut byte) {
            Ok(0) => {}
            Ok(_) => panic!("a refused socket must not be answered"),
            Err(error) => assert!(
                !matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ),
                "the excess socket was held open instead of closed: {error}"
            ),
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        // Closing one pending socket frees a slot for a real client.
        drop(raw.pop());
        let deadline = Instant::now() + Duration::from_secs(3);
        let authenticated = loop {
            let mut client = TcpStream::connect(("127.0.0.1", broker.port)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            if write_response(&mut client, &auth_frame(&broker.token)).is_ok() {
                if let Ok(Some(response)) = read_frame_value(&mut client) {
                    if response.get("result").is_some() {
                        break client;
                    }
                }
            }
            assert!(Instant::now() < deadline, "no slot freed after a close");
            thread::sleep(Duration::from_millis(50));
        };
        drop(authenticated);
        drop(raw);
    }

    #[test]
    fn repeated_raw_probes_cannot_extend_an_expired_observer_backstop() {
        let (shared, shutdown) = observer_test_shared();
        // Keep at least one raw socket accounted throughout setup and churn.
        // This protects it from expiry, without authenticating a participant.
        let mut probe = accept_client(&shared.clients).unwrap();
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(15));
        let expired_origin = Instant::now() - Duration::from_secs(1);
        shared.clients.lock_recover().observer_idle_since = Some(expired_origin);
        for _ in 0..8 {
            let next = accept_client(&shared.clients).unwrap();
            shared.detach_client(probe);
            probe = next;
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.count, 2);
            assert_eq!(clients.participants(), 0);
            assert_eq!(clients.pending_authentications(), 1);
            assert_eq!(clients.observer_idle_since, Some(expired_origin));
            assert!(shared.observer_deadline(&clients).is_none());
            assert!(!clients.shutting_down);
        }
        assert!(shutdown.try_recv().is_err());
        shared.detach_client(probe);
        shutdown
            .recv_timeout(Duration::from_secs(2))
            .expect("the original expired deadline applies as soon as the raw probe closes");
        assert!(shared.clients.lock_recover().shutting_down);
        shared.detach_client(observer);
    }

    #[test]
    fn an_authenticated_legacy_connection_resets_observer_idle_without_hello() {
        let shared = Arc::new(broker_shared_for_test());
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(15));
        let original = Instant::now() - Duration::from_secs(300);
        shared.clients.lock_recover().observer_idle_since = Some(original);
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let participant = accept_client(&shared.clients).unwrap();
        assert_eq!(
            shared.clients.lock_recover().observer_idle_since,
            Some(original)
        );
        let serving = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                assert!(authenticate_tcp_client(&mut server, "expected", "instance").is_ok());
                serve_broker_tcp_connection(server, &serving, participant, false);
                serving.detach_client(participant);
            });
        write_response(
            &mut client,
            &json!({"id":0,"method":"broker.authenticate","params":{"token":"expected"}}),
        )
        .unwrap();
        assert!(read_frame_value(&mut client)
            .unwrap()
            .unwrap()
            .get("result")
            .is_some());
        // This cached legacy request proves the authenticated serve loop ran;
        // the client never sends broker.hello.
        assert!(send_request(&mut client, "system.status", json!({}))
            .get("result")
            .is_some());
        {
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.participants(), 1);
            assert!(clients.observer_idle_since.is_none());
            assert!(shared.observer_deadline(&clients).is_none());
        }
        let left_at = Instant::now();
        drop(client);
        connection.join().unwrap();
        let fresh_origin = shared.clients.lock_recover().observer_idle_since.unwrap();
        assert!(fresh_origin >= left_at);
        assert!(fresh_origin > original);
        // Replacing a row through hello keeps it authenticated in both roles.
        let mut hello = rpc_request("broker.hello");
        hello.params = json!({"role":"participant"});
        shared.clients.lock_recover().idle_release_retry_not_before =
            Some(Instant::now() + Duration::from_secs(60));
        assert!(
            dispatch_broker_client_request(&shared, Some(observer), hello)
                .get("result")
                .is_some()
        );
        let clients = shared.clients.lock_recover();
        assert_eq!(clients.participants(), 1);
        assert!(clients.idle_release_retry_not_before.is_none());
        drop(clients);
        declare_observer(&shared, observer, json!(15));
        assert!(shared.clients.lock_recover().connections[&observer].authenticated);
        shared.detach_client(observer);
    }

    #[test]
    fn observer_backstop_uses_each_policy_plus_five_or_the_no_policy_fallback() {
        let (shared, _rx) = observer_test_shared();
        let first = accept_client(&shared.clients).unwrap();
        let second = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, first, Value::Null);
        declare_observer(&shared, second, json!(15));
        let check = |minutes| {
            let clients = shared.clients.lock_recover();
            assert_eq!(
                shared.observer_deadline(&clients).unwrap() - clients.observer_idle_since.unwrap(),
                shared.observer_minute * minutes
            );
        };
        check(60);
        declare_observer(&shared, first, json!(1440));
        check(1445);
        declare_observer(&shared, second, json!(60));
        check(1445);
        declare_observer(&shared, first, Value::Null);
        check(65);
        declare_observer(&shared, second, json!(15));
        check(60);
        declare_observer(&shared, first, json!(1));
        check(20);
        declare_observer(&shared, second, json!(1));
        check(6);
        // End the worker without leaving a timer behind this test.
        shared.detach_client(first);
        shared.detach_client(second);
    }

    #[test]
    fn observer_backstop_fires_and_participant_attachment_cancels_it() {
        let (shared, rx) = observer_test_shared();
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(15));
        let participant = accept_authenticated(&shared.clients);
        shared.update_observer_backstop();
        assert!(
            rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "even a completely silent participant must prevent reap"
        );
        shared.detach_client(participant);
        rx.recv_timeout(Duration::from_secs(2))
            .expect("last participant leaving arms backstop");
        assert!(shared.clients.lock_recover().shutting_down);
        assert!(!accept_or_refuse_client(&shared.clients));
        assert!(
            rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "only one shutdown is committed"
        );
    }

    #[test]
    fn observer_backstop_never_starts_for_a_silent_legacy_participant() {
        let (shared, rx) = observer_test_shared();
        assert!(accept_or_refuse_client(&shared.clients));
        shared.update_observer_backstop();
        assert!(rx.recv_timeout(Duration::from_millis(250)).is_err());
        let clients = shared.clients.lock_recover();
        assert!(clients.observer_idle_since.is_none());
        assert!(!clients.observer_timer_running);
    }

    #[test]
    fn resent_hello_recomputes_deadline_without_detaching_or_resetting_idle_time() {
        let (shared, rx) = observer_test_shared();
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(1));
        let since = shared.clients.lock_recover().observer_idle_since;
        declare_observer(&shared, observer, json!(1440));
        {
            let clients = shared.clients.lock_recover();
            assert_eq!(clients.count, 1);
            assert_eq!(clients.observers(), 1);
            assert_eq!(clients.observer_idle_since, since);
        }
        assert!(
            rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "old deadline must be invalidated"
        );
        // Shorten it below already elapsed idle time: release promptly, not
        // after a freshly restarted grace period.
        declare_observer(&shared, observer, Value::Null);
        rx.recv_timeout(Duration::from_secs(1))
            .expect("recomputed shorter deadline expires");
    }

    #[test]
    fn shutdown_rechecks_a_participant_that_arrives_behind_the_request_lock() {
        let (shared, _rx) = observer_test_shared();
        let observer = accept_client(&shared.clients).unwrap();
        declare_observer(&shared, observer, json!(1440));
        let held = shared.request_lock.lock_recover();
        let (started_tx, started_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let waiting = Arc::clone(&shared);
        let worker = test_support::FixtureJob::spawn_plain(move || {
            response_tx
                .send(request_broker_shutdown_after_precheck(
                    &waiting,
                    rpc_request("broker.shutdown"),
                    None,
                    || started_tx.send(()).unwrap(),
                ))
                .unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(response_rx.try_recv().is_err());
        let participant = accept_authenticated(&shared.clients);
        drop(held);
        let response = response_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("left open"));
        assert!(!shared.clients.lock_recover().shutting_down);
        worker.join().unwrap();
        shared.detach_client(observer);
        shared.detach_client(participant);
    }

    /// The wire names the app's reader (`DeviceBrokerDescriptor` in
    /// `apps/windows/src-tauri/src/device_broker_descriptor.rs`) expects,
    /// including the creation time it checks a live pid against.
    #[test]
    fn descriptor_records_this_process_creation_time_under_the_shared_wire_names() {
        let created_at = own_process_created_at();
        #[cfg(windows)]
        assert!(
            created_at.is_some_and(|value| value > 0),
            "a broker can always read its own creation time"
        );
        assert_eq!(
            own_process_created_at(),
            created_at,
            "a process's creation time never changes, so readers can re-derive it"
        );
        let descriptor = serde_json::to_value(BrokerDescriptor {
            port: 1,
            bearer_token: "t".into(),
            instance_id: "i".into(),
            process_id: std::process::id(),
            process_created_at: created_at,
            published_at_unix_ms: 5,
            gateway_api_version: generated_gateway::API_VERSION,
        })
        .unwrap();
        let mut keys: Vec<&str> = descriptor
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "bearerToken",
                "gatewayApiVersion",
                "instanceId",
                "port",
                "processCreatedAt",
                "processId",
                "publishedAtUnixMs"
            ]
        );
        assert_eq!(descriptor["processCreatedAt"], json!(created_at));
        assert_eq!(
            descriptor["gatewayApiVersion"],
            json!(generated_gateway::API_VERSION)
        );
    }

    #[test]
    fn two_tcp_clients_both_get_responses_and_both_receive_state_frame_pushes() {
        let (mut scratch, descriptor_path) = broker_scratch_descriptor_path("two-clients");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);

        let mut client_a = authenticated_tcp_client(serve.port, &serve.token);
        let mut client_b = authenticated_tcp_client(serve.port, &serve.token);

        let response_a = send_request(&mut client_a, "device.status", json!({}));
        assert!(
            response_a.get("result").is_some(),
            "client A did not get a response: {response_a:?}"
        );
        let response_b = send_request(&mut client_b, "device.status", json!({}));
        assert!(
            response_b.get("result").is_some(),
            "client B did not get a response: {response_b:?}"
        );

        serve
            .controller
            .inject_state_frame_for_test(DecodedStateFrame {
                stream_id: "tcp-test-stream".into(),
                sequence: 1,
                observed_at: 0,
                states: vec![StateUpdate::empty("tcp-test")],
                tempo_clock: None,
            });

        let push_a = read_notification(&mut client_a, "device.stateFrame");
        let push_b = read_notification(&mut client_b, "device.stateFrame");
        assert!(
            push_a.is_some(),
            "client A never received the state frame push"
        );
        assert!(
            push_b.is_some(),
            "client B never received the state frame push"
        );
    }

    #[test]
    fn broker_exits_5s_after_last_client_disconnects_unless_a_new_one_arrives() {
        let (mut scratch, descriptor_path) = broker_scratch_descriptor_path("grace-period");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);

        let client = authenticated_tcp_client(serve.port, &serve.token);
        drop(client);

        // The peer's drop precedes the server observing EOF and retiring its
        // connection actors. Measure grace only once detach is observed.
        let detach_deadline = Instant::now() + Duration::from_secs(3);
        while serve.handle.0.clients.lock_recover().count != 0 {
            assert!(
                Instant::now() < detach_deadline,
                "server did not observe client departure"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(serve.handle.0.idle_grace(), Duration::from_secs(5));
        let disconnected_at = std::time::Instant::now();
        thread::sleep(Duration::from_millis(500));
        assert!(
            serve.alive(),
            "the broker must remain alive through the idle-shutdown grace period"
        );

        let deadline = disconnected_at + IDLE_SHUTDOWN_GRACE + Duration::from_secs(3);
        while serve.alive() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !serve.alive(),
            "the broker must exit after the 5s grace period with no new client"
        );
    }

    #[test]
    fn a_new_client_during_the_grace_period_cancels_the_pending_shutdown() {
        let (mut scratch, descriptor_path) = broker_scratch_descriptor_path("grace-period-cancel");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);

        let client = authenticated_tcp_client(serve.port, &serve.token);
        drop(client);
        thread::sleep(Duration::from_secs(2));
        let _second_client = authenticated_tcp_client(serve.port, &serve.token);
        thread::sleep(Duration::from_secs(4));
        assert!(
            serve.alive(),
            "a new client within the grace period must cancel shutdown"
        );
    }

    /// Device MIDI workers are keyed and lazy: publication performs no
    /// unbound WinMM open, and only a realtime request creates an owner.
    #[test]
    fn midi_workers_start_lazily_without_unbound_warm_up() {
        let workers = PerformanceMidiWorkers::default();
        assert!(workers.workers.lock().unwrap().is_empty());
    }

    #[test]
    fn publication_and_authentication_do_not_warm_unbound_midi() {
        let (mut scratch, descriptor_path) =
            broker_scratch_descriptor_path("warm-up-after-publish");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);
        let mut client = authenticated_tcp_client(serve.port, &serve.token);
        let status = send_request(&mut client, "system.status", json!({}));
        assert_eq!(
            status["result"]["gatewayApiVersion"],
            json!(generated_gateway::API_VERSION)
        );
    }

    /// A deferred auto-connect action the test holds open until it says
    /// otherwise, and can ask about: `finished` turns true only once the
    /// action has run to the end.
    struct GatedReconnect {
        release: mpsc::Sender<()>,
        finished: Arc<AtomicBool>,
    }
    impl Drop for GatedReconnect {
        fn drop(&mut self) {
            let _ = self.release.send(());
        }
    }

    fn gated_reconnect() -> (
        GatedReconnect,
        impl FnOnce(&DeviceController) + Send + 'static,
    ) {
        let (release, released) = mpsc::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_by_reconnect = Arc::clone(&finished);
        let perform_auto_connect = move |_: &DeviceController| {
            let _ = released.recv_timeout(Duration::from_secs(5));
            finished_by_reconnect.store(true, Ordering::Release);
        };
        (GatedReconnect { release, finished }, perform_auto_connect)
    }

    /// Publication means "ready to authenticate," and authentication never
    /// waits on the device connection, so a slow or stalled auto-connect
    /// handshake - which can take upwards of ten seconds against real
    /// hardware - must not delay publication or a status request.
    /// Before the fix,
    /// `DeviceController::start()` (auto-connect baked in at construction)
    /// ran in `cli.rs`'s `--broker-serve` branch ahead of
    /// `serve_broker_tcp`, so a spawning host's publish-wait budget - which
    /// covers only bind, the descriptor claim, and one file write - could
    /// elapse before the broker ever reached that call.
    #[test]
    fn publication_and_authentication_do_not_wait_for_auto_connect() {
        let (mut scratch, descriptor_path) =
            broker_scratch_descriptor_path("auto-connect-after-publish");
        let (gate, perform_auto_connect) = gated_reconnect();
        let serve =
            start_broker_serve_for_test_with_auto_connect(&descriptor_path, perform_auto_connect);
        scratch.served_by(&serve);

        let mut client = authenticated_tcp_client(serve.port, &serve.token);
        let status = send_request(&mut client, "system.status", json!({}));
        assert_eq!(
            status["result"]["gatewayApiVersion"],
            json!(generated_gateway::API_VERSION),
            "a request that does not need the device is answered during auto-connect: {status:?}"
        );
        assert!(
            !gate.finished.load(Ordering::Acquire),
            "all of the above happened while the deferred auto-connect action was still gated"
        );
        gate.release
            .send(())
            .expect("auto-connect is still waiting");
    }

    /// I6: the grace period used to be armed only by a disconnect, so a
    /// broker whose spawner died before attaching never exited.
    #[test]
    fn a_broker_no_client_ever_attaches_to_still_exits_after_the_grace_period() {
        let (mut scratch, descriptor_path) = broker_scratch_descriptor_path("never-attached");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);
        assert!(serve.alive());
        let deadline = std::time::Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(3);
        while serve.alive() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !serve.alive(),
            "a broker nobody ever attached to must still exit after its grace period"
        );
        assert!(
            !descriptor_path.exists(),
            "and it gives up its descriptor slot as it goes"
        );
    }

    /// I2 support: the broker tells a client how many clients it serves, so a
    /// session-ending call can be kept from reaching anyone but its caller.
    #[test]
    fn the_client_count_is_answered_by_the_broker_itself() {
        let (mut scratch, descriptor_path) = broker_scratch_descriptor_path("client-count");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);
        let mut first = authenticated_tcp_client(serve.port, &serve.token);
        let answer = send_request(&mut first, BROKER_CLIENT_COUNT_METHOD, json!({}));
        assert_eq!(answer["result"]["clients"], json!(1), "{answer:?}");

        let _second = authenticated_tcp_client(serve.port, &serve.token);
        let answer = send_request(&mut first, BROKER_CLIENT_COUNT_METHOD, json!({}));
        assert_eq!(answer["result"]["clients"], json!(2), "{answer:?}");
    }

    /// A wrong token is refused and the connection closed before any request
    /// can reach `handle`.
    #[test]
    fn a_client_with_the_wrong_token_is_refused_and_disconnected() {
        let (mut scratch, descriptor_path) = broker_scratch_descriptor_path("wrong-token");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);
        let mut client = TcpStream::connect(("127.0.0.1", serve.port)).expect("connects");
        write_response(
            &mut client,
            &json!({"method": "broker.authenticate", "params": {"token": "not-the-token"}, "id": 0}),
        )
        .expect("auth frame sent");
        let refusal = read_frame_value(&mut client)
            .expect("refusal readable")
            .expect("refusal present");
        assert_eq!(refusal["error"]["code"], json!(BROKER_AUTH_ERROR_CODE));
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout");
        let _ = write_response(
            &mut client,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "system.status", "params": {}}),
        );
        assert!(
            !matches!(read_frame_value(&mut client), Ok(Some(_))),
            "nothing is answered on a connection that failed authentication"
        );
    }

    fn broker_shared_for_test() -> BrokerShared {
        BrokerShared {
            controller: Arc::new(DeviceController::start_disconnected().expect("controller")),
            devices: Mutex::new(Vec::new()),
            performance_midi: Arc::new(PerformanceMidiWorkers::default()),
            request_lock: Arc::new(Mutex::new(())),
            clients: Arc::new(Mutex::new(ClientAccounting::default())),
            descriptor_path: std::env::temp_dir()
                .join(format!("qc-broker-unused-{}.json", generate_token(8))),
            instance_id: generate_token(16),
            descriptor_claim: Arc::new(Mutex::new(None)),
            on_idle_shutdown: Arc::new(|| {}),
            observer_changed: Condvar::new(),
            background: background::Manager::default(),
            observer_actions: background::Manager::default(),
            demo: crate::demo_provider::Hub::default(),
            targets: crate::target::TargetRegistry::default(),
            observer_minute: Duration::from_secs(60),
        }
    }

    #[test]
    fn icon_connect_waits_for_release_admission_and_refuses_committed_shutdown() {
        let shared = Arc::new(broker_shared_for_test());
        shared.clients.lock_recover().idle_released = true;
        let exclusive = shared.request_lock.lock_recover();
        let handle = BrokerHandle(Arc::clone(&shared));
        let (started, admitted) = std::sync::mpsc::channel();
        let (result, completed) = std::sync::mpsc::channel();
        let original = thread::spawn(move || {
            started.send(()).unwrap();
            result.send(handle.reconnect()).unwrap();
        });
        admitted.recv_timeout(Duration::from_secs(2)).unwrap();
        let early = completed.recv_timeout(Duration::from_millis(100));
        shared.clients.lock_recover().shutting_down = true;
        drop(exclusive);
        // Release the actual barrier before joining the original on every
        // assertion path; no test ever starts a real USB reconnect.
        original.join().unwrap();
        assert!(
            early.is_err(),
            "Connect must not bypass an in-progress release"
        );
        let outcome = completed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(outcome
            .unwrap_err()
            .contains("shutdown is already committed"));
        assert!(shared.clients.lock_recover().idle_released);
    }

    #[test]
    fn unverified_shutdown_retains_descriptor_until_actual_worker_retirement() {
        struct Release(Arc<AtomicBool>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let release = Arc::new(AtomicBool::new(false));
        let _release = Release(Arc::clone(&release));
        let (_scratch, path) = broker_scratch_descriptor_path("shutdown-retains-claim");
        let shared = BrokerShared {
            controller: Arc::new(DeviceController::with_stuck_worker_for_test(Arc::clone(
                &release,
            ))),
            descriptor_path: path.to_path_buf(),
            descriptor_claim: Arc::new(Mutex::new(Some(
                qc_owner_only_file::ClaimedFile::try_claim(&path).unwrap(),
            ))),
            ..broker_shared_for_test()
        };
        let report = shared.finish_shutdown();
        assert!(!report.all_verified());
        assert!(shared.descriptor_claim.lock_recover().is_some());
        assert!(
            qc_owner_only_file::ClaimedFile::try_claim(&path).is_err(),
            "unknown original still excludes a successor"
        );
        release.store(true, Ordering::Release);
        let deadline = Instant::now() + crate::worker::WORKER_EXIT_BUDGET;
        while !shared.controller.fixture_worker_acknowledged() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            shared.controller.fixture_worker_acknowledged(),
            "original worker did not acknowledge retirement before the bounded deadline"
        );
        let report = shared.finish_shutdown();
        assert!(report.all_verified(), "{}", report.summary());
        assert!(shared.descriptor_claim.lock_recover().is_none());
        qc_owner_only_file::ClaimedFile::try_claim(&path)
            .unwrap()
            .release()
            .unwrap();
    }

    #[test]
    fn a_release_whose_worker_never_stops_is_not_reported_as_clean() {
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let controller = DeviceController::with_stuck_worker_for_test(Arc::clone(&release));
        let handle = BrokerHandle(Arc::new(BrokerShared {
            controller: Arc::new(controller),
            ..broker_shared_for_test()
        }));
        let started = Instant::now();
        let outcome = handle.release_and_exit(false).unwrap();
        let elapsed = started.elapsed();
        release.store(true, std::sync::atomic::Ordering::Release);
        assert!(
            outcome.contains("clean disconnect NOT verified for 1"),
            "a timed-out worker must not read as a clean release: {outcome}"
        );
        // The stuck worker never acks either pre-existing disconnect (the
        // shutdown request's and `finish_shutdown`'s, 1 s each), then one
        // shared worker budget. Not one budget per controller.
        assert!(
            elapsed
                < Duration::from_secs(2)
                    + crate::worker::WORKER_EXIT_BUDGET
                    + Duration::from_secs(1),
            "{elapsed:?}"
        );
    }

    #[test]
    fn forced_release_is_recorded_even_when_a_participant_cannot_receive_its_notice() {
        let shared = broker_shared_for_test();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (writer, _) = listener.accept().unwrap();
        let output = Arc::new(Mutex::new(writer));
        let participant = accept_authenticated(&shared.clients);
        shared
            .clients
            .lock_recover()
            .connections
            .get_mut(&participant)
            .unwrap()
            .output = Some(Arc::clone(&output));
        let stalled_writer = output.lock_recover();
        let mut request = rpc_request("broker.shutdown");
        request.params = json!({"force":true});
        let response = request_broker_shutdown(&shared, request);
        assert_eq!(response["result"]["shutdown"], true);
        let marker_path =
            BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id);
        let marker: BrokerReleaseMarker =
            serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
        assert_eq!(marker.instance_id, shared.instance_id);
        assert!(marker.forced);
        assert!(qc_owner_only_file::is_owner_only(&marker_path).unwrap());
        // Hold the writer throughout the entire broadcast, deterministically
        // excluding socket delivery. The durable generation survives exit.
        shared.notify_shutdown(u64::MAX, true);
        shared.finish_shutdown();
        assert!(marker_path.exists());
        drop(stalled_writer);
        std::fs::remove_file(marker_path).unwrap();
    }

    #[test]
    fn a_failed_release_record_refuses_shutdown_before_committing_it() {
        let mut shared = broker_shared_for_test();
        let parent = std::env::temp_dir().join(format!("qc-release-blocked-{}", generate_token(8)));
        std::fs::write(&parent, b"a file cannot be a release directory").unwrap();
        shared.descriptor_path = parent.join("device-broker.json");
        let mut request = rpc_request("broker.shutdown");
        request.params = json!({"force":true});
        let response = request_broker_shutdown(&shared, request);
        assert_eq!(response["error"]["code"], -32002);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("device was left open"));
        assert!(!shared.clients.lock_recover().shutting_down);
        std::fs::remove_file(parent).unwrap();
    }

    #[test]
    fn ordinary_idle_shutdown_does_not_record_a_forced_release() {
        let shared = broker_shared_for_test();
        let response = request_broker_shutdown(&shared, rpc_request("broker.shutdown"));
        assert_eq!(response["result"]["shutdown"], true);
        assert!(
            !BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id).exists()
        );
    }

    /// Regression for the race `ClientAccounting`'s own doc comment
    /// describes: a bare `client_count: AtomicUsize` let the accept loop's
    /// increment and a grace thread's zero-check-then-exit interleave with
    /// no coordination. `arm_idle_shutdown` is armed on every disconnect-to-
    /// zero, and nothing used to stop two grace threads from being live at
    /// once (a client connecting and disconnecting again within one grace
    /// period arms a second). Proves only one of two concurrently armed
    /// grace threads actually commits to shutting down - the `shutting_down`
    /// flag under the same lock as `count`, not the count alone, is what
    /// makes that true.
    #[test]
    fn only_one_of_two_concurrently_armed_grace_threads_shuts_down() {
        let shared = Arc::new(broker_shared_for_test());
        let shutdown_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&shutdown_calls);
        // Replaced after construction since `on_idle_shutdown` needs to
        // count calls this test can observe; `broker_shared_for_test`'s
        // no-op is fine for every other test that doesn't care.
        let shared = Arc::new(BrokerShared {
            on_idle_shutdown: Arc::new(move || {
                counted.fetch_add(1, Ordering::AcqRel);
            }),
            ..Arc::try_unwrap(shared).unwrap_or_else(|_| unreachable!("sole owner"))
        });
        shared.arm_idle_shutdown();
        shared.arm_idle_shutdown();
        let deadline = std::time::Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(3);
        while shutdown_calls.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            shutdown_calls.load(Ordering::Acquire),
            1,
            "exactly one grace thread must commit to shutting down, not zero"
        );
        // Both threads were armed back to back and sleep the identical
        // IDLE_SHUTDOWN_GRACE, so an unguarded second thread would typically
        // wake within milliseconds of the first - but heavy concurrent test
        // load means a single fixed grace window here
        // previously risked passing on the old, unguarded AtomicUsize code
        // purely because the second thread hadn't been scheduled yet, not
        // because it was actually prevented from running. Polling
        // continuously for several seconds, not sleeping once, means any
        // second call arriving late under contention is still caught instead
        // of silently missed.
        let watch_until = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < watch_until {
            assert_eq!(
                shutdown_calls.load(Ordering::Acquire),
                1,
                "a second grace thread must never commit once the first already has"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            shared.clients.lock_recover().shutting_down,
            "the accounting must record that shutdown was committed to"
        );
    }

    /// A connection accepted after a grace thread has already committed to
    /// shutting down must be refused, not accepted and then torn down when
    /// the process exits anyway - the other half of the same race. Exercises
    /// the real `accept_or_refuse_client`, the exact function the accept
    /// loop calls, not a re-derived check.
    #[test]
    fn a_connection_after_shutdown_is_committed_to_is_refused() {
        let shared = broker_shared_for_test();
        shared.clients.lock_recover().shutting_down = true;
        assert!(
            !accept_or_refuse_client(&shared.clients),
            "a connection arriving after commit must be refused"
        );
        assert_eq!(
            shared.clients.lock_recover().count,
            0,
            "a refused connection must never be counted"
        );
    }

    #[test]
    fn a_connection_before_shutdown_is_committed_to_is_accepted_and_counted() {
        let shared = broker_shared_for_test();
        assert!(accept_or_refuse_client(&shared.clients));
        assert_eq!(shared.clients.lock_recover().count, 1);
    }

    /// The race `apps/windows/src-tauri`'s own client-side pre-check cannot
    /// close on its own: two separate RPCs (`broker.clientCount`, then
    /// `device.disconnect`) are never atomic with respect to a second client
    /// attaching in between. The broker now re-validates at the moment
    /// `device.disconnect` actually arrives, using the same lock the accept
    /// loop holds - so even a disconnect sent while the caller genuinely
    /// believed it was alone must be answered as "left open" once a second
    /// client is attached by the time it's processed.
    #[test]
    fn a_disconnect_arriving_while_a_second_client_is_attached_leaves_the_session_open() {
        let (mut scratch, descriptor_path) =
            broker_scratch_descriptor_path("disconnect-second-client-race");
        let serve = start_broker_serve_for_test(&descriptor_path);
        scratch.served_by(&serve);
        let mut first = authenticated_tcp_client(serve.port, &serve.token);
        let mut second = authenticated_tcp_client(serve.port, &serve.token);

        let response = send_request(&mut first, "device.disconnect", json!({}));
        assert_eq!(
            response["result"]["detail"],
            json!("Another client is still attached; the shared session was left open"),
            "a disconnect while a second client is attached must not report a real teardown: {response:?}"
        );

        // Both connections remain open and counted - a real disconnect would
        // still leave the TCP connections themselves alive too, but this
        // proves the broker didn't tear down the shared session state.
        let status = send_request(&mut second, "broker.clientCount", json!({}));
        assert_eq!(status["result"]["clients"], json!(2));
        drop(first);
        drop(second);
    }

    /// The gap behind the flaky variant of the test above: the broker used to
    /// write the authentication reply and only then count the connection as a
    /// participant, so a disconnect from the first client processed in between
    /// saw nobody else attached. The accounting lock is held here so the
    /// registration provably cannot have happened yet: the reply must not
    /// reach the client until it can, and once it has, the client is counted.
    #[test]
    fn a_client_is_counted_as_a_participant_before_it_is_told_it_is_authenticated() {
        let shared = Arc::new(broker_shared_for_test());
        let first = accept_authenticated(&shared.clients);
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let second = accept_client(&shared.clients).unwrap();

        let registration_blocked = shared.clients.lock_recover();
        let serving = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                authenticate_and_serve_tcp_client(
                    server, &serving, second, "expected", "instance", false,
                )
            });
        write_response(
            &mut client,
            &json!({"id":0,"method":"broker.authenticate","params":{"token":"expected"}}),
        )
        .unwrap();
        // Poll without consuming or timing out: Winsock leaves a socket whose
        // receive timed out in an indeterminate state, so the reply read
        // below must not reuse one. A non-blocking peek never times out.
        client.set_nonblocking(true).unwrap();
        let quiet_until = Instant::now() + Duration::from_millis(500);
        let mut probe = [0_u8; 1];
        while Instant::now() < quiet_until {
            match client.peek(&mut probe) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                other => panic!(
                    "the authentication reply was sent before the connection was counted as a participant: {other:?}"
                ),
            }
        }
        client.set_nonblocking(false).unwrap();

        drop(registration_blocked);
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let reply = read_frame_value(&mut client)
            .expect("authentication reply readable")
            .expect("authentication reply present");
        assert!(reply.get("result").is_some(), "{reply}");
        assert_eq!(shared.clients.lock_recover().participants(), 2);
        assert!(
            shared.disconnect_blocked_for(Some(first), &shared.controller_for(Some(first))),
            "a client that has been told it is authenticated must block another's disconnect"
        );
        drop(client);
        connection.join().unwrap();
    }

    /// The race behind the flaky Linux SIGTERM test: the broker used to write
    /// the authentication reply and only then register the connection's
    /// writer, so a shutdown broadcast in between reached nobody and the
    /// client read a bare EOF. The connection thread is held right after its
    /// reply, before anything serves it, and the notice is broadcast there.
    #[test]
    fn a_client_told_it_is_authenticated_receives_a_shutdown_notice_sent_before_it_is_served() {
        let shared = Arc::new(broker_shared_for_test());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let pending = accept_client(&shared.clients).unwrap();
        let (reached_tx, reached) = mpsc::channel::<()>();
        let (release, release_rx) = mpsc::channel::<()>();
        let (reached_tx, release_rx) = (Mutex::new(reached_tx), Mutex::new(release_rx));
        TEST_POINTS.lock_recover().push((
            shared.instance_id.clone(),
            "tcp:authenticated",
            Arc::new(move || {
                let _ = reached_tx.lock_recover().send(());
                let _ = release_rx
                    .lock_recover()
                    .recv_timeout(Duration::from_secs(10));
                Ok(())
            }),
        ));

        let serving = Arc::clone(&shared);
        let connection = thread::spawn(move || {
            authenticate_and_serve_tcp_client(
                server, &serving, pending, "expected", "instance", false,
            )
        });
        write_response(
            &mut client,
            &json!({"id":0,"method":"broker.authenticate","params":{"token":"expected"}}),
        )
        .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let reply = read_frame_value(&mut client)
            .expect("authentication reply readable")
            .expect("authentication reply present");
        assert!(reply.get("result").is_some(), "{reply}");
        reached
            .recv_timeout(Duration::from_secs(5))
            .expect("the connection thread reached the point after its reply");

        shared.clients.lock_recover().shutting_down = true;
        shared.notify_shutdown(u64::MAX, true);
        let notice = read_frame_value(&mut client)
            .expect("a client told it is authenticated must get the shutdown notice, not EOF")
            .expect("shutdown notice present");
        assert_eq!(notice["method"], "broker.shuttingDown", "{notice}");

        drop(release);
        drop(client);
        connection.join().unwrap();
    }

    /// A non-forced `broker.shutdown` ignores sockets still authenticating,
    /// so it can commit while one is mid-handshake. That client must then be
    /// refused rather than counted and told it is authenticated by a broker
    /// that is about to exit. Sequenced, not timed: the shutdown commits and
    /// the token is on the wire before the connection thread ever starts.
    #[test]
    fn a_client_mid_authentication_is_refused_once_shutdown_is_committed() {
        let shared = Arc::new(broker_shared_for_test());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let pending = accept_client(&shared.clients).unwrap();

        let shutdown = request_broker_shutdown(&shared, rpc_request("broker.shutdown"));
        assert_eq!(shutdown["result"]["shutdown"], true, "{shutdown}");

        write_response(
            &mut client,
            &json!({"id":0,"method":"broker.authenticate","params":{"token":"expected"}}),
        )
        .unwrap();
        let serving = Arc::clone(&shared);
        let connection =
            test_support::FixtureJob::spawn_with_socket(server.try_clone().unwrap(), move || {
                authenticate_and_serve_tcp_client(
                    server, &serving, pending, "expected", "instance", false,
                )
            });
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let reply = read_frame_value(&mut client)
            .expect("authentication reply readable")
            .expect("authentication reply present");
        assert!(
            reply.get("result").is_none(),
            "a broker committed to shutting down told a client it is authenticated: {reply}"
        );
        assert_eq!(reply["error"]["code"], -32002, "{reply}");
        assert_eq!(
            reply["error"]["message"], "Broker shutdown is already committed",
            "{reply}"
        );
        connection.join().unwrap();
        let clients = shared.clients.lock_recover();
        assert_eq!(clients.participants(), 0);
        assert_eq!(clients.count, 0, "the refused connection was detached");
    }

    /// The narrower version of the same race: a disconnect can pass the
    /// first, cheap client-count check while genuinely alone, then queue
    /// behind `request_lock` (held here by a simulated in-flight operation
    /// from another client) long enough for a second client to attach before
    /// it actually runs. `dispatch_broker_request`'s second re-check, taken
    /// immediately before `handle()` executes the disconnect, is what this
    /// proves - a fix that only re-checked once, before waiting for
    /// `request_lock`, would not catch this.
    #[test]
    fn a_client_attaching_while_a_disconnect_is_queued_behind_the_request_lock_is_not_dropped() {
        let shared = Arc::new(broker_shared_for_test());
        accept_authenticated(&shared.clients);

        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (holding_tx, holding_rx) = mpsc::channel::<()>();
        let holder_shared = Arc::clone(&shared);
        let holder = test_support::FixtureJob::spawn_plain(move || {
            let _held = holder_shared
                .request_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = holding_tx.send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        });
        // Only proceed once the holder thread genuinely holds `request_lock`,
        // so the disconnect sent below is provably queued behind it.
        holding_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("holder thread took the lock");

        let dispatch_shared = Arc::clone(&shared);
        let dispatching = test_support::FixtureJob::spawn_plain(move || {
            dispatch_broker_request(&dispatch_shared, rpc_request("device.disconnect"))
        });

        // The second client attaches while the disconnect is still queued
        // behind the held lock. A short sleep here is only to make it likely
        // the dispatch thread has already reached (and is blocked on)
        // `request_lock.lock()` before the count changes and the lock is
        // released - not required for correctness, since the fix re-checks
        // regardless of exactly when the count changed relative to that.
        std::thread::sleep(Duration::from_millis(50));
        accept_authenticated(&shared.clients);
        let _ = release_tx.send(());
        holder.join().expect("holder thread");

        let response = dispatching.join().expect("dispatch thread");
        assert_eq!(
            response["result"]["detail"],
            json!("Another client is still attached; the shared session was left open"),
            "a disconnect that queued behind request_lock must re-check once it's finally \
             about to run, not just once before it started waiting: {response:?}"
        );
    }

    #[test]
    fn device_selection_is_local_to_a_connection_and_reuses_one_owner_per_unit() {
        let shared = broker_shared_for_test();
        let first = accept_authenticated(&shared.clients);
        let second = accept_authenticated(&shared.clients);
        shared.select_available_device(first, "fixture-A").unwrap();
        shared.select_available_device(second, "fixture-B").unwrap();
        let a = shared.controller_for(Some(first));
        let b = shared.controller_for(Some(second));
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(a.device_id().as_deref(), Some("fixture-A"));
        assert_eq!(b.device_id().as_deref(), Some("fixture-B"));
        assert!(
            !shared.disconnect_blocked_for(Some(first), &shared.controller_for(Some(first))),
            "B's owner cannot block A's disconnect"
        );
        let mut events = rpc_request(generated_gateway::rpc::CURRENT_STATE_EVENTS);
        events.params = json!({"afterSequence":0,"limit":1});
        let first_stream = dispatch_broker_client_request(&shared, Some(first), events)["result"]
            ["streamId"]
            .clone();
        let mut events = rpc_request(generated_gateway::rpc::CURRENT_STATE_EVENTS);
        events.params = json!({"afterSequence":0,"limit":1});
        let second_stream = dispatch_broker_client_request(&shared, Some(second), events)["result"]
            ["streamId"]
            .clone();
        assert!(first_stream.is_string());
        assert!(second_stream.is_string());
        assert_ne!(
            first_stream, second_stream,
            "each device has its own event journal"
        );
        shared.select_available_device(first, "fixture-B").unwrap();
        assert!(Arc::ptr_eq(&shared.controller_for(Some(first)), &b));
        assert!(Arc::ptr_eq(&shared.controller_for(Some(second)), &b));
        assert!(
            shared.disconnect_blocked_for(Some(first), &shared.controller_for(Some(first))),
            "shared ownership is still protected on the selected device"
        );
        let mut hello = rpc_request("broker.hello");
        hello.params = json!({"provider":"test","role":"participant"});
        assert!(dispatch_broker_client_request(&shared, Some(first), hello)
            .get("error")
            .is_none());
        assert!(
            Arc::ptr_eq(&shared.controller_for(Some(first)), &b),
            "re-announcing a provider cannot reset its device selection"
        );
        assert_eq!(shared.devices.lock_recover().len(), 1);
        assert!(
            !a.claim_device("fixture-B"),
            "a missing device never silently rebinds to another unit"
        );
    }

    #[test]
    fn an_observer_cannot_change_its_device_target() {
        let shared = broker_shared_for_test();
        let id = accept_client(&shared.clients).unwrap();
        let mut hello = rpc_request("broker.hello");
        hello.params = json!({"role":"observer"});
        assert!(dispatch_broker_client_request(&shared, Some(id), hello)
            .get("error")
            .is_none());
        assert!(shared.select_available_device(id, "fixture-B").is_err());
        assert_eq!(
            shared.controller.device_id(),
            None,
            "a refused observer cannot pin the default device"
        );
        assert!(Arc::ptr_eq(
            &shared.controller_for(Some(id)),
            &shared.controller
        ));
    }

    #[test]
    #[ignore = "manual attached-device MIDI identity check; never sends MIDI or USB commands"]
    fn attached_hid_has_an_unambiguous_matching_midi_output() {
        let devices = quadcortex_hid::usb::connected_devices().expect("enumerate HID");
        assert!(
            !devices.is_empty(),
            "a physical Quad Cortex must be attached"
        );
        let mut midi = PerformanceMidi::default();
        let mut matched = 0;
        for device in &devices {
            let endpoint = midi
                .warm_up_for_device(&device.id)
                .expect("match HID and MIDI physical containers");
            assert!(endpoint.to_lowercase().contains("quad cortex"));
            matched += 1;
        }
        assert!(matched > 0);
        assert_eq!(matched, devices.len());
        println!(
            "Matched HID and MIDI identity for {} attached unit(s); no commands sent",
            devices.len()
        );
    }

    fn rpc_request(method: &str) -> Request {
        Request {
            jsonrpc: "2.0".into(),
            id: json!(7),
            method: method.into(),
            params: json!({}),
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        }
    }

    /// I1: with several processes attached, `handle` is reached from several
    /// threads at once. A request that is not an exempt cached read must wait
    /// for the one in progress - here, one the test holds the lock for - so
    /// another client's multi-step flow cannot interleave with it.
    #[test]
    fn requests_from_different_clients_run_one_at_a_time() {
        let shared = Arc::new(broker_shared_for_test());
        let in_progress = shared.request_lock.lock().expect("request lock");

        let (answered_tx, answered) = mpsc::channel();
        let waiting = Arc::clone(&shared);
        let client = test_support::FixtureJob::spawn_plain(move || {
            let response = dispatch_broker_request(&waiting, rpc_request("device.snapshot"));
            let _ = answered_tx.send(response);
        });
        assert!(
            answered.recv_timeout(Duration::from_millis(300)).is_err(),
            "a device request must wait for the request already in progress"
        );

        // The 1 Hz health poll is not held up behind it.
        let status = dispatch_broker_request(&shared, rpc_request("system.status"));
        assert!(status.get("result").is_some(), "{status:?}");

        drop(in_progress);
        let response = answered
            .recv_timeout(Duration::from_secs(5))
            .expect("the waiting request runs once the lock is free");
        assert_eq!(response["id"], json!(7));
        client.join().expect("client thread");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn direct_disconnect_waits_for_raw_midi_close_ack_before_hid_disconnect() {
        struct CloseGate(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for CloseGate {
            fn drop(&mut self) {
                let (released, changed) = &*self.0;
                *released.lock_recover() = true;
                changed.notify_all();
            }
        }
        struct Endpoint {
            gate: Arc<(Mutex<bool>, Condvar)>,
            retiring: Arc<AtomicBool>,
        }
        impl PerformanceMidiEndpoint for Endpoint {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                Ok(PerformanceMidiReceipt {
                    endpoint: "fixture".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }
            fn retire(&mut self) {
                self.retiring.store(true, Ordering::Release);
                let (released, changed) = &*self.gate;
                let mut released = released.lock_recover();
                while !*released {
                    released = changed
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = CloseGate(Arc::clone(&gate));
        let retiring = Arc::new(AtomicBool::new(false));
        let factory_gate = Arc::clone(&gate);
        let factory_retiring = Arc::clone(&retiring);
        let midi = PerformanceMidiWorkers::with_factory(move || {
            Box::new(Endpoint {
                gate: Arc::clone(&factory_gate),
                retiring: Arc::clone(&factory_retiring),
            })
        });
        let controller = DeviceController::start_disconnected().expect("controller");
        assert!(controller.claim_device("direct-disconnect-fixture"));
        midi.send("direct-disconnect-fixture", 1, 2, || {})
            .expect("fixture MIDI send");
        let initial = controller.status();
        let (done_tx, done_rx) = mpsc::channel();
        let disconnecting = thread::spawn(move || {
            done_tx
                .send(handle(&controller, &midi, rpc_request("device.disconnect")))
                .unwrap();
            controller
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !retiring.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(retiring.load(Ordering::Acquire), "MIDI close entered");

        let result = done_rx
            .recv_timeout(PERFORMANCE_MIDI_RETIRE_BUDGET + Duration::from_secs(1))
            .expect("disconnect returns at the bounded MIDI retirement deadline");
        drop(_release_gate);
        let after_timeout = disconnecting.join().expect("disconnect thread");
        assert!(
            result["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("still draining"),
            "an unacknowledged close refuses HID disconnect: {result}"
        );
        assert_eq!(
            after_timeout.status().detail,
            initial.detail,
            "the HID worker must not receive disconnect before MIDI close acknowledgement"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resident_release_does_not_record_idle_release_before_midi_close_ack() {
        struct CloseGate(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for CloseGate {
            fn drop(&mut self) {
                let (released, changed) = &*self.0;
                *released.lock_recover() = true;
                changed.notify_all();
            }
        }
        struct Endpoint {
            gate: Arc<(Mutex<bool>, Condvar)>,
            retiring: Arc<AtomicBool>,
        }
        impl PerformanceMidiEndpoint for Endpoint {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                Ok(PerformanceMidiReceipt {
                    endpoint: "fixture".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }
            fn retire(&mut self) {
                self.retiring.store(true, Ordering::Release);
                let (released, changed) = &*self.gate;
                let mut released = released.lock_recover();
                while !*released {
                    released = changed
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let release_gate = CloseGate(Arc::clone(&gate));
        let retiring = Arc::new(AtomicBool::new(false));
        let factory_gate = Arc::clone(&gate);
        let factory_retiring = Arc::clone(&retiring);
        let midi = PerformanceMidiWorkers::with_factory(move || {
            Box::new(Endpoint {
                gate: Arc::clone(&factory_gate),
                retiring: Arc::clone(&factory_retiring),
            })
        });
        let mut shared = broker_shared_for_test();
        shared.controller.claim_device("resident-release-fixture");
        shared.performance_midi = Arc::new(midi);
        shared
            .performance_midi
            .send("resident-release-fixture", 1, 2, || {})
            .expect("fixture MIDI send");
        let shared = Arc::new(shared);
        let before = shared.clients.lock_recover().idle_released;
        let release_shared = Arc::clone(&shared);
        let (done_tx, done_rx) = mpsc::channel();
        let release = thread::spawn(move || {
            let result = BrokerHandle(release_shared)
                .release_devices_staying()
                .map(|_| ());
            done_tx.send(result).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !retiring.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(retiring.load(Ordering::Acquire), "MIDI close entered");
        let idle_released_before_ack = shared.clients.lock_recover().idle_released;

        drop(release_gate);
        let outcome = done_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("release completes after MIDI close acknowledgement");
        release.join().expect("release thread");
        outcome.expect("the acknowledged resident release succeeds");
        assert!(!before, "fixture starts with no idle release marker");
        assert!(
            !idle_released_before_ack,
            "idle release bookkeeping must follow the MIDI close acknowledgement"
        );
        assert!(shared.clients.lock_recover().idle_released);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shutdown_retains_descriptor_claim_until_raw_midi_close_ack() {
        struct CloseGate(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for CloseGate {
            fn drop(&mut self) {
                let (released, changed) = &*self.0;
                *released.lock_recover() = true;
                changed.notify_all();
            }
        }
        struct Endpoint(Arc<(Mutex<bool>, Condvar)>);
        impl PerformanceMidiEndpoint for Endpoint {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                Ok(PerformanceMidiReceipt {
                    endpoint: "fixture".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }
            fn retire(&mut self) {
                let (released, changed) = &*self.0;
                let mut released = released.lock_recover();
                while !*released {
                    released = changed
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let release_gate = CloseGate(Arc::clone(&gate));
        let factory_gate = Arc::clone(&gate);
        let midi = PerformanceMidiWorkers::with_factory(move || {
            Box::new(Endpoint(Arc::clone(&factory_gate)))
        });
        let (_scratch, path) = broker_scratch_descriptor_path("midi-close-shutdown-claim");
        let controller = Arc::new(DeviceController::start_disconnected().expect("controller"));
        controller.claim_device("shutdown-midi-fixture");
        let claim = qc_owner_only_file::ClaimedFile::try_claim(&path).expect("descriptor claim");
        let mut shared = broker_shared_for_test();
        shared.controller = Arc::clone(&controller);
        shared.descriptor_path = path.to_path_buf();
        shared.descriptor_claim = Arc::new(Mutex::new(Some(claim)));
        shared.performance_midi = Arc::new(midi);
        shared
            .performance_midi
            .send("shutdown-midi-fixture", 1, 2, || {})
            .expect("fixture MIDI send");
        let marker = BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id);

        let report = shared.finish_shutdown();
        assert!(!report.all_verified(), "unacknowledged close is not clean");
        assert!(shared.descriptor_claim.lock_recover().is_some());
        assert!(
            qc_owner_only_file::ClaimedFile::try_claim(&path).is_err(),
            "a successor must remain excluded while MIDI still owns the unit"
        );
        assert!(
            !marker.exists(),
            "an unacknowledged MIDI close must not publish a release marker"
        );

        drop(release_gate);
        let report = shared.finish_shutdown();
        assert!(report.all_verified(), "{}", report.summary());
        assert!(shared.descriptor_claim.lock_recover().is_none());
        qc_owner_only_file::ClaimedFile::try_claim(&path)
            .expect("descriptor becomes available after the MIDI close acknowledgement")
            .release()
            .expect("release fixture claim");
    }

    /// A worker that never returns must not hold `request_lock` indefinitely;
    /// after the budget, the caller gets an unknown outcome and quarantines
    /// subsequent writes before releasing the lock.
    #[test]
    fn midi_registry_drop_keeps_global_admission_until_endpoint_close_retires() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct ClosingEndpoint {
            gate: Arc<(Mutex<bool>, Condvar)>,
            entered: Arc<AtomicBool>,
        }
        impl PerformanceMidiEndpoint for ClosingEndpoint {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                panic!("this fixture must not send native MIDI");
            }
        }
        impl Drop for ClosingEndpoint {
            fn drop(&mut self) {
                self.entered.store(true, Ordering::Release);
                let (lock, changed) = &*self.gate;
                let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                drop(
                    changed
                        .wait_timeout_while(guard, Duration::from_secs(30), |released| !*released),
                );
            }
        }
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release = ReleaseGate(Arc::clone(&gate));
        let entered = Arc::new(AtomicBool::new(false));
        let (g, e) = (Arc::clone(&gate), Arc::clone(&entered));
        let mut pool = PerformanceMidiWorkers::with_factory(move || {
            Box::new(ClosingEndpoint {
                gate: Arc::clone(&g),
                entered: Arc::clone(&e),
            })
        });
        pool.worker_budget = &BUDGET;
        let (sender, _) = pool.get_or_start("fixture").unwrap();
        drop(sender);
        drop(pool);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !entered.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            entered.load(Ordering::Acquire),
            "actual endpoint destructor must enter"
        );
        assert_eq!(BUDGET.count(), 1);
        assert!(
            BUDGET.reserve().is_err(),
            "replacement instance must not bypass original cap"
        );
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let deadline = Instant::now() + Duration::from_secs(2);
        while BUDGET.count() != 0 && Instant::now() < deadline {
            qc_native_platform::thread_owner::reap();
            thread::yield_now();
        }
        assert_eq!(BUDGET.count(), 0, "actual original must retire and join");
        assert!(BUDGET.reserve().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn midi_registry_drop_keeps_admission_through_native_tls_teardown() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct TlsGate(Arc<(Mutex<bool>, Condvar)>, Arc<AtomicBool>);
        impl Drop for TlsGate {
            fn drop(&mut self) {
                self.1.store(true, Ordering::Release);
                let guard = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
                drop(
                    self.0
                         .1
                        .wait_timeout_while(guard, Duration::from_secs(30), |released| !*released),
                );
            }
        }
        thread_local! { static GATE: std::cell::RefCell<Option<TlsGate>> = const { std::cell::RefCell::new(None) }; }
        struct NoMidi;
        impl PerformanceMidiEndpoint for NoMidi {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                panic!("this fixture must not send native MIDI");
            }
        }
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release = ReleaseGate(Arc::clone(&gate));
        let entered = Arc::new(AtomicBool::new(false));
        let (g, e) = (Arc::clone(&gate), Arc::clone(&entered));
        let mut pool = PerformanceMidiWorkers::with_factory(move || {
            GATE.with(|slot| *slot.borrow_mut() = Some(TlsGate(Arc::clone(&g), Arc::clone(&e))));
            Box::new(NoMidi)
        });
        pool.worker_budget = &BUDGET;
        let (sender, _) = pool.get_or_start("fixture").unwrap();
        drop(sender);
        drop(pool);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !entered.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            entered.load(Ordering::Acquire),
            "native TLS destructor must enter"
        );
        qc_native_platform::thread_owner::reap();
        assert_eq!(BUDGET.count(), 1, "body completion is not native exit");
        assert!(BUDGET.reserve().is_err());
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let deadline = Instant::now() + Duration::from_secs(2);
        while BUDGET.count() != 0 && Instant::now() < deadline {
            qc_native_platform::thread_owner::reap();
            thread::yield_now();
        }
        assert_eq!(
            BUDGET.count(),
            0,
            "signal plus join releases original admission"
        );
    }

    #[test]
    fn a_stuck_performance_midi_lock_gives_up_after_its_budget_instead_of_hanging_forever() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let started = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_gate = Arc::clone(&gate);
        let factory_started = Arc::clone(&started);
        let factory_calls = Arc::clone(&calls);
        let mut workers = PerformanceMidiWorkers::with_factory(move || {
            Box::new(GatedMidiEndpoint {
                gate: Arc::clone(&factory_gate),
                started: Arc::clone(&factory_started),
                calls: Arc::clone(&factory_calls),
            })
        });
        workers.budget = Duration::from_millis(40);
        let quarantined = Arc::new(AtomicBool::new(false));
        let callback_quarantined = Arc::clone(&quarantined);
        let workers = Arc::new(workers);
        let caller = Arc::clone(&workers);
        let request = test_support::FixtureJob::spawn_with_gate(Arc::clone(&gate), move || {
            caller.send("test-device", 1, 2, move || {
                callback_quarantined.store(true, Ordering::Release);
            })
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            started.load(Ordering::Acquire),
            "fake native endpoint started"
        );
        let result = request.join().unwrap();
        assert!(result.err().unwrap().contains("outcome is unknown"));
        assert!(
            quarantined.load(Ordering::Acquire),
            "quarantine callback precedes return"
        );
        assert!(workers.workers.lock().unwrap()["test-device"]
            .poisoned
            .load(Ordering::Acquire));
        let poisoned = workers.send("test-device", 1, 2, || {});
        assert!(poisoned.err().unwrap().contains("poisoned"));
        let (released, changed) = &*gate;
        *released.lock().unwrap() = true;
        changed.notify_all();
    }

    #[test]
    fn a_queued_midi_timeout_is_atomically_cancelled_before_dispatch() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let started = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_gate = Arc::clone(&gate);
        let factory_started = Arc::clone(&started);
        let factory_calls = Arc::clone(&calls);
        let mut pool = PerformanceMidiWorkers::with_factory(move || {
            Box::new(GatedMidiEndpoint {
                gate: Arc::clone(&factory_gate),
                started: Arc::clone(&factory_started),
                calls: Arc::clone(&factory_calls),
            })
        });
        pool.budget = Duration::from_secs(1);
        let pool = Arc::new(pool);
        let first_pool = Arc::clone(&pool);
        let first = test_support::FixtureJob::spawn_with_gate(Arc::clone(&gate), move || {
            first_pool.send("queued-device", 1, 2, || {})
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            started.load(Ordering::Acquire),
            "first command entered fake endpoint"
        );
        let queued = pool.send_with_budget("queued-device", 3, 4, Duration::from_millis(40), || {});
        assert!(queued.err().unwrap().contains("before dispatch"));
        let (released, changed) = &*gate;
        *released.lock().unwrap() = true;
        changed.notify_all();
        assert!(first.join().unwrap().is_ok());
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "cancelled queue item never reaches endpoint"
        );
    }

    #[test]
    fn queued_midi_revalidates_session_fence_before_native_send() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let started = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_gate = Arc::clone(&gate);
        let factory_started = Arc::clone(&started);
        let factory_calls = Arc::clone(&calls);
        let workers = Arc::new(PerformanceMidiWorkers::with_factory(move || {
            Box::new(GatedMidiEndpoint {
                gate: Arc::clone(&factory_gate),
                started: Arc::clone(&factory_started),
                calls: Arc::clone(&factory_calls),
            })
        }));
        let first_workers = Arc::clone(&workers);
        let first = test_support::FixtureJob::spawn_with_gate(Arc::clone(&gate), move || {
            first_workers.send("fenced-device", 1, 2, || {})
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(started.load(Ordering::Acquire), "first fake send started");

        let generation = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let expected_generation = Arc::clone(&generation);
        let second_workers = Arc::clone(&workers);
        let (result_tx, result_rx) = mpsc::channel();
        let (queued_tx, queued_rx) = mpsc::channel();
        let second = test_support::FixtureJob::spawn_plain(move || {
            let validator_generation = Arc::clone(&expected_generation);
            let result = second_workers.send_with_budget_and_validator_notified(
                "fenced-device",
                3,
                4,
                Duration::from_secs(1),
                move || {
                    if validator_generation.load(Ordering::Acquire) == 1 {
                        Ok(())
                    } else {
                        Err("session fence is stale".into())
                    }
                },
                move || {
                    let _ = queued_tx.send(());
                },
                || {},
            );
            let _ = result_tx.send(result);
        });

        queued_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second command is admitted behind first native call");
        generation.store(2, Ordering::Release);
        let (released, changed) = &*gate;
        *released.lock().unwrap() = true;
        changed.notify_all();

        assert!(first.join().unwrap().is_ok());
        let stale = match result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("queued command returns after first owner releases")
        {
            Ok(_) => panic!("stale fence must suppress native MIDI"),
            Err(error) => error,
        };
        assert!(stale.contains("session fence is stale"));
        second.join().unwrap();
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "stale command never reached endpoint"
        );
    }

    #[test]
    fn midi_timeout_during_fence_validation_cancels_before_native_dispatch() {
        let validator_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = ReleaseGate(Arc::clone(&validator_gate));
        let validator_entered = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let endpoint_gate = Arc::new((Mutex::new(true), Condvar::new()));
        let factory_gate = Arc::clone(&endpoint_gate);
        let factory_calls = Arc::clone(&calls);
        let workers = Arc::new(PerformanceMidiWorkers::with_factory(move || {
            Box::new(GatedMidiEndpoint {
                gate: Arc::clone(&factory_gate),
                started: Arc::new(AtomicBool::new(false)),
                calls: Arc::clone(&factory_calls),
            })
        }));
        let run_workers = Arc::clone(&workers);
        let run_gate = Arc::clone(&validator_gate);
        let run_entered = Arc::clone(&validator_entered);
        let quarantine = Arc::new(AtomicBool::new(false));
        let run_quarantine = Arc::clone(&quarantine);
        let timed_out =
            test_support::FixtureJob::spawn_with_gate(Arc::clone(&validator_gate), move || {
                run_workers.send_with_budget_and_validator(
                    "validation-timeout-device",
                    1,
                    2,
                    Duration::from_millis(500),
                    move || {
                        run_entered.store(true, Ordering::Release);
                        let (released, changed) = &*run_gate;
                        let mut released = released.lock().unwrap();
                        while !*released {
                            released = changed.wait(released).unwrap();
                        }
                        Ok(())
                    },
                    move || run_quarantine.store(true, Ordering::Release),
                )
            });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !validator_entered.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            validator_entered.load(Ordering::Acquire),
            "worker entered fence validation"
        );
        let result = timed_out.join().unwrap();
        assert!(result.err().unwrap().contains("before dispatch"));
        assert!(
            !quarantine.load(Ordering::Acquire),
            "no native call means no quarantine"
        );

        let (released, changed) = &*validator_gate;
        *released.lock().unwrap() = true;
        changed.notify_all();
        assert!(workers
            .send("validation-timeout-device", 3, 4, || {})
            .is_ok());
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "cancelled command never reached endpoint"
        );
    }

    #[test]
    fn midi_workers_keep_one_endpoint_owner_per_device() {
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let started = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factories = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_gate = Arc::clone(&gate);
        let factory_started = Arc::clone(&started);
        let factory_calls = Arc::clone(&calls);
        let factory_count = Arc::clone(&factories);
        let workers = PerformanceMidiWorkers::with_factory(move || {
            factory_count.fetch_add(1, Ordering::AcqRel);
            Box::new(GatedMidiEndpoint {
                gate: Arc::clone(&factory_gate),
                started: Arc::clone(&factory_started),
                calls: Arc::clone(&factory_calls),
            })
        });
        assert!(workers.send("device-a", 1, 2, || {}).is_ok());
        assert!(workers.send("device-a", 3, 4, || {}).is_ok());
        assert!(workers.send("device-b", 5, 6, || {}).is_ok());
        assert_eq!(factories.load(Ordering::Acquire), 2);
        assert_eq!(calls.load(Ordering::Acquire), 3);
        assert_eq!(workers.workers.lock().unwrap().len(), 2);
    }

    /// `request_lock` remains held while the ordered MIDI intent waits, then
    /// releases when a stuck native call exceeds its bounded response budget.
    #[test]
    fn a_stuck_midi_wait_releases_request_lock_for_other_clients_once_its_budget_expires() {
        let shared = Arc::new(broker_shared_for_test());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let started = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory_gate = Arc::clone(&gate);
        let factory_started = Arc::clone(&started);
        let factory_calls = Arc::clone(&calls);
        let mut pool = PerformanceMidiWorkers::with_factory(move || {
            Box::new(GatedMidiEndpoint {
                gate: Arc::clone(&factory_gate),
                started: Arc::clone(&factory_started),
                calls: Arc::clone(&factory_calls),
            })
        });
        pool.budget = Duration::from_millis(40);
        let workers = Arc::new(pool);
        let request_lock = Arc::clone(&shared.request_lock);
        let midi_workers = Arc::clone(&workers);
        let midi_client = test_support::FixtureJob::spawn_with_gate(Arc::clone(&gate), move || {
            let _exclusive = request_lock.lock().unwrap();
            midi_workers.send("stuck-device", 1, 2, || {}).map(|_| ())
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !started.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(
            started.load(Ordering::Acquire),
            "fake endpoint entered native work"
        );
        assert!(midi_client.join().unwrap().is_err());
        assert!(
            shared.request_lock.try_lock().is_ok(),
            "timeout releases request_lock"
        );
        let (released, changed) = &*gate;
        *released.lock().unwrap() = true;
        changed.notify_all();
    }

    #[test]
    fn only_cached_reads_skip_the_request_lock() {
        for method in [
            "system.status",
            "device.stateEvents",
            "device.tempoClock",
            "device.masterVolume",
        ] {
            assert!(runs_without_request_lock(method), "{method}");
        }
        for method in [
            "device.snapshot",
            "device.setParameter",
            "device.reconnect",
            "device.disconnect",
            "device.resetSession",
        ] {
            assert!(
                generated_gateway::broker_dispatch(method).is_some(),
                "{method} is a real gateway method"
            );
            assert!(!runs_without_request_lock(method), "{method}");
        }
    }
}

/// Holds an idle reopen's in-flight count and records its outcome when the
/// reopen thread ends, however it ends: a panic, or a thread that never
/// started, is recorded as a failure, so neither waiters nor the shutdown
/// paths that wait for `reopens_in_flight` can be left waiting forever.
struct ReopenGuard {
    clients: Arc<Mutex<ClientAccounting>>,
    controller: Arc<DeviceController>,
    attempt: u64,
    outcome: Option<Result<(), String>>,
}

impl Drop for ReopenGuard {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err("the reopen stopped unexpectedly".into()));
        let mut clients = self.clients.lock_recover();
        clients.reopens_in_flight = clients.reopens_in_flight.saturating_sub(1);
        let attempt = self.attempt;
        let Some(entry) = clients.idle_entry_mut(&self.controller) else {
            return;
        };
        if !(entry.reopening && entry.attempt == attempt) {
            return;
        }
        match outcome {
            Ok(()) => clients.forget_idle_release(&self.controller),
            Err(reason) => {
                entry.reopening = false;
                entry.last_failure = Some((attempt, reason));
            }
        }
    }
}

/// Ask the worker to reconnect: returns once it acknowledges.
#[cfg(not(test))]
fn idle_reconnect(controller: &DeviceController, close_unacknowledged: bool) -> Result<(), String> {
    let reconnect = if close_unacknowledged {
        controller.reconnect_after_pending_close()
    } else {
        controller.reconnect()
    };
    reconnect
        .map(|_| ())
        .map_err(|error| format!("could not reopen the Quad Cortex: {error}"))
}

/// Wait for the session the reconnect started.
#[cfg(not(test))]
fn idle_wait(controller: &DeviceController) -> Result<(), String> {
    let status = controller.wait_for_ready(IDLE_REOPEN_WAIT);
    if status.phase == "ready" {
        Ok(())
    } else {
        Err(format!(
            "its session, closed for idleness, did not reopen: {}",
            status.detail
        ))
    }
}

#[cfg(test)]
fn idle_reconnect(controller: &DeviceController, close_unacknowledged: bool) -> Result<(), String> {
    if close_unacknowledged {
        if let Some(hook) = test_hook(
            &PENDING_CLOSE_RECONNECT_HOOKS,
            controller as *const DeviceController as usize,
        ) {
            return hook();
        }
    }
    match test_hook(
        &IDLE_RECONNECT_HOOKS,
        controller as *const DeviceController as usize,
    ) {
        Some(hook) => hook(),
        None => Ok(()),
    }
}

#[cfg(test)]
fn idle_wait(controller: &DeviceController) -> Result<(), String> {
    match test_hook(
        &IDLE_REOPEN_HOOKS,
        controller as *const DeviceController as usize,
    ) {
        Some(hook) => hook(),
        None => Err("no idle-reopen hook registered for this test controller".into()),
    }
}

/// The icon's Connect: a real reconnect in production. Tests never ask the
/// worker to search for hardware; they only record that Connect was chosen.
#[cfg(not(test))]
fn icon_reconnect(controller: &DeviceController, close_unacknowledged: bool) -> Result<(), String> {
    if close_unacknowledged {
        controller.reconnect_after_pending_close().map(|_| ())
    } else {
        controller.reconnect().map(|_| ())
    }
}

#[cfg(test)]
fn icon_reconnect(controller: &DeviceController, close_unacknowledged: bool) -> Result<(), String> {
    if close_unacknowledged {
        if let Some(hook) = test_hook(
            &PENDING_CLOSE_RECONNECT_HOOKS,
            controller as *const DeviceController as usize,
        ) {
            return hook();
        }
    }
    Ok(())
}
#[cfg(test)]
#[path = "rpc_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn dispatch_gesture_regression_request(
    controller: &DeviceController,
    method: &str,
    params: Value,
) -> Value {
    handle(
        controller,
        &PerformanceMidiWorkers::default(),
        Request {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: method.into(),
            params,
            origin: None,
            physical_only: false,
            expected_broker_target: None,
        },
    )
}
