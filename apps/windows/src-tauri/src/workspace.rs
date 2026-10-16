use super::*;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const MAX_WORKSPACE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_NATIVE_BACKUP_BYTES: usize = 32 * 1024 * 1024;

static NEXT_DOCUMENT_STAGE: AtomicU64 = AtomicU64::new(0);

/// Keep the previous document intact until its replacement is fully written.
/// The stage shares the destination's directory so rename stays on one volume.
struct StagedDocument {
    path: PathBuf,
    file: Option<File>,
    owned: bool,
}

impl Drop for StagedDocument {
    fn drop(&mut self) {
        // Windows cannot remove a file while this handle is open.
        self.file.take();
        if self.owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn write_document_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    write_document_with_effects(
        path,
        write,
        |directory| {
            let sequence = NEXT_DOCUMENT_STAGE.fetch_add(1, Ordering::Relaxed);
            directory.join(format!(
                ".qc-document-{}-{sequence}.tmp",
                std::process::id()
            ))
        },
        |stage, target| fs::rename(stage, target),
    )
}

fn write_document_with_effects(
    path: &Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
    mut next_stage: impl FnMut(&Path) -> PathBuf,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let directory = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Document has no directory",
        )
    })?;
    let mut allocated = None;
    for _ in 0..64 {
        let stage_path = next_stage(directory);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage_path)
        {
            Ok(file) => {
                allocated = Some(StagedDocument {
                    path: stage_path,
                    file: Some(file),
                    owned: true,
                });
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let mut stage = allocated.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Could not reserve an exclusive document stage after 64 attempts",
        )
    })?;
    let file = stage.file.as_mut().expect("new stage has a file");
    write(file)?;
    file.sync_all()?;
    stage.file.take();
    rename(&stage.path, path)?;
    stage.owned = false;
    Ok(())
}

fn write_document_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_document_with(path, |file| file.write_all(bytes))
}

pub(super) fn write_native_backup(path: &Path, document: &Value) -> Result<(), String> {
    validate_native_backup(document)?;
    let bytes = serde_json::to_vec(document).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_NATIVE_BACKUP_BYTES {
        return Err("The native backup exceeds the 32 MiB safety limit".into());
    }
    write_document_atomically(path, &bytes)
        .map_err(|error| format!("Could not save native backup: {error}"))
}

pub(super) fn validate_native_backup(document: &Value) -> Result<(), String> {
    if !document.is_object()
        || document.get("type").and_then(Value::as_str) != Some("backup")
        || document.get("creator").and_then(Value::as_str) != Some("quad")
    {
        return Err("The device returned an unsupported native backup document".into());
    }
    let payload = document
        .get("payload")
        .and_then(Value::as_str)
        .ok_or("The native backup has no payload")?;
    if payload.is_empty() || payload.len() > MAX_NATIVE_BACKUP_BYTES {
        return Err("The native backup payload is empty or oversized".into());
    }
    let payload_hash = document
        .get("payload_hash")
        .and_then(Value::as_str)
        .ok_or("The native backup has no integrity identifier")?;
    if payload_hash.len() != 64 || !payload_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The native backup integrity identifier is malformed".into());
    }
    Ok(())
}

/// Hold the caller's backup name to the one shared rule before anything is sent.
///
/// This truncated at 80 while `quadcortex::device::request::finalize_device_backup`
/// hard-rejects anything over `BACKUP_NAME_MAX_CHARACTERS`, so a 65-80 character
/// name passed here, opened the save dialog, ran the whole device transfer, and
/// only then failed at the shared boundary. It also substituted `_` for
/// `<>:"/\|?*` and silently dropped control characters, which the backup name
/// contract forbids: it must be used exactly as given, without truncating or
/// silently deleting characters.
///
/// So the name is now validated, never reshaped, and refused early. Filesystem
/// sanitisation moves to `backup_file_stem`, which shapes only the *suggested*
/// filename in the save dialog - the user can edit it, and the document keeps
/// the name they actually gave.
pub(super) fn validate_backup_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("Backup name cannot be empty.".into());
    }
    if name.chars().count() > quadcortex::device::request::BACKUP_NAME_MAX_CHARACTERS
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "Backup name must contain 1-{} visible characters",
            quadcortex::device::request::BACKUP_NAME_MAX_CHARACTERS
        ));
    }
    Ok(name.to_string())
}

