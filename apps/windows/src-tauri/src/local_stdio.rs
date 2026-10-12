use crate::local_access;
use async_trait::async_trait;
use qc_relay_client::{AccessMode, DeviceAdapter, DeviceError};
use qc_remote_mcp::{BackendError, PrincipalRoute, QcBackend, QcMcp};
use reqwest::Url;
use rmcp::ServiceExt;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::watch;

const LOCAL_PRINCIPAL: &str = "qc-remote-local-user";
const LOCAL_DEVICE: &str = "qc-remote-windows";
const STATE_OVERRIDE: &str = "QC_REMOTE_LOCAL_MCP_STATE";
const CLIENT_ID_OVERRIDE: &str = "QC_REMOTE_LOCAL_MCP_CLIENT";
/// Every managed route names itself with this argument as well as with
/// `QC_REMOTE_LOCAL_MCP_CLIENT`. `client_packages.rs` reads the same constant
/// when it writes a route and when it decides whether a route already on disk
/// is one this app would write, so the writer and the reader cannot drift.
pub(crate) const CLIENT_ID_ARGUMENT: &str = "--local-mcp-client";
#[cfg(any(not(target_os = "linux"), test))]
const APP_DIRECTORY: &str = "org.freevia.qcremote";
const STATE_FILE: &str = "local-mcp.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
// A running app re-stamps `publishedAtUnixMs` on every descriptor copy every
// 30 s. Five missed refreshes means the process is gone, and its ephemeral port
// may since have been handed to something else, so an expired descriptor must
// not be dialled. Only `descriptorRevision` 4 and later refresh; earlier
// releases stamp once at startup and are exempt, or every long-running old app
// would look dead. The JavaScript bridge implements the same two constants.
pub(crate) const DESCRIPTOR_REFRESH_REVISION: u8 = 4;
const DESCRIPTOR_STALE_AFTER_MS: u64 = 150_000;
// What the user is told when the app cannot be reached. The path list, the
// parser message and the transport wording are operator detail and go to stderr.
pub(crate) const OFFLINE_ADVICE: &str =
    "QC Remote is not running. Start QC Remote on this PC and connect the Quad Cortex over USB, then try again.";
/// What a tool call is told while a user's Quit stands (`crate::user_quit_refusal`):
/// nothing was started, and only the user brings QC Remote back. Recoverable:
/// answered as `device_unavailable`, retryable, like [`OFFLINE_ADVICE`].
pub(crate) const POST_QUIT_ADVICE: &str =
    "QC Remote was quit. To use the Quad Cortex again, open QC Remote and click Connect (or the USB button), then try again.";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long a heartbeat failure that keeps giving the same reason waits before
/// stderr hears about it again.
///
/// The announcement runs every [`HEARTBEAT_INTERVAL`], so a bridge that cannot
/// reach the app for an hour has 720 attempts to report. Writing all of them
/// buries the rest of what the host captured; writing none of them is the
/// defect this constant exists to close - a bridge whose announcements were
/// failing said nothing at all, and the panel row sat amber with no reason
/// recorded anywhere. One line when it breaks, one when the reason changes,
/// one every ten minutes while it stays broken, and one when it recovers, is
/// seven lines for that hour instead of 720.
const HEARTBEAT_FAILURE_REPEAT_AFTER: Duration = Duration::from_secs(10 * 60);
/// What the heartbeat says about a descriptor the app stopped refreshing.
///
/// Deliberately without the age. This string ends up inside the reason, and the
/// reason is the rate limiter's key in [`HeartbeatReport::observe`], so a value
/// that moves on its own makes a new key on every tick. The age did exactly
/// that: the announcement runs every [`HEARTBEAT_INTERVAL`], a descriptor an
/// abnormally exited app left on disk is never re-stamped, so the age grew by
/// five seconds a tick, every line counted as new information and was written
/// at once, and the bridge wrote one line every five seconds for as long as the
/// host ran - the run [`HEARTBEAT_FAILURE_REPEAT_AFTER`] exists to compress.
const HEARTBEAT_EXPIRED_NOTE: &str = "a descriptor QC Remote stopped refreshing was ignored";

#[derive(Clone)]
struct DesktopBackend {
    client: reqwest::Client,
    /// This bridge's `stdio-<random>` id, the one its heartbeat announces.
    /// Sent on every relayed tool call (never on a resource read), and only
    /// to the validated loopback
    /// endpoint, so a host can tell the chat's tool call from its heartbeat:
    /// after the tray's Unlink only the call lists the session again. `None`
    /// sends nothing.
    session_id: Option<Arc<str>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndpointDocument {
    schema_version: u8,
    rpc_url: String,
    bearer_token: String,
    #[serde(default)]
    published_at_unix_ms: u64,
    #[serde(default)]
    descriptor_revision: u8,
    // The three identity fields the app itself reads back before it publishes,
    // and when the user asks the LOCAL panel to test a pairing. They are
    // deliberately not validated here: the bridge never dials `/health` and
    // never compares instances, so demanding them would retire descriptors the
    // bridge can still use. `local_access.rs` validates what it uses.
    #[serde(default)]
    health_url: String,
    #[serde(default)]
    instance_id: String,
    #[serde(default)]
    process_id: u32,
    /// Absent from releases that predate the identity check.
    #[serde(default)]
    identity_url: Option<String>,
}

/// One descriptor, already validated and chosen by [`descriptor_from_paths_at`].
///
/// `pub(crate)` because the running app reads its own published descriptor
/// through the same reader its bridges use. A second reader with its own
/// path order, its own staleness rule and its own newest-wins choice is how
/// the four that existed drifted apart in the first place.
#[cfg_attr(test, derive(Debug))]
pub(crate) struct Endpoint {
    pub(crate) rpc_url: Url,
    pub(crate) bearer_token: String,
    pub(crate) published_at_unix_ms: u64,
    pub(crate) descriptor_revision: u8,
    pub(crate) health_url: String,
    pub(crate) instance_id: String,
    pub(crate) process_id: u32,
    /// Where to check the listener holds the token before sending it. `None`
    /// only for a descriptor from a release without the check.
    pub(crate) identity_url: Option<String>,
}

impl Endpoint {
    /// Prove the listener on this port is the instance that published the
    /// descriptor before the bearer token is sent to it. See `local_identity`.
    pub(crate) async fn prove_identity(
        &self,
        client: &reqwest::Client,
    ) -> Result<(), crate::local_identity::IdentityFailure> {
        match &self.identity_url {
            None => Ok(()),
            Some(identity_url) => {
                crate::local_identity::verify(
                    client,
                    identity_url,
                    &self.instance_id,
                    &self.bearer_token,
                )
                .await
            }
        }
    }
}

#[derive(Deserialize)]
struct RpcEnvelope {
    ok: bool,
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: String,
    message: String,
    retryable: bool,
    #[serde(default, rename = "deviceCode")]
    device_code: Option<String>,
    #[serde(default)]
    details: Option<Box<Value>>,
}

/// Whether a process whose descriptor paths came from [`state_paths`] may
/// publish its bearer token there.
///
/// `QC_REMOTE_LOCAL_MCP_STATE` redirects *reading* in every build: the release
/// installer's native smoke test points a release bridge at a sandboxed
/// descriptor through it. Publishing is another matter. A hosting bridge
/// writes the token to every path it reads, so honouring the override there
/// let the environment choose where a release build drops a live credential
/// (owner-only DACL or not, the place may be synced or shared). Like
/// `broker_executable_override`, that knob is for debug builds only; a release
/// bridge started with the override relays and never hosts.
fn may_publish_at_state_paths(debug_build: bool, override_set: bool) -> bool {
    debug_build || !override_set
}

/// The settings a bridge hosts with, or `None` when it must only relay
/// (see [`may_publish_at_state_paths`]).
fn hosting_settings() -> Option<HeadlessHostSettings> {
    if !may_publish_at_state_paths(
        cfg!(debug_assertions),
        std::env::var_os(STATE_OVERRIDE).is_some(),
    ) {
        eprintln!(
            "QC Remote local MCP: {STATE_OVERRIDE} is set, so this release bridge reads the descriptor it names but will not host or publish one."
        );
        return None;
    }
    HeadlessHostSettings::for_this_user()
}

pub(crate) fn state_paths() -> Result<Vec<PathBuf>, String> {
    if let Some(path) = std::env::var_os(STATE_OVERRIDE) {
        return Ok(vec![PathBuf::from(path)]);
    }
    #[cfg(target_os = "linux")]
    {
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths::prepare_directory(&directory)?;
        Ok(vec![directory.join(STATE_FILE)])
    }
    #[cfg(not(target_os = "linux"))]
    {
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                profile
                    .as_ref()
                    .map(|path| path.join("AppData").join("Local"))
            });
        let roaming = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| profile.map(|path| path.join("AppData").join("Roaming")));
        let paths = state_paths_from(local, roaming);
        if paths.is_empty() {
            Err("Windows user data directories are unavailable.".into())
        } else {
            Ok(paths)
        }
    }
}

/// The candidate list, separated from the environment that resolves it so the
/// set itself can be asserted.
#[cfg(not(target_os = "linux"))]
fn state_paths_from(local: Option<PathBuf>, roaming: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(local) = local {
        paths.push(local.join(APP_DIRECTORY).join(STATE_FILE));
    }
    if let Some(roaming) = roaming {
        let path = roaming.join("Claude").join("qc-remote").join(STATE_FILE);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn validate_descriptor(document: EndpointDocument) -> Result<Endpoint, String> {
    if document.schema_version != 1 {
        return Err("QC Remote published an unsupported local MCP descriptor.".into());
    }
    let rpc_url = Url::parse(&document.rpc_url)
        .map_err(|_| "QC Remote published an invalid local RPC address.".to_string())?;
    if rpc_url.scheme() != "http"
        || rpc_url.host_str() != Some("127.0.0.1")
        || rpc_url.port().is_none()
        || rpc_url.path() != "/rpc"
        || !rpc_url.username().is_empty()
        || rpc_url.password().is_some()
        || rpc_url.query().is_some()
        || rpc_url.fragment().is_some()
    {
        return Err("QC Remote published an invalid local RPC address.".into());
    }
    if document.bearer_token.len() < 43
        || !document
            .bearer_token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("QC Remote published an invalid local MCP credential.".into());
    }
    if let Some(identity_url) = &document.identity_url {
        // A descriptor that names an identity check is held to it: the check
        // must share the RPC origin and have an instance to prove.
        if !crate::local_identity::identity_url_matches(&rpc_url, identity_url)
            || document.instance_id.is_empty()
        {
            return Err("QC Remote published an invalid local identity address.".into());
        }
    }
    Ok(Endpoint {
        rpc_url,
        bearer_token: document.bearer_token,
        published_at_unix_ms: document.published_at_unix_ms,
        descriptor_revision: document.descriptor_revision,
        health_url: document.health_url,
        instance_id: document.instance_id,
        process_id: document.process_id,
        identity_url: document.identity_url,
    })
}

/// The descriptor read the heartbeat performs, whose failures it rate-limits
/// itself.
///
/// [`descriptor_from_paths_at`] writes its diagnosis to stderr on every failed
/// read, which is proportionate for a tool call and is 720 lines an hour for an
/// announcement that runs every five seconds. Rate-limiting the reporter while
/// the reader kept printing would have moved the spam rather than removed it,
/// so the heartbeat takes the detail back as part of its error and decides for
/// itself when to write it.
fn heartbeat_descriptor() -> Result<Endpoint, String> {
    let paths = state_paths()?;
    heartbeat_descriptor_from_paths(&paths, now_unix_ms())
}

fn heartbeat_descriptor_from_paths(paths: &[PathBuf], now_ms: u64) -> Result<Endpoint, String> {
    // A successful announcement reports nothing at all, expired copies
    // included: an announcement that worked has nothing to tell the user, and
    // the request path names the abandoned copy whenever they actually ask for
    // something.
    choose_descriptor(paths, now_ms)
        .map(|chosen| chosen.endpoint)
        .map_err(|absence| {
            let mut detail = absence.detail;
            if !absence.expired_ages_ms.is_empty() {
                detail.push_str("; ");
                detail.push_str(HEARTBEAT_EXPIRED_NOTE);
            }
            format!("{} ({})", absence.advice, detail)
        })
}

fn now_unix_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

pub(crate) fn descriptor_from_paths(paths: &[PathBuf]) -> Result<Endpoint, String> {
    descriptor_from_paths_at(paths, now_unix_ms())
}

/// Why no descriptor could be used, in the two registers the app keeps apart:
/// the sentence the user gets, and the operator detail that names paths and
/// parser messages.
///
/// `expired_ages_ms` stays a number rather than a sentence because the two
/// callers may not word it the same way: only one of them is allowed to name
/// an age. See [`HEARTBEAT_EXPIRED_NOTE`].
struct NoDescriptor {
    advice: &'static str,
    detail: String,
    expired_ages_ms: Vec<u64>,
}

/// The descriptor that won, and the ages of the copies passed over for no
/// longer being refreshed - again data, not prose.
struct ChosenDescriptor {
    endpoint: Endpoint,
    expired_ages_ms: Vec<u64>,
}

/// How the request path words one copy the app stopped refreshing.
///
/// A tool call is not on a timer, so an age is proportionate here and is worth
/// keeping; the heartbeat has [`HEARTBEAT_EXPIRED_NOTE`] instead.
fn expired_note(age_ms: u64) -> String {
    format!(
        "a descriptor last refreshed {}s ago was ignored",
        age_ms / 1000
    )
}

/// The operator detail the request path writes when nothing is usable.
///
/// Separated from the `eprintln!` that writes it so the wording can be
/// asserted: stderr is not observable from a unit test, and the wording is the
/// part that must not drift.
fn request_path_detail(mut detail: String, expired_ages_ms: &[u64]) -> String {
    for age_ms in expired_ages_ms {
        detail.push_str("; ");
        detail.push_str(&expired_note(*age_ms));
    }
    detail
}

/// The reading and choosing, with no opinion about where a diagnosis goes.
///
/// Both callers agree on which descriptor wins and why; they differ in whether
/// the detail is printed now or carried back to a caller that will decide, and
/// in how each words a copy the app stopped refreshing. Keeping the choice in
/// one function is what stops the two from drifting - four readers with their
/// own path order and their own staleness rule is how this went wrong before.
pub(crate) fn descriptor_from_paths_at(paths: &[PathBuf], now_ms: u64) -> Result<Endpoint, String> {
    match choose_descriptor(paths, now_ms) {
        Ok(chosen) => {
            // One live copy beside an abandoned one is worth a line to whoever
            // asked for the descriptor.
            for age_ms in chosen.expired_ages_ms {
                eprintln!("QC Remote local MCP: {}", expired_note(age_ms));
            }
            Ok(chosen.endpoint)
        }
        Err(absence) => {
            eprintln!(
                "QC Remote local MCP: {}",
                request_path_detail(absence.detail, &absence.expired_ages_ms)
            );
            Err(absence.advice.to_string())
        }
    }
}

fn choose_descriptor(paths: &[PathBuf], now_ms: u64) -> Result<ChosenDescriptor, NoDescriptor> {
    let mut invalid = Vec::new();
    let mut valid = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let endpoint = std::fs::read(path)
            .map_err(|error| format!("could not be read: {error}"))
            .and_then(|contents| {
                serde_json::from_slice::<EndpointDocument>(&contents)
                    .map_err(|error| format!("is invalid: {error}"))
            })
            .and_then(validate_descriptor);
        match endpoint {
            Ok(endpoint) => valid.push(endpoint),
            Err(error) => invalid.push(format!("{}: {error}", path.display())),
        }
    }
    // The app mirrors one descriptor for restricted extension hosts. An
    // abnormal exit can leave either copy behind, so path order is not a
    // freshness signal. Always follow the newest publication instead of a
    // syntactically valid descriptor whose ephemeral port belonged to a dead
    // app process.
    // A descriptor the app has stopped refreshing belongs to a dead process,
    // whose ephemeral port may since have been reused. Dropping it before the
    // newest-wins choice keeps a corpse from outranking a live older instance.
    // The ages go back as numbers and no wording is chosen here. One caller
    // prints an age; the other may not, because its reason is a rate limiter's
    // key and a key that carries a value which changes on its own is never the
    // same key twice.
    let mut expired_ages_ms = Vec::new();
    valid.retain(|endpoint| {
        if endpoint.descriptor_revision < DESCRIPTOR_REFRESH_REVISION {
            return true;
        }
        let age_ms = now_ms.saturating_sub(endpoint.published_at_unix_ms);
        if age_ms > DESCRIPTOR_STALE_AFTER_MS {
            expired_ages_ms.push(age_ms);
            return false;
        }
        true
    });
    if let Some(endpoint) = valid
        .into_iter()
        .max_by_key(|endpoint| endpoint.published_at_unix_ms)
    {
        return Ok(ChosenDescriptor {
            endpoint,
            expired_ages_ms,
        });
    }
    // Both remaining cases mean the same thing to the user — the app is not
    // reachable — so the diagnosis is kept separate from the message, which
    // stays actionable rather than naming the user's own filesystem.
    let detail = if !invalid.is_empty() {
        format!("unusable connection file: {}", invalid.join("; "))
    } else {
        format!(
            "no connection file at {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(" or ")
        )
    };
    Err(NoDescriptor {
        advice: OFFLINE_ADVICE,
        detail,
        expired_ages_ms,
    })
}

fn backend_code(code: &str) -> &'static str {
    match code {
        "forbidden" => "forbidden",
        "device_unavailable" => "device_unavailable",
        "device_error" => "device_error",
        "unknown_action" => "unknown_action",
        _ => "local_bridge_error",
    }
}

/// Which host this bridge announces itself as, resolved once.
///
/// Antigravity retains the configured environment in its own server state
/// but does not reliably pass it to a native STDIO child, so a bridge
/// identified only by `env` announces itself anonymously and the app cannot
/// credit the heartbeat to that host. Prefer the explicit launcher argument,
/// and keep the environment fallback for the MCP clients that do propagate it.
fn configured_client_id() -> Option<String> {
    std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| (pair[0] == CLIENT_ID_ARGUMENT).then(|| pair[1].trim().to_owned()))
        .or_else(|| {
            std::env::var(CLIENT_ID_OVERRIDE)
                .ok()
                .map(|value| value.trim().to_owned())
        })
        .filter(|value| !value.is_empty() && value.len() <= 64)
}

/// How a heartbeat attempt is described on stderr, and what it is worth saying.
///
/// The announcement is telemetry: whatever it reports, the caller keeps serving
/// MCP. What it must not do is report nothing, which is what discarding the
/// `Result` did - `send_heartbeat` distinguishes an unreadable descriptor, an
/// unreachable endpoint and a refused status, and every one of those was
/// dropped on the floor.
#[derive(Default)]
struct HeartbeatReport {
    /// The reason the last written line gave, while the failure lasts.
    reported: Option<String>,
    /// When that line was written, in milliseconds since the loop started.
    reported_at_ms: u64,
    /// Attempts that failed since then and said nothing new.
    suppressed: u64,
}

/// What stderr is told about a bridge with no identity on its command line.
///
/// This is not a heartbeat failure - the app answers it - but it is the reason
/// a working host reads as never-attached, so it is worth exactly one line.
fn anonymous_bridge_notice() -> String {
    format!(
        "QC Remote local MCP: this bridge was started without {CLIENT_ID_ARGUMENT}, so QC Remote cannot credit its heartbeat to a host and the LOCAL panel cannot show that host as connected."
    )
}

/// How startup handles configured-client attribution for a bridge session.
///
/// Both outcomes keep the session heartbeat loop running. `local_rpc` passes
/// `sessionId` and `clientName` to `host_presence`, which records a valid, linked
/// session for broker-tray presence independently of `client`. Configured-client
/// evidence is credited only when `client` is non-empty and within its limit.
/// While the session remains linked, `Withheld` renews its presence lease just
/// as `As` does, without crediting a configured host identity. Tray Unlink stops
/// recording that session; the heartbeat does not restore it. The configured
/// identity is not carried on tool calls, so this does not join it to a later
/// request.
///
/// The heartbeat never reaches the MCP backend. In the background-host path,
/// its live linked-session lease keeps `host_in_use` true and makes the host
/// retain its broker connection. A bridge continues serving tools whether its
/// configured-client identity is available or withheld.
enum Announcement {
    /// Include this configured identity in the session heartbeat.
    As(String),
    /// Emit this notice once, then heartbeat the session without client attribution.
    Withheld(String),
}

