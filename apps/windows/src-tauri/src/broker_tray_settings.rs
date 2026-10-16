//! Shared app/tray preferences. This module deliberately has no Tauri dependency.
use serde::Serialize;
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const SETTINGS_FILE: &str = "device-broker-tray.json";
const LOCK_FILE: &str = "device-broker-tray.lock";
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

// Bound cooperative lock admission, not already-started filesystem operations.
// Keep the same stable sidefile and retained handle through the transaction.
pub(crate) fn lock_with_budget(file: &fs::File, label: &str) -> Result<(), String> {
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(format!("Another {label} update is still in progress; no settings were changed. Retry after that writer finishes"));
                }
                std::thread::sleep(LOCK_RETRY.min(remaining));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("Lock {label}: {error}"));
            }
        }
    }
}
// Same software document envelope as the broker's preferences reader. This
// is not a firmware/schema field limit; unknown fields are preserved within it.
const MAX_SETTINGS_BYTES: u64 = 128 * 1024;
fn read_settings_bytes(dir: &Path) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(dir.join(SETTINGS_FILE))?;
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(std::io::Error::other(
            "Tray settings exceeded the 128 KiB software document envelope",
        ));
    }
    Ok(bytes)
}
const IDLE_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 0..=1440;
const IDLE_MINUTES_ERROR: &str =
    "idleDisconnectMinutes must be an integer from 0 through 1440, or null for Never";
#[cfg(test)]
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
    read_settings_bytes(dir)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

// Display fallback does not authorize replacing an unreadable existing document.
fn read_document_for_update(dir: &Path) -> Result<Map<String, Value>, String> {
    read_document_for_update_with(|| read_settings_bytes(dir))
}

fn read_document_for_update_with(
    read: impl FnOnce() -> std::io::Result<Vec<u8>>,
) -> Result<Map<String, Value>, String> {
    let bytes = match read() {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("Read existing tray settings: {error}")),
    };
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| format!("Parse existing tray settings: {error}"))?
        .as_object()
        .cloned()
        .ok_or_else(|| "Existing tray settings must be a JSON object".to_owned())
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
    lock_with_budget(&lock, "tray settings")?;

    let mut document = read_document_for_update(dir)?;
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
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err("Updated tray settings exceed the 128 KiB software document envelope; original bytes were preserved".into());
    }
    atomic_write(dir, &bytes)?;
    SETTINGS_REVISION.fetch_add(1, Ordering::Release);
    Ok(settings)
}

