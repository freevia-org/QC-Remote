#![cfg(test)]

//! Residence versus the device hold. The process that is also the session's
//! background MCP host outlives its last client only while it is resident; a
//! reap then closes the device sessions instead of exiting, and each device
//! comes back lazily with the first request routed to it. A broker that is
//! not resident (tray switched off, no desktop, `--stdio`) is unchanged: the
//! tests in `rpc_tray_hold_tests.rs` and the grace tests still pin it.
use super::*;
use std::sync::atomic::AtomicUsize;

struct ResidentThread<T: Send + 'static>(Option<thread::JoinHandle<T>>);
fn spawn_fixture<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> ResidentThread<T> {
    ResidentThread(Some(thread::spawn(work)))
}
impl<T: Send + 'static> ResidentThread<T> {
    fn is_finished(&self) -> bool {
        crate::platform::process::thread_signalled(self.0.as_ref().unwrap())
    }
    fn join(mut self) -> thread::Result<T> {
        let deadline = Instant::now() + IDLE_REOPEN_WAIT + Duration::from_secs(2);
        while !self.is_finished() {
            assert!(
                Instant::now() < deadline,
                "resident fixture caller did not retire"
            );
            thread::sleep(Duration::from_millis(1));
        }
        self.0.take().unwrap().join()
    }
}
impl<T: Send + 'static> Drop for ResidentThread<T> {
    fn drop(&mut self) {
        let Some(worker) = self.0.take() else { return };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !crate::platform::process::thread_signalled(&worker) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        if crate::platform::process::thread_signalled(&worker) {
            let _ = worker.join();
        } else {
            static RETAINED: Mutex<Vec<Box<dyn std::any::Any + Send>>> = Mutex::new(Vec::new());
            RETAINED.lock_recover().push(Box::new(worker));
            eprintln!("resident fixture caller did not acknowledge exit; original handle retained");
        }
    }
}

struct ResidentFixture {
    shared: Arc<BrokerShared>,
    root: Arc<crate::device_lease::testing::FixtureRoot>,
}
impl std::ops::Deref for ResidentFixture {
    type Target = Arc<BrokerShared>;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}
impl Drop for ResidentFixture {
    fn drop(&mut self) {
        self.shared.clients.lock_recover().shutting_down = true;
        self.shared.observer_changed.notify_all();
        // Remove only this fixture's registrations. Admitted hooks own their
        // cloned callback until completion; no other fixture is swept.
        TEST_POINTS
            .lock_recover()
            .retain(|(id, _, _)| id != &self.shared.instance_id);
        TEST_GRACE
            .lock_recover()
            .retain(|(id, _)| id != &self.shared.instance_id);
        TEST_MINUTE
            .lock_recover()
            .retain(|(id, _)| id != &self.shared.instance_id);
        // The existing unused-device grace actor owns constituent Arcs, not
        // BrokerShared. Wait for resource retirement as well as its observers;
        // these counts are not a claim that an OS thread has been joined.
        let resources_retired = || {
            Arc::strong_count(&self.shared) == 1
                && Arc::strong_count(&self.shared.clients) == 1
                && Arc::strong_count(&self.shared.request_lock) == 1
        };
        let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(2);
        while !resources_retired() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if !resources_retired() {
            static RETAINED: Mutex<
                Vec<(
                    Arc<BrokerShared>,
                    Arc<crate::device_lease::testing::FixtureRoot>,
                )>,
            > = Mutex::new(Vec::new());
            RETAINED
                .lock_recover()
                .push((Arc::clone(&self.shared), Arc::clone(&self.root)));
            eprintln!("resident fixture resources did not retire; exact broker and root retained");
        }
    }
}

fn resident_shared(minute: Duration) -> (ResidentFixture, mpsc::Receiver<()>) {
    resident_shared_with_controller(
        minute,
        Arc::new(DeviceController::start_disconnected().expect("controller")),
    )
}

fn resident_shared_with_controller(
    minute: Duration,
    controller: Arc<DeviceController>,
) -> (ResidentFixture, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let root = crate::device_lease::testing::fresh_root("resident-rpc");
    let shared = Arc::new(BrokerShared {
        controller,
        devices: Mutex::new(Vec::new()),
        performance_midi: Arc::new(PerformanceMidiWorkers::default()),
        request_lock: Arc::new(Mutex::new(())),
        clients: Arc::new(Mutex::new(ClientAccounting::default())),
        descriptor_path: root.0.join("device-broker.json"),
        instance_id: generate_token(16),
        descriptor_claim: Arc::new(Mutex::new(None)),
        on_idle_shutdown: Arc::new(move || {
            let _ = tx.send(());
        }),
        observer_changed: Condvar::new(),
        observer_minute: minute,
        background: background::Manager::default(),
        observer_actions: background::Manager::default(),
    });
    TEST_GRACE
        .lock_recover()
        .push((shared.instance_id.clone(), GRACE));
    shared.set_resident(true);
    (ResidentFixture { shared, root }, rx)
}

/// A backup export intentionally does not reopen an idle-released session.
/// It must reach its own handler once, rather than being redispatched forever
/// by the post-lock idle-release race check. Run the dispatch in a subprocess
/// so a regression that recurses to stack exhaustion cannot abort this suite.
#[test]
fn backup_target_after_idle_release_does_not_redispatch_recursively() {
    const CHILD: &str = "QC_TEST_IDLE_BACKUP_REDISPATCH_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
        let participant = attach_participant(&shared);
        shared.clients.lock_recover().idle_released_devices =
            vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
        for (id, method) in [
            (1, "broker.backupTarget"),
            (2, "broker.createDeviceBackupBound"),
        ] {
            let response =
                dispatch_broker_client_request(&shared, Some(participant), request(id, method));
            assert!(
                response.is_object(),
                "{method} handler must answer: {response}"
            );
        }
        shared.clients.lock_recover().shutting_down = true;
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .arg("backup_target_after_idle_release_does_not_redispatch_recursively")
        .env(CHILD, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("child test process starts");
    assert!(
        output.success(),
        "backupTarget after idle release must return without recursive dispatch; child exited {output}"
    );
}

/// If an idle HID disconnect command was accepted but its worker has not
/// acknowledged it within the deadline, the command may still close the
/// session later. After MIDI retirement, publish an idle marker so the next
/// participant request reopens instead of talking to that closing session.
#[test]
fn resident_idle_release_records_a_queued_hid_close_without_acknowledgment() {
    let release_worker = Arc::new(AtomicBool::new(false));
    struct ReleaseOnDrop(Arc<AtomicBool>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let controller = Arc::new(DeviceController::with_stuck_worker_for_test(Arc::clone(
        &release_worker,
    )));
    let (shared, _shutdown) =
        resident_shared_with_controller(Duration::from_secs(60), Arc::clone(&controller));
    let _release_on_drop = ReleaseOnDrop(Arc::clone(&release_worker));

    shared.release_idle_device();

    let clients = shared.clients.lock_recover();
    assert!(
        clients.idle_released,
        "an accepted close stays represented even without a timely ACK"
    );
    assert!(
        clients.idle_entry(&controller).is_some(),
        "the controller whose close entered the worker queue must be marked"
    );
    drop(clients);
    release_worker.store(true, Ordering::Release);
    shared.clients.lock_recover().shutting_down = true;
}

/// The disconnect report distinguishes command admission from worker
/// acknowledgment: after successful MIDI retirement, a close accepted into
/// the HID worker queue is listed as queued even when its ACK times out.
#[test]
fn disconnect_report_retains_queued_hid_close_when_acknowledgment_times_out() {
    let release_worker = Arc::new(AtomicBool::new(false));
    let controller = Arc::new(DeviceController::with_stuck_worker_for_test(Arc::clone(
        &release_worker,
    )));
    let (shared, _shutdown) =
        resident_shared_with_controller(Duration::from_secs(60), Arc::clone(&controller));
    struct ReleaseOnDrop(Arc<AtomicBool>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let _release_on_drop = ReleaseOnDrop(Arc::clone(&release_worker));

    let report = shared.disconnect_devices_report();

    assert!(
        report.midi_retired,
        "MIDI retired before HID close admission"
    );
    assert_eq!(report.queued.len(), 1, "the HID command was accepted");
    assert!(Arc::ptr_eq(&report.queued[0], &controller));
    assert_eq!(report.failures.len(), 1, "its ACK missed the deadline");
    assert!(
        report.failures[0].contains("did not acknowledge the disconnect request"),
        "reported independently from admission: {:?}",
        report.failures
    );
}

/// Explicit reconnect/reset can pass the preflight before an idle close
/// publishes its marker, then wait for the close's request lock. Once it
/// acquires the lock, the session command owns the new generation and must
/// withdraw the marker published while it waited.
#[test]
fn authenticated_reconnect_during_resident_idle_close_withdraws_the_late_idle_marker() {
    for observer in [false, true] {
        let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
        let (resume, reached) = pause_at(&shared, "disconnect_devices:done");
        let releasing = {
            let shared = Arc::clone(&shared);
            spawn_fixture(move || shared.release_idle_device())
        };
        reached.recv_timeout(Duration::from_secs(2)).unwrap();

        // The close has finished; model a session that is healthy by the time the
        // queued reconnect reaches its handler. This avoids depending on real USB.
        shared.controller.set_connection_for_test(true, "ready");
        let caller = attach_participant(&shared);
        if observer {
            shared
                .clients
                .lock_recover()
                .connections
                .get_mut(&caller)
                .unwrap()
                .role = ConnectionRole::Observer {
                idle_policy_minutes: Some(15),
            };
        }
        let _reconnect = ReconnectHook::install(&shared.controller, || Ok(()));
        let reconnecting = {
            let shared = Arc::clone(&shared);
            spawn_fixture(move || {
                dispatch_broker_client_request(
                    &shared,
                    Some(caller),
                    request(1, generated_gateway::rpc::RECONNECT),
                )
            })
        };
        thread::sleep(Duration::from_millis(100));
        assert!(
            !reconnecting.is_finished(),
            "authenticated reconnect queues behind the close (observer={observer})"
        );

        resume.send(()).unwrap();
        releasing.join().unwrap();
        let response = reconnecting.join().unwrap();
        assert!(
            response.get("error").is_none(),
            "reconnect succeeds (observer={observer}): {response}"
        );
        let clients = shared.clients.lock_recover();
        assert!(!clients.idle_released, "the reconnect owns the session");
        assert!(clients.idle_entry(&shared.controller).is_none());
        drop(clients);
        shared.clients.lock_recover().shutting_down = true;
    }
}

#[test]
fn an_explicit_fixture_clock_override_rebases_the_already_queued_startup_grace() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    // Remove resident_shared's fast fixture clock, matching actual serving:
    // its first grace is admitted at the production five-second duration.
    TEST_GRACE
        .lock_recover()
        .retain(|(id, _)| id != &shared.instance_id);
    assert_eq!(shared.idle_grace(), IDLE_SHUTDOWN_GRACE);
    shared.arm_idle_shutdown();
    assert!(shared.background.pending_key([1, 0, 0]));
    test_support::set_grace(&handle, Duration::from_millis(150));
    assert!(
        wait_for(
            || shared.clients.lock_recover().idle_released,
            Duration::from_secs(2)
        ),
        "explicit fixture override must not retain the old five-second deadline"
    );
    assert_eq!(shared.clients.lock_recover().reopens_in_flight, 0);
    assert!(shared.clients.lock_recover().resident);
}

#[test]
fn resident_fixture_wakes_a_never_watcher_and_removes_only_its_registrations_on_unwind() {
    let (peer, _peer_shutdown) = resident_shared(Duration::from_secs(60));
    on_point(&peer, "fixture-peer", || {});
    let peer_id = peer.instance_id.clone();
    let mut path = None;
    let mut weak = None;
    let mut retired_id = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (fixture, _shutdown) = resident_shared(Duration::from_secs(60));
        path = Some(fixture.root.0.clone());
        weak = Some(Arc::downgrade(&fixture.shared));
        retired_id = Some(fixture.instance_id.clone());
        on_point(&fixture, "fixture-owned", || {});
        fixture.set_tray_hold(Some(IdlePolicy::Never));
        assert!(fixture.clients.lock_recover().observer_timer_running);
        panic!("injected resident fixture assertion failure");
    }));
    assert!(outcome.is_err());
    assert!(weak.unwrap().upgrade().is_none(), "Never watcher retired");
    assert!(!path.unwrap().exists());
    let id = retired_id.unwrap();
    assert!(!TEST_POINTS
        .lock_recover()
        .iter()
        .any(|(entry, _, _)| entry == &id));
    assert!(!TEST_GRACE
        .lock_recover()
        .iter()
        .any(|(entry, _)| entry == &id));
    assert!(TEST_POINTS
        .lock_recover()
        .iter()
        .any(|(entry, _, _)| entry == &peer_id));
    assert!(TEST_GRACE
        .lock_recover()
        .iter()
        .any(|(entry, _)| entry == &peer_id));
}

