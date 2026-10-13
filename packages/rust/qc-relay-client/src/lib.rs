use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::watch;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig, Message,
    },
};

pub use qc_relay_protocol::{
    is_valid_grant_id, pairing_owner_refusal, project_dispatch_error_details, DeviceError,
    DeviceFrame, McpClientPresence, BACKOFF_JITTER_MS, COMPLETED_REQUEST_CACHE_SIZE,
    CREDENTIAL_TTL_DAYS, DEVICE_CONNECTORS_PATH, DEVICE_CONNECT_PATH, DEVICE_PAIR_PATH,
    DEVICE_REPLACEMENT_PATH, DEVICE_REVOKE_PATH, DEVICE_ROSTER_PATH, HOST_APPROVAL_TIMEOUT_MS,
    MAXIMUM_BACKOFF_EXPONENT, MAXIMUM_BACKOFF_SECONDS, MAXIMUM_FAILURE_COUNT,
    MAXIMUM_INVOKE_DEADLINE_MS, MAX_REQUEST_FRAME_BYTES, MAX_RESULT_FRAME_BYTES,
    MINIMUM_CREDENTIAL_LENGTH, PAIRING_CODE_MAXIMUM_LENGTH, PAIRING_CODE_MINIMUM_LENGTH,
    PROTOCOL_VERSION, READINESS_INTERVAL_MS,
};

mod generated_actions;

const ADAPTER_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// The deadline used when a relay sends no `deadlineMs`.
///
/// A flat host-side timeout is a fourth layer on top of the device profile, the
/// relay budget and the caller's own wait, and it was the shortest of the four:
/// seven persistent and catalog mutations were abandoned at five minutes inside
/// a budget of up to twenty-one, and reported as retryable. A retry after a
/// persistent write that may have committed is what leaves a second copy of a
/// preset on the unit. The relay now states the budget per call and this value
/// only covers a relay that predates that field.
const ADAPTER_INVOKE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Honour the relay's stated budget, but never past the generated ceiling: a
/// hostile or confused relay must not be able to pin the USB adapter for ever.
fn adapter_invoke_timeout(deadline_ms: Option<u64>) -> Duration {
    match deadline_ms {
        Some(0) | None => ADAPTER_INVOKE_TIMEOUT,
        Some(value) => Duration::from_millis(value.min(MAXIMUM_INVOKE_DEADLINE_MS)),
    }
}
/// The relay never sends this host more than one request frame at a time, so
/// tungstenite must refuse anything larger at the frame layer rather than
/// buffer the 64 MiB message / 16 MiB frame it allows by default and only then
/// let the length check after it discard the text.
fn relay_socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_REQUEST_FRAME_BYTES))
        .max_frame_size(Some(MAX_REQUEST_FRAME_BYTES))
}
const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessMode {
    #[default]
    Full,
    Modify,
    Performance,
    ReadOnly,
}

impl AccessMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            generated_actions::ACCESS_FULL => Ok(Self::Full),
            generated_actions::ACCESS_MODIFY => Ok(Self::Modify),
            generated_actions::ACCESS_PERFORMANCE => Ok(Self::Performance),
            generated_actions::ACCESS_READ_ONLY => Ok(Self::ReadOnly),
            _ => Err("Relay access mode must be read-only, performance, modify, or full".into()),
        }
    }

    /// Stable generated spelling used in persisted access preferences.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => generated_actions::ACCESS_FULL,
            Self::Modify => generated_actions::ACCESS_MODIFY,
            Self::Performance => generated_actions::ACCESS_PERFORMANCE,
            Self::ReadOnly => generated_actions::ACCESS_READ_ONLY,
        }
    }

    /// Apply the shared assistant-access policy at every provider transport.
    pub fn permits(self, method: &str) -> bool {
        match self {
            Self::Full => generated_actions::contains(method),
            Self::Modify => {
                generated_actions::is_read_only(method)
                    || generated_actions::is_performance(method)
                    || generated_actions::is_modify(method)
            }
            Self::Performance => {
                generated_actions::is_read_only(method) || generated_actions::is_performance(method)
            }
            Self::ReadOnly => generated_actions::is_read_only(method),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    #[default]
    Stopped,
    Connecting,
    Connected,
    Reconnecting,
    PairingRequired,
    InvalidEndpoint,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    /// Set only by an identity-bound host callback after this credential is rejected.
    #[serde(default)]
    pub credential_rejected: bool,
    /// Host-owned connection incarnation; never sent to the relay.
    #[serde(default)]
    pub connection_generation: u64,
    /// Encrypted by the host credential store; never transmitted or automatically restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_credential: Option<Box<Credentials>>,
    pub endpoint: String,
    pub device_credential: String,
    pub device_id: String,
    /// The identity system which authorized this host. Older credentials omit
    /// this value and must be paired again rather than silently crossing an
    /// ownership boundary after an identity-provider migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_provider: Option<IdentityProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing: Option<PairingMetadata>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityProvider {
    Google,
}

impl Credentials {
    pub fn uses_current_identity_provider(&self) -> bool {
        self.identity_provider == Some(IdentityProvider::Google)
    }
}

/// Display-only identity returned with a successful browser pairing receipt.
/// This is deliberately not used as an authorization credential.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingMetadata {
    pub email: String,
    pub device: String,
    pub paired_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnector {
    pub grant_id: String,
    pub client_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub authorized_at: u64,
    pub expires_at: Option<u64>,
    pub authorized_from: Option<String>,
    pub device: Option<String>,
    pub browser: Option<String>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub client_uri: Option<String>,
    pub active: Option<bool>,
    pub last_active_at: Option<u64>,
    pub app_version: Option<String>,
    pub activity_device: Option<String>,
    pub activity_browser: Option<String>,
    pub activity_ip_address: Option<String>,
    pub activity_country: Option<String>,
    pub activity_city: Option<String>,
}

#[derive(Deserialize)]
struct ConnectorResponse {
    connectors: Vec<RemoteConnector>,
}

/// A short-lived QR/browser pairing invitation. The receiver secret is held by
/// the initiating host only; the URL is safe to render as a QR code.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPairingSession {
    #[serde(skip)]
    pub endpoint: String,
    pub session_id: String,
    pub receiver_secret: String,
    pub pair_url: String,
    pub expires_at: u64,
    pub protocol: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostPairingPoll {
    state: String,
    pairing_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairingResponse {
    #[serde(default)]
    previous_device_replacement_pending: Option<bool>,
    device_credential: String,
    device_id: String,
    #[serde(default)]
    identity_provider: Option<IdentityProvider>,
    #[serde(default)]
    pairing: Option<PairingMetadata>,
}

/// Aggregated relay activity for the UTC day. This deliberately contains no
/// prompt, device-state, or credential data, so it is safe to display in a
/// host application's connection panel.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub day: String,
    pub commands: u64,
    #[serde(default)]
    pub last_command_at: Option<u64>,
    #[serde(default)]
    pub platform_usage: Option<String>,
}

/// One device paired to the same account as this host.
///
/// Display-only, and deliberately narrower than the relay's own record: no
/// credential, no credential hash, nothing the person did not already see on
/// the pairing page for that device. `current` is the host asking.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub device_id: String,
    pub name: String,
    pub paired_at: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub expired: bool,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The browser the sign-in was completed in, as that device reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_from: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RosterResponse {
    #[serde(default)]
    devices: Vec<PairedDevice>,
}

#[async_trait]
pub trait DeviceAdapter: Send + Sync + 'static {
    async fn ready(&self) -> bool;
    async fn invoke(&self, method: &str, params: Value) -> Result<Value, DeviceError>;
    /// A policy source that is authoritative for this serving adapter. Hosts
    /// with cross-process settings should override this and read their
    /// request-time policy here; older adapters keep the connection's watch
    /// snapshot by leaving the default `None` in place.
    fn effective_access_mode(&self) -> Option<AccessMode> {
        None
    }
    /// Whether this paired remote session is still enabled by the serving
    /// host. This is checked for every relay invocation; local MCP requests do
    /// not use it. Adapters without a separate session latch remain compatible.
    fn remote_session_enabled(&self) -> bool {
        true
    }
    /// `invoke` for a call the relay marked as an MCP resource read
    /// ([`qc_relay_protocol::RESOURCE_READ_ORIGIN`]). A host that can serve a
    /// read without bringing a device connection or session up overrides
    /// this; the default is `invoke`, today's behaviour.
    async fn invoke_resource_read(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, DeviceError> {
        self.invoke(method, params).await
    }
}

/// No remote method waits for a second approval at the paired device. The
/// initiating AI client owns the confirmation for persistent and risky
/// actions; its MCP/cloud action boundary rejects calls without the required
/// explicit confirmation before forwarding them. The paired host still checks
/// pairing, access mode, allowlists, and the device workflow's exact target
/// guards. A model confirmation flag is not evidence of a human gesture by
/// itself; this contract relies on the initiating client's confirmation UX.
pub const HOST_APPROVAL_METHODS: &[&str] = &[];

pub fn normalize_endpoint(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let parsed = Url::parse(trimmed).map_err(|_| "Relay endpoint is not a valid URL")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "Relay endpoint must be a plain HTTPS origin without credentials, query, or fragment"
                .into(),
        );
    }
    let mut normalized = parsed;
    let path = normalized.path().trim_end_matches('/').to_owned();
    normalized.set_path(&path);
    Ok(normalized.to_string().trim_end_matches('/').to_owned())
}

