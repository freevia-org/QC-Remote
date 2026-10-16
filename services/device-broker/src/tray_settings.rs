//! The broker tray preferences, read from beside the descriptor.
//!
//! The app's Settings window owns `trayEnabled`; this process only reads it. The
//! broker's own menu may write `idleDisconnectMinutes`, through the same stable
//! side-file lock and temp-plus-rename the app uses, so neither writer can tear
//! the document or erase the other's key.
//!
//! The same document carries the update relay's settings
//! (`updaterRelayEnabled`, `updaterRelayExtraDomains`,
//! `updaterRelayMaxDownloadBytes`); see [`UpdaterRelaySettings`]. The relay is
//! on unless `updaterRelayEnabled` is `false`. A Settings screen's switch for
//! it writes that key with [`set_updater_relay_enabled_in`] (or the app's copy
//! of the same lock-and-rename writer), and every other key in the file is left
//! as it was.
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

pub const SETTINGS_FILE: &str = "device-broker-tray.json";
const LOCK_FILE: &str = "device-broker-tray.lock";
const TRAY_ENABLED_KEY: &str = "trayEnabled";
const IDLE_MINUTES_KEY: &str = "idleDisconnectMinutes";
pub const IDLE_MINUTES_MAX: u32 = 1440;
const DEFAULT_IDLE_MINUTES: u32 = 15;
const IDLE_MINUTES_ERROR: &str =
    "idleDisconnectMinutes must be an integer from 0 through 1440, or null for Never";
const UPDATER_RELAY_ENABLED_KEY: &str = "updaterRelayEnabled";
const UPDATER_RELAY_DOMAINS_KEY: &str = "updaterRelayExtraDomains";
const UPDATER_RELAY_MAX_BYTES_KEY: &str = "updaterRelayMaxDownloadBytes";
/// The firmware-image size cap when the file names none: far above any CorOS
/// image, far below a disk-filling answer.
pub const UPDATER_RELAY_DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const UPDATER_RELAY_MIN_MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024;
const UPDATER_RELAY_MAX_MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const UPDATER_RELAY_MAX_EXTRA_DOMAINS: usize = 8;
#[cfg(test)]
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
const MAX_SETTINGS_BYTES: u64 = 128 * 1024;

fn read_settings(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Tray settings exceed 128 KiB",
        ));
    }
    Ok(bytes)
}

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
    read_settings(&directory.join(SETTINGS_FILE))
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

/// The update relay's settings. Absent, unreadable or malformed values mean the
/// defaults, key by key: the relay on, `neuraldsp.com` only, the default size cap.
/// The running relay reads the file with [`try_read_updater_relay_in`] instead,
/// so a file that exists but cannot be used keeps its last known value rather
/// than falling back to these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdaterRelaySettings {
    /// Whether the broker answers the unit's type-61 update requests. Only a
    /// JSON `false` turns it off.
    pub enabled: bool,
    /// Domains allowed in addition to `neuraldsp.com`, each with its
    /// subdomains: lower-case plain DNS names. An entry that is not a plain
    /// name is ignored.
    pub extra_domains: Vec<String>,
    /// The most body bytes accepted for a firmware image.
    pub max_download_bytes: u64,
}

impl Default for UpdaterRelaySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_domains: Vec::new(),
            max_download_bytes: UPDATER_RELAY_DEFAULT_MAX_DOWNLOAD_BYTES,
        }
    }
}

fn applied_updater_relay(document: &Map<String, Value>) -> UpdaterRelaySettings {
    let defaults = UpdaterRelaySettings::default();
    let mut extra_domains: Vec<String> = Vec::new();
    if let Some(entries) = document
        .get(UPDATER_RELAY_DOMAINS_KEY)
        .and_then(Value::as_array)
    {
        for entry in entries {
            let Some(host) = entry
                .as_str()
                .and_then(crate::updater_relay::policy::normalize_allowlist_domain)
            else {
                continue;
            };
            if extra_domains.len() < UPDATER_RELAY_MAX_EXTRA_DOMAINS
                && !extra_domains.contains(&host)
            {
                extra_domains.push(host);
            }
        }
    }
    UpdaterRelaySettings {
        enabled: document
            .get(UPDATER_RELAY_ENABLED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(defaults.enabled),
        extra_domains,
        max_download_bytes: document
            .get(UPDATER_RELAY_MAX_BYTES_KEY)
            .and_then(Value::as_u64)
            .filter(|bytes| {
                (UPDATER_RELAY_MIN_MAX_DOWNLOAD_BYTES..=UPDATER_RELAY_MAX_MAX_DOWNLOAD_BYTES)
                    .contains(bytes)
            })
            .unwrap_or(defaults.max_download_bytes),
    }
}

/// The update relay's settings, from beside the descriptor.
pub fn read_updater_relay_in(directory: &Path) -> UpdaterRelaySettings {
    applied_updater_relay(&read_document(directory))
}

/// Why the settings file could not be turned into settings at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsReadError {
    /// The file is there but could not be read: a sharing violation while the
    /// app replaces it, a scanner's lock, a permissions fault.
    Unreadable(std::io::ErrorKind),
    /// The file was read but is not a JSON object.
    Malformed,
}