/// A filename Windows can actually save, offered as the save dialog's default.
///
/// This is presentation, not validation: it never decides whether a name is
/// acceptable, and it is not what gets sent to the device.
pub(super) fn backup_file_stem(name: &str) -> String {
    let substituted: String = name
        .chars()
        .map(|character| {
            if r#"<>:"/\|?*"#.contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = substituted.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "QC Device Backup".into()
    } else {
        trimmed.to_string()
    }
}

/// Validate the stable version-1 document fields before saving or exposing
/// an opened file to the renderer. Optional newer snapshot fields and unknown
/// fields are preserved so older workspaces remain readable.
pub(super) fn validate_workspace(document: &Value) -> Result<(), String> {
    if !document.is_object() || document.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("Unsupported or invalid QC workspace document".into());
    }
    let invalid = |field: &str| format!("Workspace document has an invalid or missing {field}");
    let source = document
        .get("source")
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("source"))?;
    let snapshot = document
        .get("snapshot")
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("snapshot"))?;
    let ui = document
        .get("ui")
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("ui"))?;
    for (name, value) in [("source", source), ("snapshot", snapshot)] {
        for field in [
            "deviceName",
            "setlistKey",
            "setlistName",
            "presetLocation",
            "presetName",
        ] {
            if value.get(field).and_then(Value::as_str).is_none() {
                return Err(invalid(&format!("{name}.{field}")));
            }
        }
        if value
            .get("presetPosition")
            .and_then(Value::as_u64)
            .is_none()
        {
            return Err(invalid(&format!("{name}.presetPosition")));
        }
    }
    if document.get("savedAt").and_then(Value::as_str).is_none() {
        return Err(invalid("savedAt"));
    }
    for field in ["selectedBlockId", "formFactorId"] {
        if ui.get(field).and_then(Value::as_str).is_none() {
            return Err(invalid(&format!("ui.{field}")));
        }
    }
    generated_gateway::validate_result(rpc::CURRENT_SNAPSHOT, snapshot)
        .map_err(|error| format!("Invalid workspace snapshot: {error}"))?;
    if !snapshot
        .get("mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| matches!(mode, "PRESET" | "SCENE" | "STOMP" | "HYBRID" | "UNKNOWN"))
    {
        return Err(invalid("snapshot.mode"));
    }
    if !snapshot
        .get("activeScene")
        .and_then(Value::as_u64)
        .is_some_and(|scene| scene < quadcortex::domain::SCENE_COUNT as u64)
    {
        return Err(invalid("snapshot.activeScene"));
    }
    if !snapshot
        .get("scenes")
        .and_then(Value::as_array)
        .is_some_and(|scenes| scenes.iter().all(Value::is_string))
    {
        return Err(invalid("snapshot.scenes"));
    }
    for field in ["tempo", "masterVolume"] {
        if snapshot.get(field).and_then(Value::as_f64).is_none() {
            return Err(invalid(&format!("snapshot.{field}")));
        }
    }
    if snapshot.get("dirty").and_then(Value::as_bool).is_none() {
        return Err(invalid("snapshot.dirty"));
    }
    for (field, valid) in [
        (
            "blocks",
            snapshot.get("blocks").is_some_and(|value| {
                serde_json::from_value::<Vec<quadcortex::generated_payloads::GridBlock>>(
                    value.clone(),
                )
                .is_ok()
            }),
        ),
        (
            "routes",
            snapshot.get("routes").is_some_and(|value| {
                serde_json::from_value::<Vec<quadcortex::generated_payloads::GridRoute>>(
                    value.clone(),
                )
                .is_ok()
            }),
        ),
    ] {
        if !valid {
            return Err(invalid(&format!("snapshot.{field}")));
        }
    }
    if document
        .get("selectedBlock")
        .is_some_and(|value| !value.is_object() && !value.is_null())
    {
        return Err(invalid("selectedBlock"));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn workspace_fixture() -> Value {
    json!({
        "version": 1, "savedAt": "2026-10-13T00:00:00Z",
        "source": {"deviceName": "Quad Cortex", "setlistKey": "user", "setlistName": "User",
            "presetPosition": 0, "presetLocation": "1A", "presetName": "Test"},
        "snapshot": {"deviceName": "Quad Cortex", "setlistKey": "user", "setlistName": "User",
            "presetPosition": 0, "presetLocation": "1A", "presetName": "Test", "mode": "PRESET",
            "activeScene": 0, "scenes": ["A", "B", "C", "D", "E", "F", "G", "H"],
            "blocks": [], "routes": [], "tempo": 120, "masterVolume": 0.5, "dirty": false},
        "ui": {"selectedBlockId": "", "formFactorId": "desktop"}
    })
}

pub(super) fn validate_workspace_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Workspace path must be absolute".into());
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        != Some("qcw".into())
    {
        return Err("Workspace files must use the .qcw extension".into());
    }
    Ok(())
}

pub(super) fn read_workspace_bytes(path: &Path) -> Result<Vec<u8>, String> {
    read_workspace_bytes_with(path, || {})
}

fn read_workspace_bytes_with(path: &Path, before_read: impl FnOnce()) -> Result<Vec<u8>, String> {
    // Inspect and read one held file. The byte envelope, not metadata, is the
    // authority even if an ordinary concurrent writer grows the document.
    let file = File::open(path).map_err(|error| format!("Could not open workspace: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect workspace: {error}"))?;
    if metadata.len() > MAX_WORKSPACE_BYTES as u64 {
        return Err("Workspace exceeds the 16 MiB size limit".into());
    }
    before_read();
    let mut bytes = Vec::new();
    file.take(MAX_WORKSPACE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not open workspace: {error}"))?;
    if bytes.len() > MAX_WORKSPACE_BYTES {
        return Err("Workspace exceeds the 16 MiB size limit".into());
    }
    Ok(bytes)
}

pub(super) fn write_workspace(path: &Path, document: &Value) -> Result<Value, String> {
    validate_workspace(document)?;
    validate_workspace_path(path)?;
    let bytes = serde_json::to_vec_pretty(document).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_WORKSPACE_BYTES {
        return Err("Workspace exceeds the 16 MiB size limit".into());
    }
    write_document_atomically(path, &bytes)
        .map_err(|error| format!("Could not save workspace: {error}"))?;
    Ok(json!({
        "cancelled": false,
        "path": path.to_string_lossy(),
        "name": path.file_name().and_then(|value| value.to_str()).unwrap_or("workspace.qcw")
    }))
}

pub(super) fn safe_workspace_name(value: &str) -> String {
    let filtered: String = value
        .chars()
        .map(|character| {
            if r#"<>:"/\|?*"#.contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = filtered.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        "QC Workspace".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod atomic_document_tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "qc-document-test-{}-{}-{}",
                std::process::id(),
                unix_seconds(),
                NEXT_DOCUMENT_STAGE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }

        fn assert_no_stages(&self) {
            assert!(!fs::read_dir(&self.0).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".qc-document-")));
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn partial_write_failure_preserves_the_previous_document_and_removes_stage() {
        let scratch = Scratch::new();
        let path = scratch.0.join("saved.qcw");
        fs::write(&path, b"previous complete document").unwrap();
        let result = write_document_with(&path, |file| {
            file.write_all(b"partial replacement")?;
            Err(std::io::Error::other("injected disk full"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous complete document");
        scratch.assert_no_stages();
    }

    #[test]
    fn workspace_reader_accepts_exact_envelope_and_refuses_one_extra_byte() {
        let scratch = Scratch::new();
        let path = scratch.0.join("envelope.qcw");
        fs::write(&path, vec![b' '; MAX_WORKSPACE_BYTES]).unwrap();
        let bytes = read_workspace_bytes(&path).unwrap();
        assert_eq!(bytes.len(), MAX_WORKSPACE_BYTES);
        assert!(bytes.iter().all(|byte| *byte == b' '));
        drop(bytes);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b" ").unwrap();
        drop(file);
        assert!(read_workspace_bytes(&path).unwrap_err().contains("16 MiB"));
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            MAX_WORKSPACE_BYTES as u64 + 1
        );
    }

    #[test]
    fn workspace_reader_refuses_growth_after_metadata_without_modifying_the_file() {
        let scratch = Scratch::new();
        let path = scratch.0.join("growing.qcw");
        fs::write(&path, b" ").unwrap();
        let result = read_workspace_bytes_with(&path, || {
            // Grow the same actual file after metadata inspection, no path swap.
            let file = OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(MAX_WORKSPACE_BYTES as u64 + 1).unwrap();
        });
        assert!(
            result.is_err(),
            "metadata alone must not authorize an unbounded read"
        );
        assert!(result.unwrap_err().contains("16 MiB"));
        assert!(fs::metadata(&path).unwrap().len() > MAX_WORKSPACE_BYTES as u64);
    }

    #[test]
    fn successful_transfer_does_not_delete_a_new_occupant_of_the_former_stage_path() {
        let scratch = Scratch::new();
        let target = scratch.0.join("saved.qcw");
        let stage = scratch.0.join(".qc-document-fixed.tmp");
        write_document_with_effects(
            &target,
            |file| file.write_all(b"new document"),
            |_| stage.clone(),
            |source, destination| {
                fs::rename(source, destination)?;
                fs::write(source, b"later independent occupant")
            },
        )
        .unwrap();
        assert_eq!(fs::read(target).unwrap(), b"new document");
        assert_eq!(fs::read(stage).unwrap(), b"later independent occupant");
    }

    #[test]
    fn exclusive_document_stage_collision_is_bounded_and_preserves_unowned_bytes() {
        let scratch = Scratch::new();
        let target = scratch.0.join("saved.qcw");
        fs::write(&target, b"old document").unwrap();
        let stage = scratch.0.join(".qc-document-fixed.tmp");
        fs::write(&stage, b"unowned collision").unwrap();
        let mut attempts = 0;
        let error = write_document_with_effects(
            &target,
            |_| panic!("no stage admitted"),
            |_| {
                attempts += 1;
                stage.clone()
            },
            |_, _| panic!("no transfer admitted"),
        )
        .unwrap_err();
        assert_eq!(attempts, 64);
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(target).unwrap(), b"old document");
        assert_eq!(fs::read(stage).unwrap(), b"unowned collision");
    }

    #[test]
    fn an_unwind_preserves_the_previous_document_and_removes_stage() {
        let scratch = Scratch::new();
        let path = scratch.0.join("saved.json");
        fs::write(&path, b"previous backup").unwrap();
        let result = std::panic::catch_unwind(|| {
            let _ = write_document_with(&path, |file| {
                file.write_all(b"partial backup")?;
                panic!("injected interruption")
            });
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous backup");
        scratch.assert_no_stages();
    }

    #[test]
    fn failed_replacement_removes_stage_and_leaves_destination_untouched() {
        let scratch = Scratch::new();
        let path = scratch.0.join("destination");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("sentinel"), b"keep").unwrap();
        assert!(write_document_atomically(&path, b"replacement").is_err());
        assert_eq!(fs::read(path.join("sentinel")).unwrap(), b"keep");
        scratch.assert_no_stages();
    }

    #[test]
    fn workspace_and_native_backup_replace_existing_documents_without_stages() {
        let scratch = Scratch::new();
        let workspace = scratch.0.join("saved.qcw");
        let backup = scratch.0.join("saved.json");
        fs::write(&workspace, b"old workspace").unwrap();
        fs::write(&backup, b"old backup").unwrap();
        let workspace_document = workspace_fixture();
        let backup_document = json!({"type": "backup", "creator": "quad",
            "payload": "AA==", "payload_hash": "0".repeat(64)});
        write_workspace(&workspace, &workspace_document).unwrap();
        write_native_backup(&backup, &backup_document).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&workspace).unwrap()).unwrap(),
            workspace_document
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&backup).unwrap()).unwrap(),
            backup_document
        );
        assert!(write_native_backup(&backup, &json!({"type": "backup"})).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&backup).unwrap()).unwrap(),
            backup_document
        );
        scratch.assert_no_stages();
    }

    #[test]
    fn malformed_workspace_fields_are_refused_before_the_renderer_or_saved_file_changes() {
        let scratch = Scratch::new();
        let path = scratch.0.join("saved.qcw");
        let valid = workspace_fixture();
        write_workspace(&path, &valid).unwrap();
        let original = fs::read(&path).unwrap();
        for pointer in [
            "/source",
            "/source/setlistName",
            "/source/presetPosition",
            "/savedAt",
            "/ui",
            "/ui/selectedBlockId",
            "/snapshot",
            "/snapshot/blocks",
            "/snapshot/routes",
            "/snapshot/activeScene",
            "/snapshot/dirty",
            "/snapshot/mode",
            "/snapshot/scenes",
            "/snapshot/tempo",
            "/snapshot/masterVolume",
        ] {
            let mut malformed = valid.clone();
            *malformed.pointer_mut(pointer).unwrap() = json!(null);
            assert!(validate_workspace(&malformed).is_err(), "{pointer}");
            assert!(write_workspace(&path, &malformed).is_err(), "{pointer}");
            assert_eq!(fs::read(&path).unwrap(), original);
        }
        let mut older = valid.clone();
        older["snapshot"]["futureField"] = json!({"preserve": true});
        assert!(
            validate_workspace(&older).is_ok(),
            "optional new fields are not required"
        );
        assert!(validate_workspace(&json!({"version": 1, "snapshot": {}})).is_err());
        scratch.assert_no_stages();
    }
}
