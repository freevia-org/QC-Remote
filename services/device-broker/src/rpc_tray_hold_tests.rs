#![cfg(test)]

//! The broker's own icon hold: the idle policy now lives in this process, and
//! it may keep the device past the shipped grace only while the icon shows.
use super::*;

struct TrayFixture {
    shared: Arc<BrokerShared>,
    root: Arc<crate::device_lease::testing::FixtureRoot>,
}
impl std::ops::Deref for TrayFixture {
    type Target = Arc<BrokerShared>;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}
impl Drop for TrayFixture {
    fn drop(&mut self) {
        self.shared.clients.lock_recover().shutting_down = true;
        self.shared.observer_changed.notify_all();
        let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(2);
        let retired = || {
            Arc::strong_count(&self.shared) == 1
                && Arc::strong_count(&self.shared.clients) == 1
                && Arc::strong_count(&self.shared.request_lock) == 1
        };
        while !retired() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if !retired() {
            static RETAINED: Mutex<
                Vec<(
                    Arc<BrokerShared>,
                    Arc<crate::device_lease::testing::FixtureRoot>,
                )>,
            > = Mutex::new(Vec::new());
            RETAINED
                .lock_recover()
                .push((Arc::clone(&self.shared), Arc::clone(&self.root)));
            eprintln!(
                "tray fixture resources unacknowledged; exact shared state and root retained"
            );
        }
    }
}

fn shared_with_minute(minute: Duration) -> (TrayFixture, mpsc::Receiver<()>) {
    let root = crate::device_lease::testing::fresh_root("tray-hold");
    shared_with_minute_at(minute, root.0.join("device-broker.json"), root)
}

fn shared_with_minute_at(
    minute: Duration,
    descriptor_path: PathBuf,
    root: Arc<crate::device_lease::testing::FixtureRoot>,
) -> (TrayFixture, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(BrokerShared {
        controller: Arc::new(DeviceController::start_disconnected().expect("controller")),
        devices: Mutex::new(Vec::new()),
        performance_midi: Arc::new(PerformanceMidiWorkers::default()),
        request_lock: Arc::new(Mutex::new(())),
        clients: Arc::new(Mutex::new(ClientAccounting::default())),
        descriptor_path,
        instance_id: generate_token(16),
        descriptor_claim: Arc::new(Mutex::new(None)),
        on_idle_shutdown: Arc::new(move || {
            let _ = tx.send(());
        }),
        observer_changed: Condvar::new(),
        background: background::Manager::default(),
        observer_actions: background::Manager::default(),
        demo: crate::demo_provider::Hub::default(),
        targets: crate::target::TargetRegistry::default(),
        target_transition: Mutex::new(()),
        observer_minute: minute,
    });
    (TrayFixture { shared, root }, rx)
}

fn attach_participant(shared: &Arc<BrokerShared>) -> u64 {
    let id = accept_client(&shared.clients).expect("accepted");
    shared
        .mark_client_authenticated(id)
        .expect("not shutting down");
    shared.update_observer_backstop();
    id
}

fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    predicate()
}

#[test]
fn tray_fixture_unwind_wakes_its_never_holder_before_retiring_the_root() {
    let scratch = QuitScratch::new();
    let directory = scratch.0.clone();
    let mut weak = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (shared, _shutdown) = shared_with_minute_at(
            Duration::from_secs(60),
            scratch.descriptor(),
            Arc::clone(&scratch.1),
        );
        weak = Some(Arc::downgrade(&shared.shared));
        shared.set_tray_hold(Some(IdlePolicy::Never));
        panic!("injected tray assertion failure");
    }));
    assert!(failure.is_err());
    assert!(
        weak.unwrap().upgrade().is_none(),
        "Never watcher retired its fixture reference"
    );
    drop(scratch);
    assert!(!directory.exists());
}

