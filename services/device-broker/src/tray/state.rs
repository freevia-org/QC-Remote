//! The tray's state thread: the only tray code that talks to the broker.
use super::host_status::{self, HostPresence};
use super::model::{MenuAction, Snapshot};
use crate::resident::{RemoteUnlink, ResidentHost};
use crate::rpc::BrokerHandle;
use crate::tray_settings::{self, TraySettings};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
const SETTINGS_INTERVAL: Duration = Duration::from_secs(1);
const HOST_INTERVAL: Duration = Duration::from_secs(1);

/// What the state and UI threads share. The UI thread reads `snapshot` for a
/// copy and writes only `registered`; the state thread writes the rest.
pub struct Shared {
    pub snapshot: Mutex<Snapshot>,
    /// The user's switch: whether the icon should be shown.
    pub wanted: AtomicBool,
    /// Whether the shell currently holds the icon. Only this enables a hold.
    pub registered: AtomicBool,
    /// When the state thread last replaced `snapshot`. The UI thread shows a
    /// wedged broker when this stops moving.
    pub refreshed: Mutex<Instant>,
}

/// Whether the UI callback remains part of this broker's tray lifecycle.
///
/// A UI backend can retire without dropping its action sender immediately
/// (for example, while its D-Bus service finishes shutting down). `Retired`
/// tells the state loop to release residency and the tray hold directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyStatus {
    Alive,
    Retired,
}

/// Wakes the UI thread to re-render from `Shared::snapshot` and reports its
/// lifecycle.
pub type Notify = Box<dyn Fn() -> NotifyStatus + Send + Sync>;