fn atomic_write(dir: &Path, bytes: &[u8]) -> Result<(), String> {
    // The shared publisher reserves a bounded exclusive owner-private stage,
    // retains cleanup on failed write/sync/rename/unwind, and never truncates
    // the live preferences. The stable settings writer lease remains held.
    qc_owner_only_file::publish_atomically(&dir.join(SETTINGS_FILE), bytes)
        .map_err(|error| format!("Write tray settings: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct Scratch(PathBuf, Arc<ScratchRoot>);
    struct ScratchRoot(PathBuf);
    impl Drop for ScratchRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    static CHILD_COUNT: AtomicU64 = AtomicU64::new(0);
    static RETAINED_CHILDREN: std::sync::Mutex<Vec<SettingsChild>> =
        std::sync::Mutex::new(Vec::new());
    struct ChildPermit;
    impl Drop for ChildPermit {
        fn drop(&mut self) {
            CHILD_COUNT.fetch_sub(1, Ordering::AcqRel);
        }
    }
    struct SettingsChild {
        child: Option<std::process::Child>,
        root: Option<Arc<ScratchRoot>>,
        permit: Option<ChildPermit>,
        exited: Option<std::process::ExitStatus>,
    }
    impl SettingsChild {
        fn reap() {
            let pending = std::mem::take(
                &mut *RETAINED_CHILDREN
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            let mut still_pending = Vec::new();
            for mut original in pending {
                if !matches!(original.try_wait(), Ok(Some(_))) {
                    still_pending.push(original);
                }
            }
            RETAINED_CHILDREN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(still_pending);
        }
        #[allow(
            deprecated,
            reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
        )]
        fn spawn(command: &mut Command, root: Arc<ScratchRoot>) -> std::io::Result<Self> {
            Self::reap();
            CHILD_COUNT
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    (count < 4).then_some(count + 1)
                })
                .map_err(|_| {
                    std::io::Error::other("Four original settings fixtures are still retiring")
                })?;
            let permit = ChildPermit;
            let child = command.spawn()?;
            Ok(Self {
                child: Some(child),
                root: Some(root),
                permit: Some(permit),
                exited: None,
            })
        }
        fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
            if let Some(status) = self.exited {
                return Ok(Some(status));
            }
            let status = self.child.as_mut().expect("owned original").try_wait()?;
            if let Some(status) = status {
                self.exited = Some(status);
                self.child.take();
                self.permit.take();
                self.root.take();
            }
            Ok(status)
        }
        fn stop_within(&mut self, within: Duration) -> bool {
            if matches!(self.try_wait(), Ok(Some(_))) {
                return true;
            }
            let _ = self.child.as_mut().expect("owned original").kill();
            let until = Instant::now() + within;
            loop {
                if matches!(self.try_wait(), Ok(Some(_))) {
                    return true;
                }
                if Instant::now() >= until {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for SettingsChild {
        fn drop(&mut self) {
            if self.child.is_none() || self.stop_within(Duration::from_secs(2)) {
                return;
            }
            let original = Self {
                child: self.child.take(),
                root: self.root.take(),
                permit: self.permit.take(),
                exited: self.exited,
            };
            RETAINED_CHILDREN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(original);
            let _ = std::panic::catch_unwind(|| {
                eprintln!("Settings fixture process retirement unknown; exact child, root and admission retained")
            });
        }
    }

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
            Self(dir.clone(), Arc::new(ScratchRoot(dir)))
        }

        fn file(&self) -> PathBuf {
            self.0.join(SETTINGS_FILE)
        }
    }

    #[test]
    fn oversized_existing_preferences_are_bounded_and_all_mutations_preserve_the_document() {
        let dir = Scratch::new();
        const ENVELOPE: u64 = 128 * 1024;
        let mut bytes = br#"{"trayEnabled":false,"future":""#.to_vec();
        bytes.resize((ENVELOPE + 1) as usize - 2, b'x');
        bytes.extend_from_slice(br#""}"#);
        fs::write(dir.file(), &bytes).unwrap();
        assert!(
            matches!(read_document_for_update(&dir.0), Err(error) if error.contains("128 KiB")),
            "valid oversized input must refuse before JSON mutation"
        );
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings::default()
        );
        assert!(set_tray_enabled_in(&dir.0, false).is_err());
        assert!(set_idle_minutes_in(&dir.0, Some(30)).is_err());
        assert!(set_updater_relay_enabled_in(&dir.0, false).is_err());
        assert_eq!(fs::read(dir.file()).unwrap(), bytes);
        let file = fs::File::create(dir.file()).unwrap();
        file.set_len(ENVELOPE + 1).unwrap();
        drop(file);
        assert!(
            matches!(read_document_for_update(&dir.0), Err(error) if error.contains("128 KiB")),
            "sparse malformed input is bounded before parse"
        );
        assert!(set_tray_enabled_in(&dir.0, false).is_err());
        assert_eq!(fs::metadata(dir.file()).unwrap().len(), ENVELOPE + 1);
        assert!(!fs::read_dir(&dir.0).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn exact_envelope_preferences_keep_unknown_fields_through_a_healthy_mutation() {
        let dir = Scratch::new();
        const ENVELOPE: usize = 128 * 1024;
        let base = br#"{"trayEnabled":false,"future":{"private":"retained"}}"#;
        let mut bytes = base.to_vec();
        bytes.resize(ENVELOPE, b' ');
        fs::write(dir.file(), &bytes).unwrap();
        assert_eq!(
            read_document_for_update(&dir.0).unwrap()["future"],
            json!({"private":"retained"})
        );
        assert!(!broker_tray_settings_in(&dir.0).tray_enabled);
        set_idle_minutes_in(&dir.0, Some(30)).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(dir.file()).unwrap()).unwrap();
        assert_eq!(saved["future"], json!({"private":"retained"}));
        assert_eq!(saved["trayEnabled"], false);
        assert_eq!(saved["idleDisconnectMinutes"], 30);
    }

    #[test]
    fn pretty_serialization_cannot_publish_a_document_that_its_bounded_reader_will_refuse() {
        let dir = Scratch::new();
        const ENVELOPE: u64 = 128 * 1024;
        let mut document = Map::new();
        for index in 0..10_000 {
            document.insert(format!("k{index}"), Value::Null);
        }
        let bytes = serde_json::to_vec(&document).unwrap();
        assert!(bytes.len() as u64 <= ENVELOPE);
        assert!(serde_json::to_vec_pretty(&document).unwrap().len() as u64 > ENVELOPE);
        fs::write(dir.file(), &bytes).unwrap();
        assert!(set_tray_enabled_in(&dir.0, false)
            .unwrap_err()
            .contains("original bytes were preserved"));
        assert_eq!(fs::read(dir.file()).unwrap(), bytes);
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
    fn actual_failed_preferences_rename_preserves_destination_and_retires_owned_stage() {
        let dir = Scratch::new();
        fs::create_dir(dir.file()).unwrap();
        let sentinel = dir.file().join("sentinel");
        fs::write(&sentinel, b"independent destination").unwrap();
        assert!(atomic_write(&dir.0, br#"{"trayEnabled":false}"#).is_err());
        assert_eq!(fs::read(sentinel).unwrap(), b"independent destination");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
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
    fn all_preference_mutations_refuse_damaged_existing_documents_without_replacing_bytes() {
        let dir = Scratch::new();
        for bytes in [b"{broken".as_slice(), b"[]", b"null", b"false", b"42"] {
            fs::write(dir.file(), bytes).unwrap();
            assert!(set_tray_enabled_in(&dir.0, false).is_err());
            assert_eq!(fs::read(dir.file()).unwrap(), bytes);
            assert!(set_idle_minutes_in(&dir.0, Some(30)).is_err());
            assert_eq!(fs::read(dir.file()).unwrap(), bytes);
            assert!(set_updater_relay_enabled_in(&dir.0, true).is_err());
            assert_eq!(fs::read(dir.file()).unwrap(), bytes);
            assert!(!fs::read_dir(&dir.0).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        }
    }

    #[test]
    fn mutation_reader_defaults_only_for_missing_not_unreadable_documents() {
        assert!(read_document_for_update_with(|| Err(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )))
        .is_err());
        assert_eq!(
            read_document_for_update_with(|| Err(std::io::Error::from(
                std::io::ErrorKind::NotFound
            )))
            .unwrap(),
            Map::new()
        );
        let dir = Scratch::new();
        fs::create_dir(dir.file()).unwrap();
        assert!(set_tray_enabled_in(&dir.0, false).is_err());
        assert!(dir.file().is_dir());
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
        run_document_readers(&dir, None, Arc::new(AtomicU64::new(0)));
        assert!(!fs::read_dir(&dir.0).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    // Armed inside the scope before its first fallible spawn. On unwind each
    // original reader observes channel disconnection before scope joins it.
    struct ReaderStartGate(Vec<std::sync::mpsc::Sender<()>>);
    impl ReaderStartGate {
        fn release(mut self) {
            for start in self.0.drain(..) {
                start
                    .send(())
                    .expect("owned reader must still await admission");
            }
        }
    }
    impl Drop for ReaderStartGate {
        fn drop(&mut self) {
            self.0.clear();
        }
    }
    fn run_document_readers(dir: &Scratch, reject_spawn: Option<usize>, cancelled: Arc<AtomicU64>) {
        std::thread::scope(|scope| {
            let mut start = ReaderStartGate(Vec::new());
            for index in 0..4 {
                let (admit, admitted) = std::sync::mpsc::channel();
                start.0.push(admit);
                let path = dir.file();
                let cancelled = cancelled.clone();
                let spawn = if reject_spawn == Some(index) {
                    Err(std::io::Error::other("injected rejected reader spawn"))
                } else {
                    std::thread::Builder::new().spawn_scoped(scope, move || {
                    if admitted.recv_timeout(Duration::from_secs(5)).is_err() {
                        cancelled.fetch_add(1, Ordering::Release);
                        return;
                    }
                    for _ in 0..500 {
                        let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        assert_eq!(raw["trayEnabled"], true);
                        assert!([json!(1), json!(1440)].contains(&raw["idleDisconnectMinutes"]));
                    }
                    })
                };
                spawn.expect("reader thread spawn failed");
            }
            start.release();
            for i in 0..100 {
                set_idle_minutes_in(&dir.0, Some(if i % 2 == 0 { 1440 } else { 1 })).unwrap();
            }
        });
    }

    #[test]
    fn rejected_second_reader_spawn_unwinds_and_acknowledges_the_first_original_reader() {
        let dir = Scratch::new();
        set_idle_minutes_in(&dir.0, Some(1)).unwrap();
        let before = fs::read(dir.file()).unwrap();
        let cancelled = Arc::new(AtomicU64::new(0));
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_document_readers(&dir, Some(1), cancelled.clone());
        }));
        assert!(
            failed.is_err(),
            "rejected second spawn must remain a failure"
        );
        assert_eq!(
            cancelled.load(Ordering::Acquire),
            1,
            "the first original reader exited before scope returned"
        );
        assert_eq!(fs::read(dir.file()).unwrap(), before);
    }

    #[test]
    #[ignore = "subprocess helper; invoked by concurrent_process_setters_preserve_both_updates"]
    fn settings_child_writer() {
        let dir = PathBuf::from(std::env::var_os("QC_TRAY_SETTINGS_TEST_DIR").unwrap());
        let key = std::env::var("QC_TRAY_SETTINGS_TEST_KEY").unwrap();
        fs::write(dir.join(format!("ready-{key}")), b"ready").unwrap();
        if key == "held" {
            for (name, result) in [
                ("tray", set_tray_enabled_in(&dir, false).map(|_| ())),
                ("idle", set_idle_minutes_in(&dir, Some(1440)).map(|_| ())),
                (
                    "relay",
                    set_updater_relay_enabled_in(&dir, false).map(|_| ()),
                ),
            ] {
                let error = result.expect_err("held settings lease must refuse the update");
                assert!(error.contains("still in progress"), "{error}");
                fs::write(dir.join(format!("refused-{name}")), error).unwrap();
            }
            return;
        }
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
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "broker_tray_settings::tests::settings_child_writer",
                        "--ignored",
                    ])
                    .env("QC_TRAY_SETTINGS_TEST_DIR", &dir.0)
                    .env("QC_TRAY_SETTINGS_TEST_KEY", key);
                SettingsChild::spawn(&mut command, Arc::clone(&dir.1)).unwrap()
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
                    assert!(
                        child.stop_within(Duration::from_secs(2)),
                        "original writer remains retained if termination is unknown"
                    );
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
    fn a_held_settings_lease_refuses_all_writers_without_mutation_and_releases_exactly() {
        let dir = Scratch::new();
        set_idle_minutes_in(&dir.0, Some(15)).unwrap();
        let before = fs::read(dir.file()).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.0.join(LOCK_FILE))
            .unwrap();
        lock.lock().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "broker_tray_settings::tests::settings_child_writer",
                "--ignored",
            ])
            .env("QC_TRAY_SETTINGS_TEST_DIR", &dir.0)
            .env("QC_TRAY_SETTINGS_TEST_KEY", "held");
        let mut child = SettingsChild::spawn(&mut command, Arc::clone(&dir.1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "held-lease writer failed: {status}");
                break;
            }
            if Instant::now() >= deadline {
                assert!(
                    child.stop_within(Duration::from_secs(2)),
                    "unknown original remains retained"
                );
                panic!("settings writer did not refuse a held lease within its observation budget");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for name in ["tray", "idle", "relay"] {
            assert!(dir.0.join(format!("refused-{name}")).is_file());
        }
        assert_eq!(fs::read(dir.file()).unwrap(), before);
        let still_blocked = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.0.join(LOCK_FILE))
            .unwrap();
        assert!(
            matches!(
                still_blocked.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ),
            "the original handle still owns its lease"
        );
        drop(still_blocked);
        drop(lock);
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.0.join(LOCK_FILE))
            .unwrap();
        contender.try_lock().unwrap();
        drop(contender);
        set_tray_enabled_in(&dir.0, false).unwrap();
        set_idle_minutes_in(&dir.0, Some(1440)).unwrap();
        assert_eq!(
            broker_tray_settings_in(&dir.0),
            BrokerTraySettings {
                tray_enabled: false,
                idle_disconnect_minutes: Some(1440)
            }
        );
        assert!(
            dir.0.join(LOCK_FILE).is_file(),
            "stable lease inode is never removed"
        );
    }

    #[test]
    fn a_settings_writer_assertion_unwind_stops_its_exact_process_before_scratch_retirement() {
        let mut path = None;
        #[cfg(windows)]
        let mut held_original = None;
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let dir = Scratch::new();
            path = Some(dir.0.clone());
            set_idle_minutes_in(&dir.0, Some(15)).unwrap();
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.0.join(LOCK_FILE))
                .unwrap();
            lock.lock().unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "broker_tray_settings::tests::settings_child_writer",
                    "--ignored",
                ])
                .env("QC_TRAY_SETTINGS_TEST_DIR", &dir.0)
                .env("QC_TRAY_SETTINGS_TEST_KEY", "tray");
            let mut child = SettingsChild::spawn(&mut command, Arc::clone(&dir.1)).unwrap();
            #[cfg(windows)]
            {
                use std::os::windows::io::AsHandle;
                held_original = Some(
                    child
                        .child
                        .as_ref()
                        .unwrap()
                        .as_handle()
                        .try_clone_to_owned()
                        .unwrap(),
                );
            }
            let until = Instant::now() + Duration::from_secs(5);
            while !dir.0.join("ready-tray").exists() {
                assert!(
                    Instant::now() < until,
                    "actual child reached its locked writer"
                );
                assert!(child.try_wait().unwrap().is_none());
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("injected assertion before the second writer exists");
        }));
        assert!(failed.is_err());
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            assert_eq!(
                unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(
                        held_original.as_ref().unwrap().as_raw_handle() as _,
                        0,
                    )
                },
                windows_sys::Win32::Foundation::WAIT_OBJECT_0
            );
        }
        assert!(
            !path.unwrap().exists(),
            "root deletion follows actual process acknowledgement"
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