#[test]
fn a_shown_icon_holds_past_the_grace_and_releases_when_its_period_ends() {
    let minute = Duration::from_millis(400);
    let (shared, shutdown) = shared_with_minute(minute);
    let participant = attach_participant(&shared);
    shared.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    let left = Instant::now();
    shared.detach_client(participant);
    shutdown
        .recv_timeout(Duration::from_secs(3))
        .expect("the icon's own period must release the device");
    let elapsed = left.elapsed();
    assert!(
        elapsed >= minute - Duration::from_millis(50),
        "released after {elapsed:?}, before the one-minute period"
    );
    assert!(
        elapsed < IDLE_SHUTDOWN_GRACE,
        "the icon's timer, not the grace, must release it"
    );
    assert!(shared.clients.lock_recover().shutting_down);
    assert!(!accept_or_refuse_client(&shared.clients));
}

#[test]
fn a_returning_participant_cancels_the_hold_and_restarts_the_period() {
    let minute = Duration::from_millis(300);
    let (shared, shutdown) = shared_with_minute(minute);
    shared.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    let first = attach_participant(&shared);
    shared.detach_client(first);
    assert!(wait_for(Duration::from_secs(2), || {
        shared
            .clients
            .lock_recover()
            .tray_idle_since
            .is_some_and(|idle_since| idle_since.elapsed() >= Duration::from_millis(200))
    }));
    let second = attach_participant(&shared);
    assert!(shared.clients.lock_recover().tray_idle_since.is_none());
    assert!(
        shutdown.recv_timeout(Duration::from_millis(500)).is_err(),
        "an attached participant is never reaped"
    );
    let left = Instant::now();
    shared.detach_client(second);
    shutdown
        .recv_timeout(Duration::from_secs(3))
        .expect("released a full period after the second participant left");
    assert!(left.elapsed() >= minute - Duration::from_millis(50));
}

#[test]
fn never_holds_through_the_grace_until_the_icon_goes() {
    let (shared, shutdown) = shared_with_minute(Duration::from_millis(5));
    shared.set_tray_hold(Some(IdlePolicy::Never));
    let participant = attach_participant(&shared);
    shared.detach_client(participant);
    assert!(
        shutdown
            .recv_timeout(IDLE_SHUTDOWN_GRACE + Duration::from_secs(1))
            .is_err(),
        "Never must outlast both the grace and any multiple of a minute"
    );
    assert!(!shared.clients.lock_recover().shutting_down);
    // The icon disappearing with nobody attached falls back to the shipped
    // grace: a broker without its icon never holds longer than it used to.
    let removed = Instant::now();
    shared.set_tray_hold(None);
    shutdown
        .recv_timeout(IDLE_SHUTDOWN_GRACE + Duration::from_secs(3))
        .expect("removing the icon re-arms the five-second grace");
    assert!(removed.elapsed() >= IDLE_SHUTDOWN_GRACE - Duration::from_millis(100));
}

#[test]
fn immediately_and_no_icon_hold_nothing_past_the_grace() {
    let mut clients = ClientAccounting::default();
    assert!(!clients.tray_holds_past_grace());
    clients.tray_hold = Some(IdlePolicy::Immediately);
    assert!(!clients.tray_holds_past_grace());
    clients.tray_hold = Some(IdlePolicy::Minutes(1));
    assert!(clients.tray_holds_past_grace());
    clients.tray_hold = Some(IdlePolicy::Never);
    assert!(clients.tray_holds_past_grace());
}

#[test]
fn the_icon_starts_its_origin_on_appearing_and_a_policy_change_keeps_it() {
    let (shared, _shutdown) = shared_with_minute(Duration::from_secs(60));
    let before = Instant::now();
    shared.set_tray_hold(Some(IdlePolicy::Minutes(5)));
    let origin = shared
        .clients
        .lock_recover()
        .tray_idle_since
        .expect("nobody attached: the icon's idle period starts when it appears");
    assert!(origin >= before);
    shared.set_tray_hold(Some(IdlePolicy::Minutes(10)));
    assert_eq!(shared.clients.lock_recover().tray_idle_since, Some(origin));
    {
        let clients = shared.clients.lock_recover();
        assert_eq!(
            shared.reap_deadline(&clients),
            Some(origin + shared.observer_minute * 10)
        );
    }
    shared.set_tray_hold(None);
    assert_eq!(shared.clients.lock_recover().tray_idle_since, None);
    shared.clients.lock_recover().shutting_down = true;
}