/// Build the actual host pair body. Never forward a stored secret across relay origins.
pub fn pairing_request_body(
    endpoint: &str,
    code: &str,
    name: &str,
    previous: Option<&Credentials>,
) -> Result<Value, String> {
    let origin = normalize_endpoint(endpoint)?;
    let mut body = json!({"pairingCode": code, "deviceName": name, "protocol": PROTOCOL_VERSION});
    if let Some(old) = previous {
        if normalize_endpoint(&old.endpoint).ok().as_ref() != Some(&origin) {
            return Err("PAIRING_ORIGIN_CHANGED: Unpair before changing relay origins.".into());
        }
        if !old.credential_rejected
            && old
                .pairing
                .as_ref()
                .map(|p| p.email.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(
                "PAIRING_OWNER_UNKNOWN: Unpair before replacing an unverifiable account.".into(),
            );
        }
        if normalize_endpoint(&old.endpoint).ok().as_ref() == Some(&origin)
            && (8..=64).contains(&old.device_id.len())
            && old
                .device_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && (32..=256).contains(&old.device_credential.len())
        {
            body["previousDeviceId"] = json!(old.device_id);
            body["previousDeviceCredential"] = json!(old.device_credential);
        } else {
            return Err("PAIRING_PREVIOUS_INVALID: Stored identity cannot authorize replacement; explicitly unpair first.".into());
        }
    }
    Ok(body)
}

fn pairing_request(
    client: &reqwest::Client,
    endpoint: &str,
    code: &str,
    name: &str,
    previous: Option<&Credentials>,
) -> Result<reqwest::Request, String> {
    let origin = normalize_endpoint(endpoint)?;
    let body = pairing_request_body(&origin, code, name, previous)?;
    let path = if previous.is_some() {
        DEVICE_REPLACEMENT_PATH
    } else {
        DEVICE_PAIR_PATH
    };
    client
        .post(format!("{origin}{path}"))
        .json(&body)
        .build()
        .map_err(|_| "Could not construct a pairing request".into())
}

pub async fn pair(
    endpoint: &str,
    pairing_code: &str,
    device_name: &str,
) -> Result<Credentials, String> {
    pair_replacing(endpoint, pairing_code, device_name, None).await
}

fn pairing_http_error(status: u16, replacing: bool) -> Option<&'static str> {
    if replacing && status == 404 {
        return Some("PAIRING_REPLACEMENT_UNSUPPORTED: Relay upgrade required; no legacy pairing fallback was attempted.");
    }
    if !(200..300).contains(&status) {
        return Some("The public relay rejected the pairing code");
    }
    None
}

pub async fn pair_replacing(
    endpoint: &str,
    pairing_code: &str,
    device_name: &str,
    previous: Option<&Credentials>,
) -> Result<Credentials, String> {
    let endpoint = normalize_endpoint(endpoint)?;
    let code = pairing_code.trim();
    let name = device_name.trim();
    if !(PAIRING_CODE_MINIMUM_LENGTH..=PAIRING_CODE_MAXIMUM_LENGTH).contains(&code.len())
        || name.is_empty()
    {
        return Err("A valid pairing code and device name are required".into());
    }
    // A 307/308 must not forward a predecessor credential to a different origin.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "Could not prepare a secure pairing request".to_string())?;
    let request = pairing_request(&client, &endpoint, code, name, previous)?;
    let response = client
        .execute(request)
        .await
        .map_err(|_| "Could not reach the public relay".to_string())?;
    if let Some(error) = pairing_http_error(response.status().as_u16(), previous.is_some()) {
        return Err(error.into());
    }
    let receipt: PairingResponse = response
        .json()
        .await
        .map_err(|_| "The public relay returned an invalid pairing receipt".to_string())?;
    if previous.is_some() && receipt.previous_device_replacement_pending.is_none() {
        return Err("The relay did not confirm the versioned replacement contract; no new connection was started".into());
    }
    if receipt.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
        || receipt.device_id.trim().is_empty()
    {
        return Err("The public relay returned an invalid device credential".into());
    }
    if let Some(pairing) = &receipt.pairing {
        if !valid_pairing_metadata(pairing) {
            return Err("The public relay returned invalid pairing details".into());
        }
    }
    if receipt.identity_provider != Some(IdentityProvider::Google) {
        return Err("QC Remote Cloud returned an unsupported pairing identity".into());
    }
    Ok(Credentials {
        credential_rejected: false,
        connection_generation: 0,
        previous_credential: None,
        endpoint,
        device_credential: receipt.device_credential,
        device_id: receipt.device_id,
        identity_provider: receipt.identity_provider,
        pairing: receipt.pairing,
    })
}

fn valid_pairing_metadata(pairing: &PairingMetadata) -> bool {
    pairing.email.len() <= 320
        && pairing.email.contains('@')
        && pairing.device.len() <= 80
        && !pairing.device.trim().is_empty()
        && pairing.paired_at > 0
}

/// Read the owning account's daily relay command count. The host uses its
/// device credential directly; callers never receive that capability.
pub async fn usage(credentials: &Credentials) -> Result<Usage, String> {
    let endpoint = normalize_endpoint(&credentials.endpoint)?;
    if credentials.device_id.trim().is_empty()
        || credentials.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
    {
        return Err("The stored relay credentials are invalid".into());
    }
    let mut url = Url::parse(&format!("{endpoint}/v1/device/usage"))
        .map_err(|_| "The relay endpoint is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("deviceId", &credentials.device_id);
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(&credentials.device_credential)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    if !response.status().is_success() {
        return Err(if matches!(response.status().as_u16(), 401 | 403) {
            "QC Remote Cloud no longer recognizes this computer. Pair it again.".into()
        } else {
            "QC Remote Cloud could not read relay usage".into()
        });
    }
    response
        .json::<Usage>()
        .await
        .map_err(|_| "QC Remote Cloud returned invalid relay usage".to_string())
}

/// Every device paired to the account that owns this host.
///
/// A host has only ever known the pairing it performed itself, so a panel
/// could show the computer it runs on and nothing else - an account with a
/// phone on it as well looked empty from both. The device credential is its
/// own authorization here, exactly as it is for usage and revocation.
///
/// A relay that predates this route answers 404, which is reported distinctly
/// so the caller can fall back to what it knows locally rather than show an
/// error for a deployment that is merely older.
pub async fn roster(credentials: &Credentials) -> Result<Vec<PairedDevice>, String> {
    let endpoint = normalize_endpoint(&credentials.endpoint)?;
    if credentials.device_id.trim().is_empty()
        || credentials.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
    {
        return Err("The stored relay credentials are invalid".into());
    }
    let mut url = Url::parse(&format!("{endpoint}{DEVICE_ROSTER_PATH}"))
        .map_err(|_| "The relay endpoint is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("deviceId", &credentials.device_id);
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(&credentials.device_credential)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 => {
                "QC Remote Cloud no longer recognizes this computer. Pair it again.".into()
            }
            404 => ROSTER_UNSUPPORTED.to_string(),
            _ => "QC Remote Cloud could not list the devices on this account".to_string(),
        });
    }
    Ok(response
        .json::<RosterResponse>()
        .await
        .map_err(|_| "QC Remote Cloud returned an invalid device list".to_string())?
        .devices)
}

/// What `roster` returns when the relay has no such route.
///
/// Not an error the person should be shown: it means this deployment is older
/// than the roster, and the caller already holds its own pairing to display.
pub const ROSTER_UNSUPPORTED: &str = "This QC Remote Cloud deployment cannot list paired devices";

pub async fn connectors(credentials: &Credentials) -> Result<Vec<RemoteConnector>, String> {
    let endpoint = normalize_endpoint(&credentials.endpoint)?;
    if credentials.device_id.trim().is_empty()
        || credentials.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
    {
        return Err("The stored relay credentials are invalid".into());
    }
    let mut url = Url::parse(&format!("{endpoint}{DEVICE_CONNECTORS_PATH}"))
        .map_err(|_| "The relay endpoint is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("deviceId", &credentials.device_id);
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(&credentials.device_credential)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 => {
                "QC Remote Cloud no longer recognizes this computer. Pair it again.".into()
            }
            404 => "This QC Remote Cloud deployment cannot list Remote MCP connectors".into(),
            _ => "QC Remote Cloud could not list Remote MCP connectors".into(),
        });
    }
    Ok(response
        .json::<ConnectorResponse>()
        .await
        .map_err(|_| "QC Remote Cloud returned an invalid connector list".to_string())?
        .connectors)
}

/// What QC Remote Cloud said about one connector revoke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectorRevoke {
    /// The relay confirmed the revoke.
    Revoked,
    /// The relay answered 404: the grant (or, on a relay that predates the
    /// route, the route itself) was not found. It may already be unlinked;
    /// callers that report to a person must not call this "unlinked".
    NotFound,
}

