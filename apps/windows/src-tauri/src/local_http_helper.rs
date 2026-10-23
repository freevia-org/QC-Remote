//! Short-lived direct HTTP credential helper. Never starts a host or device.
//! Its proof gates credential issuance, NOT later cached-client connections.
use crate::local_http_binding::StablePort;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DESCRIPTOR_BYTES: usize = 4096;
const MAX_AGE_MS: u64 = 150_000;
pub(crate) const PORT_FILE: &str = "codex-http-port.json";
pub(crate) const DESCRIPTOR_FILE: &str = "codex-http.json";

/// Deliberately no Debug: this record contains a bearer credential.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectDescriptor {
    pub(crate) schema_version: u8,
    pub(crate) port: u16,
    pub(crate) bearer_token: String,
    pub(crate) instance_id: String,
    pub(crate) process_id: u32,
    pub(crate) process_created_at: u64,
    pub(crate) published_at_unix_ms: u64,
}

#[cfg(windows)]
pub(crate) fn directory() -> Result<PathBuf, String> {
    // dirs 6 uses SHGetKnownFolderPath(FOLDERID_LocalAppData) on Windows.
    // No APPDATA mirror, QC_REMOTE_LOCAL_MCP_STATE, temp or environment fallback.
    dirs::data_local_dir()
        .filter(|path| path.is_absolute())
        .map(|path| path.join(crate::broker_paths::APP_DIRECTORY))
        .ok_or_else(|| "The native LocalAppData directory is unavailable".into())
}

#[cfg(target_os = "macos")]
pub(crate) fn directory() -> Result<PathBuf, String> {
    #[cfg(feature = "macos-app-store")]
    let directory = qc_native_platform::app_store::shared_directory()?;
    #[cfg(not(feature = "macos-app-store"))]
    let directory = qc_native_platform::paths_macos::default_directory();
    qc_native_platform::paths_macos::prepare_directory(&directory)?;
    Ok(directory)
}

fn parse_descriptor(bytes: &[u8], port: u16, now: u64) -> Result<DirectDescriptor, String> {
    if bytes.len() > DESCRIPTOR_BYTES {
        return Err("Shared HTTP descriptor exceeds its byte budget".into());
    }
    let value: DirectDescriptor =
        serde_json::from_slice(bytes).map_err(|_| "Shared HTTP descriptor is invalid")?;
    let safe_text = |text: &str, length: usize| {
        text.len() == length
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    };
    if value.schema_version != 1
        || value.port == 0
        || value.port != port
        || value.process_id == 0
        || value.process_created_at == 0
        || !safe_text(&value.bearer_token, 43)
        || !safe_text(&value.instance_id, 22)
        || value.published_at_unix_ms > now
        || now.saturating_sub(value.published_at_unix_ms) > MAX_AGE_MS
    {
        return Err("Shared HTTP descriptor is stale or does not match this pairing".into());
    }
    Ok(value)
}

fn read_descriptor(path: &Path, port: u16) -> Result<DirectDescriptor, String> {
    let bytes = qc_owner_only_file::read_owner_only_bounded(path, DESCRIPTOR_BYTES)
        .map_err(|_| "Could not read the owner-private shared HTTP descriptor")?;
    parse_descriptor(&bytes, port, crate::broker_paths::unix_time_ms())
}

fn refuse_quit(directory: &Path) -> Result<(), String> {
    let broker = crate::device_broker_descriptor::device_broker_descriptor_path(directory);
    if crate::user_quit_refusal(&broker).is_some() {
        return Err("The user quit the broker; choose Connect explicitly".into());
    }
    Ok(())
}

pub(crate) async fn verified_headers(directory: &Path) -> Result<serde_json::Value, String> {
    refuse_quit(directory)?;
    let port = StablePort::read(&directory.join(PORT_FILE))?;
    let descriptor = read_descriptor(&directory.join(DESCRIPTOR_FILE), port.port())?;
    if qc_native_platform::process::os_process_created_at(descriptor.process_id)
        != Some(descriptor.process_created_at)
    {
        return Err("The HTTP host's exact OS identity is unavailable or changed".into());
    }
    let client = crate::loopback_http::loopback_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|_| "Could not prepare local identity verification")?;
    crate::local_identity::verify(
        &client,
        &format!("http://127.0.0.1:{}/identity", port.port()),
        &descriptor.instance_id,
        &descriptor.bearer_token,
    )
    .await
    .map_err(|_| "The local HTTP listener did not prove its identity")?;
    refuse_quit(directory)?;
    let after = read_descriptor(&directory.join(DESCRIPTOR_FILE), port.port())?;
    if after.instance_id != descriptor.instance_id
        || after.bearer_token != descriptor.bearer_token
        || after.process_id != descriptor.process_id
        || after.process_created_at != descriptor.process_created_at
        || qc_native_platform::process::os_process_created_at(after.process_id)
            != Some(after.process_created_at)
    {
        return Err("The local HTTP host changed during credential verification".into());
    }
    Ok(serde_json::json!({"Authorization":format!("Bearer {}", descriptor.bearer_token)}))
}

pub(crate) fn run() -> Result<(), String> {
    let directory = directory()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start bounded credential verification")?;
    let headers = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(6), verified_headers(&directory))
            .await
            .map_err(|_| "Local HTTP identity verification timed out")?
    })?;
    let bytes = serde_json::to_vec(&headers).map_err(|_| "Could not encode credential headers")?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&bytes)
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .map_err(|_| "Could not write credential headers".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> serde_json::Value {
        serde_json::json!({"schemaVersion":1,"port":41001,"bearerToken":"T".repeat(43),"instanceId":"I".repeat(22),"processId":10,"processCreatedAt":1,"publishedAtUnixMs":1000})
    }
    #[test]
    fn direct_descriptor_is_bounded_exact_port_epoch_and_no_arbitrary_url() {
        let value = fixture();
        assert!(parse_descriptor(&serde_json::to_vec(&value).unwrap(), 41001, 1000).is_ok());
        for key in ["url", "rpcUrl", "identityUrl", "legacyToken"] {
            let mut value = fixture();
            value[key] = serde_json::json!("not-accepted");
            assert!(parse_descriptor(&serde_json::to_vec(&value).unwrap(), 41001, 1000).is_err());
        }
        assert!(parse_descriptor(&serde_json::to_vec(&value).unwrap(), 41002, 1000).is_err());
        assert!(parse_descriptor(&serde_json::to_vec(&value).unwrap(), 41001, 151001).is_err());
        assert!(parse_descriptor(&serde_json::to_vec(&value).unwrap(), 41001, 999).is_err());
    }
}
