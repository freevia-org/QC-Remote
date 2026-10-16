//! Policy callbacks run only on the owned background manager. Queue producers
//! never hold clients while scheduling; operations take request_lock BEFORE
//! the short ticket/clients admission section, then release ticket/clients
//! before any native wait. Superseded work cannot pass that admission fence.
use super::background::{Ticket, Work};
use super::*;

const IDLE: background::Key = [1, 0, 0];
pub(super) const OBSERVER: background::Key = [5, 0, 0];
fn identity(controller: &Arc<DeviceController>) -> u64 {
    Arc::as_ptr(controller) as usize as u64
}
pub(super) fn selected(
    shared: &BrokerShared,
    requester: u64,
    controller: Arc<DeviceController>,
) -> Result<(), String> {
    let clients = Arc::clone(&shared.clients);
    let default = Arc::clone(&shared.controller);
    let request_lock = Arc::clone(&shared.request_lock);
    #[cfg(test)]
    let before = selected_point(shared, "selected-reconnect:before-request-lock");
    #[cfg(test)]
    let after = selected_point(shared, "selected-reconnect:after-admission");
    shared.background.requested(
        [3, identity(&controller), 0],
        requester,
        Work::new(move |ticket| {
            #[cfg(test)]
            if let Some(hook) = before {
                let _ = hook();
            }
            let _exclusive = request_lock.lock_recover();
            let admitted = ticket
                .admit_requesters(|requesters| {
                    let clients = clients.lock_recover();
                    !clients.shutting_down
                        && !clients.device_is_closing(&controller)
                        && requesters.iter().any(|id| {
                            clients.connections.get(id).is_some_and(|row| {
                                row.is_participant()
                                    && Arc::ptr_eq(
                                        row.selected_device.as_ref().unwrap_or(&default),
                                        &controller,
                                    )
                            })
                        })
                })
                .unwrap_or(false);
            #[cfg(test)]
            if let Some(hook) = after {
                let _ = hook();
            }
            if admitted {
                // Same generated reconnect/ack behavior; test hooks avoid hardware.
                if let Err(error) = idle_reconnect(&controller) {
                    controller.record_flight_event(format!("selected reconnect refused: {error}"));
                }
            }
        }),
    )
}
#[cfg(test)]
fn selected_point(shared: &BrokerShared, point: &'static str) -> Option<TestHook> {
    TEST_POINTS
        .lock_recover()
        .iter()
        .find(|(id, name, _)| id == &shared.instance_id && *name == point)
        .map(|(_, _, hook)| Arc::clone(hook))
}
pub(super) fn unused(shared: &BrokerShared, device: Arc<DeviceController>) -> Result<(), String> {
    let work = UnusedDevice {
        clients: Arc::clone(&shared.clients),
        request_lock: Arc::clone(&shared.request_lock),
        default: Arc::clone(&shared.controller),
        device: Arc::clone(&device),
        performance_midi: Arc::clone(&shared.performance_midi),
        #[cfg(all(test, target_os = "linux"))]
        retry_hook: selected_point(shared, "unused:retry-admitted"),
        #[cfg(all(test, target_os = "linux"))]
        disconnect_hook: selected_point(shared, "unused:disconnect"),
    };
    shared.background.latest(
        [2, identity(&device), 0],
        Instant::now() + IDLE_SHUTDOWN_GRACE,
        work.work(2),
    )
}