#[test]
fn a_pending_authentication_defers_the_reap_without_moving_the_origin() {
    let (shared, _shutdown) = shared_with_minute(Duration::from_secs(60));
    shared.set_tray_hold(Some(IdlePolicy::Minutes(1)));
    let origin = shared.clients.lock_recover().tray_idle_since.unwrap();
    let probe = accept_client(&shared.clients).unwrap();
    shared.update_observer_backstop();
    {
        let clients = shared.clients.lock_recover();
        assert_eq!(shared.reap_deadline(&clients), None);
        assert_eq!(clients.tray_idle_since, Some(origin));
    }
    // A probe that never authenticates leaves; the original origin stands.
    shared.detach_client(probe);
    let clients = shared.clients.lock_recover();
    assert_eq!(clients.tray_idle_since, Some(origin));
    assert_eq!(
        shared.reap_deadline(&clients),
        Some(origin + shared.observer_minute)
    );
    drop(clients);
    shared.clients.lock_recover().shutting_down = true;
}

#[test]
fn the_latest_holder_wins_between_the_icon_and_a_legacy_observer() {
    let (shared, _shutdown) = shared_with_minute(Duration::from_secs(60));
    let observer = accept_client(&shared.clients).unwrap();
    {
        let mut clients = shared.clients.lock_recover();
        let row = clients.connections.get_mut(&observer).unwrap();
        row.authenticated = true;
        row.role = ConnectionRole::Observer {
            idle_policy_minutes: Some(1),
        };
    }
    shared.set_tray_hold(Some(IdlePolicy::Minutes(30)));
    let mut clients = shared.clients.lock_recover();
    let origin = Instant::now() - Duration::from_secs(1);
    clients.observer_idle_since = Some(origin);
    clients.tray_idle_since = Some(origin);
    // The old tray's backstop is its period plus five minutes: 6 < 30.
    assert_eq!(
        shared.reap_deadline(&clients),
        Some(origin + shared.observer_minute * 30)
    );
    clients.tray_hold = Some(IdlePolicy::Minutes(2));
    assert_eq!(
        shared.reap_deadline(&clients),
        Some(origin + shared.observer_minute * 6)
    );
    clients.tray_hold = Some(IdlePolicy::Never);
    assert_eq!(shared.reap_deadline(&clients), None);
    clients.shutting_down = true;
}

#[test]
fn an_installer_stop_is_honoured_only_with_nobody_attached() {
    let (shared, shutdown) = shared_with_minute(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    shared.set_tray_hold(Some(IdlePolicy::Never));
    let participant = attach_participant(&shared);
    assert!(
        !handle.release_for_install(),
        "a participant keeps the broker"
    );
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(shutdown.try_recv().is_err());
    shared.detach_client(participant);
    let probe = accept_client(&shared.clients).unwrap();
    assert!(
        !handle.release_for_install(),
        "a socket still authenticating may be a participant a moment from now"
    );
    shared.detach_client(probe);
    assert!(
        handle.release_for_install(),
        "a held, unattended broker steps aside"
    );
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("the installer's stop exits the broker");
    assert!(!accept_or_refuse_client(&shared.clients));
    assert!(
        !BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id).exists(),
        "an upgrade is not a user's forced release"
    );
}

