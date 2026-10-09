//! The two broker tray preferences, read from beside the descriptor.
//!
//! The app's Settings window owns `trayEnabled`; this process only reads it. The
//! broker's own menu may write `idleDisconnectMinutes`, through the same stable
//! side-file lock and temp-plus-rename the app uses, so neither writer can tear
//! the document or erase the other's key.
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const SETTINGS_FILE: &str = "device-broker-tray.json";
const LOCK_FILE: &str = "device-broker-tray.lock";
const TRAY_ENABLED_KEY: &str = "trayEnabled";
const IDLE_MINUTES_KEY: &str = "idleDisconnectMinutes";
pub const IDLE_MINUTES_MAX: u32 = 1440;
const DEFAULT_IDLE_MINUTES: u32 = 15;
const IDLE_MINUTES_ERROR: &str =
    "idleDisconnectMinutes must be an integer from 0 through 1440, or null for Never";
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// What the broker does once no participant is attached, while its icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdlePolicy {
    /// Stored as `0`: the shipped five-second grace.
    Immediately,
    /// Stored as `1..=1440`.
    Minutes(u32),
    /// Stored as `null`: held until released from the icon.
    Never,
}

impl IdlePolicy {
    pub fn from_minutes(minutes: Option<u32>) -> Self {
        match minutes {
            None => Self::Never,
            Some(0) => Self::Immediately,
            Some(minutes) => Self::Minutes(minutes),
        }
    }