fn announcement(client: Option<String>) -> Announcement {
    match client {
        // [`configured_client_id`] already rejects empty and over-long values.
        // Repeat the non-empty check here because only a non-empty `client`
        // earns configured-client connection credit; the session lease is
        // recorded independently from that attribution.
        Some(client) if !client.is_empty() => Announcement::As(client),
        _ => Announcement::Withheld(anonymous_bridge_notice()),
    }
}

impl HeartbeatReport {
    /// Folds one attempt into the report and returns the line to write, if any.
    ///
    /// `elapsed_ms` is monotonic time since the loop began; it is a parameter
    /// rather than a clock read so the rate limit can be asserted.
    ///
    /// `client` stays optional so a caller without an identity can be reported
    /// as unidentified. The serving path passes an empty string when the
    /// configured identity is withheld; that value is not a client identity.
    /// `a_bridge_with_no_configured_identity_is_still_attributable` pins the
    /// wording for the `None` case.
    fn observe(
        &mut self,
        client: Option<&str>,
        outcome: Result<(), String>,
        elapsed_ms: u64,
    ) -> Option<String> {
        let host = client.unwrap_or("an unidentified host");
        match outcome {
            Ok(()) => {
                // Nothing was ever reported, so there is no recovery to
                // announce and a healthy bridge stays silent for its whole
                // life.
                let reason = self.reported.take()?;
                let failures = std::mem::take(&mut self.suppressed) + 1;
                self.reported_at_ms = elapsed_ms;
                Some(format!(
                    "QC Remote local MCP: {host} announced itself again after {failures} failed attempts; the last reason was: {reason}"
                ))
            }
            Err(reason) => {
                let repeating = self.reported.as_deref() == Some(reason.as_str());
                let due = elapsed_ms.saturating_sub(self.reported_at_ms)
                    >= HEARTBEAT_FAILURE_REPEAT_AFTER.as_millis() as u64;
                if repeating && !due {
                    self.suppressed += 1;
                    return None;
                }
                let suppressed = std::mem::take(&mut self.suppressed);
                self.reported = Some(reason.clone());
                self.reported_at_ms = elapsed_ms;
                Some(if repeating {
                    format!(
                        "QC Remote local MCP: {host} still cannot announce itself: {reason} ({suppressed} further attempts failed for the same reason since the last line)"
                    )
                } else {
                    format!("QC Remote local MCP: {host} could not announce itself: {reason}")
                })
            }
        }
    }
}

async fn send_heartbeat(
    client: &reqwest::Client,
    client_id: &str,
    session_id: &str,
    client_name: &str,
    client_version: Option<&str>,
) -> Result<(), String> {
    let endpoint = heartbeat_descriptor()?;
    endpoint.prove_identity(client).await?;
    let response = client
        .post(endpoint.rpc_url)
        .bearer_auth(endpoint.bearer_token)
        .json(&serde_json::json!({
            "rpc": "local.bridgeHeartbeat",
            "params": {
                "client": client_id,
                "sessionId": session_id,
                "clientName": client_name,
                "clientVersion": client_version,
            }
        }))
        .send()
        .await
        // Every arm names one distinguishable cause, because these strings are
        // now the only record of why a working host read as never-attached.
        // They are also the rate limiter's key: a reason that changes is new
        // information and is written immediately.
        .map_err(|error| format!("the loopback endpoint did not answer ({error})"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "QC Remote refused the announcement ({})",
            response.status()
        ))
    }
}

// No HTTP status came back, so nothing is listening on the port the descriptor
// advertises: the app closed since it published, or is still starting. That is
// the app's absence, not a transport fault the user can act on, so the endpoint
// and the reqwest wording go to stderr and the caller gets the same sentence a
// missing descriptor produces.
fn unreachable_app(error: reqwest::Error) -> RelayFailure {
    eprintln!("QC Remote local MCP: could not reach the app: {error}");
    let unavailable = BackendError::unavailable(OFFLINE_ADVICE.to_string());
    // Only a failed connect proves the request never left: a timeout or a
    // dropped connection may have reached an app that acted on it, and a
    // write must not then run a second time through another host.
    if error.is_connect() {
        RelayFailure::NoPublisher(unavailable)
    } else {
        RelayFailure::Refused(unavailable)
    }
}

fn is_local_route(route: &PrincipalRoute) -> bool {
    route.principal_id.as_ref() == LOCAL_PRINCIPAL && route.device_id.as_ref() == LOCAL_DEVICE
}

/// Why a relayed request failed, in the two kinds a bridge treats differently.
enum RelayFailure {
    /// Nobody publishes, or the published address refused the connection:
    /// the request reached no one, so this bridge may host and answer it
    /// itself.
    NoPublisher(BackendError),
    /// Any other failure, including an answer that was itself an error.
    Refused(BackendError),
}

impl RelayFailure {
    fn into_backend_error(self) -> BackendError {
        match self {
            Self::NoPublisher(error) | Self::Refused(error) => error,
        }
    }
}

impl DesktopBackend {
    /// The relayed request exactly as sent: bearer token, the bridge's
    /// session id when it has one, and the `{rpc, params}` body.
    fn rpc_request(
        &self,
        endpoint: &Endpoint,
        rpc: &str,
        params: Map<String, Value>,
    ) -> reqwest::RequestBuilder {
        let request = self
            .client
            .post(endpoint.rpc_url.clone())
            .bearer_auth(&endpoint.bearer_token);
        // Only a tool call names the session: a resource read is the client
        // reading context, not the chat asking again, and `qc://models`
        // shares its rpc with the `list_models` tool, so the origin decides.
        // A resource read says so instead, so the host that answers it never
        // brings a broker connection up for it, as it would not for its own.
        let resource_read = qc_remote_mcp::serving_resource_read();
        match self.session_id.as_ref().filter(|_| !resource_read) {
            Some(session) => request.header(
                crate::host_presence::BRIDGE_SESSION_HEADER,
                session.as_ref(),
            ),
            None if resource_read => request.header(
                local_access::REQUEST_ORIGIN_HEADER,
                local_access::RESOURCE_READ_ORIGIN,
            ),
            None => request,
        }
        .json(&serde_json::json!({ "rpc": rpc, "params": params }))
    }

    async fn relay(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, RelayFailure> {
        if !is_local_route(route) {
            return Err(RelayFailure::Refused(BackendError {
                code: "forbidden",
                message: "The local route does not belong to this QC Remote instance.".into(),
                retryable: false,
                detail_code: None,
                details: None,
            }));
        }
        let paths = state_paths()
            .map_err(|error| RelayFailure::NoPublisher(BackendError::unavailable(error)))?;
        self.relay_at(&paths, rpc, params).await
    }

    /// Relay to whichever process publishes at `paths`.
    async fn relay_at(
        &self,
        paths: &[PathBuf],
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, RelayFailure> {
        let endpoint = descriptor_from_paths(paths)
            .map_err(|error| RelayFailure::NoPublisher(BackendError::unavailable(error)))?;
        self.send(endpoint, rpc, params).await
    }

    async fn send(
        &self,
        endpoint: Endpoint,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, RelayFailure> {
        // Nothing was sent if the proof fails, so nobody legitimate is serving
        // this descriptor and the request may be answered elsewhere.
        if let Err(reason) = endpoint.prove_identity(&self.client).await {
            eprintln!("QC Remote local MCP: {reason}");
            return Err(RelayFailure::NoPublisher(BackendError::unavailable(
                OFFLINE_ADVICE.to_string(),
            )));
        }
        let response = self
            .rpc_request(&endpoint, rpc, params)
            .send()
            .await
            .map_err(unreachable_app)?;
        let status = response.status();
        let envelope = response.json::<RpcEnvelope>().await.map_err(|error| {
            RelayFailure::Refused(BackendError {
                code: "local_bridge_error",
                message: format!(
                    "QC Remote returned an invalid local response ({status}): {error}"
                ),
                retryable: status.is_server_error(),
                detail_code: None,
                details: None,
            })
        })?;
        if envelope.ok {
            return envelope.result.ok_or_else(|| {
                RelayFailure::Refused(BackendError {
                    code: "local_bridge_error",
                    message: "QC Remote returned a successful response without a result.".into(),
                    retryable: false,
                    detail_code: None,
                    details: None,
                })
            });
        }
        let error = envelope.error.unwrap_or(RpcError {
            code: "local_bridge_error".into(),
            message: format!("QC Remote refused the local request ({status})."),
            retryable: status.is_server_error(),
            device_code: None,
            details: None,
        });
        Err(RelayFailure::Refused(BackendError {
            code: backend_code(&error.code),
            message: error.message,
            retryable: error.retryable,
            detail_code: error.device_code,
            details: error.details,
        }))
    }
}

#[async_trait]
impl QcBackend for DesktopBackend {
    async fn request(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        self.relay(route, rpc, params)
            .await
            .map_err(RelayFailure::into_backend_error)
    }
}

/// Answers device calls for a headless `--local-mcp-bridge` process that has
/// become the MCP-HTTP publisher itself, using the same attach-or-elect
/// sequence a GUI window's [`crate::GatewayProcess`] uses against the shared
/// device broker.
///
/// [`crate::Gateway::request_detailed`] is synchronous - it blocks on a
/// broker round trip over a channel fed by its own reader thread - so every
/// call is bounced through [`tokio::task::spawn_blocking`] rather than run
/// directly on an async worker thread, exactly as [`crate::WindowsRelayAdapter`]
/// already does for the GUI's own Tauri-managed `Gateway`. The difference is
/// only where the `Gateway` itself lives: the GUI keeps it in Tauri-managed
/// state reached through an `AppHandle`; this process has no `AppHandle`; so
/// this adapter owns an `Arc<Mutex<Gateway>>` directly and clones the `Arc`
/// into the blocking closure instead.
#[derive(Clone)]
pub(crate) struct HeadlessDeviceAdapter {
    gateway: Arc<Mutex<crate::Gateway>>,
    /// The gateway's lock-free read slot, reachable without its mutex.
    lock_free_reads: crate::LockFreeReadSlot,
    stopped: Arc<AtomicBool>,
    /// Set only for the background host inside `qc-device-broker.exe`: the
    /// broker in the same process, read while this adapter's gateway is
    /// detached. See [`HeadlessDeviceAdapter::readiness`].
    broker: Option<BrokerReadinessProbe>,
    /// The broker read that has not returned yet, shared by `/health` and the
    /// relay (they hold clones of one adapter). A caller arriving while it
    /// runs waits for that read's answer instead of starting another, so two
    /// overlapping probes never see a spurious not-ready, and a wedged read
    /// costs one blocking thread, not one per probe.
    broker_read: Arc<Mutex<Option<BrokerRead>>>,
    /// How long one broker read may take, counted from when it started:
    /// [`BROKER_READINESS_TIMEOUT`] outside tests.
    broker_read_budget: Duration,
}

/// What a broker read answers once it has one. `Err` is a read that panicked
/// or never ran.
type BrokerAnswer = Option<Result<BrokerDeviceReadiness, ()>>;

/// One broker read in flight: when it started, and its answer once it has
/// one. Every caller's wait ends at `started` plus the budget, so a caller
/// joining a hung read late waits only what remains of it, and none once it
/// is past.
#[derive(Clone)]
struct BrokerRead {
    started: std::time::Instant,
    answer: Arc<watch::Sender<BrokerAnswer>>,
}

/// Ends a broker read's turn in the slot however the read ends, so the next
/// caller starts a fresh one. Dropped unrun (the runtime shutting down), it
/// also drops the sender, and every waiter sees the read as failed.
struct BrokerReadSlot(Arc<Mutex<Option<BrokerRead>>>);

impl Drop for BrokerReadSlot {
    fn drop(&mut self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// What the device broker in this process says about its device session.
/// The broker crate's own type is mapped onto this one by the binary that
/// links both (`src/bin/qc-device-broker.rs`), so this crate never depends on
/// the broker crate outside that build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerDeviceReadiness {
    /// The session is open.
    Ready,
    /// The broker is serving and the session is not open.
    NotReady { phase: String },
    /// The broker cannot answer for the device at all.
    Unavailable { reason: String },
}

/// A read of the in-process broker's device readiness that never opens USB,
/// never claims the idle reopen and never attaches a participant
/// (`qc_device_broker::resident::DeviceReadinessProbe`).
pub type BrokerReadinessProbe = Arc<dyn Fn() -> BrokerDeviceReadiness + Send + Sync>;

/// How long a broker readiness read may run before every caller waiting on
/// it answers not ready, counted from when the read started. The read takes
/// only the worker's short status lock, so anything near this is a wedged
/// broker, not a slow one. Because the deadline belongs to the shared read
/// and not to each caller, a hung read costs the relay's loop (which awaits
/// readiness inline) at most this once: every probe after the deadline
/// answers not ready at once until the read returns, then a fresh read
/// starts.
const BROKER_READINESS_TIMEOUT: Duration = Duration::from_secs(2);

/// A headless host's readiness, with the reason when it is not ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostReadiness {
    Ready,
    NotReady(String),
}

impl HeadlessDeviceAdapter {
    /// Readiness for `/health` and the relay's `DeviceFrame::Ready`.
    ///
    /// The gateway's `connected` is consulted only while the gateway is
    /// attached to the broker. The background host attaches only while MCP
    /// clients are present, and `release_when_unused` clears `connected` when
    /// it detaches, so with no client attached the gateway's flag says nothing
    /// about the Quad Cortex; answering from it made `/health` 503 and the
    /// cloud's device status `connected: 0` while the broker held a ready
    /// device. Detached, the in-process broker answers instead, from its
    /// cached session state: no request, no USB, no idle reopen, no
    /// participant. A host with no broker in its process (a standalone
    /// `--local-mcp-bridge`) keeps the gateway's answer.
    ///
    /// Never waits for the gateway lock, and never waits on the broker longer
    /// than [`BROKER_READINESS_TIMEOUT`].
    pub(crate) async fn readiness(&self) -> HostReadiness {
        match crate::gateway_attachment(&self.gateway) {
            crate::GatewayAttachment::Attached(true) => HostReadiness::Ready,
            crate::GatewayAttachment::Attached(false) => HostReadiness::NotReady(
                "the host's broker connection reports the Quad Cortex not connected".into(),
            ),
            crate::GatewayAttachment::Poisoned => {
                HostReadiness::NotReady("the gateway session lock was poisoned".into())
            }
            // A request (or an attach, or a release) holds the lock. With the
            // broker in this process its answer is current; without it, keep
            // what the gateway last published rather than inventing a
            // disconnection out of contention.
            crate::GatewayAttachment::Busy(published) => match &self.broker {
                Some(_) => self.broker_readiness().await,
                None if published => HostReadiness::Ready,
                None => HostReadiness::NotReady(
                    "the gateway last reported the Quad Cortex not connected".into(),
                ),
            },
            crate::GatewayAttachment::Detached => match &self.broker {
                Some(_) => self.broker_readiness().await,
                None => HostReadiness::NotReady(
                    "no MCP client is attached and this host has no broker to ask".into(),
                ),
            },
        }
    }

    async fn broker_readiness(&self) -> HostReadiness {
        let Some(probe) = self.broker.clone() else {
            return HostReadiness::NotReady("this host has no broker to ask".into());
        };
        let (mut answer, deadline) = self.join_or_start_broker_read(probe);
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let answered = tokio::time::timeout(remaining, async {
            answer
                .wait_for(Option::is_some)
                .await
                .map(|value| value.clone())
        })
        .await;
        match answered {
            Ok(Ok(Some(Ok(BrokerDeviceReadiness::Ready)))) => HostReadiness::Ready,
            Ok(Ok(Some(Ok(BrokerDeviceReadiness::NotReady { phase })))) => HostReadiness::NotReady(
                format!("the device broker reports the Quad Cortex session {phase}"),
            ),
            Ok(Ok(Some(Ok(BrokerDeviceReadiness::Unavailable { reason })))) => {
                HostReadiness::NotReady(reason)
            }
            Ok(Ok(Some(Err(())) | None)) | Ok(Err(_)) => {
                HostReadiness::NotReady("the readiness read of the device broker failed".into())
            }
            Err(_) => HostReadiness::NotReady(format!(
                "the device broker did not answer the readiness read within {} ms",
                self.broker_read_budget.as_millis()
            )),
        }
    }

    /// The answer of the broker read already running, or of one started now,
    /// and the moment that read's budget runs out.
    fn join_or_start_broker_read(
        &self,
        probe: BrokerReadinessProbe,
    ) -> (watch::Receiver<BrokerAnswer>, std::time::Instant) {
        let mut slot = self
            .broker_read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(read) = slot.as_ref() {
            return (
                read.answer.subscribe(),
                read.started + self.broker_read_budget,
            );
        }
        let started = std::time::Instant::now();
        let (sender, receiver) = watch::channel(None);
        let sender = Arc::new(sender);
        *slot = Some(BrokerRead {
            started,
            answer: sender.clone(),
        });
        drop(slot);
        let turn = BrokerReadSlot(self.broker_read.clone());
        // Detached on purpose: a caller that gave up waiting must not cancel
        // the read the next caller is about to join.
        drop(tokio::task::spawn_blocking(move || {
            let answer =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe())).map_err(|_| ());
            // Out of the slot before answering: a caller after this point
            // starts afresh rather than joining a read that has finished.
            drop(turn);
            sender.send_replace(Some(answer));
        }));
        (receiver, started + self.broker_read_budget)
    }

    /// Test-only: callers waiting on the broker read in flight (0 when none).
    #[cfg(test)]
    pub(crate) fn broker_read_waiters(&self) -> usize {
        self.broker_read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(0, |read| read.answer.receiver_count())
    }

    /// Test-only: a broker read budget other than the shipped two seconds.
    #[cfg(test)]
    pub(crate) fn with_broker_read_budget(mut self, budget: Duration) -> Self {
        self.broker_read_budget = budget;
        self
    }

    /// Gives a host the broker in its own process to answer readiness from
    /// while its gateway is detached.
    pub(crate) fn with_broker_readiness(mut self, probe: BrokerReadinessProbe) -> Self {
        self.broker = Some(probe);
        self
    }

    /// Test-only: whether the gateway holds a broker connection, which is
    /// what a participant attach (and any device request) would leave behind.
    #[cfg(test)]
    pub(crate) fn gateway_attached(&self) -> bool {
        self.gateway
            .lock()
            .map(|gateway| gateway.process.is_some())
            .unwrap_or(true)
    }

    /// Test-only: takes the `Child` handle of the broker this adapter's
    /// gateway spawned (`GatewayProcess::take_hosted_broker_child`).
    #[cfg(test)]
    pub(crate) fn take_hosted_broker_child(&self) -> Option<std::process::Child> {
        self.gateway
            .lock()
            .ok()
            .and_then(|mut gateway| gateway.process.as_mut()?.take_hosted_broker_child())
    }

    pub(crate) fn retain_while_used(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut gateway) = self.gateway.try_lock() {
            // Presence is not permission to undo a deliberate tray release.
            if gateway
                .respect_intentional_release(
                    quadcortex::device::generated_gateway::rpc::RUNTIME_STATUS,
                )
                .is_err()
            {
                return;
            }
            if gateway.process.is_none() {
                let _ = gateway.request_detailed(
                    quadcortex::device::generated_gateway::rpc::RUNTIME_STATUS,
                    serde_json::json!({}),
                );
            }
            if self.stopped.load(Ordering::Acquire) {
                gateway.retire_current_process();
            }
        }
    }
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.release_when_unused();
    }
    pub(crate) fn release_when_unused(&self) {
        // No request in flight may be interrupted to satisfy an idle check.
        if let Ok(mut gateway) = self.gateway.try_lock() {
            gateway.retire_current_process();
            let _ = gateway.respect_intentional_release(
                quadcortex::device::generated_gateway::rpc::RUNTIME_STATUS,
            );
            gateway.connected = false;
            gateway.device = crate::DeviceSession::default();
        }
    }
    /// `descriptor_path` is the shared broker's descriptor, the very file
    /// every window meets it through. A window learns that path in Tauri
    /// setup, which this process never runs; left to `Gateway::default()`, the
    /// gateway would fall back to a private `--stdio` broker and open a second
    /// USB connection beside the shared one.
    pub(crate) fn new(descriptor_path: PathBuf, auto_connect: bool) -> Self {
        Self::with_identity(descriptor_path, auto_connect, "QC Remote MCP host", None)
    }
    /// Test-only: an adapter over a gateway a test built, for example one
    /// already attached to a scripted broker connection.
    #[cfg(test)]
    pub(crate) fn over_gateway(gateway: crate::Gateway) -> Self {
        let lock_free_reads = gateway.lock_free_read_slot();
        Self {
            stopped: Arc::new(AtomicBool::new(false)),
            lock_free_reads,
            broker: None,
            broker_read: Arc::new(Mutex::new(None)),
            broker_read_budget: BROKER_READINESS_TIMEOUT,
            gateway: Arc::new(Mutex::new(gateway)),
        }
    }
    /// `provider` is the name this adapter's connection shows in
    /// `broker.hello`. `host_token` is set only for the background host inside
    /// `qc-device-broker.exe`: the broker's per-process token, which is what
    /// actually identifies that host to its own broker. A name never does.
    pub(crate) fn with_identity(
        descriptor_path: PathBuf,
        auto_connect: bool,
        provider: &'static str,
        host_token: Option<String>,
    ) -> Self {
        let gateway = crate::Gateway {
            descriptor_path: Some(descriptor_path),
            auto_connect_first_broker: auto_connect,
            participant_provider: Some(provider),
            participant_host_token: host_token,
            ..crate::Gateway::default()
        };
        let lock_free_reads = gateway.lock_free_read_slot();
        Self {
            stopped: Arc::new(AtomicBool::new(false)),
            lock_free_reads,
            broker: None,
            broker_read: Arc::new(Mutex::new(None)),
            broker_read_budget: BROKER_READINESS_TIMEOUT,
            gateway: Arc::new(Mutex::new(gateway)),
        }
    }
}