pub fn start<U>(
    handle: BrokerHandle,
    host: Option<Arc<dyn ResidentHost>>,
    start_ui: U,
) -> Result<(), String>
where
    U: FnOnce(Arc<Shared>, mpsc::Sender<MenuAction>) -> Result<Notify, String>,
{
    let shared = Arc::new(Shared {
        snapshot: Mutex::new(Snapshot::default()),
        wanted: AtomicBool::new(false),
        registered: AtomicBool::new(false),
        refreshed: Mutex::new(Instant::now()),
    });
    let (actions, received) = mpsc::channel();
    let notify = start_ui(Arc::clone(&shared), actions)?;
    std::thread::Builder::new()
        .name("qc-broker-tray-state".into())
        .spawn(move || {
            let fallback = handle.clone();
            // A panic here would otherwise leave the last hold and residency
            // standing with nothing left to withdraw them: fall back to the
            // shipped broker (no hold, the five-second grace).
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(handle, host, shared, received, notify)
            }));
            if outcome.is_err() {
                fallback.set_tray_hold(None);
                fallback.set_resident(false);
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn run(
    handle: BrokerHandle,
    resident_host: Option<Arc<dyn ResidentHost>>,
    shared: Arc<Shared>,
    received: mpsc::Receiver<MenuAction>,
    notify: Notify,
) {
    let directory = handle.directory();
    let mut settings = TraySettings::default();
    let mut host: Option<HostPresence> = None;
    let mut settings_at: Option<Instant> = None;
    let mut host_at: Option<Instant> = None;
    let (finished, results) = mpsc::channel::<String>();
    let mut busy = false;
    let mut action_detail = String::new();
    let mut device_status = crate::worker::BrokerStatus::default();
    loop {
        let now = Instant::now();
        if settings_at.is_none_or(|at| now.duration_since(at) >= SETTINGS_INTERVAL) {
            settings = tray_settings::read_in(&directory);
            settings_at = Some(now);
        }
        shared
            .wanted
            .store(settings.tray_enabled, Ordering::Release);
        #[cfg(not(target_os = "macos"))]
        handle.set_tray_hold(super::hold_for(
            shared.registered.load(Ordering::Acquire),
            settings,
        ));
        #[cfg(target_os = "macos")]
        handle.set_tray_hold(settings.tray_enabled.then_some(settings.idle));
        // Residence follows the switch and this thread, not the shell: an
        // Explorer restart that briefly drops the icon must not end the
        // session's MCP host. The device hold above still needs the icon.
        handle.set_resident(settings.tray_enabled);
        if host_at.is_none_or(|at| now.duration_since(at) >= HOST_INTERVAL) {
            host = host_status::read(&directory);
            host_at = Some(now);
        }
        while let Ok(detail) = results.try_recv() {
            busy = false;
            action_detail = detail;
        }
        if let Some(mut current) = handle.try_tray_status() {
            // The cached worker status knows the preset name immediately.
            // Its address comes from the retained session state, which a
            // device operation may legitimately hold. Keep the last address
            // (and name when the cached frame has none) until that short
            // authoritative read becomes available again.
            if current.synchronized && device_status.synchronized {
                if current.active_preset_location.is_none() {
                    current.active_preset_location = device_status.active_preset_location.clone();
                }
                if current.active_preset_name.is_none() {
                    current.active_preset_name = device_status.active_preset_name.clone();
                }
            }
            device_status = current;
        }
        let mut snapshot = Snapshot {
            participants: handle.participants_without_host(),
            host: host.clone(),
            idle: settings.idle,
            action_detail: action_detail.clone(),
            observed_at_unix_ms: unix_time_ms(),
            ..Snapshot::default()
        };
        snapshot.apply_status(&device_status);
        *shared
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
        *shared
            .refreshed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        if notify() == NotifyStatus::Retired {
            ui_disconnected(&shared, &handle);
            return;
        }
        match received.recv_timeout(POLL_INTERVAL) {
            Ok(MenuAction::SetIdle(minutes)) => {
                action_detail = tray_settings::set_idle_minutes_in(&directory, minutes)
                    .err()
                    .unwrap_or_default();
                settings_at = None;
            }
            Ok(MenuAction::OpenApp) => action_detail = open_app().err().unwrap_or_default(),
            // A lock and a map removal in this process's host: no device, no
            // broker lock, so it runs here rather than on an action thread.
            Ok(MenuAction::EndLocalSession(session)) => {
                action_detail = end_local_session(resident_host.as_deref(), &session);
                host_at = None;
            }
            Ok(action) if busy => {
                let _ = action;
                action_detail = "Waiting for the previous action to finish".into();
            }
            Ok(action) => {
                busy = true;
                action_detail = "Working…".into();
                let handle = handle.clone();
                let resident_host = resident_host.clone();
                let finished = finished.clone();
                // Disconnect waits for the request lock like any client, which
                // can take as long as another client's backup: never on this
                // thread, so the icon keeps reporting meanwhile.
                let spawned = std::thread::Builder::new()
                    .name("qc-broker-tray-action".into())
                    .spawn(move || {
                        let outcome = match action {
                            MenuAction::Connect => handle.reconnect(),
                            MenuAction::Disconnect => handle.disconnect(),
                            // Handled on the state thread above; never a
                            // reason to release the device.
                            MenuAction::OpenApp
                            | MenuAction::SetIdle(_)
                            | MenuAction::EndLocalSession(_) => Ok(String::new()),
                            // A request to the relay: on this action thread,
                            // never the state or UI thread.
                            MenuAction::UnlinkRemoteSession { session_id, .. } => {
                                Ok(unlink_remote_session(resident_host.as_deref(), &session_id))
                            }
                            MenuAction::UnlinkLinkedApp { entry_id, .. } => {
                                Ok(unlink_linked_app(resident_host.as_deref(), &entry_id))
                            }
                            MenuAction::ReleaseAndExit => {
                                // This process's own MCP host ends with it; only
                                // another client makes it a forced release,
                                // exactly as for Disconnect and the installer.
                                let forced = handle.participants_other_than_host() != 0;
                                // Forced or not, a user's Quit: recorded, so only a
                                // user action brings QC Remote back.
                                handle.quit_by_user(forced)
                            }
                        };
                        let _ = finished.send(outcome.unwrap_or_else(|error| error));
                    });
                if let Err(error) = spawned {
                    busy = false;
                    action_detail = error.to_string();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The UI action channel is gone, so drop the hold and residency.
                ui_disconnected(&shared, &handle);
                return;
            }
        }
    }
}

fn ui_disconnected(shared: &Shared, handle: &BrokerHandle) {
    shared.registered.store(false, Ordering::Release);
    handle.set_tray_hold(None);
    handle.set_resident(false);
}

fn unix_time_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

/// The icon's Unlink on a local client: ends that one session in this
/// process's background host, and reports the outcome as the menu's notice.
fn end_local_session(host: Option<&dyn ResidentHost>, session: &str) -> String {
    match host.map(|host| host.end_local_session(session)) {
        Some(Ok(true)) => "Unlinked. That chat links again on its next request.".into(),
        Some(Ok(false)) => "That session had already ended".into(),
        Some(Err(error)) => error,
        None => "No background MCP host runs in this process".into(),
    }
}

/// The icon's Unlink on a remote client, after the user confirmed: the host
/// in this process revokes that one session's grant. One attempt, reported as
/// the menu's notice; a failure leaves everything as it was.
fn unlink_remote_session(host: Option<&dyn ResidentHost>, session: &str) -> String {
    unlink_notice(
        host.map(|host| host.unlink_remote_session(session)),
        "That remote session had already ended; nothing was unlinked",
    )
}

/// The icon's Unlink on a linked remote app (one grant), after the user
/// confirmed: the host revokes the grant its own list holds for the entry.
fn unlink_linked_app(host: Option<&dyn ResidentHost>, entry: &str) -> String {
    unlink_notice(
        host.map(|host| host.unlink_linked_app(entry)),
        "That app is no longer listed; nothing was unlinked",
    )
}

/// The account's list, read just before the revoke, no longer had the app.
pub const UNLINK_NOT_LISTED: &str =
    "QC Remote Cloud no longer lists that app, so nothing was unlinked. The list was refreshed.";
/// The host's list was fetched under an earlier pairing.
pub const UNLINK_STALE_LIST: &str =
    "The linked-app list was from an earlier pairing, so nothing was unlinked. It will refresh.";

fn unlink_notice(outcome: Option<Result<RemoteUnlink, String>>, gone: &str) -> String {
    match outcome {
        Some(Ok(RemoteUnlink::Unlinked)) => {
            "Remote app unlinked. It must sign in again to use QC Remote.".into()
        }
        Some(Ok(RemoteUnlink::NotListed)) => UNLINK_NOT_LISTED.into(),
        Some(Ok(RemoteUnlink::StaleList)) => UNLINK_STALE_LIST.into(),
        Some(Ok(RemoteUnlink::SessionEnded)) => gone.into(),
        Some(Ok(RemoteUnlink::GrantUnknown)) => {
            "QC Remote Cloud did not name that app's authorization; unlink it in the Remote panel"
                .into()
        }
        Some(Err(error)) => format!("Unlink failed: {error}"),
        None => "No background MCP host runs in this process".into(),
    }
}

/// Open QC Remote from beside the broker. The app focuses an existing window
/// itself when one is already running.
fn open_app() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let app = app_beside(&executable).ok_or("QC Remote is not installed beside the broker")?;
    let mut command = std::process::Command::new(app);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open QC Remote: {error}"))
}

fn app_beside(broker: &Path) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let candidate = broker.parent()?.join("qc-remote.exe");
    #[cfg(target_os = "linux")]
    let candidate = broker.parent()?.join("qc-remote");
    #[cfg(not(any(windows, target_os = "linux")))]
    let candidate = broker.parent()?.join("qc-remote");
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resident::{DeviceReadinessProbe, RunningHost};

    #[test]
    fn busy_device_state_does_not_stop_the_tray_heartbeat() {
        let handle = crate::rpc::test_support::handle();
        let (notified_tx, notified_rx) = mpsc::channel();
        crate::rpc::test_support::with_session_lock(&handle, || {
            start(handle.clone(), None, move |_shared, _actions| {
                Ok(Box::new(move || {
                    let _ = notified_tx.send(());
                    NotifyStatus::Retired
                }))
            })
            .expect("tray state thread starts");
            assert_eq!(
                notified_rx.recv_timeout(Duration::from_secs(1)),
                Ok(()),
                "the tray heartbeat waited behind retained device state"
            );
        });
        crate::rpc::test_support::finish(&handle);
    }

    #[test]
    fn lost_ui_releases_registration_and_process_residency() {
        let handle = crate::rpc::test_support::handle();
        handle.set_resident(true);
        let shared = Shared {
            snapshot: Mutex::new(Snapshot::default()),
            wanted: AtomicBool::new(true),
            registered: AtomicBool::new(true),
            refreshed: Mutex::new(Instant::now()),
        };

        ui_disconnected(&shared, &handle);

        assert!(!shared.registered.load(Ordering::Acquire));
        assert!(!handle.is_resident());
    }

    #[test]
    fn retired_ui_releases_residency_while_action_sender_is_still_alive() {
        struct FinishOnDrop(BrokerHandle);

        impl Drop for FinishOnDrop {
            fn drop(&mut self) {
                crate::rpc::test_support::finish(&self.0);
            }
        }

        let handle = crate::rpc::test_support::handle();
        let _cleanup = FinishOnDrop(handle.clone());
        handle.set_resident(true);
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot::default()),
            wanted: AtomicBool::new(true),
            registered: AtomicBool::new(true),
            refreshed: Mutex::new(Instant::now()),
        });
        let (actions, received) = mpsc::channel();
        let actions_kept_alive = actions.clone();
        let resident_before_retirement = handle.clone();
        let notified = Arc::new(AtomicBool::new(false));
        let notified_once = Arc::clone(&notified);
        let handle_with_hold = handle.clone();

        run(
            handle.clone(),
            None,
            shared.clone(),
            received,
            Box::new(move || {
                assert!(
                    !notified_once.swap(true, Ordering::AcqRel),
                    "retired UI must not be notified again"
                );
                assert!(resident_before_retirement.is_resident());
                assert!(crate::rpc::test_support::tray_hold(&handle_with_hold).is_some());
                assert!(actions_kept_alive
                    .send(MenuAction::EndLocalSession("fixture-session".into()))
                    .is_ok());
                NotifyStatus::Retired
            }),
        );

        assert!(
            !handle.is_resident(),
            "retirement must release residency immediately"
        );
        assert!(!shared.registered.load(Ordering::Acquire));
        assert_eq!(crate::rpc::test_support::tray_hold(&handle), None);
        drop(actions);
    }

    /// A host whose sessions are a set of ids; nothing else exists.
    struct FakeHost(Mutex<Vec<String>>);

    impl ResidentHost for FakeHost {
        fn start(
            &self,
            _: &Path,
            _: &str,
            _: DeviceReadinessProbe,
        ) -> Result<Box<dyn RunningHost>, String> {
            Err("not under test".into())
        }
        fn end_local_session(&self, session_id: &str) -> Result<bool, String> {
            let mut sessions = self.0.lock().unwrap();
            let before = sessions.len();
            sessions.retain(|id| id != session_id);
            Ok(sessions.len() != before)
        }
    }

    /// A host whose remote sessions each hold a grant, and which records every
    /// revoke it is asked for.
    struct RemoteHost {
        grants: Vec<(&'static str, Option<&'static str>)>,
        fail: bool,
        revoked: Mutex<Vec<String>>,
    }

    impl RemoteHost {
        fn revoke(&self, grant: &str) -> Result<RemoteUnlink, String> {
            self.revoked.lock().unwrap().push(grant.to_owned());
            match (self.fail, grant) {
                (true, _) => Err("QC Remote Cloud could not unlink that connector".into()),
                (false, "GrantGONE404") => Ok(RemoteUnlink::NotListed),
                (false, "GrantOLDPAIR") => Ok(RemoteUnlink::StaleList),
                (false, _) => Ok(RemoteUnlink::Unlinked),
            }
        }
    }

    impl ResidentHost for RemoteHost {
        fn start(
            &self,
            _: &Path,
            _: &str,
            _: DeviceReadinessProbe,
        ) -> Result<Box<dyn RunningHost>, String> {
            Err("not under test".into())
        }
        fn unlink_remote_session(&self, session_id: &str) -> Result<RemoteUnlink, String> {
            match self.grants.iter().find(|(id, _)| *id == session_id) {
                None => Ok(RemoteUnlink::SessionEnded),
                Some((_, None)) => Ok(RemoteUnlink::GrantUnknown),
                Some((_, Some(grant))) => self.revoke(grant),
            }
        }
        /// Entries are the linked apps; an entry with a grant is listed.
        fn unlink_linked_app(&self, entry_id: &str) -> Result<RemoteUnlink, String> {
            match self.grants.iter().find(|(id, _)| *id == entry_id) {
                Some((_, Some(grant))) => self.revoke(grant),
                _ => Ok(RemoteUnlink::SessionEnded),
            }
        }
    }

    #[test]
    fn a_linked_app_unlinks_without_a_live_session_and_an_unlisted_one_is_not_called_unlinked() {
        let host = RemoteHost {
            grants: vec![
                ("app-1", Some("GrantAAAA1")),
                ("app-2", Some("GrantGONE404")),
                ("app-3", Some("GrantOLDPAIR")),
            ],
            fail: false,
            revoked: Mutex::new(Vec::new()),
        };
        assert_eq!(
            unlink_linked_app(Some(&host), "app-1"),
            "Remote app unlinked. It must sign in again to use QC Remote."
        );
        let not_listed = unlink_linked_app(Some(&host), "app-2");
        assert_eq!(not_listed, UNLINK_NOT_LISTED);
        assert!(
            !not_listed.starts_with("Remote app unlinked"),
            "{not_listed}"
        );
        assert_eq!(unlink_linked_app(Some(&host), "app-3"), UNLINK_STALE_LIST);
        assert_eq!(
            unlink_linked_app(Some(&host), "app-9"),
            "That app is no longer listed; nothing was unlinked"
        );
        assert_eq!(
            *host.revoked.lock().unwrap(),
            ["GrantAAAA1", "GrantGONE404", "GrantOLDPAIR"]
        );
        assert_eq!(
            unlink_linked_app(Some(&OldHost), "app-1"),
            "Unlink failed: This build's background host cannot unlink a remote app"
        );
    }

    #[test]
    fn remote_unlink_revokes_only_the_named_sessions_grant_and_reports_failure() {
        let host = RemoteHost {
            grants: vec![
                ("relay-1", Some("GrantAAAA1")),
                ("relay-2", Some("GrantBBBB2")),
                ("relay-old", None),
            ],
            fail: false,
            revoked: Mutex::new(Vec::new()),
        };
        assert_eq!(
            unlink_remote_session(Some(&host), "relay-2"),
            "Remote app unlinked. It must sign in again to use QC Remote."
        );
        assert_eq!(*host.revoked.lock().unwrap(), ["GrantBBBB2"]);
        assert!(unlink_remote_session(Some(&host), "relay-old").contains("Remote panel"));
        assert!(unlink_remote_session(Some(&host), "relay-gone").contains("already ended"));
        assert_eq!(*host.revoked.lock().unwrap(), ["GrantBBBB2"]);
        let failing = RemoteHost {
            fail: true,
            revoked: Mutex::new(Vec::new()),
            ..host
        };
        assert_eq!(
            unlink_remote_session(Some(&failing), "relay-1"),
            "Unlink failed: QC Remote Cloud could not unlink that connector"
        );
        assert_eq!(
            *failing.revoked.lock().unwrap(),
            ["GrantAAAA1"],
            "one attempt, no other grant"
        );
        assert_eq!(
            unlink_remote_session(Some(&OldHost), "relay-1"),
            "Unlink failed: This build's background host cannot unlink a remote app"
        );
        assert_eq!(
            unlink_remote_session(None, "relay-1"),
            "No background MCP host runs in this process"
        );
    }

    struct OldHost;

    impl ResidentHost for OldHost {
        fn start(
            &self,
            _: &Path,
            _: &str,
            _: DeviceReadinessProbe,
        ) -> Result<Box<dyn RunningHost>, String> {
            Err("not under test".into())
        }
    }

    #[test]
    fn unlink_ends_only_the_named_session_and_says_what_happened() {
        let host = FakeHost(Mutex::new(vec![
            "stdio-a".into(),
            "stdio-b".into(),
            "http-c".into(),
        ]));
        assert_eq!(
            end_local_session(Some(&host), "stdio-b"),
            "Unlinked. That chat links again on its next request."
        );
        assert_eq!(*host.0.lock().unwrap(), ["stdio-a", "http-c"]);
        assert_eq!(
            end_local_session(Some(&host), "stdio-b"),
            "That session had already ended"
        );
        assert_eq!(*host.0.lock().unwrap(), ["stdio-a", "http-c"]);
        assert_eq!(
            end_local_session(None, "stdio-a"),
            "No background MCP host runs in this process"
        );
        assert_eq!(
            end_local_session(Some(&OldHost), "stdio-a"),
            "This build's background host cannot end a session"
        );
    }

    #[test]
    fn the_app_is_looked_up_only_beside_the_broker() {
        let owner = AppScratch::new();
        let directory = &owner.0;
        #[cfg(windows)]
        let app_name = "qc-remote.exe";
        #[cfg(not(windows))]
        let app_name = "qc-remote";
        let broker = directory.join("qc-device-broker.exe");
        assert_eq!(app_beside(&broker), None);
        std::fs::write(directory.join(app_name), b"").unwrap();
        assert_eq!(app_beside(&broker), Some(directory.join(app_name)));
    }

    struct AppScratch(std::path::PathBuf);
    impl AppScratch {
        fn new() -> Self {
            for _ in 0..64 {
                let path = std::env::temp_dir().join(format!(
                    "qc-broker-open-app-{:032x}",
                    rand::random::<u128>()
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("reserve app path fixture: {error}"),
                }
            }
            panic!("could not reserve app path fixture after 64 collisions");
        }
    }
    impl Drop for AppScratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn app_path_fixture_root_is_removed_on_assertion_unwind() {
        let mut root = None;
        let fault = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let owner = AppScratch::new();
            root = Some(owner.0.clone());
            std::fs::write(owner.0.join("partial"), b"partial setup").unwrap();
            panic!("injected fixture assertion failure");
        }));
        assert!(fault.is_err());
        assert!(!root.unwrap().exists());
    }
}