/// The outcome a revoke response status means. 404 has always been accepted
/// rather than refused, so it stays a success, but a distinct one.
pub fn connector_revoke_outcome(status: u16) -> Result<ConnectorRevoke, String> {
    match status {
        200..=299 => Ok(ConnectorRevoke::Revoked),
        404 => Ok(ConnectorRevoke::NotFound),
        401 | 403 => {
            Err("QC Remote Cloud no longer recognizes this computer. Pair it again.".into())
        }
        _ => Err("QC Remote Cloud could not unlink that connector".into()),
    }
}

pub async fn revoke_connector(
    credentials: &Credentials,
    grant_id: &str,
) -> Result<ConnectorRevoke, String> {
    let endpoint = normalize_endpoint(&credentials.endpoint)?;
    let grant_id = grant_id.trim();
    if grant_id.is_empty()
        || credentials.device_id.trim().is_empty()
        || credentials.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
    {
        return Err("The stored relay credentials are invalid".into());
    }
    let mut url = Url::parse(&format!("{endpoint}{DEVICE_CONNECTORS_PATH}"))
        .map_err(|_| "The relay endpoint is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("deviceId", &credentials.device_id)
        .append_pair("target", grant_id);
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&credentials.device_credential)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    connector_revoke_outcome(response.status().as_u16())
}

/// Retire a device of this account that is not this host.
///
/// The credential being withdrawn cannot authorize this: it lives on the
/// device being removed, which is precisely the device that is not here. This
/// host's own credential stands for the account instead, and the relay checks
/// both devices belong to it.
pub async fn revoke_paired_device(credentials: &Credentials, target: &str) -> Result<(), String> {
    let endpoint = normalize_endpoint(&credentials.endpoint)?;
    let target = target.trim();
    if target.is_empty() || target == credentials.device_id {
        return Err("A different paired device is required".into());
    }
    if credentials.device_id.trim().is_empty()
        || credentials.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
    {
        return Err("The stored relay credentials are invalid".into());
    }
    let mut url = Url::parse(&format!("{endpoint}{DEVICE_ROSTER_PATH}/revoke"))
        .map_err(|_| "The relay endpoint is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("deviceId", &credentials.device_id)
        .append_pair("target", target);
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&credentials.device_credential)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    // A device the relay no longer holds is already unpaired.
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    Err(if matches!(response.status().as_u16(), 401 | 403) {
        "QC Remote Cloud no longer recognizes this computer. Pair it again.".into()
    } else {
        "QC Remote Cloud could not unpair that device".into()
    })
}

/// Withdraw this host's credential at the relay.
///
/// Unpairing used to clear only the local credential store, which left the
/// relay honouring the withdrawn credential for ever. The credential being
/// retired is its own authorization: the host that holds it is exactly the
/// party entitled to retire it, and requiring anything else would mean an
/// unpair could not reach the relay at all.
///
/// A relay that has already forgotten the device answers success, so an unpair
/// is never blocked by a pairing the relay no longer has.
pub async fn revoke(credentials: &Credentials) -> Result<(), String> {
    let endpoint = normalize_endpoint(&credentials.endpoint)?;
    if credentials.device_id.trim().is_empty()
        || credentials.device_credential.len() < MINIMUM_CREDENTIAL_LENGTH
    {
        return Err("The stored relay credentials are invalid".into());
    }
    let mut url = Url::parse(&format!("{endpoint}{DEVICE_REVOKE_PATH}"))
        .map_err(|_| "The relay endpoint is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("deviceId", &credentials.device_id);
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&credentials.device_credential)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    // A credential the relay no longer recognizes is already withdrawn.
    if response.status().is_success() || matches!(response.status().as_u16(), 401 | 403 | 404) {
        return Ok(());
    }
    Err("QC Remote Cloud could not withdraw this pairing".into())
}

/// Begin a browser pairing flow without putting an owner credential in the
/// native app. The service authenticates the person who opens `pair_url`.
/// What a host declares about itself when it opens a pairing session.
///
/// The relay shows both on the confirmation page the person signs in from,
/// so they can tell their own computer's request from one someone sent them.
/// They are the host's own claim, labelled as such there, and the relay
/// bounds them the same way: label to 80 characters, platform to 32.
pub fn host_pairing_request_body(label: &str, platform: &str) -> Value {
    fn bounded(value: &str, limit: usize) -> Option<String> {
        let value: String = value
            .trim()
            .chars()
            .filter(|character| !character.is_control())
            .take(limit)
            .collect();
        (!value.is_empty()).then_some(value)
    }
    let mut body = serde_json::Map::new();
    if let Some(label) = bounded(label, 80) {
        body.insert("label".into(), Value::String(label));
    }
    if let Some(platform) = bounded(platform, 32) {
        body.insert("platform".into(), Value::String(platform));
    }
    Value::Object(body)
}

pub async fn begin_host_pairing(
    endpoint: &str,
    label: &str,
    platform: &str,
) -> Result<HostPairingSession, String> {
    let endpoint = normalize_endpoint(endpoint)?;
    let response = reqwest::Client::new()
        .post(format!("{endpoint}/v1/host/pairing-sessions"))
        .json(&host_pairing_request_body(label, platform))
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    if !response.status().is_success() {
        return Err("QC Remote Cloud could not start pairing".into());
    }
    let mut session: HostPairingSession = response
        .json()
        .await
        .map_err(|_| "QC Remote Cloud returned an invalid pairing session".to_string())?;
    session.endpoint = endpoint;
    if !session.pair_url.starts_with("https://")
        || session.session_id.len() < 20
        || session.receiver_secret.len() < MINIMUM_CREDENTIAL_LENGTH
        || session.protocol != PROTOCOL_VERSION
    {
        return Err("QC Remote Cloud returned an invalid pairing session".into());
    }
    Ok(session)
}

/// Poll a host-pairing session. `Ok(None)` means the browser sign-in is still
/// pending; `Ok(Some(code))` is passed straight into [`pair`] and never shown.
pub async fn poll_host_pairing(
    endpoint: &str,
    session: &HostPairingSession,
) -> Result<Option<String>, String> {
    let endpoint = normalize_endpoint(endpoint)?;
    if session.session_id.len() < 20 || session.receiver_secret.len() < MINIMUM_CREDENTIAL_LENGTH {
        return Err("The pairing session is invalid".into());
    }
    let response = reqwest::Client::new()
        .get(format!(
            "{endpoint}/v1/host/pairing-sessions/{}",
            session.session_id
        ))
        .bearer_auth(&session.receiver_secret)
        .send()
        .await
        .map_err(|_| "Could not reach QC Remote Cloud".to_string())?;
    match response.status().as_u16() {
        202 => Ok(None),
        200 => {
            let completed: HostPairingPoll = response
                .json()
                .await
                .map_err(|_| "QC Remote Cloud returned an invalid pairing result".to_string())?;
            if completed.state != "complete" {
                return Err("QC Remote Cloud returned an invalid pairing result".into());
            }
            let code = completed.pairing_code.filter(|code| {
                (PAIRING_CODE_MINIMUM_LENGTH..=PAIRING_CODE_MAXIMUM_LENGTH).contains(&code.len())
            });
            code.map(Some)
                .ok_or_else(|| "QC Remote Cloud returned an invalid pairing result".into())
        }
        404 | 410 => {
            Err("The browser pairing session expired. Start it again from QC Remote.".into())
        }
        _ => Err("QC Remote Cloud could not complete pairing".into()),
    }
}

/// Run the call after the initiating client has performed its confirmation.
///
/// `invoking` flips when the adapter owns the call, so the caller's heartbeat
/// knows whether asking the adapter for its readiness would queue behind this
/// request.
async fn invoke_with_deadline(
    adapter: Arc<dyn DeviceAdapter>,
    method: &str,
    params: Value,
    deadline_ms: Option<u64>,
    resource_read: bool,
    invoking: &AtomicBool,
) -> Result<Value, DeviceError> {
    invoking.store(true, AtomicOrdering::Release);
    let call = async {
        if resource_read {
            adapter.invoke_resource_read(method, params).await
        } else {
            adapter.invoke(method, params).await
        }
    };
    match tokio::time::timeout(adapter_invoke_timeout(deadline_ms), call).await {
        Ok(result) => result,
        Err(_) => Err(DeviceError::new(
            "DEVICE_TIMEOUT",
            "The local device adapter did not complete the request before its deadline",
            true,
        )),
    }
}

type CredentialStatusCallback = Arc<dyn Fn(&Credentials, bool) + Send + Sync>;
type ReplacementAckCallback = Arc<dyn Fn(&Credentials, &str) + Send + Sync>;

pub struct Client {
    credentials: Credentials,
    adapter: Arc<dyn DeviceAdapter>,
    gateway_api_version: Option<u64>,
    state: watch::Sender<ConnectionState>,
    access: watch::Receiver<AccessMode>,
    stop: watch::Receiver<bool>,
    completed: HashSet<String>,
    completed_order: VecDeque<String>,
    mcp_presence: Option<watch::Sender<Option<Vec<McpClientPresence>>>>,
    credential_status: Option<CredentialStatusCallback>,
    replacement_ack: Option<ReplacementAckCallback>,
}