#[async_trait]
impl DeviceAdapter for HeadlessDeviceAdapter {
    async fn ready(&self) -> bool {
        // Never blocks: a probe that waited for the gateway lock would sit
        // behind a whole backup on an async worker thread.
        self.readiness().await == HostReadiness::Ready
    }

    async fn invoke(&self, method: &str, params: Value) -> Result<Value, DeviceError> {
        // Read here: the blocking thread the gateway call runs on has no
        // task-local.
        self.invoke_as(method, params, crate::ConnectionUse::of_current_task())
            .await
    }

    /// A call the paired relay marked as an MCP resource read is served as
    /// this host serves its own: from the broker connection it holds, never
    /// bringing one up. The mode is passed explicitly to the gateway, not
    /// left to a task-local.
    async fn invoke_resource_read(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, DeviceError> {
        qc_remote_mcp::as_resource_read(self.invoke_as(
            method,
            params,
            crate::ConnectionUse::ExistingOnly,
        ))
        .await
    }
}

impl HeadlessDeviceAdapter {
    /// One call, in the connection mode the caller chose.
    async fn invoke_as(
        &self,
        method: &str,
        params: Value,
        connection: crate::ConnectionUse,
    ) -> Result<Value, DeviceError> {
        // A remote or MCP backup is written to this PC; the caller gets its
        // location, never the document (see `remote_backup`).
        crate::remote_backup::invoke_saving_backups(
            method,
            params,
            crate::remote_backup::default_backup_directory(),
            |params| self.invoke_gateway(method, params, connection),
        )
        .await
    }

    /// A resource read (`ExistingOnly`) may use the broker connection this
    /// host holds but never bring one up; decided under the gateway lock.
    async fn invoke_gateway(
        &self,
        method: &str,
        params: Value,
        connection: crate::ConnectionUse,
    ) -> Result<Value, DeviceError> {
        let gateway = self.gateway.clone();
        let lock_free_reads = self.lock_free_reads.clone();
        let stopped = self.stopped.clone();
        let method = method.to_owned();
        tokio::task::spawn_blocking(move || {
            // The same routing as a window's relay adapter, plus this host's
            // stop flag.
            crate::relay_gateway_request(
                &gateway,
                &lock_free_reads,
                &method,
                params,
                connection,
                Some(&*stopped),
            )
        })
        .await
        .map_err(|error| crate::relay_device_error(error.to_string()))?
        .map_err(crate::device_error_from_gateway_failure)
    }
}

/// How often a headless host re-reads the user's access level and the local
/// MCP switch.
///
/// A window applies a change the moment it is made, because the change is
/// made through that window's own [`crate::relay::RelayBridge`]. A headless
/// host shares no memory with the window where the user changes it; the only
/// channel between them is what the window persists - the access level in
/// Windows Credential Manager and the switch in `local-mcp-enabled.json` - so
/// the host polls both. Five seconds bounds how long a tightened access level
/// can go unenforced by a headless host, at the cost of one Credential Manager
/// read and one small file read per interval.
const HEADLESS_SETTINGS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Everything a headless bridge needs to decide whether, and how, to host.
///
/// The production values come from [`HeadlessHostSettings::for_this_user`];
/// tests substitute a scratch directory, an access level of their own, and
/// no USB session, so they never write the user's Credential Manager entry,
/// never meet the installed app's broker, and never touch a real device.
#[derive(Clone)]
pub(crate) struct HeadlessHostSettings {
    /// The directory a window's Tauri `app_local_data_dir` resolves to: the
    /// shared broker's descriptor and the local MCP switch both live here.
    app_local_data_dir: PathBuf,
    load_access_mode: Arc<dyn Fn() -> AccessMode + Send + Sync>,
    refresh_interval: Duration,
    /// Whether a broker this host spawns opens the device session itself;
    /// see `Gateway::auto_connect_first_broker`.
    auto_connect: bool,
}

impl HeadlessHostSettings {
    /// The settings a window of this user's installed app would apply.
    ///
    /// `None` when Windows reports no local data directory at all; the bridge
    /// then relays only, exactly as before it could host.
    pub(crate) fn for_this_user() -> Option<Self> {
        Some(Self {
            app_local_data_dir: app_local_data_dir()?,
            load_access_mode: Arc::new(crate::relay::load_access_mode),
            refresh_interval: HEADLESS_SETTINGS_REFRESH_INTERVAL,
            auto_connect: true,
        })
    }

    fn local_mcp_enabled(&self) -> bool {
        local_access::local_mcp_enabled_in(&self.app_local_data_dir)
    }

    fn device_broker_descriptor_path(&self) -> PathBuf {
        crate::device_broker_descriptor::device_broker_descriptor_path(&self.app_local_data_dir)
    }
}

/// What Tauri's `app_local_data_dir` resolves to for this app: the local
/// application data folder joined with the bundle identifier. The local MCP
/// descriptor's first Windows path uses the same identifier, and a test pins
/// `APP_DIRECTORY` to the identifier in `tauri.conf.json`.
///
/// Falls back to the OS temp directory when neither `LOCALAPPDATA` nor
/// `USERPROFILE` resolves, the same fallback the windowed app's own Tauri
/// `.setup()` now applies to `app.path().app_local_data_dir()`'s failure.
/// Without this, the two disagreed on that failure: the window would host or
/// attach the shared broker at `%TEMP%\QC Remote`, while this headless host
/// would give up on it entirely, per `for_this_user`'s doc comment, and
/// relay-only forever - the two processes on the same machine never finding
/// each other's broker.
pub(crate) fn app_local_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths::prepare_directory(&directory).ok()?;
        Some(directory)
    }
    #[cfg(not(target_os = "linux"))]
    {
        resolve_app_local_data_dir(
            std::env::var_os("LOCALAPPDATA"),
            std::env::var_os("USERPROFILE"),
        )
    }
}

/// `app_local_data_dir`'s resolution, taking the two environment lookups as
/// parameters so the fallback chain - including the all-else-fails branch,
/// which real env vars can't safely simulate in a parallel test run - is
/// directly testable rather than only reasoned about in a comment.
#[cfg(any(not(target_os = "linux"), test))]
fn resolve_app_local_data_dir(
    local_appdata: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    local_appdata
        .map(PathBuf::from)
        .or_else(|| {
            user_profile.map(|profile| PathBuf::from(profile).join("AppData").join("Local"))
        })
        .or_else(|| Some(std::env::temp_dir()))
        .map(|local| local.join(APP_DIRECTORY))
}

/// A headless host's listener, stopped at most once.
///
/// `RunningLocalAccess::stop` deletes every descriptor path it published,
/// whoever has written there since. The settings refresh and the bridge's own
/// shutdown can both stop a host; the second stop must not delete a
/// descriptor a window published after the first.
struct HostedListener {
    /// Behind a lock only because the listener holds a channel receiver,
    /// which cannot be shared between threads bare.
    running: Mutex<local_access::RunningLocalAccess>,
    stop_acknowledged: Mutex<Option<bool>>,
}

impl HostedListener {
    fn stop(&self) -> bool {
        let Ok(mut result) = self.stop_acknowledged.lock() else {
            return false;
        };
        if let Some(acknowledged) = *result {
            return acknowledged;
        }
        let acknowledged = match self.running.lock() {
            Ok(running) => running.stop(),
            Err(_) => return false,
        };
        if acknowledged {
            *result = Some(true);
        }
        acknowledged
    }
}

/// Test-only: where a test receives the listener `attempt_headless_mcp_hosting`
/// acquires for one state path, at the instant it is acquired - before the
/// settings watcher starts, before the host is returned, before the caller
/// stores it anywhere - so the test's cleanup owns it from then on.
#[cfg(test)]
type HostedListenerSink = Arc<Mutex<Vec<Arc<HostedListener>>>>;

/// Sinks keyed by the first state path, with whether to panic right after
/// the handle is stored (the injection point inside the acquisition).
#[cfg(test)]
static HOSTED_LISTENER_SINKS: Mutex<Vec<(PathBuf, HostedListenerSink, bool)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
fn register_hosted_listener(paths: &[PathBuf], running: &Arc<HostedListener>) {
    let Some(first) = paths.first() else {
        return;
    };
    let entry = HOSTED_LISTENER_SINKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(path, _, _)| path == first)
        .map(|(_, sink, panic_after)| (sink.clone(), *panic_after));
    if let Some((sink, panic_after)) = entry {
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(running.clone());
        if panic_after {
            panic!("injected panic inside attempt_headless_mcp_hosting after the listener");
        }
    }
}

/// Installs a hosted-listener sink for one state path for the life of the
/// guard.
#[cfg(test)]
struct HostedListenerSinkGuard(PathBuf);

#[cfg(test)]
impl HostedListenerSinkGuard {
    fn install(
        first_state_path: &std::path::Path,
        panic_after: bool,
    ) -> (Self, HostedListenerSink) {
        let sink = HostedListenerSink::default();
        HOSTED_LISTENER_SINKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((first_state_path.to_path_buf(), sink.clone(), panic_after));
        (Self(first_state_path.to_path_buf()), sink)
    }
}

#[cfg(test)]
impl Drop for HostedListenerSinkGuard {
    fn drop(&mut self) {
        HOSTED_LISTENER_SINKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(path, _, _)| *path != self.0);
    }
}

/// The published MCP-HTTP listener a headless bridge process is now hosting
/// itself, together with the device adapter and access-mode receiver it was
/// started with - reused so this same process's own stdio session answers
/// from the identical `Gateway`/device-broker connection its HTTP surface
/// does, rather than opening a second one or looping a request back through
/// its own HTTP listener.
pub(crate) struct HeadlessMcpHost {
    running: Arc<HostedListener>,
    /// The concrete adapter behind `adapter`, for tests that inspect which
    /// broker its gateway reached.
    #[cfg_attr(not(test), allow(dead_code))]
    device: HeadlessDeviceAdapter,
    adapter: Arc<dyn DeviceAdapter>,
    access: watch::Receiver<AccessMode>,
    /// Set once the user turns local MCP off; the host has then stopped
    /// serving and unpublished, and must not answer another request.
    disabled: Arc<AtomicBool>,
    /// Dropped, or sent to, to end the settings refresh thread.
    stop_refresh: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl HeadlessMcpHost {
    /// The instance id this process just published under, so a caller can
    /// prove it is what a fresh read of the descriptor finds - see
    /// `a_bridge_process_hosts_mcp_itself_when_no_publisher_is_live`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn instance_id(&self) -> String {
        self.running
            .running
            .lock()
            .map(|running| running.instance_id().to_owned())
            .unwrap_or_default()
    }

    /// Stops serving and unpublishes the descriptors this process wrote.
    pub(crate) fn stop(&self) {
        if let Ok(mut stop) = self.stop_refresh.lock() {
            stop.take();
        }
        self.running.stop();
    }

    fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    /// A [`local_access::LocalBackend`] backed by this host's own adapter and
    /// access-mode receiver - the exact gating the HTTP router applies,
    /// reused rather than reimplemented for the in-process stdio session.
    fn backend(&self) -> Arc<dyn QcBackend> {
        Arc::new(local_access::LocalBackend {
            adapter: self.adapter.clone(),
            access: self.access.clone(),
        })
    }
}

/// Keeps a headless host in step with what the user sets in a window: the
/// access level is re-read and re-applied, and a local MCP switch turned off
/// stops the host. Runs until the host stops or turns itself off.
fn refresh_headless_settings(
    settings: HeadlessHostSettings,
    access: watch::Sender<AccessMode>,
    running: Arc<HostedListener>,
    disabled: Arc<AtomicBool>,
    stop: std::sync::mpsc::Receiver<()>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    while let Err(RecvTimeoutError::Timeout) = stop.recv_timeout(settings.refresh_interval) {
        if !settings.local_mcp_enabled() {
            // The same outcome a window's off switch has on its own listener:
            // nothing is served and the descriptor is gone. Relaying resumes
            // for this bridge, and with nothing published that relay reports
            // QC Remote as offline - the behaviour before a bridge could host.
            disabled.store(true, Ordering::Release);
            running.stop();
            return;
        }
        let mode = (settings.load_access_mode)();
        access.send_if_modified(|current| {
            let changed = *current != mode;
            *current = mode;
            changed
        });
    }
}

/// Tries to become the MCP-HTTP publisher for this machine's headless bridge,
/// using the *existing*, unmodified `local_access::start_listener` - the same
/// publisher-ownership guard every GUI or headless process meets, whether it
/// is racing another bridge process or the GUI that starts publishing at the
/// same moment.
///
/// Honours the two choices a window honours: nothing is hosted while the user
/// has local MCP switched off, and the access level the user chose - not full
/// access - gates every request, kept current by
/// [`refresh_headless_settings`].
///
/// Returns `None` when local MCP is off, when it lost that race, or when
/// `start_listener` failed for any other reason: the caller falls back to
/// relaying through whatever process is (or becomes) live, rather than
/// treating this as fatal.
pub(crate) fn attempt_headless_mcp_hosting(
    paths: &[PathBuf],
    settings: &HeadlessHostSettings,
) -> Option<HeadlessMcpHost> {
    if !settings.local_mcp_enabled() {
        return None;
    }
    // The hosting admission itself: whoever decided to host (a bridge at
    // startup, or per request after waiting for its host lock and a blocking
    // thread) asked before waiting; a Quit recorded during that wait must not
    // get a fresh listener published. Rechecked here, immediately before the
    // listener starts.
    if crate::user_quit_refusal(&settings.device_broker_descriptor_path()).is_some() {
        return None;
    }
    let device = HeadlessDeviceAdapter::new(
        settings.device_broker_descriptor_path(),
        settings.auto_connect,
    );
    let adapter: Arc<dyn DeviceAdapter> = Arc::new(device.clone());
    let (access_tx, access_rx) = watch::channel((settings.load_access_mode)());
    match local_access::start_listener(paths.to_vec(), adapter.clone(), access_rx.clone()) {
        Ok(running) => {
            let running = Arc::new(HostedListener {
                running: Mutex::new(running),
                stop_acknowledged: Mutex::new(None),
            });
            #[cfg(test)]
            register_hosted_listener(paths, &running);
            let disabled = Arc::new(AtomicBool::new(false));
            let (stop_tx, stop_rx) = std::sync::mpsc::channel();
            let refresh = {
                let (settings, running, disabled) =
                    (settings.clone(), running.clone(), disabled.clone());
                std::thread::Builder::new()
                    .name("qc-headless-settings".into())
                    .spawn(move || {
                        refresh_headless_settings(settings, access_tx, running, disabled, stop_rx)
                    })
            };
            if let Err(error) = refresh {
                // Without the refresh, a later change in a window would never
                // reach this host. Serving a stale access level indefinitely
                // is worse than not hosting.
                eprintln!(
                    "QC Remote local MCP: this bridge could not watch the access settings, so it will not host: {error}"
                );
                running.stop();
                return None;
            }
            Some(HeadlessMcpHost {
                running,
                device,
                adapter,
                access: access_rx,
                disabled,
                stop_refresh: Mutex::new(Some(stop_tx)),
            })
        }
        Err(failure) => {
            // Losing the race is the ordinary case whenever a GUI window or
            // another bridge process is already live, so it says nothing on
            // stderr; anything else (a bind failure, a filesystem error) is
            // worth a line, since it means this bridge will fall back to
            // relaying for a reason other than "something else already
            // serves this".
            if !failure.served_by_another_instance {
                eprintln!(
                    "QC Remote local MCP: this bridge could not host the MCP-HTTP surface itself: {}",
                    failure.message
                );
            }
            None
        }
    }
}

/// The backend a bridge's stdio session answers from.
///
/// It relays to whatever process publishes the local MCP descriptor - a
/// window, or another bridge hosting headlessly - and hosts itself whenever
/// nobody does. That is decided per request, not once at startup: a bridge
/// that was relaying to another headless bridge would otherwise be stranded
/// the moment that bridge's MCP client closed it, with no window to take over.
struct BridgeBackend {
    relay: DesktopBackend,
    /// The local MCP descriptor paths; `None` when Windows reports no user
    /// data directories, which leaves the relay's own error to answer.
    paths: Option<Vec<PathBuf>>,
    /// `None` when the app's local data directory cannot be resolved; this
    /// bridge then only ever relays.
    settings: Option<HeadlessHostSettings>,
    host: tokio::sync::Mutex<Option<Arc<HeadlessMcpHost>>>,
}

impl BridgeBackend {
    fn new(
        relay: DesktopBackend,
        paths: Option<Vec<PathBuf>>,
        settings: Option<HeadlessHostSettings>,
        host: Option<HeadlessMcpHost>,
    ) -> Self {
        Self {
            relay,
            paths,
            settings,
            host: tokio::sync::Mutex::new(host.map(Arc::new)),
        }
    }

    /// The backend of the host this bridge runs, unless it has none or the
    /// user has since turned local MCP off.
    async fn hosted_backend(&self) -> Option<Arc<dyn QcBackend>> {
        let mut host = self.host.lock().await;
        if host.as_ref().is_some_and(|current| current.is_disabled()) {
            if let Some(stopped) = host.take() {
                stopped.stop();
            }
        }
        host.as_ref().map(|current| current.backend())
    }

    /// Become the host, if nobody else is one by now. Serialised by the
    /// `host` lock, so concurrent requests start at most one listener.
    async fn host_now(&self) -> Option<Arc<dyn QcBackend>> {
        let (Some(paths), Some(settings)) = (self.paths.clone(), self.settings.clone()) else {
            return None;
        };
        // Before waiting for the host lock or a blocking thread; the hosting
        // admission (`attempt_headless_mcp_hosting`) asks again after both.
        if self.user_quit_refusal().is_some() {
            return None;
        }
        let mut host = self.host.lock().await;
        if let Some(current) = host.as_ref() {
            return Some(current.backend());
        }
        let started =
            tokio::task::spawn_blocking(move || attempt_headless_mcp_hosting(&paths, &settings))
                .await
                .ok()
                .flatten()?;
        let started = Arc::new(started);
        *host = Some(started.clone());
        Some(started.backend())
    }