    pub fn minutes(self) -> Option<u32> {
        match self {
            Self::Never => None,
            Self::Immediately => Some(0),
            Self::Minutes(minutes) => Some(minutes),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraySettings {
    pub tray_enabled: bool,
    pub idle: IdlePolicy,
}

impl Default for TraySettings {
    /// Absent, unreadable or malformed means on, 15 minutes: never a silent
    /// opt-out of the icon, and never a silent five-second teardown.
    fn default() -> Self {
        Self {
            tray_enabled: true,
            idle: IdlePolicy::Minutes(DEFAULT_IDLE_MINUTES),
        }
    }
}

/// Zero means Immediately, null means Never, positive values are minutes.
/// Rejects rather than clamps, with one message naming both ends.
pub fn parse_idle_minutes(value: &Value) -> Result<Option<u32>, String> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .and_then(|minutes| u32::try_from(minutes).ok())
        .filter(|minutes| *minutes <= IDLE_MINUTES_MAX)
        .map(Some)
        .ok_or_else(|| IDLE_MINUTES_ERROR.to_owned())
}

fn read_document(directory: &Path) -> Map<String, Value> {
    fs::read(directory.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn applied(document: &Map<String, Value>) -> TraySettings {
    let defaults = TraySettings::default();
    TraySettings {
        tray_enabled: document
            .get(TRAY_ENABLED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(defaults.tray_enabled),
        idle: document
            .get(IDLE_MINUTES_KEY)
            .and_then(|value| parse_idle_minutes(value).ok())
            .map(IdlePolicy::from_minutes)
            .unwrap_or(defaults.idle),
    }
}

/// Each key falls back to its own default independently.
pub fn read_in(directory: &Path) -> TraySettings {
    applied(&read_document(directory))
}

/// Replace `idleDisconnectMinutes` only. Validation precedes any I/O, so a
/// rejected value leaves the file's bytes untouched.
pub fn set_idle_minutes_in(directory: &Path, minutes: Option<u32>) -> Result<TraySettings, String> {
    let value = minutes.map_or(Value::Null, Value::from);
    parse_idle_minutes(&value)?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Create tray settings directory: {error}"))?;
    // Lock the stable side-file, never the replaced document, and keep it after
    // unlock; this is the app's protocol, so the two processes serialize.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(LOCK_FILE))
        .map_err(|error| format!("Open tray settings lock: {error}"))?;
    lock.lock()
        .map_err(|error| format!("Lock tray settings: {error}"))?;
    let mut document = read_document(directory);
    let defaults = TraySettings::default();
    document
        .entry(TRAY_ENABLED_KEY)
        .or_insert(Value::Bool(defaults.tray_enabled));
    document.insert(IDLE_MINUTES_KEY.to_owned(), value);
    let settings = applied(&document);
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("Serialize tray settings: {error}"))?;
    atomic_write(directory, &bytes)?;
    Ok(settings)
}

fn atomic_write(directory: &Path, bytes: &[u8]) -> Result<(), String> {
    let (temp_path, mut temp) = loop {
        let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            ".{SETTINGS_FILE}.broker-{}.{sequence}.tmp",
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
        fs::rename(&temp_path, directory.join(SETTINGS_FILE))
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

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "qc-broker-tray-settings-{}-{}",
                std::process::id(),
                NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed) + 1_000_000
            ));
            let _ = fs::remove_dir_all(&directory);
            fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }
        fn write(&self, bytes: &[u8]) {
            fs::write(self.0.join(SETTINGS_FILE), bytes).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn absent_unreadable_and_malformed_files_mean_icon_on_and_fifteen_minutes() {
        let dir = Scratch::new();
        let expected = TraySettings {
            tray_enabled: true,
            idle: IdlePolicy::Minutes(15),
        };
        assert_eq!(read_in(&dir.0), expected);
        for bytes in [&b"not json"[..], b"[]", b"\"x\"", b"{}"] {
            dir.write(bytes);
            assert_eq!(
                read_in(&dir.0),
                expected,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn the_literal_keys_the_app_writes_are_the_ones_read() {
        let dir = Scratch::new();
        // Exactly the document the app's Settings writer produces.
        dir.write(br#"{"trayEnabled": false, "idleDisconnectMinutes": 30}"#);
        assert_eq!(
            read_in(&dir.0),
            TraySettings {
                tray_enabled: false,
                idle: IdlePolicy::Minutes(30)
            }
        );
        dir.write(br#"{"trayEnabled": true, "idleDisconnectMinutes": null}"#);
        assert_eq!(read_in(&dir.0).idle, IdlePolicy::Never);
        dir.write(br#"{"idleDisconnectMinutes": 0}"#);
        assert_eq!(read_in(&dir.0).idle, IdlePolicy::Immediately);
    }

    #[test]
    fn each_key_falls_back_on_its_own() {
        let dir = Scratch::new();
        dir.write(br#"{"trayEnabled": "false", "idleDisconnectMinutes": 45}"#);
        assert_eq!(
            read_in(&dir.0),
            TraySettings {
                tray_enabled: true,
                idle: IdlePolicy::Minutes(45)
            }
        );
        for bad in [
            json!(1441),
            json!(-1),
            json!(1.5),
            json!("15"),
            json!(true),
            json!({}),
        ] {
            dir.write(
                &serde_json::to_vec(&json!({"trayEnabled": false, "idleDisconnectMinutes": bad}))
                    .unwrap(),
            );
            assert_eq!(
                read_in(&dir.0),
                TraySettings {
                    tray_enabled: false,
                    idle: IdlePolicy::Minutes(15)
                }
            );
        }
    }

    #[test]
    fn idle_bounds_are_inclusive_and_rejection_names_both_ends() {
        assert_eq!(parse_idle_minutes(&json!(0)), Ok(Some(0)));
        assert_eq!(parse_idle_minutes(&json!(1440)), Ok(Some(1440)));
        assert_eq!(parse_idle_minutes(&Value::Null), Ok(None));
        let error = parse_idle_minutes(&json!(1441)).unwrap_err();
        assert!(error.contains("0 through 1440"), "{error}");
        assert_eq!(IdlePolicy::from_minutes(Some(0)), IdlePolicy::Immediately);
        assert_eq!(IdlePolicy::from_minutes(None), IdlePolicy::Never);
        assert_eq!(IdlePolicy::from_minutes(Some(7)).minutes(), Some(7));
    }

    #[test]
    fn the_broker_writes_only_the_idle_key_and_keeps_the_apps_switch() {
        let dir = Scratch::new();
        dir.write(br#"{"trayEnabled": false, "idleDisconnectMinutes": 30, "other": 1}"#);
        let applied = set_idle_minutes_in(&dir.0, None).unwrap();
        assert_eq!(applied.idle, IdlePolicy::Never);
        assert!(!applied.tray_enabled);
        let document: Value =
            serde_json::from_slice(&fs::read(dir.0.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(
            document,
            json!({"trayEnabled": false, "idleDisconnectMinutes": null, "other": 1})
        );
        // A missing switch is written out as its default, never as off.
        let fresh = Scratch::new();
        set_idle_minutes_in(&fresh.0, Some(60)).unwrap();
        let document: Value =
            serde_json::from_slice(&fs::read(fresh.0.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(
            document,
            json!({"trayEnabled": true, "idleDisconnectMinutes": 60})
        );
        assert!(fs::read_dir(&fresh.0).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn a_rejected_period_leaves_the_file_bytes_untouched() {
        let dir = Scratch::new();
        dir.write(br#"{"trayEnabled":true,"idleDisconnectMinutes":30}"#);
        let before = fs::read(dir.0.join(SETTINGS_FILE)).unwrap();
        assert!(set_idle_minutes_in(&dir.0, Some(1441))
            .unwrap_err()
            .contains("0 through 1440"));
        assert_eq!(fs::read(dir.0.join(SETTINGS_FILE)).unwrap(), before);
    }
}
