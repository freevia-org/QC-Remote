//! Shared read-only installation diagnostics.
use std::{fs, path::Path};
/// QC Remote keeps the legacy `QC Control` AppData directory as a read fallback for the flight
/// recorder (see `flight.rs`). A machine that still has a `qc-device-broker.exe`
/// under the legacy directory has two independently launchable brokers on
/// disk - the same hazard as a stale broker left running: whichever one a
/// leftover shortcut or scheduled task starts can claim exclusive USB
/// ownership of the device out from under this app. Nothing here deletes or
/// touches either file; nothing the user has not put there themselves is
/// ever removed. It only makes the situation visible instead of silent.
#[derive(serde::Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateBrokerInstallation {
    pub path: String,
    pub modified_unix_ms: Option<u128>,
}

pub fn detect_duplicate_broker_installations(
    local_app_data: &Path,
) -> Vec<DuplicateBrokerInstallation> {
    ["QC Remote", "QC Control"]
        .into_iter()
        .map(|directory| local_app_data.join(directory).join("qc-device-broker.exe"))
        .filter(|path| path.is_file())
        .map(|path| {
            let modified_unix_ms = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis());
            DuplicateBrokerInstallation {
                path: path.to_string_lossy().into_owned(),
                modified_unix_ms,
            }
        })
        .collect()
}
