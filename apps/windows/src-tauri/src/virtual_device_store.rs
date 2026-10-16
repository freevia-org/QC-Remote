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
use std::io::{Read, Write};
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
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAXIMUM_STORE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("Could not read the demo device: {error}"))?;
            if bytes.len() > MAXIMUM_STORE_BYTES {
                return Err("The demo device document is too large to read.".into());
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| format!("Could not read the demo device: {error}"))
        }
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
    replace_store(path, text.as_bytes(), |file, bytes| file.write_all(bytes))
}

/// A writer owns only its exclusively-created stage, including partial-write
/// and rename failures. Concurrent invocations never truncate or remove it.
struct StoreStage {
    path: PathBuf,
    file: Option<fs::File>,
    owned: bool,
}

impl Drop for StoreStage {
    fn drop(&mut self) {
        self.file.take();
        if self.owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn replace_store(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or("The demo device store has no parent directory.")?;
    let mut stage = None;
    for _ in 0..8 {
        let staging = directory.join(format!(
            ".store-{}-{:032x}.tmp",
            std::process::id(),
            rand::random::<u128>()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(file) => {
                stage = Some(StoreStage {
                    path: staging,
                    file: Some(file),
                    owned: true,
                });
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not save the demo device: {error}")),
        }
    }
    let mut stage = stage.ok_or("Could not create a unique demo device staging file.")?;
    write(stage.file.as_mut().expect("owned stage"), bytes)
        .and_then(|()| stage.file.as_ref().expect("owned stage").sync_all())
        .map_err(|error| format!("Could not save the demo device: {error}"))?;
    // Windows cannot rename an open file. Close our handle before publication.
    stage.file.take();
    fs::rename(&stage.path, path)
        .map_err(|error| format!("Could not save the demo device: {error}"))?;
    stage.owned = false;
    Ok(())
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

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl Scratch {
        fn path(&self) -> PathBuf {
            self.0.join("virtual-device").join("store.json")
        }
    }

    fn scratch(name: &str) -> Scratch {
        let directory = std::env::temp_dir().join(format!(
            "qc-remote-virtual-store-{name}-{}-{:032x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        fs::create_dir(&directory).expect("unique scratch root");
        Scratch(directory)
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
        let scratch = scratch("round-trip");
        let path = scratch.path();
        assert_eq!(read_store(&path).expect("read"), None);
        let text = "{\n  \"version\": 1,\n  \"session\": \"s\"\n}\n";
        write_store(&path, text).expect("write");
        assert_eq!(read_store(&path).expect("read").as_deref(), Some(text));
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the staging file is renamed away"
        );
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_foreign_document_is_refused_and_leaves_the_library_intact() {
        let scratch = scratch("refuse");
        let path = scratch.path();
        let library = "{\"version\":1}";
        write_store(&path, library).expect("write");
        assert!(write_store(&path, "not json").is_err());
        assert!(write_store(&path, "{\"version\":2}").is_err());
        assert!(write_store(&path, "[1]").is_err());
        assert_eq!(read_store(&path).expect("read").as_deref(), Some(library));
    }

    #[test]
    fn failed_partial_write_and_failed_rename_remove_only_owned_staging() {
        let scratch = scratch("failures");
        let path = scratch.path();
        write_store(&path, "{\"version\":1}").unwrap();
        let foreign = path.with_extension("json.tmp");
        fs::write(&foreign, "another writer").unwrap();
        assert!(replace_store(&path, b"replacement", |file, _| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("injected write failure"))
        })
        .is_err());
        assert_eq!(
            read_store(&path).unwrap().as_deref(),
            Some("{\"version\":1}")
        );
        let blocked = path.parent().unwrap().join("directory-target");
        fs::create_dir(&blocked).unwrap();
        assert!(replace_store(&blocked, b"replacement", |file, bytes| file
            .write_all(bytes))
        .is_err());
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "another writer");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 3);
    }

    #[test]
    fn concurrent_writes_publish_whole_documents_without_staging_leaks() {
        let scratch = scratch("concurrent");
        let path = scratch.path();
        let start = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            for index in 0..8 {
                let path = &path;
                let start = &start;
                scope.spawn(move || {
                    let text = format!(
                        "{{\"version\":1,\"writer\":{index},\"padding\":\"{}\"}}",
                        "x".repeat(8192)
                    );
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !start.load(std::sync::atomic::Ordering::Acquire) {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "writer release timed out"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    write_store(path, &text).unwrap();
                });
            }
            start.store(true, std::sync::atomic::Ordering::Release);
        });
        let document: Value = serde_json::from_str(&read_store(&path).unwrap().unwrap()).unwrap();
        assert!(document["writer"].as_u64().unwrap() < 8);
        assert_eq!(document["padding"].as_str().unwrap().len(), 8192);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn oversized_existing_store_is_refused_on_read() {
        let scratch = scratch("read-limit");
        let path = scratch.path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path)
            .unwrap()
            .set_len(MAXIMUM_STORE_BYTES as u64 + 1)
            .unwrap();
        assert!(read_store(&path).unwrap_err().contains("too large"));
    }
}
