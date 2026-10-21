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
// Software discovery/preference/history envelope, not a firmware limit.
const MAX_LOCAL_DOCUMENT_BYTES: usize = 128 * 1024;
pub(crate) fn read_local_document(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    qc_owner_only_file::read_with_byte_budget(&mut file, MAX_LOCAL_DOCUMENT_BYTES)
}
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
    #[cfg(target_os = "macos")]
    {
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&directory)?;
        Ok(vec![directory.join(STATE_FILE)])
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
        let endpoint = read_local_document(path)
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
/// `Result` did - `send_bridge_status` distinguishes an unreadable descriptor, an
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

#[derive(Clone)]
struct BridgeAnnouncement {
    client_id: String,
    client_name: String,
    client_version: Option<String>,
}

/// Token-free lifecycle metadata conveyed only over authenticated local RPC.
/// Read it in the publisher's filesystem view, not the bridge launcher's view.
#[derive(Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeLifecycleHint {
    pub(crate) user_quit: bool,
    owner: Option<BridgeOwner>,
}

#[derive(Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeOwner {
    process_id: u32,
    process_created_at: u64,
    instance_id: String,
}

impl BridgeOwner {
    fn valid(&self) -> bool {
        self.process_id != 0
            && self.process_created_at != 0
            && !self.instance_id.is_empty()
            && self.instance_id.len() <= 256
    }
}

pub(crate) fn broker_lifecycle_hint(descriptor: &std::path::Path) -> BridgeLifecycleHint {
    let owner = crate::device_broker_descriptor::read_descriptor(descriptor, now_unix_ms())
        .filter(|owner| {
            qc_native_platform::owner_identity::native(owner.process_id, owner.process_created_at)
                == qc_native_platform::owner_identity::Assessment::Same
        })
        .and_then(|owner| {
            Some(BridgeOwner {
                process_id: owner.process_id,
                process_created_at: owner.process_created_at?,
                instance_id: owner.instance_id,
            })
        });
    BridgeLifecycleHint {
        user_quit: crate::user_quit_refusal(descriptor).is_some(),
        owner,
    }
}