/// The shutdown grace these fixtures run with, in place of the shipped five
/// seconds (`BrokerShared::idle_grace`).
const GRACE: Duration = Duration::from_millis(300);

/// Pause a named point inside the broker until the returned sender is used
/// (or dropped). Only this fixture's broker pauses.
fn pause_at(shared: &BrokerShared, point: &'static str) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let reached_tx = Mutex::new(reached_tx);
    TEST_POINTS.lock_recover().push((
        shared.instance_id.clone(),
        point,
        Arc::new(move || {
            let _ = reached_tx.lock_recover().send(());
            let _ = release_rx
                .lock_recover()
                .recv_timeout(Duration::from_secs(10));
            Ok(())
        }),
    ));
    (release_tx, reached_rx)
}

/// Run `hook` when the named point is reached, without pausing.
fn on_point(shared: &BrokerShared, point: &'static str, hook: impl Fn() + Send + Sync + 'static) {
    TEST_POINTS.lock_recover().push((
        shared.instance_id.clone(),
        point,
        Arc::new(move || {
            hook();
            Ok(())
        }),
    ));
}

/// Replaces one controller's reconnect stage for the life of the guard.
struct ReconnectHook(usize);
impl ReconnectHook {
    fn install(
        controller: &Arc<DeviceController>,
        hook: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let key = Arc::as_ptr(controller) as usize;
        IDLE_RECONNECT_HOOKS
            .lock_recover()
            .push((key, Arc::new(hook)));
        Self(key)
    }
}
impl Drop for ReconnectHook {
    fn drop(&mut self) {
        IDLE_RECONNECT_HOOKS
            .lock_recover()
            .retain(|(key, _)| *key != self.0);
    }
}

fn attach_participant(shared: &Arc<BrokerShared>) -> u64 {
    let id = accept_client(&shared.clients).expect("accepted");
    shared
        .mark_client_authenticated(id)
        .expect("not shutting down");
    shared.update_observer_backstop();
    id
}

#[test]
fn an_admitted_disconnect_refuses_new_target_holders_until_the_worker_retires_its_reservation() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let caller = attach_participant(&shared);
    let observer = attach_participant(&shared);
    shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&observer)
        .unwrap()
        .role = ConnectionRole::Observer {
        idle_policy_minutes: Some(15),
    };
    let (release, reserved) = pause_at(&shared, "disconnect:reserved");
    let pending_shared = Arc::clone(&shared);
    let disconnect = spawn_fixture(move || {
        dispatch_broker_client_request(
            &pending_shared,
            Some(caller),
            request(1, generated_gateway::rpc::DISCONNECT),
        )
    });
    reserved.recv_timeout(Duration::from_secs(2)).unwrap();
    let newcomer = accept_client(&shared.clients).unwrap();
    let refused = shared.mark_client_authenticated(newcomer).unwrap_err();
    assert!(refused.contains("closing"), "{refused}");
    assert!(!shared.clients.lock_recover().connections[&newcomer].authenticated);
    let control = accept_client(&shared.clients).unwrap();
    shared
        .mark_client_authenticated_as(control, AuthenticatedAs::Control, None)
        .unwrap();
    assert!(shared.clients.lock_recover().is_control(control));
    let mut promote = request(2, "broker.hello");
    promote.params = json!({"role":"participant"});
    let refused = dispatch_broker_client_request(&shared, Some(observer), promote);
    assert!(refused.get("error").is_some(), "{refused}");
    assert!(shared.clients.lock_recover().is_observer(observer));
    release.send(()).unwrap();
    let completed = disconnect.join().unwrap();
    assert!(completed.get("error").is_none(), "{completed}");
    // A reply is sent before the command's separate admission guard drops.
    assert!(wait_for(
        || !shared
            .clients
            .lock_recover()
            .device_is_closing(&shared.controller),
        Duration::from_secs(1),
    ));
    shared.mark_client_authenticated(newcomer).unwrap();
    assert!(shared.clients.lock_recover().connections[&newcomer].authenticated);
}

#[test]
fn participant_selection_cannot_join_a_reserved_device_or_replace_its_current_target() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let participant = attach_participant(&shared);
    // Register both logical owners without HID discovery or transport opening.
    assert!(shared.controller.claim_device("fixture-A"));
    let b = Arc::new(DeviceController::start_for_device(Some("fixture-B".into()), false).unwrap());
    shared.devices.lock_recover().push(Arc::clone(&b));
    let a = shared.controller_for(Some(participant));
    let reservation = {
        let _exclusive = shared.request_lock.lock_recover();
        shared
            .reserve_disconnect_for(None, &b)
            .expect("unused B can close")
    };
    let refused = shared
        .select_available_device(participant, "fixture-B")
        .unwrap_err();
    assert!(refused.contains("closing"), "{refused}");
    assert!(Arc::ptr_eq(&shared.controller_for(Some(participant)), &a));
    assert_eq!(shared.clients.lock_recover().participants(), 1);
    drop(reservation);
    shared
        .select_available_device(participant, "fixture-B")
        .unwrap();
    assert!(Arc::ptr_eq(&shared.controller_for(Some(participant)), &b));
}

fn wait_for(condition: impl Fn() -> bool, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    condition()
}

fn request(id: u64, method: &str) -> Request {
    Request {
        jsonrpc: "2.0".into(),
        id: json!(id),
        method: method.into(),
        params: json!({}),
        origin: None,
    }
}

/// Replaces one controller's wait stage for the life of the guard.
struct Hook(usize);
impl Hook {
    fn install(
        controller: &Arc<DeviceController>,
        hook: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let key = Arc::as_ptr(controller) as usize;
        IDLE_REOPEN_HOOKS.lock_recover().push((key, Arc::new(hook)));
        Self(key)
    }
}
impl Drop for Hook {
    fn drop(&mut self) {
        IDLE_REOPEN_HOOKS
            .lock_recover()
            .retain(|(key, _)| *key != self.0);
    }
}

fn state_of(shared: &BrokerShared, controller: &Arc<DeviceController>) -> Option<IdleReopen> {
    shared.clients.lock_recover().idle_reopen_state(controller)
}

