//! Persistence for the demo (virtual) device.
//!
//! The demo device keeps one JSON document: its setlists, stored presets and
//! the preset being edited. The frontend owns the document's shape
//! (`packages/typescript/qc-ui/src/virtual-gateway.ts`); this side owns only
//! where it lives. The path is fixed here, never taken from the webview, so
//! these two commands can read and replace exactly one file and nothing else.
//!
//! It sits beside every other per-user file the app writes, under
//! `%LOCALAPPDATA%\QC Remote\` (`runtime-health.json`, `integrations\`), and
//! matches `VIRTUAL_DEVICE_STORE_RELATIVE_PATH` on the TypeScript side.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Far above any real library (256 slots per setlist of a few KB each), and
/// low enough that a runaway write cannot fill the disk.
const MAXIMUM_STORE_BYTES: usize = 64 * 1024 * 1024;

pub fn virtual_device_store_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("QC Remote")
        .join("virtual-device")
        .join("store.json")
}

/// The stored document, or `None` when the demo device has never been saved.
pub fn read_store(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read the demo device: {error}")),
    }
}

/// Replace the stored document. It must be a version 1 demo-device document;
/// anything else is refused rather than written over the user's library. The
/// new text is written beside the old file and renamed over it, so a failed
/// write leaves the previous document intact.
pub fn write_store(path: &Path, text: &str) -> Result<(), String> {
    if text.len() > MAXIMUM_STORE_BYTES {
        return Err("The demo device document is too large to save.".into());
    }
    let document: Value = serde_json::from_str(text)
        .map_err(|error| format!("The demo device document is not valid JSON: {error}"))?;
    if document.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("The demo device document is not a version 1 virtual device document.".into());
    }
    let directory = path
        .parent()
        .ok_or_else(|| "The demo device store has no parent directory.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create the demo device folder: {error}"))?;
    let staging = path.with_extension("json.tmp");
    fs::write(&staging, text)
        .map_err(|error| format!("Could not save the demo device: {error}"))?;
    fs::rename(&staging, path).map_err(|error| {
        let _ = fs::remove_file(&staging);
        format!("Could not save the demo device: {error}")
    })
}

#[tauri::command]
pub fn read_virtual_device_store() -> Result<Option<String>, String> {
    read_store(&virtual_device_store_path())
}

#[tauri::command]
pub fn write_virtual_device_store(text: String) -> Result<(), String> {
    write_store(&virtual_device_store_path(), &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store path under a scratch root of this test's own, and the guard
    /// that removes that root when the test ends or unwinds.
    fn scratch(name: &str) -> (crate::tests::RemoveDirOnDrop, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "qc-remote-virtual-store-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let path = directory.join("virtual-device").join("store.json");
        (crate::tests::RemoveDirOnDrop::new(directory), path)
    }

    #[test]
    fn the_store_lives_under_the_apps_local_data_folder() {
        let path = virtual_device_store_path();
        assert!(path.ends_with(
            Path::new("QC Remote")
                .join("virtual-device")
                .join("store.json")
        ));
    }

    #[test]
    fn a_missing_store_reads_as_none_and_a_write_round_trips() {
        let (_scratch, path) = scratch("round-trip");
        assert_eq!(read_store(&path).expect("read"), None);
        let text = "{\n  \"version\": 1,\n  \"session\": \"s\"\n}\n";
        write_store(&path, text).expect("write");
        assert_eq!(read_store(&path).expect("read").as_deref(), Some(text));
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the staging file is renamed away"
        );
    }

    #[test]
    fn a_foreign_document_is_refused_and_leaves_the_library_intact() {
        let (_scratch, path) = scratch("refuse");
        let library = "{\"version\":1}";
        write_store(&path, library).expect("write");
        assert!(write_store(&path, "not json").is_err());
        assert!(write_store(&path, "{\"version\":2}").is_err());
        assert!(write_store(&path, "[1]").is_err());
        assert_eq!(read_store(&path).expect("read").as_deref(), Some(library));
    }
}
