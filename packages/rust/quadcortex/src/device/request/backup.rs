use super::*;

/// The public backup name's whole domain, in one place.
///
/// The device backup workflow requires this boundary to enforce 1-64 visible
/// characters "without truncating or silently deleting characters", so every
/// host must refuse an out-of-domain name rather than reshape it. The
/// desktop's `safe_backup_name` truncated at 80 instead, which let a 65-80
/// character name pass its own check, open the save dialog, run the entire
/// transfer, and only then fail here.
pub const BACKUP_NAME_MAX_CHARACTERS: usize = 64;

/// Validate a requested backup name before any host begins the native export.
pub fn validate_backup_name(requested_name: &str) -> Result<String, String> {
    if requested_name.trim().is_empty() {
        return Err("Backup name cannot be empty.".into());
    }
    // Keep the public maximum pinned at the shared validation boundary.
    const _: () = assert!(BACKUP_NAME_MAX_CHARACTERS == 64);
    validate_visible_text(
        requested_name.to_string(),
        "Backup name",
        BACKUP_NAME_MAX_CHARACTERS,
    )
}

/// Validate a raw backup document returned by the device and stamp it with a name.
///
/// `requested_name` is checked with [`validate_backup_name`]. `raw` must be a JSON object whose
/// `type` is `"backup"` and `creator` is `"quad"`, with a non-empty Base64 `payload` and a
/// 64-hex-digit `payload_hash`. The hash is checked for shape only; it is not recomputed. On success
/// the document is returned with its `name` field set to the validated name.
///
/// # Errors
///
/// Returns a message when the name is out of domain, `raw` is not valid JSON, the document is not a
/// supported backup, or the payload or hash is missing or malformed.
pub fn finalize_device_backup(raw: &str, requested_name: &str) -> Result<Value, String> {
    let name = validate_backup_name(requested_name)?;
    let mut document: Value = serde_json::from_str(raw)
        .map_err(|error| format!("The Quad Cortex returned malformed backup JSON: {error}"))?;
    if !document.is_object()
        || document.get("type").and_then(Value::as_str) != Some("backup")
        || document.get("creator").and_then(Value::as_str) != Some("quad")
    {
        return Err("The Quad Cortex returned an unsupported backup document.".into());
    }
    let payload = document
        .get("payload")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "The Quad Cortex backup is missing its native payload or integrity identifier."
                .to_string()
        })?;
    document
        .get("payload_hash")
        .and_then(Value::as_str)
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            "The Quad Cortex backup is missing its native payload or integrity identifier."
                .to_string()
        })?;
    BASE64
        .decode(payload)
        .map_err(|_| "The Quad Cortex backup payload is not valid Base64.".to_string())?;
    document["name"] = Value::String(name);
    Ok(document)
}
