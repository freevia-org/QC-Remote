//! Shared app/tray preferences. This module deliberately has no Tauri dependency.
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const SETTINGS_FILE: &str = "device-broker-tray.json";
const LOCK_FILE: &str = "device-broker-tray.lock";
const IDLE_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 0..=1440;
const IDLE_MINUTES_ERROR: &str =
    "idleDisconnectMinutes must be an integer from 0 through 1440, or null for Never";
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
static SETTINGS_REVISION: AtomicU64 = AtomicU64::new(0);

/// In-process setters invalidate the host's short announcement-policy cache.
/// Other processes' writes are discovered by its bounded polling interval.
#[allow(dead_code)] // The tray reads settings directly; the host uses this revision.
pub(crate) fn settings_revision() -> u64 {
    SETTINGS_REVISION.load(Ordering::Acquire)
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrokerTraySettings {
    pub tray_enabled: bool,
    pub idle_disconnect_minutes: Option<u32>,
}

impl Default for BrokerTraySettings {
    fn default() -> Self {
        Self {
            tray_enabled: true,
            idle_disconnect_minutes: Some(15),
        }
    }
}

/// The file reader and command boundary share strict integer/range validation.
/// Zero means Immediately, null means Never, and positive values are minutes.
/// Missing fields are handled by the reader and retain the default 15 minutes.
pub fn parse_idle_minutes(value: &Value) -> Result<Option<u32>, String> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .and_then(|minutes| u32::try_from(minutes).ok())
        .filter(|minutes| IDLE_MINUTES_RANGE.contains(minutes))
        .map(Some)
        .ok_or_else(|| IDLE_MINUTES_ERROR.to_owned())
}