    async fn stop_hosting(&self) {
        if let Some(host) = self.host.lock().await.take() {
            host.stop();
        }
    }

    #[cfg(test)]
    async fn is_hosting(&self) -> bool {
        self.host.lock().await.is_some()
    }
}

#[async_trait]
impl QcBackend for BridgeBackend {
    async fn request(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        // A host this bridge already runs answers a resource read from the
        // broker connection it holds, or offline; its gateway decides that
        // under its own lock (`crate::ConnectionUse`).
        if let Some(backend) = self.hosted_backend().await {
            return backend.request(route, rpc, params).await;
        }
        match self.relay_once(route, rpc, params.clone()).await {
            // A resource read is the client reading context, not the chat
            // asking (`DesktopBackend::rpc_request` sends no session on it
            // for the same reason), and a host's device call spawns an
            // auto-connecting broker: with nobody to relay to, it is answered
            // offline, and this bridge does not host. Tool calls still do.
            Err(RelayFailure::NoPublisher(error)) if qc_remote_mcp::serving_resource_read() => {
                Err(error)
            }
            Err(RelayFailure::NoPublisher(error)) => {
                // After a user's Quit only the user brings QC Remote back: a
                // tool call neither hosts nor starts anything, and says how
                // to reconnect. Nobody publishes, so nothing was relayed.
                if let Some(advice) = self.user_quit_refusal() {
                    return Err(BackendError::unavailable(advice));
                }
                match self.host_now().await {
                    Some(backend) => backend.request(route, rpc, params).await,
                    // Hosting was refused. If that is because another process
                    // began publishing meanwhile, relay to it; the first attempt
                    // reached no one, so sending again cannot repeat anything.
                    None => match self.relay_once(route, rpc, params).await {
                        // Refused at the hosting admission because a Quit was
                        // recorded while this request waited: say so.
                        Err(RelayFailure::NoPublisher(_)) => match self.user_quit_refusal() {
                            Some(advice) => Err(BackendError::unavailable(advice)),
                            None => Err(error),
                        },
                        relayed => relayed.map_err(RelayFailure::into_backend_error),
                    },
                }
            }
            relayed => relayed.map_err(RelayFailure::into_backend_error),
        }
    }
}

impl BridgeBackend {
    /// `Some(advice)` while a user's Quit stands for the directory this bridge
    /// would host in (`crate::user_quit_refusal`). `None` when it cannot host
    /// at all: it then only ever relays, as before.
    fn user_quit_refusal(&self) -> Option<String> {
        let settings = self.settings.as_ref()?;
        crate::user_quit_refusal(&settings.device_broker_descriptor_path())
    }

    async fn relay_once(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, RelayFailure> {
        match &self.paths {
            Some(paths) if is_local_route(route) => self.relay.relay_at(paths, rpc, params).await,
            _ => self.relay.relay(route, rpc, params).await,
        }
    }
}

/// Serves MCP over stdio for a bridge process: a hosting bridge answers from
/// its own [`HeadlessMcpHost`], a relaying one through [`DesktopBackend`], and
/// either can become the other's replacement per [`BridgeBackend`].
async fn serve_bridge(
    host: Option<HeadlessMcpHost>,
    settings: Option<HeadlessHostSettings>,
) -> Result<(), String> {
    let client = crate::loopback_http::loopback_client_builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not initialize the local MCP client: {error}"))?;
    // One id for this bridge process: its heartbeat announces it and its tool
    // calls carry it, so the host can tell the chat asking again from the
    // bridge merely being alive.
    let session_id = format!("stdio-{:032x}", rand::random::<u128>());
    let backend = Arc::new(BridgeBackend::new(
        DesktopBackend {
            client: client.clone(),
            session_id: Some(Arc::from(session_id.as_str())),
        },
        state_paths().ok(),
        settings,
        host,
    ));
    let route = PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE);
    let service = QcMcp::for_route(backend.clone() as Arc<dyn QcBackend>, route)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| format!("Could not serve MCP over stdio: {error}"))?;
    // Identity comes from the MCP initialize exchange, not from installation
    // detection. Each bridge has its own lease, even two chats in one host.
    let client_name = service
        .peer()
        .peer_info()
        .map(|info| {
            info.client_info
                .title
                .as_deref()
                .filter(|title| {
                    !title.trim().is_empty()
                        && title.len() <= 128
                        && !title.chars().any(char::is_control)
                })
                .unwrap_or(&info.client_info.name)
                .to_owned()
        })
        .unwrap_or_else(|| "MCP client".into());
    // Shown in the broker icon's details for this session; the host ignores
    // a value it would not show.
    let client_version = service
        .peer()
        .peer_info()
        .map(|info| info.client_info.version.clone());
    let client_id = match announcement(configured_client_id()) {
        Announcement::As(id) => id,
        Announcement::Withheld(notice) => {
            eprintln!("{notice}");
            String::new()
        }
    };
    let heartbeat = {
        let heartbeat_client = client.clone();
        let session_id = session_id.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
            let started = std::time::Instant::now();
            let mut report = HeartbeatReport::default();
            loop {
                interval.tick().await;
                let outcome = send_heartbeat(
                    &heartbeat_client,
                    &client_id,
                    &session_id,
                    &client_name,
                    client_version.as_deref(),
                )
                .await;
                if let Some(line) = report.observe(
                    Some(&client_id),
                    outcome,
                    started.elapsed().as_millis() as u64,
                ) {
                    eprintln!("{line}");
                }
            }
        })
    };
    let result = service
        .waiting()
        .await
        .map(|_| ())
        .map_err(|error| format!("The MCP stdio connection failed: {error}"));
    heartbeat.abort();
    let _ = heartbeat.await;
    if let Ok(endpoint) = heartbeat_descriptor() {
        let close = async {
            endpoint.prove_identity(&client).await?;
            client
                .post(endpoint.rpc_url.clone())
                .bearer_auth(&endpoint.bearer_token)
                .json(
                    &serde_json::json!({"rpc":"local.bridgeClosed","params":{"sessionId":session_id}}),
                )
                .send()
                .await
                .map_err(|error| error.to_string())
        };
        let _ = tokio::time::timeout(Duration::from_millis(500), close).await;
    }
    backend.stop_hosting().await;
    result
}

pub fn run() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start the local MCP runtime: {error}"))?;
    runtime.block_on(async {
        // A live descriptor means some other process (a GUI window, or
        // another bridge that got here first) already publishes, so this
        // bridge relays to it. Only when nothing is live does it try to
        // become the publisher itself; if that also fails (local MCP is
        // switched off, it lost the race, or start_listener failed for any
        // other reason), it relays. Either way the question is asked again
        // whenever a relayed request finds nobody publishing; see
        // `BridgeBackend`.
        let settings = hosting_settings();
        let headless_host = startup_host(state_paths().ok(), settings.clone());
        serve_bridge(headless_host, settings).await
    })
}