#[derive(Clone)]
struct UnusedDevice {
    clients: Arc<Mutex<ClientAccounting>>,
    request_lock: Arc<Mutex<()>>,
    default: Arc<DeviceController>,
    device: Arc<DeviceController>,
    performance_midi: Arc<PerformanceMidiWorkers>,
    #[cfg(all(test, target_os = "linux"))]
    retry_hook: Option<TestHook>,
    #[cfg(all(test, target_os = "linux"))]
    disconnect_hook: Option<TestHook>,
}
impl UnusedDevice {
    fn work(self, retries: usize) -> Work {
        #[cfg(not(target_os = "linux"))]
        let _ = retries;
        Work::new(move |ticket| {
            let exclusive = self.request_lock.lock_recover();
            let admission = ticket
                .with_current(|| {
                    BrokerShared::reserve_unused_device(&self.clients, &self.default, &self.device)
                })
                .flatten();
            if let Some(admission) = admission {
                if let Err(error) = self.performance_midi.retire_controller(&self.device) {
                    eprintln!("Unused device remains open: {error}");
                    drop(admission);
                    drop(exclusive);
                    #[cfg(target_os = "linux")]
                    if retries > 0 {
                        match ticket.retry_after(
                            Instant::now() + IDLE_SHUTDOWN_GRACE,
                            self.clone().work(retries - 1),
                        ) {
                            Ok(true) =>
                            {
                                #[cfg(test)]
                                if let Some(hook) = &self.retry_hook {
                                    let _ = hook();
                                }
                            }
                            Ok(false) => {} // A newer generation owns the policy.
                            Err(error) => {
                                eprintln!("Unused device retry was not admitted: {error}")
                            }
                        }
                    }
                    return;
                }
                #[cfg(all(test, target_os = "linux"))]
                if let Some(hook) = &self.disconnect_hook {
                    let _ = hook();
                }
                // Reservation follows the actual queued worker even on caller timeout.
                let _ = self
                    .device
                    .disconnect_with_admission(Some(Box::new(admission)));
            }
        })
    }
}
fn idle_work(shared: &Arc<BrokerShared>) -> Work {
    let owner = Arc::downgrade(shared);
    Work::new(move |ticket| {
        let Some(shared) = owner.upgrade() else {
            return;
        };
        enum Decision {
            None,
            Retry,
            Resident,
            Shutdown,
        }
        let decision = ticket
            .with_current(|| {
                let mut clients = shared.clients.lock_recover();
                if clients.shutting_down || clients.tray_holds_past_grace() {
                    return Decision::None;
                }
                if clients.count != 0 {
                    clients.reap_deferred = true;
                    return Decision::None;
                }
                if clients.reopens_in_flight != 0 {
                    return Decision::Retry;
                }
                if clients.resident {
                    return Decision::Resident;
                }
                clients.shutting_down = true;
                Decision::Shutdown
            })
            .unwrap_or(Decision::None);
        match decision {
            Decision::None => {}
            Decision::Retry => {
                // Atomic generation CAS: an older callback cannot overwrite a
                // newer arm's deadline while rearming for a running handshake.
                if let Err(error) = shared.background.reschedule(
                    &ticket,
                    Instant::now() + shared.idle_grace(),
                    idle_work(&shared),
                ) {
                    shared
                        .controller
                        .record_flight_event(format!("idle grace rearm refused: {error}"));
                }
            }
            Decision::Resident => release_idle(&shared, Some(&ticket)),
            Decision::Shutdown => {
                shared.finish_shutdown();
            }
        }
    })
}
pub(super) fn idle(shared: &Arc<BrokerShared>) -> Result<(), String> {
    shared.background.latest(
        IDLE,
        Instant::now() + shared.idle_grace(),
        idle_work(shared),
    )
}
pub(super) fn release_idle(shared: &Arc<BrokerShared>, ticket: Option<&Ticket>) {
    let exclusive = shared.request_lock.lock_recover();
    let all = shared.all_controllers();
    let admit = || {
        let mut clients = shared.clients.lock_recover();
        let release = !clients.shutting_down
            && clients.resident
            && !clients.idle_released
            && clients.reopens_in_flight == 0
            && clients.participants() == 0
            && clients.pending_authentications() == 0;
        if release {
            clients.observer_idle_since = None;
            clients.tray_idle_since = None;
            clients.record_idle_release(all);
        }
        release
    };
    let release = match ticket {
        Some(ticket) => ticket.with_current(admit).unwrap_or(false),
        None => admit(),
    };
    if release {
        shared.test_point("release_idle_device:recorded");
        shared
            .controller
            .record_flight_event("resident broker closing the idle logical device session");
        let _ = shared.disconnect_devices();
        drop(exclusive);
        return;
    }
    drop(exclusive);
    shared.update_observer_backstop();
}
pub(super) fn observer(shared: &Arc<BrokerShared>, marker: Arc<()>) -> Result<(), String> {
    let owner = Arc::downgrade(shared);
    let clients = Arc::clone(&shared.clients);
    let held_marker = Arc::clone(&marker);
    shared.background.schedule(
        OBSERVER,
        Instant::now(),
        Work::new(move |ticket| {
            if let Some(shared) = owner.upgrade() {
                shared.watch_observer_backstop(ticket, marker);
            }
        })
        .on_retired(move |_| {
            let mut clients = clients.lock_recover();
            if clients
                .observer_owner
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &held_marker))
            {
                clients.observer_timer_running = false;
                clients.observer_owner = None;
            }
        }),
    )
}
pub(super) fn reopen(
    shared: &BrokerShared,
    controller: Arc<DeviceController>,
    attempt: u64,
) -> Result<(), String> {
    let mut guard = ReopenGuard {
        clients: Arc::clone(&shared.clients),
        controller: Arc::clone(&controller),
        attempt,
        outcome: None,
    };
    let clients = Arc::clone(&shared.clients);
    let request_lock = Arc::clone(&shared.request_lock);
    let outcome = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&outcome);
    shared.background.schedule(
        [4, identity(&controller), attempt],
        Instant::now(),
        Work::new(move |ticket| {
            let result = (|| -> Result<(), String> {
                {
                    let _exclusive = request_lock.lock_recover();
                    let admitted = ticket
                        .with_current(|| {
                            let clients = clients.lock_recover();
                            if clients.shutting_down || clients.device_is_closing(&controller) {
                                return Err(
                                    "broker/device shutdown is already committed".to_string()
                                );
                            }
                            Ok(clients
                                .idle_entry(&controller)
                                .is_some_and(|entry| entry.reopening && entry.attempt == attempt))
                        })
                        .ok_or("idle reopen cancelled before native dispatch")??;
                    if !admitted {
                        return Err("idle reopen superseded before native dispatch".into());
                    }
                    controller
                        .record_flight_event("resident broker reopening an idle-released device");
                    idle_reconnect(&controller)?;
                }
                if !ticket.current() {
                    return Err("idle reopen cancelled during ready wait".into());
                }
                idle_wait(&controller)
            })();
            *recorded.lock_recover() = Some(result);
        })
        .on_retired(move |joined| {
            guard.outcome = Some(if joined {
                outcome
                    .lock_recover()
                    .take()
                    .unwrap_or_else(|| Err("idle reopen cancelled before dispatch".into()))
            } else {
                Err("idle reopen original worker failed or never started".into())
            });
            // Guard releases accounting AFTER actual original join, not body return.
            drop(guard);
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    static FIXTURES: Mutex<()> = Mutex::new(());
    struct Fixture(BrokerHandle);
    impl Fixture {
        fn new() -> Self {
            Self(test_support::handle())
        }
        fn shared(&self) -> &Arc<BrokerShared> {
            &self.0 .0
        }
        fn finish(&self) {
            test_support::finish(&self.0);
            self.shared()
                .background
                .finish_before(Instant::now() + Duration::from_secs(2))
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            test_support::finish(&self.0);
            // On failed assertions the manager retains any exact unknown
            // originals. This fixture owns no bytes that could be deleted
            // underneath them; controller/job resources remain with owners.
            let _ = self
                .shared()
                .background
                .finish_before(Instant::now() + Duration::from_secs(2));
        }
    }
    struct Hook {
        registry: &'static Mutex<Vec<(usize, TestHook)>>,
        key: usize,
        callback: TestHook,
        _controller: Arc<DeviceController>,
    }
    impl Hook {
        fn new(
            registry: &'static Mutex<Vec<(usize, TestHook)>>,
            controller: Arc<DeviceController>,
            callback: TestHook,
        ) -> Self {
            let key = identity(&controller) as usize;
            registry.lock_recover().push((key, Arc::clone(&callback)));
            Self {
                registry,
                key,
                callback,
                _controller: controller,
            }
        }
    }
    impl Drop for Hook {
        fn drop(&mut self) {
            self.registry.lock_recover().retain(|(key, callback)| {
                *key != self.key || !Arc::ptr_eq(callback, &self.callback)
            });
        }
    }
    struct Point {
        id: String,
        name: &'static str,
        callback: TestHook,
    }
    impl Point {
        fn new(shared: &BrokerShared, name: &'static str, callback: TestHook) -> Self {
            TEST_POINTS.lock_recover().push((
                shared.instance_id.clone(),
                name,
                Arc::clone(&callback),
            ));
            Self {
                id: shared.instance_id.clone(),
                name,
                callback,
            }
        }
    }
    impl Drop for Point {
        fn drop(&mut self) {
            TEST_POINTS.lock_recover().retain(|(id, name, hook)| {
                id != &self.id || *name != self.name || !Arc::ptr_eq(hook, &self.callback)
            });
        }
    }
    #[derive(Clone)]
    struct Gate(Arc<(Mutex<bool>, Condvar)>);
    impl Gate {
        fn new() -> Self {
            Self(Arc::new((Mutex::new(false), Condvar::new())))
        }
        fn release(&self) {
            *self.0 .0.lock_recover() = true;
            self.0 .1.notify_all();
        }
        fn wait(&self) {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut released = self.0 .0.lock_recover();
            while !*released {
                assert!(Instant::now() < deadline);
                released = self
                    .0
                     .1
                    .wait_timeout(released, Duration::from_millis(10))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
        }
    }
    struct ReleaseGate(Gate);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    fn wait_selected_retired(shared: &BrokerShared, controller: &Arc<DeviceController>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.background.pending_key([3, identity(controller), 0]) {
            assert!(
                Instant::now() < deadline,
                "actual background original did not join/retire"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn unused_release_retries_delayed_midi_close_but_preserves_reselected_client() {
        let _serial = FIXTURES.lock_recover();
        struct Endpoint(Gate);
        impl PerformanceMidiEndpoint for Endpoint {
            fn send(&mut self, _: &str, _: u8, _: u8) -> Result<PerformanceMidiReceipt, String> {
                Ok(PerformanceMidiReceipt {
                    endpoint: "fixture".into(),
                    dispatch_latency_ms: 0.0,
                    throttle_delay_ms: 0.0,
                })
            }
            fn retire(&mut self) {
                self.0.wait();
            }
        }
        for reselect in [false, true] {
            let midi_gate = Gate::new();
            let release_midi = ReleaseGate(midi_gate.clone());
            let mut handle = test_support::handle();
            Arc::get_mut(&mut handle.0).unwrap().performance_midi =
                Arc::new(PerformanceMidiWorkers::with_factory(move || {
                    Box::new(Endpoint(midi_gate.clone()))
                }));
            let fixture = Fixture(handle);
            let shared = fixture.shared();
            let participant = test_support::attach(&fixture.0, false);
            shared
                .select_available_device(participant, "unused-fixture-old")
                .unwrap();
            let old = shared.controller_for(Some(participant));
            shared
                .performance_midi
                .send("unused-fixture-old", 1, 2, || {})
                .unwrap();
            shared
                .select_available_device(participant, "unused-fixture-new")
                .unwrap();
            let new = shared.controller_for(Some(participant));
            assert!(!Arc::ptr_eq(&old, &new));
            let retry_gate = Gate::new();
            let release_retry = ReleaseGate(retry_gate.clone());
            let (admitted, retried) = mpsc::channel();
            let disconnects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = Arc::clone(&disconnects);
            let work = UnusedDevice {
                clients: Arc::clone(&shared.clients),
                request_lock: Arc::clone(&shared.request_lock),
                default: Arc::clone(&shared.controller),
                device: Arc::clone(&old),
                performance_midi: Arc::clone(&shared.performance_midi),
                retry_hook: Some(Arc::new(move || {
                    let _ = admitted.send(());
                    retry_gate.wait();
                    Ok(())
                })),
                disconnect_hook: Some(Arc::new(move || {
                    observed.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                })),
            };
            let key = [2, identity(&old), 0];
            shared
                .background
                .latest(key, Instant::now(), work.work(2))
                .unwrap();
            retried
                .recv_timeout(Duration::from_secs(10))
                .expect("MIDI close exceeded its bound and admitted an owned retry");
            assert_eq!(
                disconnects.load(Ordering::Acquire),
                0,
                "no HID release before MIDI close acknowledgement"
            );
            if reselect {
                shared
                    .select_available_device(participant, "unused-fixture-old")
                    .unwrap();
                assert!(Arc::ptr_eq(&shared.controller_for(Some(participant)), &old));
            }
            drop(release_midi);
            drop(release_retry);
            let deadline = Instant::now() + Duration::from_secs(12);
            while shared.background.pending_key(key) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(
                !shared.background.pending_key(key),
                "retry original must retire"
            );
            assert_eq!(disconnects.load(Ordering::Acquire), usize::from(!reselect));
            if !reselect {
                assert!(Arc::ptr_eq(&shared.controller_for(Some(participant)), &new));
            }
            fixture.finish();
        }
    }

    #[test]
    fn selected_reconnect_superseded_behind_request_lock_never_reaches_the_old_controller() {
        let _serial = FIXTURES.lock_recover();
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let participant = test_support::attach(&fixture.0, false);
        shared
            .select_available_device(participant, "fixture-original")
            .unwrap();
        let controller = Arc::clone(&shared.controller);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let _hook = Hook::new(
            &IDLE_RECONNECT_HOOKS,
            Arc::clone(&controller),
            Arc::new(move || {
                observed.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }),
        );
        let gate = Gate::new();
        let release = ReleaseGate(gate.clone());
        let (entered, admission) = mpsc::channel();
        let _point = Point::new(
            shared,
            "selected-reconnect:before-request-lock",
            Arc::new(move || {
                let _ = entered.send(());
                gate.wait();
                Ok(())
            }),
        );
        selected(shared, participant, Arc::clone(&controller)).unwrap();
        admission.recv_timeout(Duration::from_secs(2)).unwrap();
        shared
            .select_available_device(participant, "fixture-replacement")
            .unwrap();
        drop(release);
        wait_selected_retired(shared, &controller);
        assert_eq!(
            calls.load(Ordering::Acquire),
            0,
            "obsolete requested target must not reconnect"
        );
        fixture.finish();
    }
    #[test]
    fn coalesced_selected_reconnect_keeps_another_still_current_requester() {
        let _serial = FIXTURES.lock_recover();
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let first = test_support::attach(&fixture.0, false);
        shared
            .select_available_device(first, "fixture-original")
            .unwrap();
        let second = test_support::attach(&fixture.0, false);
        let controller = Arc::clone(&shared.controller);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let _hook = Hook::new(
            &IDLE_RECONNECT_HOOKS,
            Arc::clone(&controller),
            Arc::new(move || {
                observed.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }),
        );
        let gate = Gate::new();
        let release = ReleaseGate(gate.clone());
        let (entered, admission) = mpsc::channel();
        let _point = Point::new(
            shared,
            "selected-reconnect:before-request-lock",
            Arc::new(move || {
                let _ = entered.send(());
                gate.wait();
                Ok(())
            }),
        );
        selected(shared, first, Arc::clone(&controller)).unwrap();
        admission.recv_timeout(Duration::from_secs(2)).unwrap();
        selected(shared, second, Arc::clone(&controller)).unwrap();
        shared
            .select_available_device(first, "fixture-replacement")
            .unwrap();
        drop(release);
        wait_selected_retired(shared, &controller);
        assert_eq!(calls.load(Ordering::Acquire), 1, "one original reconnect serves the remaining exact requester, without duplicate mutation");
        fixture.finish();
    }
    #[test]
    fn a_current_requester_arriving_after_a_refused_predicate_gets_a_fresh_operation() {
        let _serial = FIXTURES.lock_recover();
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let first = test_support::attach(&fixture.0, false);
        shared
            .select_available_device(first, "fixture-original")
            .unwrap();
        let second = test_support::attach(&fixture.0, false);
        // Real selection has completed before scheduling its auto-connect;
        // that scheduling step runs outside select_available_device's lock.
        shared
            .select_available_device(second, "fixture-original")
            .unwrap();
        let controller = Arc::clone(&shared.controller);
        shared
            .select_available_device(first, "fixture-replacement")
            .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let _hook = Hook::new(
            &IDLE_RECONNECT_HOOKS,
            Arc::clone(&controller),
            Arc::new(move || {
                observed.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }),
        );
        let gate = Gate::new();
        let release = ReleaseGate(gate.clone());
        let (entered, admission) = mpsc::channel();
        let _point = Point::new(
            shared,
            "selected-reconnect:after-admission",
            Arc::new(move || {
                let _ = entered.send(());
                gate.wait();
                Ok(())
            }),
        );
        selected(shared, first, Arc::clone(&controller)).unwrap();
        admission.recv_timeout(Duration::from_secs(2)).unwrap();
        // The old false predicate already evaluated, but its original hasn't
        // exited. Merging here must not silently discard this current demand.
        selected(shared, second, Arc::clone(&controller)).unwrap();
        drop(release);
        wait_selected_retired(shared, &controller);
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "late current requester must reach the native reconnect hook exactly once"
        );
        fixture.finish();
    }
    #[cfg(windows)]
    #[test]
    fn reopen_result_and_accounting_guard_wait_for_actual_thread_exit_after_the_body() {
        let _serial = FIXTURES.lock_recover();
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let controller = Arc::clone(&shared.controller);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        struct Release(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0 .0.lock_recover() = true;
                self.0 .1.notify_all();
            }
        }
        let release = Release(Arc::clone(&gate));
        struct ExitFinalizer {
            entered: mpsc::Sender<()>,
            gate: Arc<(Mutex<bool>, Condvar)>,
        }
        impl Drop for ExitFinalizer {
            fn drop(&mut self) {
                let _ = self.entered.send(());
                let deadline = Instant::now() + Duration::from_secs(30);
                let mut released = self.gate.0.lock_recover();
                while !*released {
                    assert!(Instant::now() < deadline, "owned exit gate not released");
                    released = self
                        .gate
                        .1
                        .wait_timeout(released, Duration::from_millis(10))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
            }
        }
        thread_local! { static EXIT: std::cell::RefCell<Option<ExitFinalizer>> = const { std::cell::RefCell::new(None) }; }
        let (entered, body_returned) = mpsc::channel();
        let _hook = Hook::new(
            &IDLE_REOPEN_HOOKS,
            Arc::clone(&controller),
            Arc::new(move || {
                EXIT.with(|slot| {
                    *slot.borrow_mut() = Some(ExitFinalizer {
                        entered: entered.clone(),
                        gate: Arc::clone(&gate),
                    })
                });
                Ok(())
            }),
        );
        {
            let mut clients = shared.clients.lock_recover();
            clients.record_idle_release(vec![Arc::clone(&controller)]);
            let entry = clients.idle_entry_mut(&controller).unwrap();
            entry.attempt = 1;
            entry.reopening = true;
            clients.reopens_in_flight = 1;
        }
        reopen(shared, Arc::clone(&controller), 1).unwrap();
        body_returned.recv_timeout(Duration::from_secs(2)).unwrap();
        {
            let clients = shared.clients.lock_recover();
            assert_eq!(
                clients.reopens_in_flight, 1,
                "body returned but exact native original is still in TLS teardown"
            );
            assert!(clients.idle_entry(&controller).unwrap().reopening);
        }
        assert!(shared
            .background
            .finish_before(Instant::now() + Duration::from_millis(20))
            .unwrap_err()
            .contains("unacknowledged"));
        assert_eq!(shared.clients.lock_recover().reopens_in_flight, 1);
        drop(release);
        fixture.finish();
        let clients = shared.clients.lock_recover();
        assert_eq!(clients.reopens_in_flight, 0);
        assert!(
            clients.idle_entry(&controller).is_none(),
            "healthy already-admitted reopen outcome publishes only after native original join"
        );
    }
    #[test]
    fn a_never_observer_backstop_is_owned_and_retires_on_manager_stop() {
        let _serial = FIXTURES.lock_recover();
        let fixture = Fixture::new();
        let shared = fixture.shared();
        shared.set_tray_hold(Some(IdlePolicy::Never));
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.background.pending_keys() == 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(shared.clients.lock_recover().observer_timer_running);
        fixture.finish();
        let clients = shared.clients.lock_recover();
        assert!(!clients.observer_timer_running);
        assert!(clients.observer_owner.is_none());
        drop(clients);
        assert_eq!(shared.background.pending_keys(), 0);
    }
    #[test]
    fn ordinary_resident_idle_release_preserves_the_existing_recorded_release_policy() {
        let _serial = FIXTURES.lock_recover();
        let fixture = Fixture::new();
        let shared = fixture.shared();
        shared.clients.lock_recover().resident = true;
        shared.release_idle_device();
        let clients = shared.clients.lock_recover();
        assert!(clients.idle_released);
        assert!(clients.idle_entry(&shared.controller).is_some());
        drop(clients);
        fixture.finish();
    }
}