#[test]
fn a_resident_broker_releases_every_idle_device_after_the_grace_and_stays() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let second = Arc::new(DeviceController::start_disconnected().unwrap());
    shared.devices.lock_recover().push(Arc::clone(&second));
    let participant = attach_participant(&shared);
    shared.detach_client(participant);
    assert!(
        shutdown
            .recv_timeout(GRACE + Duration::from_millis(700))
            .is_err(),
        "residence keeps the process past the grace"
    );
    assert!(shared.clients.lock_recover().idle_released);
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released)
    );
    assert_eq!(state_of(&shared, &second), Some(IdleReopen::Released));
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(accept_or_refuse_client(&shared.clients), "still published");
}

#[test]
fn a_resident_icon_period_closes_the_device_sessions_not_the_process() {
    let minute = Duration::from_millis(200);
    let (shared, shutdown) = resident_shared(minute);
    shared.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    let participant = attach_participant(&shared);
    shared.detach_client(participant);
    assert!(wait_for(
        || shared.clients.lock_recover().idle_released,
        Duration::from_secs(3)
    ));
    assert!(shutdown.try_recv().is_err(), "the process is not reaped");
    let clients = shared.clients.lock_recover();
    assert_eq!(
        shared.reap_deadline(&clients),
        None,
        "nothing left to release"
    );
    drop(clients);
    shared.clients.lock_recover().shutting_down = true;
}

/// The first-request race: requests that arrive while the device is being
/// reopened wait for the reopen and are then served on the reopened session,
/// in order, instead of failing against the closed one.
#[test]
fn requests_during_an_idle_reopen_wait_for_it_and_are_served_after_it() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let participant = attach_participant(&shared);
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let reopens = Arc::new(AtomicUsize::new(0));
    let (entered, entered_rx) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let (asking, asking_rx) = mpsc::channel();
    let _hook = {
        let (events, reopens) = (Arc::clone(&events), Arc::clone(&reopens));
        Hook::install(&shared.controller, move || {
            reopens.fetch_add(1, Ordering::SeqCst);
            events.lock_recover().push("reopen started".into());
            let _ = entered.send(());
            released
                .lock_recover()
                .recv_timeout(Duration::from_secs(2))
                .map_err(|error| format!("fixture reopen gate: {error}"))?;
            events.lock_recover().push("reopen ready".into());
            Ok(())
        })
    };
    let started = Instant::now();
    let handles: Vec<_> = (1..=2)
        .map(|id| {
            let (shared, events) = (Arc::clone(&shared), Arc::clone(&events));
            let asking = asking.clone();
            let handle = spawn_fixture(move || {
                let _ = asking.send(id);
                let response = dispatch_broker_client_request(
                    &shared,
                    Some(participant),
                    request(id, generated_gateway::rpc::CURRENT_STATE_EVENTS),
                );
                events.lock_recover().push(format!("request {id} answered"));
                response
            });
            if id == 1 {
                entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            }
            handle
        })
        .collect();
    assert_eq!(asking_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    assert_eq!(asking_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
    thread::sleep(Duration::from_millis(400));
    assert!(
        handles.iter().all(|handle| !handle.is_finished()),
        "both requests remain admitted during the reopen"
    );
    release.send(()).unwrap();
    // Collect both admissions before joining: a lazy spawn.map(join) would
    // silently serialize them and never exercise the reopen-sharing race.
    let responses: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "both waited"
    );
    for response in &responses {
        assert!(
            response.get("error").is_none(),
            "served, not failed: {response}"
        );
        assert!(response["result"].is_object(), "{response}");
    }
    let events = events.lock_recover().clone();
    assert_eq!(
        events[..2],
        ["reopen started", "reopen ready"],
        "{events:?}"
    );
    assert_eq!(events.len(), 4, "{events:?}");
    assert_eq!(
        reopens.load(Ordering::SeqCst),
        1,
        "joined, not reopened twice"
    );
    assert_eq!(state_of(&shared, &shared.controller), None);
    assert!(!shared.clients.lock_recover().idle_released);
    shared.clients.lock_recover().shutting_down = true;
}

#[test]
fn a_reopen_that_fails_or_runs_out_of_time_is_a_clear_error_and_the_next_request_retries() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let participant = attach_participant(&shared);
    let method = generated_gateway::rpc::CURRENT_STATE_EVENTS;

    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let hook = Hook::install(&shared.controller, || Err("device unplugged".into()));
    let failed = dispatch_broker_client_request(&shared, Some(participant), request(1, method));
    assert_eq!(failed["error"]["code"], -32002, "{failed}");
    assert_eq!(
        failed["error"]["message"], "The Quad Cortex is not connected: device unplugged",
        "the reopen's own reason reaches the waiter"
    );
    assert_eq!(failed["error"]["data"]["category"], "notConnected");
    assert_eq!(failed["error"]["data"]["retryable"], true);
    assert!(matches!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Failed(_))
    ));
    drop(hook);

    // A reopen slower than the bound: the request gets a timeout, not a hang.
    let hook = Hook::install(&shared.controller, || {
        thread::sleep(IDLE_REOPEN_WAIT + Duration::from_millis(700));
        Ok(())
    });
    let started = Instant::now();
    let timed_out = dispatch_broker_client_request(&shared, Some(participant), request(2, method));
    let waited = started.elapsed();
    assert!(waited >= IDLE_REOPEN_WAIT && waited < IDLE_REOPEN_WAIT + Duration::from_millis(600));
    assert!(
        timed_out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("did not reopen within"),
        "{timed_out}"
    );
    // The slow reopen finishes; the next request is served without another.
    assert!(wait_for(
        || state_of(&shared, &shared.controller).is_none(),
        Duration::from_secs(2)
    ));
    drop(hook);
    let served = dispatch_broker_client_request(&shared, Some(participant), request(3, method));
    assert!(served.get("error").is_none(), "{served}");
    shared.clients.lock_recover().shutting_down = true;
}

/// A client that selected a non-default device gets that device back, and
/// only that one: never the default in its place, never an unused device.
#[test]
fn only_the_device_a_request_is_routed_to_is_reopened() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let second = Arc::new(DeviceController::start_disconnected().unwrap());
    let unused = Arc::new(DeviceController::start_disconnected().unwrap());
    shared.devices.lock_recover().push(Arc::clone(&second));
    shared.devices.lock_recover().push(Arc::clone(&unused));
    // The release closes all three.
    shared.release_idle_device();
    for controller in [&shared.controller, &second, &unused] {
        assert_eq!(state_of(&shared, controller), Some(IdleReopen::Released));
    }
    let reopened = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let hooks: Vec<_> = [
        (&shared.controller, "default"),
        (&second, "second"),
        (&unused, "unused"),
    ]
    .into_iter()
    .map(|(controller, name)| {
        let reopened = Arc::clone(&reopened);
        Hook::install(controller, move || {
            reopened.lock_recover().push(name);
            Ok(())
        })
    })
    .collect();
    let on_second = attach_participant(&shared);
    shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&on_second)
        .unwrap()
        .selected_device = Some(Arc::clone(&second));
    let method = generated_gateway::rpc::CURRENT_STATE_EVENTS;
    let response = dispatch_broker_client_request(&shared, Some(on_second), request(1, method));
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(*reopened.lock_recover(), ["second"]);
    assert!(Arc::ptr_eq(
        &shared.controller_for(Some(on_second)),
        &second
    ));
    assert_eq!(state_of(&shared, &second), None);
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released)
    );
    assert_eq!(state_of(&shared, &unused), Some(IdleReopen::Released));

    // A client on the default device brings the default back, and only it.
    let on_default = attach_participant(&shared);
    let response = dispatch_broker_client_request(&shared, Some(on_default), request(2, method));
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(*reopened.lock_recover(), ["second", "default"]);
    assert_eq!(
        state_of(&shared, &unused),
        Some(IdleReopen::Released),
        "never opened"
    );
    drop(hooks);
    shared.clients.lock_recover().shutting_down = true;
}

#[test]
fn only_a_participants_device_request_claims_the_reopen() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let reopens = Arc::new(AtomicUsize::new(0));
    let _hook = {
        let reopens = Arc::clone(&reopens);
        Hook::install(&shared.controller, move || {
            reopens.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };
    let events = generated_gateway::rpc::CURRENT_STATE_EVENTS;
    // A socket still authenticating: its row starts as a participant, but it
    // is nobody's participant yet.
    let pending = accept_client(&shared.clients).unwrap();
    dispatch_broker_client_request(&shared, Some(pending), request(1, events));
    // An observer.
    let observer = attach_participant(&shared);
    shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&observer)
        .unwrap()
        .role = ConnectionRole::Observer {
        idle_policy_minutes: Some(15),
    };
    dispatch_broker_client_request(&shared, Some(observer), request(2, events));
    // A participant's status poll is not device demand either.
    let participant = attach_participant(&shared);
    let started = Instant::now();
    let status = dispatch_broker_client_request(
        &shared,
        Some(participant),
        request(3, generated_gateway::rpc::RUNTIME_STATUS),
    );
    assert!(status.get("error").is_none(), "{status}");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the poll never waits"
    );
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        reopens.load(Ordering::SeqCst),
        0,
        "no USB without device demand"
    );
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released)
    );
    // The participant's device request is.
    let served = dispatch_broker_client_request(&shared, Some(participant), request(4, events));
    assert!(served.get("error").is_none(), "{served}");
    assert_eq!(reopens.load(Ordering::SeqCst), 1);

    // A disconnect is not device demand either, and may still be refused:
    // the released entry stays until the disconnect really runs.
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    assert!(shared
        .prepare_idle_released_target(
            Some(participant),
            &shared.controller,
            generated_gateway::rpc::DISCONNECT
        )
        .is_ok());
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released)
    );
    assert_eq!(reopens.load(Ordering::SeqCst), 1);
    shared.clients.lock_recover().shutting_down = true;
}

