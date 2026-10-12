//! Owner-only transient UI observations, never credentials or device payloads.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
const FILE_NAME: &str = "tray-host-status.json";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    process_id: u32,
    process_created_at: u64,
    published_at_unix_ms: u64,
    status: Value,
}
pub(crate) fn publish(directory: &Path, status: Value) -> Result<(), String> {
    let pid = std::process::id();
    let created = qc_native_platform::process::os_process_created_at(pid)
        .ok_or("Host process identity is unavailable")?;
    let record = Record {
        process_id: pid,
        process_created_at: created,
        published_at_unix_ms: crate::broker_paths::unix_time_ms(),
        status,
    };
    qc_owner_only_file::publish_atomically(
        &directory.join(FILE_NAME),
        &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
    )
}
fn valid(
    record: &Record,
    now: u64,
    alive: impl FnOnce(u32) -> bool,
    created: impl FnOnce(u32) -> Option<u64>,
) -> bool {
    record.process_id != 0
        && record.process_created_at != 0
        && record.published_at_unix_ms <= now
        && now - record.published_at_unix_ms < 3_000
        && alive(record.process_id)
        && created(record.process_id) == Some(record.process_created_at)
}
pub(crate) fn read(directory: &Path) -> Option<Value> {
    let path = directory.join(FILE_NAME);
    if !qc_owner_only_file::is_owner_only(&path).ok()? {
        return None;
    }
    if std::fs::metadata(&path).ok()?.len() > 128 * 1024 {
        return None;
    }
    let record: Record = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    valid(
        &record,
        crate::broker_paths::unix_time_ms(),
        qc_native_platform::process::os_process_is_alive,
        qc_native_platform::process::os_process_created_at,
    )
    .then_some(record.status)
}
fn remove_owned(directory: &Path, pid: u32, created: u64) {
    let path = directory.join(FILE_NAME);
    let owned = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        .is_some_and(|record| record.process_id == pid && record.process_created_at == created);
    if owned {
        let _ = std::fs::remove_file(path);
    }
}
pub(crate) fn remove(directory: &Path) {
    if let Some(created) = qc_native_platform::process::os_process_created_at(std::process::id()) {
        remove_owned(directory, std::process::id(), created);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_recycled_unknown_and_future_processes_are_not_live_status() {
        let record = Record {
            process_id: 7,
            process_created_at: 99,
            published_at_unix_ms: 1000,
            status: serde_json::json!({"remoteState":"connected"}),
        };
        assert!(valid(&record, 2000, |_| true, |_| Some(99)));
        assert!(!valid(&record, 4000, |_| true, |_| Some(99)));
        assert!(!valid(&record, 2000, |_| true, |_| Some(100)));
        assert!(!valid(&record, 2000, |_| true, |_| None));
        assert!(!valid(&record, 999, |_| true, |_| Some(99)));
        assert!(!valid(&record, 2000, |_| false, |_| Some(99)));
    }
    #[test]
    fn cleanup_preserves_another_owner_and_removes_its_own_record() {
        let directory =
            std::env::temp_dir().join(format!("qc-runtime-status-{:032x}", rand::random::<u128>()));
        // On a failing run the checked removal at the end never runs, so this
        // removes the directory; on a passing run that removal goes first and
        // this finds the directory already gone.
        let _cleanup = crate::tests::RemoveDirOnDrop::new(directory.clone());
        std::fs::create_dir_all(&directory).unwrap();
        let record = Record {
            process_id: 7,
            process_created_at: 99,
            published_at_unix_ms: 1,
            status: serde_json::json!({}),
        };
        let path = directory.join(FILE_NAME);
        qc_owner_only_file::publish_atomically(&path, &serde_json::to_vec(&record).unwrap())
            .unwrap();
        remove_owned(&directory, 7, 100);
        assert!(path.exists());
        remove_owned(&directory, 8, 99);
        assert!(path.exists());
        remove_owned(&directory, 7, 99);
        assert!(!path.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