/// Like [`read_updater_relay_in`], but a file that is there and cannot be used
/// is an error instead of the defaults. Only an absent file means the defaults
/// (on); a caller that already holds a value from an earlier read keeps it
/// across an error, so a read that fails at the wrong moment cannot turn a
/// switch the user cleared back on.
pub fn try_read_updater_relay_in(
    directory: &Path,
) -> Result<UpdaterRelaySettings, SettingsReadError> {
    let bytes = match read_settings(&directory.join(SETTINGS_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UpdaterRelaySettings::default());
        }
        Err(error) => return Err(SettingsReadError::Unreadable(error.kind())),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(document)) => Ok(applied_updater_relay(&document)),
        _ => Err(SettingsReadError::Malformed),
    }
}

/// Switch the update relay on or off, leaving every other key as it was. This
/// is the call a Settings screen's switch makes.
pub fn set_updater_relay_enabled_in(
    directory: &Path,
    enabled: bool,
) -> Result<UpdaterRelaySettings, String> {
    let document = update_key(directory, UPDATER_RELAY_ENABLED_KEY, Value::Bool(enabled))?;
    Ok(applied_updater_relay(&document))
}

/// Replace `idleDisconnectMinutes` only. Validation precedes any I/O, so a
/// rejected value leaves the file's bytes untouched.
pub fn set_idle_minutes_in(directory: &Path, minutes: Option<u32>) -> Result<TraySettings, String> {
    let value = minutes.map_or(Value::Null, Value::from);
    parse_idle_minutes(&value)?;
    update_key(directory, IDLE_MINUTES_KEY, value).map(|document| applied(&document))
}

/// Replace one key under the stable lock and write the whole document back,
/// so no other key is lost. A missing tray switch is written out as its
/// default, never as off.
fn update_key(directory: &Path, key: &str, value: Value) -> Result<Map<String, Value>, String> {
    update_key_with_reader(directory, key, value, read_settings)
}

fn update_key_with_reader(
    directory: &Path,
    key: &str,
    value: Value,
    read: impl FnOnce(&Path) -> std::io::Result<Vec<u8>>,
) -> Result<Map<String, Value>, String> {
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
    // Tolerant defaults belong to display readers, not a read/modify/write
    // transaction: losing a read must never erase the other runtime's keys.
    let mut document =
        match read(&directory.join(SETTINGS_FILE)) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(document)) => document,
                _ => return Err(
                    "Existing tray settings must be a valid JSON object; no settings were changed"
                        .into(),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(error) => {
                return Err(format!(
                    "Read existing tray settings; no settings were changed: {error}"
                ))
            }
        };
    let defaults = TraySettings::default();
    document
        .entry(TRAY_ENABLED_KEY)
        .or_insert(Value::Bool(defaults.tray_enabled));
    document.insert(key.to_owned(), value);
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("Serialize tray settings: {error}"))?;
    atomic_write(directory, &bytes)?;
    Ok(document)
}