/// H1: a `--resident` start publishes with the device closed and nobody to
/// open it. The first participant's device request opens it.
#[test]
fn a_broker_started_disconnected_opens_on_the_first_device_request() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    handle.mark_started_disconnected();
    assert!(shared.clients.lock_recover().idle_released);
    // The icon registers a day-long hold: no grace runs, nothing to release.
    shared.set_tray_hold(Some(IdlePolicy::Never));
    let reopened = Arc::new(AtomicUsize::new(0));
    let _hook = {
        let reopened = Arc::clone(&reopened);
        Hook::install(&shared.controller, move || {
            reopened.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };
    let host = attach_participant(&shared);
    let response = dispatch_broker_client_request(
        &shared,
        Some(host),
        request(1, generated_gateway::rpc::CURRENT_STATE_EVENTS),
    );
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(reopened.load(Ordering::SeqCst), 1);
    assert!(!shared.clients.lock_recover().idle_released);
    shared.clients.lock_recover().shutting_down = true;
}

/// M1: the icon's Connect opens the device itself, so it leaves the released
/// set, and the idle policy can release it again afterwards.
#[test]
fn the_icons_connect_ends_the_idle_release() {
    let (shared, _shutdown) = resident_shared(Duration::from_millis(100));
    let handle = BrokerHandle(Arc::clone(&shared));
    shared.release_idle_device();
    assert!(shared.clients.lock_recover().idle_released);
    shared.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    {
        let clients = shared.clients.lock_recover();
        assert_eq!(
            shared.reap_deadline(&clients),
            None,
            "nothing open to release"
        );
    }
    // Under test the icon's reconnect never reaches the worker or hardware.
    handle.reconnect().unwrap();
    assert!(!shared.clients.lock_recover().idle_released);
    assert_eq!(state_of(&shared, &shared.controller), None);
    assert!(
        wait_for(
            || shared.clients.lock_recover().idle_released,
            Duration::from_secs(3)
        ),
        "with something open, the icon's period releases it again"
    );
    shared.clients.lock_recover().shutting_down = true;
}

/// M3: the icon's Disconnect leaves this process's own MCP host out. With only
/// the host attached it closes the devices and keeps the process; with a real
/// client attached it is still the forced release.
#[test]
fn the_icons_disconnect_does_not_count_the_in_process_host() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    let host = attach_participant(&shared);
    shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&host)
        .unwrap()
        .in_process_host = true;
    assert_eq!(handle.participants_other_than_host(), 0);
    assert!(handle.disconnect().is_ok());
    assert!(
        shutdown.try_recv().is_err(),
        "the host's AI session keeps the process"
    );
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(
        shared.clients.lock_recover().idle_released,
        "L6: recorded as released"
    );
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released)
    );
    let marker = BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id);
    assert!(!marker.exists(), "no forced release");

    let _window = attach_participant(&shared);
    assert_eq!(handle.participants_other_than_host(), 1);
    assert!(handle.disconnect().is_ok());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("a real client attached: the forced release");
    assert!(marker.exists());
    std::fs::remove_file(marker).unwrap();
}

/// L3: no grace, reap or installer stop commits under a running reopen.
#[test]
fn a_running_reopen_holds_off_every_shutdown_commit_until_it_finishes() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let (finish_tx, finish_rx) = mpsc::channel::<()>();
    let finish_rx = Mutex::new(finish_rx);
    let _hook = Hook::install(&shared.controller, move || {
        let _ = finish_rx
            .lock_recover()
            .recv_timeout(Duration::from_secs(20));
        Ok(())
    });
    let participant = attach_participant(&shared);
    let waiting = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            dispatch_broker_client_request(
                &shared,
                Some(participant),
                request(1, generated_gateway::rpc::CURRENT_STATE_EVENTS),
            )
        })
    };
    assert!(wait_for(
        || shared.clients.lock_recover().reopens_in_flight == 1,
        Duration::from_secs(2)
    ));
    // The participant gives up and leaves mid-handshake.
    shared.detach_client(participant);
    assert!(
        !handle.release_for_install(),
        "an installer stop waits for the handshake"
    );
    // Still resident, nobody attached: only the running reopen stops this.
    assert!(shared.clients.lock_recover().resident);
    shared.release_idle_device();
    assert!(
        !shared.clients.lock_recover().idle_released,
        "nor does an idle release"
    );
    assert!(
        handle
            .release_and_exit(false)
            .unwrap_err()
            .contains("reopening"),
        "nor an unforced Release and exit"
    );
    // Residence lost: the grace would now exit, but waits for the handshake.
    shared.set_resident(false);
    assert!(
        shutdown
            .recv_timeout(GRACE + Duration::from_millis(700))
            .is_err(),
        "nor the grace"
    );
    assert!(!shared.clients.lock_recover().shutting_down);
    // The request gives up at its bound; the handshake does not.
    let timed_out = waiting.join().unwrap();
    assert!(
        timed_out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("did not reopen within"),
        "{timed_out}"
    );
    assert_eq!(shared.clients.lock_recover().reopens_in_flight, 1);
    assert!(
        !shared.clients.lock_recover().shutting_down,
        "still held off"
    );
    finish_tx.send(()).unwrap();
    assert!(wait_for(
        || shared.clients.lock_recover().reopens_in_flight == 0,
        Duration::from_secs(2)
    ));
    shutdown
        .recv_timeout(GRACE + Duration::from_secs(2))
        .expect("once the handshake is done the grace exits a non-resident broker");
}

/// L1: a stale idle-release flag never keeps a broker that is not resident.
#[test]
fn a_stale_idle_release_does_not_keep_a_non_resident_broker() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    {
        let mut clients = shared.clients.lock_recover();
        clients.idle_released = true;
        clients.observer_idle_since = Some(Instant::now());
    }
    let observer = accept_client(&shared.clients).unwrap();
    {
        let mut clients = shared.clients.lock_recover();
        let row = clients.connections.get_mut(&observer).unwrap();
        row.authenticated = true;
        row.role = ConnectionRole::Observer {
            idle_policy_minutes: Some(1),
        };
    }
    {
        let clients = shared.clients.lock_recover();
        assert_eq!(
            shared.reap_deadline(&clients),
            None,
            "resident: nothing to release"
        );
    }
    shared.clients.lock_recover().resident = false;
    let clients = shared.clients.lock_recover();
    assert!(
        shared.reap_deadline(&clients).is_some(),
        "not resident: the observer's backstop runs"
    );
    drop(clients);
    shared.clients.lock_recover().shutting_down = true;
}

#[test]
fn losing_residence_with_nobody_attached_falls_back_to_the_shipped_grace() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    assert!(handle.is_resident());
    let removed = Instant::now();
    handle.set_resident(false);
    shutdown
        .recv_timeout(GRACE + Duration::from_secs(2))
        .expect("trayEnabled=false: no host, and the five-second grace reaps");
    assert!(removed.elapsed() >= GRACE - Duration::from_millis(50));
    assert!(shared.clients.lock_recover().shutting_down);
}

#[test]
fn an_idle_release_never_closes_a_session_a_participant_reached_first() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let _participant = attach_participant(&shared);
    shared.release_idle_device();
    assert!(!shared.clients.lock_recover().idle_released);
    assert!(shared
        .clients
        .lock_recover()
        .idle_released_devices
        .is_empty());
    assert!(shutdown.try_recv().is_err());
    shared.clients.lock_recover().shutting_down = true;
}

#[test]
fn a_resident_broker_still_steps_aside_for_an_installer_and_still_exits_on_a_forced_release() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    let participant = attach_participant(&shared);
    assert!(!handle.release_for_install(), "a participant keeps it");
    shared.detach_client(participant);
    assert!(handle.release_for_install());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("an installer stop exits even a resident process");

    let (forced, exited) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&forced));
    let _attached = attach_participant(&forced);
    handle.release_and_exit(true).unwrap();
    exited
        .recv_timeout(Duration::from_secs(2))
        .expect("a forced release exits the whole process");
    let marker = BrokerReleaseMarker::path_for(&forced.descriptor_path, &forced.instance_id);
    assert!(
        marker.exists(),
        "attached clients must not silently reacquire"
    );
    std::fs::remove_file(marker).unwrap();
}

/// A reopen whose thread is still waiting for `request_lock` when a
/// (forced) shutdown commits never reconnects.
#[test]
fn a_shutdown_committed_before_the_reconnect_cancels_it() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let reconnects = Arc::new(AtomicUsize::new(0));
    let _reconnect = {
        let reconnects = Arc::clone(&reconnects);
        ReconnectHook::install(&shared.controller, move || {
            reconnects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };
    let _wait = Hook::install(&shared.controller, || Ok(()));
    let participant = attach_participant(&shared);
    let held = shared.request_lock.lock_recover();
    let waiting = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            dispatch_broker_client_request(
                &shared,
                Some(participant),
                request(1, generated_gateway::rpc::CURRENT_STATE_EVENTS),
            )
        })
    };
    assert!(wait_for(
        || shared.clients.lock_recover().reopens_in_flight == 1,
        Duration::from_secs(2)
    ));
    // A forced release commits while the reopen waits for the lock.
    shared.clients.lock_recover().shutting_down = true;
    drop(held);
    let response = waiting.join().unwrap();
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("shutdown is already committed"),
        "{response}"
    );
    assert_eq!(reconnects.load(Ordering::SeqCst), 0, "no handshake started");
    assert_eq!(shared.clients.lock_recover().reopens_in_flight, 0);
}

