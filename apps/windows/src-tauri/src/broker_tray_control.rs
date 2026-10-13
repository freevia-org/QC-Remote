//! App control of the session's resident host, shared without initializing
//! Tauri or opening a device.
//!
//! Since 0.1.178 the resident host is `qc-device-broker.exe` itself: the
//! device broker, its icon and the background MCP host in one process.
//! `trayEnabled` is the one switch for all three: on, the app starts the
//! resident host now (`--resident`) and at login (the platform's startup
//! registration); off, the running process drops its icon and its host and
//! falls back to the shipped five-second grace on its own.
//!
//! The Run value replaces 0.1.176/0.1.177's `QC Remote Tray`, which started a
//! separate `qc-remote-tray.exe`. Every enable rewrites the new value and
//! removes the old one, and every disable removes both, so the migration is
//! idempotent and completes on the first app start after any upgrade, even one
//! whose installer was interrupted.
use crate::broker_tray_settings::{
    broker_tray_settings_in, parse_idle_minutes, set_idle_minutes_in, set_tray_enabled_in,
    set_updater_relay_enabled_in, updater_relay_enabled_in,
};
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(target_os = "linux")]
#[path = "broker_tray_control_linux.rs"]
mod linux;
#[cfg(target_os = "macos")]
#[path = "broker_tray_control_macos.rs"]
mod macos;
#[cfg(target_os = "linux")]
pub(crate) use linux::spawn_owned;
#[cfg(target_os = "macos")]
pub(crate) use macos::spawn_owned;

const CONTROL_LOCK: &str = "device-broker-tray-control.lock";
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "QC Remote";
/// The value 0.1.176/0.1.177 wrote for the separate `qc-remote-tray.exe`.
#[cfg(windows)]
const LEGACY_RUN_VALUE: &str = "QC Remote Tray";
/// The last argument of a `--resident` start that is not the user's own action
/// (the broker's `cli::KEEP_USER_QUIT_FLAG`): it never clears a recorded Quit
/// and does not serve while one stands.
#[cfg_attr(not(windows), allow(dead_code))]
const KEEP_USER_QUIT_FLAG: &str = "--keep-user-quit";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrokerTrayStatus {
    pub tray_enabled: bool,
    pub idle_disconnect_minutes: Option<u32>,
    pub tray_running: bool,
    pub tray_stopping: bool,
    /// "Internet for the Quad Cortex": the broker carries the unit's own update
    /// requests to Neural DSP. On unless the settings file says otherwise.
    pub updater_relay_enabled: bool,
    pub last_error: Option<String>,
}

/// The production boundary is replaceable in tests; tests never touch HKCU or
/// launch a real broker. There is deliberately no process-kill operation.
pub trait TrayEffects: Send + Sync {
    fn is_running(&self) -> bool;
    fn locate(&self) -> Result<PathBuf, String>;
    /// `supersede_user_quit` false starts `--resident ... --keep-user-quit`: it
    /// never clears a recorded Quit and does not serve while one stands.
    /// `apply` always passes false: the Settings switch clears at its own one
    /// point before any spawn, and the startup reconcile never clears.
    fn spawn(
        &self,
        executable: &Path,
        directory: &Path,
        supersede_user_quit: bool,
    ) -> Result<(), String>;
    fn set_run_entry(&self, executable: &Path, directory: &Path) -> Result<(), String>;
    fn remove_run_entry(&self) -> Result<(), String>;
}

pub struct NativeTrayEffects {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    directory: PathBuf,
}

pub struct BrokerTrayControl<E = NativeTrayEffects> {
    directory: PathBuf,
    effects: E,
    last_error: Mutex<Option<String>>,
}