#[test]
fn the_icons_disconnect_keeps_both_unattended_and_attended_brokers() {
    let (shared, shutdown) = shared_with_minute(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    assert!(handle.disconnect().is_ok());
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(
        shutdown.try_recv().is_err(),
        "Connect can start the session again"
    );
    let _participant = attach_participant(&shared);
    assert!(handle
        .release_and_exit(false)
        .unwrap_err()
        .contains("participant is still attached"));
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(handle.disconnect().is_ok());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect_err("Disconnect must not exit an attended broker");
    let marker = BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id);
    assert!(!marker.exists(), "Disconnect is not Quit");
    assert!(!shared.clients.lock_recover().shutting_down);
}

#[test]
fn the_icon_lists_authenticated_participants_only() {
    let (shared, _shutdown) = shared_with_minute(Duration::from_secs(60));
    let handle = BrokerHandle(Arc::clone(&shared));
    let window = attach_participant(&shared);
    let observer = attach_participant(&shared);
    let host = attach_participant(&shared);
    let impostor = attach_participant(&shared);
    let _pending = accept_client(&shared.clients).unwrap();
    {
        let mut clients = shared.clients.lock_recover();
        clients.connections.get_mut(&window).unwrap().provider = "QC Remote window".into();
        let row = clients.connections.get_mut(&observer).unwrap();
        row.provider = "QC Remote tray".into();
        row.role = ConnectionRole::Observer {
            idle_policy_minutes: Some(15),
        };
        let row = clients.connections.get_mut(&host).unwrap();
        row.provider = IN_PROCESS_HOST_PROVIDER.into();
        row.in_process_host = true;
        // Only the token makes a connection this process's host; the name
        // alone is an ordinary client and stays listed.
        clients.connections.get_mut(&impostor).unwrap().provider = IN_PROCESS_HOST_PROVIDER.into();
    }
    assert_eq!(
        handle.participants(),
        [
            "QC Remote window",
            IN_PROCESS_HOST_PROVIDER,
            IN_PROCESS_HOST_PROVIDER
        ]
    );
    assert_eq!(
        handle.participants_without_host(),
        ["QC Remote window", IN_PROCESS_HOST_PROVIDER],
        "the icon never lists this process's own host"
    );
    assert_eq!(
        handle.directory(),
        shared.descriptor_path.parent().unwrap().to_path_buf()
    );
    shared.clients.lock_recover().shutting_down = true;
}

/// One test's own scratch directory: a fresh random name created exclusively,
/// removed when the test ends or unwinds.
struct QuitScratch(PathBuf, Arc<crate::device_lease::testing::FixtureRoot>);

impl QuitScratch {
    fn new() -> Self {
        let root = crate::device_lease::testing::fresh_root("user-quit");
        Self(root.0.clone(), root)
    }
    fn descriptor(&self) -> PathBuf {
        self.0.join("device-broker.json")
    }
}

fn recorded_quit(shared: &BrokerShared) -> Option<crate::user_quit::UserQuitRecord> {
    crate::user_quit::read(&shared.descriptor_path).map(|record| record.expect("readable"))
}

/// The icon's Quit with nobody but this process's host attached is not
/// forced and writes no release marker, but it is still the user's Quit:
/// recorded for this generation.
#[test]
fn the_icons_unforced_quit_records_the_users_quit() {
    let scratch = QuitScratch::new();
    let (shared, shutdown) = shared_with_minute_at(
        Duration::from_secs(60),
        scratch.descriptor(),
        Arc::clone(&scratch.1),
    );
    let handle = BrokerHandle(Arc::clone(&shared));
    assert!(handle.quit_by_user(false).is_ok());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("the Quit exits the broker");
    assert_eq!(
        recorded_quit(&shared),
        Some(crate::user_quit::UserQuitRecord {
            instance_id: shared.instance_id.clone(),
            source: crate::user_quit::UserQuitSource::Tray,
        })
    );
    assert!(
        !BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id).exists(),
        "unforced: no forced-release marker"
    );
}