/// Whether a bridge starting now hosts, and the host if it does: only when
/// nobody publishes, and never while a user's Quit stands (a new chat after
/// Quit stays disconnected like the ones already open).
fn startup_host(
    paths: Option<Vec<PathBuf>>,
    settings: Option<HeadlessHostSettings>,
) -> Option<HeadlessMcpHost> {
    paths
        .filter(|paths| descriptor_from_paths(paths).is_err())
        .zip(settings)
        .filter(|(_, settings)| {
            crate::user_quit_refusal(&settings.device_broker_descriptor_path()).is_none()
        })
        .and_then(|(paths, settings)| attempt_headless_mcp_hosting(&paths, &settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_bridge_never_publishes_where_the_state_override_points() {
        // The override still redirects reading in a release build (the
        // installer's native smoke test depends on it), but a release bridge
        // that would host writes its token to every path it reads, so with the
        // override set it must only relay.
        assert!(!may_publish_at_state_paths(false, true));
        assert!(may_publish_at_state_paths(false, false));
        assert!(may_publish_at_state_paths(true, true));
        assert!(may_publish_at_state_paths(true, false));
    }

    #[test]
    fn local_rpc_device_code_survives_deserialization_and_backend_mapping() {
        let envelope: RpcEnvelope = serde_json::from_value(serde_json::json!({
            "ok": false,
            "error": {
                "code": "device_error",
                "deviceCode": "DEVICE_SUSPENDED",
                "details": {"requestSent": false},
                "message": "A backup is in progress",
                "retryable": false
            }
        }))
        .expect("local RPC error envelope");
        let error = envelope.error.expect("local RPC error");
        let mapped = BackendError {
            code: backend_code(&error.code),
            message: error.message,
            retryable: error.retryable,
            detail_code: error.device_code,
            details: error.details,
        };

        assert_eq!(mapped.code, "device_error");
        assert_eq!(mapped.detail_code.as_deref(), Some("DEVICE_SUSPENDED"));
        assert_eq!(mapped.details.as_ref().unwrap()["requestSent"], false);
        assert!(!mapped.retryable);
    }

    fn document(url: &str, token: &str) -> EndpointDocument {
        EndpointDocument {
            schema_version: 1,
            rpc_url: url.into(),
            bearer_token: token.into(),
            published_at_unix_ms: 1,
            descriptor_revision: 4,
            health_url: url.replace("/rpc", "/health"),
            instance_id: "instance".into(),
            process_id: 1,
            identity_url: None,
        }
    }

    #[test]
    fn an_identity_address_off_the_rpc_origin_retires_the_descriptor() {
        let token = "B".repeat(43);
        let mut matching = document("http://127.0.0.1:41006/rpc", &token);
        matching.identity_url = Some("http://127.0.0.1:41006/identity".into());
        let endpoint = validate_descriptor(matching).expect("a matching identity address");
        assert_eq!(
            endpoint.identity_url.as_deref(),
            Some("http://127.0.0.1:41006/identity")
        );
        let mut elsewhere = document("http://127.0.0.1:41006/rpc", &token);
        elsewhere.identity_url = Some("http://127.0.0.1:41007/identity".into());
        assert!(validate_descriptor(elsewhere).is_err());
        let mut anonymous = document("http://127.0.0.1:41006/rpc", &token);
        anonymous.identity_url = Some("http://127.0.0.1:41006/identity".into());
        anonymous.instance_id = String::new();
        assert!(validate_descriptor(anonymous).is_err());
    }

    /// A process squatting on a dead app's port answers the identity check
    /// with the published instance id but cannot produce the proof. The
    /// bridge must then send it nothing: no token, no request.
    #[tokio::test]
    async fn a_squatter_on_the_published_port_never_receives_the_token() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = vec![0_u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                recorded
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer[..read]).into_owned());
                let body = r#"{"instanceId":"instance","proof":"forged"}"#;
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        let token = "S".repeat(43);
        let mut published = document(&format!("http://127.0.0.1:{port}/rpc"), &token);
        published.identity_url = Some(format!("http://127.0.0.1:{port}/identity"));
        let endpoint = validate_descriptor(published).unwrap();
        let backend = DesktopBackend {
            client: crate::loopback_http::loopback_client_builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            session_id: Some(Arc::from("stdio-squatter-must-not-see-this")),
        };
        let outcome = backend
            .send(endpoint, "system.status", Map::new())
            .await
            .expect_err("a forged proof must stop the request");
        assert!(matches!(outcome, RelayFailure::NoPublisher(_)));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "only the identity check was sent: {seen:?}");
        assert!(seen[0].starts_with("POST /identity "), "{}", seen[0]);
        assert!(!seen[0].contains(&token), "the token reached the squatter");
        assert!(!seen[0].to_ascii_lowercase().contains("authorization"));
        assert!(
            !seen[0].contains("stdio-squatter-must-not-see-this"),
            "the session id reached the squatter"
        );
    }

    /// A tool call names this bridge's session, so after the tray's Unlink the
    /// chat's next call (and not its heartbeat) lists it again. Nothing is
    /// printed from the built request: it carries the bearer token.
    #[test]
    fn relayed_tool_calls_carry_the_bridge_session_id_only_when_it_has_one() {
        let token = "T".repeat(43);
        let endpoint = validate_descriptor(document("http://127.0.0.1:41000/rpc", &token)).unwrap();
        let mut params = Map::new();
        params.insert("bpm".into(), serde_json::json!(120));
        let built = |session_id: Option<Arc<str>>| {
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id,
            }
            .rpc_request(&endpoint, "device.setTempo", params.clone())
            .build()
            .expect("request builds")
        };
        let request = built(Some(Arc::from("stdio-0123456789abcdef")));
        assert_eq!(request.url().as_str(), "http://127.0.0.1:41000/rpc");
        assert_eq!(
            request
                .headers()
                .get(crate::host_presence::BRIDGE_SESSION_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("stdio-0123456789abcdef")
        );
        assert!(request
            .headers()
            .contains_key(reqwest::header::AUTHORIZATION));
        let body: Value = serde_json::from_slice(
            request
                .body()
                .and_then(|body| body.as_bytes())
                .expect("JSON body"),
        )
        .expect("body is JSON");
        assert_eq!(
            body,
            serde_json::json!({"rpc":"device.setTempo","params":{"bpm":120}}),
            "the id travels in the header, never in the action's params"
        );
        let anonymous = built(None);
        assert!(!anonymous
            .headers()
            .contains_key(crate::host_presence::BRIDGE_SESSION_HEADER));
    }

    /// Every resource-backed method, `device.listModels` included (it is also
    /// the `list_models` tool), is sent without the session id when it serves
    /// a resource read, so reading context never relinks an unlinked chat.
    #[tokio::test]
    async fn resource_reads_never_carry_the_bridge_session_id() {
        let token = "T".repeat(43);
        let endpoint = validate_descriptor(document("http://127.0.0.1:41000/rpc", &token)).unwrap();
        let backend = DesktopBackend {
            client: reqwest::Client::new(),
            session_id: Some(Arc::from("stdio-0123456789abcdef")),
        };
        assert_eq!(qc_remote_mcp::resource_rpcs().len(), 3);
        for (uri, rpc) in qc_remote_mcp::resource_rpcs() {
            let read = qc_remote_mcp::as_resource_read(async {
                backend
                    .rpc_request(&endpoint, rpc, Map::new())
                    .build()
                    .expect("request builds")
            })
            .await;
            assert!(
                !read
                    .headers()
                    .contains_key(crate::host_presence::BRIDGE_SESSION_HEADER),
                "{uri} relinks"
            );
            let call = backend
                .rpc_request(&endpoint, rpc, Map::new())
                .build()
                .expect("request builds");
            assert!(
                call.headers()
                    .contains_key(crate::host_presence::BRIDGE_SESSION_HEADER),
                "the same rpc as a tool call names its session: {rpc}"
            );
        }
    }

    fn fixture_dir(name: &str) -> PathBuf {
        crate::tests::unique_fixture_directory(name)
    }

    fn write_descriptor(path: &PathBuf, port: u16, published_at_unix_ms: u64, revision: u8) {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "descriptorRevision": revision,
                "rpcUrl": format!("http://127.0.0.1:{port}/rpc"),
                "bearerToken": "A".repeat(43),
                "publishedAtUnixMs": published_at_unix_ms,
            }))
            .expect("descriptor JSON"),
        )
        .expect("write descriptor");
    }

    /// Replays a run of announcement outcomes at `HEARTBEAT_INTERVAL` and
    /// collects only the lines the policy chose to write.
    fn replay(
        client: Option<&str>,
        outcomes: impl IntoIterator<Item = Result<(), String>>,
    ) -> Vec<String> {
        let mut report = HeartbeatReport::default();
        let mut written = Vec::new();
        for (tick, outcome) in outcomes.into_iter().enumerate() {
            let elapsed_ms = tick as u64 * HEARTBEAT_INTERVAL.as_millis() as u64;
            if let Some(line) = report.observe(client, outcome, elapsed_ms) {
                written.push(line);
            }
        }
        written
    }

    #[test]
    fn a_heartbeat_failure_names_its_reason_and_the_host_it_belongs_to() {
        let written = replay(
            Some("codex:9.9.9"),
            [Err(
                "the loopback endpoint did not answer (refused)".to_string()
            )],
        );
        assert_eq!(written.len(), 1, "a failure must not be discarded");
        assert!(
            written[0].contains("codex:9.9.9"),
            "the line must say which host this bridge is: {}",
            written[0]
        );
        assert!(
            written[0].contains("the loopback endpoint did not answer (refused)"),
            "the line must carry the diagnosis the announcement produced: {}",
            written[0]
        );
    }

    #[test]
    fn an_announcement_that_keeps_failing_the_same_way_is_rate_limited() {
        // An hour of announcements at five seconds each, all failing
        // identically: the case that would otherwise write 720 lines.
        let ticks = 3_600 / HEARTBEAT_INTERVAL.as_secs() as usize;
        let per_repeat =
            (HEARTBEAT_FAILURE_REPEAT_AFTER.as_secs() / HEARTBEAT_INTERVAL.as_secs()) as usize;
        let written = replay(
            Some("codex:9.9.9"),
            vec![Err("the same reason".to_string()); ticks],
        );
        assert_eq!(
            written.len(),
            ticks.div_ceil(per_repeat),
            "one line for the failure and one per repeat interval, not {ticks}"
        );
        assert!(
            written[1].contains(&(per_repeat - 1).to_string()),
            "a repeat must account for the attempts it stood in for: {}",
            written[1]
        );
    }

    #[test]
    fn a_reason_that_changes_is_new_information_and_is_written_at_once() {
        let written = replay(
            Some("codex:9.9.9"),
            [
                Err("first reason".to_string()),
                Err("first reason".to_string()),
                Err("second reason".to_string()),
            ],
        );
        assert_eq!(written.len(), 2, "{written:?}");
        assert!(written[0].contains("first reason"));
        assert!(written[1].contains("second reason"));
    }

    #[test]
    fn recovery_is_written_once_and_a_healthy_bridge_stays_silent() {
        assert!(
            replay(Some("codex:9.9.9"), [Ok(()), Ok(()), Ok(())]).is_empty(),
            "a bridge that never failed has nothing to report"
        );
        let written = replay(
            Some("codex:9.9.9"),
            [
                Err("a reason".to_string()),
                Err("a reason".to_string()),
                Ok(()),
                Ok(()),
            ],
        );
        assert_eq!(written.len(), 2, "{written:?}");
        assert!(
            written[1].contains('2') && written[1].contains("a reason"),
            "recovery must say how many attempts failed and why: {}",
            written[1]
        );
    }

    #[test]
    fn the_heartbeat_is_handed_the_diagnosis_rather_than_stderr_taking_it() {
        let directory = fixture_dir("quiet");
        let missing = directory.join("nothing-here.json");

        // The announcement runs every five seconds forever, so a reader that
        // printed on every failed read wrote the lines the rate limiter exists
        // to prevent. The detail comes back instead.
        let reason = heartbeat_descriptor_from_paths(std::slice::from_ref(&missing), 1_000)
            .expect_err("no descriptor means the app is not running");
        assert!(
            reason.contains(OFFLINE_ADVICE),
            "the heartbeat still has to know what to tell the user: {reason}"
        );
        assert!(
            reason.contains("nothing-here"),
            "the detail must reach the caller that decides whether to write it: {reason}"
        );
        // The request path is unchanged: there the diagnosis goes to stderr and
        // the user's own filesystem stays out of the message.
        let request_path = descriptor_from_paths_at(&[missing], 1_000)
            .expect_err("no descriptor means the app is not running");
        assert_eq!(request_path, OFFLINE_ADVICE);

        // And once it is a reason, the same reason on every tick writes once.
        let written = replay(Some("codex:9.9.9"), vec![Err(reason); 3]);
        assert_eq!(written.len(), 1, "{written:?}");
        assert!(written[0].contains("nothing-here"));
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// The heartbeat's reason is the rate limiter's key, so it has to be the
    /// same string while the same thing is wrong. A descriptor an abnormally
    /// exited app left behind is never re-stamped, so anything derived from its
    /// age is different on every tick.
    #[test]
    fn a_descriptor_that_stopped_being_refreshed_reads_the_same_at_every_clock() {
        let directory = fixture_dir("stale-key");
        let stale = directory.join("stale.json");
        // Refreshed 200s ago against a 150s allowance, and nothing will
        // refresh it again.
        write_descriptor(&stale, 41004, 1_000_000, 4);
        let paths = [stale];
        let tick_ms = HEARTBEAT_INTERVAL.as_millis() as u64;

        let now = heartbeat_descriptor_from_paths(&paths, 1_200_000)
            .expect_err("an unrefreshed descriptor must not be dialled");
        let a_tick_later = heartbeat_descriptor_from_paths(&paths, 1_200_000 + tick_ms)
            .expect_err("an unrefreshed descriptor must not be dialled");
        let an_hour_later = heartbeat_descriptor_from_paths(&paths, 1_200_000 + 3_600_000)
            .expect_err("an unrefreshed descriptor must not be dialled");

        assert_eq!(
            now, a_tick_later,
            "one tick of the clock is not new information"
        );
        assert_eq!(
            now, an_hour_later,
            "an hour of the same dead descriptor is not new information"
        );
        assert!(
            now.contains(OFFLINE_ADVICE) && now.contains(HEARTBEAT_EXPIRED_NOTE),
            "the heartbeat still has to say what is wrong: {now}"
        );

        // The request path keeps the age: a tool call asked for this, and it is
        // not on a five-second loop.
        let absence = choose_descriptor(&paths, 1_200_000)
            .err()
            .expect("an unrefreshed descriptor leaves nothing usable");
        assert_eq!(
            absence.expired_ages_ms,
            vec![200_000],
            "the age goes back as a number for the caller that may name it"
        );
        let printed = request_path_detail(absence.detail, &absence.expired_ages_ms);
        assert!(
            printed.ends_with("; a descriptor last refreshed 200s ago was ignored"),
            "the request path's wording is unchanged: {printed}"
        );
        assert_eq!(
            descriptor_from_paths_at(&paths, 1_200_000).expect_err("nothing usable"),
            OFFLINE_ADVICE,
            "and the user still gets the sentence, not the filesystem"
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// An app killed rather than closed leaves its descriptor on disk forever,
    /// and this is the run that produced: 720 lines an hour, from the reader
    /// the rate limiter was supposed to be covering.
    #[test]
    fn an_hour_of_one_abandoned_descriptor_is_not_an_hour_of_lines() {
        let directory = fixture_dir("stale-hour");
        let stale = directory.join("stale.json");
        write_descriptor(&stale, 41005, 1_000_000, 4);
        let paths = [stale];

        let ticks = 3_600 / HEARTBEAT_INTERVAL.as_secs() as usize;
        let per_repeat =
            (HEARTBEAT_FAILURE_REPEAT_AFTER.as_secs() / HEARTBEAT_INTERVAL.as_secs()) as usize;
        // The reasons the loop really produces, read at each tick's own clock.
        // A test that wrote the strings itself would agree with itself and see
        // nothing.
        let reasons = (0..ticks)
            .map(|tick| {
                let now_ms = 1_200_000 + tick as u64 * HEARTBEAT_INTERVAL.as_millis() as u64;
                Err(heartbeat_descriptor_from_paths(&paths, now_ms)
                    .expect_err("an unrefreshed descriptor must not be dialled"))
            })
            .collect::<Vec<Result<(), String>>>();
        assert_eq!(reasons.len(), ticks, "an hour of announcements to replay");

        let written = replay(Some("codex:9.9.9"), reasons);
        assert_eq!(
            written.len(),
            ticks.div_ceil(per_repeat),
            "one line for the failure and one per repeat interval, not {ticks}: {written:?}"
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// The name libtest registers the re-executed test under, the variable
    /// that tells a process it is the child, and the line the child writes last.
    ///
    /// A `--exact` filter that matches nothing runs no tests and still exits 0,
    /// so the parent insists on seeing the last line rather than trusting the
    /// status alone.
    const STDERR_CHILD_TEST: &str =
        "local_stdio::tests::the_request_path_alone_reports_the_copy_the_app_abandoned";
    const STDERR_CHILD_VAR: &str = "QC_REMOTE_LOCAL_MCP_STDERR_CHILD";
    const STDERR_CHILD_FINISHED: &str = "the stderr fixture ran to the end";

    /// Both readers, over one fixture, in a re-executed child process.
    ///
    /// `eprintln!` is not observable in-process on stable Rust -
    /// `std::io::set_output_capture` is unstable - so an in-process test could assert
    /// what the request path *would* print but not that the heartbeat's own
    /// read printed nothing. A child whose stderr the parent reads can.
    ///
    /// One live descriptor beside one the app stopped refreshing. The request
    /// path owes whoever asked for a descriptor a line naming the abandoned
    /// copy. The heartbeat's read succeeds, so it owes nothing at all - and a
    /// line from it would be written every five seconds for the life of the
    /// host, which is the run this separation exists to prevent.
    #[test]
    fn the_request_path_alone_reports_the_copy_the_app_abandoned() {
        if std::env::var_os(STDERR_CHILD_VAR).is_some() {
            // `fixture_dir` keys on the process id, so this is the child's own
            // directory and the child is the one that removes it.
            let directory = fixture_dir("stderr-child");
            let live = directory.join("live.json");
            let abandoned = directory.join("abandoned.json");
            write_descriptor(&live, 41006, 1_190_000, 4);
            write_descriptor(&abandoned, 41007, 1_000_000, 4);
            let paths = [live, abandoned];

            let announced = heartbeat_descriptor_from_paths(&paths, 1_200_000)
                .expect("the refreshed descriptor is still usable");
            assert_eq!(announced.rpc_url.as_str(), "http://127.0.0.1:41006/rpc");
            let requested = descriptor_from_paths_at(&paths, 1_200_000)
                .expect("the refreshed descriptor is still usable");
            assert_eq!(requested.rpc_url.as_str(), "http://127.0.0.1:41006/rpc");

            std::fs::remove_dir_all(directory).expect("remove fixture");
            eprintln!("{STDERR_CHILD_FINISHED}");
            return;
        }

        let output = std::process::Command::new(
            std::env::current_exe().expect("this test binary re-executes itself"),
        )
        .args(["--exact", STDERR_CHILD_TEST, "--nocapture"])
        .env(STDERR_CHILD_VAR, "1")
        .output()
        .expect("re-execute this test binary as a child");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "a child that panicked would otherwise read as a silent pass: {stderr}"
        );
        assert!(
            stderr.contains(STDERR_CHILD_FINISHED),
            "the child has to say it ran, because a filter matching nothing also exits 0: {stderr}"
        );

        // Both wordings end in these two words, so one count covers the request
        // path's sentence and the heartbeat's age-free note together.
        assert_eq!(
            stderr.matches("was ignored").count(),
            1,
            "exactly one of the two readers may name the abandoned copy: {stderr}"
        );
        assert!(
            stderr.contains(&expired_note(200_000)),
            "and it is the request path, with the age a tool call is entitled to: {stderr}"
        );
        assert!(
            !stderr.contains(HEARTBEAT_EXPIRED_NOTE),
            "a successful announcement reports nothing, expired copies included: {stderr}"
        );
    }

    /// Withheld means the configured-client identity is unavailable. This unit
    /// tests that classification only. The serving loop still sends the session
    /// heartbeat, which renews linked-session presence independently of
    /// configured-client credit and can keep the background host in use.
    #[test]
    fn a_bridge_without_configured_identity_is_withheld_for_attribution() {
        let Announcement::Withheld(notice) = announcement(None) else {
            panic!("missing configured identity must withhold configured-client attribution")
        };
        assert!(
            notice.contains(CLIENT_ID_ARGUMENT),
            "the single line written instead must name the argument that fixes it: {notice}"
        );
        assert!(
            matches!(announcement(Some(String::new())), Announcement::Withheld(_)),
            "an empty configured-client id earns no client credit; local_rpc still answers 200 and separately records an eligible linked session"
        );
        let Announcement::As(client) = announcement(Some("codex:9.9.9".to_string())) else {
            panic!("a named bridge still announces itself every interval")
        };
        assert_eq!(client, "codex:9.9.9");
    }

    #[test]
    fn a_bridge_with_no_configured_identity_is_still_attributable() {
        let written = replay(None, [Err("a reason".to_string())]);
        assert_eq!(written.len(), 1);
        assert!(
            written[0].contains("unidentified"),
            "an anonymous bridge must be described, not left blank: {}",
            written[0]
        );
        assert!(
            anonymous_bridge_notice().contains(CLIENT_ID_ARGUMENT),
            "the start-up notice must name the argument that fixes it"
        );
    }

    #[test]
    fn a_descriptor_the_app_stopped_refreshing_is_not_used() {
        let directory = fixture_dir("expired");
        let expired = directory.join("expired.json");
        // Refreshed 200s ago against a 150s allowance.
        write_descriptor(&expired, 41000, 1_000_000, 4);

        let error = descriptor_from_paths_at(&[expired], 1_200_000)
            .expect_err("an unrefreshed descriptor must not be dialled");
        assert_eq!(error, OFFLINE_ADVICE);
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn a_live_older_instance_outranks_an_expired_newer_one() {
        let directory = fixture_dir("outranks");
        let live = directory.join("live.json");
        let expired = directory.join("expired.json");
        write_descriptor(&live, 41001, 1_190_000, 4);
        write_descriptor(&expired, 41002, 1_000_000, 4);

        let endpoint = descriptor_from_paths_at(&[live, expired], 1_200_000)
            .expect("the refreshed descriptor must win");
        assert_eq!(endpoint.rpc_url.as_str(), "http://127.0.0.1:41001/rpc");
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn a_release_that_never_refreshes_is_exempt_from_expiry() {
        let directory = fixture_dir("legacy");
        let legacy = directory.join("legacy.json");
        // descriptorRevision 3 stamps once at startup, so age says nothing.
        write_descriptor(&legacy, 41003, 1_000, 3);

        let endpoint = descriptor_from_paths_at(&[legacy], 9_999_999)
            .expect("a pre-refresh release must still be reachable");
        assert_eq!(endpoint.rpc_url.as_str(), "http://127.0.0.1:41003/rpc");
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn an_absent_app_is_reported_without_naming_the_users_filesystem() {
        let directory = fixture_dir("absent");
        let missing = directory.join("nothing-here.json");

        let error = descriptor_from_paths_at(&[missing], 1_000)
            .expect_err("no descriptor means the app is not running");
        assert_eq!(error, OFFLINE_ADVICE);
        assert!(!error.contains("nothing-here"), "paths belong on stderr");
        assert!(
            error.contains("Start QC Remote"),
            "the message must name the action"
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn an_unreachable_app_is_reported_without_the_endpoint_or_the_transport_wording() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .expect("client");
        // Nothing is listening: a descriptor left behind by a closed app.
        let error = client
            .post("http://127.0.0.1:1/rpc")
            .send()
            .await
            .expect_err("a dead loopback port must not answer");

        let mapped = unreachable_app(error).into_backend_error();
        assert_eq!(mapped.message, OFFLINE_ADVICE);
        assert!(
            !mapped.message.contains("127.0.0.1"),
            "the endpoint belongs on stderr"
        );
        assert!(
            !mapped
                .message
                .to_lowercase()
                .contains("error sending request"),
            "transport wording must not reach the user"
        );
        assert!(mapped.message.contains("Start QC Remote"));
    }

    /// I4: a connection that never opened delivered nothing, so another host
    /// may answer the request; anything past the connect might have been
    /// acted on and must not be replayed elsewhere.
    #[tokio::test]
    async fn only_a_failed_connect_counts_as_nobody_publishing() {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .build()
            .expect("client");
        let error = client
            .post("http://127.0.0.1:1/rpc")
            .send()
            .await
            .expect_err("a dead loopback port must not answer");
        assert!(error.is_connect(), "{error}");
        assert!(matches!(
            unreachable_app(error),
            RelayFailure::NoPublisher(_)
        ));

        // A listener that accepts and then never answers: the request was
        // delivered, and only the response timed out.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .expect("client");
        let error = client
            .post(format!("http://127.0.0.1:{port}/rpc"))
            .send()
            .await
            .expect_err("nothing ever answers");
        assert!(!error.is_connect(), "{error}");
        assert!(matches!(unreachable_app(error), RelayFailure::Refused(_)));
        drop(listener);
    }

    #[test]
    fn descriptor_accepts_only_the_authenticated_loopback_rpc_endpoint() {
        let token = "A".repeat(43);
        assert!(validate_descriptor(document("http://127.0.0.1:41000/rpc", &token)).is_ok());
        for url in [
            "https://127.0.0.1:41000/rpc",
            "http://localhost:41000/rpc",
            "http://127.0.0.1:41000/mcp",
            "http://127.0.0.1/rpc",
            "http://user@127.0.0.1:41000/rpc",
            "http://127.0.0.1:41000/rpc?token=x",
        ] {
            assert!(validate_descriptor(document(url, &token)).is_err(), "{url}");
        }
        assert!(validate_descriptor(document("http://127.0.0.1:41000/rpc", "short")).is_err());
    }

    #[test]
    fn descriptor_falls_back_from_a_stale_copy_to_a_valid_copy() {
        let directory = std::env::temp_dir().join(format!(
            "qc-native-local-descriptor-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("descriptor fixture directory");
        let stale = directory.join("stale.json");
        let current = directory.join("current.json");
        std::fs::write(&stale, b"not JSON").expect("stale descriptor");
        std::fs::write(
            &current,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "rpcUrl": "http://127.0.0.1:41000/rpc",
                "bearerToken": "A".repeat(43),
            }))
            .expect("descriptor JSON"),
        )
        .expect("current descriptor");

        let endpoint = descriptor_from_paths(&[stale, current]).expect("fallback descriptor");
        assert_eq!(endpoint.rpc_url.as_str(), "http://127.0.0.1:41000/rpc");
        std::fs::remove_dir_all(directory).expect("remove descriptor fixture");
    }

    #[test]
    fn descriptor_prefers_the_newest_valid_mirror_over_path_order() {
        let directory = std::env::temp_dir().join(format!(
            "qc-native-local-descriptor-freshness-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("descriptor fixture directory");
        let old_primary = directory.join("primary.json");
        let current_mirror = directory.join("mirror.json");
        std::fs::write(
            &old_primary,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "rpcUrl": "http://127.0.0.1:14990/rpc",
                "bearerToken": "A".repeat(43),
                "publishedAtUnixMs": 100,
            }))
            .expect("primary descriptor JSON"),
        )
        .expect("primary descriptor");
        std::fs::write(
            &current_mirror,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "rpcUrl": "http://127.0.0.1:53343/rpc",
                "bearerToken": "B".repeat(43),
                "publishedAtUnixMs": 200,
            }))
            .expect("mirror descriptor JSON"),
        )
        .expect("mirror descriptor");

        let endpoint =
            descriptor_from_paths(&[old_primary, current_mirror]).expect("freshest descriptor");
        assert_eq!(endpoint.rpc_url.as_str(), "http://127.0.0.1:53343/rpc");
        assert_eq!(endpoint.bearer_token, "B".repeat(43));
        std::fs::remove_dir_all(directory).expect("remove descriptor fixture");
    }

    /// Settings for a headless host that touch nothing of the user's: the
    /// broker descriptor and the local MCP switch live in `directory`, the
    /// access level is whatever `mode` holds, and no broker it spawns opens a
    /// USB session (the machine running the tests may have a real Quad Cortex
    /// in use by the installed app).
    fn test_settings(
        directory: &std::path::Path,
        mode: Arc<Mutex<AccessMode>>,
    ) -> HeadlessHostSettings {
        HeadlessHostSettings {
            app_local_data_dir: directory.to_path_buf(),
            load_access_mode: Arc::new(move || *mode.lock().expect("mode")),
            refresh_interval: Duration::from_millis(20),
            auto_connect: false,
        }
    }

    fn full_access() -> Arc<Mutex<AccessMode>> {
        Arc::new(Mutex::new(AccessMode::Full))
    }

    fn fresh_fixture_dir(name: &str) -> PathBuf {
        fixture_dir(name)
    }

    fn wait_for(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        condition()
    }

    fn local_route() -> PrincipalRoute {
        PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE)
    }

    /// A method only full access permits, to observe which access level a
    /// host enforces without ever reaching a device.
    const WRITE_RPC: &str = "device.setParameter";

    #[test]
    fn a_write_is_refused_below_full_access() {
        assert!(AccessMode::Full.permits(WRITE_RPC));
        assert!(!AccessMode::ReadOnly.permits(WRITE_RPC));
    }

    #[test]
    fn empty_background_host_does_not_attach_and_passive_presence_honors_explicit_release() {
        let directory = fresh_fixture_dir("background-no-client");
        let device = HeadlessDeviceAdapter::new(directory.join("device-broker.json"), false);
        device.release_when_unused();
        assert!(device.gateway.lock().unwrap().process.is_none());
        device.gateway.lock().unwrap().intentional_release = true;
        device.retain_while_used();
        let gateway = device.gateway.lock().unwrap();
        assert!(
            gateway.intentional_release,
            "passive presence cannot undo a tray release"
        );
        assert!(
            gateway.process.is_none(),
            "no broker spawned from passive presence after release"
        );
        assert!(!directory.join("device-broker.json").exists());
        drop(gateway);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn active_background_client_retains_one_shared_participant_and_idle_drops_it() {
        let _serialize = crate::tests::serialize_real_broker_test();
        let directory = fresh_fixture_dir("background-participant");
        let descriptor = crate::device_broker_descriptor::device_broker_descriptor_path(&directory);
        // Bound before the adapter, so it drops after it: the broker can end
        // through its own grace period, then the directory goes.
        let _cleanup = crate::tests::armed_broker_teardown(&descriptor);
        let device = HeadlessDeviceAdapter::new(descriptor, false);
        device.retain_while_used();
        let pid = device
            .gateway
            .lock()
            .unwrap()
            .process
            .as_ref()
            .expect("active client attaches")
            .broker_pid;
        device.retain_while_used();
        assert_eq!(
            device
                .gateway
                .lock()
                .unwrap()
                .process
                .as_ref()
                .unwrap()
                .broker_pid,
            pid
        );
        device.release_when_unused();
        assert!(
            device.gateway.lock().unwrap().process.is_none(),
            "listener alone cannot retain a broker participant"
        );
    }

    /// Black-box proof that a headless bridge process with no live publisher
    /// becomes one itself, and is discoverable through the *existing*,
    /// unmodified `descriptor_from_paths_at` - not through any special-cased
    /// internal state only this bridge's own code understands.
    #[test]
    fn a_bridge_process_hosts_mcp_itself_when_no_publisher_is_live() {
        let directory = fresh_fixture_dir("headless-host");
        let paths = vec![directory.join(STATE_FILE)];
        assert!(
            descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
            "precondition: nothing is publishing yet"
        );

        let hosted =
            attempt_headless_mcp_hosting(&paths, &test_settings(&directory, full_access()))
                .expect("a bridge process with no live publisher must become one");

        // It must now be discoverable exactly like a GUI-hosted publisher.
        let discovered = descriptor_from_paths_at(&paths, now_unix_ms())
            .expect("the newly self-hosted endpoint must be discoverable");
        assert_eq!(discovered.instance_id, hosted.instance_id());

        hosted.stop();
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// Two processes racing to self-host: exactly one becomes the publisher,
    /// and the other must fall back to relaying rather than also trying to
    /// serve. Reuses `local_access`'s own single-instance-guard behavior
    /// (`a_second_instance_does_not_take_over_a_live_instances_descriptor`)
    /// rather than reinventing the race here.
    #[test]
    fn a_bridge_process_falls_back_to_relaying_when_it_loses_the_hosting_race() {
        let directory = fresh_fixture_dir("headless-race");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());

        let first =
            attempt_headless_mcp_hosting(&paths, &settings).expect("the first process hosts");

        // A second process attempting the same thing must not also become a
        // publisher; it must detect the live one and behave as an ordinary
        // relay client instead.
        let second_attempt = attempt_headless_mcp_hosting(&paths, &settings);
        assert!(
            second_attempt.is_none(),
            "a second process must not also host when one is already live"
        );

        // The first is still the one and only publisher.
        let discovered = descriptor_from_paths_at(&paths, now_unix_ms())
            .expect("the first process's endpoint is still discoverable");
        assert_eq!(discovered.instance_id, first.instance_id());

        first.stop();
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// C1: the headless host's first real device call must reach the shared
    /// broker every window uses - the one published in the app's local data
    /// directory - and not a private `--stdio` broker with its own USB
    /// connection. The descriptor path used to be known only to Tauri setup,
    /// which a `--local-mcp-bridge` process never runs.
    #[test]
    fn a_headless_hosts_device_calls_reach_the_shared_broker() {
        let _serialize = crate::tests::serialize_real_broker_test();
        let directory = fresh_fixture_dir("headless-shared-broker");
        let paths = vec![directory.join(STATE_FILE)];
        // Bound before the host, so it drops after it: the broker can end
        // through its own grace period, then the directory goes.
        let _cleanup = crate::tests::armed_broker_teardown(
            &crate::device_broker_descriptor::device_broker_descriptor_path(&directory),
        );
        let hosted =
            attempt_headless_mcp_hosting(&paths, &test_settings(&directory, full_access()))
                .expect("the bridge hosts");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let status = runtime
            .block_on(
                hosted
                    .adapter
                    .invoke(crate::rpc::RUNTIME_STATUS, serde_json::json!({})),
            )
            .expect("the first device call is answered");
        assert!(status.get("gatewayApiVersion").is_some(), "{status}");

        let gateway = hosted.device.gateway.lock().expect("gateway");
        let process = gateway.process.as_ref().expect("the call started a broker");
        assert!(
            process.uses_shared_broker(),
            "the call must go through the shared broker, not a private child"
        );
        let published = crate::device_broker_descriptor::read_descriptor(
            &crate::device_broker_descriptor::device_broker_descriptor_path(&directory),
            crate::device_broker_descriptor::unix_time_ms(),
        )
        .expect("the broker is published where every window looks for it");
        assert_eq!(published.process_id, process.broker_pid);
        drop(gateway);

        hosted.stop();
    }

    /// C2: a user who switched local MCP off gets no headless host either.
    #[test]
    fn a_bridge_does_not_host_while_local_mcp_is_switched_off() {
        let directory = fresh_fixture_dir("headless-switched-off");
        let paths = vec![directory.join(STATE_FILE)];
        local_access::store_local_mcp_enabled_in(&directory, false).expect("preference");

        assert!(
            attempt_headless_mcp_hosting(&paths, &test_settings(&directory, full_access()))
                .is_none(),
            "local MCP is off, so nothing may be served"
        );
        assert!(
            descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
            "and nothing is published"
        );
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// C2: the host enforces the access level the user chose, not full
    /// access, and follows a later change made in a window.
    #[test]
    fn a_headless_host_enforces_and_follows_the_persisted_access_level() {
        let directory = fresh_fixture_dir("headless-access");
        let paths = vec![directory.join(STATE_FILE)];
        let mode = Arc::new(Mutex::new(AccessMode::ReadOnly));
        let hosted = attempt_headless_mcp_hosting(&paths, &test_settings(&directory, mode.clone()))
            .expect("the bridge hosts");
        assert_eq!(*hosted.access.borrow(), AccessMode::ReadOnly);

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let refused = runtime
            .block_on(
                hosted
                    .backend()
                    .request(&local_route(), WRITE_RPC, Map::new()),
            )
            .expect_err("read-only access refuses a write");
        assert_eq!(refused.code, "forbidden");

        *mode.lock().unwrap() = AccessMode::Full;
        assert!(
            wait_for(Duration::from_secs(5), || *hosted.access.borrow()
                == AccessMode::Full),
            "a change the user makes later reaches the running host"
        );
        hosted.stop();
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// C2: switching local MCP off while a headless host serves stops it and
    /// unpublishes it, as the switch does for a window's own listener.
    #[test]
    fn switching_local_mcp_off_stops_a_running_headless_host() {
        let directory = fresh_fixture_dir("headless-switch-off-later");
        let paths = vec![directory.join(STATE_FILE)];
        let hosted =
            attempt_headless_mcp_hosting(&paths, &test_settings(&directory, full_access()))
                .expect("the bridge hosts");

        local_access::store_local_mcp_enabled_in(&directory, false).expect("preference");
        assert!(
            wait_for(Duration::from_secs(5), || hosted.is_disabled()),
            "the host notices the switch"
        );
        // `is_disabled()` flips in memory before the unpublish (a file
        // removal) necessarily lands on disk - under heavy disk contention
        // (e.g. many concurrent test runs on the same machine) there is a
        // brief window where the flag is set but the descriptor file has not
        // been removed yet. Poll rather than check once.
        assert!(
            wait_for(Duration::from_secs(5), || descriptor_from_paths_at(
                &paths,
                now_unix_ms()
            )
            .is_err()),
            "and is no longer published"
        );
        hosted.stop();
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// `/health` and the relay probe clones of one adapter. A read that
    /// overlaps another must wait for and share that read's answer: refusing
    /// it gave an occasional false 503 and a relay tick with
    /// `usbConnected: false` that flapped the cloud status. Only one broker
    /// read runs for both.
    #[test]
    fn overlapping_readiness_reads_share_one_broker_read() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        // Nothing here runs on a timer: the first broker read is released
        // only once the second caller is registered on it, and the budget is
        // far beyond any scheduling delay. The timeouts below are safety
        // nets that fail the test by name, never a path to a pass.
        const SAFETY: Duration = Duration::from_secs(60);
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let released_by_safety_net = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(Some(release_rx)));
        let (counted, safety_net) = (probes.clone(), released_by_safety_net.clone());
        let adapter = HeadlessDeviceAdapter::new(std::env::temp_dir().join("unused.json"), false)
            .with_broker_read_budget(SAFETY * 2)
            .with_broker_readiness(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                // Only the first read waits to be released.
                if let Some(release) = release_rx.lock().unwrap().take() {
                    let _ = entered_tx.lock().unwrap().send(());
                    if release.recv_timeout(SAFETY).is_err() {
                        safety_net.store(true, Ordering::SeqCst);
                    }
                }
                BrokerDeviceReadiness::Ready
            }));
        adapter.release_when_unused();

        let first = runtime.spawn({
            let adapter = adapter.clone();
            async move { adapter.readiness().await }
        });
        entered
            .recv_timeout(SAFETY)
            .expect("the first read reached the broker");
        let second = runtime.spawn({
            let adapter = adapter.clone();
            async move { adapter.readiness().await }
        });
        let joined = wait_for(SAFETY, || adapter.broker_read_waiters() == 2);
        release.send(()).expect("release the broker read");
        assert!(joined, "the second read never joined the first");
        assert_eq!(runtime.block_on(first).unwrap(), HostReadiness::Ready);
        assert_eq!(runtime.block_on(second).unwrap(), HostReadiness::Ready);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "one broker read served both"
        );
        assert_eq!(adapter.broker_read_waiters(), 0, "the read left the slot");
        assert!(
            !released_by_safety_net.load(Ordering::SeqCst),
            "the broker read was released by the safety net, not by the test"
        );

        // The next read is a fresh one.
        assert_eq!(runtime.block_on(adapter.readiness()), HostReadiness::Ready);
        assert_eq!(probes.load(Ordering::SeqCst), 2);
    }

    /// The budget belongs to the shared read, not to each caller. A caller
    /// joining a hung read 1.5 s into its 2 s budget waits only what remains,
    /// and one joining after the deadline answers at once: the relay awaits
    /// readiness inside its message loop, so a hung read must not stall it
    /// for a fresh two seconds per probe.
    #[test]
    fn a_late_joiner_waits_only_for_what_remains_of_the_shared_read() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let counted = probes.clone();
        let adapter = HeadlessDeviceAdapter::new(std::env::temp_dir().join("unused.json"), false)
            .with_broker_readiness(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(30));
                BrokerDeviceReadiness::Ready
            }));
        adapter.release_when_unused();
        assert_eq!(adapter.broker_read_budget, Duration::from_secs(2));

        let read_started = std::time::Instant::now();
        let first = runtime.spawn({
            let adapter = adapter.clone();
            async move { adapter.readiness().await }
        });
        assert!(wait_for(Duration::from_secs(1), || adapter
            .broker_read_waiters()
            == 1));
        std::thread::sleep(Duration::from_millis(1500).saturating_sub(read_started.elapsed()));

        let joined = std::time::Instant::now();
        let HostReadiness::NotReady(reason) = runtime.block_on(adapter.readiness()) else {
            panic!("a hung read answered ready");
        };
        let waited = joined.elapsed();
        assert!(reason.contains("did not answer"), "{reason}");
        assert!(
            waited < Duration::from_millis(900),
            "a joiner 1.5 s in waited {waited:?}, not what remained of the budget"
        );

        let HostReadiness::NotReady(_) = runtime.block_on(first).unwrap() else {
            panic!("the hung read answered ready");
        };
        // Past the deadline, a joiner answers at once.
        let joined = std::time::Instant::now();
        let HostReadiness::NotReady(_) = runtime.block_on(adapter.readiness()) else {
            panic!("ready while the read is hung");
        };
        assert!(joined.elapsed() < Duration::from_millis(200));
        assert_eq!(probes.load(Ordering::SeqCst), 1, "a hung read piled up");
        release.send(()).expect("release");
    }

    /// While a request holds the gateway lock, a host with a broker in its
    /// process answers from the broker, not from the last published value.
    #[test]
    fn a_busy_gateway_answers_from_the_broker_when_there_is_one() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        for (answer, ready) in [
            (BrokerDeviceReadiness::Ready, true),
            (
                BrokerDeviceReadiness::NotReady {
                    phase: "disconnected".into(),
                },
                false,
            ),
        ] {
            let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = asked.clone();
            let adapter =
                HeadlessDeviceAdapter::new(std::env::temp_dir().join("unused.json"), false)
                    .with_broker_readiness(Arc::new(move || {
                        counted.fetch_add(1, Ordering::SeqCst);
                        answer.clone()
                    }));
            let busy = adapter.gateway.clone();
            let (held_tx, held) = std::sync::mpsc::channel();
            let (release_tx, release) = std::sync::mpsc::channel::<()>();
            let holder = std::thread::spawn(move || {
                let _guard = busy.lock().expect("gateway");
                held_tx.send(()).expect("held");
                let _ = release.recv();
            });
            held.recv().expect("the gateway is busy");
            let observed = runtime.block_on(adapter.readiness());
            release_tx.send(()).expect("release");
            holder.join().expect("holder");
            assert_eq!(observed == HostReadiness::Ready, ready, "{observed:?}");
            assert_eq!(asked.load(Ordering::SeqCst), 1, "the broker was asked");
        }
    }

    /// I3: a readiness probe must not wait for the gateway lock - a backup
    /// holds it for minutes, and the probe runs on an async worker thread.
    #[test]
    fn a_headless_readiness_probe_does_not_wait_for_a_busy_gateway() {
        let adapter = HeadlessDeviceAdapter::new(std::env::temp_dir().join("unused.json"), false);
        let busy = adapter.gateway.clone();
        let (held_tx, held) = std::sync::mpsc::channel();
        let (release_tx, release) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _guard = busy.lock().expect("gateway");
            held_tx.send(()).expect("held");
            let _ = release.recv();
        });
        held.recv().expect("the gateway is busy");

        // On a thread of its own: a probe that does block would block the
        // runtime driving it too, and no async timeout could then fire.
        let (answered_tx, answered) = std::sync::mpsc::channel();
        let probe = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let _ = answered_tx.send(runtime.block_on(adapter.ready()));
        });
        assert!(
            answered.recv_timeout(Duration::from_secs(2)).is_ok(),
            "the probe answered while the gateway was busy"
        );

        release_tx.send(()).expect("release");
        holder.join().expect("holder");
        probe.join().expect("probe");
    }

    /// I4: a bridge that was relaying to another headless host must take over
    /// when that host goes away, not be stranded with nobody to relay to.
    #[test]
    fn a_relaying_bridge_hosts_itself_once_the_publisher_it_used_is_gone() {
        let directory = fresh_fixture_dir("headless-takeover");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, Arc::new(Mutex::new(AccessMode::ReadOnly)));
        let first = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge A hosts");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        // Bridge B, relaying to A through the same descriptor paths.
        let bridge = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            Some(settings.clone()),
            None,
        );
        let relayed = runtime
            .block_on(bridge.request(&local_route(), WRITE_RPC, Map::new()))
            .expect_err("A answers, and read-only access refuses the write");
        assert_eq!(relayed.code, "forbidden", "{}", relayed.message);
        assert!(
            !runtime.block_on(bridge.is_hosting()),
            "B relays while A hosts"
        );

        // A's MCP client closes it.
        first.stop();
        assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_err());

        let answered = runtime
            .block_on(bridge.request(&local_route(), WRITE_RPC, Map::new()))
            .expect_err("B answers now, still under read-only access");
        assert_eq!(answered.code, "forbidden", "{}", answered.message);
        assert!(
            runtime.block_on(bridge.is_hosting()),
            "with nobody publishing, B became the host itself"
        );
        let discovered = descriptor_from_paths_at(&paths, now_unix_ms())
            .expect("and it is discoverable in turn");
        assert_ne!(discovered.instance_id, first.instance_id());

        runtime.block_on(bridge.stop_hosting());
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// I5: a headless host has no window to ask for the device session, so
    /// the broker it spawns must open it on its own.
    #[test]
    fn a_headless_host_for_this_user_opens_the_device_session_itself() {
        let settings = HeadlessHostSettings::for_this_user()
            .expect("the test machine has a local data directory");
        assert!(settings.auto_connect);
        let adapter = HeadlessDeviceAdapter::new(
            settings.device_broker_descriptor_path(),
            settings.auto_connect,
        );
        assert!(adapter.gateway.lock().unwrap().broker_auto_connect());
    }

    /// C1: the headless host must look for the shared broker where a window
    /// does - Tauri's `app_local_data_dir`, the local data folder joined with
    /// the bundle identifier - or the two would elect separate brokers.
    #[cfg(windows)]
    #[test]
    fn a_headless_host_uses_the_windows_app_local_data_directory() {
        let config: Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(config["identifier"], APP_DIRECTORY);
        let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA");
        let settings = HeadlessHostSettings::for_this_user().expect("settings");
        assert_eq!(
            settings.app_local_data_dir,
            PathBuf::from(local).join(APP_DIRECTORY)
        );
        assert_eq!(
            settings.device_broker_descriptor_path(),
            crate::device_broker_descriptor::device_broker_descriptor_path(
                &PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join(APP_DIRECTORY)
            )
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_headless_host_uses_the_shared_linux_state_directory() {
        let settings = HeadlessHostSettings::for_this_user().expect("settings");
        let expected = crate::broker_paths::app_local_data_dir();
        assert_eq!(settings.app_local_data_dir, expected);
        assert_eq!(
            settings.device_broker_descriptor_path(),
            crate::device_broker_descriptor::device_broker_descriptor_path(&expected)
        );
    }

    /// The windowed app's own Tauri `.setup()` falls back to
    /// `std::env::temp_dir().join(&app.config().identifier)` when
    /// `app_local_data_dir()` fails - `identifier` proven equal to
    /// `APP_DIRECTORY` by the `config["identifier"]` assertion above. This
    /// exercises the actual fallback chain (via injected env values, not
    /// real ones - clearing real `LOCALAPPDATA`/`USERPROFILE` in-process is
    /// unsafe under a parallel test run) to prove `resolve_app_local_data_dir`
    /// lands on that identical directory when both are absent, not just that
    /// it's reasoned to by construction.
    #[test]
    fn a_missing_local_appdata_and_user_profile_falls_back_to_temp_dir() {
        assert_eq!(
            resolve_app_local_data_dir(None, None),
            Some(std::env::temp_dir().join(APP_DIRECTORY))
        );
    }

    #[test]
    fn a_present_local_appdata_is_used_as_is() {
        assert_eq!(
            resolve_app_local_data_dir(Some("C:\\Users\\test\\AppData\\Local".into()), None),
            Some(PathBuf::from("C:\\Users\\test\\AppData\\Local").join(APP_DIRECTORY))
        );
    }

    #[test]
    fn a_missing_local_appdata_falls_back_to_user_profile() {
        assert_eq!(
            resolve_app_local_data_dir(None, Some("C:\\Users\\test".into())),
            Some(
                PathBuf::from("C:\\Users\\test")
                    .join("AppData")
                    .join("Local")
                    .join(APP_DIRECTORY)
            )
        );
    }

    /// Serves `backend` through the real `QcMcp` stdio surface, as
    /// `serve_bridge` does, over an in-memory pipe instead of stdin and
    /// stdout, and returns the answer to each of `requests` (method and
    /// params; the id is assigned here), sent one at a time after the
    /// initialize handshake. A resource read therefore reaches `backend`
    /// inside the very `as_resource_read` scope `QcMcp::read_resource`
    /// enters, and a tool call reaches it outside any.
    async fn mcp_exchange(backend: Arc<dyn QcBackend>, requests: &[Value]) -> Vec<Value> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let server = QcMcp::for_route(backend, local_route());
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let serving = tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let (read, mut write) = tokio::io::split(client_io);
        let mut lines = BufReader::new(read).lines();
        let initialize = serde_json::json!({
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "resource-origin-test", "version": "1"}
            }
        });
        let mut answers = Vec::new();
        for (id, request) in std::iter::once(&initialize).chain(requests).enumerate() {
            let mut message = request.clone();
            message["jsonrpc"] = Value::from("2.0");
            message["id"] = Value::from(id);
            write
                .write_all(format!("{message}\n").as_bytes())
                .await
                .expect("request written");
            let answer = tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    let line = lines
                        .next_line()
                        .await
                        .expect("response read")
                        .expect("the server is still answering");
                    let value: Value = serde_json::from_str(&line).expect("response JSON");
                    if value["id"] == id {
                        break value;
                    }
                }
            })
            .await
            .expect("answered within a minute");
            if id == 0 {
                write
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                    .await
                    .expect("initialized written");
            } else {
                answers.push(answer);
            }
        }
        drop(lines);
        drop(write);
        let _ = tokio::time::timeout(Duration::from_secs(5), serving).await;
        answers
    }

    /// One `resources/read` for every backend-answered resource.
    fn backend_resource_reads() -> Vec<Value> {
        let reads: Vec<Value> = qc_remote_mcp::resource_rpcs()
            .into_iter()
            .map(|(uri, _)| serde_json::json!({"method": "resources/read", "params": {"uri": uri}}))
            .collect();
        assert_eq!(reads.len(), 3, "{reads:?}");
        reads
    }

    /// The `list_models` tool: the same rpc as `qc://models`, asked by the
    /// chat.
    fn list_models_tool_call() -> Value {
        serde_json::json!({
            "method": "tools/call",
            "params": {"name": "list_models", "arguments": {"query": null}}
        })
    }

    /// Every answer is the structured offline error a bridge with nobody to
    /// relay to gives.
    fn assert_offline(requests: &[Value], answers: &[Value]) {
        assert_eq!(answers.len(), requests.len(), "{answers:?}");
        for (request, answer) in requests.iter().zip(answers) {
            assert_eq!(
                answer["error"]["data"]["code"], "device_unavailable",
                "{request} -> {answer}"
            );
            assert_eq!(answer["error"]["data"]["retryable"], true, "{answer}");
            assert_eq!(answer["error"]["message"], OFFLINE_ADVICE, "{answer}");
        }
    }

    /// A bridge that relays through `paths`, as `serve_bridge` builds one.
    fn relaying_bridge(
        paths: &[PathBuf],
        settings: &HeadlessHostSettings,
        session: &str,
    ) -> Arc<BridgeBackend> {
        Arc::new(BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: Some(Arc::from(session)),
            },
            Some(paths.to_vec()),
            Some(settings.clone()),
            None,
        ))
    }

    /// A relaying bridge whose publisher has quit: a host published at
    /// `paths` and stopped, as the tray's Quit stops the resident host and
    /// unpublishes it, and the user's Quit recorded as the quitting broker
    /// records it (`record_user_quit`).
    fn bridge_after_quit(paths: &[PathBuf], settings: &HeadlessHostSettings) -> Arc<BridgeBackend> {
        let bridge = bridge_after_publisher_left(paths, settings);
        record_user_quit(settings, "quit-generation");
        bridge
    }

    /// A relaying bridge whose publisher stopped without a user's Quit: a
    /// window closed, or another bridge's client went away.
    fn bridge_after_publisher_left(
        paths: &[PathBuf],
        settings: &HeadlessHostSettings,
    ) -> Arc<BridgeBackend> {
        let resident = attempt_headless_mcp_hosting(paths, settings).expect("a host to quit");
        resident.stop();
        assert!(
            descriptor_from_paths_at(paths, now_unix_ms()).is_err(),
            "precondition: nobody publishes after the Quit"
        );
        relaying_bridge(paths, settings, "stdio-after-quit-test")
    }

    /// The record the broker writes when the user quits generation
    /// `instance` (`services/device-broker/src/user_quit.rs`), byte for byte.
    fn record_user_quit(settings: &HeadlessHostSettings, instance: &str) {
        std::fs::write(
            crate::device_broker_descriptor::user_quit_path(
                &settings.device_broker_descriptor_path(),
            ),
            format!(r#"{{"instanceId":"{instance}","source":"tray"}}"#),
        )
        .expect("the user's Quit recorded");
    }

    /// A scratch directory of this test's own: a fresh random name, created
    /// exclusively, removed when the test ends or unwinds.
    fn exclusive_fixture_dir(name: &str) -> (PathBuf, crate::tests::RemoveDirOnDrop) {
        let directory = std::env::temp_dir().join(format!(
            "qc-native-local-{name}-{:032x}",
            rand::random::<u128>()
        ));
        std::fs::create_dir(&directory).expect("a scratch directory of this test's own");
        let guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
        (directory, guard)
    }

    /// Bridge A relays MCP resource reads over the
    /// authenticated loopback `/rpc` into bridge B, which hosts with no
    /// broker connection and no broker anywhere. B must start, attach to,
    /// elect or spawn no broker, and A must not host; every read gets the
    /// structured offline answer. Before v2 the relayed request lost its
    /// origin and B spawned a broker for it.
    #[test]
    fn a_relayed_resource_read_never_makes_the_hosting_bridge_start_a_broker() {
        let directory = fresh_fixture_dir("relayed-resource-read");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let hosting = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge B hosts");
        let device = hosting.device.clone();
        let broker_descriptor = settings.device_broker_descriptor_path();
        let relaying = relaying_bridge(&paths, &settings, "stdio-relaying-bridge-a");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let reads = backend_resource_reads();
        let answers =
            runtime.block_on(mcp_exchange(relaying.clone() as Arc<dyn QcBackend>, &reads));

        assert_offline(&reads, &answers);
        assert!(
            !device.gateway_attached(),
            "B's gateway brought no broker connection up"
        );
        assert!(
            !broker_descriptor.exists(),
            "no broker was spawned or published"
        );
        assert!(
            !runtime.block_on(relaying.is_hosting()),
            "A relayed and did not host"
        );
        assert_eq!(
            descriptor_from_paths_at(&paths, now_unix_ms())
                .expect("B still publishes")
                .instance_id,
            hosting.instance_id()
        );
        hosting.stop();
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// Control (HTTP): the `list_models` tool, relayed the same way,
    /// still lets B bring a broker up, as before. The broker opens no USB
    /// session (`test_settings`); like C1 it outlives the bridges by its
    /// grace period, which the teardown waits out before the fixture goes.
    #[test]
    fn a_relayed_tool_call_still_lets_the_hosting_bridge_start_a_broker() {
        let _serialize = crate::tests::serialize_real_broker_test();
        let directory = fresh_fixture_dir("relayed-tool-call");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        // Bound before both bridges, so it drops after them.
        let _cleanup =
            crate::tests::armed_broker_teardown(&settings.device_broker_descriptor_path());
        let hosting = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge B hosts");
        let device = hosting.device.clone();
        let relaying = relaying_bridge(&paths, &settings, "stdio-relaying-tool-call");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let answers = runtime.block_on(mcp_exchange(
            relaying.clone() as Arc<dyn QcBackend>,
            &[list_models_tool_call()],
        ));

        assert!(
            device.gateway_attached(),
            "a relayed tool call still brings B's broker connection up: {answers:?}"
        );
        assert!(
            crate::device_broker_descriptor::read_descriptor(
                &settings.device_broker_descriptor_path(),
                crate::device_broker_descriptor::unix_time_ms(),
            )
            .is_some(),
            "and the broker it reached is published"
        );
        assert!(!runtime.block_on(relaying.is_hosting()));
        hosting.stop();
    }

    /// With nobody publishing, an MCP resource read must not make a
    /// relaying bridge host. A hosting bridge's device call spawns an
    /// auto-connecting broker: after Quit, only the user brings it back.
    #[test]
    fn a_resource_read_after_quit_does_not_make_the_bridge_host() {
        let directory = fresh_fixture_dir("resource-read-after-quit");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let bridge = bridge_after_quit(&paths, &settings);

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let reads = backend_resource_reads();
        let answers = runtime.block_on(mcp_exchange(bridge.clone() as Arc<dyn QcBackend>, &reads));

        assert_offline(&reads, &answers);
        assert!(
            !runtime.block_on(bridge.is_hosting()),
            "a resource read must not make the bridge host: {answers:?}"
        );
        assert!(
            descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
            "and nothing is published"
        );
        assert!(!settings.device_broker_descriptor_path().exists());
        runtime.block_on(bridge.stop_hosting());
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// The answer to one tool call, as the client sees it, in one string.
    fn tool_answer_text(answers: &[Value]) -> String {
        assert_eq!(answers.len(), 1, "{answers:?}");
        answers[0].to_string()
    }

    /// After the user's Quit, the `list_models` tool does not make the bridge
    /// host and starts nothing. It is answered `device_unavailable`,
    /// retryable, saying QC Remote was quit and how to reconnect. The record
    /// still stands. No process is started: the start seam counts any start that is attempted.
    #[test]
    fn a_tool_call_after_quit_does_not_make_the_bridge_host() {
        let (directory, _directory_guard) = exclusive_fixture_dir("tool-call-after-quit");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let seam = crate::GatewayStartSeam::install(&settings.device_broker_descriptor_path());
        let bridge = bridge_after_quit(&paths, &settings);

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let answers = runtime.block_on(mcp_exchange(
            bridge.clone() as Arc<dyn QcBackend>,
            &[list_models_tool_call()],
        ));

        let answer = tool_answer_text(&answers);
        assert!(answer.contains(POST_QUIT_ADVICE), "{answer}");
        assert!(answer.contains("device_unavailable"), "{answer}");
        assert!(!answer.contains("from its icon"), "{answer}");
        assert!(
            !runtime.block_on(bridge.is_hosting()),
            "a tool call after Quit must not make the bridge host"
        );
        assert!(
            descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
            "and nothing is published"
        );
        assert_eq!(seam.starts(), 0, "no broker start was attempted");
        assert!(
            crate::device_broker_descriptor::read_user_quit(
                &settings.device_broker_descriptor_path()
            )
            .is_some(),
            "a tool call never clears the Quit"
        );
        runtime.block_on(bridge.stop_hosting());
    }

    /// First use is unchanged. Nobody publishes and no Quit was
    /// recorded (a window closed, another bridge's client went away, or
    /// nothing ever ran), so the tool call makes the bridge host and reach
    /// for a broker, here the start seam, which starts no process.
    #[test]
    fn a_tool_call_with_no_recorded_quit_still_makes_the_bridge_host() {
        let (directory, _directory_guard) = exclusive_fixture_dir("tool-call-first-use");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let seam = crate::GatewayStartSeam::install(&settings.device_broker_descriptor_path());
        let bridge = bridge_after_publisher_left(&paths, &settings);

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let answers = runtime.block_on(mcp_exchange(
            bridge.clone() as Arc<dyn QcBackend>,
            &[list_models_tool_call()],
        ));

        assert!(
            runtime.block_on(bridge.is_hosting()),
            "a tool call still makes the bridge host: {answers:?}"
        );
        assert!(
            descriptor_from_paths_at(&paths, now_unix_ms()).is_ok(),
            "and it publishes"
        );
        assert!(seam.starts() >= 1, "and its gateway reaches for a broker");
        let answer = tool_answer_text(&answers);
        assert!(!answer.contains(POST_QUIT_ADVICE), "{answer}");
        runtime.block_on(bridge.stop_hosting());
    }

    /// An explicit user start supersedes the Quit (it clears the
    /// record, `crate::acknowledge_user_connect_in`), and the next tool call
    /// hosts again.
    #[test]
    fn an_explicit_start_after_quit_lets_a_tool_call_host_again() {
        let (directory, _directory_guard) = exclusive_fixture_dir("explicit-start-after-quit");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let seam = crate::GatewayStartSeam::install(&settings.device_broker_descriptor_path());
        let bridge = bridge_after_quit(&paths, &settings);
        assert!(
            crate::acknowledge_user_connect_in(&settings.device_broker_descriptor_path()).is_ok()
        );

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let answers = runtime.block_on(mcp_exchange(
            bridge.clone() as Arc<dyn QcBackend>,
            &[list_models_tool_call()],
        ));

        assert!(
            runtime.block_on(bridge.is_hosting()),
            "the Quit no longer stands: {answers:?}"
        );
        assert!(seam.starts() >= 1);
        runtime.block_on(bridge.stop_hosting());
    }

    /// A bridge starting after the Quit (a new chat) does not
    /// host either; with no Quit recorded it hosts as before.
    #[test]
    fn a_new_bridge_after_quit_does_not_host_at_startup() {
        let (directory, _directory_guard) = exclusive_fixture_dir("startup-after-quit");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        record_user_quit(&settings, "quit-generation");
        assert!(startup_host(Some(paths.clone()), Some(settings.clone())).is_none());
        assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_err());

        crate::device_broker_descriptor::clear_user_quit(&settings.device_broker_descriptor_path())
            .expect("cleared");
        let host = startup_host(Some(paths.clone()), Some(settings))
            .expect("with no Quit recorded a starting bridge hosts");
        assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_ok());
        host.stop();
    }

    /// A bridge that decided to host before a Quit was recorded
    /// (it waited for its host lock and a blocking thread meanwhile) is
    /// refused at the hosting admission itself: no listener is published, and
    /// `host_now` refuses before it waits at all. With the Quit cleared, the
    /// same admission hosts.
    #[test]
    fn the_hosting_admission_refuses_a_quit_recorded_while_the_bridge_waited() {
        let (directory, _directory_guard) = exclusive_fixture_dir("admission-after-wait");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let bridge = bridge_after_publisher_left(&paths, &settings);
        // Decided to host with nothing recorded; the Quit lands during the wait.
        record_user_quit(&settings, "quit-during-the-wait");
        assert!(attempt_headless_mcp_hosting(&paths, &settings).is_none());
        assert!(
            descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
            "no fresh listener was published"
        );
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        assert!(runtime.block_on(bridge.host_now()).is_none());
        assert!(!runtime.block_on(bridge.is_hosting()));

        crate::device_broker_descriptor::clear_user_quit(&settings.device_broker_descriptor_path())
            .expect("cleared");
        let host = attempt_headless_mcp_hosting(&paths, &settings)
            .expect("with the Quit cleared the same admission hosts");
        assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_ok());
        host.stop();
    }

    /// A bridge that hosts while its gateway holds no broker connection -
    /// as `run` leaves one that started while nobody published - answers a
    /// resource read in its own process offline, without attaching to,
    /// recovering or spawning a broker, and goes on hosting.
    #[test]
    fn a_bridge_hosting_after_quit_answers_a_resource_read_offline() {
        let directory = fresh_fixture_dir("hosting-resource-read-after-quit");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let hosted = attempt_headless_mcp_hosting(&paths, &settings)
            .expect("the bridge hosts, as `run` does when nobody publishes");
        let device = hosted.device.clone();
        assert!(
            !device.gateway_attached(),
            "precondition: no broker connection"
        );
        let bridge = Arc::new(BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: Some(Arc::from("stdio-hosting-after-quit-test")),
            },
            Some(paths.clone()),
            Some(settings.clone()),
            Some(hosted),
        ));

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let reads = backend_resource_reads();
        let answers = runtime.block_on(mcp_exchange(bridge.clone() as Arc<dyn QcBackend>, &reads));

        assert_offline(&reads, &answers);
        assert!(
            !device.gateway_attached(),
            "a resource read must not bring a broker connection up"
        );
        assert!(!settings.device_broker_descriptor_path().exists());
        assert!(
            runtime.block_on(bridge.is_hosting()),
            "the MCP surface it hosts is left as it was"
        );
        runtime.block_on(bridge.stop_hosting());
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// Initialize (every exchange starts with it), the listings and
    /// the static guide and knowledge resources never reach a backend, so
    /// they neither make a bridge host after Quit nor bring a hosting
    /// bridge's broker connection up through a relay. The heartbeat is
    /// passive by construction: `send_heartbeat` only reads the descriptor
    /// and posts `local.bridgeHeartbeat`, which `local_rpc` answers from
    /// presence alone, and holds no `BridgeBackend`.
    #[test]
    fn listings_and_static_resources_stay_passive_after_quit_and_through_a_relay() {
        let passive = [
            serde_json::json!({"method": "tools/list", "params": {}}),
            serde_json::json!({"method": "resources/list", "params": {}}),
            serde_json::json!({
                "method": "resources/read",
                "params": {"uri": "qc://assistant-guide"}
            }),
            serde_json::json!({
                "method": "resources/read",
                "params": {"uri": "qc://device-knowledge/guide"}
            }),
        ];
        let runtime = tokio::runtime::Runtime::new().expect("runtime");

        let directory = fresh_fixture_dir("passive-after-quit");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let bridge = bridge_after_quit(&paths, &settings);
        let answers =
            runtime.block_on(mcp_exchange(bridge.clone() as Arc<dyn QcBackend>, &passive));
        assert_eq!(
            answers.len(),
            4,
            "every passive request must return a response"
        );
        for answer in &answers {
            assert!(answer.get("error").is_none(), "{answer}");
        }
        assert!(!runtime.block_on(bridge.is_hosting()), "nothing here hosts");
        assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_err());
        std::fs::remove_dir_all(directory).expect("remove fixture");

        let directory = fresh_fixture_dir("passive-through-relay");
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let hosting = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge B hosts");
        let device = hosting.device.clone();
        let relaying = relaying_bridge(&paths, &settings, "stdio-passive-relay");
        let answers = runtime.block_on(mcp_exchange(
            relaying.clone() as Arc<dyn QcBackend>,
            &passive,
        ));
        assert_eq!(
            answers.len(),
            4,
            "every passive request must return a response"
        );
        for answer in &answers {
            assert!(answer.get("error").is_none(), "{answer}");
        }
        assert!(!device.gateway_attached(), "B stays detached");
        assert!(!settings.device_broker_descriptor_path().exists());
        assert!(!runtime.block_on(relaying.is_hosting()));
        hosting.stop();
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    /// A relayed resource read says so on the wire, since the
    /// body cannot (`qc://models` and `list_models` send the same rpc), and
    /// a tool call never does, with or without a session to name.
    #[tokio::test]
    async fn relayed_resource_reads_carry_their_origin_and_tool_calls_do_not() {
        let token = "T".repeat(43);
        let endpoint = validate_descriptor(document("http://127.0.0.1:41000/rpc", &token)).unwrap();
        for session_id in [Some(Arc::<str>::from("stdio-0123456789abcdef")), None] {
            let backend = DesktopBackend {
                client: reqwest::Client::new(),
                session_id,
            };
            for (uri, rpc) in qc_remote_mcp::resource_rpcs() {
                let read = qc_remote_mcp::as_resource_read(async {
                    backend
                        .rpc_request(&endpoint, rpc, Map::new())
                        .build()
                        .expect("request builds")
                })
                .await;
                assert_eq!(
                    read.headers()
                        .get(local_access::REQUEST_ORIGIN_HEADER)
                        .and_then(|value| value.to_str().ok()),
                    Some(local_access::RESOURCE_READ_ORIGIN),
                    "{uri}"
                );
                let call = backend
                    .rpc_request(&endpoint, rpc, Map::new())
                    .build()
                    .expect("request builds");
                assert!(
                    !call
                        .headers()
                        .contains_key(local_access::REQUEST_ORIGIN_HEADER),
                    "a tool call is never marked as a resource read: {rpc}"
                );
            }
        }
    }

    /// The listener a hosting fixture started, read from its published
    /// descriptor: where it accepts, and which instance it is.
    #[derive(Clone)]
    struct HostedListenerIdentity {
        address: std::net::SocketAddr,
        instance_id: String,
    }

    fn published_listener(paths: &[PathBuf]) -> Option<HostedListenerIdentity> {
        let endpoint = descriptor_from_paths_at(paths, now_unix_ms()).ok()?;
        use std::net::ToSocketAddrs as _;
        let address = format!(
            "{}:{}",
            endpoint.rpc_url.host_str()?,
            endpoint.rpc_url.port_or_known_default()?
        )
        .to_socket_addrs()
        .ok()?
        .next()?;
        Some(HostedListenerIdentity {
            address,
            instance_id: endpoint.instance_id,
        })
    }

    /// A bridge hosting the local MCP surface in its own process, with no
    /// broker connection, as `run` leaves one when nobody publishes.
    struct HostingFixture {
        directory: PathBuf,
        settings: HeadlessHostSettings,
        bridge: Arc<BridgeBackend>,
        runtime: Arc<tokio::runtime::Runtime>,
        listener: Option<HostedListenerIdentity>,
    }

    /// Where `start_hosting_fixture` fails on purpose, for the startup tests:
    /// after the directory, runtime, listener and bridge are acquired and
    /// before the fixture is handed to the test.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum StartFault {
        None,
        ReturnError,
        Panic,
        /// A panic inside `attempt_headless_mcp_hosting`, right after
        /// the listener was acquired and its handle handed to the test.
        PanicInsideHostingAfterListener,
        /// A panic the instant `attempt_headless_mcp_hosting`
        /// returns, before the fixture stores the host or the bridge.
        PanicBeforeBridgeSlot,
    }

    /// What a startup reached, so a test can check it after a startup fault.
    #[derive(Default)]
    struct StartupSeen {
        directory: Option<PathBuf>,
        listener: Option<std::net::SocketAddr>,
        /// A bounded test-only injection into the real listener teardown,
        /// applied after the listener was really stopped.
        cleanup_fault: CleanupFault,
        /// Set by the runtime teardown when it has shut the runtime down.
        runtime_shut_down: Arc<AtomicBool>,
        /// Set from the listener thread's stop acknowledgment, which is more
        /// direct evidence than a loopback probe on Windows.
        listener_stop_acknowledged: Arc<AtomicBool>,
        /// The listener address the listener teardown found published before it
        /// stopped it, for startups that fail before recording it.
        cleanup_saw_listener: Arc<std::sync::Mutex<Option<std::net::SocketAddr>>>,
    }

    /// What the listener teardown is made to do after stopping the listener
    /// for real, so the tests can prove what follows.
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    enum CleanupFault {
        #[default]
        None,
        /// Report the stop as not acknowledged.
        ListenerUnacknowledged,
        /// Panic inside the step.
        ListenerPanics,
    }

    type Slot<T> = Arc<std::sync::Mutex<Option<T>>>;

    fn slot<T>() -> Slot<T> {
        Arc::new(std::sync::Mutex::new(None))
    }

    fn take<T>(slot: &Slot<T>) -> Option<T> {
        slot.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Starts a hosting fixture, arming each teardown before the acquisition
    /// it undoes, so a failure or panic anywhere in startup still releases
    /// everything acquired so far. Steps run in reverse: the listener (and
    /// bridge) stop first, then the runtime shuts down, then the directory
    /// goes - only if the listener was acknowledged stopped.
    fn start_hosting_fixture(
        name: &str,
        cleanup: &mut crate::tests::CleanupStack,
        fault: StartFault,
        seen: &std::sync::Mutex<StartupSeen>,
    ) -> Result<HostingFixture, String> {
        let directory = fixture_dir(name);
        let armed = directory.clone();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&armed)
        });
        seen.lock().unwrap().directory = Some(directory.clone());
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());

        let (cleanup_fault, runtime_shut_down, listener_stop_acknowledged, cleanup_saw_listener) = {
            let seen = seen.lock().unwrap();
            (
                seen.cleanup_fault,
                seen.runtime_shut_down.clone(),
                seen.listener_stop_acknowledged.clone(),
                seen.cleanup_saw_listener.clone(),
            )
        };
        let runtime_slot: Slot<Arc<tokio::runtime::Runtime>> = slot();
        let bridge_slot: Slot<Arc<BridgeBackend>> = slot();
        let device_slot: Slot<HeadlessDeviceAdapter> = slot();
        // The listener is owned by cleanup from its acquisition
        // inside `attempt_headless_mcp_hosting` (`HostedListenerSinkGuard`),
        // not from when the fixture stores the bridge.
        let (listener_guard, listener_sink) = super::HostedListenerSinkGuard::install(
            &paths[0],
            fault == StartFault::PanicInsideHostingAfterListener,
        );
        {
            let runtime_slot = runtime_slot.clone();
            cleanup.arm("runtime", move |report| {
                if let Some(runtime) = take(&runtime_slot) {
                    match Arc::try_unwrap(runtime) {
                        Ok(runtime) => {
                            runtime.shutdown_timeout(Duration::from_secs(2));
                            runtime_shut_down.store(true, Ordering::SeqCst);
                        }
                        Err(_) => report
                            .problems
                            .push("runtime: still shared; not shut down".into()),
                    }
                }
            });
        }
        {
            let (paths, runtime_slot, bridge_slot, device_slot, listener_sink) = (
                paths.clone(),
                runtime_slot.clone(),
                bridge_slot.clone(),
                device_slot.clone(),
                listener_sink.clone(),
            );
            cleanup.arm("listener", move |report| {
                let _owned_until_stopped = listener_guard;
                if let Some(found) = published_listener(&paths) {
                    *cleanup_saw_listener.lock().unwrap() = Some(found.address);
                }
                stop_hosting_listener(
                    &paths,
                    &runtime_slot,
                    &bridge_slot,
                    &device_slot,
                    &listener_sink,
                    &listener_stop_acknowledged,
                    report,
                );
                match cleanup_fault {
                    CleanupFault::None => {}
                    CleanupFault::ListenerUnacknowledged => report
                        .unacknowledged("listener: termination not acknowledged (injected)".into()),
                    CleanupFault::ListenerPanics => panic!("injected cleanup panic"),
                }
            });
        }

        let runtime = Arc::new(tokio::runtime::Runtime::new().expect("runtime"));
        *runtime_slot.lock().unwrap() = Some(runtime.clone());
        let hosted = attempt_headless_mcp_hosting(&paths, &settings)
            .ok_or("the bridge did not host, as `run` does when nobody publishes")?;
        if fault == StartFault::PanicBeforeBridgeSlot {
            panic!("injected panic before the bridge slot");
        }
        let device = hosted.device.clone();
        *device_slot.lock().unwrap() = Some(device.clone());
        let bridge = Arc::new(BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: Some(Arc::from(format!("stdio-{name}"))),
            },
            Some(paths.clone()),
            Some(settings.clone()),
            Some(hosted),
        ));
        *bridge_slot.lock().unwrap() = Some(bridge.clone());
        let listener = published_listener(&paths);
        seen.lock().unwrap().listener = listener.as_ref().map(|listener| listener.address);

        match fault {
            StartFault::None
            | StartFault::PanicInsideHostingAfterListener
            | StartFault::PanicBeforeBridgeSlot => {}
            StartFault::ReturnError => return Err("injected startup failure".into()),
            StartFault::Panic => panic!("injected startup panic"),
        }
        Ok(HostingFixture {
            directory,
            settings,
            bridge,
            runtime,
            listener,
        })
    }

    /// Whether anything still accepts at `address`, asked for up to `bound`.
    /// What a connect to a captured listener address says. Only a refused
    /// connect proves nothing listens; a timeout or any other error is
    /// unknown, never "stopped".
    #[derive(Debug, PartialEq, Eq)]
    enum ListenerState {
        Accepting,
        Refused,
        Unknown(String),
    }

    /// Asked for up to `bound`, until the answer is anything but accepting.
    fn listener_state(address: std::net::SocketAddr, bound: Duration) -> ListenerState {
        let deadline = std::time::Instant::now() + bound;
        loop {
            let state =
                match std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
                    Ok(_) => ListenerState::Accepting,
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        ListenerState::Refused
                    }
                    Err(error) => ListenerState::Unknown(error.to_string()),
                };
            if state != ListenerState::Accepting || std::time::Instant::now() >= deadline {
                return state;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The listener teardown: stops the bridge through the handle the test
    /// holds, then proves the stop. The listener's identity is read here,
    /// before stopping, whether or not startup got as far as reading it. A
    /// listener published with no bridge handle to stop it is reported and
    /// blocks removal; nothing is ever stopped by anything but its handle.
    fn stop_hosting_listener(
        paths: &[PathBuf],
        runtime_slot: &Slot<Arc<tokio::runtime::Runtime>>,
        bridge_slot: &Slot<Arc<BridgeBackend>>,
        device_slot: &Slot<HeadlessDeviceAdapter>,
        listener_sink: &super::HostedListenerSink,
        listener_stop_acknowledged: &AtomicBool,
        report: &mut crate::tests::CleanupReport,
    ) {
        let listener = published_listener(paths);
        let runtime = runtime_slot.lock().unwrap().clone();
        let bridge = take(bridge_slot);
        // Every listener handed over at its acquisition. Stopping
        // one the bridge has already stopped is a no-op (`HostedListener`
        // stops once).
        let acquired = std::mem::take(
            &mut *listener_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        match (&bridge, &runtime) {
            (Some(bridge), Some(runtime)) => {
                runtime.block_on(bridge.stop_hosting());
                if runtime.block_on(bridge.is_hosting()) {
                    report.unacknowledged("the bridge still hosts".into());
                }
            }
            (Some(_), None) => {
                report.unacknowledged("bridge: no runtime to stop it with".into());
            }
            (None, _) => {}
        }
        if !acquired.is_empty() {
            let mut acknowledged = true;
            for running in &acquired {
                let stopped = running.stop();
                acknowledged &= stopped;
            }
            listener_stop_acknowledged.store(acknowledged, Ordering::Release);
            if !acknowledged {
                report.unacknowledged("listener: stop was not acknowledged by its thread".into());
            }
        }
        if bridge.is_none() && acquired.is_empty() {
            // Nothing was acquired - or something was, with no handle.
            if let Some(listener) = &listener {
                report.unacknowledged(format!(
                    "listener {} is published but no owned handle: not stopped",
                    listener.instance_id
                ));
            }
            return;
        }
        match &listener {
            None => report
                .unacknowledged("listener: no identity was published; nothing was verified".into()),
            Some(listener) => {
                match listener_state(listener.address, Duration::from_secs(5)) {
                    ListenerState::Refused => {}
                    ListenerState::Accepting => report.unacknowledged(format!(
                        "listener {} still accepts after it was stopped",
                        listener.address
                    )),
                    ListenerState::Unknown(error)
                        if !listener_stop_acknowledged.load(Ordering::Acquire) =>
                    {
                        report.unacknowledged(format!(
                            "listener {}: stop not acknowledged, connect failed: {error}",
                            listener.address
                        ));
                    }
                    ListenerState::Unknown(_) => {}
                }
                // Unpublished means every state file is proven absent; one
                // that exists, or whose existence cannot be read, is not.
                for path in paths {
                    match path.try_exists() {
                        Ok(false) => {}
                        Ok(true) => report.unacknowledged(format!(
                            "listener {} is still published at {}",
                            listener.instance_id,
                            path.display()
                        )),
                        Err(error) => report.unacknowledged(format!(
                            "listener {}: cannot tell whether {} is published: {error}",
                            listener.instance_id,
                            path.display()
                        )),
                    }
                }
            }
        }
        if let Some(device) = take(device_slot) {
            if device.gateway_attached() {
                report.unacknowledged("a broker connection is still held".into());
            }
            if let Some(child) = device.take_hosted_broker_child() {
                report.unacknowledged(format!(
                    "the fixture spawned a broker (pid {}); it is never expected to",
                    child.id()
                ));
                let mut owned = crate::tests::OwnedChild::new(child);
                owned.stop(
                    "unexpected broker",
                    Duration::ZERO,
                    Duration::from_secs(5),
                    report,
                    |child| child.kill(),
                );
            }
        }
    }

    /// A bridge that hosts with no broker connection answers
    /// every backend resource read offline without reaching
    /// `GatewayProcess::start` - no attach, election, shared spawn or
    /// private `--stdio` child is even attempted. Steps 2h-2j: its cleanup is
    /// armed as each resource is acquired, and verified whatever happens.
    #[test]
    fn a_hosting_bridge_resource_read_never_reaches_the_start_seam() {
        let seen = std::sync::Mutex::new(StartupSeen::default());
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let fixture = start_hosting_fixture(
                "hosting-resource-read-start-seam",
                cleanup,
                StartFault::None,
                &seen,
            )
            .expect("the hosting fixture starts");
            // Installed after hosting: only what the reads do is counted.
            let seam =
                crate::GatewayStartSeam::install(&fixture.settings.device_broker_descriptor_path());
            let reads = backend_resource_reads();
            let answers = fixture.runtime.block_on(mcp_exchange(
                fixture.bridge.clone() as Arc<dyn QcBackend>,
                &reads,
            ));
            assert_offline(&reads, &answers);
            assert_eq!(seam.starts(), 0, "no start was attempted");
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// A failure injected into a real hosting fixture's
    /// assertions still leaves no listener, no broker connection or spawned
    /// broker, and no directory; the injected failure comes back as the
    /// primary payload with a clean report. The harness catches the failure
    /// it injects, so this test passes.
    #[test]
    fn an_injected_failure_in_a_hosting_fixture_leaves_nothing_behind() {
        let seen = std::sync::Mutex::new(StartupSeen::default());
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let fixture =
                start_hosting_fixture("hosting-injected-failure", cleanup, StartFault::None, &seen)
                    .expect("the hosting fixture starts");
            assert!(
                fixture.listener.is_some(),
                "the fixture published its listener"
            );
            assert!(fixture.directory.exists());
            panic!("injected failure");
        });
        assert_released(outcome, report, &seen, "injected failure");
    }

    /// A startup that returns an error after acquiring the
    /// directory, runtime, listener and bridge releases all of them.
    #[test]
    fn a_hosting_startup_failure_releases_everything_it_acquired() {
        let seen = std::sync::Mutex::new(StartupSeen::default());
        let (outcome, report) =
            crate::tests::run_with_cleanup(|cleanup| {
                match start_hosting_fixture(
                    "hosting-startup-failure",
                    cleanup,
                    StartFault::ReturnError,
                    &seen,
                ) {
                    Ok(_) => panic!("the injected startup failure did not occur"),
                    Err(failure) => std::panic::panic_any(failure),
                }
            });
        assert_released(outcome, report, &seen, "injected startup failure");
    }

    /// The same with a startup panic.
    #[test]
    fn a_hosting_startup_panic_releases_everything_it_acquired() {
        let seen = std::sync::Mutex::new(StartupSeen::default());
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            start_hosting_fixture("hosting-startup-panic", cleanup, StartFault::Panic, &seen)
                .map(|_| ())
        });
        assert_released(outcome.map(|_| ()), report, &seen, "injected startup panic");
    }

    /// The primary failure is `expected`, the cleanup report is clean, and
    /// nothing the startup acquired survives: no listener accepts where it
    /// was published, and its directory is gone.
    fn assert_released(
        outcome: std::thread::Result<()>,
        report: crate::tests::CleanupReport,
        seen: &std::sync::Mutex<StartupSeen>,
        expected: &str,
    ) {
        let primary = outcome.expect_err("the injected fault is the primary failure");
        let text = primary
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| primary.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert_eq!(text, expected);
        assert!(report.is_clean(), "{report}");
        let seen = seen.lock().unwrap();
        let _ = seen.listener.expect("the startup published a listener");
        assert!(
            seen.listener_stop_acknowledged.load(Ordering::Acquire),
            "listener thread acknowledged shutdown"
        );
        assert!(
            !seen.directory.as_ref().expect("a directory").exists(),
            "no directory"
        );
    }

    /// Negative twin: the same injected failure with a
    /// cleanup that cannot finish - a file in the fixture directory held open
    /// without sharing - fails with both the injected failure and the
    /// cleanup evidence; the listener is still proven stopped (the only
    /// problem is the removal).
    #[cfg(windows)]
    #[test]
    fn a_hosting_fixture_whose_cleanup_fails_reports_both_failures() {
        use std::os::windows::fs::OpenOptionsExt as _;
        let seen = std::sync::Mutex::new(StartupSeen::default());
        let held = std::sync::Mutex::new(None);
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let fixture =
                start_hosting_fixture("hosting-cleanup-fails", cleanup, StartFault::None, &seen)
                    .expect("the hosting fixture starts");
            *held.lock().unwrap() = Some(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .share_mode(0)
                    .open(fixture.directory.join("held"))
                    .expect("hold a file in the fixture directory"),
            );
            panic!("injected failure");
        });
        assert_eq!(
            report.problems.len(),
            1,
            "only the removal failed: {report}"
        );
        assert!(report.problems[0].contains("could not remove"), "{report}");
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tests::finish_or_rethrow(outcome, report)
        }))
        .expect_err("the test fails");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(message.starts_with("injected failure"), "{message}");
        assert!(message.contains("could not remove"), "{message}");

        drop(held.lock().unwrap().take());
        let directory = seen.lock().unwrap().directory.clone().expect("a directory");
        let mut retry = crate::tests::CleanupReport::default();
        retry.remove_dir_unless_blocked(&directory);
        assert!(retry.is_clean(), "{retry}");
    }

    /// Runs a real hosting fixture whose assertions fail ("injected failure")
    /// and whose listener teardown is given `fault` after it really stopped
    /// the listener. Returns the startup record, the failure message
    /// `finish_or_rethrow` produced, and whether the directory and its bytes
    /// were still there before the test's own removal.
    fn run_hosting_fixture_with_cleanup_fault(
        name: &str,
        fault: CleanupFault,
    ) -> (StartupSeen, String, bool) {
        let seen = std::sync::Mutex::new(StartupSeen {
            cleanup_fault: fault,
            ..StartupSeen::default()
        });
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let fixture = start_hosting_fixture(name, cleanup, StartFault::None, &seen)
                .expect("the hosting fixture starts");
            std::fs::write(fixture.directory.join("bytes-to-keep"), b"0123456789").unwrap();
            panic!("injected failure");
        });
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tests::finish_or_rethrow(outcome, report)
        }))
        .expect_err("the test fails with both pieces of evidence");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        let seen = seen.into_inner().unwrap();
        let directory = seen.directory.clone().expect("a directory");
        let kept = directory.join("bytes-to-keep").exists();
        (seen, message, kept)
    }

    /// The fixture's own resources are gone (the listener no longer accepts,
    /// the runtime was shut down by the later step), a bystander was never
    /// touched; then the test removes the directory cleanup preserved.
    fn assert_reaped_then_remove(seen: &StartupSeen, bystander: &mut std::process::Child) {
        let untouched = matches!(bystander.try_wait(), Ok(None));
        let _ = bystander.kill();
        let _ = bystander.wait();
        let directory = seen.directory.clone().expect("a directory");
        let mut removal = crate::tests::CleanupReport::default();
        removal.remove_dir_unless_blocked(&directory);

        let _ = seen.listener.expect("the startup published a listener");
        assert!(
            seen.listener_stop_acknowledged.load(Ordering::Acquire),
            "the listener thread acknowledged shutdown"
        );
        assert!(
            seen.runtime_shut_down.load(Ordering::SeqCst),
            "the runtime step still ran after the listener step"
        );
        assert!(untouched, "no foreign process was touched");
        assert!(removal.is_clean(), "{removal}");
    }

    fn test_owned_sleeper() -> std::process::Child {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("ping");
            command.args(["-n", "60", "127.0.0.1"]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = std::process::Command::new("/usr/bin/sleep");
            command.arg("60");
            command
        };
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the bystander sleeper")
    }

    /// On the real hosting fixture: the listener stop is
    /// made not acknowledged by a bounded test-only injection. The directory
    /// and its bytes are preserved and reported, the primary failure is
    /// retained, no foreign process is touched, and the fixture's listener
    /// and runtime are still reaped.
    #[test]
    fn a_hosting_fixture_whose_stop_is_not_acknowledged_keeps_its_bytes() {
        let mut bystander = test_owned_sleeper();
        let (seen, message, kept) = run_hosting_fixture_with_cleanup_fault(
            "hosting-stop-unacknowledged",
            CleanupFault::ListenerUnacknowledged,
        );
        assert!(kept, "the directory and its bytes were preserved");
        assert!(message.starts_with("injected failure"), "{message}");
        assert!(
            message.contains("listener: termination not acknowledged (injected)"),
            "{message}"
        );
        assert!(
            message.contains("termination not acknowledged; directory left in place"),
            "{message}"
        );
        assert!(message.contains("(bytes: 10)"), "{message}");
        assert_reaped_then_remove(&seen, &mut bystander);
    }

    /// On the real hosting fixture: the listener teardown
    /// panics (after really stopping the listener). The panic is reported
    /// with the primary failure, the later runtime step still runs, removal
    /// is blocked so the bytes are kept, and nothing is left running.
    #[test]
    fn a_panicking_hosting_teardown_step_does_not_stop_the_later_steps() {
        let mut bystander = test_owned_sleeper();
        let (seen, message, kept) = run_hosting_fixture_with_cleanup_fault(
            "hosting-teardown-panics",
            CleanupFault::ListenerPanics,
        );
        assert!(kept, "a panicked step never authorises removal");
        assert!(message.starts_with("injected failure"), "{message}");
        assert!(
            message.contains("cleanup step `listener` panicked: injected cleanup panic"),
            "{message}"
        );
        assert!(
            message.contains("termination not acknowledged; directory left in place"),
            "{message}"
        );
        assert_reaped_then_remove(&seen, &mut bystander);
    }

    /// A startup that panics at `fault`, before the fixture recorded or
    /// stored anything after hosting, still has its listener stopped through
    /// the handle cleanup received at the acquisition: the panic comes back,
    /// the report is clean (the stop was acknowledged), nothing accepts where
    /// the listener was published, the runtime was shut down and the
    /// directory is gone.
    fn assert_listener_reaped_after(fault: StartFault, name: &str, expected: &str) {
        let seen = std::sync::Mutex::new(StartupSeen::default());
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            start_hosting_fixture(name, cleanup, fault, &seen).map(|_| ())
        });
        let primary = outcome.expect_err("the injected panic is the primary failure");
        let text = primary
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| primary.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let seen = seen.into_inner().unwrap();
        let listener = *seen.cleanup_saw_listener.lock().unwrap();
        assert_eq!(text, expected);
        assert_eq!(
            seen.listener, None,
            "the startup never recorded its listener"
        );
        assert!(report.is_clean(), "{report}");
        let _ = listener.expect("cleanup found the listener published");
        assert!(
            seen.listener_stop_acknowledged.load(Ordering::Acquire),
            "listener thread acknowledged shutdown"
        );
        assert!(seen.runtime_shut_down.load(Ordering::SeqCst));
        assert!(
            !seen.directory.expect("a directory").exists(),
            "the directory was removed"
        );
    }

    /// A panic inside `attempt_headless_mcp_hosting`
    /// right after the listener was acquired - before the settings watcher,
    /// before the host is returned.
    #[test]
    fn a_panic_inside_hosting_right_after_the_listener_still_stops_it() {
        assert_listener_reaped_after(
            StartFault::PanicInsideHostingAfterListener,
            "hosting-panic-inside-acquisition",
            "injected panic inside attempt_headless_mcp_hosting after the listener",
        );
    }

    /// A panic the instant hosting returns, before the
    /// fixture stores the host or the bridge.
    #[test]
    fn a_panic_before_the_bridge_slot_still_stops_the_listener() {
        assert_listener_reaped_after(
            StartFault::PanicBeforeBridgeSlot,
            "hosting-panic-before-bridge-slot",
            "injected panic before the bridge slot",
        );
    }
}