/// A reopen that panics still releases its in-flight count and fails
/// its waiters; the next request tries again.
#[test]
fn a_panicking_reopen_fails_its_waiters_and_releases_every_shutdown_path() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let participant = attach_participant(&shared);
    let method = generated_gateway::rpc::CURRENT_STATE_EVENTS;
    let hook = Hook::install(&shared.controller, || panic!("reopen exploded"));
    let started = Instant::now();
    let failed = dispatch_broker_client_request(&shared, Some(participant), request(1, method));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "not the full wait"
    );
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("idle reopen original worker failed or never started"),
        "{failed}"
    );
    assert!(
        failed["result"].is_null(),
        "panic must not publish success: {failed}"
    );
    assert_eq!(shared.clients.lock_recover().reopens_in_flight, 0);
    assert!(matches!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Failed(_))
    ));
    drop(hook);
    shared.detach_client(participant);
    assert!(
        handle.release_and_exit(false).is_ok(),
        "nothing in flight blocks an unforced exit"
    );
}

/// Neither the idle release nor the icon's Disconnect holds `clients`
/// while it waits for `devices`, so a path that holds `devices` and then
/// takes `clients` (as `broker.devices` once did) cannot deadlock with them.
#[test]
fn the_idle_release_and_the_icons_disconnect_never_wait_for_devices_holding_clients() {
    for path in ["idle release", "icon disconnect"] {
        let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
        let devices = shared.devices.lock_recover();
        let worker = {
            let shared = Arc::clone(&shared);
            spawn_fixture(move || {
                if path == "idle release" {
                    shared.release_idle_device();
                } else {
                    let _ = BrokerHandle(Arc::clone(&shared)).disconnect();
                }
            })
        };
        // The worker holds request_lock and is now blocked on `devices`.
        assert!(wait_for(
            || shared.request_lock.try_lock().is_err(),
            Duration::from_secs(2)
        ));
        thread::sleep(Duration::from_millis(200));
        assert!(
            shared.clients.try_lock().is_ok(),
            "{path}: `clients` must be free while it waits for `devices`"
        );
        drop(devices);
        worker.join().unwrap();
        assert!(shared.clients.lock_recover().idle_released, "{path}");
        shared.clients.lock_recover().shutting_down = true;
    }
}

/// A request arriving after resident idle release has closed and recorded the
/// devices waits for the release operation, then reopens.
#[test]
fn a_request_after_resident_idle_release_reopens_after_the_close() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    {
        let events = Arc::clone(&events);
        on_point(&shared, "disconnect_devices:done", move || {
            events.lock_recover().push("closed")
        });
    }
    let _reconnect = {
        let events = Arc::clone(&events);
        ReconnectHook::install(&shared.controller, move || {
            events.lock_recover().push("reconnect");
            Ok(())
        })
    };
    let _wait = Hook::install(&shared.controller, || Ok(()));
    let (resume, reached) = pause_at(&shared, "release_idle_device:recorded");
    let releasing = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || shared.release_idle_device())
    };
    reached.recv_timeout(Duration::from_secs(2)).unwrap();
    // The close is complete and recorded. A participant arrives while the
    // release still owns request_lock, then asks for the device.
    let participant = attach_participant(&shared);
    let asking = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            dispatch_broker_client_request(
                &shared,
                Some(participant),
                request(1, generated_gateway::rpc::CURRENT_STATE_EVENTS),
            )
        })
    };
    assert!(wait_for(
        || shared.clients.lock_recover().reopens_in_flight == 1,
        Duration::from_secs(2)
    ));
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        *events.lock_recover(),
        ["closed"],
        "native close must precede release publication, and the reopen waits"
    );
    resume.send(()).unwrap();
    releasing.join().unwrap();
    let response = asking.join().unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(*events.lock_recover(), ["closed", "reconnect"]);
    shared.clients.lock_recover().shutting_down = true;
}

/// A request that starts while native close is in flight must still observe
/// the successful close once it gets request_lock and reopen the session.
#[test]
fn a_request_during_resident_idle_close_reopens_after_the_close() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (resume, resume_rx) = mpsc::channel();
    let (reached_tx, reached_rx) = mpsc::channel();
    let resume_rx = Mutex::new(resume_rx);
    {
        let events = Arc::clone(&events);
        on_point(&shared, "disconnect_devices:done", move || {
            events.lock_recover().push("closed");
            let _ = reached_tx.send(());
            let _ = resume_rx
                .lock_recover()
                .recv_timeout(Duration::from_secs(10));
        });
    }
    let _reconnect = {
        let events = Arc::clone(&events);
        ReconnectHook::install(&shared.controller, move || {
            events.lock_recover().push("reconnect");
            Ok(())
        })
    };
    let _wait = Hook::install(&shared.controller, || Ok(()));
    let releasing = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || shared.release_idle_device())
    };
    reached_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        !shared.clients.lock_recover().idle_released,
        "release publication waits for close completion"
    );

    let participant = attach_participant(&shared);
    let (request_started, request_waiting) = mpsc::channel();
    let asking = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            let _ = request_started.send(());
            dispatch_broker_client_request(
                &shared,
                Some(participant),
                request(1, generated_gateway::rpc::CURRENT_PRESET),
            )
        })
    };
    request_waiting
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(
        !asking.is_finished(),
        "the request is queued behind resident close while marker is not yet published"
    );

    resume.send(()).unwrap();
    releasing.join().unwrap();
    let response = asking.join().unwrap();
    assert_eq!(
        response["error"]["message"], "No Quad Cortex preset has been synchronized yet",
        "the request reaches the device handler after a successful reopen"
    );
    assert_eq!(*events.lock_recover(), ["closed", "reconnect"]);
    shared.clients.lock_recover().shutting_down = true;
}

/// Only the token proves the host. A client naming
/// itself after the host, with no token, a wrong one, or after a re-hello
/// without it, is an ordinary participant, and the icon's Disconnect forces
/// it off.
#[test]
fn the_host_is_recognised_by_its_token_never_by_its_name() {
    let (shared, shutdown) = resident_shared(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    let id = accept_client(&shared.clients).unwrap();
    shared.mark_client_authenticated(id).unwrap();
    let hello = |params: Value| {
        let mut request = request(7, "broker.hello");
        request.params = params;
        let response = dispatch_broker_client_request(&shared, Some(id), request);
        assert_eq!(response["result"]["accepted"], true, "{response}");
        shared.clients.lock_recover().connections[&id].in_process_host
    };
    let name = IN_PROCESS_HOST_PROVIDER;
    assert!(
        !hello(json!({"role":"participant","provider":name})),
        "name alone"
    );
    assert!(!hello(
        json!({"role":"participant","provider":name,"hostToken":"0".repeat(64)})
    ));
    assert!(!hello(
        json!({"role":"participant","provider":name,"hostToken":7})
    ));
    assert!(hello(
        json!({"role":"participant","provider":name,"hostToken":crate::resident::host_token()})
    ));
    assert_eq!(handle.participants_other_than_host(), 0);
    // A re-hello without the token loses it.
    assert!(
        !hello(json!({"role":"participant","provider":name})),
        "re-hello"
    );
    assert_eq!(handle.participants_other_than_host(), 1);
    assert!(handle.disconnect().is_ok());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("an unproven 'host' is a client: Disconnect is the forced release");
    let marker = BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id);
    assert!(marker.exists());
    std::fs::remove_file(marker).unwrap();
}

/// The reap rechecks shortly while a reopen runs.
#[test]
fn the_reap_rechecks_in_a_quarter_second_while_a_reopen_runs() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    shared.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    let mut clients = shared.clients.lock_recover();
    clients.reopens_in_flight = 1;
    let before = Instant::now();
    let deadline = shared.reap_deadline(&clients).expect("a recheck");
    assert!(deadline >= before + Duration::from_millis(240));
    assert!(deadline <= Instant::now() + Duration::from_millis(260));
    clients.reopens_in_flight = 0;
    clients.shutting_down = true;
}

/// An observer's reconnect ends the idle release, so the idle
/// policy can close the session it opened.
#[test]
fn an_observers_reconnect_ends_the_idle_release() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    shared.release_idle_device();
    assert!(shared.clients.lock_recover().idle_released);
    let observer = attach_participant(&shared);
    shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&observer)
        .unwrap()
        .role = ConnectionRole::Observer {
        idle_policy_minutes: Some(1),
    };
    assert!(shared
        .prepare_idle_released_target(
            Some(observer),
            &shared.controller,
            generated_gateway::rpc::RECONNECT
        )
        .is_ok());
    assert!(!shared.clients.lock_recover().idle_released);
    assert_eq!(state_of(&shared, &shared.controller), None);
    shared.clients.lock_recover().shutting_down = true;
}

/// Lock-free reads keep working during a committed shutdown when
/// there is nothing to reopen; a released device is refused clearly.
#[test]
fn a_committed_shutdown_refuses_only_requests_that_need_a_reopen() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let participant = attach_participant(&shared);
    shared.clients.lock_recover().shutting_down = true;
    let method = generated_gateway::rpc::CURRENT_STATE_EVENTS;
    assert!(shared
        .prepare_idle_released_target(Some(participant), &shared.controller, method)
        .is_ok());
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let refused = shared
        .prepare_idle_released_target(Some(participant), &shared.controller, method)
        .unwrap_err();
    assert!(refused.contains("not connected"), "{refused}");
}

