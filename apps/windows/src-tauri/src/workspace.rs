use super::*;

pub(super) const MAX_WORKSPACE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_NATIVE_BACKUP_BYTES: usize = 32 * 1024 * 1024;

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

// This checks the shape only: any object with `version: 1` and an object `snapshot` is written
// as given. The Windows front end strips the device's model preset list before it sends a
// document (`saveWorkspaceDocument`), and nothing here backs that up, so a caller that builds its
// own document would persist whatever it puts in the snapshot. Rejecting or removing a
// `snapshot.modelPresetCatalog` key here would be defence in depth; it is proposed, not made.
pub(super) fn validate_workspace(document: &Value) -> Result<(), String> {
    if !document.is_object() || document.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("Unsupported or invalid QC workspace document".into());
    }
    if !document.get("snapshot").is_some_and(Value::is_object) {
        return Err("Workspace document has no normalized snapshot".into());
    }
    Ok(())
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

pub(super) fn write_workspace(path: &Path, document: &Value) -> Result<Value, String> {
    validate_workspace(document)?;
    validate_workspace_path(path)?;
    let bytes = serde_json::to_vec_pretty(document).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_WORKSPACE_BYTES {
        return Err("Workspace exceeds the 16 MiB size limit".into());
    }
    fs::write(path, bytes).map_err(|error| format!("Could not save workspace: {error}"))?;
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
