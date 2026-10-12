//! Device backups requested by a remote or MCP caller are saved on this PC.
//!
//! A backup is the whole Quad Cortex: every preset, setlist, capture and
//! setting the owner has. The window's own Backup command saves it through a
//! save dialog; a remote caller (the relay, or an MCP client through the local
//! listener) used to receive the complete document in its result instead, so
//! the owner's entire library left the machine as JSON in a model's context.
//! Android has always kept the file on the device and answered with where it
//! went. This module gives Windows the same contract: the host writes the file
//! under the user's Documents folder and the caller learns only its path, its
//! file name and its size.
use qc_relay_client::DeviceError;
use quadcortex::device::generated_gateway::rpc;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Where remote backups land: `Documents\QC Remote\Backups`.
///
/// The window's Backup command asks the person where to save; a remote call
/// has nobody to ask, so this documented default is used instead.
pub(crate) fn default_backup_directory() -> Option<PathBuf> {
    dirs::document_dir().map(|documents| documents.join("QC Remote").join("Backups"))
}

/// Run `call` for `method`, keeping a device backup on this PC.
///
/// Every other method passes through untouched. For a backup the name is
/// validated before the device is asked (the same rule the window applies),
/// the returned document is validated and written to `directory`, and the
/// caller gets `{path, name, size}` - never the document.
pub(crate) async fn invoke_saving_backups<F, Fut>(
    method: &str,
    params: Value,
    directory: Option<PathBuf>,
    call: F,
) -> Result<Value, DeviceError>
where
    F: FnOnce(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, DeviceError>>,
{
    if method != rpc::CREATE_DEVICE_BACKUP {
        return call(params).await;
    }
    let requested = params.get("name").and_then(Value::as_str).unwrap_or("");
    let name = crate::validate_backup_name(requested)
        .map_err(|message| DeviceError::new("INVALID_PARAMS", message, false))?;
    let Some(directory) = directory else {
        return Err(save_failure(
            "QC Remote could not find this user's Documents folder, so the backup was not taken"
                .into(),
        ));
    };
    let document = call(json!({ "name": name })).await?;
    tokio::task::spawn_blocking(move || save_backup_in(&directory, &document, &name))
        .await
        .map_err(|error| save_failure(error.to_string()))?
        .map_err(save_failure)
}

fn save_failure(message: String) -> DeviceError {
    DeviceError::new("BACKUP_SAVE_FAILED", message, false)
}

/// Validate `document` and write it to a new file in `directory`.
///
/// Never overwrites: an existing backup with the same name keeps its file and
/// the new one is numbered, `Name (2).json`, `Name (3).json`, and so on.
pub(crate) fn save_backup_in(
    directory: &Path,
    document: &Value,
    name: &str,
) -> Result<Value, String> {
    crate::validate_native_backup(document)?;
    let bytes = serde_json::to_vec(document).map_err(|error| error.to_string())?;
    if bytes.len() > crate::MAX_NATIVE_BACKUP_BYTES {
        return Err("The native backup exceeds the 32 MiB safety limit".into());
    }
    fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create the backup folder: {error}"))?;
    let stem = crate::backup_file_stem(name);
    for attempt in 1..=1000_u32 {
        let file_name = if attempt == 1 {
            format!("{stem}.json")
        } else {
            format!("{stem} ({attempt}).json")
        };
        let path = directory.join(&file_name);
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not save native backup: {error}")),
        };
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(format!("Could not save native backup: {error}"));
        }
        return Ok(json!({
            "path": path.to_string_lossy(),
            "name": file_name,
            "size": bytes.len(),
        }));
    }
    Err("Too many backups already use this name; choose another name".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn document() -> Value {
        json!({
            "type": "backup",
            "creator": "quad",
            "name": "Tour",
            "payload": "AQID",
            "payload_hash": "a".repeat(64),
        })
    }

    /// A directory of this test's own, and the guard that removes it when the
    /// test ends or unwinds.
    fn scratch(label: &str) -> (crate::tests::RemoveDirOnDrop, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "qc-remote-backup-{label}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let _ = fs::remove_dir_all(&directory);
        (crate::tests::RemoveDirOnDrop::new(directory.clone()), directory)
    }

    #[tokio::test]
    async fn a_remote_backup_is_saved_here_and_only_its_location_is_returned() {
        let (_scratch, directory) = scratch("remote");
        let calls = AtomicUsize::new(0);
        let result = invoke_saving_backups(
            rpc::CREATE_DEVICE_BACKUP,
            json!({"name": " Tour: 2026/10 "}),
            Some(directory.clone()),
            |params| {
                calls.fetch_add(1, Ordering::SeqCst);
                // The device sees the validated name, exactly as given.
                assert_eq!(params, json!({"name": "Tour: 2026/10"}));
                async { Ok(document()) }
            },
        )
        .await
        .expect("saved");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let keys: Vec<&str> = result
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["path", "name", "size"], "nothing but the location");
        assert!(!result.to_string().contains("AQID"), "the payload leaked");
        assert_eq!(result["name"], "Tour_ 2026_10.json");
        let path = PathBuf::from(result["path"].as_str().unwrap());
        assert_eq!(path, directory.join("Tour_ 2026_10.json"));
        let written = fs::read(&path).unwrap();
        assert_eq!(result["size"], written.len());
        assert_eq!(
            serde_json::from_slice::<Value>(&written).unwrap(),
            document()
        );

        // A second backup under the same name never overwrites the first.
        let second = invoke_saving_backups(
            rpc::CREATE_DEVICE_BACKUP,
            json!({"name": "Tour: 2026/10"}),
            Some(directory.clone()),
            |_| async { Ok(document()) },
        )
        .await
        .unwrap();
        assert_eq!(second["name"], "Tour_ 2026_10 (2).json");
        assert_eq!(fs::read(&path).unwrap(), written);
    }

    #[tokio::test]
    async fn a_bad_name_or_document_is_refused_and_nothing_is_written() {
        let (_scratch, directory) = scratch("refused");
        for name in [json!(""), json!("x".repeat(65)), json!("a\nb"), Value::Null] {
            let refusal = invoke_saving_backups(
                rpc::CREATE_DEVICE_BACKUP,
                json!({ "name": name }),
                Some(directory.clone()),
                |_| async { panic!("an invalid name must not reach the device") },
            )
            .await
            .unwrap_err();
            assert_eq!(refusal.code, "INVALID_PARAMS");
        }
        let mut forged = document();
        forged["creator"] = json!("someone-else");
        let refusal = invoke_saving_backups(
            rpc::CREATE_DEVICE_BACKUP,
            json!({"name": "Tour"}),
            Some(directory.clone()),
            |_| async move { Ok(forged) },
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.code, "BACKUP_SAVE_FAILED");
        assert!(!directory.join("Tour.json").exists());
        let refusal = invoke_saving_backups(
            rpc::CREATE_DEVICE_BACKUP,
            json!({"name": "Tour"}),
            None,
            |_| async { panic!("no folder to save into means no transfer") },
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.code, "BACKUP_SAVE_FAILED");
    }

    #[tokio::test]
    async fn every_other_method_passes_through_untouched() {
        let result = invoke_saving_backups(
            "device.currentPreset",
            json!({"anything": 1}),
            None,
            |params| async move { Ok(json!({ "echo": params })) },
        )
        .await
        .unwrap();
        assert_eq!(result, json!({"echo": {"anything": 1}}));
    }

    #[test]
    fn the_default_folder_is_under_documents() {
        if let (Some(documents), Some(folder)) = (dirs::document_dir(), default_backup_directory())
        {
            assert_eq!(folder, documents.join("QC Remote").join("Backups"));
        }
    }
}