impl Client {
    pub fn new(
        credentials: Credentials,
        adapter: Arc<dyn DeviceAdapter>,
        state: watch::Sender<ConnectionState>,
        access: watch::Receiver<AccessMode>,
        stop: watch::Receiver<bool>,
    ) -> Self {
        Self {
            credentials,
            adapter,
            gateway_api_version: None,
            state,
            access,
            stop,
            completed: HashSet::new(),
            completed_order: VecDeque::new(),
            mcp_presence: None,
            credential_status: None,
            replacement_ack: None,
        }
    }

    /// Advertise the gateway version so the relay can bound legacy allowances.
    pub fn with_gateway_api_version(mut self, version: u64) -> Self {
        self.gateway_api_version = Some(version);
        self
    }

    pub fn with_mcp_presence(
        mut self,
        presence: watch::Sender<Option<Vec<McpClientPresence>>>,
    ) -> Self {
        self.mcp_presence = Some(presence);
        self
    }

    /// Reports only authenticated connect outcomes; the host fences identity and connection generation.
    pub fn with_credential_status(mut self, callback: CredentialStatusCallback) -> Self {
        self.credential_status = Some(callback);
        self
    }

    pub fn with_replacement_ack(mut self, callback: ReplacementAckCallback) -> Self {
        self.replacement_ack = Some(callback);
        self
    }

    fn consume_replacement_ack(&self, text: &str) -> Result<bool, ()> {
        let frame: Value = serde_json::from_str(text).map_err(|_| ())?;
        if frame.get("type").and_then(Value::as_str)
            != Some(qc_relay_protocol::REPLACEMENT_ACK_FRAME_TYPE)
        {
            return Ok(false);
        }
        let previous = self.credentials.previous_credential.as_ref().ok_or(())?;
        if frame.get("previousDeviceId").and_then(Value::as_str)
            != Some(previous.device_id.as_str())
            || frame
                .get("previousCredentialAccepted")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err(());
        }
        if !*self.stop.borrow() {
            if let Some(callback) = &self.replacement_ack {
                callback(&self.credentials, &previous.device_id);
            }
        }
        Ok(true)
    }

    fn report_credential_status(&self, rejected: bool) {
        if *self.stop.borrow() {
            return;
        }
        if let Some(callback) = &self.credential_status {
            callback(&self.credentials, rejected);
        }
    }

    fn rejected_credential(&self) {
        if *self.stop.borrow() {
            return;
        }
        self.report_credential_status(true);
        self.state.send_replace(ConnectionState::PairingRequired);
    }

    pub async fn run(mut self) {
        let endpoint = match normalize_endpoint(&self.credentials.endpoint) {
            Ok(value) => value,
            Err(_) => {
                self.state.send_replace(ConnectionState::InvalidEndpoint);
                return;
            }
        };
        let mut url = Url::parse(&format!("{endpoint}{DEVICE_CONNECT_PATH}"))
            .expect("normalized relay origin");
        url.set_scheme("wss").expect("secure websocket scheme");
        url.query_pairs_mut()
            .append_pair("deviceId", &self.credentials.device_id);
        if self.credentials.previous_credential.is_some() {
            url.query_pairs_mut()
                .append_pair(qc_relay_protocol::REPLACEMENT_ACK_QUERY_PARAMETER, "1");
        }
        let ws_endpoint = url.to_string();
        let mut failures = 0_u32;
        loop {
            if *self.stop.borrow() {
                self.state.send_replace(ConnectionState::Stopped);
                return;
            }
            self.state.send_replace(if failures == 0 {
                ConnectionState::Connecting
            } else {
                ConnectionState::Reconnecting
            });
            let mut request = match ws_endpoint.clone().into_client_request() {
                Ok(value) => value,
                Err(_) => {
                    self.state.send_replace(ConnectionState::InvalidEndpoint);
                    return;
                }
            };
            let credential = match format!("Bearer {}", self.credentials.device_credential).parse()
            {
                Ok(value) => value,
                Err(_) => {
                    self.state.send_replace(ConnectionState::PairingRequired);
                    return;
                }
            };
            request.headers_mut().insert("Authorization", credential);
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static(PROTOCOL_VERSION),
            );
            let connection = tokio::select! {
                result = tokio::time::timeout(SOCKET_CONNECT_TIMEOUT, connect_async_with_config(request, Some(relay_socket_config()), false)) => result,
                changed = self.stop.changed() => {
                    if changed.is_err() || *self.stop.borrow() {
                        self.state.send_replace(ConnectionState::Stopped);
                        return;
                    }
                    continue;
                }
            };
            match connection {
                Ok(Ok((socket, response))) => {
                    if response
                        .headers()
                        .get("Sec-WebSocket-Protocol")
                        .and_then(|value| value.to_str().ok())
                        != Some(PROTOCOL_VERSION)
                    {
                        self.state.send_replace(ConnectionState::InvalidEndpoint);
                        return;
                    }
                    self.report_credential_status(false);
                    self.state.send_replace(ConnectionState::Connected);
                    if self.connected(socket).await.is_err() && !*self.stop.borrow() {
                        // Escalate rather than reset. A relay that accepts the
                        // upgrade and then drops, or answers with one malformed
                        // frame, used to pin `failures` at 1 for ever and
                        // reconnect every two seconds indefinitely, because the
                        // reset above ran on every successful handshake.
                        failures = failures.saturating_add(1).clamp(1, MAXIMUM_FAILURE_COUNT);
                    } else {
                        failures = 0;
                    }
                    if *self.state.borrow() == ConnectionState::PairingRequired {
                        return;
                    }
                }
                Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
                    if qc_relay_protocol::pairing_rejection_http(response.status().as_u16()) =>
                {
                    self.rejected_credential();
                    return;
                }
                Ok(Err(_)) | Err(_) => {
                    failures = failures.saturating_add(1).clamp(1, MAXIMUM_FAILURE_COUNT)
                }
            }
            if *self.stop.borrow() {
                self.state.send_replace(ConnectionState::Stopped);
                return;
            }
            self.state.send_replace(ConnectionState::Reconnecting);
            let base = 1_u64 << failures.min(MAXIMUM_BACKOFF_EXPONENT);
            let delay = Duration::from_millis(
                base.min(MAXIMUM_BACKOFF_SECONDS) * 1000
                    + rand::rng().random_range(0..BACKOFF_JITTER_MS),
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                changed = self.stop.changed() => if changed.is_err() || *self.stop.borrow() { self.state.send_replace(ConnectionState::Stopped); return; }
            }
        }
    }

    async fn connected<S>(
        &mut self,
        socket: tokio_tungstenite::WebSocketStream<S>,
    ) -> Result<(), ()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (mut sink, mut stream) = socket.split();
        let _presence_lifetime = PresenceLifetime(self.mcp_presence.clone());
        let mut readiness = tokio::time::interval(Duration::from_millis(READINESS_INTERVAL_MS));
        let mut deferred = VecDeque::new();
        loop {
            tokio::select! {
                _ = readiness.tick() => {
                    let adapter = Arc::clone(&self.adapter);
                    let ready = tokio::select! {
                        result = tokio::time::timeout(ADAPTER_READY_TIMEOUT, adapter.ready()) => result.unwrap_or(false),
                        changed = self.stop.changed() => {
                            if changed.is_err() || *self.stop.borrow() {
                                let _ = send_close(&mut sink).await;
                                return Ok(());
                            }
                            false
                        }
                    };
                    let frame = DeviceFrame::Ready { protocol: PROTOCOL_VERSION.into(), usb_connected: ready, mcp_presence: self.mcp_presence.is_some(), gateway_api_version: self.gateway_api_version };
                    if send_frame(&mut sink, &frame).await.is_err() { return Err(()); }
                }
                changed = self.stop.changed() => {
                    if changed.is_err() || *self.stop.borrow() {
                        let _ = send_close(&mut sink).await;
                        return Ok(());
                    }
                }
                message = async { match deferred.pop_front() { Some(message) => Some(Ok(message)), None => stream.next().await } } => match message {
                    Some(Ok(Message::Text(text))) if text.len() <= MAX_REQUEST_FRAME_BYTES => {
                        if self.consume_replacement_ack(&text)? { continue; }
                        let frame = serde_json::from_str(&text).map_err(|_| ())?;
                        if let DeviceFrame::McpPresence { clients } = frame {
                            if let Some(presence) = &self.mcp_presence {
                                if !valid_mcp_presence(&clients) { return Err(()); }
                                presence.send_replace(Some(clients));
                            }
                            continue;
                        }
                        let DeviceFrame::Invoke { id, action: _, method, params, deadline_ms, origin } = frame else { return Err(()); };
                        // Only narrows: the relay's MCP ingress marks a resource read.
                        let resource_read = origin.as_deref() == Some(qc_relay_protocol::RESOURCE_READ_ORIGIN);
                        let response = if id.is_empty() || id.len() > 128 {
                            DeviceFrame::Result { id, ok: false, result: None, error: Some(DeviceError::new("INVALID_REQUEST", qc_relay_protocol::INVALID_REQUEST_ID_TEXT, false)) }
                        } else if !self.completed.insert(id.clone()) {
                            DeviceFrame::Result { id, ok: false, result: None, error: Some(DeviceError::new("REPLAYED_REQUEST", qc_relay_protocol::REPLAYED_REQUEST_TEXT, false)) }
                        } else {
                            self.completed_order.push_back(id.clone());
                            if self.completed_order.len() > COMPLETED_REQUEST_CACHE_SIZE { if let Some(old) = self.completed_order.pop_front() { self.completed.remove(&old); } }
                            let result = if !self.adapter.remote_session_enabled() {
                                Err(DeviceError::new(
                                    "REMOTE_SESSION_DISABLED",
                                    "Public relay access has been disabled on this device",
                                    false,
                                ))
                            } else if !generated_actions::contains(&method) {
                                Err(DeviceError::new("METHOD_NOT_ALLOWED", qc_relay_protocol::METHOD_NOT_ALLOWED_TEXT, false))
                            } else if !self
                                .adapter
                                .effective_access_mode()
                                .unwrap_or(*self.access.borrow())
                                .permits(&method)
                            {
                                Err(DeviceError::new("ACCESS_MODE_RESTRICTED", qc_relay_protocol::ACCESS_MODE_RESTRICTED_TEXT, false))
                            } else {
                                // The device call is polled beside the
                                // heartbeat, pongs and stop signal so long
                                // persistent writes do not make the relay drop
                                // an otherwise active connection.
                                let invoking = AtomicBool::new(false);
                                let work = invoke_with_deadline(
                                    Arc::clone(&self.adapter),
                                    &method,
                                    params,
                                    deadline_ms,
                                    resource_read,
                                    &invoking,
                                );
                                tokio::pin!(work);
                                // Keep the readiness heartbeat running while the
                                // device works. A library write legitimately
                                // takes minutes, and this loop used to fall
                                // silent for all of it, so the relay and every
                                // intermediary between us saw an idle socket
                                // during exactly the operations that must not be
                                // interrupted. While the device works, readiness
                                // is asserted rather than re-read: asking the
                                // adapter would queue behind the call being
                                // waited on.
                                loop {
                                    tokio::select! {
                                        outcome = &mut work => break outcome,
                                        _ = readiness.tick() => {
                                            let usb_connected = if invoking.load(AtomicOrdering::Acquire) {
                                                true
                                            } else {
                                                tokio::time::timeout(ADAPTER_READY_TIMEOUT, self.adapter.ready())
                                                    .await
                                                    .unwrap_or(false)
                                            };
                                            let frame = DeviceFrame::Ready {
                                                protocol: PROTOCOL_VERSION.into(),
                                                usb_connected,
                                                mcp_presence: self.mcp_presence.is_some(),
                                                gateway_api_version: self.gateway_api_version,
                                            };
                                            if send_frame(&mut sink, &frame).await.is_err() {
                                                return Err(());
                                            }
                                        }
                                        incoming = stream.next() => {
                                            match incoming {
                                                Some(Ok(Message::Text(text))) if text.len() <= MAX_REQUEST_FRAME_BYTES => {
                                                    if self.consume_replacement_ack(&text)? { continue; }
                                                    match serde_json::from_str::<DeviceFrame>(&text).map_err(|_| ())? {
                                                        DeviceFrame::McpPresence { clients } => {
                                                            if !valid_mcp_presence(&clients) { return Err(()); }
                                                            if let Some(presence)=&self.mcp_presence { presence.send_replace(Some(clients)); }
                                                        }
                                                        DeviceFrame::Invoke { .. } if deferred.len() < 16 => deferred.push_back(Message::Text(text)),
                                                        _ => return Err(()),
                                                    }
                                                }
                                                Some(Ok(Message::Ping(value))) => { if send_message(&mut sink,Message::Pong(value)).await.is_err() { return Err(()); } }
                                                Some(Ok(Message::Pong(_))) => {}
                                                _ => return Err(()),
                                            }
                                        }
                                        changed = self.stop.changed() => {
                                            if changed.is_err() || *self.stop.borrow() {
                                                let _ = send_close(&mut sink).await;
                                                return Ok(());
                                            }
                                            break Err(DeviceError::new(
                                                "DEVICE_INTERRUPTED",
                                                "The local device request was interrupted",
                                                true,
                                            ));
                                        }
                                    }
                                }
                            };
                            match result {
                                Ok(value) => DeviceFrame::Result { id, ok: true, result: Some(value), error: None },
                                Err(error) => DeviceFrame::Result { id, ok: false, result: None, error: Some(error) },
                            }
                        };
                        if send_frame(&mut sink, &response).await.is_err() { return Err(()); }
                    }
                    Some(Ok(Message::Ping(value))) => if send_message(&mut sink, Message::Pong(value)).await.is_err() { return Err(()); },
                    Some(Ok(Message::Close(Some(frame)))) if qc_relay_protocol::pairing_rejection_close(u16::from(frame.code)) => {
                        self.rejected_credential();
                        return Ok(());
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(()),
                    Some(Ok(Message::Text(_))) => { let _ = send_close(&mut sink).await; return Err(()); }
                    _ => {}
                }
            }
        }
    }
}