/// Counts real reconnects (the reconnect stage) and lets the wait succeed.
fn counting_reopen(controller: &Arc<DeviceController>) -> (Arc<AtomicUsize>, ReconnectHook, Hook) {
    let reconnects = Arc::new(AtomicUsize::new(0));
    let reconnect = {
        let reconnects = Arc::clone(&reconnects);
        ReconnectHook::install(controller, move || {
            reconnects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };
    (reconnects, reconnect, Hook::install(controller, || Ok(())))
}

/// Third pass (a): a disconnect refused because another participant is
/// attached leaves the idle release alone. The other participant's request
/// really reconnects the device, and later requests are served on it.
#[test]
fn a_refused_disconnect_never_cancels_the_idle_reopen() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let a = attach_participant(&shared);
    let b = attach_participant(&shared);
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let (reconnects, _reconnect, _wait) = counting_reopen(&shared.controller);
    let refused = dispatch_broker_client_request(
        &shared,
        Some(a),
        request(1, generated_gateway::rpc::DISCONNECT),
    );
    assert!(
        refused["result"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("left open")),
        "{refused}"
    );
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released),
        "a refused disconnect leaves the entry"
    );
    let method = generated_gateway::rpc::CURRENT_STATE_EVENTS;
    let served = dispatch_broker_client_request(&shared, Some(b), request(2, method));
    assert!(served.get("error").is_none(), "{served}");
    assert_eq!(reconnects.load(Ordering::SeqCst), 1, "a real reconnect");
    assert_eq!(state_of(&shared, &shared.controller), None);
    let again = dispatch_broker_client_request(&shared, Some(a), request(3, method));
    assert!(again.get("error").is_none(), "{again}");
    assert_eq!(
        reconnects.load(Ordering::SeqCst),
        1,
        "served on the reopened session"
    );
    shared.clients.lock_recover().shutting_down = true;
}

/// Third pass (b): an observer's disconnect while a participant's reopen
/// waits for `request_lock` neither cancels nor short-circuits that reopen.
#[test]
fn an_observers_disconnect_during_a_queued_reopen_leaves_it_to_reconnect() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let participant = attach_participant(&shared);
    let observer = attach_participant(&shared);
    shared
        .clients
        .lock_recover()
        .connections
        .get_mut(&observer)
        .unwrap()
        .role = ConnectionRole::Observer {
        idle_policy_minutes: Some(1),
    };
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let (reconnects, _reconnect, _wait) = counting_reopen(&shared.controller);
    let method = generated_gateway::rpc::CURRENT_STATE_EVENTS;
    let held = shared.request_lock.lock_recover();
    let waiting = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            dispatch_broker_client_request(&shared, Some(participant), request(1, method))
        })
    };
    assert!(wait_for(
        || shared.clients.lock_recover().reopens_in_flight == 1,
        Duration::from_secs(2)
    ));
    let observed = dispatch_broker_client_request(
        &shared,
        Some(observer),
        request(2, generated_gateway::rpc::DISCONNECT),
    );
    assert!(observed.get("error").is_none(), "{observed}");
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Reopening),
        "the queued reopen still owns the device"
    );
    thread::sleep(Duration::from_millis(100));
    assert!(
        !waiting.is_finished(),
        "the waiter still waits for the reopen"
    );
    drop(held);
    let served = waiting.join().unwrap();
    assert!(served.get("error").is_none(), "{served}");
    assert_eq!(reconnects.load(Ordering::SeqCst), 1, "a real reconnect");
    assert_eq!(state_of(&shared, &shared.controller), None);
    let again = dispatch_broker_client_request(&shared, Some(participant), request(3, method));
    assert!(again.get("error").is_none(), "{again}");
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    shared.clients.lock_recover().shutting_down = true;
}

/// A disconnect that really runs (the sole participant's) withdraws the idle
/// release: the caller closed the session, so nothing reopens it unasked.
#[test]
fn a_disconnect_that_runs_withdraws_the_idle_release() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let sole = attach_participant(&shared);
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
    let (reconnects, _reconnect, _wait) = counting_reopen(&shared.controller);
    let closed = dispatch_broker_client_request(
        &shared,
        Some(sole),
        request(1, generated_gateway::rpc::DISCONNECT),
    );
    assert!(closed.get("error").is_none(), "{closed}");
    assert_eq!(state_of(&shared, &shared.controller), None);
    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    shared.clients.lock_recover().shutting_down = true;
}

/// Two devices, each idle-released so a disconnect that really runs is
/// visible: it withdraws its own device's entry just before `handle()`.
fn two_released_devices(shared: &Arc<BrokerShared>) -> Arc<DeviceController> {
    let b = Arc::new(DeviceController::start_disconnected().unwrap());
    shared.devices.lock_recover().push(Arc::clone(&b));
    shared.clients.lock_recover().idle_released_devices = vec![
        IdleReopenEntry::released(Arc::clone(&shared.controller)),
        IdleReopenEntry::released(Arc::clone(&b)),
    ];
    b
}

fn set_row(
    shared: &BrokerShared,
    id: u64,
    role: ConnectionRole,
    device: Option<Arc<DeviceController>>,
) {
    let mut clients = shared.clients.lock_recover();
    let row = clients.connections.get_mut(&id).unwrap();
    row.role = role;
    row.selected_device = device;
}

/// An observer's disconnect of device A passes the cheap pre-check, then
/// waits; meanwhile a participant reaches A and the same connection's
/// selection moves to B. The re-check under `request_lock` must count A, the
/// device the disconnect acts on, and refuse it; re-reading the selection
/// counted B instead and closed A under its participant.
#[test]
fn a_disconnect_rechecks_the_device_it_acts_on_after_a_selection_change() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let a = Arc::clone(&shared.controller);
    let b = two_released_devices(&shared);
    let observer = attach_participant(&shared);
    set_row(
        &shared,
        observer,
        ConnectionRole::Observer {
            idle_policy_minutes: Some(15),
        },
        None,
    );
    let (release, reached) = pause_at(&shared, "disconnect:after_precheck");
    let waiting = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            dispatch_broker_client_request(
                &shared,
                Some(observer),
                request(1, generated_gateway::rpc::DISCONNECT),
            )
        })
    };
    reached
        .recv_timeout(Duration::from_secs(5))
        .expect("the disconnect of A passed its pre-check");
    // A participant reaches A; the observer's selection moves to B.
    let participant = attach_participant(&shared);
    assert!(Arc::ptr_eq(&shared.controller_for(Some(participant)), &a));
    set_row(
        &shared,
        observer,
        ConnectionRole::Observer {
            idle_policy_minutes: Some(15),
        },
        Some(Arc::clone(&b)),
    );
    release.send(()).unwrap();
    let refused = waiting.join().unwrap();
    assert!(
        refused["result"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("left open")),
        "A still has a participant: {refused}"
    );
    assert_eq!(
        state_of(&shared, &a),
        Some(IdleReopen::Released),
        "nothing ran on A"
    );
    assert_eq!(
        state_of(&shared, &b),
        Some(IdleReopen::Released),
        "nothing ran on B"
    );
    shared.clients.lock_recover().shutting_down = true;
}

/// The realistic form of the same race: `broker.selectDevice` needs a
/// participant, so the waiting caller is promoted and then selects B. Its
/// own participation on B must not discount A's participant.
#[test]
fn a_caller_participating_on_another_device_does_not_discount_the_target_holder() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let a = Arc::clone(&shared.controller);
    let b = two_released_devices(&shared);
    let caller = attach_participant(&shared);
    set_row(
        &shared,
        caller,
        ConnectionRole::Observer {
            idle_policy_minutes: Some(15),
        },
        None,
    );
    let (release, reached) = pause_at(&shared, "disconnect:after_precheck");
    let waiting = {
        let shared = Arc::clone(&shared);
        spawn_fixture(move || {
            dispatch_broker_client_request(
                &shared,
                Some(caller),
                request(1, generated_gateway::rpc::DISCONNECT),
            )
        })
    };
    reached
        .recv_timeout(Duration::from_secs(5))
        .expect("the disconnect of A passed its pre-check");
    let _holder = attach_participant(&shared);
    set_row(
        &shared,
        caller,
        ConnectionRole::Participant,
        Some(Arc::clone(&b)),
    );
    assert!(
        shared.disconnect_blocked_for(Some(caller), &a),
        "A's participant blocks the caller now on B"
    );
    assert!(
        !shared.disconnect_blocked_for(Some(caller), &b),
        "the caller alone on B may close B"
    );
    release.send(()).unwrap();
    let refused = waiting.join().unwrap();
    assert!(
        refused["result"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("left open")),
        "A still has a participant: {refused}"
    );
    assert_eq!(state_of(&shared, &a), Some(IdleReopen::Released));
    assert_eq!(state_of(&shared, &b), Some(IdleReopen::Released));
    shared.clients.lock_recover().shutting_down = true;
}