fn read_document(dir: &Path) -> Map<String, Value> {
    fs::read(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn applied_settings(document: &Map<String, Value>) -> BrokerTraySettings {
    let defaults = BrokerTraySettings::default();
    BrokerTraySettings {
        tray_enabled: document
            .get("trayEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(defaults.tray_enabled),
        idle_disconnect_minutes: document
            .get("idleDisconnectMinutes")
            .and_then(|value| parse_idle_minutes(value).ok())
            .unwrap_or(defaults.idle_disconnect_minutes),
    }
}

/// Read from the broker descriptor's resolved directory, including its fallback.
/// Absent, unreadable and malformed values use defaults independently per key.
pub fn broker_tray_settings_in(dir: &Path) -> BrokerTraySettings {
    applied_settings(&read_document(dir))
}

/// Only the app changes whether the tray runs.
pub fn set_tray_enabled_in(dir: &Path, enabled: bool) -> Result<BrokerTraySettings, String> {
    update_key(dir, "trayEnabled", Value::Bool(enabled))
}

/// Both the app and the tray may change the idle period. Validate before any I/O
/// so rejected values leave even the original file's bytes unchanged.
pub fn set_idle_minutes_in(dir: &Path, minutes: Option<u32>) -> Result<BrokerTraySettings, String> {
    let value = minutes.map_or(Value::Null, Value::from);
    parse_idle_minutes(&value)?;
    update_key(dir, "idleDisconnectMinutes", value)
}

/// The key the broker reads (`tray_settings::read_updater_relay_in`) to decide
/// whether it carries the Quad Cortex's own update requests to Neural DSP.
const UPDATER_RELAY_KEY: &str = "updaterRelayEnabled";

/// "Internet for the Quad Cortex": on unless the file says `false`, so a fresh
/// install and an install whose file predates the key both read as on. Any
/// other value, a missing or unreadable file included, means on.
pub fn updater_relay_enabled_in(dir: &Path) -> bool {
    read_document(dir)
        .get(UPDATER_RELAY_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Switch the broker's update relay. Written under the same lock and rename as
/// the tray keys, so neither of those is disturbed; the running broker notices
/// within about two seconds and ends a transfer that is in progress when it is
/// switched off. Returns the value now stored.
pub fn set_updater_relay_enabled_in(dir: &Path, enabled: bool) -> Result<bool, String> {
    update_key(dir, UPDATER_RELAY_KEY, Value::Bool(enabled))?;
    Ok(updater_relay_enabled_in(dir))
}

fn update_key(dir: &Path, key: &str, value: Value) -> Result<BrokerTraySettings, String> {
    fs::create_dir_all(dir).map_err(|error| format!("Create tray settings directory: {error}"))?;
    // Lock a stable sidefile, never the replaced document. Keep it after unlock:
    // removing it would let a third writer lock a new inode while another waits
    // on the old one. Dropping this handle releases the OS lock, including when
    // a writer crashes, and serializes separate app/tray processes too.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .map_err(|error| format!("Open tray settings lock: {error}"))?;
    lock.lock()
        .map_err(|error| format!("Lock tray settings: {error}"))?;

    let mut document = read_document(dir);
    let defaults = BrokerTraySettings::default();
    document
        .entry("trayEnabled")
        .or_insert(Value::Bool(defaults.tray_enabled));
    document.entry("idleDisconnectMinutes").or_insert_with(|| {
        defaults
            .idle_disconnect_minutes
            .map_or(Value::Null, Value::from)
    });
    document.insert(key.to_owned(), value);
    let settings = applied_settings(&document);
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("Serialize tray settings: {error}"))?;
    atomic_write(dir, &bytes)?;
    SETTINGS_REVISION.fetch_add(1, Ordering::Release);
    Ok(settings)
}

fn atomic_write(dir: &Path, bytes: &[u8]) -> Result<(), String> {
    // create_new also protects against leftovers from a previous PID lifetime.
    let (temp_path, mut temp) = loop {
        let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            ".{SETTINGS_FILE}.{}.{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Create tray settings temporary file: {error}")),
        }
    };
    let result = (|| {
        temp.write_all(bytes)?;
        temp.sync_all()?;
        drop(temp);
        fs::rename(&temp_path, dir.join(SETTINGS_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(|error| format!("Write tray settings: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "qc-tray-settings-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(SETTINGS_FILE)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn absent_unreadable_and_malformed_files_default_on() {
        let dir = Scratch::new();
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings::default()
        );
        fs::create_dir(dir.file()).unwrap();
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings::default()
        );
        fs::remove_dir(dir.file()).unwrap();
        for bytes in ["{not json", "[]", "null", "false", "42"] {
            fs::write(dir.file(), bytes).unwrap();
            assert_eq!(
                broker_tray_settings_in(&dir.0),
                BrokerTraySettings::default()
            );
        }
    }

    #[test]
    fn missing_and_invalid_keys_default_independently_but_only_null_disables_idle() {
        let dir = Scratch::new();
        for value in [json!({}), json!({"trayEnabled": false})] {
            fs::write(dir.file(), value.to_string()).unwrap();
            assert_eq!(
                broker_tray_settings_in(&dir.0).idle_disconnect_minutes,
                Some(15)
            );
        }
        for invalid in [
            json!(1441),
            json!(-1),
            json!(1.5),
            json!("15"),
            json!(true),
            json!([]),
            json!({}),
        ] {
            fs::write(
                dir.file(),
                json!({"trayEnabled": false, "idleDisconnectMinutes": invalid}).to_string(),
            )
            .unwrap();
            assert_eq!(
                broker_tray_settings_in(&dir.0),
                BrokerTraySettings {
                    tray_enabled: false,
                    idle_disconnect_minutes: Some(15)
                }
            );
        }
        fs::write(
            dir.file(),
            br#"{"trayEnabled":"false","idleDisconnectMinutes":null}"#,
        )
        .unwrap();
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings {
                tray_enabled: true,
                idle_disconnect_minutes: None
            }
        );
        set_idle_minutes_in(&dir.0, None).unwrap();
        assert_eq!(
            broker_tray_settings_in(&dir.0).idle_disconnect_minutes,
            None
        );
    }

    #[test]
    fn invalid_range_and_types_have_one_named_error_and_leave_bytes_unchanged() {
        let dir = Scratch::new();
        let original = b"{ \"trayEnabled\": false, \"idleDisconnectMinutes\": 30 }\n";
        fs::write(dir.file(), original).unwrap();
        for invalid in [Some(1441), Some(u32::MAX)] {
            assert_eq!(
                set_idle_minutes_in(&dir.0, invalid).unwrap_err(),
                IDLE_MINUTES_ERROR
            );
            assert_eq!(fs::read(dir.file()).unwrap(), original);
        }
        for invalid in [
            json!(-1),
            json!(1441),
            json!(u64::MAX),
            json!("15"),
            json!(1.0),
            json!(1.5),
            json!(true),
            json!([]),
            json!({}),
        ] {
            let result = parse_idle_minutes(&invalid)
                .and_then(|minutes| set_idle_minutes_in(&dir.0, minutes));
            assert_eq!(result.unwrap_err(), IDLE_MINUTES_ERROR);
            assert_eq!(fs::read(dir.file()).unwrap(), original);
        }
    }

    #[test]
    fn boundaries_null_and_literal_keys_round_trip() {
        let dir = Scratch::new();
        for minutes in [Some(0), Some(1), Some(1440), None] {
            assert_eq!(
                set_idle_minutes_in(&dir.0, minutes)
                    .unwrap()
                    .idle_disconnect_minutes,
                minutes
            );
            assert_eq!(
                broker_tray_settings_in(&dir.0).idle_disconnect_minutes,
                minutes
            );
            let raw: Value = serde_json::from_slice(&fs::read(dir.file()).unwrap()).unwrap();
            assert_eq!(
                raw,
                json!({"trayEnabled": true, "idleDisconnectMinutes": minutes})
            );
            assert_eq!(
                serde_json::to_value(broker_tray_settings_in(&dir.0)).unwrap(),
                raw
            );
        }
    }

    #[test]
    fn each_setter_preserves_the_other_key_and_unknown_fields() {
        let dir = Scratch::new();
        fs::write(
            dir.file(),
            br#"{"trayEnabled":true,"idleDisconnectMinutes":30,"future":{"keep":true}}"#,
        )
        .unwrap();
        set_tray_enabled_in(&dir.0, false).unwrap();
        assert_eq!(
            broker_tray_settings_in(&dir.0).idle_disconnect_minutes,
            Some(30)
        );
        set_idle_minutes_in(&dir.0, Some(60)).unwrap();
        assert!(!broker_tray_settings_in(&dir.0).tray_enabled);
        let raw: Value = serde_json::from_slice(&fs::read(dir.file()).unwrap()).unwrap();
        assert_eq!(raw["future"], json!({"keep": true}));
    }

    #[test]
    fn concurrent_readers_always_see_complete_documents() {
        let dir = Scratch::new();
        set_idle_minutes_in(&dir.0, Some(1)).unwrap();
        let start = Arc::new(Barrier::new(5));
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let start = Arc::clone(&start);
                let path = dir.file();
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..500 {
                        let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        assert_eq!(raw["trayEnabled"], true);
                        assert!([json!(1), json!(1440)].contains(&raw["idleDisconnectMinutes"]));
                    }
                });
            }
            start.wait();
            for i in 0..100 {
                set_idle_minutes_in(&dir.0, Some(if i % 2 == 0 { 1440 } else { 1 })).unwrap();
            }
        });
        assert!(!fs::read_dir(&dir.0).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    #[ignore = "subprocess helper; invoked by concurrent_process_setters_preserve_both_updates"]
    fn settings_child_writer() {
        let dir = PathBuf::from(std::env::var_os("QC_TRAY_SETTINGS_TEST_DIR").unwrap());
        let key = std::env::var("QC_TRAY_SETTINGS_TEST_KEY").unwrap();
        fs::write(dir.join(format!("ready-{key}")), b"ready").unwrap();
        if key == "tray" {
            set_tray_enabled_in(&dir, false).unwrap();
        } else {
            set_idle_minutes_in(&dir, Some(1440)).unwrap();
        }
    }

    #[test]
    fn concurrent_process_setters_preserve_both_updates() {
        let dir = Scratch::new();
        set_idle_minutes_in(&dir.0, Some(15)).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.0.join(LOCK_FILE))
            .unwrap();
        lock.lock().unwrap();
        let mut children: Vec<_> = ["tray", "idle"]
            .into_iter()
            .map(|key| {
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "broker_tray_settings::tests::settings_child_writer",
                        "--ignored",
                    ])
                    .env("QC_TRAY_SETTINGS_TEST_DIR", &dir.0)
                    .env("QC_TRAY_SETTINGS_TEST_KEY", key)
                    .spawn()
                    .unwrap()
            })
            .collect();
        assert_eq!(children.len(), 2, "both settings writers must be running");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !dir.0.join("ready-tray").exists() || !dir.0.join("ready-idle").exists() {
            assert!(Instant::now() < deadline, "child writers did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        for child in &mut children {
            assert!(
                child.try_wait().unwrap().is_none(),
                "writer bypassed the process lock"
            );
        }
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings::default()
        );
        drop(lock);
        for mut child in children {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("settings writer did not finish after unlocking");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings {
                tray_enabled: false,
                idle_disconnect_minutes: Some(1440)
            }
        );
    }

    #[test]
    fn the_update_relay_reads_on_unless_the_file_says_false() {
        let dir = Scratch::new();
        assert!(updater_relay_enabled_in(&dir.0), "no file");
        for bytes in ["{not json", "[]", "{}", r#"{"trayEnabled": false}"#] {
            fs::write(dir.file(), bytes).unwrap();
            assert!(updater_relay_enabled_in(&dir.0), "{bytes}");
        }
        for not_a_bool in [json!("false"), json!(0), json!(null), json!([])] {
            fs::write(
                dir.file(),
                json!({"updaterRelayEnabled": not_a_bool}).to_string(),
            )
            .unwrap();
            assert!(updater_relay_enabled_in(&dir.0), "{not_a_bool}");
        }
        fs::write(dir.file(), r#"{"updaterRelayEnabled": false}"#).unwrap();
        assert!(!updater_relay_enabled_in(&dir.0));
    }

    #[test]
    fn switching_the_update_relay_keeps_every_other_key_and_the_tray_defaults() {
        let dir = Scratch::new();
        fs::write(
            dir.file(),
            r#"{"trayEnabled": false, "idleDisconnectMinutes": 30, "updaterRelayExtraDomains": ["cdn.example.net"]}"#,
        )
        .unwrap();
        assert!(!set_updater_relay_enabled_in(&dir.0, false).unwrap());
        let document: Value = serde_json::from_slice(&fs::read(dir.file()).unwrap()).unwrap();
        assert_eq!(
            document,
            json!({
                "trayEnabled": false,
                "idleDisconnectMinutes": 30,
                "updaterRelayExtraDomains": ["cdn.example.net"],
                "updaterRelayEnabled": false
            })
        );
        assert!(set_updater_relay_enabled_in(&dir.0, true).unwrap());
        // The tray's own setters keep the relay key, and the relay's keeps the tray's.
        set_idle_minutes_in(&dir.0, Some(10)).unwrap();
        set_updater_relay_enabled_in(&dir.0, false).unwrap();
        set_tray_enabled_in(&dir.0, true).unwrap();
        assert!(!updater_relay_enabled_in(&dir.0));
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings {
                tray_enabled: true,
                idle_disconnect_minutes: Some(10)
            }
        );
        // A fresh file gets the tray defaults, never an off switch.
        let fresh = Scratch::new();
        set_updater_relay_enabled_in(&fresh.0, false).unwrap();
        assert!(broker_tray_settings_in(&fresh.0).tray_enabled);
    }
}