async fn send_bridge_status(
    client: &reqwest::Client,
    paths: Option<&[PathBuf]>,
    session_id: &str,
    announcement: Option<&BridgeAnnouncement>,
) -> Result<Option<BridgeLifecycleHint>, String> {
    let endpoint = heartbeat_descriptor_from_paths(paths.ok_or(OFFLINE_ADVICE)?, now_unix_ms())?;
    endpoint.prove_identity(client).await?;
    let body = match announcement {
        Some(row) => serde_json::json!({"rpc":"local.bridgeHeartbeat", "params":{
            "client":row.client_id, "sessionId":session_id,
            "clientName":row.client_name, "clientVersion":row.client_version,
        }}),
        // Before MCP initialize, read lifecycle without advertising a client.
        None => serde_json::json!({"rpc":"local.bridgeLifecycle", "params":{}}),
    };
    let response = client
        .post(endpoint.rpc_url)
        .bearer_auth(endpoint.bearer_token)
        .timeout(Duration::from_secs(2))
        .json(&body)
        .send()
        .await
        // Every arm names one distinguishable cause, because these strings are
        // now the only record of why a working host read as never-attached.
        // They are also the rate limiter's key: a reason that changes is new
        // information and is written immediately.
        .map_err(|error| format!("the loopback endpoint did not answer ({error})"))?;
    if response.status().is_success() {
        // Older publishers have no hint. Do not turn absent/malformed optional
        // metadata into either a heartbeat failure or a false shutdown.
        let hint = qc_relay_client::bounded_response_json::<Value>(response)
            .await
            .ok()
            .and_then(|body| {
                serde_json::from_value(body["result"]["brokerLifecycle"].clone()).ok()
            });
        Ok(hint)
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

/// Only admitted, unanswered mutations lose their safe retry boundary.
/// Identity/connect refusal and a complete typed server error stay distinct.
fn unanswered_rpc(rpc: &str, error: BackendError) -> BackendError {
    if !crate::generated_gateway::is_mutation_method(rpc) && rpc != "broker.createDeviceBackupBound"
    {
        return error;
    }
    BackendError {
        code: "device_error",
        message: format!(
            "The request may have completed; verify the device before retrying. {}",
            error.message
        ),
        retryable: false,
        detail_code: Some("WRITE_OUTCOME_AMBIGUOUS".into()),
        details: Some(Box::new(serde_json::json!({
            "code": "WRITE_OUTCOME_AMBIGUOUS", "requestSent": true,
            "retryable": false, "userActionRequired": true, "verification": "unknown"
        }))),
    }
}

impl DesktopBackend {
    /// The relayed request exactly as sent: bearer token, the bridge's
    /// session id when it has one, and the `{rpc, params}` body.
    #[cfg(test)]
    fn rpc_request(
        &self,
        endpoint: &Endpoint,
        rpc: &str,
        params: Map<String, Value>,
    ) -> reqwest::RequestBuilder {
        self.rpc_request_targeted(endpoint, rpc, params, None)
    }

    fn rpc_request_targeted(
        &self,
        endpoint: &Endpoint,
        rpc: &str,
        params: Map<String, Value>,
        expected: Option<&qc_remote_mcp::BrokerTarget>,
    ) -> reqwest::RequestBuilder {
        let mut body = serde_json::json!({ "rpc":rpc, "params":params });
        if let Some(expected) = expected {
            body["expectedBrokerTarget"] = serde_json::json!(expected);
        }
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
        .json(&body)
    }

    async fn relay(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, RelayFailure> {
        self.relay_targeted(route, rpc, params, None).await
    }

    async fn relay_targeted(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
        expected: Option<qc_remote_mcp::BrokerTarget>,
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
        self.relay_at_targeted(&paths, rpc, params, expected).await
    }

    /// Relay to whichever process publishes at `paths`.
    async fn relay_at_targeted(
        &self,
        paths: &[PathBuf],
        rpc: &'static str,
        params: Map<String, Value>,
        expected: Option<qc_remote_mcp::BrokerTarget>,
    ) -> Result<Value, RelayFailure> {
        let endpoint = descriptor_from_paths(paths)
            .map_err(|error| RelayFailure::NoPublisher(BackendError::unavailable(error)))?;
        self.send_targeted(endpoint, rpc, params, expected).await
    }

    #[cfg(test)]
    async fn send(
        &self,
        endpoint: Endpoint,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, RelayFailure> {
        self.send_targeted(endpoint, rpc, params, None).await
    }

    async fn send_targeted(
        &self,
        endpoint: Endpoint,
        rpc: &'static str,
        params: Map<String, Value>,
        expected: Option<qc_remote_mcp::BrokerTarget>,
    ) -> Result<Value, RelayFailure> {
        // Nothing was sent if the proof fails, so nobody legitimate is serving
        // this descriptor and the request may be answered elsewhere.
        if let Err(reason) = endpoint.prove_identity(&self.client).await {
            eprintln!("QC Remote local MCP: {reason}");
            return Err(RelayFailure::NoPublisher(BackendError::unavailable(
                OFFLINE_ADVICE.to_string(),
            )));
        }
        if expected.is_some() {
            // Older /rpc publishers ignore unknown JSON fields. Prove support
            // before sending a guarded action; never retry it without a fence.
            // Bind the credential to the already-validated RPC origin, never
            // to the descriptor's unvalidated diagnostic URL.
            let mut health_url = endpoint.rpc_url.clone();
            health_url.set_path("/health");
            let health = self
                .client
                .get(health_url)
                .bearer_auth(&endpoint.bearer_token)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .map_err(|_| {
                    RelayFailure::Refused(BackendError::unavailable(
                        "The local host could not confirm target-guard support",
                    ))
                })?;
            let health = qc_relay_client::bounded_response_json::<Value>(health)
                .await
                .map_err(|_| {
                    RelayFailure::Refused(BackendError::unavailable(
                        "The local host returned invalid target-guard capabilities",
                    ))
                })?;
            if health["instanceId"].as_str() != Some(endpoint.instance_id.as_str())
                || !health["capabilities"].as_array().is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| value == "localRpcBrokerTargetGuard")
                })
            {
                return Err(RelayFailure::Refused(BackendError {
                    code: "device_error",
                    message:
                        "Update QC Remote before using target-aware actions through this local host"
                            .into(),
                    retryable: false,
                    detail_code: Some("BROKER_TARGET_GUARD_UNAVAILABLE".into()),
                    details: Some(Box::new(
                        serde_json::json!({"requestSent":false,"retryable":false}),
                    )),
                }));
            }
        }
        let response = self
            .rpc_request_targeted(&endpoint, rpc, params, expected.as_ref())
            .send()
            .await
            .map_err(|error| match unreachable_app(error) {
                RelayFailure::NoPublisher(error) => RelayFailure::NoPublisher(error),
                RelayFailure::Refused(error) => RelayFailure::Refused(unanswered_rpc(rpc, error)),
            })?;
        let status = response.status();
        let envelope = qc_relay_client::bounded_response_json::<RpcEnvelope>(response).await.map_err(|_| {
            RelayFailure::Refused(unanswered_rpc(
                rpc,
                BackendError {
                    code: "local_bridge_error",
                    message: format!(
                        "QC Remote returned an invalid, oversized, or incomplete local response ({status})"
                    ),
                    retryable: status.is_server_error(),
                    detail_code: None,
                    details: None,
                },
            ))
        })?;
        if envelope.ok {
            return envelope.result.ok_or_else(|| {
                RelayFailure::Refused(unanswered_rpc(
                    rpc,
                    BackendError {
                        code: "local_bridge_error",
                        message: "QC Remote returned a successful response without a result."
                            .into(),
                        retryable: false,
                        detail_code: None,
                        details: None,
                    },
                ))
            });
        }
        let Some(error) = envelope.error else {
            return Err(RelayFailure::Refused(unanswered_rpc(
                rpc,
                BackendError {
                    code: "local_bridge_error",
                    message: format!(
                        "QC Remote returned no typed answer to the local request ({status})."
                    ),
                    retryable: status.is_server_error(),
                    detail_code: None,
                    details: None,
                },
            )));
        };
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
    async fn request_targeted(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
        expected: Option<qc_remote_mcp::BrokerTarget>,
    ) -> Result<Value, BackendError> {
        self.relay_targeted(route, rpc, params, expected)
            .await
            .map_err(RelayFailure::into_backend_error)
    }
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
    access_policy_directory: Option<PathBuf>,
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
    /// Only a temporary native cache/inventory contention, not target loss.
    Unknown,
    /// The session is open.
    Ready,
    ReadyTarget {
        usb_connected: bool,
        target: qc_relay_client::BrokerTarget,
    },
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
    async fn sample_publication_readiness(&self) -> Option<qc_relay_client::TargetReadiness> {
        if self.stopped.load(Ordering::Acquire) {
            return Some(Default::default());
        }
        if self.broker.is_some() {
            return Some(match self.broker_readiness_answer().await {
                Ok(BrokerDeviceReadiness::Unknown) => return None,
                Ok(BrokerDeviceReadiness::ReadyTarget {
                    usb_connected,
                    target,
                }) => qc_relay_client::TargetReadiness {
                    target_ready: true,
                    usb_connected,
                    broker_target: Some(target),
                },
                Ok(BrokerDeviceReadiness::Ready) => qc_relay_client::TargetReadiness {
                    target_ready: true,
                    usb_connected: true,
                    broker_target: None,
                },
                _ => Default::default(),
            });
        }
        let gateway = self.gateway.clone();
        let reads = self.lock_free_reads.clone();
        tokio::task::spawn_blocking(move || {
            crate::request_gateway(
                &gateway,
                &reads,
                "system.status",
                serde_json::json!({}),
                crate::WhenGatewayBusy::Refuse,
                |gateway, params| {
                    gateway.request_detailed_as(
                        "system.status",
                        params,
                        crate::ConnectionUse::ExistingOnly,
                    )
                },
            )
            .map(|status| crate::publication_readiness_from_status(&status))
            .unwrap_or(Some(Default::default()))
        })
        .await
        .unwrap_or(Some(Default::default()))
    }

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
        if self.broker.is_some() {
            return self.broker_readiness().await;
        }
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
        match self.broker_readiness_answer().await {
            Ok(BrokerDeviceReadiness::Unknown) => HostReadiness::NotReady(
                "the device broker's current target readiness is temporarily unknown".into(),
            ),
            Ok(BrokerDeviceReadiness::Ready | BrokerDeviceReadiness::ReadyTarget { .. }) => {
                HostReadiness::Ready
            }
            Ok(BrokerDeviceReadiness::NotReady { phase }) => HostReadiness::NotReady(format!(
                "the device broker reports the Quad Cortex session {phase}"
            )),
            Ok(BrokerDeviceReadiness::Unavailable { reason }) | Err(reason) => {
                HostReadiness::NotReady(reason)
            }
        }
    }

    async fn broker_readiness_answer(&self) -> Result<BrokerDeviceReadiness, String> {
        let Some(probe) = self.broker.clone() else {
            return Err("this host has no broker to ask".into());
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
            Ok(Ok(Some(Ok(answer)))) => Ok(answer),
            Ok(Ok(Some(Err(())) | None)) | Ok(Err(_)) => {
                Err("the readiness read of the device broker failed".into())
            }
            Err(_) => Err(format!(
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
            access_policy_directory: None,
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
            access_policy_directory: None,
            gateway: Arc::new(Mutex::new(gateway)),
        }
    }
}

#[async_trait]
impl DeviceAdapter for HeadlessDeviceAdapter {
    async fn target_readiness(&self) -> qc_relay_client::TargetReadiness {
        self.sample_publication_readiness()
            .await
            .unwrap_or_default()
    }

    async fn publication_readiness(&self) -> Option<qc_relay_client::TargetReadiness> {
        self.sample_publication_readiness().await
    }

    async fn invoke_targeted(
        &self,
        method: &str,
        params: Value,
        expected: Option<qc_relay_client::BrokerTarget>,
        resource_read: bool,
    ) -> Result<Value, DeviceError> {
        let expected = expected
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| DeviceError::new("INVALID_ARGUMENT", error.to_string(), false))?;
        self.invoke_with_target_as(
            method,
            params,
            if resource_read {
                crate::ConnectionUse::ExistingOnly
            } else {
                crate::ConnectionUse::MayConnect
            },
            expected,
        )
        .await
    }
    fn effective_access_mode(&self) -> Option<AccessMode> {
        self.access_policy_directory
            .as_deref()
            .map(crate::access_policy_store::effective_mode)
    }

    fn remote_session_enabled(&self) -> bool {
        self.access_policy_directory
            .as_deref()
            .is_none_or(crate::access_policy_store::effective_remote_enabled)
    }

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
    pub(crate) fn with_access_policy_directory(mut self, directory: PathBuf) -> Self {
        self.access_policy_directory = Some(directory);
        self
    }

    /// One call, in the connection mode the caller chose.
    async fn invoke_as(
        &self,
        method: &str,
        params: Value,
        connection: crate::ConnectionUse,
    ) -> Result<Value, DeviceError> {
        self.invoke_with_target_as(method, params, connection, None)
            .await
    }

    async fn invoke_with_target_as(
        &self,
        method: &str,
        params: Value,
        connection: crate::ConnectionUse,
        expected: Option<Value>,
    ) -> Result<Value, DeviceError> {
        // A remote or MCP backup is written to this PC; the caller gets its
        // location, never the document (see `remote_backup`).
        crate::remote_backup::invoke_saving_backups(
            method,
            params,
            crate::remote_backup::default_backup_directory(),
            |params| self.invoke_gateway_targeted(method, params, connection, expected),
        )
        .await
    }

    /// A resource read (`ExistingOnly`) may use the broker connection this
    /// host holds but never bring one up; decided under the gateway lock.
    async fn invoke_gateway_targeted(
        &self,
        method: &str,
        params: Value,
        connection: crate::ConnectionUse,
        expected: Option<Value>,
    ) -> Result<Value, DeviceError> {
        let gateway = self.gateway.clone();
        let lock_free_reads = self.lock_free_reads.clone();
        let stopped = self.stopped.clone();
        let method = method.to_owned();
        let method_for_action = method.clone();
        crate::await_relay_gateway(
            method,
            &crate::RELAY_GATEWAY_NATIVE_BUDGET,
            crate::RELAY_GATEWAY_NATIVE_TIMEOUT,
            move |admission| {
                // The same routing as a window's relay adapter, plus this host's
                // stop flag.
                crate::relay_gateway_request_targeted(
                    &gateway,
                    &lock_free_reads,
                    &method_for_action,
                    params,
                    connection,
                    crate::RelayGatewayContext {
                        stopped: Some(&stopped),
                        admission: Some(&admission),
                    },
                    expected,
                )
            },
        )
        .await
    }
}

/// How often a headless host re-reads the user's access level and the local
/// MCP switch.
///
/// A window applies a change the moment it is made, because the change is
/// made through that window's own [`crate::relay::RelayBridge`]. A headless
/// host shares no memory with the window where the user changes it, so its
/// status watch refreshes from the canonical policy file and local MCP switch.
/// Request admission also reads the canonical file directly; the polling
/// interval therefore affects displayed state, not enforcement latency.
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
    refresh_access_mode: Arc<dyn Fn(AccessMode) -> AccessMode + Send + Sync>,
    refresh_interval: Duration,
    refresh_budget: Arc<std::sync::atomic::AtomicUsize>,
    /// Whether a broker this host spawns opens the device session itself;
    /// see `Gateway::auto_connect_first_broker`.
    auto_connect: bool,
}

impl HeadlessHostSettings {
    /// The settings a window of this user's installed app would apply.
    ///
    /// `None` when the operating system reports no local data directory; the
    /// bridge then relays only, exactly as before it could host.
    pub(crate) fn for_this_user() -> Option<Self> {
        Some(Self {
            app_local_data_dir: app_local_data_dir()?,
            load_access_mode: Arc::new(|| AccessMode::ReadOnly),
            refresh_access_mode: Arc::new(|previous| {
                tauri::async_runtime::block_on(crate::relay::refresh_access_mode(previous))
            }),
            refresh_interval: HEADLESS_SETTINGS_REFRESH_INTERVAL,
            refresh_budget: production_refresh_budget(),
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
    #[cfg(target_os = "macos")]
    {
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&directory).ok()?;
        Some(directory)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
#[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
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
    // The existing acquisition sink must also own the exact adapter on a
    // fixture panic before HeadlessMcpHost/BridgeBackend can be returned.
    #[cfg(test)]
    fixture_device: HeadlessDeviceAdapter,
    /// Behind a lock only because the listener holds a channel receiver,
    /// which cannot be shared between threads bare.
    running: Mutex<local_access::RunningLocalAccess>,
    stop_acknowledged: Mutex<Option<bool>>,
}

impl HostedListener {
    fn stop(&self) -> bool {
        let mut result = self
            .stop_acknowledged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(acknowledged) = *result {
            return acknowledged;
        }
        let acknowledged = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        if acknowledged {
            *result = Some(true);
        }
        acknowledged
    }
}

impl Drop for HostedListener {
    fn drop(&mut self) {
        if !self.stop() {
            eprintln!("QC Remote local MCP: acquired listener retirement was not acknowledged");
        }
    }
}

struct RefreshWorker {
    thread: std::thread::JoinHandle<()>,
    // Original owner retains admission through native TLS exit and actual join.
    _permit: Arc<RefreshPermit>,
}
impl std::ops::Deref for RefreshWorker {
    type Target = std::thread::JoinHandle<()>;
    fn deref(&self) -> &Self::Target {
        &self.thread
    }
}
impl RefreshWorker {
    fn join(self) -> std::thread::Result<()> {
        let Self { thread, _permit } = self;
        let result = thread.join();
        drop(_permit);
        result
    }
}
static RETAINED_HEADLESS_REFRESH: Mutex<Vec<RefreshWorker>> = Mutex::new(Vec::new());
const HEADLESS_REFRESH_LIMIT: usize = 4;
fn production_refresh_budget() -> Arc<std::sync::atomic::AtomicUsize> {
    static BUDGET: std::sync::OnceLock<Arc<std::sync::atomic::AtomicUsize>> =
        std::sync::OnceLock::new();
    BUDGET
        .get_or_init(|| Arc::new(std::sync::atomic::AtomicUsize::new(0)))
        .clone()
}
struct RefreshPermit(Arc<std::sync::atomic::AtomicUsize>);
impl RefreshPermit {
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    fn acquire(budget: &Arc<std::sync::atomic::AtomicUsize>) -> Option<Arc<Self>> {
        reap_refresh_workers();
        budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < HEADLESS_REFRESH_LIMIT).then_some(count + 1)
            })
            .ok()?;
        Some(Arc::new(Self(budget.clone())))
    }
}
impl Drop for RefreshPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
fn reap_refresh_workers() {
    let complete = {
        let mut retained = RETAINED_HEADLESS_REFRESH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut complete = Vec::new();
        let mut index = 0;
        while index < retained.len() {
            if qc_native_platform::process::thread_signalled(&retained[index].thread) {
                complete.push(retained.swap_remove(index));
            } else {
                index += 1;
            }
        }
        complete
    };
    for original in complete {
        let _ = original.join();
    }
}

fn retire_refresh_worker(worker: RefreshWorker, budget: Duration) {
    let deadline = std::time::Instant::now() + budget;
    while !qc_native_platform::process::thread_signalled(&worker.thread)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    if qc_native_platform::process::thread_signalled(&worker.thread) {
        if worker.join().is_err() {
            eprintln!("QC Remote local MCP: settings refresh worker panicked");
        }
    } else {
        eprintln!("QC Remote local MCP: retaining the exact settings refresh worker after retirement deadline");
        reap_refresh_workers();
        RETAINED_HEADLESS_REFRESH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(worker);
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
    refresh_worker: Mutex<Option<RefreshWorker>>,
}

impl Drop for HeadlessMcpHost {
    fn drop(&mut self) {
        self.stop();
    }
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
        // Withdraw device admission before waiting on listener/refresher
        // retirement. A previously cloned backend may still be queued for
        // the gateway; its under-lock stop check must refuse late dispatch.
        // Already admitted native work retains its owner and may complete.
        self.device.stop();
        self.stop_refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.running.stop();
        let worker = self
            .refresh_worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            retire_refresh_worker(worker, Duration::from_secs(3));
        }
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
        let previous = (settings.load_access_mode)();
        let mode = (settings.refresh_access_mode)(previous);
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
    // Admission precedes publication; ownership follows the real worker.
    let refresh_permit = RefreshPermit::acquire(&settings.refresh_budget)?;
    let device = HeadlessDeviceAdapter::new(
        settings.device_broker_descriptor_path(),
        settings.auto_connect,
    )
    .with_access_policy_directory(settings.app_local_data_dir.clone());
    let adapter: Arc<dyn DeviceAdapter> = Arc::new(device.clone());
    let (access_tx, access_rx) = watch::channel((settings.load_access_mode)());
    match local_access::start_listener(paths.to_vec(), adapter.clone(), access_rx.clone()) {
        Ok(running) => {
            let running = Arc::new(HostedListener {
                #[cfg(test)]
                fixture_device: device.clone(),
                running: Mutex::new(running),
                stop_acknowledged: Mutex::new(None),
            });
            #[cfg(test)]
            register_hosted_listener(paths, &running);
            let disabled = Arc::new(AtomicBool::new(false));
            let (stop_tx, stop_rx) = std::sync::mpsc::channel();
            let original_refresh_permit = Arc::clone(&refresh_permit);
            let refresh = {
                let (settings, running, disabled) =
                    (settings.clone(), running.clone(), disabled.clone());
                std::thread::Builder::new()
                    .name("qc-headless-settings".into())
                    .spawn(move || {
                        let _refresh_permit = refresh_permit;
                        refresh_headless_settings(settings, access_tx, running, disabled, stop_rx)
                    })
            };
            let refresh = match refresh {
                Ok(refresh) => refresh,
                Err(error) => {
                    // Without the refresh, a later change in a window would never
                    // reach this host. Serving a stale access level indefinitely
                    // is worse than not hosting.
                    eprintln!(
                    "QC Remote local MCP: this bridge could not watch the access settings, so it will not host: {error}"
                );
                    running.stop();
                    return None;
                }
            };
            Some(HeadlessMcpHost {
                running,
                device,
                adapter,
                access: access_rx,
                disabled,
                stop_refresh: Mutex::new(Some(stop_tx)),
                refresh_worker: Mutex::new(Some(RefreshWorker {
                    thread: refresh,
                    _permit: original_refresh_permit,
                })),
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
    #[cfg(test)]
    fixture_dependencies: Mutex<std::sync::Weak<tests::McpDependencies>>,
    #[cfg(test)]
    host_start_budget: &'static qc_native_platform::thread_owner::Budget,
    #[cfg(test)]
    host_start_timeout: Duration,
    #[cfg(test)]
    host_start_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    host_created_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    host_start_attempts: Option<Arc<std::sync::atomic::AtomicUsize>>,
    relay: DesktopBackend,
    /// The local MCP descriptor paths; `None` when Windows reports no user
    /// data directories, which leaves the relay's own error to answer.
    paths: Option<Vec<PathBuf>>,
    /// `None` when the app's local data directory cannot be resolved; this
    /// bridge then only ever relays.
    settings: Option<HeadlessHostSettings>,
    host: Arc<tokio::sync::Mutex<Option<Arc<HeadlessMcpHost>>>>,
    host_stopping: Arc<AtomicBool>,
}

// Software admission/observation limits, not device or OS-cancellation claims.
static HOST_START_THREADS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(4);
const HOST_START_TIMEOUT: Duration = Duration::from_secs(10);

impl BridgeBackend {
    fn new(
        relay: DesktopBackend,
        paths: Option<Vec<PathBuf>>,
        settings: Option<HeadlessHostSettings>,
        host: Option<HeadlessMcpHost>,
    ) -> Self {
        Self {
            relay,
            #[cfg(test)]
            fixture_dependencies: Mutex::new(std::sync::Weak::new()),
            #[cfg(test)]
            host_start_budget: &HOST_START_THREADS,
            #[cfg(test)]
            host_start_timeout: HOST_START_TIMEOUT,
            #[cfg(test)]
            host_start_hook: None,
            #[cfg(test)]
            host_created_hook: None,
            #[cfg(test)]
            host_start_attempts: None,
            paths,
            settings,
            host: Arc::new(tokio::sync::Mutex::new(host.map(Arc::new))),
            host_stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The backend of the host this bridge runs, unless it has none or the
    /// user has since turned local MCP off.
    async fn hosted_backend(&self) -> Option<Arc<dyn QcBackend>> {
        if self.host_stopping.load(Ordering::Acquire) {
            return None;
        }
        // A retained native creator owns this slot. A cache lookup must not
        // wait indefinitely before reaching relay/host_now's bounded path.
        let mut host = self.host.try_lock().ok()?;
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
        if self.host_stopping.load(Ordering::Acquire) {
            return None;
        }
        let (Some(paths), Some(settings)) = (self.paths.clone(), self.settings.clone()) else {
            return None;
        };
        #[cfg(test)]
        let (budget, within) = (self.host_start_budget, self.host_start_timeout);
        #[cfg(not(test))]
        let (budget, within) = (&HOST_START_THREADS, HOST_START_TIMEOUT);
        let deadline = std::time::Instant::now() + within;
        // Before waiting for the host lock or a blocking thread; the hosting
        // admission (`attempt_headless_mcp_hosting`) asks again after both.
        if self.user_quit_refusal().is_some() {
            return None;
        }
        let host = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.host.clone().lock_owned(),
        )
        .await
        .ok()?;
        if std::time::Instant::now() >= deadline || self.host_stopping.load(Ordering::Acquire) {
            return None;
        }
        if let Some(current) = host.as_ref() {
            return Some(current.backend());
        }
        #[cfg(test)]
        let resource = self
            .fixture_dependencies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade();
        #[cfg(not(test))]
        let resource = ();
        #[cfg(test)]
        let hook = self.host_start_hook.clone();
        #[cfg(test)]
        let created_hook = self.host_created_hook.clone();
        #[cfg(test)]
        let attempts = self.host_start_attempts.clone();
        let creation_settings = settings.clone();
        let stopping = self.host_stopping.clone();
        let (mut host, started) = match crate::owned_blocking::run_with_resource(
            "qc-headless-host-start",
            budget,
            deadline,
            resource,
            move |context| -> Result<_, std::convert::Infallible> {
                #[cfg(test)]
                if let Some(hook) = hook {
                    hook();
                }
                // This exact original owns the actual host slot through all
                // synchronous creation. Caller cancellation cannot release
                // serialization while started native work is still running.
                if stopping.load(Ordering::Acquire) || !context.begin_dispatch() {
                    return Ok(None);
                }
                #[cfg(test)]
                if let Some(attempts) = attempts {
                    attempts.fetch_add(1, Ordering::AcqRel);
                }
                let started = attempt_headless_mcp_hosting(&paths, &creation_settings);
                #[cfg(test)]
                if started.is_some() {
                    if let Some(hook) = created_hook {
                        hook();
                    }
                }
                // Started synchronous OS work cannot be forcibly canceled.
                // Its original remains owned; a late result is stopped rather
                // than adopted by an obsolete caller.
                if stopping.load(Ordering::Acquire) || !context.current() {
                    if let Some(started) = started {
                        started.stop();
                    }
                    return Ok(None);
                }
                Ok(started.map(|started| (host, started)))
            },
        )
        .await
        {
            Ok(Ok(Some(started))) => started,
            _ => return None,
        };
        if std::time::Instant::now() >= deadline
            || self.host_stopping.load(Ordering::Acquire)
            || started.is_disabled()
            || !settings.local_mcp_enabled()
            || self.user_quit_refusal().is_some()
        {
            started.stop();
            return None;
        }
        let started = Arc::new(started);
        *host = Some(started.clone());
        Some(started.backend())
    }

    async fn stop_hosting(&self) -> Result<(), String> {
        // Irrevocable admission fence, even when synchronous creation cannot
        // acknowledge shutdown yet. Its exact owned original remains intact.
        self.host_stopping.store(true, Ordering::Release);
        let mut slot = self.host.try_lock().map_err(|_| {
            "Local MCP hosting shutdown is not acknowledged; its exact native original remains owned and retiring".to_string()
        })?;
        if let Some(host) = slot.take() {
            host.stop();
        }
        // Host-slot/fence completion only. Internal listener/refresher
        // originals keep their existing bounded observations/retained owners.
        Ok(())
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
        self.request_targeted(route, rpc, params, None).await
    }

    async fn request_targeted(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
        expected: Option<qc_remote_mcp::BrokerTarget>,
    ) -> Result<Value, BackendError> {
        if self.host_stopping.load(Ordering::Acquire) {
            return Err(BackendError::unavailable(
                "The local MCP bridge is stopping.",
            ));
        }
        // A host this bridge already runs answers a resource read from the
        // broker connection it holds, or offline; its gateway decides that
        // under its own lock (`crate::ConnectionUse`).
        if let Some(backend) = self.hosted_backend().await {
            return backend.request_targeted(route, rpc, params, expected).await;
        }
        match self
            .relay_once_targeted(route, rpc, params.clone(), expected.clone())
            .await
        {
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
                    Some(backend) => backend.request_targeted(route, rpc, params, expected).await,
                    // Hosting was refused. If that is because another process
                    // began publishing meanwhile, relay to it; the first attempt
                    // reached no one, so sending again cannot repeat anything.
                    None => match self.relay_once_targeted(route, rpc, params, expected).await {
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

    async fn relay_once_targeted(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
        expected: Option<qc_remote_mcp::BrokerTarget>,
    ) -> Result<Value, RelayFailure> {
        match &self.paths {
            Some(paths) if is_local_route(route) => {
                self.relay
                    .relay_at_targeted(paths, rpc, params, expected)
                    .await
            }
            _ => {
                self.relay
                    .relay_targeted(route, rpc, params, expected)
                    .await
            }
        }
    }
}

/// Serves MCP over stdio for a bridge process: a hosting bridge answers from
/// its own [`HeadlessMcpHost`], a relaying one through [`DesktopBackend`], and
/// either can become the other's replacement per [`BridgeBackend`].
fn finish_bridge_shutdown(
    result: Result<(), String>,
    stopped: Result<(), String>,
) -> Result<(), String> {
    match (result, stopped) {
        (Ok(()), stopped) => stopped,
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; shutdown: {cleanup}")),
    }
}

/// A bridge follows one broker generation, not the lifetime of its AI host's
/// stdin. Missing/stale metadata and inaccessible process identity are NOT
/// proof of death. Disconnect leaves this generation alive and never stops MCP.
struct BridgeLifecycle {
    descriptor: PathBuf,
    observed: Option<crate::device_broker_descriptor::DeviceBrokerDescriptor>,
}

impl BridgeLifecycle {
    fn stopped(&mut self) -> bool {
        if crate::user_quit_refusal(&self.descriptor).is_some() {
            return true;
        }
        self.observe(
            crate::device_broker_descriptor::read_descriptor(
                &self.descriptor,
                crate::device_broker_descriptor::unix_time_ms(),
            ),
            crate::broker_attach::published_broker_process_is_alive,
        )
    }

    fn observe(
        &mut self,
        current: Option<crate::device_broker_descriptor::DeviceBrokerDescriptor>,
        alive: impl Fn(&crate::device_broker_descriptor::DeviceBrokerDescriptor) -> bool,
    ) -> bool {
        if let Some(observed) = &self.observed {
            return !alive(observed);
        }
        self.observed = current.filter(alive);
        false
    }
}

async fn wait_for_bridge_stop(
    backend: &BridgeBackend,
    client: &reqwest::Client,
    session_id: &str,
    mut announcement: watch::Receiver<Option<BridgeAnnouncement>>,
) {
    // An explicit state override is isolated to that directory, including in
    // release builds which intentionally cannot host. Never consult another
    // user's/store's Quit marker as a fallback.
    let descriptor = backend
        .settings
        .as_ref()
        .map(HeadlessHostSettings::device_broker_descriptor_path)
        .or_else(|| {
            backend
                .paths
                .as_ref()?
                .first()?
                .parent()
                .map(crate::device_broker_descriptor::device_broker_descriptor_path)
        });
    let mut lifecycle = descriptor.map(|descriptor| BridgeLifecycle {
        descriptor,
        observed: None,
    });
    let mut publisher_authoritative = false;
    let mut publisher_owner = None;
    let mut next_heartbeat = std::time::Instant::now();
    let started = std::time::Instant::now();
    let mut report = HeartbeatReport::default();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        if std::time::Instant::now() >= next_heartbeat {
            let row = announcement.borrow_and_update().clone();
            let outcome =
                send_bridge_status(client, backend.paths.as_deref(), session_id, row.as_ref())
                    .await;
            if let Ok(Some(hint)) = &outcome {
                // This authenticated publisher sees the broker's actual store,
                // even when our launcher virtualizes LocalAppData. Do not let
                // an unrelated old marker in our own view override it.
                publisher_authoritative = true;
                if hint.user_quit {
                    return;
                }
                if publisher_owner.is_none() {
                    publisher_owner = hint.owner.as_ref().filter(|owner| owner.valid()).cloned();
                }
            }
            if let Some(row) = &row {
                if let Some(line) = report.observe(
                    Some(&row.client_id),
                    outcome.map(|_| ()),
                    started.elapsed().as_millis() as u64,
                ) {
                    eprintln!("{line}");
                }
            }
            next_heartbeat = std::time::Instant::now() + HEARTBEAT_INTERVAL;
        }
        if publisher_owner.as_ref().is_some_and(|owner| {
            qc_native_platform::owner_identity::native(
                owner.process_id,
                Some(owner.process_created_at),
            ) == qc_native_platform::owner_identity::Assessment::GoneOrDifferent
        }) {
            return;
        }
        if !publisher_authoritative && lifecycle.as_mut().is_some_and(BridgeLifecycle::stopped) {
            return;
        }
        tokio::select! {
            _ = interval.tick() => {},
            changed = announcement.changed() => {
                if changed.is_err() { return; }
                next_heartbeat = std::time::Instant::now();
            },
        }
    }
}

async fn wait_for_bridge_service<T>(
    waiting: impl std::future::Future<Output = Result<T, tokio::task::JoinError>>,
    stopped: impl std::future::Future<Output = ()>,
    cancel: impl FnOnce(),
) -> Result<(), String> {
    tokio::pin!(waiting);
    let result = tokio::select! {
        result = &mut waiting => result,
        () = stopped => {
            cancel();
            // Await the actual service original through cancellation.
            waiting.await
        }
    };
    result
        .map(|_| ())
        .map_err(|error| format!("The MCP stdio connection failed: {error}"))
}

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
    let (announcement_tx, announcement_rx) = watch::channel(None);
    let stopped = wait_for_bridge_stop(&backend, &client, &session_id, announcement_rx);
    tokio::pin!(stopped);
    let route = PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE);
    let mcp = QcMcp::for_route(backend.clone() as Arc<dyn QcBackend>, route);
    let initializing = mcp.serve(rmcp::transport::stdio());
    tokio::pin!(initializing);
    let service = tokio::select! {
        result = &mut initializing => match result {
            Ok(service) => service,
            Err(error) => return finish_bridge_shutdown(
                Err(format!("Could not serve MCP over stdio: {error}")),
                backend.stop_hosting().await,
            ),
        },
        () = &mut stopped => return finish_bridge_shutdown(Ok(()), backend.stop_hosting().await),
    };
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
    let _ = announcement_tx.send(Some(BridgeAnnouncement {
        client_id,
        client_name,
        client_version,
    }));
    let cancellation = service.cancellation_token();
    let result =
        wait_for_bridge_service(service.waiting(), &mut stopped, || cancellation.cancel()).await;
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
    let stopped = backend.stop_hosting().await;
    finish_bridge_shutdown(result, stopped)
}

pub fn run() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start the local MCP runtime: {error}"))?;
    let result = runtime.block_on(async {
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
    });
    finish_bridge_runtime(runtime, result)
}

fn finish_bridge_runtime(
    runtime: tokio::runtime::Runtime,
    result: Result<(), String>,
) -> Result<(), String> {
    // Tokio stdin uses an uncancellable blocking read while the AI host keeps
    // its input pipe open. The MCP service/listener have retired above; do not
    // let Runtime::drop wait forever for that read on an explicit broker Quit.
    // This is the bridge-only process entrypoint: returning closes its pipes
    // and the OS retires that process's remaining stdin read, not another host.
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
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
    #[test]
    #[ignore = "owned subprocess: bridge_quit_exits_with_ai_host_input_still_open"]
    fn bridge_quit_open_stdin_child() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(serve_bridge(None, None));
        finish_bridge_runtime(runtime, result).unwrap();
        eprintln!("bridge-shutdown-completed");
    }

    #[test]
    fn bridge_quit_exits_with_ai_host_input_still_open() {
        use crate::tests::{ExitObservation, OwnedChild};
        use std::io::Write;
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, guard) = exclusive_fixture_dir("bridge-open-stdin");
            let stdout = directory.join("stdout");
            let stderr = directory.join("stderr");
            let slot: crate::tests::ChildSlot = Default::default();
            crate::tests::arm_owned_child(
                cleanup,
                "bridge-open-stdin",
                &slot,
                None,
                Duration::ZERO,
            );
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "local_stdio::tests::bridge_quit_open_stdin_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env(STATE_OVERRIDE, directory.join(STATE_FILE))
                .stdin(std::process::Stdio::piped())
                .stdout(std::fs::File::create(&stdout).unwrap())
                .stderr(std::fs::File::create(&stderr).unwrap())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            crate::tests::fill_slot(&slot, OwnedChild::new(child).with_resource(guard));
            input.write_all(concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n",
                "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            ).as_bytes()).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            while !std::fs::read_to_string(&stdout)
                .unwrap_or_default()
                .contains("\"protocolVersion\"")
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "child did not initialize MCP: {}",
                    std::fs::read_to_string(&stderr).unwrap_or_default()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::write(
                crate::device_broker_descriptor::user_quit_path(
                    &directory.join("device-broker.json"),
                ),
                r#"{"instanceId":"fixture-quit","source":"tray"}"#,
            )
            .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let observation = slot.lock().unwrap().as_mut().unwrap().observe();
                if observation == ExitObservation::Exited {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "bridge must exit without the AI host closing stdin: {observation:?}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(std::fs::read_to_string(&stderr)
                .unwrap()
                .contains("bridge-shutdown-completed"));
            // Keep the host's input alive until AFTER the original child exited.
            drop(input);
        });
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn bridge_quit_cancels_the_actual_mcp_service_and_closes_its_transport() {
        use tokio::io::AsyncReadExt;
        let client = crate::loopback_http::loopback_client_builder()
            .build()
            .unwrap();
        let backend = Arc::new(BridgeBackend::new(
            DesktopBackend {
                client,
                session_id: None,
            },
            None,
            None,
            None,
        ));
        let (server, mut ai_host) = tokio::io::duplex(4096);
        let service = rmcp::service::serve_directly(
            QcMcp::for_route(
                backend as Arc<dyn QcBackend>,
                PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE),
            ),
            server,
            None,
        );
        let cancellation = service.cancellation_token();
        tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_bridge_service(service.waiting(), std::future::ready(()), || {
                cancellation.cancel()
            }),
        )
        .await
        .expect("the original service must retire")
        .unwrap();
        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), ai_host.read_to_end(&mut output))
            .await
            .expect("AI host observes EOF without closing its own input")
            .unwrap();
    }

    #[test]
    fn bridge_lifecycle_keeps_missing_metadata_and_stops_only_a_proven_gone_generation() {
        use crate::device_broker_descriptor::DeviceBrokerDescriptor;
        let descriptor = DeviceBrokerDescriptor {
            port: 1,
            bearer_token: "fixture-only".into(),
            instance_id: "original".into(),
            process_id: 42,
            process_created_at: Some(100),
            published_at_unix_ms: 0,
            gateway_api_version: None,
        };
        // Same policy in the ordinary user store and a packaged filesystem view.
        for path in [
            "user-store/device-broker.json",
            "package-view/device-broker.json",
        ] {
            let mut lifecycle = BridgeLifecycle {
                descriptor: PathBuf::from(path),
                observed: None,
            };
            assert!(
                !lifecycle.observe(None, |_| false),
                "not running at startup is not a Quit"
            );
            assert!(!lifecycle.observe(Some(descriptor.clone()), |_| true));
            assert!(
                !lifecycle.observe(None, |_| true),
                "stale/missing file is not process death"
            );
            let mut replacement = descriptor.clone();
            replacement.instance_id = "replacement".into();
            assert!(
                lifecycle.observe(Some(replacement), |_| false),
                "replacement cannot prolong the original session"
            );
        }
    }

    #[test]
    fn bridge_lifecycle_observes_quit_only_in_its_own_store() {
        let (root, _guard) = exclusive_fixture_dir("bridge-quit-stores");
        let first = root.join("user-store");
        let second = root.join("package-view");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let mut first = BridgeLifecycle {
            descriptor: first.join("device-broker.json"),
            observed: None,
        };
        let mut second = BridgeLifecycle {
            descriptor: second.join("device-broker.json"),
            observed: None,
        };
        std::fs::write(
            crate::device_broker_descriptor::user_quit_path(&first.descriptor),
            r#"{"instanceId":"fixture-generation","source":"tray"}"#,
        )
        .unwrap();
        assert!(
            first.stopped(),
            "Quit ends bridges even without further tool calls"
        );
        assert!(
            !second.stopped(),
            "another filesystem view is not this bridge's broker"
        );
    }

    #[tokio::test]
    async fn guarded_local_relay_authenticates_the_real_listener_capability_probe() {
        type Captured = (Value, Option<qc_relay_client::BrokerTarget>);
        struct Adapter(Mutex<Vec<Captured>>);
        #[async_trait]
        impl DeviceAdapter for Adapter {
            async fn ready(&self) -> bool {
                true
            }
            async fn invoke(&self, _: &str, _: Value) -> Result<Value, DeviceError> {
                panic!("the relay must retain its expected target")
            }
            async fn invoke_targeted(
                &self,
                method: &str,
                params: Value,
                target: Option<qc_relay_client::BrokerTarget>,
                _: bool,
            ) -> Result<Value, DeviceError> {
                assert_eq!(method, "device.setTempo");
                self.0.lock().unwrap().push((params, target));
                Ok(serde_json::json!({"accepted":true}))
            }
        }
        let (directory, _directory) = exclusive_fixture_dir("guarded-health-auth");
        let paths = vec![directory.join("local-mcp.json")];
        let adapter = Arc::new(Adapter(Mutex::new(Vec::new())));
        let (_access, receiver) = watch::channel(AccessMode::Full);
        let listener = local_access::start_listener(paths.clone(), adapter.clone(), receiver)
            .unwrap_or_else(|e| panic!("listener: {}", e.message));
        let mut endpoint =
            descriptor_from_paths(&paths).unwrap_or_else(|e| panic!("descriptor: {e}"));
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(&endpoint.health_url)
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "the production capability endpoint must remain authenticated"
        );
        // An edited diagnostic URL must neither receive the bearer nor divert
        // a guarded call away from the instance that proved its identity.
        endpoint.health_url = "http://127.0.0.1:1/untrusted-diagnostic".into();
        let backend = DesktopBackend {
            client,
            session_id: None,
        };
        let target = qc_relay_client::BrokerTarget {
            source: qc_relay_client::BrokerTargetSource::Demo,
            session_id: "isolated-demo".into(),
            generation: 1,
        };
        let params = serde_json::json!({"tempo":121});
        let answer = backend
            .send_targeted(
                endpoint,
                "device.setTempo",
                params.as_object().unwrap().clone(),
                Some(target.clone()),
            )
            .await;
        assert!(
            matches!(answer, Ok(ref value) if value["accepted"] == true),
            "authenticated guarded call was refused"
        );
        assert_eq!(
            *adapter.0.lock().unwrap(),
            vec![(params, Some(target))],
            "send once, retaining the original guard"
        );
        assert!(listener.stop(), "the exact listener must retire");
    }

    #[test]
    fn bridge_lifecycle_follows_authenticated_publisher_across_filesystem_views() {
        struct NoDeviceCalls;
        #[async_trait]
        impl DeviceAdapter for NoDeviceCalls {
            async fn ready(&self) -> bool {
                panic!("lifecycle must stay passive");
            }
            async fn invoke(&self, _: &str, _: Value) -> Result<Value, DeviceError> {
                panic!("no device call is allowed");
            }
        }
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let root = fixture_dir("bridge-lifecycle-publisher-view");
            let owned_root = root.clone();
            cleanup.arm("directory", move |report| {
                report.remove_dir_unless_blocked(&owned_root)
            });
            let publisher = root.join("installed");
            let own = root.join("package-view");
            std::fs::create_dir(&publisher).unwrap();
            std::fs::create_dir(&own).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for directory in [&root, &publisher, &own] {
                    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                }
            }
            let paths = vec![publisher.join(STATE_FILE)];
            let settings = test_settings(&own, full_access());
            record_user_quit(&settings, "unrelated-old-generation");
            let listener: Arc<Mutex<Option<local_access::RunningLocalAccess>>> =
                Arc::new(Mutex::new(None));
            let owned_listener = listener.clone();
            cleanup.arm("listener", move |report| {
                if let Some(listener) = owned_listener.lock().unwrap().take() {
                    if !listener.stop() {
                        report.unacknowledged("publisher listener did not retire".into());
                    }
                }
            });
            let (_access, receiver) = watch::channel(AccessMode::ReadOnly);
            *listener.lock().unwrap() = Some(
                local_access::start_listener(paths.clone(), Arc::new(NoDeviceCalls), receiver)
                    .unwrap_or_else(|e| panic!("{}", e.message)),
            );
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let client = crate::loopback_http::loopback_client_builder().build().unwrap();
                let backend = BridgeBackend::new(DesktopBackend { client:client.clone(), session_id:None }, Some(paths), Some(settings), None);
                let (announce, receiver) = watch::channel(None);
                let stopped = wait_for_bridge_stop(&backend, &client, "two-view-fixture", receiver);
                tokio::pin!(stopped);
                tokio::select! {
                    () = &mut stopped => panic!("the unrelated Quit in our own view must not override the authenticated publisher"),
                    () = tokio::time::sleep(Duration::from_millis(100)) => {},
                }
                std::fs::write(crate::device_broker_descriptor::user_quit_path(&publisher.join("device-broker.json")), r#"{"instanceId":"publisher-quit","source":"tray"}"#).unwrap();
                announce.send(Some(BridgeAnnouncement {client_id:String::new(), client_name:"two-view-fixture".into(), client_version:None})).unwrap();
                tokio::time::timeout(Duration::from_secs(4), &mut stopped).await.expect("publisher Quit must end the bridge despite its different filesystem view");
                backend.stop_hosting().await.unwrap();
            });
        });
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn guarded_local_relay_refuses_old_publishers_and_keeps_metadata_out_of_action_params() {
        for supported in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
            let recorded = calls.clone();
            let router = axum::Router::new()
                .route("/health", axum::routing::get(move |headers: axum::http::HeaderMap| async move {
                    // The real local router protects /health, unlike /identity.
                    // An unauthenticated capability probe must not pass this fixture.
                    if headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())
                        != Some(format!("Bearer {}", "T".repeat(43)).as_str()) {
                        return axum::response::IntoResponse::into_response(axum::http::StatusCode::UNAUTHORIZED);
                    }
                    axum::response::IntoResponse::into_response(axum::Json(serde_json::json!({"instanceId":"instance", "capabilities":if supported { vec!["localRpcBrokerTargetGuard"] } else { vec![] }})))
                }))
                .route("/rpc", axum::routing::post(move |axum::Json(value): axum::Json<serde_json::Value>| {
                    let recorded = recorded.clone();
                    async move {
                        recorded.lock().unwrap().push(value);
                        axum::Json(serde_json::json!({"ok":true,"result":{"accepted":true}}))
                    }
                }));
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
            let endpoint = super::validate_descriptor(document(
                &format!("http://127.0.0.1:{port}/rpc"),
                &"T".repeat(43),
            ))
            .unwrap();
            let backend = super::DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            };
            let target = qc_remote_mcp::BrokerTarget {
                source: qc_relay_client::BrokerTargetSource::Demo,
                session_id: "broker-1:demo-1".into(),
                generation: 1,
            };
            let mut params = serde_json::Map::new();
            params.insert("tempo".into(), serde_json::json!(120));
            let answer = backend
                .send_targeted(endpoint, "device.setTempo", params, Some(target.clone()))
                .await;
            let _ = stop.send(());
            server.await.unwrap();
            let calls = calls.lock().unwrap();
            if supported {
                assert!(answer.is_ok());
                assert_eq!(calls.len(), 1);
                assert_eq!(
                    calls[0]["expectedBrokerTarget"],
                    serde_json::to_value(target).unwrap()
                );
                assert_eq!(calls[0]["params"], serde_json::json!({"tempo":120}));
            } else {
                assert!(matches!(answer, Err(super::RelayFailure::Refused(_))));
                assert!(
                    calls.is_empty(),
                    "an older publisher must never receive a guarded action it would ignore"
                );
            }
        }
    }
    #[tokio::test]
    async fn resident_demo_readiness_is_fresh_and_never_attaches_or_claims_usb() {
        use qc_relay_client::DeviceAdapter;
        let generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let observed = generation.clone();
        let adapter = super::HeadlessDeviceAdapter::new(
            std::env::temp_dir().join("unused-demo-readiness.json"),
            false,
        )
        .with_broker_readiness(std::sync::Arc::new(move || {
            super::BrokerDeviceReadiness::ReadyTarget {
                usb_connected: false,
                target: qc_relay_client::BrokerTarget {
                    source: qc_relay_client::BrokerTargetSource::Demo,
                    session_id: "broker-1:demo-1".into(),
                    generation: observed.load(std::sync::atomic::Ordering::Acquire),
                },
            }
        }));
        let first = adapter.target_readiness().await;
        assert!(first.target_ready);
        assert!(!first.usb_connected);
        assert_eq!(first.broker_target.unwrap().generation, 1);
        generation.store(3, std::sync::atomic::Ordering::Release);
        assert_eq!(
            adapter
                .target_readiness()
                .await
                .broker_target
                .unwrap()
                .generation,
            3
        );
        assert!(adapter.ready().await);
        assert!(adapter.gateway.lock().unwrap().process.is_none());
    }
    #[tokio::test]
    async fn resident_unknown_skips_publication_but_loss_and_ambiguity_clear_readiness() {
        use qc_relay_client::DeviceAdapter;
        let physical = super::BrokerDeviceReadiness::ReadyTarget {
            usb_connected: true,
            target: qc_relay_client::BrokerTarget {
                source: qc_relay_client::BrokerTargetSource::Physical,
                session_id: "physical".into(),
                generation: 1,
            },
        };
        let sample = std::sync::Arc::new(std::sync::Mutex::new(physical));
        let observed = sample.clone();
        let adapter = super::HeadlessDeviceAdapter::new(
            std::env::temp_dir().join("unused-publication-probe.json"),
            false,
        )
        .with_broker_readiness(std::sync::Arc::new(move || {
            observed.lock().unwrap().clone()
        }));
        let known = adapter.publication_readiness().await.unwrap();
        assert!(known.target_ready && known.usb_connected);
        *sample.lock().unwrap() = super::BrokerDeviceReadiness::Unknown;
        assert!(adapter.publication_readiness().await.is_none());
        assert!(!adapter.target_readiness().await.target_ready);
        assert!(!adapter.ready().await);
        for unavailable in [
            super::BrokerDeviceReadiness::Unavailable {
                reason: "verified ambiguity".into(),
            },
            super::BrokerDeviceReadiness::Unavailable {
                reason: "shutting down".into(),
            },
            super::BrokerDeviceReadiness::NotReady {
                phase: "handshaking".into(),
            },
            super::BrokerDeviceReadiness::NotReady {
                phase: "disconnected".into(),
            },
        ] {
            *sample.lock().unwrap() = unavailable;
            let published = adapter.publication_readiness().await.unwrap();
            assert!(!published.target_ready);
            assert!(published.broker_target.is_none());
        }
        *sample.lock().unwrap() = super::BrokerDeviceReadiness::ReadyTarget {
            usb_connected: false,
            target: qc_relay_client::BrokerTarget {
                source: qc_relay_client::BrokerTargetSource::Demo,
                session_id: "demo".into(),
                generation: 3,
            },
        };
        let demo = adapter.publication_readiness().await.unwrap();
        assert!(demo.target_ready && !demo.usb_connected);
        assert!(
            adapter.gateway.lock().unwrap().process.is_none(),
            "a publication probe never attaches or opens USB"
        );
    }
    use super::*;
    struct StdioRelease(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for StdioRelease {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    static RETAINED_STDIO_FIXTURE_THREADS: Mutex<
        Vec<(std::thread::JoinHandle<()>, StdioFixturePermit)>,
    > = Mutex::new(Vec::new());
    static STDIO_FIXTURE_THREADS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    struct StdioFixturePermit;
    impl StdioFixturePermit {
        #[allow(
            deprecated,
            reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
        )]
        fn acquire() -> Self {
            let finished = {
                let mut retained = RETAINED_STDIO_FIXTURE_THREADS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut finished = Vec::new();
                let mut index = 0;
                while index < retained.len() {
                    if qc_native_platform::process::thread_signalled(&retained[index].0) {
                        finished.push(retained.swap_remove(index));
                    } else {
                        index += 1;
                    }
                }
                finished
            };
            // Joining and permit release never happen under the registry lock.
            for (worker, permit) in finished {
                let _ = worker.join();
                drop(permit);
            }
            STDIO_FIXTURE_THREADS
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    (count < 16).then_some(count + 1)
                })
                .expect(
                    "STDIO fixture thread budget exhausted; previous exact workers remain owned",
                );
            Self
        }
    }
    impl Drop for StdioFixturePermit {
        fn drop(&mut self) {
            STDIO_FIXTURE_THREADS.fetch_sub(1, Ordering::AcqRel);
        }
    }
    struct StdioFixtureThread {
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
        permit: Option<StdioFixturePermit>,
    }
    impl StdioFixtureThread {
        fn finish(&mut self) -> Result<(), String> {
            self.stop.store(true, Ordering::Release);
            let Some(worker) = self.worker.take() else {
                return Ok(());
            };
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !qc_native_platform::process::thread_signalled(&worker)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            let permit = self
                .permit
                .take()
                .expect("each admitted fixture worker retains its permit");
            if qc_native_platform::process::thread_signalled(&worker) {
                let result = worker
                    .join()
                    .map_err(|_| "STDIO fixture worker panicked".into());
                drop(permit);
                result
            } else {
                RETAINED_STDIO_FIXTURE_THREADS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((worker, permit));
                Err(
                    "STDIO fixture worker retirement unknown; exact original thread retained"
                        .into(),
                )
            }
        }
    }
    impl Drop for StdioFixtureThread {
        fn drop(&mut self) {
            if let Err(error) = self.finish() {
                eprintln!("{error}");
            }
        }
    }

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
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let permit = StdioFixturePermit::acquire();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => return,
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
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
        let mut fixture_worker = StdioFixtureThread {
            stop,
            worker: Some(worker),
            permit: Some(permit),
        };
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
        fixture_worker
            .finish()
            .expect("the exact squatter listener retired");
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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

    /// The exact ignored child test and the line that proves its body ran.
    /// An ambient environment flag never selects a child role or skips parent assertions.
    ///
    /// A `--exact` filter that matches nothing runs no tests and still exits 0,
    /// so the parent insists on seeing the last line rather than trusting the
    /// status alone.
    const STDERR_CHILD_TEST: &str = "local_stdio::tests::stderr_reader_process_fixture";
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
    #[ignore = "owned subprocess fixture, invoked only by the_request_path_alone_reports_the_copy_the_app_abandoned"]
    fn stderr_reader_process_fixture() {
        // The child exclusively allocates and owns this scratch root.
        let directory = fixture_dir("stderr-child");
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
    }

    #[test]
    fn the_request_path_alone_reports_the_copy_the_app_abandoned() {
        let output = crate::tests::run_owned_test_binary_fixture(
            std::process::Command::new(
                std::env::current_exe().expect("this test binary re-executes itself"),
            )
            .args(["--exact", STDERR_CHILD_TEST, "--ignored", "--nocapture"]),
        )
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
        let _directory = crate::tests::RemoveDirOnDrop::new(directory.clone());
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

    #[tokio::test]
    async fn admitted_http_mutations_with_lost_or_untyped_answers_are_not_retryable() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        for response in [
            "",
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{",
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad",
            "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{\"ok\":false}",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let applied = effects.clone();
            let mut server = Server(tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut length = None;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" { break; }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let length = length.unwrap();
                assert!(length < 4096);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).await.unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(request["rpc"], "device.setTempo");
                assert_eq!(request["params"]["bpm"], 120);
                applied.fetch_add(1, Ordering::SeqCst);
                reader.get_mut().write_all(response.as_bytes()).await.unwrap();
            }));
            let endpoint = validate_descriptor(document(
                &format!("http://{address}/rpc"), &"T".repeat(43)
            )).unwrap();
            let backend = DesktopBackend {
                client: reqwest::Client::builder().timeout(Duration::from_secs(2)).build().unwrap(),
                session_id: None,
            };
            let params = serde_json::json!({"bpm":120}).as_object().unwrap().clone();
            let failure = match backend.send(endpoint, "device.setTempo", params).await {
                Err(failure) => failure,
                Ok(value) => panic!("unexpected successful reply: {value}"),
            };
            let RelayFailure::Refused(error) = failure else { panic!("admitted write must not fall back"); };
            assert!(!error.retryable);
            assert_eq!(error.detail_code.as_deref(), Some("WRITE_OUTCOME_AMBIGUOUS"));
            assert_eq!(error.details.as_ref().unwrap()["requestSent"], true);
            assert_eq!(effects.load(Ordering::SeqCst), 1);
            tokio::time::timeout(Duration::from_secs(2), &mut server.0).await.unwrap().unwrap();
        }
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
        let (directory, _directory) = exclusive_fixture_dir("descriptor-fallback");
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
        let (directory, _directory) = exclusive_fixture_dir("descriptor-freshness");
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

    #[test]
    fn oversized_descriptor_is_refused_before_json_selection_and_valid_mirror_survives() {
        let (directory, _owned) = exclusive_fixture_dir("oversized-discovery");
        let oversized = directory.join("oversized.json");
        let valid = directory.join("valid.json");
        write_descriptor(&oversized, 41004, 900, 4);
        write_descriptor(&valid, 41005, 800, 4);
        let mut bytes = std::fs::read(&oversized).unwrap();
        bytes.resize(128 * 1024 + 1, b' ');
        // Syntactically valid JSON: the failure is the byte budget, not parser damage.
        assert!(serde_json::from_slice::<Value>(&bytes).is_ok());
        std::fs::write(&oversized, &bytes).unwrap();
        assert!(choose_descriptor(std::slice::from_ref(&oversized), 1000).is_err());
        let selected = descriptor_from_paths_at(&[oversized.clone(), valid], 1000).unwrap();
        assert_eq!(selected.rpc_url.port(), Some(41005));
        assert_eq!(
            std::fs::read(oversized).unwrap(),
            bytes,
            "refusal never edits discovery bytes"
        );
    }

    #[tokio::test]
    async fn actual_local_rpc_reply_refuses_growing_json_before_eof_without_fallback_or_replay() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        struct Finished(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Finished {
            fn drop(&mut self) {
                if let Some(done) = self.0.take() {
                    let _ = done.send(());
                }
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint =
            validate_descriptor(document(&format!("http://{address}/rpc"), &"T".repeat(43)))
                .unwrap();
        let backend = DesktopBackend {
            client: crate::loopback_http::loopback_client_builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            session_id: None,
        };
        let (done, observed) = tokio::sync::oneshot::channel();
        let result = {
            // No spawned/detachable server task: select owns the real socket
            // future on result, timeout, cancellation, and assertion unwind.
            let server = async move {
                let _finished = Finished(Some(done));
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await;
                let prefix = b"{\"ok\":true,\"result\":{}}";
                let mut body = prefix.to_vec();
                body.resize(16 * 1024 * 1024 + 1, b' ');
                for chunk in body.chunks(16 * 1024) {
                    if socket
                        .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                    if socket.write_all(chunk).await.is_err() {
                        break;
                    }
                    if socket.write_all(b"\r\n").await.is_err() {
                        break;
                    }
                }
                // Deliberately no terminal chunk/EOF; rejection must be byte-driven.
                std::future::pending::<()>().await;
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    biased;
                    () = server => unreachable!("controlled socket remains open until dropped"),
                    result = backend.send(endpoint, "device.setTempo", Map::new()) => result,
                }
            })
            .await
        };
        tokio::time::timeout(Duration::from_secs(2), observed)
            .await
            .unwrap()
            .unwrap();
        let failure = match result.expect("oversized reply waited for EOF/request deadline") {
            Err(error) => error,
            Ok(value) => panic!("oversized reply accepted: {value}"),
        };
        let RelayFailure::Refused(error) = failure else {
            panic!("admitted mutation must never select another publisher");
        };
        assert!(!error.retryable);
        assert_eq!(
            error.detail_code.as_deref(),
            Some("WRITE_OUTCOME_AMBIGUOUS")
        );
        assert_eq!(error.details.as_ref().unwrap()["requestSent"], true);
    }

    /// Settings for a headless host that touch nothing of the user's: the
    /// broker descriptor, local MCP switch, and canonical access policy live
    /// in `directory`; policy changes are written through the real store API
    /// from the fixture's `mode`, and no broker it spawns opens a
    /// USB session (the machine running the tests may have a real Quad Cortex
    /// in use by the installed app).
    fn test_settings(
        directory: &std::path::Path,
        mode: Arc<Mutex<AccessMode>>,
    ) -> HeadlessHostSettings {
        crate::access_policy_store::write(directory, *mode.lock().expect("mode"))
            .expect("persist initial test policy");
        let load_directory = directory.to_path_buf();
        let refresh_directory = directory.to_path_buf();
        let refresh_mode = mode.clone();
        HeadlessHostSettings {
            app_local_data_dir: directory.to_path_buf(),
            load_access_mode: Arc::new(move || {
                crate::access_policy_store::effective_mode(&load_directory)
            }),
            refresh_access_mode: Arc::new(move |_previous| {
                let current = crate::access_policy_store::effective_mode(&refresh_directory);
                let requested = *refresh_mode.lock().expect("mode");
                if current == requested {
                    current
                } else {
                    crate::access_policy_store::write(&refresh_directory, requested)
                        .map(|record| record.mode)
                        .unwrap_or(AccessMode::ReadOnly)
                }
            }),
            refresh_interval: Duration::from_millis(20),
            refresh_budget: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            auto_connect: false,
        }
    }

    fn full_access() -> Arc<Mutex<AccessMode>> {
        Arc::new(Mutex::new(AccessMode::Full))
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
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("background-no-client", cleanup, full_access());
            let _ = (&settings, &runtime, &bridges);
            let device = HeadlessDeviceAdapter::new(directory.join("device-broker.json"), false);
            arm_direct_fixture_device(cleanup, &device);
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
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    #[test]
    fn active_background_client_retains_one_shared_participant_and_idle_drops_it() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("background-participant", cleanup, full_access());
            let _ = (&settings, &runtime, &bridges);
            let descriptor =
                crate::device_broker_descriptor::device_broker_descriptor_path(&directory);
            let device = HeadlessDeviceAdapter::new(descriptor, false);
            #[cfg(not(windows))]
            let portable_originals = crate::tests::arm_mcp_fixture_children(cleanup);
            arm_direct_fixture_device(cleanup, &device);
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
            #[cfg(not(windows))]
            capture_portable_direct_originals(&device, &portable_originals);
            device.release_when_unused();
            assert!(
                device.gateway.lock().unwrap().process.is_none(),
                "listener alone cannot retain a broker participant"
            );
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// Black-box proof that a headless bridge process with no live publisher
    /// becomes one itself, and is discoverable through the *existing*,
    /// unmodified `descriptor_from_paths_at` - not through any special-cased
    /// internal state only this bridge's own code understands.
    #[test]
    fn a_bridge_process_hosts_mcp_itself_when_no_publisher_is_live() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("headless-host", cleanup, full_access());
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];
            assert!(
                descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
                "precondition: nothing is publishing yet"
            );

            let hosted = attempt_headless_mcp_hosting(&paths, &settings)
                .expect("a bridge process with no live publisher must become one");

            // It must now be discoverable exactly like a GUI-hosted publisher.
            let discovered = descriptor_from_paths_at(&paths, now_unix_ms())
                .expect("the newly self-hosted endpoint must be discoverable");
            assert_eq!(discovered.instance_id, hosted.instance_id());

            hosted.stop();
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// Two processes racing to self-host: exactly one becomes the publisher,
    /// and the other must fall back to relaying rather than also trying to
    /// serve. Reuses `local_access`'s own single-instance-guard behavior
    /// (`a_second_instance_does_not_take_over_a_live_instances_descriptor`)
    /// rather than reinventing the race here.
    #[test]
    fn a_bridge_process_falls_back_to_relaying_when_it_loses_the_hosting_race() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("headless-race", cleanup, full_access());
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];

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
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// C1: the headless host's first real device call must reach the shared
    /// broker every window uses - the one published in the app's local data
    /// directory - and not a private `--stdio` broker with its own USB
    /// connection. The descriptor path used to be known only to Tauri setup,
    /// which a `--local-mcp-bridge` process never runs.
    #[test]
    fn a_headless_hosts_device_calls_reach_the_shared_broker() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("headless-shared-broker", cleanup, full_access());
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];
            let hosted = attempt_headless_mcp_hosting(&paths, &settings).expect("the bridge hosts");

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
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// C2: a user who switched local MCP off gets no headless host either.
    #[test]
    fn a_bridge_does_not_host_while_local_mcp_is_switched_off() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("headless-switched-off", cleanup, full_access());
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];
            local_access::store_local_mcp_enabled_in(&directory, false).expect("preference");

            assert!(
                attempt_headless_mcp_hosting(&paths, &settings).is_none(),
                "local MCP is off, so nothing may be served"
            );
            assert!(
                descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
                "and nothing is published"
            );
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// C2: the host enforces the access level the user chose, not full
    /// access, and follows a later change made in a window.
    #[test]
    fn a_headless_host_enforces_and_follows_the_persisted_access_level() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let mode = Arc::new(Mutex::new(AccessMode::ReadOnly));
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture_with_mode("headless-access", cleanup, mode.clone());
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];
            let hosted = attempt_headless_mcp_hosting(&paths, &settings).expect("the bridge hosts");
            assert_eq!(*hosted.access.borrow(), AccessMode::ReadOnly);

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
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// C2: switching local MCP off while a headless host serves stops it and
    /// unpublishes it, as the switch does for a window's own listener.
    #[test]
    fn switching_local_mcp_off_stops_a_running_headless_host() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) = arm_legacy_mcp_fixture_with_mode(
                "headless-switch-off-later",
                cleanup,
                full_access(),
            );
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];
            let hosted = attempt_headless_mcp_hosting(&paths, &settings).expect("the bridge hosts");

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
        });
        crate::tests::finish_or_rethrow(outcome, report);
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
        let _release_on_unwind = StdioRelease(Some(release.clone()));
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
        let _release_on_unwind = StdioRelease(Some(release.clone()));
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
            let permit = StdioFixturePermit::acquire();
            let holder = std::thread::spawn(move || {
                let _guard = busy.lock().expect("gateway");
                held_tx.send(()).expect("held");
                let _ = release.recv_timeout(Duration::from_secs(30));
            });
            let mut holder = StdioFixtureThread {
                stop: Arc::new(AtomicBool::new(false)),
                worker: Some(holder),
                permit: Some(permit),
            };
            let _release_on_unwind = StdioRelease(Some(release_tx.clone()));
            held.recv_timeout(Duration::from_secs(2))
                .expect("the gateway is busy");
            let observed = runtime.block_on(adapter.readiness());
            release_tx.send(()).expect("release");
            holder.finish().expect("holder");
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
        let holder_permit = StdioFixturePermit::acquire();
        let holder = std::thread::spawn(move || {
            let _guard = busy.lock().expect("gateway");
            held_tx.send(()).expect("held");
            let _ = release.recv_timeout(Duration::from_secs(30));
        });
        let mut holder = StdioFixtureThread {
            stop: Arc::new(AtomicBool::new(false)),
            worker: Some(holder),
            permit: Some(holder_permit),
        };
        let release_on_unwind = StdioRelease(Some(release_tx.clone()));
        held.recv_timeout(Duration::from_secs(2))
            .expect("the gateway is busy");

        // On a thread of its own: a probe that does block would block the
        // runtime driving it too, and no async timeout could then fire.
        let (answered_tx, answered) = std::sync::mpsc::channel();
        let probe_permit = StdioFixturePermit::acquire();
        let probe = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let _ = answered_tx.send(runtime.block_on(adapter.ready()));
        });
        let mut probe = StdioFixtureThread {
            stop: Arc::new(AtomicBool::new(false)),
            worker: Some(probe),
            permit: Some(probe_permit),
        };
        // Release the held lock BEFORE either probe/holder retirement on an
        // assertion unwind; moving this guard changes its lexical drop order.
        let _release_on_unwind = release_on_unwind;
        assert!(
            answered.recv_timeout(Duration::from_secs(2)).is_ok(),
            "the probe answered while the gateway was busy"
        );

        release_tx.send(()).expect("release");
        holder.finish().expect("holder");
        probe.finish().expect("probe");
    }

    /// I4: a bridge that was relaying to another headless host must take over
    /// when that host goes away, not be stranded with nobody to relay to.
    #[test]
    fn a_relaying_bridge_hosts_itself_once_the_publisher_it_used_is_gone() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) = arm_legacy_mcp_fixture_with_mode(
                "headless-takeover",
                cleanup,
                Arc::new(Mutex::new(AccessMode::ReadOnly)),
            );
            let _ = (&settings, &runtime, &bridges);
            let paths = vec![directory.join(STATE_FILE)];
            let first = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge A hosts");

            // Bridge B, relaying to A through the same descriptor paths.
            let bridge = keep_mcp_bridge(
                &bridges,
                Arc::new(BridgeBackend::new(
                    DesktopBackend {
                        client: reqwest::Client::new(),
                        session_id: None,
                    },
                    Some(paths.clone()),
                    Some(settings.clone()),
                    None,
                )),
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

            runtime
                .block_on(bridge.stop_hosting())
                .expect("host slot retired");
        });
        crate::tests::finish_or_rethrow(outcome, report);
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
    static MCP_TEST_ACTORS: qc_native_platform::thread_owner::Budget =
        qc_native_platform::thread_owner::Budget::new(64);
    struct McpTestActor {
        stop: watch::Sender<bool>,
        task: Option<qc_native_platform::thread_owner::Owned>,
    }
    impl McpTestActor {
        fn stop_within(&mut self, within: Duration) -> Result<(), String> {
            let _ = self.stop.send(true);
            let joined = self
                .task
                .as_mut()
                .is_none_or(|task| task.wait_within(within));
            if joined {
                self.task.take();
                Ok(())
            } else {
                Err("MCP fixture retirement is unknown; exact original, attached resource, and admission remain retained".into())
            }
        }
    }
    impl Drop for McpTestActor {
        fn drop(&mut self) {
            let _ = self.stop.send(true);
            // Owned retains the exact unresolved original and its resource;
            // never abort the outer await that owns rmcp's private inner join.
        }
    }
    fn start_mcp_test_actor(
        backend: Arc<dyn QcBackend>,
        budget: &'static qc_native_platform::thread_owner::Budget,
        resource: impl Send + 'static,
        after_runtime: impl FnOnce() + Send + 'static,
    ) -> Result<(McpTestActor, tokio::io::DuplexStream), String> {
        let permit = budget.reserve()?;
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let (stop, mut stopped) = watch::channel(false);
        let task = qc_native_platform::thread_owner::Owned::spawn_reserved(
            "qc-mcp-exchange-fixture",
            permit,
            resource,
            move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("owned MCP fixture runtime");
                runtime.block_on(async move {
                    let server = QcMcp::for_route(backend, local_route());
                    let initialize = server.serve(server_io);
                    tokio::pin!(initialize);
                    let running = tokio::select! {
                        // rmcp only creates the service original when this
                        // initialization future returns Ready. Prefer that
                        // result so a simultaneously arriving stop cannot
                        // discard an already-created RunningService.
                        biased;
                        result = &mut initialize => match result {
                            Ok(running) => running,
                            Err(_) => return,
                        },
                        () = async {
                            while !*stopped.borrow() {
                                if stopped.changed().await.is_err() { break; }
                            }
                        } => return,
                    };
                    let cancel = running.cancellation_token();
                    let waiting = running.waiting();
                    tokio::pin!(waiting);
                    tokio::select! {
                        result = &mut waiting => { let _ = result; },
                        () = async {
                            while !*stopped.borrow() {
                                if stopped.changed().await.is_err() { break; }
                            }
                        } => {
                            cancel.cancel();
                            // Never time out/drop this private original's
                            // await. The external Owned observation is bounded.
                            let _ = waiting.await;
                        }
                    }
                });
                drop(runtime);
                after_runtime();
            },
        )?;
        Ok((
            McpTestActor {
                stop,
                task: Some(task),
            },
            client_io,
        ))
    }
    #[test]
    fn dependent_runtime_cleanup_reaped_inside_tokio_runs_on_its_owned_native_original() {
        // This independent async-reaper oracle remains unchanged below.
        static ORIGINALS: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        let mut cleanup = crate::tests::CleanupStack::default();
        let dependent_runtime = tokio::runtime::Runtime::new().unwrap();
        let (completed, completion) = std::sync::mpsc::channel();
        cleanup.arm("actual dependent runtime", move |_| {
            assert!(
                tokio::runtime::Handle::try_current().is_err(),
                "runtime shutdown must not run in the async reaper context"
            );
            dependent_runtime.shutdown_timeout(Duration::from_secs(2));
            let _ = completed.send(());
        });
        let (_, dependencies) = arm_mcp_exchange(&mut cleanup, Duration::from_millis(10));
        let (release, waiting) = std::sync::mpsc::channel();
        struct Finalize(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Finalize {
            fn drop(&mut self) {
                drop(self.0.take());
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while ORIGINALS.count() != 0 && std::time::Instant::now() < deadline {
                    qc_native_platform::thread_owner::reap();
                    std::thread::sleep(Duration::from_millis(2));
                }
                qc_native_platform::thread_owner::reap();
            }
        }
        let mut finalize = Finalize(Some(release));
        let original = qc_native_platform::thread_owner::Owned::spawn(
            "dependent-runtime-late-reap",
            &ORIGINALS,
            dependencies,
            move || {
                let _ = waiting.recv();
            },
        )
        .unwrap();
        drop(original);
        let initial = cleanup.run();
        assert!(
            initial.blocked,
            "still-live original owns the dependent transaction"
        );
        assert!(completion.try_recv().is_err());
        drop(finalize.0.take());
        let caller_runtime = tokio::runtime::Runtime::new().unwrap();
        caller_runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                qc_native_platform::thread_owner::reap();
                if completion.try_recv().is_ok() {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "actual dependent runtime cleanup failed after async-context late reap"
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        // The cleanup actor may still be in native TLS after publishing its
        // report; exact shared reaping, not the channel, releases admission.
        drop(finalize);
    }

    #[test]
    fn sole_owned_normal_mcp_cleanup_runs_declared_steps_on_the_outer_original() {
        let outer = std::thread::current().id();
        let root = fixture_dir("normal-outer-mcp-cleanup");
        let seen = Arc::new(AtomicBool::new(false));
        let owned = seen.clone();
        let directory = root.clone();
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            cleanup.arm("actual root", move |report| {
                report.remove_dir_unless_blocked(&directory)
            });
            cleanup.arm("actual normal outer transaction", move |_| {
                assert!(tokio::runtime::Handle::try_current().is_err());
                assert_eq!(std::thread::current().id(), outer,
                    "sole-owned normal cleanup must not compete with the late executor observation deadline");
                owned.store(true, Ordering::Release);
            });
            let (_actors, dependencies) = arm_mcp_exchange(cleanup, Duration::from_millis(10));
            drop(dependencies);
        });
        crate::tests::finish_or_rethrow(outcome, report);
        assert!(seen.load(Ordering::Acquire));
        assert!(!root.exists());
    }

    static MCP_CLEANUP_ACTORS: qc_native_platform::thread_owner::Budget =
        qc_native_platform::thread_owner::Budget::new(64);
    type McpCleanupCommand = (
        std::sync::mpsc::Sender<()>,
        qc_native_platform::thread_owner::Owned,
    );
    pub(super) struct McpDependencies {
        cleanup: std::sync::Mutex<Option<crate::tests::CleanupStack>>,
        pending: Arc<std::sync::Mutex<Option<crate::tests::CleanupStack>>>,
        original: std::sync::Mutex<Option<McpCleanupCommand>>,
    }
    impl McpDependencies {
        fn complete_on_outer(&mut self) -> Option<crate::tests::CleanupReport> {
            // Only Arc::try_unwrap outside Tokio may enter this branch. The
            // pre-armed executor has not received a stack or wake yet.
            if self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
            {
                return None;
            }
            let (wake, mut original) = self
                .original
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()?;
            let mut cleanup = self
                .cleanup
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()?;
            drop(wake); // Empty executor: retire the SAME original first.
            let joined = original.wait_within(Duration::from_secs(5));
            drop(original); // Unknown remains in the existing shared owner.
            if !joined {
                // This runs first, so the normal root callback retains bytes.
                cleanup.arm("empty MCP cleanup executor original", |report| {
                    report.unacknowledged("MCP cleanup executor native retirement unknown; original/admission retained".into());
                });
            }
            // These are the declared existing listener/broker/runtime cleanup
            // policies, not a second competing five-second report deadline.
            Some(cleanup.run())
        }
    }
    impl Drop for McpDependencies {
        fn drop(&mut self) {
            // A shared original can be reaped by an async caller. Never run
            // runtime.shutdown_timeout on that caller: the exact pre-armed
            // native cleanup original owns the entire dependent transaction.
            let cleanup = self
                .cleanup
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            *self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = cleanup;
            if let Some((wake, mut original)) = self
                .original
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                drop(wake); // Disconnect also wakes; no lost Stop notification.
                let within = if tokio::runtime::Handle::try_current().is_ok() {
                    Duration::ZERO
                } else {
                    Duration::from_secs(5)
                };
                let _ = original.wait_within(within);
                // Unknown retirement remains in the SAME shared original
                // family with pending/runtime/root ownership, never detached.
            }
        }
    }
    fn arm_mcp_exchange(
        cleanup: &mut crate::tests::CleanupStack,
        within: Duration,
    ) -> (Slot<McpTestActor>, Arc<McpDependencies>) {
        let report = Arc::new(std::sync::Mutex::new(None));
        let pending = Arc::new(std::sync::Mutex::new(None::<crate::tests::CleanupStack>));
        let pending_worker = pending.clone();
        let completed = report.clone();
        let (wake, waiting) = std::sync::mpsc::channel::<()>();
        // Start BEFORE moving the already-armed acquisition stack. If native
        // admission/spawn fails, the caller's original stack remains intact
        // for run_with_cleanup after block_on unwinds.
        let original = qc_native_platform::thread_owner::Owned::spawn(
            "qc-mcp-dependent-cleanup",
            &MCP_CLEANUP_ACTORS,
            pending.clone(),
            move || {
                let _ = waiting.recv();
                let cleanup = pending_worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(cleanup) = cleanup {
                    let report = cleanup.run();
                    let unclean = !report.is_clean();
                    *completed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(report);
                    if unclean {
                        let _ = std::panic::catch_unwind(|| {
                            if let Some(report) = completed
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .as_ref()
                            {
                                eprintln!("MCP dependent fixture retirement: {report}");
                            }
                        });
                    }
                }
            },
        )
        .expect("owned MCP dependent cleanup admitted before stack transfer");
        // Move the ACTUAL dependent cleanup, not merely its pathname. It
        // contains acquisition sinks, broker owners, runtime and root. It
        // cannot contain this actor callback, which is armed only afterward.
        let dependencies = Arc::new(McpDependencies {
            cleanup: std::sync::Mutex::new(Some(std::mem::take(cleanup))),
            pending,
            original: std::sync::Mutex::new(Some((wake, original))),
        });
        let actors: Slot<McpTestActor> = slot();
        let owned = actors.clone();
        let retained_dependencies = dependencies.clone();
        cleanup.arm("MCP actor and dependent transaction", move |outer| {
            if let Some(mut actor) = take(&owned) {
                if let Err(error) = actor.stop_within(within) {
                    outer.unacknowledged(error);
                }
                // Unknown Owned retains the same dependencies Arc in the
                // existing shared original family until actual native join.
                drop(actor);
            }
            // Ready resource holders must be actually reaped before deciding
            // whether this callback is the sole dependency authority.
            qc_native_platform::thread_owner::reap();
            let direct = if tokio::runtime::Handle::try_current().is_err() {
                match Arc::try_unwrap(retained_dependencies) {
                    Ok(mut dependencies) => dependencies.complete_on_outer(),
                    Err(shared) => { drop(shared); None }
                }
            } else {
                drop(retained_dependencies);
                None
            };
            let completed = direct.or_else(|| report.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner).take());
            if let Some(completed) = completed {
                outer.blocked |= completed.blocked;
                outer.problems.extend(completed.problems);
                outer.terminated.extend(completed.terminated);
            } else {
                outer.unacknowledged(
                    "Native original still owns the dependent listener/device/broker/runtime/root transaction".into(),
                );
            }
        });
        (actors, dependencies)
    }

    async fn mcp_exchange(
        cleanup: &mut crate::tests::CleanupStack,
        backend: Arc<BridgeBackend>,
        requests: &[Value],
    ) -> Vec<Value> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (actors, dependencies) = arm_mcp_exchange(cleanup, Duration::from_secs(5));
        *backend
            .fixture_dependencies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&dependencies);
        let client_io = {
            let mut owned = actors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (serving, client_io) =
                start_mcp_test_actor(backend, &MCP_TEST_ACTORS, dependencies, || {})
                    .expect("owned MCP fixture actor admitted");
            *owned = Some(serving);
            client_io
        };
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
        let mut owned = actors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owned
            .as_mut()
            .expect("armed MCP original")
            .stop_within(Duration::from_secs(5))
            .expect("MCP fixture original acknowledged shutdown");
        owned.take();
        answers
    }

    #[cfg(windows)]
    #[test]
    fn actual_mcp_fixture_unwind_retains_admitted_service_and_root_until_native_join() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Receipt(Arc<AtomicBool>);
        impl Drop for Receipt {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        struct PendingBackend {
            entered: std::sync::mpsc::Sender<()>,
            dropped: Arc<AtomicBool>,
            release: tokio::sync::watch::Receiver<bool>,
        }
        #[async_trait]
        impl QcBackend for PendingBackend {
            async fn request(
                &self,
                _route: &PrincipalRoute,
                _rpc: &'static str,
                _params: Map<String, Value>,
            ) -> Result<Value, BackendError> {
                let _receipt = Receipt(self.dropped.clone());
                let _ = self.entered.send(());
                let mut release = self.release.clone();
                while !*release.borrow_and_update() {
                    if release.changed().await.is_err() {
                        break;
                    }
                }
                Ok(serde_json::json!([]))
            }
        }
        struct TlsGate {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl Drop for TlsGate {
            fn drop(&mut self) {
                let _ = self.entered.send(());
                let _ = self.release.recv_timeout(Duration::from_secs(30));
            }
        }
        thread_local! { static TLS: std::cell::RefCell<Option<TlsGate>> = const { std::cell::RefCell::new(None) }; }
        struct Finalize {
            backend_release: tokio::sync::watch::Sender<bool>,
            release: Option<std::sync::mpsc::Sender<()>>,
            root: PathBuf,
        }
        impl Drop for Finalize {
            fn drop(&mut self) {
                // The actual SDK waits for an admitted backend request. Always
                // unblock it before TLS, even if any observation/assertion fails.
                let _ = self.backend_release.send(true);
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while BUDGET.count() != 0 && std::time::Instant::now() < deadline {
                    qc_native_platform::thread_owner::reap();
                    std::thread::sleep(Duration::from_millis(2));
                }
                if BUDGET.count() != 0 {
                    eprintln!("MCP fixture exact original/resource/admission retained after unknown native retirement: {}", self.root.display());
                }
            }
        }
        qc_native_platform::thread_owner::reap();
        assert_eq!(BUDGET.count(), 0);
        let mut cleanup = crate::tests::CleanupStack::default();
        let (root, settings, runtime, bridges) =
            arm_legacy_mcp_fixture("mcp-original-root", &mut cleanup);
        let (release, release_rx) = std::sync::mpsc::channel();
        let (backend_release, backend_release_rx) = tokio::sync::watch::channel(false);
        let finalize = Finalize {
            backend_release: backend_release.clone(),
            release: Some(release),
            root: root.clone(),
        };
        let (tls_entered, tls_observed) = std::sync::mpsc::channel();
        let (entered, observed) = std::sync::mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let paths = vec![root.join(STATE_FILE)];
        let hosted = attempt_headless_mcp_hosting(&paths, &settings)
            .expect("actual dependent listener acquired");
        let listener_address = published_listener(&paths).unwrap().address;
        drop(keep_mcp_bridge(
            &bridges,
            Arc::new(BridgeBackend::new(
                DesktopBackend {
                    client: reqwest::Client::new(),
                    session_id: None,
                },
                Some(paths),
                Some(settings),
                Some(hosted),
            )),
        ));
        drop(bridges);
        let (actors, resource) = arm_mcp_exchange(&mut cleanup, Duration::from_millis(50));
        let (actor, client) = start_mcp_test_actor(
            Arc::new(PendingBackend {
                entered,
                dropped: dropped.clone(),
                release: backend_release_rx.clone(),
            }),
            &BUDGET,
            resource,
            move || {
                TLS.with(|slot| {
                    *slot.borrow_mut() = Some(TlsGate {
                        entered: tls_entered,
                        release: release_rx,
                    })
                });
            },
        )
        .unwrap();
        *actors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(actor);
        let (read, mut write) = tokio::io::split(client);
        let mut lines = tokio::io::BufReader::new(read).lines();
        runtime.block_on(async {
            let init = serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owned-pending-fixture","version":"1"}}});
            write.write_all(format!("{init}\n").as_bytes()).await.unwrap();
            let answer = tokio::time::timeout(Duration::from_secs(2), lines.next_line()).await.unwrap().unwrap().unwrap();
            assert_eq!(serde_json::from_str::<Value>(&answer).unwrap()["id"], 0);
            write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
            write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"list_models\",\"arguments\":{\"query\":null}}}\n").await.unwrap();
        });
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        // Only the nested dependent stack owns the client runtime now.
        drop(runtime);
        // Keep the client pipe alive. Actual rmcp cancellation does not
        // discard an admitted backend request: ownership must remain retained.
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned_before_assertion = cleanup;
            panic!("injected MCP fixture assertion failure");
        }));
        assert!(failed.is_err());
        assert!(
            matches!(
                tls_observed.recv_timeout(Duration::from_millis(50)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "runtime/TLS must not retire while the admitted backend is held"
        );
        assert!(!dropped.load(Ordering::Acquire));
        qc_native_platform::thread_owner::reap();
        assert!(root.exists());
        assert!(
            std::net::TcpStream::connect_timeout(&listener_address, Duration::from_millis(100))
                .is_ok(),
            "unknown SDK original still owns its real dependent listener"
        );
        assert_eq!(BUDGET.count(), 1);
        assert!(
            BUDGET.reserve().is_err(),
            "pending admitted work still owns admission"
        );
        backend_release.send(true).unwrap();
        tls_observed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            dropped.load(Ordering::Acquire),
            "actual inner handler future did not retire"
        );
        qc_native_platform::thread_owner::reap();
        assert!(
            root.exists(),
            "resource retired on runtime/body acknowledgement instead of original join"
        );
        assert!(
            std::net::TcpStream::connect_timeout(&listener_address, Duration::from_millis(100))
                .is_ok(),
            "dependent listener cannot retire on body-only acknowledgement"
        );
        assert_eq!(BUDGET.count(), 1);
        assert!(start_mcp_test_actor(
            Arc::new(PendingBackend {
                entered: std::sync::mpsc::channel().0,
                dropped: dropped.clone(),
                release: backend_release_rx,
            }),
            &BUDGET,
            (),
            || panic!("must not spawn at capacity")
        )
        .is_err());
        drop(finalize);
        assert_eq!(BUDGET.count(), 0);
        assert!(!root.exists());
        assert!(
            std::net::TcpStream::connect_timeout(&listener_address, Duration::from_millis(100))
                .is_err(),
            "actual original join retires its dependent listener before root cleanup"
        );
        drop(lines);
        drop(write);
    }

    /// One `resources/read` for every backend-answered resource.
    #[test]
    fn bridge_shutdown_keeps_protocol_and_unknown_retirement_failures() {
        assert_eq!(finish_bridge_shutdown(Ok(()), Ok(())), Ok(()));
        assert_eq!(
            finish_bridge_shutdown(Err("protocol failed".into()), Ok(())),
            Err("protocol failed".into())
        );
        assert_eq!(
            finish_bridge_shutdown(Ok(()), Err("native retirement unknown".into())),
            Err("native retirement unknown".into())
        );
        assert_eq!(
            finish_bridge_shutdown(
                Err("protocol failed".into()),
                Err("native retirement unknown".into())
            ),
            Err("protocol failed; shutdown: native retirement unknown".into())
        );
    }

    #[test]
    fn actual_bridge_request_and_stop_do_not_wait_on_a_canceled_host_creator() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Finish(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Finish {
            fn drop(&mut self) {
                drop(self.0.take());
                let until = std::time::Instant::now() + Duration::from_secs(5);
                while BUDGET.count() != 0 && std::time::Instant::now() < until {
                    qc_native_platform::thread_owner::reap();
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        let mut cleanup = crate::tests::CleanupStack::default();
        let (root, settings, runtime, bridges) =
            arm_legacy_mcp_fixture("host-request-entry-held-original", &mut cleanup);
        let paths = vec![root.join(STATE_FILE)];
        let (wake, waiting) = std::sync::mpsc::channel();
        let mut finish = Finish(Some(wake));
        let waiting = Mutex::new(Some(waiting));
        let (entered, observed) = std::sync::mpsc::channel();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut backend = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            Some(settings),
            None,
        );
        backend.host_start_budget = &BUDGET;
        backend.host_start_timeout = Duration::from_secs(5);
        backend.host_start_attempts = Some(attempts.clone());
        backend.host_start_hook = Some(Arc::new(move || {
            let _ = entered.send(());
            if let Some(waiting) = waiting.lock().unwrap().take() {
                let _ = waiting.recv_timeout(Duration::from_secs(30));
            }
        }));
        let bridge = keep_mcp_bridge(&bridges, Arc::new(backend));
        drop(bridges);
        let (_, resource) = arm_mcp_exchange(&mut cleanup, Duration::from_millis(50));
        *bridge.fixture_dependencies.lock().unwrap() = Arc::downgrade(&resource);
        drop(resource);
        let creator = bridge.clone();
        let task = runtime.spawn(async move { creator.host_now().await });
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        task.abort();
        let canceled = runtime.block_on(task);
        assert!(matches!(canceled, Err(error) if error.is_cancelled()));
        assert_eq!(BUDGET.count(), 1);
        let mut reader = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            bridge.settings.clone(),
            None,
        );
        reader.host = bridge.host.clone();
        reader.host_stopping = bridge.host_stopping.clone();
        reader.host_start_budget = &BUDGET;
        reader.host_start_timeout = Duration::from_millis(100);
        // Actual QcBackend entry, not host_now alone. The held original owns
        // its real host lock while this request first performs cache lookup.
        let result = runtime
            .block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(2),
                    reader.request(&local_route(), "system.status", Map::new()),
                )
                .await
            })
            .expect("actual request entry must reach bounded refusal, not wait forever on cache");
        assert!(result.is_err());
        let error = runtime
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(1), reader.stop_hosting()).await
            })
            .expect("stop observation must not wait for unknown native creation")
            .unwrap_err();
        assert!(error.contains("not acknowledged"));
        assert!(reader.host_stopping.load(Ordering::Acquire));
        assert_eq!(
            BUDGET.count(),
            1,
            "shutdown observation does not discard original"
        );
        assert_eq!(attempts.load(Ordering::Acquire), 0);
        assert!(!paths[0].exists());
        drop(reader);
        drop(bridge);
        drop(runtime);
        let report = cleanup.run();
        assert!(report.blocked);
        assert!(root.exists());
        drop(finish.0.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while BUDGET.count() != 0 {
            qc_native_platform::thread_owner::reap();
            assert!(
                std::time::Instant::now() < deadline,
                "exact original failed to retire"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            attempts.load(Ordering::Acquire),
            0,
            "stopped creator must never publish late"
        );
        assert!(!root.exists());
    }

    #[test]
    fn canceled_hosting_admission_retains_original_dependencies_and_refuses_late_publication() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Release(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(release) = self.0.take() {
                    let _ = release.send(());
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while BUDGET.count() != 0 && std::time::Instant::now() < deadline {
                    qc_native_platform::thread_owner::reap();
                    std::thread::sleep(Duration::from_millis(2));
                }
                if BUDGET.count() != 0 {
                    eprintln!("hosting fixture original/dependencies remain retained");
                }
            }
        }
        qc_native_platform::thread_owner::reap();
        assert_eq!(BUDGET.count(), 0);
        let mut cleanup = crate::tests::CleanupStack::default();
        let (root, settings, runtime, bridges) =
            arm_legacy_mcp_fixture("host-start-canceled-original", &mut cleanup);
        let paths = vec![root.join(STATE_FILE)];
        let (release, blocked) = std::sync::mpsc::channel();
        let mut finish = Release(Some(release));
        let blocked = std::sync::Mutex::new(Some(blocked));
        let (entered, observed) = std::sync::mpsc::channel();
        let hooks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_count = hooks.clone();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut backend = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            Some(settings),
            None,
        );
        backend.host_start_budget = &BUDGET;
        backend.host_start_timeout = Duration::from_secs(5);
        backend.host_start_attempts = Some(attempts.clone());
        backend.host_start_hook = Some(Arc::new(move || {
            hook_count.fetch_add(1, Ordering::AcqRel);
            let _ = entered.send(());
            if let Some(blocked) = blocked.lock().unwrap().take() {
                let _ = blocked.recv_timeout(Duration::from_secs(30));
            }
        }));
        let bridge = keep_mcp_bridge(&bridges, Arc::new(backend));
        drop(bridges);
        let (_actors, resource) = arm_mcp_exchange(&mut cleanup, Duration::from_millis(50));
        *bridge.fixture_dependencies.lock().unwrap() = Arc::downgrade(&resource);
        // The actual host original upgrades this weak dependency. There is
        // no fixture-only strong backedge from BridgeBackend into its owner.
        drop(resource);
        let caller = bridge.clone();
        let task = runtime.spawn(async move { caller.host_now().await });
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(BUDGET.count(), 1);
        task.abort();
        let aborted = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(2), task).await })
            .expect("caller cancellation acknowledged");
        assert!(matches!(aborted, Err(error) if error.is_cancelled()));
        assert_eq!(
            BUDGET.count(),
            1,
            "caller abort cannot discard the native original"
        );
        assert!(
            bridge.host.try_lock().is_err(),
            "original retains actual host serialization"
        );
        let mut contender = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            bridge.settings.clone(),
            None,
        );
        contender.host = bridge.host.clone();
        contender.host_start_budget = &BUDGET;
        contender.host_start_timeout = Duration::from_millis(100);
        assert!(
            runtime.block_on(contender.host_now()).is_none(),
            "contender has one total observation budget"
        );
        drop(contender);
        assert_eq!(
            hooks.load(Ordering::Acquire),
            1,
            "no second native actor admitted behind held host slot"
        );
        assert_eq!(attempts.load(Ordering::Acquire), 0);
        assert!(!paths[0].exists());
        drop(bridge);
        drop(runtime);
        let report = cleanup.run();
        assert!(
            report.blocked,
            "pending native owner must keep dependent transaction: {report}"
        );
        assert!(root.exists(), "pending host original lost its root");
        finish.0.take().unwrap().send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while BUDGET.count() != 0 {
            qc_native_platform::thread_owner::reap();
            assert!(
                std::time::Instant::now() < deadline,
                "original did not acknowledge retirement"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            attempts.load(Ordering::Acquire),
            0,
            "canceled queued host started publication late"
        );
        assert!(
            !root.exists(),
            "actual original retirement completes dependent cleanup"
        );
    }

    #[test]
    fn canceled_admitted_host_stops_actual_published_listener_before_dependent_retirement() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Release(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(release) = self.0.take() {
                    let _ = release.send(());
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while BUDGET.count() != 0 && std::time::Instant::now() < deadline {
                    qc_native_platform::thread_owner::reap();
                    std::thread::sleep(Duration::from_millis(2));
                }
                if BUDGET.count() != 0 {
                    eprintln!("hosting fixture original/dependencies remain retained");
                }
            }
        }
        qc_native_platform::thread_owner::reap();
        assert_eq!(BUDGET.count(), 0);
        let mut cleanup = crate::tests::CleanupStack::default();
        let (root, settings, runtime, bridges) =
            arm_legacy_mcp_fixture("host-start-canceled-after-created", &mut cleanup);
        let paths = vec![root.join(STATE_FILE)];
        let (release, blocked) = std::sync::mpsc::channel();
        let mut finish = Release(Some(release));
        let blocked = std::sync::Mutex::new(Some(blocked));
        let (entered, observed) = std::sync::mpsc::channel();
        let hooks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook_count = hooks.clone();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut backend = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            Some(settings),
            None,
        );
        backend.host_start_budget = &BUDGET;
        backend.host_start_timeout = Duration::from_secs(5);
        backend.host_start_attempts = Some(attempts.clone());
        backend.host_created_hook = Some(Arc::new(move || {
            hook_count.fetch_add(1, Ordering::AcqRel);
            let _ = entered.send(());
            if let Some(blocked) = blocked.lock().unwrap().take() {
                let _ = blocked.recv_timeout(Duration::from_secs(30));
            }
        }));
        let bridge = keep_mcp_bridge(&bridges, Arc::new(backend));
        drop(bridges);
        let (_actors, resource) = arm_mcp_exchange(&mut cleanup, Duration::from_millis(50));
        *bridge.fixture_dependencies.lock().unwrap() = Arc::downgrade(&resource);
        // The actual host original upgrades this weak dependency. There is
        // no fixture-only strong backedge from BridgeBackend into its owner.
        drop(resource);
        let caller = bridge.clone();
        let task = runtime.spawn(async move { caller.host_now().await });
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(BUDGET.count(), 1);
        task.abort();
        let aborted = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(2), task).await })
            .expect("caller cancellation acknowledged");
        assert!(matches!(aborted, Err(error) if error.is_cancelled()));
        assert_eq!(
            BUDGET.count(),
            1,
            "caller abort cannot discard the native original"
        );
        assert!(
            bridge.host.try_lock().is_err(),
            "original retains actual host serialization"
        );
        let mut contender = BridgeBackend::new(
            DesktopBackend {
                client: reqwest::Client::new(),
                session_id: None,
            },
            Some(paths.clone()),
            bridge.settings.clone(),
            None,
        );
        contender.host = bridge.host.clone();
        contender.host_start_budget = &BUDGET;
        contender.host_start_timeout = Duration::from_millis(100);
        assert!(
            runtime.block_on(contender.host_now()).is_none(),
            "contender has one total observation budget"
        );
        drop(contender);
        assert_eq!(
            hooks.load(Ordering::Acquire),
            1,
            "no second native actor admitted behind held host slot"
        );
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        let published = published_listener(&paths)
            .expect("actual loopback host was published before cancellation");
        let address = published.address;
        assert!(
            std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok(),
            "actual created listener remains live while original is held"
        );
        drop(bridge);
        drop(runtime);
        let report = cleanup.run();
        assert!(
            report.blocked,
            "pending native owner must keep dependent transaction: {report}"
        );
        assert!(root.exists(), "pending host original lost its root");
        assert!(
            paths[0].exists(),
            "dependent cleanup must not erase a still-owned published host"
        );
        finish.0.take().unwrap().send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while BUDGET.count() != 0 {
            qc_native_platform::thread_owner::reap();
            assert!(
                std::time::Instant::now() < deadline,
                "original did not acknowledge retirement"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            attempts.load(Ordering::Acquire),
            1,
            "exactly one actual host creation"
        );
        assert!(
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err(),
            "late started.stop must retire the exact published loopback listener"
        );
        assert!(
            !root.exists(),
            "actual original retirement completes dependent cleanup"
        );
    }

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

    // Actual acquisition cleanup, armed before any corresponding effect.
    type McpFixtureBridges = Arc<std::sync::Mutex<Vec<Arc<BridgeBackend>>>>;
    type McpFixtureParts = (
        PathBuf,
        HeadlessHostSettings,
        Arc<tokio::runtime::Runtime>,
        McpFixtureBridges,
    );
    fn arm_legacy_mcp_fixture(
        name: &str,
        cleanup: &mut crate::tests::CleanupStack,
    ) -> McpFixtureParts {
        arm_legacy_mcp_fixture_with_mode(name, cleanup, full_access())
    }

    fn arm_legacy_mcp_fixture_with_mode(
        name: &str,
        cleanup: &mut crate::tests::CleanupStack,
        mode: Arc<Mutex<AccessMode>>,
    ) -> McpFixtureParts {
        // This actual file lease is Send and remains in the same dependency
        // stack through late actor retirement, not just the caller's scope.
        let real_broker_lock = crate::tests::serialize_real_broker_test();
        cleanup.arm("real broker fixture lease", move |_| drop(real_broker_lock));
        let directory = fixture_dir(name);
        let root = directory.clone();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&root)
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .expect("protect scratch directory");
        }
        let settings = test_settings(&directory, mode);
        let paths = [directory.join(STATE_FILE)];
        let runtime_slot: Slot<Arc<tokio::runtime::Runtime>> = slot();
        let owned_runtime = runtime_slot.clone();
        cleanup.arm("runtime", move |report| {
            if let Some(runtime) = take(&owned_runtime) {
                match Arc::try_unwrap(runtime) {
                    Ok(runtime) => runtime.shutdown_timeout(Duration::from_secs(2)),
                    Err(_) => report.unacknowledged("MCP client runtime remains shared".into()),
                }
            }
        });
        #[cfg(windows)]
        crate::tests::arm_mcp_fixture_broker(cleanup, &settings.device_broker_descriptor_path());
        #[cfg(not(windows))]
        let children = crate::tests::arm_mcp_fixture_children(cleanup);
        let (guard, acquired) = HostedListenerSinkGuard::install(&paths[0], false);
        let bridges = Arc::new(std::sync::Mutex::new(Vec::<Arc<BridgeBackend>>::new()));
        let cleanup_bridges = bridges.clone();
        let refresh_budget = settings.refresh_budget.clone();
        cleanup.arm("MCP listeners and devices", move |report| {
            let _guard = guard;
            let bridges = std::mem::take(
                &mut *cleanup_bridges
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            for bridge in bridges {
                match bridge.host.try_lock() {
                    Ok(mut host) => {
                        if let Some(host) = host.take() {
                            host.stop();
                        }
                    }
                    Err(_) => report
                        .unacknowledged("MCP hosting admission still owns its bridge slot".into()),
                };
            }
            let listeners = std::mem::take(
                &mut *acquired
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            for listener in listeners {
                if !listener.stop() {
                    report.unacknowledged("MCP listener original retirement unknown".into());
                }
                let device = &listener.fixture_device;
                device.stopped.store(true, Ordering::Release);
                match device.gateway.try_lock() {
                    Ok(mut gateway) => {
                        #[cfg(not(windows))]
                        {
                            let gateway: &mut crate::Gateway = &mut gateway;
                            let mut children = children
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            for process in [&mut gateway.process, &mut gateway.retiring_process]
                                .into_iter()
                                .flatten()
                            {
                                if let Some(child) = process.take_hosted_broker_child() {
                                    children.push(crate::tests::OwnedChild::new(child));
                                }
                            }
                        }
                        gateway.retire_current_process();
                    }
                    Err(_) => report.unacknowledged("MCP device gateway remains in use".into()),
                };
            }
            reap_refresh_workers();
            if refresh_budget.load(Ordering::Acquire) != 0 {
                report.unacknowledged("MCP settings refresher original remains retained".into());
            }
        });
        let runtime = Arc::new(tokio::runtime::Runtime::new().expect("MCP fixture runtime"));
        *runtime_slot.lock().unwrap() = Some(runtime.clone());
        (directory, settings, runtime, bridges)
    }

    /// Direct adapters use the same prearmed dependency stack as hosted ones.
    fn arm_direct_fixture_device(
        cleanup: &mut crate::tests::CleanupStack,
        device: &HeadlessDeviceAdapter,
    ) {
        // Original Child slots are armed before any retain/invoke can spawn.
        // Keep current and retiring generations distinct; never overwrite one.
        let current: crate::tests::ChildSlot = Default::default();
        let retiring: crate::tests::ChildSlot = Default::default();
        for slot in [&current, &retiring] {
            crate::tests::arm_owned_child(
                cleanup,
                "direct MCP hosted broker",
                slot,
                None,
                Duration::ZERO,
            );
        }
        let device = device.clone();
        cleanup.arm("direct MCP device", move |report| {
            device.stopped.store(true, Ordering::Release);
            match device.gateway.try_lock() {
                Ok(mut gateway) => {
                    let gateway: &mut crate::Gateway = &mut gateway;
                    for (process, slot) in [
                        (&mut gateway.process, &current),
                        (&mut gateway.retiring_process, &retiring),
                    ] {
                        if let Some(child) = process
                            .as_mut()
                            .and_then(|process| process.take_hosted_broker_child())
                        {
                            crate::tests::fill_slot(slot, crate::tests::OwnedChild::new(child));
                        }
                    }
                    gateway.retire_current_process();
                }
                Err(_) => {
                    report.unacknowledged("Direct MCP fixture gateway remains owned/in use".into())
                }
            };
        });
    }

    #[cfg(not(windows))]
    fn capture_portable_direct_originals(
        device: &HeadlessDeviceAdapter,
        originals: &Arc<Mutex<Vec<crate::tests::OwnedChild>>>,
    ) {
        // The cohort is already admitted/armed. Capture BEFORE an explicit
        // release can consume the transport and its PID-pinning Child.
        let mut originals = originals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut gateway = device
            .gateway
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let gateway: &mut crate::Gateway = &mut gateway;
        for process in [&mut gateway.process, &mut gateway.retiring_process]
            .into_iter()
            .flatten()
        {
            if let Some(child) = process.take_hosted_broker_child() {
                originals.push(crate::tests::OwnedChild::new(child));
            }
        }
    }

    #[test]
    fn legacy_headless_host_and_direct_adapter_unwind_retire_dependencies_before_root() {
        for direct in [false, true] {
            let seen_root = Mutex::new(None::<PathBuf>);
            let seen_listener = Mutex::new(None::<std::net::SocketAddr>);
            let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
                let (root, settings, _runtime, _bridges) = arm_legacy_mcp_fixture_with_mode(
                    if direct {
                        "legacy-direct-unwind"
                    } else {
                        "legacy-host-unwind"
                    },
                    cleanup,
                    full_access(),
                );
                *seen_root.lock().unwrap() = Some(root.clone());
                if direct {
                    let device =
                        HeadlessDeviceAdapter::new(settings.device_broker_descriptor_path(), false);
                    #[cfg(not(windows))]
                    let portable_originals = crate::tests::arm_mcp_fixture_children(cleanup);
                    arm_direct_fixture_device(cleanup, &device);
                    device.retain_while_used();
                    assert!(
                        device.gateway.lock().unwrap().process.is_some(),
                        "actual isolated software broker participant must be acquired",
                    );
                    #[cfg(not(windows))]
                    capture_portable_direct_originals(&device, &portable_originals);
                    device.release_when_unused();
                    assert!(device.gateway.lock().unwrap().process.is_none());
                    panic!("injected legacy direct-adapter assertion after real release");
                } else {
                    let paths = vec![root.join(STATE_FILE)];
                    let host = attempt_headless_mcp_hosting(&paths, &settings)
                        .expect("actual legacy listener acquired");
                    *seen_listener.lock().unwrap() =
                        Some(published_listener(&paths).unwrap().address);
                    assert!(root.exists());
                    assert!(!host.is_disabled());
                    panic!("injected legacy hosted assertion");
                }
            });
            assert!(
                outcome.is_err(),
                "the original assertion failure is preserved"
            );
            assert!(report.is_clean(), "{report}");
            let root = seen_root
                .into_inner()
                .unwrap()
                .expect("actual acquired root");
            assert!(
                !root.exists(),
                "dependency retirement must precede root removal"
            );
            if let Some(address) = seen_listener.into_inner().unwrap() {
                // Original retirement was already required by the cleanup
                // report. A connect timeout is not an exit observation on
                // Windows; require actual same-address ownership instead.
                let rebound = std::net::TcpListener::bind(address)
                    .expect("acknowledged original listener must release its exact address");
                assert_eq!(rebound.local_addr().unwrap(), address);
            }
        }
    }
    fn keep_mcp_bridge(
        bridges: &McpFixtureBridges,
        bridge: Arc<BridgeBackend>,
    ) -> Arc<BridgeBackend> {
        bridges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(bridge.clone());
        bridge
    }

    /// A scratch directory of this test's own: a fresh random name, created
    /// exclusively, removed when the test ends or unwinds.
    fn exclusive_fixture_dir(name: &str) -> (PathBuf, crate::tests::RemoveDirOnDrop) {
        let directory = crate::tests::unique_fixture_directory(&format!("qc-native-local-{name}"));
        let guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .expect("protect scratch directory");
        }
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
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("relayed-resource-read", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let hosting = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge B hosts");
            let device = hosting.device.clone();
            let broker_descriptor = settings.device_broker_descriptor_path();
            let relaying = keep_mcp_bridge(
                &bridges,
                relaying_bridge(&paths, &settings, "stdio-relaying-bridge-a"),
            );

            let reads = backend_resource_reads();
            let answers = runtime.block_on(mcp_exchange(cleanup, relaying.clone(), &reads));

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
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// Control (HTTP): the `list_models` tool, relayed the same way,
    /// still lets B bring a broker up, as before. The broker opens no USB
    /// session (`test_settings`); like C1 it outlives the bridges by its
    /// grace period, which the teardown waits out before the fixture goes.
    #[test]
    fn a_relayed_tool_call_still_lets_the_hosting_bridge_start_a_broker() {
        // The actual healthy broker-start oracle remains unchanged below.
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("relayed-tool-call", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let hosting = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge B hosts");
            let device = hosting.device.clone();
            let relaying = keep_mcp_bridge(
                &bridges,
                relaying_bridge(&paths, &settings, "stdio-relaying-tool-call"),
            );

            let answers = runtime.block_on(mcp_exchange(
                cleanup,
                relaying.clone(),
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
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    #[test]
    fn stopping_a_real_host_refuses_a_previously_cloned_backend_queued_at_its_gateway() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, _bridges) =
                arm_legacy_mcp_fixture("host-stop-queued-request", cleanup);
            let host = attempt_headless_mcp_hosting(&[directory.join(STATE_FILE)], &settings)
                .expect("actual isolated hosted listener");
            let backend = host.backend();
            let device = host.device.clone();
            let descriptor = settings.device_broker_descriptor_path();
            let held = device.gateway.lock().unwrap();
            assert!(held.process.is_none());
            assert!(!descriptor.exists(), "no broker before the actual request");
            let route = local_route();
            let mut request = backend.request(&route, "device.listModels", Map::new());
            runtime.block_on(std::future::poll_fn(|cx| {
                assert!(
                    request.as_mut().poll(cx).is_pending(),
                    "the actual native gateway request must queue behind the held mutex"
                );
                std::task::Poll::Ready(())
            }));
            host.stop();
            drop(held);
            let result = runtime
                .block_on(async { tokio::time::timeout(Duration::from_secs(5), request).await })
                .expect("queued request answers within its observation budget");
            let error =
                result.expect_err("a stopped host cannot dispatch a previously captured backend");
            assert!(
                error.message.to_lowercase().contains("stopping"),
                "{error:?}"
            );
            assert!(
                device.gateway.lock().unwrap().process.is_none(),
                "stop refusal happens before attach/election/broker creation"
            );
            assert!(!descriptor.exists(), "no native broker-start side effect");
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// With nobody publishing, an MCP resource read must not make a
    /// relaying bridge host. A hosting bridge's device call spawns an
    /// auto-connecting broker: after Quit, only the user brings it back.
    #[test]
    fn a_resource_read_after_quit_does_not_make_the_bridge_host() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("resource-read-after-quit", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let bridge = keep_mcp_bridge(&bridges, bridge_after_quit(&paths, &settings));

            let reads = backend_resource_reads();
            let answers = runtime.block_on(mcp_exchange(cleanup, bridge.clone(), &reads));

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
            runtime
                .block_on(bridge.stop_hosting())
                .expect("host slot retired");
        });
        crate::tests::finish_or_rethrow(outcome, report);
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
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("tool-call-after-quit", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let seam = crate::GatewayStartSeam::install(&settings.device_broker_descriptor_path());
            let bridge = keep_mcp_bridge(&bridges, bridge_after_quit(&paths, &settings));

            let answers = runtime.block_on(mcp_exchange(
                cleanup,
                bridge.clone(),
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
            runtime
                .block_on(bridge.stop_hosting())
                .expect("host slot retired");
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// First use is unchanged. Nobody publishes and no Quit was
    /// recorded (a window closed, another bridge's client went away, or
    /// nothing ever ran), so the tool call makes the bridge host and reach
    /// for a broker, here the start seam, which starts no process.
    #[test]
    fn a_tool_call_with_no_recorded_quit_still_makes_the_bridge_host() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("tool-call-first-use", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let seam = crate::GatewayStartSeam::install(&settings.device_broker_descriptor_path());
            let bridge = keep_mcp_bridge(&bridges, bridge_after_publisher_left(&paths, &settings));

            let answers = runtime.block_on(mcp_exchange(
                cleanup,
                bridge.clone(),
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
            runtime
                .block_on(bridge.stop_hosting())
                .expect("host slot retired");
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// An explicit user start supersedes the Quit (it clears the
    /// record, `crate::acknowledge_user_connect_in`), and the next tool call
    /// hosts again.
    #[test]
    fn an_explicit_start_after_quit_lets_a_tool_call_host_again() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("explicit-start-after-quit", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let seam = crate::GatewayStartSeam::install(&settings.device_broker_descriptor_path());
            let bridge = keep_mcp_bridge(&bridges, bridge_after_quit(&paths, &settings));
            assert!(
                crate::acknowledge_user_connect_in(&settings.device_broker_descriptor_path())
                    .is_ok()
            );

            let answers = runtime.block_on(mcp_exchange(
                cleanup,
                bridge.clone(),
                &[list_models_tool_call()],
            ));

            assert!(
                runtime.block_on(bridge.is_hosting()),
                "the Quit no longer stands: {answers:?}"
            );
            assert!(seam.starts() >= 1);
            runtime
                .block_on(bridge.stop_hosting())
                .expect("host slot retired");
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// A bridge starting after the Quit (a new chat) does not
    /// host either; with no Quit recorded it hosts as before.
    #[test]
    fn a_new_bridge_after_quit_does_not_host_at_startup() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, _runtime, _bridges) =
                arm_legacy_mcp_fixture("startup-after-quit", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            record_user_quit(&settings, "quit-generation");
            assert!(startup_host(Some(paths.clone()), Some(settings.clone())).is_none());
            assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_err());

            crate::device_broker_descriptor::clear_user_quit(
                &settings.device_broker_descriptor_path(),
            )
            .expect("cleared");
            let host = startup_host(Some(paths.clone()), Some(settings))
                .expect("with no Quit recorded a starting bridge hosts");
            assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_ok());
            host.stop();
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// A bridge that decided to host before a Quit was recorded
    /// (it waited for its host lock and a blocking thread meanwhile) is
    /// refused at the hosting admission itself: no listener is published, and
    /// `host_now` refuses before it waits at all. With the Quit cleared, the
    /// same admission hosts.
    #[test]
    fn the_hosting_admission_refuses_a_quit_recorded_while_the_bridge_waited() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("admission-after-wait", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let bridge = keep_mcp_bridge(&bridges, bridge_after_publisher_left(&paths, &settings));
            // Decided to host with nothing recorded; the Quit lands during the wait.
            record_user_quit(&settings, "quit-during-the-wait");
            assert!(attempt_headless_mcp_hosting(&paths, &settings).is_none());
            assert!(
                descriptor_from_paths_at(&paths, now_unix_ms()).is_err(),
                "no fresh listener was published"
            );
            assert!(runtime.block_on(bridge.host_now()).is_none());
            assert!(!runtime.block_on(bridge.is_hosting()));

            crate::device_broker_descriptor::clear_user_quit(
                &settings.device_broker_descriptor_path(),
            )
            .expect("cleared");
            let host = attempt_headless_mcp_hosting(&paths, &settings)
                .expect("with the Quit cleared the same admission hosts");
            assert!(descriptor_from_paths_at(&paths, now_unix_ms()).is_ok());
            host.stop();
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// A bridge that hosts while its gateway holds no broker connection -
    /// as `run` leaves one that started while nobody published - answers a
    /// resource read in its own process offline, without attaching to,
    /// recovering or spawning a broker, and goes on hosting.
    #[test]
    fn a_bridge_hosting_after_quit_answers_a_resource_read_offline() {
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("hosting-resource-read-after-quit", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let hosted = attempt_headless_mcp_hosting(&paths, &settings)
                .expect("the bridge hosts, as `run` does when nobody publishes");
            let device = hosted.device.clone();
            assert!(
                !device.gateway_attached(),
                "precondition: no broker connection"
            );
            let bridge = keep_mcp_bridge(
                &bridges,
                Arc::new(BridgeBackend::new(
                    DesktopBackend {
                        client: reqwest::Client::new(),
                        session_id: Some(Arc::from("stdio-hosting-after-quit-test")),
                    },
                    Some(paths.clone()),
                    Some(settings.clone()),
                    Some(hosted),
                )),
            );

            let reads = backend_resource_reads();
            let answers = runtime.block_on(mcp_exchange(cleanup, bridge.clone(), &reads));

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
            runtime
                .block_on(bridge.stop_hosting())
                .expect("host slot retired");
        });
        crate::tests::finish_or_rethrow(outcome, report);
    }

    /// Initialize (every exchange starts with it), the listings and
    /// the static guide and knowledge resources never reach a backend, so
    /// they neither make a bridge host after Quit nor bring a hosting
    /// bridge's broker connection up through a relay. The heartbeat is
    /// passive by construction: `send_bridge_status` only reads the descriptor
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

        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("passive-after-quit", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let bridge = keep_mcp_bridge(&bridges, bridge_after_quit(&paths, &settings));
            let answers = runtime.block_on(mcp_exchange(cleanup, bridge.clone(), &passive));
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
        });
        crate::tests::finish_or_rethrow(outcome, report);
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let (directory, settings, runtime, bridges) =
                arm_legacy_mcp_fixture("passive-through-relay", cleanup);
            let paths = vec![directory.join(STATE_FILE)];
            let hosting = attempt_headless_mcp_hosting(&paths, &settings).expect("bridge B hosts");
            let device = hosting.device.clone();
            let relaying = keep_mcp_bridge(
                &bridges,
                relaying_bridge(&paths, &settings, "stdio-passive-relay"),
            );
            let answers = runtime.block_on(mcp_exchange(cleanup, relaying.clone(), &passive));
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
        });
        crate::tests::finish_or_rethrow(outcome, report);
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

    #[test]
    fn dropping_an_acquired_headless_host_stops_its_real_publisher_without_a_sink() {
        let mut cleanup = crate::tests::CleanupStack::default();
        let directory = fixture_dir("headless-host-drop");
        let owned_root = directory.clone();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&owned_root)
        });
        let paths = vec![directory.join(STATE_FILE)];
        let settings = test_settings(&directory, full_access());
        let host = attempt_headless_mcp_hosting(&paths, &settings).expect("acquired host");
        let running = host.running.clone();
        let fallback = running.clone();
        cleanup.arm("listener", move |report| {
            if !fallback.stop() {
                report.unacknowledged("listener retirement was not acknowledged".into());
            }
        });
        assert!(paths[0].is_file());
        assert_eq!(*running.stop_acknowledged.lock().unwrap(), None);
        // This is the production owner being discarded by an initialization
        // error or an abandoned spawn_blocking result, not test-sink cleanup.
        drop(host);
        assert_eq!(*running.stop_acknowledged.lock().unwrap(), Some(true));
        assert!(!paths[0].exists());
    }

    #[cfg(windows)]
    #[test]
    fn native_tls_headless_refresh_holds_capacity_through_original_join() {
        struct Gate(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
        impl Drop for Gate {
            fn drop(&mut self) {
                let _ = self.0.send(());
                let _ = self.1.recv_timeout(Duration::from_secs(5));
            }
        }
        thread_local! { static GATE: std::cell::RefCell<Option<Gate>> = const { std::cell::RefCell::new(None) }; }
        struct Cleanup {
            release: Option<std::sync::mpsc::Sender<()>>,
            original: Option<RefreshWorker>,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(sender) = self.release.take() {
                    let _ = sender.send(());
                }
                if let Some(original) = self.original.take() {
                    retire_refresh_worker(original, Duration::from_secs(1));
                }
                reap_refresh_workers();
            }
        }
        let budget = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let permit = RefreshPermit::acquire(&budget).unwrap();
        let other: Vec<_> = (0..3)
            .map(|_| RefreshPermit::acquire(&budget).unwrap())
            .collect();
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            GATE.with(|gate| *gate.borrow_mut() = Some(Gate(entered, held)))
        });
        let mut cleanup = Cleanup {
            release: Some(release),
            original: Some(RefreshWorker {
                thread,
                _permit: permit,
            }),
        };
        observed.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(cleanup.original.as_ref().unwrap().thread.is_finished());
        assert!(!qc_native_platform::process::thread_signalled(
            &cleanup.original.as_ref().unwrap().thread
        ));
        let began = std::time::Instant::now();
        retire_refresh_worker(cleanup.original.take().unwrap(), Duration::from_millis(25));
        reap_refresh_workers();
        assert!(began.elapsed() < Duration::from_secs(1));
        assert_eq!(budget.load(Ordering::Acquire), 4);
        assert!(RefreshPermit::acquire(&budget).is_none());
        cleanup.release.take().unwrap().send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while budget.load(Ordering::Acquire) != 3 {
            reap_refresh_workers();
            assert!(
                std::time::Instant::now() < deadline,
                "native refresh original failed to retire"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(other);
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_stalled_refresh_worker_is_retained_until_its_actual_completion() {
        let (release, gate) = std::sync::mpsc::channel();
        struct Release(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let release = Release(Some(release));
        let (completed, completion) = std::sync::mpsc::channel();
        let budget = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let permit = RefreshPermit::acquire(&budget).unwrap();
        let worker = std::thread::spawn(move || {
            let _ = gate.recv_timeout(Duration::from_secs(30));
            let _ = completed.send(());
        });
        let worker = RefreshWorker {
            thread: worker,
            _permit: permit,
        };
        let id = worker.thread().id();
        retire_refresh_worker(worker, Duration::from_millis(10));
        assert!(RETAINED_HEADLESS_REFRESH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|worker| worker.thread().id() == id
                && !qc_native_platform::process::thread_signalled(&worker.thread)));
        assert_eq!(budget.load(Ordering::Acquire), 1);
        drop(release);
        completion
            .recv_timeout(Duration::from_secs(3))
            .expect("this exact refresh worker acknowledged release");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            reap_refresh_workers();
            let retained = RETAINED_HEADLESS_REFRESH
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|worker| worker.thread().id() == id);
            // Another reaper may have removed the entry but still be joining
            // its original. Only the final permit drop proves retirement.
            if !retained && budget.load(Ordering::Acquire) == 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned refresh did not retire after release"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }

    #[test]
    fn stalled_actual_refresh_holds_its_permit_and_refuses_publication_at_capacity() {
        let mut cleanup = crate::tests::CleanupStack::default();
        let directory = fixture_dir("headless-refresh-capacity");
        let owned_root = directory.clone();
        cleanup.arm("directory", move |report| {
            report.remove_dir_unless_blocked(&owned_root)
        });
        let mut settings = test_settings(&directory, full_access());
        let (release, waiting) = std::sync::mpsc::channel();
        let (started, ready) = std::sync::mpsc::channel();
        struct Release(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let cleanup_release = release.clone();
        let release = Release(Some(release));
        let waiting = Mutex::new(waiting);
        settings.refresh_access_mode = Arc::new(move |mode| {
            let _ = started.send(());
            let _ = waiting
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
            mode
        });
        let paths = vec![directory.join(STATE_FILE)];
        let host = attempt_headless_mcp_hosting(&paths, &settings).expect("host");
        let fallback = host.running.clone();
        cleanup.arm("listener", move |report| {
            if !fallback.stop() {
                report.unacknowledged("listener retirement was not acknowledged".into());
            }
        });
        let budget = settings.refresh_budget.clone();
        cleanup.arm("refresh worker", move |report| {
            let _ = cleanup_release.send(());
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while budget.load(Ordering::Acquire) != 0 && std::time::Instant::now() < deadline {
                reap_refresh_workers();
                std::thread::sleep(Duration::from_millis(5));
            }
            if budget.load(Ordering::Acquire) != 0 {
                report.unacknowledged("refresh worker still owns its fixture resources".into());
            }
            reap_refresh_workers();
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let elapsed = std::time::Instant::now();
        drop(host);
        assert!(elapsed.elapsed() < Duration::from_secs(4));
        assert!(!paths[0].exists());
        assert_eq!(settings.refresh_budget.load(Ordering::Acquire), 1);
        let permits: Vec<_> = (0..3)
            .map(|_| RefreshPermit::acquire(&settings.refresh_budget).unwrap())
            .collect();
        let refused = vec![directory.join("refused.json")];
        assert!(attempt_headless_mcp_hosting(&refused, &settings).is_none());
        assert!(!refused[0].exists(), "no listener admission at capacity");
        drop(permits);
        drop(release);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while settings.refresh_budget.load(Ordering::Acquire) != 0 {
            reap_refresh_workers();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        reap_refresh_workers();
        let permit = RefreshPermit::acquire(&settings.refresh_budget)
            .expect("released actual worker permits new admission");
        drop(permit);
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

    fn acquired_listener_identity(
        sink: &super::HostedListenerSink,
    ) -> Option<HostedListenerIdentity> {
        let owner = sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()?;
        let running = owner
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let endpoint = Url::parse(&running.endpoint()).ok()?;
        let address = std::net::SocketAddr::new(
            endpoint.host_str()?.parse().ok()?,
            endpoint.port_or_known_default()?,
        );
        Some(HostedListenerIdentity {
            address,
            instance_id: running.instance_id().to_owned(),
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
        let unexpected_child_slot: crate::tests::ChildSlot = Default::default();
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
        // Reserve the existing child-owner domain before any host/broker can
        // be acquired. Reverse cleanup transfers the unexpected original from
        // listener -> armed child owner -> runtime -> root.
        crate::tests::arm_owned_child(
            cleanup,
            "unexpected hosting broker",
            &unexpected_child_slot,
            None,
            Duration::ZERO,
        );
        {
            let (
                paths,
                runtime_slot,
                bridge_slot,
                device_slot,
                listener_sink,
                unexpected_child_slot,
            ) = (
                paths.clone(),
                runtime_slot.clone(),
                bridge_slot.clone(),
                device_slot.clone(),
                listener_sink.clone(),
                unexpected_child_slot.clone(),
            );
            cleanup.arm("listener", move |report| {
                let _owned_until_stopped = listener_guard;
                if let Some(found) = published_listener(&paths)
                    .or_else(|| acquired_listener_identity(&listener_sink))
                {
                    *cleanup_saw_listener.lock().unwrap() = Some(found.address);
                }
                stop_hosting_listener(
                    &paths,
                    &runtime_slot,
                    &bridge_slot,
                    (&device_slot, &unexpected_child_slot),
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
        device_and_child_slots: (&Slot<HeadlessDeviceAdapter>, &crate::tests::ChildSlot),
        listener_sink: &super::HostedListenerSink,
        listener_stop_acknowledged: &AtomicBool,
        report: &mut crate::tests::CleanupReport,
    ) {
        let (device_slot, unexpected_child_slot) = device_and_child_slots;
        // The production host Drop can already stop and unpublish during
        // startup unwind. Capture the actual acquisition owner, not a claim
        // that absent post-retirement descriptors mean nothing was acquired.
        let listener =
            published_listener(paths).or_else(|| acquired_listener_identity(listener_sink));
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
            (Some(bridge), Some(_)) => match bridge.host.try_lock() {
                Ok(mut host) => {
                    if let Some(host) = host.take() {
                        host.stop();
                    }
                }
                Err(_) => {
                    report.unacknowledged("the bridge hosting admission remains in use".into())
                }
            },
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
            device.stopped.store(true, Ordering::Release);
            match device.gateway.try_lock() {
                Ok(mut gateway) => {
                    if gateway.process.is_some() {
                        report.unacknowledged("a broker connection is still held".into());
                    }
                    if let Some(child) = gateway
                        .process
                        .as_mut()
                        .and_then(|process| process.take_hosted_broker_child())
                    {
                        capture_unexpected_hosting_child(child, unexpected_child_slot, report);
                    }
                    gateway.retire_current_process();
                }
                Err(_) => report.unacknowledged("the fixture device gateway remains in use".into()),
            };
        }
    }

    fn capture_unexpected_hosting_child(
        child: std::process::Child,
        armed_slot: &crate::tests::ChildSlot,
        report: &mut crate::tests::CleanupReport,
    ) {
        let pid = child.id();
        // Transfer the actual original before a diagnostic can fail/unwind.
        // The prearmed cleanup owns unknown termination in TestOwner rather
        // than dropping a local Child after a bounded observation.
        crate::tests::fill_slot(armed_slot, crate::tests::OwnedChild::new(child));
        report.unacknowledged(format!(
            "the fixture spawned a broker (pid {pid}); it is never expected to"
        ));
    }

    #[test]
    fn hosting_unwind_transfers_unexpected_child_before_runtime_and_root_cleanup() {
        let directory = fixture_dir("unexpected-hosting-child");
        let source: Slot<std::process::Child> = slot();
        let armed: crate::tests::ChildSlot = Default::default();
        let pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let child_domain = std::sync::Mutex::new(None);
        let (outcome, report) = crate::tests::run_with_cleanup(|cleanup| {
            let root = directory.clone();
            cleanup.arm("directory", move |report| {
                report.remove_dir_unless_blocked(&root)
            });
            // Actual original-owner admission precedes child acquisition.
            let domain = crate::tests::arm_owned_child(
                cleanup,
                "unexpected fixture child",
                &armed,
                None,
                Duration::ZERO,
            );
            *child_domain.lock().unwrap() = Some(domain);
            let (source, armed) = (source.clone(), armed.clone());
            let acquire = source.clone();
            cleanup.arm("listener", move |report| {
                if let Some(child) = take(&source) {
                    capture_unexpected_hosting_child(child, &armed, report);
                }
            });
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "tests::attached_root_child_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("QC_ATTACHED_ROOT_FIXTURE", &directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
            }
            // Lock the already-armed acquisition slot BEFORE spawning. No
            // observation/assertion can drop an unowned returned Child.
            let mut original = acquire
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let child = command.spawn().unwrap();
            pid.store(child.id(), Ordering::Release);
            *original = Some(child);
            drop(original);
            assert!(
                wait_for(Duration::from_secs(2), || {
                    std::fs::read_to_string(directory.join("ready"))
                        .ok()
                        .as_deref()
                        == Some(pid.load(Ordering::Acquire).to_string().as_str())
                }),
                "the exact controlled child must actually enter its fixed helper"
            );
            panic!("injected hosting assertion failure");
        });
        let message = format!("{report}");
        let domain = child_domain.into_inner().unwrap().unwrap();
        let child_retired = crate::tests::fixture_child_retired(&domain)
            && report.terminated.contains(&pid.load(Ordering::Acquire));
        // The unexpected-child diagnostic deliberately preserves bytes. Only
        // the fully acknowledged original permits the fixture's final removal.
        if child_retired {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        assert!(child_retired, "{message}");
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.contains("the fixture spawned a broker")));
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.contains("directory left in place")));
        assert!(
            outcome.is_err(),
            "the primary assertion failure is preserved"
        );
        assert!(take(&source).is_none());
        assert!(armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none());
        assert!(!directory.exists());
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
            let answers =
                fixture
                    .runtime
                    .block_on(mcp_exchange(cleanup, fixture.bridge.clone(), &reads));
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
        // Set once the cleanup under test acknowledged everything stopped.
        let stopped = AtomicBool::new(false);
        // The leak is what this test observes, and only until then: on every
        // exit, a failed assertion included, the held file is released first
        // and the directory removed after, unless that cleanup could not
        // acknowledge the listener stopped. Reports, never panics.
        let _release = crate::tests::RunOnDrop::new(|| {
            drop(
                held.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take(),
            );
            let directory = seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .directory
                .clone();
            let Some(directory) = directory else {
                return;
            };
            if !stopped.load(Ordering::Acquire) {
                eprintln!(
                    "test cleanup: termination not acknowledged; {} is left in place",
                    directory.display()
                );
                return;
            }
            let mut removal = crate::tests::CleanupReport::default();
            removal.remove_dir_unless_blocked(&directory);
            if !removal.is_clean() {
                eprintln!("test cleanup: {removal}");
            }
        });
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
        stopped.store(!report.blocked, Ordering::Release);
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
    /// the listener. Returns the startup record, the failure message,
    /// retained known-file contents and total retained file bytes before the
    /// test removes the directory.
    fn run_hosting_fixture_with_cleanup_fault(
        name: &str,
        fault: CleanupFault,
    ) -> (StartupSeen, String, Option<Vec<u8>>, u64) {
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
        let kept_bytes = std::fs::read(directory.join("bytes-to-keep")).ok();
        let retained_bytes = fixture_directory_bytes(&directory);
        (seen, message, kept_bytes, retained_bytes)
    }

    fn fixture_directory_bytes(path: &std::path::Path) -> u64 {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => std::fs::read_dir(path)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| fixture_directory_bytes(&entry.path()))
                        .sum()
                })
                .unwrap_or(0),
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        }
    }

    /// The fixture's own resources are gone (the listener no longer accepts,
    /// the runtime was shut down by the later step), a bystander was never
    /// touched; then the test removes the directory cleanup preserved.
    fn assert_reaped_then_remove(seen: &StartupSeen, bystander: &mut StdioBystander) {
        let untouched = matches!(bystander.try_wait(), Ok(None));
        bystander
            .finish()
            .expect("the original bystander acknowledged retirement");
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

    static STDIO_BYSTANDERS_IN_USE: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    struct StdioBystanderPermit;
    impl Drop for StdioBystanderPermit {
        fn drop(&mut self) {
            STDIO_BYSTANDERS_IN_USE.fetch_sub(1, Ordering::AcqRel);
        }
    }
    static RETAINED_STDIO_BYSTANDERS: Mutex<Vec<(std::process::Child, StdioBystanderPermit)>> =
        Mutex::new(Vec::new());
    struct StdioBystander(Option<std::process::Child>, Option<StdioBystanderPermit>);
    impl StdioBystander {
        fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
            self.0.as_mut().expect("owned bystander").try_wait()
        }
        fn finish(&mut self) -> Result<(), String> {
            let Some(mut child) = self.0.take() else {
                return Ok(());
            };
            if matches!(child.try_wait(), Ok(Some(_))) {
                return Ok(());
            }
            let kill_error = child.kill().err();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let pid = child.id();
            RETAINED_STDIO_BYSTANDERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((child, self.1.take().expect("owned bystander permit")));
            Err(format!("Bystander {pid} retirement unknown; exact Child retained; kill error: {kill_error:?}"))
        }
    }
    impl Drop for StdioBystander {
        fn drop(&mut self) {
            if let Err(error) = self.finish() {
                eprintln!("{error}");
            }
        }
    }
    #[test]
    #[ignore = "owned fixed bystander, invoked only by test_owned_sleeper"]
    fn stdio_bystander_child_helper() {
        std::thread::sleep(Duration::from_secs(60));
    }
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    fn test_owned_sleeper() -> StdioBystander {
        let mut retained = RETAINED_STDIO_BYSTANDERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut index = 0;
        while index < retained.len() {
            if matches!(retained[index].0.try_wait(), Ok(Some(_))) {
                retained.swap_remove(index);
            } else {
                index += 1;
            }
        }
        drop(retained);
        assert!(
            STDIO_BYSTANDERS_IN_USE
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| (count < 8)
                    .then_some(count + 1))
                .is_ok(),
            "bystander fixture capacity reached; no new process admitted"
        );
        let permit = StdioBystanderPermit;
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("owned test binary"));
        command.args([
            "--exact",
            "local_stdio::tests::stdio_bystander_child_helper",
            "--ignored",
            "--nocapture",
        ]);
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the bystander sleeper");
        StdioBystander(Some(child), Some(permit))
    }

    /// On the real hosting fixture: the listener stop is
    /// made not acknowledged by a bounded test-only injection. The directory
    /// and its bytes are preserved and reported, the primary failure is
    /// retained, no foreign process is touched, and the fixture's listener
    /// and runtime are still reaped.
    #[test]
    fn a_hosting_fixture_whose_stop_is_not_acknowledged_keeps_its_bytes() {
        let mut bystander = test_owned_sleeper();
        let (seen, message, kept_bytes, retained_bytes) = run_hosting_fixture_with_cleanup_fault(
            "hosting-stop-unacknowledged",
            CleanupFault::ListenerUnacknowledged,
        );
        // Reap all owned processes and remove the preserved fixture before any
        // assertion can panic and leak the test's bystander child.
        assert_reaped_then_remove(&seen, &mut bystander);
        assert_eq!(
            kept_bytes.as_deref(),
            Some(b"0123456789".as_slice()),
            "the known fixture bytes were preserved"
        );
        assert!(message.starts_with("injected failure"), "{message}");
        assert!(
            message.contains("listener: termination not acknowledged (injected)"),
            "{message}"
        );
        assert!(
            message.contains("termination not acknowledged; directory left in place"),
            "{message}"
        );
        assert!(
            message.contains(&format!("(bytes: {retained_bytes})")),
            "{message}"
        );
    }

    /// On the real hosting fixture: the listener teardown
    /// panics (after really stopping the listener). The panic is reported
    /// with the primary failure, the later runtime step still runs, removal
    /// is blocked so the bytes are kept, and nothing is left running.
    #[test]
    fn a_panicking_hosting_teardown_step_does_not_stop_the_later_steps() {
        let mut bystander = test_owned_sleeper();
        let (seen, message, kept_bytes, retained_bytes) = run_hosting_fixture_with_cleanup_fault(
            "hosting-teardown-panics",
            CleanupFault::ListenerPanics,
        );
        assert_reaped_then_remove(&seen, &mut bystander);
        assert_eq!(
            kept_bytes.as_deref(),
            Some(b"0123456789".as_slice()),
            "the known fixture bytes were preserved"
        );
        assert!(message.starts_with("injected failure"), "{message}");
        assert!(
            message.contains("cleanup step `listener` panicked: injected cleanup panic"),
            "{message}"
        );
        assert!(
            message.contains("termination not acknowledged; directory left in place"),
            "{message}"
        );
        assert!(
            message.contains(&format!("(bytes: {retained_bytes})")),
            "{message}"
        );
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
        let _ = listener.expect(
            "cleanup found the actual acquired listener identity even if already unpublished",
        );
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