/// Third pass LOW: recording a device again keeps its attempt and failure.
#[test]
fn recording_a_device_again_keeps_its_attempt_and_last_failure() {
    let mut clients = ClientAccounting::default();
    let controller = Arc::new(DeviceController::start_disconnected().unwrap());
    clients.idle_released_devices = vec![IdleReopenEntry {
        controller: Arc::clone(&controller),
        reopening: false,
        attempt: 3,
        last_failure: Some((3, "device unplugged".into())),
    }];
    let older_release = Arc::new(DeviceController::start_disconnected().unwrap());
    clients
        .idle_released_devices
        .push(IdleReopenEntry::released(Arc::clone(&older_release)));
    let worker_reconnected = Arc::new(DeviceController::start_disconnected().unwrap());
    worker_reconnected.set_connected_for_test();
    clients
        .idle_released_devices
        .push(IdleReopenEntry::released(Arc::clone(&worker_reconnected)));
    let other = Arc::new(DeviceController::start_disconnected().unwrap());
    clients.record_idle_release(vec![Arc::clone(&controller), Arc::clone(&other)]);
    let kept = clients.idle_entry(&controller).unwrap();
    assert_eq!(kept.attempt, 3);
    assert_eq!(kept.last_failure, Some((3, "device unplugged".into())));
    assert!(!kept.reopening);
    let fresh = clients.idle_entry(&other).unwrap();
    assert_eq!((fresh.attempt, fresh.last_failure.clone()), (0, None));
    assert!(
        clients.idle_entry(&older_release).is_some(),
        "a partial new release keeps the earlier accepted close marker"
    );
    assert!(
        clients.idle_entry(&worker_reconnected).is_some(),
        "cached connected status cannot prove an earlier accepted close finished"
    );
}

/// The in-process host's readiness read (`/health`, the relay's Ready frame)
/// while the host holds no participant connection. It answers from the
/// worker's cached session, so a broker holding an open Quad Cortex reports it
/// ready with nobody attached, and the read itself is never device demand: no
/// command reaches the worker, no idle reopen is claimed, no participant
/// appears, and an idle release stays recorded.
#[test]
fn device_readiness_answers_from_the_cached_session_without_device_demand() {
    let (controller, commands) = DeviceController::live_session_for_test();
    let controller = Arc::new(controller);
    let shared = Arc::new(BrokerShared {
        controller: Arc::clone(&controller),
        devices: Mutex::new(Vec::new()),
        performance_midi: Arc::new(PerformanceMidiWorkers::default()),
        request_lock: Arc::new(Mutex::new(())),
        clients: Arc::new(Mutex::new(ClientAccounting::default())),
        descriptor_path: std::env::temp_dir().join(format!(
            "qc-broker-readiness-unused-{}.json",
            generate_token(8)
        )),
        instance_id: generate_token(16),
        descriptor_claim: Arc::new(Mutex::new(None)),
        on_idle_shutdown: Arc::new(|| {}),
        observer_changed: Condvar::new(),
        observer_minute: Duration::from_secs(60),
        background: background::Manager::default(),
        observer_actions: background::Manager::default(),
    });
    let (reconnects, _reconnect, _wait) = counting_reopen(&controller);
    let handle = BrokerHandle(Arc::clone(&shared));
    let probe = crate::resident::DeviceReadinessProbe::new(handle.clone());

    controller.set_connection_for_test(true, "ready");
    assert_eq!(probe.read(), crate::resident::DeviceReadiness::Ready);

    // A session closed for idleness reads not ready, and stays closed.
    controller.set_connection_for_test(false, "disconnected");
    handle.mark_started_disconnected();
    for _ in 0..5 {
        assert_eq!(
            probe.read(),
            crate::resident::DeviceReadiness::NotReady {
                phase: "disconnected".into()
            }
        );
    }
    assert_eq!(commands.device_work(), 0, "the read reached the worker");
    assert_eq!(reconnects.load(Ordering::SeqCst), 0, "the read reopened");
    assert_eq!(
        state_of(&shared, &controller),
        Some(IdleReopen::Released),
        "the read moved the idle release"
    );
    {
        let clients = shared.clients.lock_recover();
        assert_eq!(clients.reopens_in_flight, 0, "the read claimed a reopen");
        assert!(clients.idle_released, "the read withdrew the idle release");
        assert!(
            clients.connections.is_empty() && clients.count == 0,
            "the read attached a client"
        );
    }

    shared.clients.lock_recover().shutting_down = true;
    assert!(matches!(
        probe.read(),
        crate::resident::DeviceReadiness::Unavailable(_)
    ));
    assert_eq!(commands.device_work(), 0);
}

/// A broker over `controller` with no listener, for the readiness tests.
fn readiness_shared(controller: &Arc<DeviceController>) -> Arc<BrokerShared> {
    Arc::new(BrokerShared {
        controller: Arc::clone(controller),
        devices: Mutex::new(Vec::new()),
        performance_midi: Arc::new(PerformanceMidiWorkers::default()),
        request_lock: Arc::new(Mutex::new(())),
        clients: Arc::new(Mutex::new(ClientAccounting::default())),
        descriptor_path: std::env::temp_dir().join(format!(
            "qc-broker-readiness-unused-{}.json",
            generate_token(8)
        )),
        instance_id: generate_token(16),
        descriptor_claim: Arc::new(Mutex::new(None)),
        on_idle_shutdown: Arc::new(|| {}),
        observer_changed: Condvar::new(),
        observer_minute: Duration::from_secs(60),
        background: background::Manager::default(),
        observer_actions: background::Manager::default(),
    })
}

/// A device whose idle reopen is running (the worker handshaking) or whose
/// last reopen failed reads not ready, and the read leaves both as found.
#[test]
fn a_reopening_or_failed_idle_reopen_reads_not_ready() {
    let (controller, commands) = DeviceController::live_session_for_test();
    let controller = Arc::new(controller);
    let shared = readiness_shared(&controller);
    let probe = crate::resident::DeviceReadinessProbe::new(BrokerHandle(Arc::clone(&shared)));

    controller.set_connection_for_test(false, "handshaking");
    shared.clients.lock_recover().idle_released_devices = vec![IdleReopenEntry {
        controller: Arc::clone(&controller),
        reopening: true,
        attempt: 1,
        last_failure: None,
    }];
    assert_eq!(
        probe.read(),
        crate::resident::DeviceReadiness::NotReady {
            phase: "handshaking".into()
        }
    );
    assert_eq!(state_of(&shared, &controller), Some(IdleReopen::Reopening));

    controller.set_connection_for_test(false, "disconnected");
    shared.clients.lock_recover().idle_released_devices = vec![IdleReopenEntry {
        controller: Arc::clone(&controller),
        reopening: false,
        attempt: 1,
        last_failure: Some((1, "device unplugged".into())),
    }];
    assert_eq!(
        probe.read(),
        crate::resident::DeviceReadiness::NotReady {
            phase: "disconnected".into()
        }
    );
    assert_eq!(
        state_of(&shared, &controller),
        Some(IdleReopen::Failed("device unplugged".into()))
    );
    assert_eq!(commands.device_work(), 0);
    shared.clients.lock_recover().shutting_down = true;
}

/// `/health` and the relay poll this every second. Polling must leave the
/// icon's idle clock and the observer backstop's origin exactly as found, or
/// a monitored host would never release the QC.
#[test]
fn readiness_polling_leaves_the_idle_clock_alone() {
    let (controller, _commands) = DeviceController::live_session_for_test();
    let controller = Arc::new(controller);
    let shared = readiness_shared(&controller);
    let probe = crate::resident::DeviceReadinessProbe::new(BrokerHandle(Arc::clone(&shared)));
    let origin = Instant::now() - Duration::from_secs(90);
    {
        let mut clients = shared.clients.lock_recover();
        clients.tray_hold = Some(IdlePolicy::Minutes(15));
        clients.tray_idle_since = Some(origin);
        clients.observer_idle_since = Some(origin);
    }
    for connected in [true, false, true, false] {
        controller
            .set_connection_for_test(connected, if connected { "ready" } else { "disconnected" });
        let _ = probe.read();
    }
    let clients = shared.clients.lock_recover();
    assert_eq!(
        clients.tray_idle_since,
        Some(origin),
        "the idle clock moved"
    );
    assert_eq!(clients.observer_idle_since, Some(origin));
    assert_eq!(clients.tray_hold, Some(IdlePolicy::Minutes(15)));
    assert_eq!(clients.count, 0);
    drop(clients);
    shared.clients.lock_recover().shutting_down = true;
}

/// The read waits only on the worker's own status lock: blocked while a
/// worker update holds it, answered as soon as it is released. (The host
/// bounds the wait itself; see the app's
/// `an_unreadable_broker_answers_not_ready_promptly_and_opens_nothing`.)
#[test]
fn readiness_waits_only_for_the_status_lock() {
    let (controller, commands) = DeviceController::live_session_for_test();
    let controller = Arc::new(controller);
    let shared = readiness_shared(&controller);
    let probe = crate::resident::DeviceReadinessProbe::new(BrokerHandle(Arc::clone(&shared)));
    controller.set_connection_for_test(true, "ready");
    let held = controller.hold_status_lock_for_test();
    let (answered_tx, answered) = mpsc::channel();
    let reader = spawn_fixture(move || {
        let _ = answered_tx.send(probe.read());
    });
    assert!(
        answered.recv_timeout(Duration::from_millis(150)).is_err(),
        "the read answered past a held status lock"
    );
    drop(held);
    assert_eq!(
        answered.recv_timeout(Duration::from_secs(2)),
        Ok(crate::resident::DeviceReadiness::Ready)
    );
    reader.join().unwrap();
    assert_eq!(commands.device_work(), 0);
    shared.clients.lock_recover().shutting_down = true;
}

/// `request`, marked as an MCP resource read.
fn resource_read(id: u64, method: &str) -> Request {
    Request {
        origin: Some(RESOURCE_READ_ORIGIN.into()),
        ..request(id, method)
    }
}