impl BrokerTrayControl<NativeTrayEffects> {
    pub fn new(directory: PathBuf) -> Self {
        let effects = NativeTrayEffects {
            #[cfg(target_os = "linux")]
            directory: directory.clone(),
            #[cfg(target_os = "macos")]
            directory: directory.clone(),
        };
        Self {
            directory,
            effects,
            last_error: Mutex::new(None),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn quit_broker(&self) -> Result<(), String> {
        macos::quit(&self.directory)
    }
}

impl<E: TrayEffects> BrokerTrayControl<E> {
    /// A pure status read: never reconciles, writes a file, or starts a process.
    pub fn status(&self) -> BrokerTrayStatus {
        let settings = broker_tray_settings_in(&self.directory);
        let running = self.effects.is_running();
        BrokerTrayStatus {
            tray_enabled: settings.tray_enabled,
            idle_disconnect_minutes: settings.idle_disconnect_minutes,
            tray_running: running,
            tray_stopping: running && !settings.tray_enabled,
            updater_relay_enabled: updater_relay_enabled_in(&self.directory),
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<BrokerTrayStatus, String> {
        self.apply(Some(enabled))
    }

    /// Called only by GUI startup. Headless MCP must never call this method.
    pub fn reconcile(&self) -> Result<BrokerTrayStatus, String> {
        self.apply(None)
    }

    pub fn set_idle_minutes(&self, minutes: &Value) -> Result<BrokerTrayStatus, String> {
        let minutes = parse_idle_minutes(minutes)?;
        set_idle_minutes_in(&self.directory, minutes)?;
        // An idle-period edit cannot repair an earlier spawn/autostart failure.
        Ok(self.status())
    }

    /// Only the settings file changes: the running broker re-reads it, and
    /// nothing is launched, stopped or registered, so no tray effect runs.
    pub fn set_updater_relay_enabled(&self, enabled: bool) -> Result<BrokerTrayStatus, String> {
        set_updater_relay_enabled_in(&self.directory, enabled)?;
        Ok(self.status())
    }

    fn apply(&self, enabled: Option<bool>) -> Result<BrokerTrayStatus, String> {
        // Separate from the short settings-file lock: retain this lock across
        // every effect so another app window or the tray's Turn off action
        // cannot interleave Run-entry changes with this settings transition.
        let _lock = match control_lock(&self.directory) {
            Ok(lock) => lock,
            Err(error) => return self.remember_result(Err(error)),
        };
        let result = (|| {
            let settings = match enabled {
                Some(enabled) => set_tray_enabled_in(&self.directory, enabled)?,
                None => broker_tray_settings_in(&self.directory),
            };
            if settings.tray_enabled {
                // The user's own switch turned on (`set_enabled(true)`) is an
                // explicit background start: its one clear point for a
                // recorded Quit, under the election lock, whether or not a
                // spawn follows (a quitting resident may still hold its
                // marker). The startup reconcile never clears.
                if enabled == Some(true) {
                    crate::acknowledge_user_connect_in(
                        &crate::device_broker_descriptor::device_broker_descriptor_path(
                            &self.directory,
                        ),
                    )?;
                }
                let executable = self.effects.locate()?;
                if !self.effects.is_running() {
                    // Always `--keep-user-quit`: the switch cleared above, at
                    // its one point; the resident never clears again, and a
                    // Quit recorded since stands.
                    self.effects.spawn(&executable, &self.directory, false)?;
                }
                self.effects.set_run_entry(&executable, &self.directory)
            } else {
                // The running broker reads the switch within a second, drops
                // its icon and host, and reaps itself after the grace once
                // nobody is attached. Removing autostart never kills it.
                self.effects.remove_run_entry()
            }
        })();
        self.remember_result(result)
    }

    /// Report an error of the background host's that no settings call made,
    /// such as an app launch that could not clear a recorded Quit, where the
    /// Settings window shows `last_error`. The next settings call replaces it.
    pub fn report_error(&self, error: String) {
        *self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
    }

    fn remember_result(&self, result: Result<(), String>) -> Result<BrokerTrayStatus, String> {
        *self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = result.as_ref().err().cloned();
        result.map(|()| self.status())
    }
}

fn control_lock(directory: &Path) -> Result<File, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("Create tray control directory: {error}"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(CONTROL_LOCK))
        .map_err(|error| format!("Open tray control lock: {error}"))?;
    file.lock()
        .map_err(|error| format!("Lock tray control: {error}"))?;
    // This stable lock file is intentionally never removed.
    Ok(file)
}

/// The resident host is the broker sidecar beside the app.
pub fn locate_resident_host(executable_directory: &Path) -> Option<PathBuf> {
    [
        "qc-device-broker.exe",
        "qc-device-broker-x86_64-pc-windows-msvc.exe",
        "qc-device-broker-x86_64-pc-windows-gnu.exe",
    ]
    .into_iter()
    .map(|name| executable_directory.join(name))
    .find(|candidate| candidate.is_file())
}

#[cfg(any(windows, test))]
fn quote_windows_argument(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in value.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        quoted.push_str(&"\\".repeat(if character == '"' {
            backslashes * 2 + 1
        } else {
            backslashes
        }));
        quoted.push(character);
        backslashes = 0;
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(any(windows, test))]
fn run_command(executable: &Path, directory: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or("Broker executable path is not valid Unicode")?;
    let directory = directory
        .to_str()
        .ok_or("Tray settings directory is not valid Unicode")?;
    Ok(format!(
        "{} --resident --directory {}",
        quote_windows_argument(executable),
        quote_windows_argument(directory)
    ))
}

impl TrayEffects for NativeTrayEffects {
    fn is_running(&self) -> bool {
        #[cfg(windows)]
        {
            crate::tray_native::is_running()
        }
        #[cfg(target_os = "linux")]
        {
            linux::is_running(&self.directory)
        }
        #[cfg(target_os = "macos")]
        {
            macos::is_running(&self.directory)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            false
        }
    }

    fn locate(&self) -> Result<PathBuf, String> {
        #[cfg(windows)]
        {
            let executable = std::env::current_exe()
                .map_err(|error| format!("Locate the running QC Remote executable: {error}"))?;
            executable
                .parent()
                .and_then(locate_resident_host)
                .ok_or_else(|| {
                    "The qc-device-broker sidecar is missing beside QC Remote".to_owned()
                })
        }
        #[cfg(target_os = "linux")]
        {
            linux::locate()
        }
        #[cfg(target_os = "macos")]
        {
            macos::locate()
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            Err(
                "The QC Remote menu bar or tray is supported on Windows, Linux, and macOS"
                    .to_owned(),
            )
        }
    }

    fn spawn(
        &self,
        executable: &Path,
        directory: &Path,
        supersede_user_quit: bool,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Dropping the Windows Child closes our process handle; the
            // resident host is independent and must outlive this window.
            let mut command = std::process::Command::new(executable);
            command.arg("--resident").arg("--directory").arg(directory);
            if !supersede_user_quit {
                command.arg(KEEP_USER_QUIT_FLAG);
            }
            command
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null());
            // Into the same bounded broker-stderr.log the app's own
            // `--broker-serve` children write, so a start that exits during
            // startup leaves its reason behind.
            let mut child = crate::spawn_with_broker_stderr(&mut command)
                .map_err(|error| format!("Start the QC Remote background host: {error}"))?;
            // Keep the controller's lock until the host is observable, so two
            // GUI startups cannot both decide to start it. A start that finds
            // a broker already serving exits 0 at once: that broker reads the
            // switch and becomes the resident host itself within a second.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut exited_cleanly = false;
            loop {
                if self.is_running() {
                    return Ok(());
                }
                if !exited_cleanly {
                    if let Some(status) = child.try_wait().map_err(|error| {
                        format!("Check QC Remote background host startup: {error}")
                    })? {
                        if !status.success() {
                            return Err(format!(
                                "The QC Remote background host exited during startup ({status})"
                            ));
                        }
                        exited_cleanly = true;
                    }
                }
                // A clean exit means a broker already serves this directory.
                // Without a desktop (or with its icon suppressed) that broker
                // never becomes the resident host, so the marker never
                // appears; it is still the session's broker, and starting
                // another would be wrong. Success, not a 3 s timeout.
                if exited_cleanly && serving_broker_published(directory) {
                    return Ok(());
                }
                // A `--keep-user-quit` start that found a recorded Quit standing
                // declines (exit 0) and serves nowhere: say so, not a timeout.
                if exited_cleanly
                    && !supersede_user_quit
                    && crate::device_broker_descriptor::read_user_quit(
                        &crate::device_broker_descriptor::device_broker_descriptor_path(directory),
                    )
                    .is_some()
                {
                    return Err(
                        "The QC Remote background host was not started: QC Remote was quit, \
                         and only Connect or reopening QC Remote starts it again"
                            .to_owned(),
                    );
                }
                if std::time::Instant::now() >= deadline {
                    return Err(
                        "The QC Remote background host did not become ready within 3 seconds"
                            .to_owned(),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        #[cfg(target_os = "linux")]
        {
            linux::spawn(executable, directory, supersede_user_quit)
        }
        #[cfg(target_os = "macos")]
        {
            macos::spawn(executable, directory, supersede_user_quit)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (executable, directory, supersede_user_quit);
            Err(
                "The QC Remote menu bar or tray is supported on Windows, Linux, and macOS"
                    .to_owned(),
            )
        }
    }

    fn set_run_entry(&self, executable: &Path, directory: &Path) -> Result<(), String> {
        #[cfg(windows)]
        {
            let command = run_command(executable, directory)?;
            let key = windows_registry::CURRENT_USER
                .create(RUN_KEY)
                .map_err(|error| format!("Enable tray startup at Windows sign-in: {error}"))?;
            key.set_string(RUN_VALUE, command)
                .map_err(|error| format!("Enable tray startup at Windows sign-in: {error}"))?;
            // Only once the new value is in place: a failure here leaves the
            // old one too, which the next reconcile removes.
            if key
                .get_string(LEGACY_RUN_VALUE)
                .is_ok_and(|legacy| names_legacy_tray(&legacy))
            {
                remove_run_value(LEGACY_RUN_VALUE)?;
            }
            Ok(())
        }
        #[cfg(target_os = "linux")]
        {
            linux::set_run_entry(executable, directory)
        }
        #[cfg(target_os = "macos")]
        {
            macos::set_run_entry(executable, directory)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (executable, directory);
            Err(
                "The QC Remote menu bar or tray is supported on Windows, Linux, and macOS"
                    .to_owned(),
            )
        }
    }

    fn remove_run_entry(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            remove_run_value(RUN_VALUE)?;
            let legacy = windows_registry::CURRENT_USER
                .open(RUN_KEY)
                .and_then(|key| key.get_string(LEGACY_RUN_VALUE));
            if legacy.is_ok_and(|legacy| names_legacy_tray(&legacy)) {
                remove_run_value(LEGACY_RUN_VALUE)?;
            }
            Ok(())
        }
        #[cfg(target_os = "linux")]
        {
            linux::remove_run_entry()
        }
        #[cfg(target_os = "macos")]
        {
            macos::remove_run_entry()
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        Ok(())
    }
}

/// A fresh descriptor naming a live broker is published in `directory`.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn serving_broker_published(directory: &Path) -> bool {
    crate::device_broker_descriptor::read_descriptor(
        &crate::device_broker_descriptor::device_broker_descriptor_path(directory),
        crate::device_broker_descriptor::unix_time_ms(),
    )
    .is_some_and(|descriptor| {
        // Alive, and still the process that published (a recycled pid has a
        // different creation time). A descriptor without one is not enough.
        qc_native_platform::process::os_process_is_alive(descriptor.process_id)
            && descriptor.process_created_at.is_some()
            && qc_native_platform::process::os_process_created_at(descriptor.process_id)
                == descriptor.process_created_at
    })
}

#[cfg(windows)]
fn remove_run_value(name: &str) -> Result<(), String> {
    let result = windows_registry::CURRENT_USER
        .options()
        .write()
        .open(RUN_KEY)
        .and_then(|key| key.remove_value(name));
    match result {
        Ok(()) => Ok(()),
        // Missing key/value already means autostart is off. Do not create the
        // Run key merely to remove an absent value.
        Err(error) if matches!(error.code().0 as u32, 0x8007_0002 | 0x8007_0003) => Ok(()),
        Err(error) => Err(format!("Disable tray startup at Windows sign-in: {error}")),
    }
}

/// Whether an old `QC Remote Tray` value is one of ours: it starts some
/// installation's `qc-remote-tray.exe`, which 0.1.178 no longer ships. Any
/// other command under that name is left alone.
#[cfg(any(windows, test))]
fn names_legacy_tray(command: &str) -> bool {
    command
        .to_ascii_lowercase()
        .contains("\\qc-remote-tray.exe\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Condvar};
    use std::time::{Duration, Instant};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "qc-tray-control-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct FakeState {
        events: Mutex<Vec<&'static str>>,
        fail: Mutex<Option<&'static str>>,
        running: AtomicBool,
        pause_spawn: AtomicBool,
        spawn_started: AtomicBool,
        release_spawn: (Mutex<bool>, Condvar),
        /// Each spawn's `supersede_user_quit`, in order.
        supersedes: Mutex<Vec<bool>>,
    }
    struct FakeEffects {
        state: Arc<FakeState>,
        directory: PathBuf,
    }
    impl FakeEffects {
        fn event(&self, event: &'static str) -> Result<(), String> {
            self.state.events.lock().unwrap().push(event);
            if *self.state.fail.lock().unwrap() == Some(event) {
                Err(format!("fake {event} failure"))
            } else {
                Ok(())
            }
        }
    }
    impl TrayEffects for FakeEffects {
        fn is_running(&self) -> bool {
            self.state.running.load(Ordering::SeqCst)
        }
        fn locate(&self) -> Result<PathBuf, String> {
            self.event("locate")?;
            Ok(self.directory.join("qc-device-broker.exe"))
        }
        fn spawn(
            &self,
            _executable: &Path,
            directory: &Path,
            supersede_user_quit: bool,
        ) -> Result<(), String> {
            assert_eq!(directory, self.directory);
            self.state
                .supersedes
                .lock()
                .unwrap()
                .push(supersede_user_quit);
            assert!(broker_tray_settings_in(directory).tray_enabled);
            self.event("spawn")?;
            self.state.spawn_started.store(true, Ordering::SeqCst);
            if self.state.pause_spawn.load(Ordering::SeqCst) {
                let (lock, condition) = &self.state.release_spawn;
                let released = lock.lock().unwrap();
                let (_guard, timeout) = condition
                    .wait_timeout_while(released, Duration::from_secs(5), |released| !*released)
                    .unwrap();
                assert!(!timeout.timed_out(), "test did not release tray spawn");
            }
            self.state.running.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn set_run_entry(&self, _executable: &Path, directory: &Path) -> Result<(), String> {
            assert!(broker_tray_settings_in(directory).tray_enabled);
            self.event("run-add")
        }
        fn remove_run_entry(&self) -> Result<(), String> {
            assert!(!broker_tray_settings_in(&self.directory).tray_enabled);
            self.event("run-remove")
        }
    }
    fn controller(directory: &Path, state: &Arc<FakeState>) -> BrokerTrayControl<FakeEffects> {
        BrokerTrayControl {
            directory: directory.to_owned(),
            effects: FakeEffects {
                state: Arc::clone(state),
                directory: directory.to_owned(),
            },
            last_error: Mutex::new(None),
        }
    }

    #[test]
    fn enable_writes_before_synchronous_spawn_and_run_entry_disable_reports_stopping() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        set_tray_enabled_in(&dir.0, false).unwrap();
        let enabled = control.set_enabled(true).unwrap();
        assert_eq!(
            *state.events.lock().unwrap(),
            ["locate", "spawn", "run-add"]
        );
        assert!(enabled.tray_enabled && enabled.tray_running && !enabled.tray_stopping);
        let disabled = control.set_enabled(false).unwrap();
        assert_eq!(
            *state.events.lock().unwrap(),
            ["locate", "spawn", "run-add", "run-remove"]
        );
        assert!(!disabled.tray_enabled && disabled.tray_running && disabled.tray_stopping);
        state.running.store(false, Ordering::SeqCst);
        assert!(!control.status().tray_stopping);
    }

    #[test]
    fn status_is_read_only_and_has_exact_wire_fields() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        assert_eq!(
            serde_json::to_value(control.status()).unwrap(),
            json!({
                "trayEnabled": true, "idleDisconnectMinutes": 15,
                "trayRunning": false, "trayStopping": false,
                "updaterRelayEnabled": true, "lastError": null
            })
        );
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
        assert!(state.events.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_reconciles_default_on_and_saved_off_without_rewriting_preferences() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        assert!(control.reconcile().unwrap().tray_running);
        assert!(!dir.0.join("device-broker-tray.json").exists());
        assert_eq!(
            *state.events.lock().unwrap(),
            ["locate", "spawn", "run-add"]
        );
        state.events.lock().unwrap().clear();
        control.reconcile().unwrap();
        assert_eq!(
            *state.events.lock().unwrap(),
            ["locate", "run-add"],
            "an already-running tray must not receive a menu-opening second launch"
        );
        set_tray_enabled_in(&dir.0, false).unwrap();
        let before = fs::read(dir.0.join("device-broker-tray.json")).unwrap();
        state.events.lock().unwrap().clear();
        assert!(control.reconcile().unwrap().tray_stopping);
        assert_eq!(*state.events.lock().unwrap(), ["run-remove"]);
        assert_eq!(
            fs::read(dir.0.join("device-broker-tray.json")).unwrap(),
            before
        );
    }

    /// Only the user's own Settings switch starts a resident that supersedes a
    /// recorded Quit; the startup reconcile's start keeps it
    /// (`--keep-user-quit`). An error no settings call made (an app launch
    /// that could not clear the Quit) is shown until the next settings call.
    #[test]
    fn only_the_users_switch_supersedes_a_recorded_quit() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        control.reconcile().unwrap();
        state.running.store(false, Ordering::SeqCst);
        control.set_enabled(true).unwrap();
        // Every spawn keeps a recorded Quit; the switch clears at
        // its own point instead.
        assert_eq!(*state.supersedes.lock().unwrap(), [false, false]);
        control.report_error("Could not clear the recorded Quit".into());
        assert_eq!(
            control.status().last_error.as_deref(),
            Some("Could not clear the recorded Quit")
        );
    }

    /// The Settings switch turned on clears a recorded Quit itself,
    /// once, even when no spawn follows (a quitting resident may still hold
    /// its marker), and the resident it does start keeps any later Quit. The
    /// startup reconcile never clears, running or not.
    #[test]
    fn the_switch_clears_a_recorded_quit_once_even_when_the_host_still_runs() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        let descriptor = crate::device_broker_descriptor::device_broker_descriptor_path(&dir.0);
        let record = crate::device_broker_descriptor::user_quit_path(&descriptor);
        let quit = || fs::write(&record, br#"{"instanceId":"quit","source":"tray"}"#).unwrap();

        quit();
        state.running.store(true, Ordering::SeqCst);
        control.reconcile().unwrap();
        assert!(record.exists(), "the startup reconcile never clears");
        state.running.store(false, Ordering::SeqCst);
        control.reconcile().unwrap();
        assert!(record.exists(), "nor does its spawn");

        state.running.store(true, Ordering::SeqCst);
        state.supersedes.lock().unwrap().clear();
        control.set_enabled(true).unwrap();
        assert!(
            !record.exists(),
            "the switch cleared it with the host still running"
        );
        assert!(
            state.supersedes.lock().unwrap().is_empty(),
            "no spawn was needed"
        );

        quit();
        state.running.store(false, Ordering::SeqCst);
        control.set_enabled(true).unwrap();
        assert!(!record.exists());
        assert_eq!(
            *state.supersedes.lock().unwrap(),
            [false],
            "the resident it starts keeps a later Quit: no second clear"
        );
        assert!(
            !crate::device_broker_descriptor::device_broker_descriptor_path(&dir.0)
                .with_extension("election")
                .exists(),
            "the clear released the election lock"
        );
    }

    #[test]
    fn effect_failures_remain_visible_without_undoing_the_requested_setting() {
        for failure in ["locate", "spawn", "run-add", "run-remove"] {
            let dir = Scratch::new();
            let state = Arc::new(FakeState::default());
            *state.fail.lock().unwrap() = Some(failure);
            let control = controller(&dir.0, &state);
            let enabled = failure != "run-remove";
            let error = control.set_enabled(enabled).unwrap_err();
            assert_eq!(control.status().last_error, Some(error.clone()));
            assert_eq!(control.status().tray_enabled, enabled);
            assert_eq!(state.events.lock().unwrap().last(), Some(&failure));
            control.set_idle_minutes(&json!(30)).unwrap();
            assert_eq!(control.status().last_error, Some(error));
            *state.fail.lock().unwrap() = None;
            assert_eq!(control.reconcile().unwrap().last_error, None);
        }
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        *state.fail.lock().unwrap() = Some("locate");
        let control = controller(&dir.0, &state);
        assert!(control.reconcile().is_err());
        assert_eq!(
            control.status().last_error.as_deref(),
            Some("fake locate failure")
        );
    }

    #[test]
    fn spike_step_12_the_settings_rows_refuse_a_bad_period_and_change_nothing() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        set_tray_enabled_in(&dir.0, false).unwrap();
        let before = fs::read(dir.0.join("device-broker-tray.json")).unwrap();
        let effects_before = state.events.lock().unwrap().clone();
        for value in [
            json!(1441),
            json!(-1),
            json!(1.5),
            json!("15"),
            json!(true),
            json!({}),
            json!([]),
        ] {
            assert!(control
                .set_idle_minutes(&value)
                .unwrap_err()
                .contains("0 through 1440"));
            assert_eq!(
                fs::read(dir.0.join("device-broker-tray.json")).unwrap(),
                before
            );
            assert_eq!(control.status().idle_disconnect_minutes, Some(15));
            assert_eq!(*state.events.lock().unwrap(), effects_before);
        }
        for minutes in [0, 1, 1440] {
            let applied = control.set_idle_minutes(&json!(minutes)).unwrap();
            assert_eq!(applied.idle_disconnect_minutes, Some(minutes));
            assert_eq!(control.status().idle_disconnect_minutes, Some(minutes));
            assert!(!applied.tray_enabled);
            assert_eq!(*state.events.lock().unwrap(), effects_before);
        }
        assert_eq!(
            control
                .set_idle_minutes(&Value::Null)
                .unwrap()
                .idle_disconnect_minutes,
            None
        );
        assert!(state.events.lock().unwrap().is_empty());
    }

    #[test]
    fn the_update_relay_switch_defaults_on_and_touches_no_tray_effect() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        assert!(control.status().updater_relay_enabled, "a fresh install");
        set_tray_enabled_in(&dir.0, false).unwrap();
        assert!(
            control.status().updater_relay_enabled,
            "a file that predates the key"
        );
        let off = control.set_updater_relay_enabled(false).unwrap();
        assert!(!off.updater_relay_enabled && !off.tray_enabled);
        assert!(!control.status().updater_relay_enabled);
        let on = control.set_updater_relay_enabled(true).unwrap();
        assert!(on.updater_relay_enabled);
        assert_eq!(on.idle_disconnect_minutes, Some(15));
        assert!(
            state.events.lock().unwrap().is_empty(),
            "the relay switch must not launch, stop or register anything"
        );
    }

    #[test]
    fn write_failure_never_launches_or_changes_autostart() {
        let dir = Scratch::new();
        fs::create_dir(dir.0.join("device-broker-tray.json")).unwrap();
        let state = Arc::new(FakeState::default());
        let control = controller(&dir.0, &state);
        assert!(control.set_enabled(true).is_err());
        assert!(control.status().last_error.is_some());
        assert!(state.events.lock().unwrap().is_empty());
    }

    #[test]
    fn separate_controllers_serialize_preferences_and_effects_together() {
        let dir = Scratch::new();
        let state = Arc::new(FakeState::default());
        state.pause_spawn.store(true, Ordering::SeqCst);
        let first = controller(&dir.0, &state);
        let second = controller(&dir.0, &state);
        std::thread::scope(|scope| {
            let enable = scope.spawn(|| first.set_enabled(true).unwrap());
            let deadline = Instant::now() + Duration::from_secs(5);
            while !state.spawn_started.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let disable = scope.spawn(|| second.set_enabled(false).unwrap());
            std::thread::sleep(Duration::from_millis(50));
            assert!(broker_tray_settings_in(&dir.0).tray_enabled);
            assert_eq!(*state.events.lock().unwrap(), ["locate", "spawn"]);
            let (lock, condition) = &state.release_spawn;
            *lock.lock().unwrap() = true;
            condition.notify_all();
            enable.join().unwrap();
            assert!(disable.join().unwrap().tray_stopping);
        });
        assert_eq!(
            *state.events.lock().unwrap(),
            ["locate", "spawn", "run-add", "run-remove"]
        );
        assert!(!first.status().tray_enabled);
    }

    #[test]
    fn sibling_lookup_and_run_command_handle_packaged_names_and_spaced_paths() {
        let dir = Scratch::new();
        assert_eq!(locate_resident_host(&dir.0), None);
        // A stale tray from 0.1.176/0.1.177 is never the resident host.
        fs::write(dir.0.join("qc-remote-tray.exe"), b"old").unwrap();
        assert_eq!(locate_resident_host(&dir.0), None);
        for name in [
            "qc-device-broker-x86_64-pc-windows-gnu.exe",
            "qc-device-broker-x86_64-pc-windows-msvc.exe",
            "qc-device-broker.exe",
        ] {
            fs::write(dir.0.join(name), b"fake").unwrap();
            assert_eq!(locate_resident_host(&dir.0), Some(dir.0.join(name)));
        }
        assert_eq!(run_command(Path::new(r"C:\Program Files\QC Remote\qc-device-broker.exe"), Path::new("C:\\Users\\A B\\data\\")).unwrap(), "\"C:\\Program Files\\QC Remote\\qc-device-broker.exe\" --resident --directory \"C:\\Users\\A B\\data\\\\\"");
        assert_eq!(quote_windows_argument("with\"quote"), "\"with\\\"quote\"");
    }

    #[test]
    fn only_an_old_tray_command_counts_as_the_legacy_run_value() {
        assert!(names_legacy_tray(
            "\"C:\\Users\\a\\AppData\\Local\\QC Remote\\qc-remote-tray.exe\" --directory \"C:\\d\""
        ));
        assert!(names_legacy_tray("\"D:\\Apps\\QC-REMOTE-TRAY.EXE\""));
        assert!(!names_legacy_tray(
            "\"C:\\QC Remote\\qc-device-broker.exe\" --resident --directory \"C:\\d\""
        ));
        assert!(!names_legacy_tray("\"C:\\Other\\not-qc-remote-tray.exe\""));
        assert!(!names_legacy_tray(""));
    }
}