struct PresenceLifetime(Option<watch::Sender<Option<Vec<McpClientPresence>>>>);
impl Drop for PresenceLifetime {
    fn drop(&mut self) {
        if let Some(sender) = &self.0 {
            sender.send_replace(None);
        }
    }
}
fn valid_mcp_presence(clients: &[McpClientPresence]) -> bool {
    let mut ids = HashSet::new();
    clients.len() <= 128
        && clients.iter().all(|row| {
            !row.id.is_empty()
                && row.id.len() <= 128
                && ids.insert(&row.id)
                && !row.name.trim().is_empty()
                && row.name.len() <= 128
                && !row.name.chars().any(char::is_control)
        })
}

async fn send_frame<S>(sink: &mut S, frame: &DeviceFrame) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let text = serde_json::to_string(frame).map_err(|_| ())?;
    if text.len() > MAX_RESULT_FRAME_BYTES {
        return Err(());
    }
    send_message(sink, Message::Text(text.into())).await
}

async fn send_message<S>(sink: &mut S, message: Message) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    tokio::time::timeout(SOCKET_SEND_TIMEOUT, sink.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn send_close<S>(sink: &mut S) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    send_message(sink, Message::Close(None)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn a_not_found_revoke_is_told_apart_from_a_confirmed_one() {
        assert_eq!(connector_revoke_outcome(200), Ok(ConnectorRevoke::Revoked));
        assert_eq!(connector_revoke_outcome(204), Ok(ConnectorRevoke::Revoked));
        assert_eq!(connector_revoke_outcome(404), Ok(ConnectorRevoke::NotFound));
        assert!(connector_revoke_outcome(401)
            .unwrap_err()
            .contains("no longer recognizes"));
        assert_eq!(
            connector_revoke_outcome(503),
            Err("QC Remote Cloud could not unlink that connector".into())
        );
    }

    #[test]
    fn mcp_presence_rejects_duplicate_or_malformed_identities_and_clears_on_disconnect() {
        let row = McpClientPresence {
            id: "one".into(),
            name: "Claude".into(),
            grant_id: None,
            version: None,
            device: None,
            browser: None,
            ip_address: None,
            country: None,
            city: None,
            connected_at_unix_ms: 108,
            last_seen_at_unix_ms: 118,
            expires_at_unix_ms: 123,
        };
        assert!(valid_mcp_presence(std::slice::from_ref(&row)));
        assert!(!valid_mcp_presence(&[row.clone(), row.clone()]));
        let mut bad = row.clone();
        bad.name = "\nClaude".into();
        assert!(!valid_mcp_presence(&[bad]));
        let (sender, receiver) = watch::channel(Some(vec![row]));
        {
            let _lifetime = PresenceLifetime(Some(sender));
        }
        assert!(receiver.borrow().is_none());
    }

    #[test]
    fn deletions_are_confirmed_by_the_initiating_client_not_the_paired_host() {
        assert!(HOST_APPROVAL_METHODS.is_empty());
        for rpc in ["device.deletePreset", "device.deleteSetlist"] {
            assert!(
                generated_actions::contains(rpc),
                "{rpc} left the relay allowlist"
            );
            assert!(
                generated_actions::requires_confirmation(rpc),
                "{rpc} lost its initiating-client confirmation contract"
            );
        }
        assert!(generated_actions::contains("device.removeBlock"));
    }

    #[test]
    fn the_socket_refuses_frames_larger_than_a_relay_request() {
        let config = relay_socket_config();
        assert_eq!(config.max_message_size, Some(MAX_REQUEST_FRAME_BYTES));
        assert_eq!(config.max_frame_size, Some(MAX_REQUEST_FRAME_BYTES));
        assert!(MAX_REQUEST_FRAME_BYTES < WebSocketConfig::default().max_frame_size.unwrap());
    }

    struct CountingAdapter(AtomicUsize);

    #[async_trait]
    impl DeviceAdapter for CountingAdapter {
        async fn ready(&self) -> bool {
            true
        }
        async fn invoke(&self, _method: &str, _params: Value) -> Result<Value, DeviceError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"deleted": true}))
        }
    }

    struct EffectivePolicyAdapter {
        mode: AtomicUsize,
        remote_enabled: AtomicBool,
        invocations: AtomicUsize,
    }

    impl EffectivePolicyAdapter {
        fn new(mode: AccessMode) -> Self {
            Self {
                mode: AtomicUsize::new(Self::encode(mode)),
                remote_enabled: AtomicBool::new(true),
                invocations: AtomicUsize::new(0),
            }
        }

        fn encode(mode: AccessMode) -> usize {
            match mode {
                AccessMode::Full => 0,
                AccessMode::Modify => 1,
                AccessMode::Performance => 2,
                AccessMode::ReadOnly => 3,
            }
        }

        fn set_mode(&self, mode: AccessMode) {
            self.mode.store(Self::encode(mode), Ordering::SeqCst);
        }

        fn set_remote_enabled(&self, enabled: bool) {
            self.remote_enabled.store(enabled, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl DeviceAdapter for EffectivePolicyAdapter {
        async fn ready(&self) -> bool {
            true
        }

        async fn invoke(&self, _method: &str, _params: Value) -> Result<Value, DeviceError> {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"accepted": true}))
        }

        fn effective_access_mode(&self) -> Option<AccessMode> {
            Some(match self.mode.load(Ordering::SeqCst) {
                0 => AccessMode::Full,
                1 => AccessMode::Modify,
                2 => AccessMode::Performance,
                _ => AccessMode::ReadOnly,
            })
        }

        fn remote_session_enabled(&self) -> bool {
            self.remote_enabled.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn authoritative_policy_revokes_and_relaxes_without_waiting_for_watch() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
        async fn result_frame<S>(relay: &mut S, id: &str) -> Value
        where
            S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
                + Unpin,
        {
            loop {
                if let Message::Text(text) = next_frame(relay).await {
                    let frame: Value = serde_json::from_str(&text).expect("response frame");
                    if frame["type"] == "result" && frame["id"] == id {
                        return frame;
                    }
                }
            }
        }
        let (host_io, relay_io) = tokio::io::duplex(1 << 20);
        let adapter = Arc::new(EffectivePolicyAdapter::new(AccessMode::ReadOnly));
        let (state, _) = watch::channel(ConnectionState::Connected);
        // Deliberately stale in both directions: Full while the policy has
        // been restricted, then ReadOnly after the policy has been relaxed.
        let (access_tx, access) = watch::channel(AccessMode::Full);
        let (stop_tx, stop) = watch::channel(false);
        let credentials: Credentials = serde_json::from_value(json!({
            "endpoint": "https://relay.example.test",
            "deviceCredential": "c".repeat(MINIMUM_CREDENTIAL_LENGTH),
            "deviceId": "device"
        }))
        .expect("credentials");
        let mut client = Client::new(credentials, adapter.clone(), state, access, stop);
        let host = tokio::spawn(async move {
            let socket = WebSocketStream::from_raw_socket(host_io, Role::Client, None).await;
            client.connected(socket).await
        });
        let mut relay = WebSocketStream::from_raw_socket(relay_io, Role::Server, None).await;
        let invoke = |id: &str| {
            Message::Text(
                json!({
                    "type": "invoke",
                    "id": id,
                    "action": "set_tempo",
                    "method": "device.setTempo",
                    "params": {}
                })
                .to_string()
                .into(),
            )
        };
        relay.send(invoke("restricted")).await.unwrap();
        let restricted = result_frame(&mut relay, "restricted").await;
        assert_eq!(restricted["ok"], false);
        assert_eq!(restricted["error"]["code"], "ACCESS_MODE_RESTRICTED");
        assert_eq!(adapter.invocations.load(Ordering::SeqCst), 0);

        adapter.set_mode(AccessMode::Full);
        access_tx.send_replace(AccessMode::ReadOnly);
        relay.send(invoke("relaxed")).await.unwrap();
        let relaxed = result_frame(&mut relay, "relaxed").await;
        assert_eq!(relaxed["ok"], true);
        assert_eq!(adapter.invocations.load(Ordering::SeqCst), 1);

        // The connected Client still holds its original Credentials and the
        // watch is permissive. A local unpair latch must deny new calls anyway.
        access_tx.send_replace(AccessMode::Full);
        adapter.set_mode(AccessMode::Full);
        adapter.set_remote_enabled(false);
        relay.send(invoke("unpaired")).await.unwrap();
        let unpaired = result_frame(&mut relay, "unpaired").await;
        assert_eq!(unpaired["ok"], false);
        assert_eq!(unpaired["error"]["code"], "REMOTE_SESSION_DISABLED");
        assert_eq!(adapter.invocations.load(Ordering::SeqCst), 1);

        // Editing access mode must not reactivate the persisted remote latch.
        adapter.set_mode(AccessMode::ReadOnly);
        access_tx.send_replace(AccessMode::Full);
        relay
            .send(invoke("mode-change-does-not-reactivate"))
            .await
            .unwrap();
        let still_disabled = result_frame(&mut relay, "mode-change-does-not-reactivate").await;
        assert_eq!(still_disabled["ok"], false);
        assert_eq!(still_disabled["error"]["code"], "REMOTE_SESSION_DISABLED");
        assert_eq!(adapter.invocations.load(Ordering::SeqCst), 1);

        // Only explicit successful pairing re-enables the same live adapter.
        adapter.set_remote_enabled(true);
        adapter.set_mode(AccessMode::Full);
        relay.send(invoke("paired-again")).await.unwrap();
        let paired_again = result_frame(&mut relay, "paired-again").await;
        assert_eq!(paired_again["ok"], true);
        assert_eq!(adapter.invocations.load(Ordering::SeqCst), 2);

        stop_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), host)
            .await
            .expect("the host did not stop")
            .unwrap()
            .expect("connected loop failed");
    }

    #[tokio::test]
    async fn lost_post_upgrade_ack_retains_fallback_and_explicit_ack_reaches_the_real_frame_loop() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
        // The cloud transaction is outside this fixture. Duplex streams model an
        // already-authenticated 101 whose first ACK is delivered or lost.
        for deliver_ack in [false, true] {
            let mut credentials: Credentials = serde_json::from_value(json!({
                "endpoint":"https://qcrc.freevia.org","deviceId":"fixture-new",
                "deviceCredential":"new-fixture-credential","connectionGeneration":2
            }))
            .unwrap();
            let mut previous = credentials.clone();
            previous.device_id = "fixture-old".into();
            credentials.previous_credential = Some(Box::new(previous));
            let (host_io, relay_io) = tokio::io::duplex(8192);
            let (state, _) = watch::channel(ConnectionState::Connected);
            let (_access_tx, access) = watch::channel(AccessMode::Full);
            let (_stop_tx, stop) = watch::channel(false);
            let (ack_tx, mut ack_rx) = watch::channel(false);
            let mut client = Client::new(
                credentials,
                Arc::new(CountingAdapter(AtomicUsize::new(0))),
                state,
                access,
                stop,
            )
            .with_replacement_ack(Arc::new(move |identity, prior| {
                assert_eq!(identity.device_id, "fixture-new");
                assert_eq!(prior, "fixture-old");
                ack_tx.send_replace(true);
            }));
            let host = tokio::spawn(async move {
                let socket = WebSocketStream::from_raw_socket(host_io, Role::Client, None).await;
                let result = client.connected(socket).await;
                (result, client.credentials.previous_credential.is_some())
            });
            let mut relay = WebSocketStream::from_raw_socket(relay_io, Role::Server, None).await;
            if deliver_ack {
                relay.send(Message::Text(json!({"type":qc_relay_protocol::REPLACEMENT_ACK_FRAME_TYPE,"previousDeviceId":"fixture-old","previousCredentialAccepted":false}).to_string().into())).await.unwrap();
                tokio::time::timeout(Duration::from_secs(5), ack_rx.changed())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(*ack_rx.borrow());
            }
            relay.send(Message::Close(None)).await.unwrap();
            let (_, retained) = tokio::time::timeout(Duration::from_secs(5), host)
                .await
                .unwrap()
                .unwrap();
            assert!(retained,"the client snapshot never discards credentials; durable host storage owns acknowledgement");
            assert_eq!(
                *ack_rx.borrow(),
                deliver_ack,
                "an upgrade or lost ACK must not become cleanup evidence"
            );
        }
    }

    #[test]
    fn replacement_ack_is_explicit_bound_and_ignored_after_stop() {
        let mut current: Credentials = serde_json::from_value(json!({
            "endpoint":"https://qcrc.freevia.org","deviceId":"fixture-new",
            "deviceCredential":"new-fixture-credential","connectionGeneration":2
        }))
        .unwrap();
        let mut previous = current.clone();
        previous.device_id = "fixture-old".into();
        current.previous_credential = Some(Box::new(previous));
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let (state, _) = watch::channel(ConnectionState::Connected);
        let (_access_tx, access) = watch::channel(AccessMode::Full);
        let (stop_tx, stop) = watch::channel(false);
        let client = Client::new(
            current,
            Arc::new(CountingAdapter(AtomicUsize::new(0))),
            state,
            access,
            stop,
        )
        .with_replacement_ack(Arc::new(move |identity, prior| {
            assert_eq!(identity.device_id, "fixture-new");
            assert_eq!(identity.connection_generation, 2);
            assert_eq!(prior, "fixture-old");
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        client.report_credential_status(false);
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "a handshake alone does not acknowledge replacement"
        );
        let valid = json!({"type":qc_relay_protocol::REPLACEMENT_ACK_FRAME_TYPE,"previousDeviceId":"fixture-old","previousCredentialAccepted":false});
        let mut wrong = valid.clone();
        wrong["previousDeviceId"] = json!("other-device");
        assert!(client.consume_replacement_ack(&wrong.to_string()).is_err());
        wrong = valid.clone();
        wrong["previousCredentialAccepted"] = json!("false");
        assert!(client.consume_replacement_ack(&wrong.to_string()).is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert_eq!(client.consume_replacement_ack(&valid.to_string()), Ok(true));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        stop_tx.send_replace(true);
        assert_eq!(client.consume_replacement_ack(&valid.to_string()), Ok(true));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn malformed_local_header_and_stopped_client_cannot_prove_server_rejection() {
        let credentials: Credentials = serde_json::from_value(json!({
            "endpoint":"https://qcrc.freevia.org", "deviceId":"fixture-device",
            "deviceCredential":"invalid\nheader", "identityProvider":"google"
        }))
        .unwrap();
        let callbacks = Arc::new(AtomicUsize::new(0));
        let counter = callbacks.clone();
        let (state, _) = watch::channel(ConnectionState::Stopped);
        let (_access_tx, access) = watch::channel(AccessMode::Full);
        let (_stop_tx, stop) = watch::channel(false);
        let client = Client::new(
            credentials.clone(),
            Arc::new(CountingAdapter(AtomicUsize::new(0))),
            state,
            access,
            stop,
        )
        .with_credential_status(Arc::new(move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        client.run().await; // Header construction fails before opening a socket.
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        let counter = callbacks.clone();
        let (state, _) = watch::channel(ConnectionState::Stopped);
        let (_access_tx, access) = watch::channel(AccessMode::Full);
        let (_stop_tx, stop) = watch::channel(true);
        let client = Client::new(
            credentials,
            Arc::new(CountingAdapter(AtomicUsize::new(0))),
            state,
            access,
            stop,
        )
        .with_credential_status(Arc::new(move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        client.rejected_credential();
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    }

    async fn next_frame<S>(relay: &mut S) -> Message
    where
        S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(5), relay.next())
            .await
            .expect("the host fell silent")
            .expect("the host closed the socket")
            .expect("the socket failed")
    }

    #[tokio::test]
    async fn confirmed_remote_delete_dispatches_without_a_second_host_prompt() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
        let (host_io, relay_io) = tokio::io::duplex(1 << 20);
        let adapter = Arc::new(CountingAdapter(AtomicUsize::new(0)));
        let (state, _) = watch::channel(ConnectionState::Connected);
        let (_access_tx, access) = watch::channel(AccessMode::Full);
        let (stop_tx, stop) = watch::channel(false);
        let credentials = Credentials {
            credential_rejected: false,
            connection_generation: 0,
            previous_credential: None,
            endpoint: "https://relay.example.test".into(),
            device_credential: "c".repeat(MINIMUM_CREDENTIAL_LENGTH),
            device_id: "device".into(),
            identity_provider: None,
            pairing: None,
        };
        let mut client = Client::new(credentials, adapter.clone(), state, access, stop)
            .with_gateway_api_version(31);
        let host = tokio::spawn(async move {
            let socket = WebSocketStream::from_raw_socket(host_io, Role::Client, None).await;
            client.connected(socket).await
        });
        let mut relay = WebSocketStream::from_raw_socket(relay_io, Role::Server, None).await;
        let invoke = json!({
            "type": "invoke", "id": "r1", "action": "delete_preset",
            "method": "device.deletePreset", "params": {"setlistKey":"/media/p4/Presets/User", "position": 3, "expectedName":"Target"}
        });
        relay
            .send(Message::Text(invoke.to_string().into()))
            .await
            .unwrap();
        let mut advertised_api_version = false;
        let mut received_result = false;
        // Readiness and replies are independent streams: either may arrive first.
        while !advertised_api_version || !received_result {
            if let Message::Text(text) = next_frame(&mut relay).await {
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["type"] == "ready" {
                    assert_eq!(frame["gatewayApiVersion"], 31);
                    advertised_api_version = true;
                }
                if frame["type"] == "result" {
                    assert_eq!(frame["id"], "r1");
                    assert_eq!(frame["ok"], true);
                    assert!(!received_result, "the request returned more than once");
                    received_result = true;
                }
            }
        }
        assert!(
            advertised_api_version,
            "the host did not advertise its gateway API version"
        );
        assert_eq!(adapter.0.load(Ordering::SeqCst), 1);
        stop_tx.send(true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), host)
            .await
            .expect("stop was not serviced")
            .unwrap();
        assert_eq!(outcome, Ok(()));
    }

    #[tokio::test]
    async fn initiating_client_confirmation_contract_covers_risky_actions() {
        for rpc in [
            "device.deletePreset",
            "device.deleteSetlist",
            "device.createBackup",
            "device.savePresetAs",
            "device.reloadPreset",
            "device.reconnect",
            "device.resetSession",
            "device.tapScreen",
            "device.swipeScreen",
            "device.dragScreen",
            "device.mouseFrame",
            "device.setMasterVolume",
            "device.setTunerMute",
            "device.showTuner",
            "device.setSplitMute",
            "device.setDeviceName",
        ] {
            assert!(
                generated_actions::requires_confirmation(rpc),
                "{rpc} lost its initiating-client confirmation contract"
            );
        }
    }

    #[test]
    fn a_pairing_session_declares_the_host_as_the_relay_bounds_it() {
        assert_eq!(
            host_pairing_request_body("QC Remote on Windows", "windows"),
            json!({"label": "QC Remote on Windows", "platform": "windows"})
        );
        let long = host_pairing_request_body(&"n".repeat(200), &"p".repeat(50));
        assert_eq!(long["label"].as_str().unwrap().chars().count(), 80);
        assert_eq!(long["platform"].as_str().unwrap().chars().count(), 32);
        assert_eq!(
            host_pairing_request_body(" Studio\nPC ", ""),
            json!({"label": "StudioPC"})
        );
        assert_eq!(host_pairing_request_body("  ", "\t"), json!({}));
    }

    #[test]
    fn endpoint_policy_matches_mobile_security_boundary() {
        assert_eq!(
            normalize_endpoint(" https://relay.example.test/ ").unwrap(),
            "https://relay.example.test"
        );
        for invalid in [
            "http://relay.example.test",
            "https://credential@localhost",
            "https://relay.example.test/tenant",
            "https://relay.example.test?q=1",
        ] {
            assert!(normalize_endpoint(invalid).is_err());
        }
    }

    #[test]
    fn generated_policy_covers_reads_and_writes() {
        assert!(generated_actions::contains("device.currentPreset"));
        assert!(!generated_actions::contains("device.snapshot"));
        assert!(generated_actions::contains("device.setChainSplit"));
        assert!(generated_actions::is_read_only("device.listModels"));
        assert!(AccessMode::Performance.permits("device.setTempo"));
        assert!(!AccessMode::Performance.permits("device.setParameter"));
        assert!(AccessMode::Modify.permits("device.setParameter"));
        assert!(!AccessMode::Modify.permits("device.setDeviceName"));
        assert!(AccessMode::Full.permits("device.setDeviceName"));
        assert!(!generated_actions::is_read_only("device.setTempo"));
        assert!(!generated_actions::contains("device.command.operation"));
    }

    #[test]
    fn replacement_body_is_bound_to_the_held_origin_and_known_owner() {
        let mut held = Credentials {
            endpoint: "https://qcrc.freevia.org".into(),
            device_id: "fixture-device".into(),
            device_credential: "x".repeat(32),
            identity_provider: Some(IdentityProvider::Google),
            credential_rejected: false,
            connection_generation: 0,
            previous_credential: None,
            pairing: Some(PairingMetadata {
                email: "owner@example.test".into(),
                device: "fixture".into(),
                paired_at: 1,
            }),
        };
        let request = pairing_request(
            &reqwest::Client::new(),
            &held.endpoint,
            "fixture-code",
            "fixture",
            Some(&held),
        )
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            format!("{}{DEVICE_REPLACEMENT_PATH}", held.endpoint)
        );
        let body: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["previousDeviceId"], held.device_id);
        assert_eq!(body["previousDeviceCredential"], held.device_credential);
        assert!(pairing_request_body(
            "https://other.example",
            "fixture-code",
            "fixture",
            Some(&held)
        )
        .is_err());
        held.pairing = None;
        assert!(
            pairing_request_body(&held.endpoint, "fixture-code", "fixture", Some(&held)).is_err()
        );
        held.credential_rejected = true;
        assert!(
            pairing_request_body(&held.endpoint, "fixture-code", "fixture", Some(&held))
                .unwrap()
                .get("previousDeviceCredential")
                .is_some()
        );
        let ordinary = pairing_request(
            &reqwest::Client::new(),
            &held.endpoint,
            "fixture-code",
            "fixture",
            None,
        )
        .unwrap();
        assert_eq!(
            ordinary.url().as_str(),
            format!("{}{DEVICE_PAIR_PATH}", held.endpoint)
        );
        assert!(pairing_http_error(404, true)
            .unwrap()
            .contains("PAIRING_REPLACEMENT_UNSUPPORTED"));
        assert!(!pairing_http_error(404, false)
            .unwrap()
            .contains("PAIRING_REPLACEMENT_UNSUPPORTED"));
        let fresh = pairing_request_body(&held.endpoint, "fixture-code", "fixture", None).unwrap();
        assert!(fresh.get("previousDeviceId").is_none());
        assert!(fresh.get("previousDeviceCredential").is_none());
    }

    #[test]
    fn shared_owner_policy_never_uses_connection_ui_state_as_proof() {
        assert_eq!(
            pairing_owner_refusal(true, "owner@example.test", "other@example.test", false),
            Some("PAIRING_OWNER_CHANGED")
        );
        assert_eq!(
            pairing_owner_refusal(true, "", "owner@example.test", false),
            Some("PAIRING_OWNER_UNKNOWN")
        );
        assert_eq!(
            pairing_owner_refusal(true, "owner@example.test", "", false),
            Some("PAIRING_OWNER_UNVERIFIED")
        );
        assert_eq!(
            pairing_owner_refusal(true, "owner@example.test", "other@example.test", true),
            None
        );
        assert!(!qc_relay_protocol::pairing_rejection_http(500));
        assert!(!qc_relay_protocol::pairing_rejection_close(1000));
    }

    #[test]
    fn legacy_credentials_require_a_google_repair() {
        let legacy: Credentials = serde_json::from_str(
            r#"{
            "endpoint":"https://qcrc.freevia.org",
            "deviceCredential":"legacy-credential",
            "deviceId":"legacy-device"
        }"#,
        )
        .unwrap();
        assert!(!legacy.uses_current_identity_provider());

        let current: Credentials = serde_json::from_str(
            r#"{
            "endpoint":"https://qcrc.freevia.org",
            "deviceCredential":"current-credential",
            "deviceId":"current-device",
            "identityProvider":"google"
        }"#,
        )
        .unwrap();
        assert!(current.uses_current_identity_provider());
    }

    /// Readiness the host can only learn by asking: whatever `ready()`
    /// answers. The paired host's adapter reads it from the broker while no
    /// MCP client is attached, so nothing here is event-driven.
    struct FlippingAdapter(AtomicBool);

    #[async_trait]
    impl DeviceAdapter for FlippingAdapter {
        async fn ready(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        async fn invoke(&self, _method: &str, _params: Value) -> Result<Value, DeviceError> {
            Err(DeviceError::new("UNUSED", "no invoke in this test", false))
        }
    }

    /// The cloud's device status is the last `usb_connected` this loop sent.
    /// It must follow the adapter's readiness both ways with no invoke, no
    /// presence change and no other event: a paired host with no MCP client
    /// attached sends nothing else, so a readiness change it only reported on
    /// an event would never reach the cloud.
    #[tokio::test]
    async fn the_ready_frame_follows_readiness_changes_with_no_other_event() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
        let (host_io, relay_io) = tokio::io::duplex(1 << 20);
        let adapter = Arc::new(FlippingAdapter(AtomicBool::new(false)));
        let (state, _) = watch::channel(ConnectionState::Connected);
        let (_access_tx, access) = watch::channel(AccessMode::Full);
        let (stop_tx, stop) = watch::channel(false);
        let credentials = Credentials {
            credential_rejected: false,
            connection_generation: 0,
            previous_credential: None,
            endpoint: "https://relay.example.test".into(),
            device_credential: "c".repeat(MINIMUM_CREDENTIAL_LENGTH),
            device_id: "device".into(),
            identity_provider: None,
            pairing: None,
        };
        let mut client = Client::new(credentials, adapter.clone(), state, access, stop);
        let host = tokio::spawn(async move {
            let socket = WebSocketStream::from_raw_socket(host_io, Role::Client, None).await;
            client.connected(socket).await
        });
        let mut relay = WebSocketStream::from_raw_socket(relay_io, Role::Server, None).await;
        async fn next_ready<S>(relay: &mut S, wanted: bool)
        where
            S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
                + Unpin,
        {
            // A frame already in flight may carry the previous value; the
            // next one after it must carry the new one.
            for _ in 0..3 {
                if let Message::Text(text) = next_frame(relay).await {
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    if frame["type"] == "ready" && frame["usbConnected"] == wanted {
                        return;
                    }
                }
            }
            panic!("no ready frame carried usbConnected={wanted}");
        }
        next_ready(&mut relay, false).await;
        adapter.0.store(true, Ordering::SeqCst);
        next_ready(&mut relay, true).await;
        adapter.0.store(false, Ordering::SeqCst);
        next_ready(&mut relay, false).await;
        stop_tx.send(true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), host)
            .await
            .expect("stop was not serviced")
            .unwrap();
        assert_eq!(outcome, Ok(()));
    }

    /// Records which entry point each call arrived through.
    struct OriginRecordingAdapter(std::sync::Mutex<Vec<String>>);

    #[async_trait]
    impl DeviceAdapter for OriginRecordingAdapter {
        async fn ready(&self) -> bool {
            true
        }
        async fn invoke(&self, method: &str, _params: Value) -> Result<Value, DeviceError> {
            self.0.lock().unwrap().push(format!("invoke {method}"));
            Ok(json!({}))
        }
        async fn invoke_resource_read(
            &self,
            method: &str,
            _params: Value,
        ) -> Result<Value, DeviceError> {
            self.0
                .lock()
                .unwrap()
                .push(format!("resource read {method}"));
            Ok(json!({}))
        }
    }

    /// An `invoke` frame the relay marked as
    /// an MCP resource read reaches the adapter's resource-read entry point;
    /// one without the field, or with an origin this host does not know,
    /// reaches `invoke` exactly as before.
    #[tokio::test]
    async fn a_relay_marked_resource_read_reaches_the_adapters_resource_read_entry() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
        let (host_io, relay_io) = tokio::io::duplex(1 << 20);
        let adapter = Arc::new(OriginRecordingAdapter(std::sync::Mutex::new(Vec::new())));
        let (state, _) = watch::channel(ConnectionState::Connected);
        let (_access_tx, access) = watch::channel(AccessMode::Full);
        let (stop_tx, stop) = watch::channel(false);
        // Built from its wire form, so the test does not depend on which
        // host-owned fields a given branch of `Credentials` carries.
        let credentials: Credentials = serde_json::from_value(json!({
            "endpoint": "https://relay.example.test",
            "deviceCredential": "c".repeat(MINIMUM_CREDENTIAL_LENGTH),
            "deviceId": "device"
        }))
        .expect("credentials");
        let mut client = Client::new(credentials, adapter.clone(), state, access, stop);
        let host = tokio::spawn(async move {
            let socket = WebSocketStream::from_raw_socket(host_io, Role::Client, None).await;
            client.connected(socket).await
        });
        let mut relay = WebSocketStream::from_raw_socket(relay_io, Role::Server, None).await;
        let invoke = |id: &str, origin: Option<&str>| {
            let mut frame = json!({
                "type": "invoke", "id": id, "action": "get_current_preset",
                "method": "device.currentPreset", "params": {}
            });
            if let Some(origin) = origin {
                frame["origin"] = json!(origin);
            }
            Message::Text(frame.to_string().into())
        };
        for (id, origin) in [
            ("r1", Some(qc_relay_protocol::RESOURCE_READ_ORIGIN)),
            ("r2", None),
            ("r3", Some("someLaterOrigin")),
        ] {
            relay.send(invoke(id, origin)).await.unwrap();
            loop {
                if let Message::Text(text) = next_frame(&mut relay).await {
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    if frame["type"] == "result" {
                        assert_eq!(frame["id"], id);
                        assert_eq!(frame["ok"], true, "{frame}");
                        break;
                    }
                }
            }
        }
        assert_eq!(
            *adapter.0.lock().unwrap(),
            vec![
                "resource read device.currentPreset".to_string(),
                "invoke device.currentPreset".to_string(),
                "invoke device.currentPreset".to_string(),
            ]
        );
        stop_tx.send(true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), host)
            .await
            .expect("stop was not serviced")
            .unwrap();
        assert_eq!(outcome, Ok(()));
    }
}