/// Counts every reconnect an idle reopen asks the worker for, and lets each
/// one succeed at once.
fn count_reopens(shared: &BrokerShared) -> (Arc<AtomicUsize>, ReconnectHook, Hook) {
    let reopens = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reopens);
    let reconnect = ReconnectHook::install(&shared.controller, move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let wait = Hook::install(&shared.controller, || Ok(()));
    (reopens, reconnect, wait)
}

fn release_for_idleness(shared: &BrokerShared) {
    shared.clients.lock_recover().idle_released_devices =
        vec![IdleReopenEntry::released(Arc::clone(&shared.controller))];
}

/// A resource read sent to a device the resident broker
/// closed for idleness is refused as not connected, and never reaches
/// `prepare_idle_released_target`: no reopen is started or joined, and the
/// device stays released. The narrowing cannot be used to widen anything
/// either: a session command or a broker-level method marked as a resource
/// read is refused the same way.
#[test]
fn a_resource_read_on_an_idle_released_device_is_refused_and_never_reopens_it() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    release_for_idleness(&shared);
    let participant = attach_participant(&shared);
    let (reopens, _reconnect, _wait) = count_reopens(&shared);

    for (id, method) in [
        (1, generated_gateway::rpc::CURRENT_PRESET),
        (2, generated_gateway::rpc::LIST_MODELS),
        (3, generated_gateway::rpc::CURRENT_STATE_EVENTS),
        (4, generated_gateway::rpc::RECONNECT),
        (5, "broker.selectDevice"),
    ] {
        let response =
            dispatch_broker_client_request(&shared, Some(participant), resource_read(id, method));
        assert_eq!(response["error"]["code"], -32002, "{method}: {response}");
        assert_eq!(response["error"]["data"]["category"], "notConnected");
        assert_eq!(response["error"]["data"]["retryable"], true);
    }
    assert_eq!(reopens.load(Ordering::SeqCst), 0, "no reopen was started");
    assert_eq!(
        state_of(&shared, &shared.controller),
        Some(IdleReopen::Released),
        "the device stays released"
    );
    // The status read is answered as before: it reports the session as it is.
    let status = dispatch_broker_client_request(
        &shared,
        Some(participant),
        resource_read(6, generated_gateway::rpc::RUNTIME_STATUS),
    );
    assert!(status.get("error").is_none(), "{status}");
    assert!(
        status["result"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == RESOURCE_READ_ORIGIN_CAPABILITY),
        "the broker says it honours the narrowing: {status}"
    );
    assert_eq!(reopens.load(Ordering::SeqCst), 0);
    shared.clients.lock_recover().shutting_down = true;
}

/// The race a client cannot close on its own: the client's
/// `system.status` is answered with the session as it is, then the resident
/// broker really releases the idle device (`release_idle_device`), then the
/// client's resource read is admitted. The read is paused at its admission
/// point, so the release lands exactly between the two; nothing sleeps. Run
/// for a read that takes `request_lock` and for a cached one that does not.
#[test]
fn a_release_between_the_status_and_the_resource_read_never_reopens_the_device() {
    for method in [
        generated_gateway::rpc::LIST_MODELS,
        generated_gateway::rpc::CURRENT_STATE_EVENTS,
    ] {
        let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
        let (reopens, _reconnect, _wait) = count_reopens(&shared);
        let status = dispatch_broker_request(
            &shared,
            resource_read(1, generated_gateway::rpc::RUNTIME_STATUS),
        );
        assert!(status.get("error").is_none(), "{method}: {status}");
        assert_eq!(
            state_of(&shared, &shared.controller),
            None,
            "not released yet"
        );

        let (resume, paused) = pause_at(&shared, "resource_read:before_admission");
        let read = spawn_fixture({
            let shared = Arc::clone(&shared);
            move || dispatch_broker_request(&shared, resource_read(2, method))
        });
        paused
            .recv_timeout(Duration::from_secs(10))
            .expect("the read reached its admission point");
        shared.release_idle_device();
        assert_eq!(
            state_of(&shared, &shared.controller),
            Some(IdleReopen::Released),
            "{method}: released between the status and the read"
        );
        resume.send(()).expect("resume the read");
        let response = read.join().expect("read thread");

        assert_eq!(response["error"]["code"], -32002, "{method}: {response}");
        assert_eq!(response["error"]["data"]["category"], "notConnected");
        assert_eq!(response["error"]["data"]["retryable"], true);
        assert_eq!(reopens.load(Ordering::SeqCst), 0, "{method}: no reopen");
        assert_eq!(
            state_of(&shared, &shared.controller),
            Some(IdleReopen::Released),
            "{method}: the device stays released"
        );
        shared.clients.lock_recover().shutting_down = true;
    }
}

/// A resource read on a device that is not connected at all (no
/// idle release involved) is refused the same way, and starts nothing.
#[test]
fn a_resource_read_on_a_disconnected_device_is_refused_as_not_connected() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let participant = attach_participant(&shared);
    let (reopens, _reconnect, _wait) = count_reopens(&shared);
    assert!(
        !shared.controller.status().connected,
        "the fixture never connects"
    );
    let response = dispatch_broker_client_request(
        &shared,
        Some(participant),
        resource_read(1, generated_gateway::rpc::CURRENT_PRESET),
    );
    assert_eq!(response["error"]["code"], -32002, "{response}");
    assert_eq!(response["error"]["data"]["category"], "notConnected");
    assert_eq!(response["error"]["data"]["retryable"], true);
    assert_eq!(reopens.load(Ordering::SeqCst), 0);
    assert_eq!(state_of(&shared, &shared.controller), None);
    shared.clients.lock_recover().shutting_down = true;
}

/// Control: the same read with no origin - a tool call - or
/// with an origin this broker does not know, is device demand exactly as
/// before, and reopens the idle-released device.
#[test]
fn a_tool_read_or_an_unknown_origin_still_reopens_an_idle_released_device() {
    let (shared, _shutdown) = resident_shared(Duration::from_secs(60));
    let participant = attach_participant(&shared);
    let (reopens, _reconnect, _wait) = count_reopens(&shared);
    let unknown = Request {
        origin: Some("someLaterOrigin".into()),
        ..request(2, generated_gateway::rpc::CURRENT_STATE_EVENTS)
    };
    for (expected, call) in [
        (1, request(1, generated_gateway::rpc::CURRENT_STATE_EVENTS)),
        (2, unknown),
    ] {
        release_for_idleness(&shared);
        let response = dispatch_broker_client_request(&shared, Some(participant), call);
        assert!(response.get("error").is_none(), "served: {response}");
        assert_eq!(reopens.load(Ordering::SeqCst), expected, "{response}");
        assert_eq!(state_of(&shared, &shared.controller), None, "reopened");
    }
    shared.clients.lock_recover().shutting_down = true;
}

/// The envelope field is optional, and the envelope still
/// rejects any other unknown field.
#[test]
fn the_request_origin_is_an_optional_envelope_field() {
    let marked: Request = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "device.currentPreset",
        "params": {}, "origin": RESOURCE_READ_ORIGIN
    }))
    .expect("an origin parses");
    assert_eq!(marked.origin.as_deref(), Some(RESOURCE_READ_ORIGIN));
    let plain: Request = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "device.currentPreset", "params": {}
    }))
    .expect("no origin parses");
    assert_eq!(plain.origin, None);
    assert!(serde_json::from_value::<Request>(json!({
        "jsonrpc": "2.0", "id": 1, "method": "device.currentPreset",
        "params": {}, "somethingElse": true
    }))
    .is_err());
}

/// The `--stdio` loop advertises the narrowing in its
/// `system.status` like the TCP broker, so it must enforce it, never drop it.
/// Its only writer is the host that spawned it (`start_owned_child`); every
/// request it reads goes through `serve_stdio_request`. A resource read of a
/// session that is not connected, and a resource read of anything but a
/// device read, are refused as not connected; the same read without the
/// origin is handled by `handle` exactly as before.
#[test]
fn the_stdio_loop_enforces_the_resource_read_origin() {
    let controller = DeviceController::start_disconnected().expect("controller");
    let performance_midi = PerformanceMidiWorkers::default();
    assert!(!controller.status().connected, "the fixture never connects");

    let status = serve_stdio_request(
        &controller,
        &performance_midi,
        resource_read(1, generated_gateway::rpc::RUNTIME_STATUS),
    );
    assert!(status.get("error").is_none(), "{status}");
    assert!(
        status["result"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == RESOURCE_READ_ORIGIN_CAPABILITY),
        "stdio advertises the narrowing it enforces: {status}"
    );

    for (id, method) in [
        (2, generated_gateway::rpc::CURRENT_PRESET),
        (3, generated_gateway::rpc::LIST_MODELS),
        (4, generated_gateway::rpc::CURRENT_STATE_EVENTS),
        (5, generated_gateway::rpc::RECONNECT),
        (6, generated_gateway::rpc::DISCONNECT),
        (7, "broker.selectDevice"),
    ] {
        let refused =
            serve_stdio_request(&controller, &performance_midi, resource_read(id, method));
        assert_eq!(refused["error"]["code"], -32002, "{method}: {refused}");
        assert_eq!(refused["error"]["data"]["category"], "notConnected");
        assert_eq!(refused["error"]["data"]["retryable"], true);
        assert_eq!(refused["error"]["message"], RESOURCE_READ_NOT_OPEN);
    }

    // Control: without the origin the same cached read is served by `handle`
    // on the disconnected session, exactly as before.
    let served = serve_stdio_request(
        &controller,
        &performance_midi,
        request(8, generated_gateway::rpc::CURRENT_STATE_EVENTS),
    );
    assert!(served.get("error").is_none(), "{served}");
    assert!(served["result"].is_object(), "{served}");
}