/// With a client attached the icon's Quit is forced: both records stand.
#[test]
fn the_icons_forced_quit_records_the_users_quit_and_the_release() {
    let scratch = QuitScratch::new();
    let (shared, shutdown) = shared_with_minute_at(
        Duration::from_secs(60),
        scratch.descriptor(),
        Arc::clone(&scratch.1),
    );
    let handle = BrokerHandle(Arc::clone(&shared));
    let _window = attach_participant(&shared);
    assert!(handle.quit_by_user(true).is_ok());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("the forced Quit exits the broker");
    assert_eq!(
        recorded_quit(&shared).map(|record| record.instance_id),
        Some(shared.instance_id.clone())
    );
    assert!(BrokerReleaseMarker::path_for(&shared.descriptor_path, &shared.instance_id).exists());
}

/// A Quit that does not commit records nothing: refused for an attached
/// client (unforced), or because the record itself cannot be written, in
/// which case the error says so and the broker keeps serving.
#[test]
fn a_quit_that_does_not_commit_records_nothing_and_says_why() {
    let scratch = QuitScratch::new();
    let (shared, shutdown) = shared_with_minute_at(
        Duration::from_secs(60),
        scratch.descriptor(),
        Arc::clone(&scratch.1),
    );
    let handle = BrokerHandle(Arc::clone(&shared));
    let _participant = attach_participant(&shared);
    assert!(handle
        .quit_by_user(false)
        .unwrap_err()
        .contains("participant is still attached"));
    assert_eq!(recorded_quit(&shared), None);

    let blocked = QuitScratch::new();
    let not_a_directory = blocked.0.join("not-a-directory");
    std::fs::write(&not_a_directory, b"a file cannot hold a record").unwrap();
    let (unwritable, unwritable_shutdown) = shared_with_minute_at(
        Duration::from_secs(60),
        not_a_directory.join("device-broker.json"),
        Arc::clone(&blocked.1),
    );
    let error = BrokerHandle(Arc::clone(&unwritable))
        .quit_by_user(false)
        .unwrap_err();
    assert!(error.contains("was not quit"), "{error}");
    assert!(error.contains("device was left open"), "{error}");
    assert!(!unwritable.clients.lock_recover().shutting_down);
    assert!(unwritable_shutdown.try_recv().is_err(), "still serving");
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(shutdown.try_recv().is_err());
}

/// The release marker failing after the Quit was recorded refuses the
/// shutdown, and the record it wrote is withdrawn: nothing stands for a Quit
/// that did not happen.
#[test]
fn a_forced_quit_whose_release_marker_fails_withdraws_its_record() {
    let scratch = QuitScratch::new();
    let (shared, shutdown) = shared_with_minute_at(
        Duration::from_secs(60),
        scratch.descriptor(),
        Arc::clone(&scratch.1),
    );
    // A directory where the marker file would go: its atomic publish fails.
    std::fs::create_dir(BrokerReleaseMarker::path_for(
        &shared.descriptor_path,
        &shared.instance_id,
    ))
    .unwrap();
    let _window = attach_participant(&shared);
    let error = BrokerHandle(Arc::clone(&shared))
        .quit_by_user(true)
        .unwrap_err();
    assert!(error.contains("device was left open"), "{error}");
    assert_eq!(recorded_quit(&shared), None, "withdrawn");
    assert!(!shared.clients.lock_recover().shutting_down);
    assert!(shutdown.try_recv().is_err());
}

/// Exits that are not a user's Quit record none: `release_and_exit` as
/// another client's or the installer's shutdown takes it.
#[test]
fn an_exit_that_is_not_a_users_quit_records_none() {
    let scratch = QuitScratch::new();
    let (shared, shutdown) = shared_with_minute_at(
        Duration::from_secs(60),
        scratch.descriptor(),
        Arc::clone(&scratch.1),
    );
    assert!(BrokerHandle(Arc::clone(&shared))
        .release_and_exit(false)
        .is_ok());
    shutdown
        .recv_timeout(Duration::from_secs(2))
        .expect("the broker exits");
    assert_eq!(recorded_quit(&shared), None);
}