fn atomic_write(directory: &Path, bytes: &[u8]) -> Result<(), String> {
    qc_owner_only_file::publish_atomically(&directory.join(SETTINGS_FILE), bytes)
        .map_err(|error| format!("Write tray settings: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn oversized_settings_are_bounded_and_never_overwritten_by_a_mutation() {
        let dir = Scratch::new();
        let path = dir.0.join(SETTINGS_FILE);
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_SETTINGS_BYTES + 1).unwrap();
        drop(file);
        assert_eq!(
            read_settings(&path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(try_read_updater_relay_in(&dir.0).is_err());
        assert_eq!(read_in(&dir.0), TraySettings::default());
        assert!(set_idle_minutes_in(&dir.0, Some(20)).is_err());
        assert!(set_updater_relay_enabled_in(&dir.0, true).is_err());
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_SETTINGS_BYTES + 1);
        assert!(fs::read_dir(&dir.0).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn failed_settings_publication_preserves_destination_and_cleans_only_its_stage() {
        let dir = Scratch::new();
        let destination = dir.0.join(SETTINGS_FILE);
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"preserve").unwrap();
        assert!(atomic_write(&dir.0, b"{}").is_err());
        assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            for _ in 0..64 {
                let directory = std::env::temp_dir().join(format!(
                    "qc-broker-tray-settings-{}-{}",
                    std::process::id(),
                    NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed) + 1_000_000
                ));
                match fs::create_dir(&directory) {
                    Ok(()) => return Self(directory),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("reserve tray settings fixture: {error}"),
                }
            }
            panic!("could not reserve tray settings fixture after 64 collisions");
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

    #[test]
    fn the_update_relay_is_on_unless_the_file_says_false() {
        let dir = Scratch::new();
        let defaults = UpdaterRelaySettings::default();
        assert!(defaults.enabled && defaults.extra_domains.is_empty());
        assert_eq!(read_updater_relay_in(&dir.0), defaults, "no file");
        for bytes in [&b"not json"[..], b"[]", b"{}", br#"{"trayEnabled":false}"#] {
            dir.write(bytes);
            assert_eq!(
                read_updater_relay_in(&dir.0),
                defaults,
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
        for not_a_bool in [
            json!("false"),
            json!(0),
            json!(null),
            json!([]),
            json!("off"),
        ] {
            dir.write(&serde_json::to_vec(&json!({"updaterRelayEnabled": not_a_bool})).unwrap());
            assert!(read_updater_relay_in(&dir.0).enabled, "{not_a_bool}");
        }
        dir.write(br#"{"updaterRelayEnabled": false}"#);
        assert!(!read_updater_relay_in(&dir.0).enabled);
        dir.write(br#"{"updaterRelayEnabled": true}"#);
        assert!(read_updater_relay_in(&dir.0).enabled);
    }

    #[test]
    fn only_an_absent_file_means_the_defaults_when_the_read_can_fail() {
        let dir = Scratch::new();
        assert_eq!(
            try_read_updater_relay_in(&dir.0),
            Ok(UpdaterRelaySettings::default()),
            "an absent file is the default, on"
        );
        dir.write(br#"{"updaterRelayEnabled": false}"#);
        assert!(!try_read_updater_relay_in(&dir.0).unwrap().enabled);
        dir.write(b"{}");
        assert!(try_read_updater_relay_in(&dir.0).unwrap().enabled);
        for bytes in [&b"not json"[..], b"[]", b"\"x\"", b"", b"{\"updaterRelay"] {
            dir.write(bytes);
            assert_eq!(
                try_read_updater_relay_in(&dir.0),
                Err(SettingsReadError::Malformed),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
        // A file that is there but cannot be read: here a directory in its place.
        fs::remove_file(dir.0.join(SETTINGS_FILE)).unwrap();
        fs::create_dir(dir.0.join(SETTINGS_FILE)).unwrap();
        assert!(matches!(
            try_read_updater_relay_in(&dir.0),
            Err(SettingsReadError::Unreadable(_))
        ));
        // Display readers still use defaults; writers instead refuse this input.
        assert!(read_updater_relay_in(&dir.0).enabled);
    }

    #[test]
    fn the_relay_keys_are_independent_of_the_tray_keys() {
        let dir = Scratch::new();
        dir.write(
            br#"{"trayEnabled": false, "idleDisconnectMinutes": 5, "updaterRelayEnabled": false}"#,
        );
        assert!(!read_updater_relay_in(&dir.0).enabled);
        assert_eq!(
            read_in(&dir.0),
            TraySettings {
                tray_enabled: false,
                idle: IdlePolicy::Minutes(5)
            }
        );
    }

    #[test]
    fn extra_domains_and_the_size_cap_fall_back_key_by_key() {
        let dir = Scratch::new();
        dir.write(
            br#"{"updaterRelayExtraDomains": ["Downloads.Example.NET", "*.bad.example", "127.0.0.1",
                 7, "downloads.example.net", "cdn.example.org"],
                 "updaterRelayMaxDownloadBytes": 104857600}"#,
        );
        let settings = read_updater_relay_in(&dir.0);
        assert_eq!(
            settings.extra_domains,
            ["downloads.example.net", "cdn.example.org"]
        );
        assert_eq!(settings.max_download_bytes, 104_857_600);
        for bad in [
            json!(0),
            json!(1023),
            json!(5_000_000_000_u64),
            json!("big"),
            json!(-1),
        ] {
            dir.write(&serde_json::to_vec(&json!({"updaterRelayMaxDownloadBytes": bad})).unwrap());
            assert_eq!(
                read_updater_relay_in(&dir.0).max_download_bytes,
                UPDATER_RELAY_DEFAULT_MAX_DOWNLOAD_BYTES,
                "{bad}"
            );
        }
        dir.write(br#"{"updaterRelayExtraDomains": "downloads.example.net"}"#);
        assert!(read_updater_relay_in(&dir.0).extra_domains.is_empty());
        let many: Vec<String> = (0..20).map(|n| format!("h{n}.example.net")).collect();
        dir.write(&serde_json::to_vec(&json!({"updaterRelayExtraDomains": many})).unwrap());
        assert_eq!(read_updater_relay_in(&dir.0).extra_domains.len(), 8);
    }

    #[test]
    fn switching_the_relay_writes_only_its_key_and_keeps_the_rest() {
        let dir = Scratch::new();
        dir.write(br#"{"trayEnabled": false, "idleDisconnectMinutes": 30, "other": 1}"#);
        let off = set_updater_relay_enabled_in(&dir.0, false).unwrap();
        assert!(!off.enabled);
        let document: Value =
            serde_json::from_slice(&fs::read(dir.0.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(
            document,
            json!({
                "trayEnabled": false, "idleDisconnectMinutes": 30, "other": 1,
                "updaterRelayEnabled": false
            })
        );
        assert!(set_updater_relay_enabled_in(&dir.0, true).unwrap().enabled);
        assert!(read_updater_relay_in(&dir.0).enabled);
        // The tray's own writer keeps the relay key.
        set_idle_minutes_in(&dir.0, Some(10)).unwrap();
        set_updater_relay_enabled_in(&dir.0, false).unwrap();
        set_idle_minutes_in(&dir.0, Some(20)).unwrap();
        assert!(!read_updater_relay_in(&dir.0).enabled);
        // A missing tray switch is written as its default, never as off.
        let fresh = Scratch::new();
        set_updater_relay_enabled_in(&fresh.0, false).unwrap();
        assert!(read_in(&fresh.0).tray_enabled);
    }

    #[test]
    fn settings_writers_refuse_corrupt_or_unreadable_existing_documents_without_staging() {
        let dir = Scratch::new();
        let assert_unchanged = |before: &[u8]| {
            assert_eq!(fs::read(dir.0.join(SETTINGS_FILE)).unwrap(), before);
            assert!(fs::read_dir(&dir.0).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        };
        for bytes in [
            &b"not json"[..],
            b"null",
            b"[]",
            b"\"text\"",
            b"{\"updaterRelayEnabled\":false",
        ] {
            dir.write(bytes);
            assert!(set_idle_minutes_in(&dir.0, Some(20))
                .unwrap_err()
                .contains("valid JSON object"));
            assert_unchanged(bytes);
            assert!(set_updater_relay_enabled_in(&dir.0, true)
                .unwrap_err()
                .contains("valid JSON object"));
            assert_unchanged(bytes);
        }
        let bytes = br#"{"trayEnabled":false,"updaterRelayEnabled":false,"other":42}"#;
        dir.write(bytes);
        for (key, value) in [
            (IDLE_MINUTES_KEY, Value::from(20)),
            (UPDATER_RELAY_ENABLED_KEY, Value::Bool(true)),
        ] {
            let error = update_key_with_reader(&dir.0, key, value, |_| {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            })
            .unwrap_err();
            assert!(error.contains("Read existing tray settings"));
            assert_unchanged(bytes);
        }
    }

    #[test]
    fn settings_scratch_root_is_removed_on_partial_setup_unwind() {
        let mut root = None;
        let fault = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let scratch = Scratch::new();
            root = Some(scratch.0.clone());
            scratch.write(b"partial fixture");
            panic!("injected setup assertion failure");
        }));
        assert!(fault.is_err());
        assert!(!root.unwrap().exists());
    }
}
