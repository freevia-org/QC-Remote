use crate::owner_only_file;
use async_trait::async_trait;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, HOST},
        HeaderMap, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use qc_relay_client::{AccessMode, DeviceAdapter};
use qc_remote_mcp::{BackendError, PrincipalRoute, QcBackend, QcMcp};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    net::TcpListener as StdTcpListener,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, MutexGuard, OnceLock, PoisonError,
    },
    time::Duration,
};
use subtle::ConstantTimeEq as _;
use tauri::{AppHandle, Manager};
use tokio::sync::watch;

const LOCAL_PRINCIPAL: &str = "qc-remote-local-user";
const LOCAL_DEVICE: &str = "qc-remote-windows";
const STATE_FILE_NAME: &str = "local-mcp.json";
/// Remembers whether the user turned the loopback MCP surface off. Kept beside
/// the descriptor in the app's own local data directory so a machine-wide
/// policy is never confused with this user's choice.
const ENABLED_FILE_NAME: &str = "local-mcp-enabled.json";
/// How often a running app re-stamps `publishedAtUnixMs` on every descriptor
/// copy it published.
///
/// That timestamp is the only liveness signal a descriptor carries. Readers
/// (`local-clients/bridge.cjs`, `local_stdio.rs`) treat a copy older than
/// [`DESCRIPTOR_STALE_AFTER`] as belonging to a process that is gone and refuse
/// to dial its port, which by then may belong to something else entirely.
/// Re-stamping five times inside that window means five consecutive writes have
/// to fail - a locked file, a suspended laptop, a stalled disk - before a live
/// app is mistaken for a dead one, while a genuinely dead app is still retired
/// within two and a half minutes. Everything except the timestamp stays
/// identical across a refresh, so a reader can tell a refresh from a restart.
const DESCRIPTOR_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// How long a connection may take to send a complete request head.
///
/// `axum::serve` sets no limit at all, so any local process could open
/// connections and trickle header bytes forever, each one holding a task and
/// its buffers on the listener's single thread. Every legitimate client here
/// is on this machine and sends its whole head at once.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(15);
/// Software headroom for simultaneous local HTTP connections, including
/// long-lived MCP streams. This is not a device or firmware limit.
const MAX_LOCAL_HTTP_CONNECTIONS: usize = 128;

/// Serve `router` on `listener` like `axum::serve`, but with a request-head
/// deadline and the same graceful shutdown: once `shutdown` completes no new
/// connection is accepted and the open ones are asked to finish.
pub(crate) async fn serve_loopback(
    listener: tokio::net::TcpListener,
    router: Router,
    header_read_timeout: Duration,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    serve_loopback_with_limit(
        listener,
        router,
        header_read_timeout,
        shutdown,
        MAX_LOCAL_HTTP_CONNECTIONS,
    )
    .await
}

async fn serve_loopback_with_limit(
    listener: tokio::net::TcpListener,
    router: Router,
    header_read_timeout: Duration,
    shutdown: impl std::future::Future<Output = ()>,
    max_connections: usize,
) -> std::io::Result<()> {
    use hyper_util::rt::{TokioIo, TokioTimer};
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    let mut connections = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    // A failed accept (a reset before it was taken, a
                    // momentary handle shortage) is not fatal to the listener.
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                // No response is promised before request parsing. Close only
                // this excess accepted socket; do not allocate another actor
                // or stall admission trying to write to an untrusted peer.
                while connections.try_join_next().is_some() {}
                if connections.len() >= max_connections {
                    drop(stream);
                    continue;
                }
                let service = hyper_util::service::TowerToHyperService::new(router.clone());
                let connection = graceful.watch(http.serve_connection(TokioIo::new(stream), service));
                connections.spawn(async move {
                    let _ = connection.await;
                });
            }
        }
    }
    drop(listener);
    graceful.shutdown().await;
    while connections.join_next().await.is_some() {}
    Ok(())
}
/// The reader-side half of that contract, declared here because this is the
/// only place that can change the 5x ratio it depends on. Readers apply it from
/// `descriptorRevision` 4 onwards; revision 3 and earlier stamped once at
/// startup and are exempt.
const DESCRIPTOR_STALE_AFTER: Duration = Duration::from_secs(150);

/// How long a stop waits for in-flight connections to finish before the
/// listener is dropped underneath them. An MCP streamable-HTTP session can be
/// held open indefinitely by its client, so a graceful shutdown alone would
/// keep the port - and the session's device access - alive after the user asked
/// for it to stop.
const LISTENER_SHUTDOWN_GRACE: Duration = Duration::from_millis(500);
/// Upper bound on how long a stop blocks its caller, including application
/// exit. A wedged listener must not keep the app on screen.
const LISTENER_STOP_TIMEOUT: Duration = Duration::from_millis(1_500);
const LISTENER_PUBLICATION_TIMEOUT: Duration = Duration::from_secs(5);
// One active GUI/headless surface is normal; unknown originals must not
// accumulate indefinitely on repeated stop/start. Test harnesses deliberately
// exercise many disjoint publishers concurrently under their own 64-slot cap.
static LISTENER_THREADS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(if cfg!(test) { 64 } else { 4 });
/// How long the publication guard waits for the owner of an existing
/// descriptor to answer `/health`.
///
/// A dead port refuses immediately, so this budget is spent only when
/// something really is listening - a live sibling instance, or an unrelated
/// process that inherited the recycled port. Erring long is the safe
/// direction: a probe that gave up early would report the descriptor free and
/// publish over a copy that is still serving, which is the whole failure this
/// guard exists to prevent.
const DESCRIPTOR_OWNER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const DESCRIPTOR_OWNER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// The budget for one `Test connection`. Long enough for a status call that
/// has to wait behind the app's own poll, short enough that a wedged app still
/// answers the user while they are looking at the panel.
const LOCAL_TEST_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_TEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// Recorded in [`LocalAccessStatus::last_error`] when this process refused to
/// publish because another copy of QC Remote is still serving. It is the one
/// start-up failure whose remedy is not "restart QC Remote".
const ANOTHER_COPY_IS_SERVING: &str = "Another copy of QC Remote is already serving AI apps on this PC. Close the other copy, then reopen this one to serve them from here.";
const NATIVE_BRIDGE_ACTIVITY_WINDOW: Duration = Duration::from_secs(12);
const ANTIGRAVITY_BRIDGE_CLIENT: &str = concat!("antigravity:", env!("CARGO_PKG_VERSION"));
static LOCAL_CLIENT_LAST_SEEN: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
static DIRECT_CODEX_ACTIVITY: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn record_direct_activity(activity: &mut HashMap<String, u64>, id: &str, now: u64) {
    activity.retain(|_, seen| native_bridge_is_recent(*seen, now));
    let evicted = if !activity.contains_key(id) && activity.len() >= MAX_LOCAL_HTTP_CONNECTIONS {
        activity
            .iter()
            .min_by_key(|(_, seen)| **seen)
            .map(|(id, _)| id.clone())
    } else {
        None
    };
    if let Some(oldest) = evicted {
        activity.remove(&oldest);
    }
    activity.insert(id.to_owned(), now);
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
struct DirectCodexPresence {
    delegate: Arc<dyn qc_remote_mcp::ClientPresence>,
    id: String,
    initialized: AtomicBool,
}
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
impl DirectCodexPresence {
    fn new() -> Self {
        Self {
            delegate: crate::host_presence::http_session(),
            id: format!("direct-{:032x}", rand::random::<u128>()),
            initialized: AtomicBool::new(false),
        }
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
impl qc_remote_mcp::ClientPresence for DirectCodexPresence {
    fn initialized(&self, client: &rmcp::model::Implementation) {
        self.delegate.initialized(client);
        if !self.initialized.swap(true, Ordering::AcqRel) {
            // Managed route observation, NOT proof of Codex binary identity.
            // Never used for authorization, policy or approval decisions.
            record_local_client_attach(
                concat!("codex:", env!("CARGO_PKG_VERSION")),
                unix_time_ms(),
            );
        }
        self.active();
    }
    fn active(&self) {
        self.delegate.active();
        if self.initialized.load(Ordering::Acquire) {
            let now = unix_time_ms();
            let mut activity = lock_recover(DIRECT_CODEX_ACTIVITY.get_or_init(Default::default));
            // Presentation evidence only, never authorization/session eviction.
            record_direct_activity(&mut activity, &self.id, now);
        }
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
impl Drop for DirectCodexPresence {
    fn drop(&mut self) {
        if let Some(activity) = DIRECT_CODEX_ACTIVITY.get() {
            lock_recover(activity).remove(&self.id);
        }
        // Dropping the delegate removes ONLY this logical HTTP presence row.
    }
}

fn local_client_activity_snapshot() -> HashMap<String, u64> {
    let clients = LOCAL_CLIENT_LAST_SEEN
        .get()
        .map(|map| lock_recover(map).clone())
        .unwrap_or_default();
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    let mut clients = clients;
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    if let Some(activity) = DIRECT_CODEX_ACTIVITY.get() {
        let now = unix_time_ms();
        let mut activity = lock_recover(activity);
        activity.retain(|_, seen| native_bridge_is_recent(*seen, now));
        if let Some(seen) = activity.values().copied().max() {
            clients
                .entry(concat!("codex:", env!("CARGO_PKG_VERSION")).into())
                .and_modify(|old| *old = (*old).max(seen))
                .or_insert(seen);
        }
    }
    clients
}
/// When this process began accepting heartbeats, or 0 before it did.
///
/// [`LOCAL_CLIENT_LAST_SEEN`] is per-process by design - a heartbeat is a
/// claim about now, and a remembered one would outlive the bridge that made
/// it - but that leaves a window at start-up in which a perfectly healthy
/// bridge has simply not ticked yet. Without this instant the app could not
/// tell that window from a host that is not there, so it answered every
/// question about every host with "not connected", and the ChatGPT row went
/// further and demanded a restart of a host whose bridge was running the whole
/// time. Knowing when the listener opened turns the first
/// [`NATIVE_BRIDGE_ACTIVITY_WINDOW`] into [`LocalClientLiveness::Settling`]:
/// still not connected, but not yet evidence of absence either.
static LOCAL_CLIENT_OBSERVATION_SINCE: AtomicU64 = AtomicU64::new(0);
/// Where the durable record of which hosts' bridges have run against this app
/// lives - beside the descriptor and the on/off preference, in the app's own
/// local data directory.
const CLIENT_HISTORY_FILE_NAME: &str = "local-mcp-clients.json";
/// The schema this build writes, and the only one it reads. A file carrying
/// any other version belongs to a build that knows something this one does
/// not, so it is read as an empty history rather than guessed at.
const CLIENT_HISTORY_SCHEMA_VERSION: u32 = 1;
/// A backstop on how many client ids the durable record keeps.
///
/// [`retain_current_clients`] is what actually bounds this record: it holds one
/// id per provider, so the live set is as large as the provider table and no
/// larger. This cap only catches a file that reached the disk some other way -
/// hand-edited, or written by a build whose provider list nothing here knows.
/// It is deliberately far above the provider count so that adding providers
/// cannot silently start evicting live rows; a record anywhere near it is
/// damaged, not busy.
const CLIENT_HISTORY_CAPACITY: usize = 32;
/// The floor between two durable writes for a client id already on record.
///
/// A bridge announces every five seconds, so writing on every announcement
/// would be seventeen thousand writes a day to re-record a fact that stops
/// changing after the first one. The *first* attach for a client id is written
/// immediately, because that is the one that changes what the panel says;
/// every announcement after it only refreshes a timestamp whose single job is
/// to order the eviction queue, and those can wait.
const CLIENT_HISTORY_WRITE_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Which client ids have had a bridge announce itself against this app, and
/// when one last did.
///
/// This is *history*, and it is the only part of a host's bridge record that
/// is allowed to outlive the process. Liveness deliberately is not:
/// [`LOCAL_CLIENT_OBSERVATION_SINCE`] above says why a remembered heartbeat
/// would be a lie, and that reasoning is untouched by this record. Keeping the
/// two apart is the whole point - a timestamp read from here must never reach
/// [`native_bridge_is_recent`], or a host whose bridge last ran yesterday
/// would come back green today, which is a worse failure than the one this
/// record exists to fix. [`local_client_evidence_at`] takes the two as
/// separate arguments so the split is visible at every call site.
///
/// What it buys: [`LOCAL_CLIENT_LAST_SEEN`] is emptied by every app restart.
/// Closing QC Remote also ends every host's STDIO bridge. `ever_attached` was
/// therefore false for every host on every launch, and the panel told hosts
/// that had been serving for hours that they had never connected and the user
/// should restart them. A host whose bridge has run before is idle, not
/// broken, and the row can say so now.
static LOCAL_CLIENT_ATTACH_HISTORY: OnceLock<Mutex<ClientAttachHistory>> = OnceLock::new();
const QC_ACTIONS_CONTRACT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../contracts/public/qc-actions.v1.json"
));

/// `pub(crate)` so a headless `--local-mcp-bridge` process that has become the
/// MCP-HTTP publisher itself (see `local_stdio::attempt_headless_mcp_hosting`)
/// can build one more of these to answer its own stdio session directly, in
/// process, with the exact same forbidden-route and access-mode gating the
/// HTTP router applies - rather than that bridge looping a second request back
/// through its own HTTP listener, or reimplementing this gating a second time.
#[derive(Clone)]
pub(crate) struct LocalBackend {
    pub(crate) adapter: Arc<dyn DeviceAdapter>,
    pub(crate) access: watch::Receiver<AccessMode>,
}

#[async_trait]
impl QcBackend for LocalBackend {
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
        expected_broker_target: Option<qc_remote_mcp::BrokerTarget>,
    ) -> Result<Value, BackendError> {
        if route.principal_id.as_ref() != LOCAL_PRINCIPAL
            || route.device_id.as_ref() != LOCAL_DEVICE
        {
            return Err(backend_error(
                "forbidden",
                "The local route does not belong to this QC Remote instance",
                false,
            ));
        }
        let access = self
            .adapter
            .effective_access_mode()
            .unwrap_or(*self.access.borrow());
        if !local_access_permits(access, rpc) {
            return Err(backend_error(
                "forbidden",
                "The selected assistant access level does not permit this action",
                false,
            ));
        }
        self.adapter
            .invoke_targeted(
                rpc,
                Value::Object(params),
                expected_broker_target,
                qc_remote_mcp::serving_resource_read(),
            )
            .await
            .map_err(|error| {
                let mut mapped = backend_error(
                    if error.retryable {
                        "device_unavailable"
                    } else {
                        "device_error"
                    },
                    error.message,
                    error.retryable,
                );
                mapped.details = error
                    .details
                    .as_deref()
                    .and_then(|details| safe_device_error_details(&error.code, details))
                    .map(Box::new);
                mapped.detail_code = mapped
                    .details
                    .as_ref()
                    .and_then(|details| details.get("code"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| match error.code.as_str() {
                        "WRITE_QUARANTINED" | "WRITE_OUTCOME_AMBIGUOUS" => Some(error.code.clone()),
                        _ => None,
                    });
                mapped
            })
    }
}

fn local_access_permits(access: AccessMode, rpc: &str) -> bool {
    // MCP resources use internal read projections rather than public action
    // RPCs. They are intentionally absent from the generated action policy,
    // but must still be available at every access tier just like the public
    // read actions they mirror.
    matches!(rpc, "system.status" | "device.snapshot") || access.permits(rpc)
}

fn backend_error(code: &'static str, message: impl Into<String>, retryable: bool) -> BackendError {
    BackendError {
        code,
        message: message.into(),
        retryable,
        detail_code: None,
        details: None,
    }
}

/// Device error codes are sent by the paired host. Keep only the compact token
/// form used by the public device protocol before exposing one to local MCP
/// clients; the transport class remains in `BackendError::code`.
fn safe_device_error_code(code: &str) -> Option<&str> {
    (!code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'))
    .then_some(code)
}

fn safe_device_error_details(wire_code: &str, details: &Value) -> Option<Value> {
    let source = details.as_object()?;
    let mut filtered = Map::new();
    if let Some(code) = source
        .get("code")
        .and_then(Value::as_str)
        .and_then(safe_device_error_code)
    {
        filtered.insert("code".into(), Value::String(code.to_owned()));
    }
    for field in ["retryable", "requestSent"] {
        if let Some(value) = source.get(field).and_then(Value::as_bool) {
            filtered.insert(field.into(), Value::Bool(value));
        }
    }
    let dispatch_code = if wire_code == "WRITE_QUARANTINED" {
        Some(wire_code)
    } else {
        source
            .get("code")
            .and_then(Value::as_str)
            .or(Some(wire_code))
    };
    qc_relay_client::project_dispatch_error_details(dispatch_code, source, &mut filtered);
    (!filtered.is_empty()).then_some(Value::Object(filtered))
}

#[derive(Clone)]
struct LocalAuth {
    token: Arc<str>,
    allowed_hosts: Arc<[String]>,
    body_timeout: Duration,
}

#[derive(Clone)]
struct LocalHealth {
    adapter: Arc<dyn DeviceAdapter>,
    instance_id: Arc<str>,
    tool_contract_sha256: Arc<str>,
    process_id: u32,
}

#[derive(Clone)]
struct LocalRpc {
    backend: Arc<dyn QcBackend>,
    endpoint: String,
    instance_id: Arc<str>,
    broker_descriptor: Option<PathBuf>,
}

/// The header a native stdio bridge puts on a `/rpc` request that serves an
/// MCP resource read it relays, with [`RESOURCE_READ_ORIGIN`] as its value.
/// The body cannot say it: `qc://models` and the `list_models` tool send the
/// same `{rpc, params}`. It is honoured only behind the bearer check, only for
/// a method a resource uses ([`qc_remote_mcp::resource_rpcs`]), and it can
/// only narrow what the request may do: the host then uses the broker
/// connection it holds or answers offline, as for its own resource reads. A
/// request without it is handled as before.
pub(crate) const REQUEST_ORIGIN_HEADER: &str = "x-qc-remote-request-origin";
pub(crate) const RESOURCE_READ_ORIGIN: &str = "resource-read";

/// Whether this `/rpc` request serves a relayed MCP resource read.
fn relays_resource_read(headers: &HeaderMap, rpc: &str) -> bool {
    headers
        .get(REQUEST_ORIGIN_HEADER)
        .is_some_and(|origin| origin.as_bytes() == RESOURCE_READ_ORIGIN.as_bytes())
        && qc_remote_mcp::resource_rpcs()
            .iter()
            .any(|(_, resource_rpc)| *resource_rpc == rpc)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalRpcRequest {
    rpc: String,
    #[serde(default)]
    params: Map<String, Value>,
    expected_broker_target: Option<qc_remote_mcp::BrokerTarget>,
}

#[cfg(test)]
fn local_router(
    backend: Arc<dyn QcBackend>,
    health: LocalHealth,
    token: Arc<str>,
    allowed_hosts: Vec<String>,
) -> Router {
    local_router_with_body_timeout(
        backend,
        health,
        token,
        allowed_hosts,
        Duration::from_secs(15),
    )
}

#[cfg(test)]
fn local_router_with_body_timeout(
    backend: Arc<dyn QcBackend>,
    health: LocalHealth,
    token: Arc<str>,
    allowed_hosts: Vec<String>,
    body_timeout: Duration,
) -> Router {
    local_router_with_lifecycle(backend, health, token, allowed_hosts, body_timeout, None)
}

fn local_router_with_lifecycle(
    backend: Arc<dyn QcBackend>,
    health: LocalHealth,
    token: Arc<str>,
    allowed_hosts: Vec<String>,
    body_timeout: Duration,
    broker_descriptor: Option<PathBuf>,
) -> Router {
    let auth = LocalAuth {
        token,
        allowed_hosts: allowed_hosts.clone().into(),
        body_timeout,
    };
    let rpc_backend = backend.clone();
    let endpoint = format!("http://{}/mcp", allowed_hosts[0]);
    let instance_id = health.instance_id.clone();
    let identity = LocalIdentity {
        token: auth.token.clone(),
        instance_id: health.instance_id.clone(),
        allowed_hosts: auth.allowed_hosts.clone(),
        body_timeout,
    };
    qc_remote_mcp::mcp_router_with_allowed_hosts(
        move || {
            Ok(QcMcp::new(backend.clone())
                .with_client_presence(crate::host_presence::http_session()))
        },
        allowed_hosts,
    )
    .route("/health", get(local_health).with_state(health))
    .route(
        "/rpc",
        post(local_rpc).with_state(LocalRpc {
            backend: rpc_backend,
            endpoint,
            instance_id,
            broker_descriptor,
        }),
    )
    .route_layer(middleware::from_fn_with_state(auth, authenticate_local))
    // Outside the bearer check on purpose: a client asks this before it will
    // send the token at all. It reveals nothing but a proof for the caller's
    // own nonce.
    .route(
        crate::local_identity::IDENTITY_PATH,
        post(local_identity).with_state(identity),
    )
}

/// A separate credential audience. Never mount legacy RPC/health here.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub(crate) fn direct_http_router(
    backend: Arc<dyn QcBackend>,
    token: Arc<str>,
    instance_id: Arc<str>,
    port: u16,
) -> Router {
    let hosts = vec![format!("127.0.0.1:{port}")];
    let auth = LocalAuth {
        token: token.clone(),
        allowed_hosts: hosts.clone().into(),
        body_timeout: Duration::from_secs(15),
    };
    let identity = LocalIdentity {
        token,
        instance_id,
        allowed_hosts: hosts.clone().into(),
        body_timeout: Duration::from_secs(3),
    };
    qc_remote_mcp::mcp_router_with_allowed_hosts(
        move || {
            Ok(QcMcp::new(backend.clone())
                .with_session_target_guard()
                .with_client_presence(Arc::new(DirectCodexPresence::new())))
        },
        hosts,
    )
    .route_layer(middleware::from_fn_with_state(auth, authenticate_local))
    .route(
        crate::local_identity::IDENTITY_PATH,
        post(local_identity).with_state(identity),
    )
    .layer(middleware::from_fn_with_state(
        Arc::new(Mutex::new((std::time::Instant::now(), 0_u32))),
        direct_envelope,
    ))
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
async fn direct_envelope(
    State(rate): State<Arc<Mutex<(std::time::Instant, u32)>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.version() != axum::http::Version::HTTP_11
        || request.uri().scheme().is_some()
        || request.uri().authority().is_some()
    {
        return local_error(
            StatusCode::BAD_REQUEST,
            "LOCAL_INVALID_REQUEST",
            "origin_form_http11_required",
        );
    }
    if request.uri().path() == crate::local_identity::IDENTITY_PATH {
        let limited = {
            let mut window = lock_recover(&rate);
            if window.0.elapsed() >= Duration::from_secs(1) {
                *window = (std::time::Instant::now(), 0);
            }
            window.1 = window.1.saturating_add(1);
            window.1 > 32
        };
        if limited {
            return local_error(
                StatusCode::TOO_MANY_REQUESTS,
                "LOCAL_IDENTITY_LIMIT",
                "identity_rate_limited",
            );
        }
    }
    next.run(request).await
}

#[derive(Clone)]
struct LocalIdentity {
    token: Arc<str>,
    instance_id: Arc<str>,
    allowed_hosts: Arc<[String]>,
    body_timeout: Duration,
}

// These match the existing consumers: rmcp3.4.1 POST4MiB,
// Axum JSON2MiB, and the identity challenge1KiB. They are software
// request-envelope limits, not device/firmware data limits.
const LOCAL_MCP_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const LOCAL_RPC_REQUEST_BYTES: usize = 2 * 1024 * 1024;
async fn read_local_request_body(
    mut body: Body,
    limit: usize,
    within: Duration,
) -> Result<axum::body::Bytes, Box<Response>> {
    use hyper::body::Body as _;
    let collect = async move {
        let mut bytes = Vec::new();
        while let Some(frame) =
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
        {
            let frame = frame.map_err(|_| {
                Box::new(local_error(
                    StatusCode::BAD_REQUEST,
                    "LOCAL_INVALID_REQUEST",
                    "invalid_request",
                ))
            })?;
            if let Ok(data) = frame.into_data() {
                if data.len() > limit.saturating_sub(bytes.len()) {
                    return Err(Box::new(local_error(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "LOCAL_REQUEST_TOO_LARGE",
                        "request_too_large",
                    )));
                }
                bytes.extend_from_slice(&data);
            }
        }
        Ok(axum::body::Bytes::from(bytes))
    };
    tokio::time::timeout(within, collect)
        .await
        .unwrap_or_else(|_| {
            Err(Box::new(local_error(
                StatusCode::REQUEST_TIMEOUT,
                "LOCAL_REQUEST_TIMEOUT",
                "request_body_timeout",
            )))
        })
}

async fn local_identity(State(state): State<LocalIdentity>, request: Request<Body>) -> Response {
    if request.headers().contains_key("origin") {
        return local_error(
            StatusCode::FORBIDDEN,
            "LOCAL_FORBIDDEN_ORIGIN",
            "browser_origin_forbidden",
        );
    }
    let presented_host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok());
    if !presented_host.is_some_and(|host| state.allowed_hosts.iter().any(|allowed| allowed == host))
    {
        return local_error(
            StatusCode::BAD_REQUEST,
            "LOCAL_INVALID_HOST",
            "invalid_host",
        );
    }
    let body = match read_local_request_body(request.into_body(), 1024, state.body_timeout).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let proof = serde_json::from_slice::<crate::local_identity::IdentityChallenge>(&body)
        .ok()
        .and_then(|challenge| {
            crate::local_identity::prove(&state.token, &state.instance_id, &challenge.nonce)
        });
    match proof {
        Some(proof) => Json(crate::local_identity::IdentityProof {
            instance_id: state.instance_id.to_string(),
            proof,
        })
        .into_response(),
        None => local_error(
            StatusCode::BAD_REQUEST,
            "LOCAL_INVALID_REQUEST",
            "invalid_request",
        ),
    }
}

/// Every failure on every local route answers with the same JSON object, so a
/// client never has to guess which of three shapes it is holding.
///
/// `browser_origin_forbidden`, `invalid_host` and `unauthorized` used to be
/// returned as bare `text/plain` strings. Both bridges parse a failure body as
/// JSON, fail, and fall back to the health endpoint for a reason - so a wrong
/// bearer token, a DNS-rebinding attempt and a browser-issued request were all
/// reported to the user as "the Quad Cortex is not connected", which is the one
/// thing they do not mean.
///
/// The complete set of `code`/`reason` pairs a local client can receive:
///
/// | status | `code`                   | `reason`                   | meaning                                               |
/// |--------|--------------------------|----------------------------|-------------------------------------------------------|
/// | 403    | `LOCAL_FORBIDDEN_ORIGIN` | `browser_origin_forbidden` | the request carried an `Origin`; a page tried to dial |
/// | 400    | `LOCAL_INVALID_HOST`     | `invalid_host`             | `Host` is not the exact loopback host published       |
/// | 401    | `LOCAL_UNAUTHORIZED`     | `unauthorized`             | missing or wrong bearer token; re-read the descriptor |
/// | 503    | `LOCAL_UNAVAILABLE`      | `device_not_ready`         | `/health`: the app is up, the Quad Cortex is not      |
/// | 400    | `LOCAL_UNKNOWN_ACTION`   | `unknown_action`           | `/rpc`: no such action in the tool contract           |
/// | 403    | `LOCAL_FORBIDDEN`        | `forbidden`                | `/rpc`: the selected access mode forbids this action  |
/// | 503    | `LOCAL_UNAVAILABLE`      | `device_unavailable`       | `/rpc`: the device dropped out mid-call; retryable    |
/// | 422    | `LOCAL_DEVICE_ERROR`     | `device_error`             | `/rpc`: the device refused the action                 |
///
/// `/rpc` keeps its `{ok, error: {code, message, retryable}}` envelope intact
/// for the native stdio bridge. `deviceCode` is additive and preserves the
/// adapter's domain code without changing the transport classification.
fn local_error(status: StatusCode, code: &'static str, reason: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({ "code": code, "reason": reason })),
    )
        .into_response()
}

/// The status and typed code for a backend failure, keeping `error.code` as the
/// `reason` so the two halves of the body never disagree.
fn local_rpc_failure(code: &str) -> (StatusCode, &'static str) {
    match code {
        "forbidden" => (StatusCode::FORBIDDEN, "LOCAL_FORBIDDEN"),
        "device_unavailable" => (StatusCode::SERVICE_UNAVAILABLE, "LOCAL_UNAVAILABLE"),
        _ => (StatusCode::UNPROCESSABLE_ENTITY, "LOCAL_DEVICE_ERROR"),
    }
}

async fn local_health(State(state): State<LocalHealth>) -> Response {
    let device_ready = state.adapter.ready().await;
    let status = if device_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let mut document = serde_json::json!({
        "schemaVersion": 1,
        "instanceId": state.instance_id.as_ref(),
        "appReady": true,
        "deviceReady": device_ready,
        "toolContractSha256": state.tool_contract_sha256.as_ref(),
        "processId": state.process_id,
        "capabilities": ["localRpcBrokerTargetGuard"],
    });
    if !device_ready {
        document["code"] = Value::String("LOCAL_UNAVAILABLE".into());
        document["reason"] = Value::String("device_not_ready".into());
    }
    // Reported here because this is the surface that still answers when the
    // runtime health record cannot be written: that record can only carry its
    // own failure once writing resumes, and a failure that never clears would
    // otherwise leave a frozen file and no way to tell it from a current one.
    if let Some(failure) = crate::last_runtime_health_failure() {
        document["runtimeHealthWriteFailure"] = Value::String(failure);
    }
    (status, Json(document)).into_response()
}

/// Stateless authenticated hop used only by QC Remote's own stdio launcher.
/// Keeping MCP framing in the launcher lets ChatGPT/Codex run without Node,
/// while this endpoint still enforces the app's selected access mode and keeps
/// the desktop process as the sole owner of the device session.
async fn local_rpc(
    State(state): State<LocalRpc>,
    headers: HeaderMap,
    Json(request): Json<LocalRpcRequest>,
) -> Response {
    let observed_at = unix_time_ms();
    if request.rpc == "host.status" {
        let result = host_status_document(
            Some(state.endpoint.clone()),
            Some(state.instance_id.as_ref()),
        );
        return Json(serde_json::json!({"ok":true,"result":result})).into_response();
    }
    if request.rpc == "local.bridgeClosed" {
        if let Some(id) = request.params.get("sessionId").and_then(Value::as_str) {
            crate::host_presence::bridge_closed(id);
        }
        return Json(serde_json::json!({"ok":true,"result":{}})).into_response();
    }
    if request.rpc == "local.bridgeLifecycle" {
        // Passive and authenticated: no adapter call, host election or USB.
        let lifecycle = state
            .broker_descriptor
            .as_deref()
            .map(crate::local_stdio::broker_lifecycle_hint);
        return Json(serde_json::json!({"ok":true,"result":{"brokerLifecycle":lifecycle}}))
            .into_response();
    }
    if request.rpc == "local.bridgeHeartbeat" {
        let lifecycle = state
            .broker_descriptor
            .as_deref()
            .map(crate::local_stdio::broker_lifecycle_hint);
        if let (Some(id), Some(name)) = (
            request.params.get("sessionId").and_then(Value::as_str),
            request.params.get("clientName").and_then(Value::as_str),
        ) {
            // Optional: a bridge older than the field sends none.
            let version = request.params.get("clientVersion").and_then(Value::as_str);
            crate::host_presence::bridge_heartbeat(id, name, version, observed_at);
        }
        if let Some(client) = request.params.get("client").and_then(Value::as_str) {
            if !client.is_empty() && client.len() <= 128 {
                let clients = LOCAL_CLIENT_LAST_SEEN.get_or_init(|| Mutex::new(HashMap::new()));
                if let Ok(mut clients) = clients.lock() {
                    clients.retain(|_, last_seen| {
                        observed_at.saturating_sub(*last_seen)
                            <= Duration::from_secs(24 * 60 * 60).as_millis() as u64
                    });
                    if clients.len() >= 64 && !clients.contains_key(client) {
                        if let Some(oldest) = clients
                            .iter()
                            .min_by_key(|(_, last_seen)| **last_seen)
                            .map(|(client, _)| client.clone())
                        {
                            clients.remove(&oldest);
                        }
                    }
                    clients.insert(client.to_string(), observed_at);
                }
                // The same announcement, recorded in the record that outlives
                // this process. Throttled and written off-thread, so an
                // announcement is never held up by a disk.
                record_local_client_attach(client, observed_at);
            }
        }
        // A newly launched bridge may be serving offline advice after Quit.
        // It still announced an initialized MCP session and belongs in the
        // client list; this does not authorize broker startup or device I/O.
        let connected = !lifecycle.as_ref().is_some_and(|hint| hint.user_quit);
        let mut result = serde_json::json!({"connected":connected});
        if let Some(lifecycle) = lifecycle {
            result["brokerLifecycle"] = serde_json::json!(lifecycle);
        }
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "result": result,
            })),
        )
            .into_response();
    }
    let rpc = match request.rpc.as_str() {
        // These are the MCP resources served alongside the generated tools.
        "system.status" => Some("system.status"),
        "device.snapshot" => Some("device.snapshot"),
        rpc => qc_remote_mcp::ACTIONS
            .iter()
            .find(|action| action.rpc == rpc)
            .map(|action| action.rpc),
    };
    let Some(rpc) = rpc else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "code": "LOCAL_UNKNOWN_ACTION",
                "reason": "unknown_action",
                "error": {
                    "code": "unknown_action",
                    "message": "QC Remote does not recognize this local action.",
                    "retryable": false,
                }
            })),
        )
            .into_response();
    };
    // A tool call from a native bridge names its session. When the tray had
    // unlinked that session, this new request links it again; otherwise it
    // changes nothing. It never delays or refuses the call, and it relinks
    // whatever the call's outcome. MCP resource reads do not relink: the
    // bridge sends no session on them, and a method only resources use is
    // ignored here too.
    if relinks_session(rpc) {
        if let Some(session) = headers
            .get(crate::host_presence::BRIDGE_SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
        {
            crate::host_presence::bridge_request(session, observed_at);
        }
    }
    let route = PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE);
    let answered =
        state
            .backend
            .request_targeted(&route, rpc, request.params, request.expected_broker_target);
    // A relayed resource read is served as this host serves its own: inside
    // the same task-local scope `QcMcp::read_resource` enters, so its gateway
    // never attaches, spawns or recovers a broker for it.
    let answered = if relays_resource_read(&headers, rpc) {
        qc_remote_mcp::as_resource_read(answered).await
    } else {
        answered.await
    };
    match answered {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "result": result,
            })),
        )
            .into_response(),
        Err(error) => {
            let (status, code) = local_rpc_failure(error.code);
            (
                status,
                Json(serde_json::json!({
                    "ok": false,
                    "code": code,
                    "reason": error.code,
                    "error": {
                        "code": error.code,
                        "deviceCode": error.detail_code,
                        "details": error.details,
                        "message": error.message,
                        "retryable": error.retryable,
                    }
                })),
            )
                .into_response()
        }
    }
}

/// Whether a request for `rpc` that names a bridge session may relink it: only
/// a tool's method can. A method no tool serves (`system.status`, the internal
/// `device.snapshot` projection) never relinks. A method that is both a
/// resource and a tool (`device.currentPreset`, `device.listModels`; see
/// [`qc_remote_mcp::resource_rpcs`]) is told apart by origin in the bridge,
/// which omits the session while serving a resource read.
fn relinks_session(rpc: &str) -> bool {
    qc_remote_mcp::ACTIONS
        .iter()
        .any(|action| action.rpc == rpc)
}

pub(crate) fn host_status_document(endpoint: Option<String>, instance_id: Option<&str>) -> Value {
    let mut presence = crate::host_presence::snapshot(endpoint);
    // The linked remote apps and each live session's place among them.
    crate::remote_grants::annotate(&mut presence);
    let mut result = serde_json::to_value(presence).unwrap_or_default();
    result["instanceId"] = serde_json::json!(instance_id);
    result["remoteState"] = serde_json::json!(crate::host_presence::remote_state());
    result["bridgeClients"] = serde_json::json!(local_client_activity_snapshot());
    result["observedSince"] =
        serde_json::json!(LOCAL_CLIENT_OBSERVATION_SINCE.load(Ordering::Acquire));
    result
}

async fn authenticate_local(
    State(state): State<LocalAuth>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if request.headers().contains_key("origin") {
        return local_error(
            StatusCode::FORBIDDEN,
            "LOCAL_FORBIDDEN_ORIGIN",
            "browser_origin_forbidden",
        );
    }
    let presented_host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok());
    if !presented_host.is_some_and(|host| state.allowed_hosts.iter().any(|allowed| allowed == host))
    {
        return local_error(
            StatusCode::BAD_REQUEST,
            "LOCAL_INVALID_HOST",
            "invalid_host",
        );
    }
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let authorized =
        presented.is_some_and(|value| bool::from(value.as_bytes().ct_eq(state.token.as_bytes())));
    if !authorized {
        return local_error(
            StatusCode::UNAUTHORIZED,
            "LOCAL_UNAUTHORIZED",
            "unauthorized",
        );
    }
    request
        .extensions_mut()
        .insert(PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE));
    // Admission is bounded before a handler/device call begins. Do not put a
    // timeout around Next: legitimate tools and SSE responses may be long-lived.
    let limit = if request.uri().path() == "/mcp" {
        LOCAL_MCP_REQUEST_BYTES
    } else {
        LOCAL_RPC_REQUEST_BYTES
    };
    let (parts, body) = request.into_parts();
    let body = match read_local_request_body(body, limit, state.body_timeout).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let request = Request::from_parts(parts, Body::from(body));
    next.run(request).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalEndpointDocument<'a> {
    schema_version: u8,
    descriptor_revision: u8,
    transport: &'static str,
    url: &'a str,
    health_url: &'a str,
    rpc_url: &'a str,
    bearer_token: &'a str,
    process_id: u32,
    instance_id: &'a str,
    published_at_unix_ms: u64,
    tool_contract_sha256: &'a str,
    /// Where a client checks this listener holds `bearer_token` before it
    /// sends it (see `local_identity`). Older readers ignore the field.
    identity_url: &'a str,
}

/// Everything a descriptor copy carries except its timestamp, held together so
/// a refresh is a re-stamp of the same values and never a new publication. A
/// reader that saw the token, port or instance id move would be right to treat
/// it as a restart and drop whatever session it had.
#[derive(Clone)]
struct LocalDescriptor {
    url: String,
    health_url: String,
    rpc_url: String,
    bearer_token: String,
    instance_id: String,
    tool_contract_sha256: String,
}

impl LocalDescriptor {
    /// The identity endpoint beside `rpc_url`.
    fn identity_url(&self) -> String {
        let base = self.rpc_url.strip_suffix("/rpc").unwrap_or(&self.rpc_url);
        format!("{base}{}", crate::local_identity::IDENTITY_PATH)
    }

    fn publish(&self, path: &Path) -> Result<(), String> {
        self.publish_at(path, unix_time_ms())
    }

    fn publish_at(&self, path: &Path, published_at_unix_ms: u64) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or("The local MCP state path has no parent directory")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the local MCP state directory: {error}"))?;
        let identity_url = self.identity_url();
        let document = LocalEndpointDocument {
            schema_version: 1,
            // Revision 4 is the first that re-stamps `publishedAtUnixMs` while
            // the app runs. Readers apply the staleness rule from 4 onwards and
            // exempt earlier revisions, which stamped once at startup.
            descriptor_revision: 4,
            transport: "streamable-http",
            url: &self.url,
            health_url: &self.health_url,
            rpc_url: &self.rpc_url,
            bearer_token: &self.bearer_token,
            process_id: std::process::id(),
            instance_id: &self.instance_id,
            published_at_unix_ms,
            tool_contract_sha256: &self.tool_contract_sha256,
            identity_url: &identity_url,
        };
        let encoded = serde_json::to_vec(&document)
            .map_err(|error| format!("Could not encode the local MCP endpoint: {error}"))?;
        publish_atomically(path, parent, "the local MCP endpoint", &encoded)
    }
}

/// Publication is all-or-nothing.
///
/// Publishing path by path and returning on the first failure left a
/// valid-looking descriptor advertising a port whose listener was dropped on
/// the very next line, while `start` had already reported success. The
/// mirror-image branch was no better: it removed every copy, including one a
/// *live* instance had published to the shared Claude mirror. So only the
/// copies this call actually wrote are removed on failure, and the error is
/// returned rather than logged.
fn publish_all_or_nothing(descriptor: &LocalDescriptor, paths: &[PathBuf]) -> Result<(), String> {
    let mut published: Vec<&PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        if let Err(error) = descriptor.publish(path) {
            for published in published {
                let _ = fs::remove_file(published);
            }
            return Err(error);
        }
        published.push(path);
    }
    Ok(())
}

/// Why the loopback listener did not start, and whether the reason was that
/// another live copy of QC Remote already owns the published descriptor.
///
/// Typed rather than sniffed back out of the message, because the LOCAL panel
/// has to say something different for it: every other start-up failure ends in
/// "restart QC Remote", and restarting is precisely the wrong advice when a
/// second copy is the problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAccessFailure {
    pub message: String,
    pub served_by_another_instance: bool,
}

impl LocalAccessFailure {
    fn other(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            served_by_another_instance: false,
        }
    }
}

impl From<String> for LocalAccessFailure {
    fn from(message: String) -> Self {
        Self::other(message)
    }
}

/// The live QC Remote that already owns the descriptor on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DescriptorOwner {
    process_id: u32,
    instance_id: String,
}

/// Only the loopback health address this app itself publishes, checked before
/// it is dialled so a doctored descriptor cannot turn the guard into a fetcher.
fn validated_health_url(raw: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != "/health"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

/// Whether a QC Remote that is still alive owns the descriptor already on disk.
///
/// A second copy of the app used to bind its own port, mint its own token and
/// overwrite all three descriptor copies unconditionally. The first copy kept
/// serving on a port no file named any more, every paired client silently
/// migrated to the second, and two gateways raced for a USB broker that is a
/// per-machine singleton.
///
/// The proof of ownership is deliberately not a process-id comparison. Windows
/// reuses process ids, so a fresh descriptor naming a live pid is no evidence
/// that the live process is QC Remote - and the mirror image, a crashed
/// instance whose descriptor is still inside the 150s refresh window, would
/// otherwise lock its own replacement out for two and a half minutes. Instead
/// the descriptor's own `/health` address is dialled with the credential it
/// publishes, and ownership is granted only when the answer carries the same
/// `instanceId` the descriptor does. That identifier is 128 random bits minted
/// per listener, so a matching answer can only come from the process that
/// wrote the file; a recycled port, a restarted instance and an unrelated
/// listener all fail to match, and the descriptor is overwritten as before.
/// `processId` is reported alongside it for the console line, never as the
/// test.
///
/// Everything else stays overwritable: an absent, malformed or no-longer
/// refreshed descriptor never reaches the probe, so a stale file can never
/// lock a fresh start out permanently.
async fn live_descriptor_owner(paths: &[PathBuf], now_ms: u64) -> Option<DescriptorOwner> {
    // The bridges' reader, not a fifth one: same path handling, same
    // newest-wins choice, same staleness rule. An absent, unreadable, invalid
    // or expired descriptor is an error here, which is exactly "free".
    let existing = crate::local_stdio::descriptor_from_paths_at(paths, now_ms).ok()?;
    if existing.descriptor_revision < crate::local_stdio::DESCRIPTOR_REFRESH_REVISION {
        // Releases through 0.1.140 stamped `publishedAtUnixMs` once at startup,
        // so their descriptors carry no liveness signal to read. Holding back
        // for one would be indefinite; it is overwritten, as it is today.
        return None;
    }
    if existing.instance_id.is_empty() {
        return None;
    }
    let health_url = validated_health_url(&existing.health_url)?;
    let client = crate::loopback_http::loopback_client_builder()
        .timeout(DESCRIPTOR_OWNER_PROBE_TIMEOUT)
        .connect_timeout(DESCRIPTOR_OWNER_CONNECT_TIMEOUT)
        .build()
        .ok()?;
    // A squatter on a dead copy's port is not a live owner, and gets no token.
    existing.prove_identity(&client).await.ok()?;
    let response = client
        .get(health_url)
        .bearer_auth(&existing.bearer_token)
        .send()
        .await
        .ok()?;
    // `/health` answers 503 with the same identity document when the Quad
    // Cortex is unplugged. A copy serving without a device is still serving.
    let document = qc_relay_client::bounded_response_json::<Value>(response)
        .await
        .ok()?;
    if document.get("instanceId").and_then(Value::as_str) != Some(existing.instance_id.as_str()) {
        return None;
    }
    Some(DescriptorOwner {
        process_id: document
            .get("processId")
            .and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .unwrap_or(existing.process_id),
        instance_id: existing.instance_id,
    })
}

fn remove_published_descriptors(paths: &[PathBuf], instance_id: &str) {
    for path in paths {
        let owned = crate::local_stdio::read_local_document(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|document| document["instanceId"] == instance_id);
        if owned {
            let _ = fs::remove_file(path);
        }
    }
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

/// Runs `refresh` every `interval` until shutdown is signalled, and never
/// again afterwards: a tick that fired during shutdown would republish a
/// descriptor the stop path has already removed.
async fn refresh_until_shutdown(
    interval: Duration,
    shutdown: &mut watch::Receiver<bool>,
    mut refresh: impl FnMut(),
) {
    loop {
        tokio::select! {
            biased;
            () = wait_for_shutdown(shutdown) => return,
            () = tokio::time::sleep(interval) => refresh(),
        }
    }
}

/// The descriptor paths this process publishes to.
///
/// The first is the app's own local data directory. The second is the shared
/// Claude mirror, which exists for a reason of its own: restricted extension
/// hosts can launch a client without `LOCALAPPDATA` while still running as the
/// ordinary interactive user, so a copy is placed where they can reach it.
fn state_file_paths(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths::prepare_directory(&directory)?;
        Ok(vec![directory.join(STATE_FILE_NAME)])
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&directory)?;
        Ok(vec![directory.join(STATE_FILE_NAME)])
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let app_local_data_dir = app
            .path()
            .app_local_data_dir()
            .map_err(|error| format!("Could not resolve the local access directory: {error}"))?;
        Ok(descriptor_paths(
            &app_local_data_dir,
            std::env::var_os("APPDATA").map(PathBuf::from),
        ))
    }
}

/// The durable directory shared by the GUI and resident/MCP processes.
/// Unix broker descriptors and preferences must use the broker's protected
/// native directory, not a second app-specific location.
fn app_local_data_directory(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths::prepare_directory(&directory)?;
        Ok(directory)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let directory = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&directory)?;
        Ok(directory)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        app.path()
            .app_local_data_dir()
            .map_err(|error| format!("Could not resolve the local access directory: {error}"))
    }
}

/// The publication set, resolved from the two directories that decide it, so a
/// test can assert what this app writes a bearer token into without standing
/// up a Tauri application handle.
pub(crate) fn descriptor_paths(
    app_local_data_dir: &Path,
    roaming: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut state_files = vec![app_local_data_dir.join(STATE_FILE_NAME)];
    if let Some(claude_state_file) = roaming.map(|directory| {
        directory
            .join("Claude")
            .join("qc-remote")
            .join(STATE_FILE_NAME)
    }) {
        if !state_files.contains(&claude_state_file) {
            state_files.push(claude_state_file);
        }
    }
    state_files
}

/// A listener that is bound, published and serving. Dropping the owning
/// [`LocalAccess`] is no longer the only way to stop one: `stop` is what both
/// the off switch and application exit call.
pub(crate) struct RunningLocalAccess {
    state_files: Vec<PathBuf>,
    shutdown: watch::Sender<bool>,
    /// Signalled by the listener thread once it has stopped serving and removed
    /// its own descriptors, so a stop can be ordered rather than hopeful.
    stopped: Mutex<mpsc::Receiver<()>>,
    stop_acknowledged: AtomicBool,
    port: u16,
    instance_id: String,
    /// The loopback MCP listener deliberately has its own small Tokio runtime.
    /// Starting it on Tauri's shared runtime made publication race the rest of
    /// application startup on some Windows machines: the descriptor appeared
    /// first, while the port was not accepting connections yet.  A dedicated
    /// thread lets us publish only after the listener belongs to a running IO
    /// runtime and keeps local integrations independent of USB/broker startup.
    _task: Mutex<Option<qc_native_platform::thread_owner::Owned>>,
}

impl RunningLocalAccess {
    pub(crate) fn native_finished(&self) -> bool {
        lock_recover(&self._task)
            .as_ref()
            .is_none_or(|task| task.native_finished())
    }
    /// Consume this exact original. Only the owned background thread uses
    /// an unbounded wait here; its external caller retains a bounded stop and
    /// the background lease/admission until that thread actually retires.
    pub(crate) fn join_original(mut self) -> std::thread::Result<()> {
        let task = self
            ._task
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match task {
            Some(task) => task.join_original(),
            None => Ok(()),
        }
    }
    pub(crate) fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
    /// Stops serving, cancels whatever is still connected, and removes the
    /// descriptors this process published - in that order, so a refresh tick
    /// cannot resurrect a file after it is gone.
    ///
    /// Bounded, because this also runs on the application exit path.
    pub(crate) fn stop(&self) -> bool {
        let deadline = std::time::Instant::now() + LISTENER_STOP_TIMEOUT;
        let _ = self.shutdown.send(true);
        let acknowledged = if self.stop_acknowledged.load(Ordering::Acquire) {
            true
        } else {
            self.stopped
                .lock()
                .map(|stopped| {
                    stopped
                        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                        .is_ok()
                })
                .unwrap_or(false)
        };
        let joined = lock_recover(&self._task).as_mut().is_none_or(|task| {
            task.wait_within(deadline.saturating_duration_since(std::time::Instant::now()))
        });
        if acknowledged {
            self.stop_acknowledged.store(true, Ordering::Release);
        }
        // The thread removes them itself; repeating it covers a wedged
        // listener that never reached its own cleanup.
        remove_published_descriptors(&self.state_files, &self.instance_id);
        acknowledged && joined
    }

    /// The instance id this process just published, so a caller that started
    /// this listener itself (rather than reading it back off disk) can prove
    /// it is the one now discoverable at these paths.
    ///
    /// Only `local_stdio`'s headless-hosting tests read this back today; a
    /// production caller has no reason to compare its own instance id against
    /// itself.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

pub(crate) fn start_listener(
    state_files: Vec<PathBuf>,
    adapter: Arc<dyn DeviceAdapter>,
    access: watch::Receiver<AccessMode>,
) -> Result<RunningLocalAccess, LocalAccessFailure> {
    #[cfg(all(any(windows, target_os = "linux", target_os = "macos"), not(test)))]
    let direct_directory = match crate::local_http_helper::directory() {
        Ok(directory) => Some(directory),
        Err(_) => {
            eprintln!("Shared HTTP LocalAppData is unavailable; compatibility remains enabled, direct HTTP refused");
            None
        }
    };
    #[cfg(any(not(any(windows, target_os = "linux", target_os = "macos")), test))]
    let direct_directory = None;
    start_listener_inner(
        state_files,
        adapter,
        access,
        &LISTENER_THREADS,
        direct_directory,
    )
}

impl Drop for RunningLocalAccess {
    fn drop(&mut self) {
        if !self.stop() {
            eprintln!(
                "Local MCP listener retirement is unknown; exact original, publication leases, and admission remain retained"
            );
        }
        // Owned's Drop transfers the exact unresolved original into the shared
        // bounded family registry; it never forgets a thread on timeout/unwind.
    }
}

#[cfg(all(test, windows))]
fn start_listener_with_budget(
    state_files: Vec<PathBuf>,
    adapter: Arc<dyn DeviceAdapter>,
    access: watch::Receiver<AccessMode>,
    budget: &'static qc_native_platform::thread_owner::Budget,
) -> Result<RunningLocalAccess, LocalAccessFailure> {
    start_listener_inner(state_files, adapter, access, budget, None)
}

fn start_listener_inner(
    state_files: Vec<PathBuf>,
    adapter: Arc<dyn DeviceAdapter>,
    access: watch::Receiver<AccessMode>,
    budget: &'static qc_native_platform::thread_owner::Budget,
    _direct_directory: Option<PathBuf>,
) -> Result<RunningLocalAccess, LocalAccessFailure> {
    // Admission precedes lease creation, bind, token minting, and publication.
    let permit = budget.reserve().map_err(LocalAccessFailure::other)?;
    // Serialize the probe AND publication across GUI/headless processes. A
    // successful owner keeps these leases until its listener has stopped.
    let mut lease_paths = state_files.clone();
    lease_paths.sort();
    lease_paths.dedup();
    let publication_leases = lease_paths
        .iter()
        .map(|path| acquire_document_lease(path, "publisher", Duration::ZERO))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|message| LocalAccessFailure {
            served_by_another_instance: message.starts_with("Another process owns"),
            message,
        })?;
    let listener = StdTcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Could not bind the local MCP listener: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure the local MCP listener: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("Could not inspect the local MCP listener: {error}"))?;

    let mut token_bytes = [0_u8; 32];
    rand::rng().fill(&mut token_bytes);
    let token = URL_SAFE_NO_PAD.encode(token_bytes);
    let mut instance_bytes = [0_u8; 16];
    rand::rng().fill(&mut instance_bytes);
    let instance_id = URL_SAFE_NO_PAD.encode(instance_bytes);
    let contract_sha256 = tool_contract_sha256();
    let port = address.port();
    let descriptor = LocalDescriptor {
        url: format!("http://127.0.0.1:{port}/mcp"),
        health_url: format!("http://127.0.0.1:{port}/health"),
        rpc_url: format!("http://127.0.0.1:{port}/rpc"),
        bearer_token: token.clone(),
        instance_id: instance_id.clone(),
        tool_contract_sha256: contract_sha256.clone(),
    };
    let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
        adapter: adapter.clone(),
        access,
    });
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    let direct = _direct_directory.as_deref().map(|directory| {
        crate::local_http_listener::DirectHost::prepare(directory, backend.clone(), &instance_id)
    });
    let router = local_router_with_lifecycle(
        backend,
        LocalHealth {
            adapter,
            instance_id: Arc::from(instance_id.clone()),
            tool_contract_sha256: Arc::from(contract_sha256),
            process_id: std::process::id(),
        },
        Arc::from(token),
        allowed_hosts(port),
        Duration::from_secs(15),
        state_files
            .first()
            .and_then(|path| path.parent())
            .map(crate::device_broker_descriptor::device_broker_descriptor_path),
    );
    let task_state_files = state_files.clone();
    #[cfg(test)]
    let fixture_path = state_files.first().cloned();
    let (shutdown, shutdown_rx) = watch::channel(false);
    // Publication happens on the listener thread, after the listener belongs to
    // a running IO runtime, but its outcome belongs to the caller: `start`
    // reported `Ok` while a half-published descriptor pointed at a port that
    // was about to be dropped.
    let (published_tx, published_rx) = mpsc::sync_channel::<Result<(), LocalAccessFailure>>(1);
    let (stopped_tx, stopped) = mpsc::sync_channel::<()>(1);
    let mut task = qc_native_platform::thread_owner::Owned::spawn_reserved(
        "qc-local-mcp", permit, publication_leases, move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = published_tx.send(Err(LocalAccessFailure::other(format!(
                        "Could not start the local MCP runtime: {error}"
                    ))));
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        // Nothing was published, so nothing is removed: the
                        // mirror paths may hold a live instance's descriptor.
                        let _ = published_tx.send(Err(LocalAccessFailure::other(format!(
                            "Could not adopt the local MCP listener: {error}"
                        ))));
                        return;
                    }
                };
                // Nothing has been written yet, and no path below this
                // point removes a descriptor: refusing here leaves the live
                // copy's files exactly as they are, still naming its port.
                if let Some(owner) = live_descriptor_owner(&task_state_files, unix_time_ms()).await
                {
                    eprintln!(
                        "QC Remote local MCP: instance {} (process {}) is already serving the published descriptor; this copy will not publish over it.",
                        owner.instance_id, owner.process_id
                    );
                    let _ = published_tx.send(Err(LocalAccessFailure {
                        message: ANOTHER_COPY_IS_SERVING.into(),
                        served_by_another_instance: true,
                    }));
                    return;
                }
                if let Err(error) = publish_all_or_nothing(&descriptor, &task_state_files) {
                    let _ = published_tx.send(Err(LocalAccessFailure::other(error)));
                    return;
                }
                if published_tx.send(Ok(())).is_err() {
                    remove_published_descriptors(&task_state_files, &descriptor.instance_id);
                    return;
                }

                let mut graceful_rx = shutdown_rx.clone();
                let mut forced_rx = shutdown_rx.clone();
                let mut refresh_rx = shutdown_rx;
                let refresh_paths = task_state_files.clone();
                let refresh_descriptor = descriptor.clone();
                let serve = serve_loopback(listener, router, HEADER_READ_TIMEOUT, async move {
                    wait_for_shutdown(&mut graceful_rx).await
                });
                tokio::select! {
                    outcome = serve => {
                        if let Err(error) = outcome {
                            eprintln!("QC Remote local MCP listener failed: {error}");
                        }
                    }
                    () = async {
                        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
                        if let Some(direct) = direct {
                            if let Err(error) = direct.run().await {
                                eprintln!("QC Remote shared HTTP listener refused: {error}");
                            }
                            // Late Pair/refresh failure must not tear down the
                            // healthy compatibility audience. This actor is
                            // terminal until the native host is restarted;
                            // Pair's identity proof reports the refusal.
                        }
                        std::future::pending::<()>().await;
                    } => {}
                    () = async move {
                        wait_for_shutdown(&mut forced_rx).await;
                        tokio::time::sleep(LISTENER_SHUTDOWN_GRACE).await;
                    } => {}
                    () = refresh_until_shutdown(
                        DESCRIPTOR_REFRESH_INTERVAL,
                        &mut refresh_rx,
                        || {
                            for path in &refresh_paths {
                                if let Err(error) = refresh_descriptor.publish(path) {
                                    eprintln!(
                                        "QC Remote local MCP descriptor refresh failed: {error}. \
                                         Clients treat it as dead {}s after the last successful \
                                         refresh.",
                                        DESCRIPTOR_STALE_AFTER.as_secs()
                                    );
                                }
                            }
                        },
                    ) => {}
                }
                remove_published_descriptors(&task_state_files,&descriptor.instance_id);
            });
            // Runtime/socket retirement precedes acknowledgement. The original
            // owner additionally retains publication leases through native TLS
            // exit and join; a successor cannot be admitted before that proof.
            drop(runtime);
            let _ = stopped_tx.send(());
            #[cfg(test)]
            tests::run_listener_exit_hook(fixture_path.as_deref());
        })
        .map_err(|error| format!("Could not start the local MCP listener thread: {error}"))?;
    match published_rx.recv_timeout(LISTENER_PUBLICATION_TIMEOUT) {
        Ok(Ok(())) => {
            begin_observing_local_clients();
            Ok(RunningLocalAccess {
                state_files,
                shutdown,
                stopped: Mutex::new(stopped),
                stop_acknowledged: AtomicBool::new(false),
                port,
                instance_id,
                _task: Mutex::new(Some(task)),
            })
        }
        Ok(Err(failure)) => {
            let _ = shutdown.send(true);
            if task.wait_within(LISTENER_STOP_TIMEOUT) {
                Err(failure)
            } else {
                Err(LocalAccessFailure::other(format!(
                    "{}; listener retirement is unknown, exact original and leases retained",
                    failure.message
                )))
            }
        }
        Err(_) => {
            let _ = shutdown.send(true);
            let joined = task.wait_within(LISTENER_STOP_TIMEOUT);
            Err(LocalAccessFailure::other(if joined {
                "The local MCP listener did not publish within its startup observation budget"
            } else {
                "Local MCP publication timed out; listener retirement is unknown, exact original and leases retained"
            }))
        }
    }
}

/// What the app tells the UI about the loopback MCP surface. The bearer token
/// is deliberately absent: the descriptor on disk is the only place it lives.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccessStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: Option<u16>,
    pub instance_id: Option<String>,
    pub descriptor_paths: Vec<String>,
    pub last_error: Option<String>,
    /// True when this copy stood down because another live copy of QC Remote
    /// already owns the published descriptor. `last_error` then carries the
    /// sentence; this is what the panel branches on, so it never has to match
    /// against the wording.
    pub served_by_another_instance: bool,
}

/// What one `Test connection` found.
///
/// The states are separated by remedy rather than by cause, because the remedy
/// is the only thing the person pressing the button can act on: turning local
/// access back on, restarting QC Remote, closing a second copy of it, and
/// plugging the Quad Cortex in are four different acts, and until now the only
/// verification offered anywhere was prose telling the user to type `/mcp` in
/// their AI app and see what happened.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LocalMcpTestState {
    /// This user turned the loopback surface off; nothing was dialled.
    Disabled,
    /// No descriptor a client could use: absent, unreadable, or no longer
    /// refreshed, which all mean the same thing to a client.
    NotPublished,
    /// The published address did not answer, or answered with something no
    /// QC Remote would send.
    NotAnswering,
    /// The published credential was refused, so the descriptor and the
    /// listener disagree.
    Unauthorized,
    /// It answered, but it is a different copy of QC Remote from this one.
    ServedByAnotherCopy,
    /// The app answered; no Quad Cortex is on the other end of it.
    DeviceNotConnected,
    /// A full round trip, device included.
    Ready,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalMcpTestResult {
    pub state: LocalMcpTestState,
    pub app_version: String,
    /// The preset the Quad Cortex reports as loaded, when one is loaded.
    ///
    /// `system.status` is the only call on this route that answers from
    /// already-synchronized session state without a device round trip, and it
    /// carries no device name - the device's own name lives behind
    /// `device.identity`, which this route deliberately does not serve. The
    /// loaded preset is what it does carry, and it is what makes a success
    /// visibly specific instead of a bare tick.
    pub preset_name: Option<String>,
    /// The raw diagnosis, for the panel's existing "Copy details" affordance.
    /// It is never the sentence the user is shown.
    pub detail: Option<String>,
}

fn local_test_result(
    state: LocalMcpTestState,
    preset_name: Option<String>,
    detail: Option<String>,
) -> LocalMcpTestResult {
    LocalMcpTestResult {
        state,
        app_version: env!("CARGO_PKG_VERSION").into(),
        preset_name,
        detail,
    }
}

/// Dial the published loopback endpoint exactly as a paired AI app would, and
/// report what happened.
///
/// The descriptor is resolved from disk rather than from this process's own
/// listener handle on purpose: a test that used the in-process port and token
/// would pass in precisely the situation the user is complaining about, where
/// the file every client actually reads names something else.
///
/// The request is `system.status`: read-only, side-effect free, served from
/// cached session state, and already reachable at every access tier. It opens
/// no preset, touches no setting and writes nothing.
pub(crate) async fn probe_local_mcp(
    paths: &[PathBuf],
    status: &LocalAccessStatus,
    now_ms: u64,
) -> LocalMcpTestResult {
    if !status.enabled {
        return local_test_result(LocalMcpTestState::Disabled, None, None);
    }
    let Ok(endpoint) = crate::local_stdio::descriptor_from_paths_at(paths, now_ms) else {
        return local_test_result(LocalMcpTestState::NotPublished, None, None);
    };
    // Whose endpoint answered. An instance id this app is not serving under
    // means the pairing works, but through a copy the user is not looking at -
    // which is a different thing to tell them than "it works".
    let another_copy = status.instance_id.as_deref() != Some(endpoint.instance_id.as_str());
    let address = endpoint.rpc_url.to_string();
    let client = match crate::loopback_http::loopback_client_builder()
        .timeout(LOCAL_TEST_TIMEOUT)
        .connect_timeout(LOCAL_TEST_CONNECT_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return local_test_result(
                LocalMcpTestState::NotAnswering,
                None,
                Some(error.to_string()),
            );
        }
    };
    if let Err(failure) = endpoint.prove_identity(&client).await {
        // A listener that answers as this instance but cannot prove the
        // published credential is the same disagreement a 401 reports.
        return local_test_result(
            if failure.credential_mismatch {
                LocalMcpTestState::Unauthorized
            } else {
                LocalMcpTestState::NotAnswering
            },
            None,
            Some(format!("{address}: {failure}")),
        );
    }
    let response = match client
        .post(endpoint.rpc_url.clone())
        .bearer_auth(&endpoint.bearer_token)
        .json(&serde_json::json!({ "rpc": "system.status", "params": {} }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return local_test_result(
                LocalMcpTestState::NotAnswering,
                None,
                Some(format!("{address}: {error}")),
            );
        }
    };
    let code = response.status().as_u16();
    if code == StatusCode::UNAUTHORIZED.as_u16() {
        return local_test_result(
            LocalMcpTestState::Unauthorized,
            None,
            Some(format!("{address} answered {code}")),
        );
    }
    let body = match qc_relay_client::bounded_response_json::<Value>(response).await {
        Ok(body) => body,
        Err(_) => {
            return local_test_result(
                LocalMcpTestState::NotAnswering,
                None,
                Some(format!(
                    "{address} answered {code}: invalid, oversized, or incomplete response"
                )),
            );
        }
    };
    if body["ok"] == Value::Bool(true) {
        let diagnostics = &body["result"]["usbDiagnostics"];
        let connected = diagnostics["connected"].as_bool().unwrap_or(false);
        let preset = diagnostics["activePresetName"]
            .as_str()
            .map(str::to_owned)
            .filter(|_| connected);
        let state = if another_copy {
            LocalMcpTestState::ServedByAnotherCopy
        } else if connected {
            LocalMcpTestState::Ready
        } else {
            LocalMcpTestState::DeviceNotConnected
        };
        return local_test_result(
            state,
            preset,
            Some(format!(
                "{address} answered {code}; deviceConnected={connected}"
            )),
        );
    }
    // The typed reason every local route now carries. `device_unavailable` and
    // `device_error` are the app telling us about the Quad Cortex; anything
    // else is the app itself answering wrongly, which has the same remedy as
    // not answering at all.
    let reason = body["reason"]
        .as_str()
        .or_else(|| body["error"]["code"].as_str())
        .unwrap_or("unrecognized")
        .to_owned();
    let state = if another_copy {
        LocalMcpTestState::ServedByAnotherCopy
    } else if matches!(reason.as_str(), "device_unavailable" | "device_error") {
        LocalMcpTestState::DeviceNotConnected
    } else {
        LocalMcpTestState::NotAnswering
    };
    local_test_result(
        state,
        None,
        Some(format!("{address} answered {code}: {reason}")),
    )
}

/// Ask the app's own published endpoint whether a pairing is working now.
///
/// The LOCAL panel could say whether a client had ever announced itself, but
/// there was no way to ask "is this working right now" - the only verification
/// offered anywhere was prose telling the user to type `/mcp` in their AI app.
#[tauri::command]
pub async fn test_local_mcp(app: AppHandle) -> Result<LocalMcpTestResult, String> {
    let (paths, status) = {
        let access = app.state::<LocalAccess>();
        (state_file_paths(&app)?, access.status())
    };
    Ok(probe_local_mcp(&paths, &status, unix_time_ms()).await)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalAccessPreference {
    enabled: bool,
}

/// Local MCP ships on, and stays on until this user turns it off: an absent,
/// unreadable or malformed preference is "on", never a silent opt-out.
fn load_enabled(path: &Path) -> bool {
    crate::local_stdio::read_local_document(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LocalAccessPreference>(&bytes).ok())
        .map(|preference| preference.enabled)
        .unwrap_or(true)
}

/// Whether this user left local MCP on, read from the app's local data
/// directory - the same preference a window's supervisor reads at startup.
/// For a headless `--local-mcp-bridge` process, which has no `AppHandle` to
/// resolve the directory with and must honour the same switch before serving.
pub(crate) fn local_mcp_enabled_in(app_local_data_dir: &Path) -> bool {
    load_enabled(&app_local_data_dir.join(ENABLED_FILE_NAME))
}

/// Records the preference the way [`LocalAccess::enable`]/[`LocalAccess::disable`]
/// do, for tests standing in for a window's settings.
#[cfg(test)]
pub(crate) fn store_local_mcp_enabled_in(
    app_local_data_dir: &Path,
    enabled: bool,
) -> Result<(), String> {
    store_enabled(&app_local_data_dir.join(ENABLED_FILE_NAME), enabled)
}

fn store_enabled(path: &Path, enabled: bool) -> Result<(), String> {
    store_enabled_with(path, enabled, |path, parent, encoded| {
        publish_atomically(path, parent, "the local MCP preference", encoded)
    })
}

fn store_enabled_with(
    path: &Path,
    enabled: bool,
    publish: impl FnOnce(&Path, &Path, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("The local MCP preference path has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the local MCP state directory: {error}"))?;
    let encoded = serde_json::to_vec(&LocalAccessPreference { enabled })
        .map_err(|error| format!("Could not encode the local MCP preference: {error}"))?;
    publish(path, parent, &encoded)
}

fn preference_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_local_data_directory(app)?.join(ENABLED_FILE_NAME))
}

// The running slot also serializes preference changes and fresh preference
// reads. Factories remain under that same lock and must not reenter it.
fn listener_policy_transaction<T>(
    slot: &Mutex<Option<RunningLocalAccess>>,
    action: impl FnOnce(&mut Option<RunningLocalAccess>) -> T,
) -> T {
    action(&mut lock_recover(slot))
}

fn listener_policy_result_transaction<T>(
    slot: &Mutex<Option<RunningLocalAccess>>,
    action: impl FnOnce(&mut Option<RunningLocalAccess>) -> Result<T, String>,
    publish_error: impl FnOnce(&str),
) -> Result<T, String> {
    listener_policy_transaction(slot, |running| {
        let result = action(running);
        if let Err(error) = &result {
            publish_error(error);
        }
        result
    })
}

#[cfg(all(test, windows))]
fn start_listener_slot(
    slot: &Mutex<Option<RunningLocalAccess>>,
    last_error: &Mutex<Option<String>>,
    served_by_another: &AtomicBool,
    factory: impl FnOnce() -> Result<RunningLocalAccess, LocalAccessFailure>,
) -> Result<(), String> {
    listener_policy_transaction(slot, |running| {
        start_listener_slot_locked(running, last_error, served_by_another, || true, factory)
    })
}

fn start_listener_slot_locked(
    running: &mut Option<RunningLocalAccess>,
    last_error: &Mutex<Option<String>>,
    served_by_another: &AtomicBool,
    current: impl FnOnce() -> bool,
    factory: impl FnOnce() -> Result<RunningLocalAccess, LocalAccessFailure>,
) -> Result<(), String> {
    // Admission is checked only after the policy/slot transaction was acquired.
    // A stale preflight may never publish after a completed disable.
    if !current() {
        return Ok(());
    }
    if running
        .as_ref()
        .is_some_and(RunningLocalAccess::native_finished)
    {
        // OS-signalled original only: never GUI-join a pending native thread.
        let _ = running.take().expect("finished original").join_original();
    }
    let started = if let Some(listener) = running.as_ref() {
        if !*listener.shutdown.borrow() {
            return Ok(());
        }
        Err(LocalAccessFailure::other(
            "The previous local MCP listener is still retiring; retry after native shutdown completes",
        ))
    } else {
        factory()
    };
    match started {
        Ok(listener) => {
            *running = Some(listener);
            *lock_recover(last_error) = None;
            served_by_another.store(false, Ordering::Release);
            begin_observing_local_clients();
            Ok(())
        }
        Err(failure) => {
            *lock_recover(last_error) = Some(failure.message.clone());
            served_by_another.store(failure.served_by_another_instance, Ordering::Release);
            Err(failure.message)
        }
    }
}

fn stop_listener_slot(
    slot: &Mutex<Option<RunningLocalAccess>>,
    last_error: &Mutex<Option<String>>,
) {
    listener_policy_transaction(slot, |running| {
        stop_listener_slot_locked(running, last_error)
    });
}

fn stop_listener_slot_locked(
    running: &mut Option<RunningLocalAccess>,
    last_error: &Mutex<Option<String>>,
) {
    if let Some(listener) = running.take() {
        if !listener.stop() {
            *lock_recover(last_error) = Some(
                "Local MCP shutdown is not acknowledged; exact listener remains owned and retiring"
                    .into(),
            );
            *running = Some(listener);
        }
    }
}

fn reconcile_listener_slot(
    slot: &Mutex<Option<RunningLocalAccess>>,
    last_error: &Mutex<Option<String>>,
    served_by_another: &AtomicBool,
    enabled: &AtomicBool,
    read_policy: impl FnOnce() -> (bool, bool),
    factory: impl FnOnce() -> Result<RunningLocalAccess, LocalAccessFailure>,
) -> Result<(), String> {
    listener_policy_transaction(slot, |running| {
        // Fresh preference read, its atomic publication and the corresponding
        // start/stop all belong to this one existing supervisor transaction.
        let (requested, permitted) = read_policy();
        enabled.store(requested, Ordering::Release);
        if !requested || !permitted {
            stop_listener_slot_locked(running, last_error);
            Ok(())
        } else {
            start_listener_slot_locked(running, last_error, served_by_another, || true, factory)
        }
    })
}

/// Owns only the loopback listener and discovery document. The existing
/// Windows broker remains the sole owner of the Quad Cortex USB session.
///
/// This is a supervisor, not the listener itself: the surface can be stopped
/// and started again for the lifetime of the process, because there was
/// previously no way at all to turn it off. Nothing revoked a descriptor,
/// nothing cancelled a live session, and the bearer token was written before
/// the user had clicked anything.
pub struct LocalAccess {
    app: AppHandle,
    adapter: Arc<dyn DeviceAdapter>,
    access: watch::Receiver<AccessMode>,
    running: Mutex<Option<RunningLocalAccess>>,
    enabled: AtomicBool,
    last_error: Mutex<Option<String>>,
    served_by_another_instance: AtomicBool,
}

impl LocalAccess {
    pub(crate) fn reconcile_ownership(&self) {
        let Ok(directory) = app_local_data_directory(&self.app) else {
            return;
        };
        let _ = reconcile_listener_slot(
            &self.running,
            &self.last_error,
            &self.served_by_another_instance,
            &self.enabled,
            || {
                (
                    local_mcp_enabled_in(&directory),
                    !crate::external_host::stopping()
                        && !crate::external_host::delegated(&directory),
                )
            },
            || {
                state_file_paths(&self.app)
                    .map_err(LocalAccessFailure::other)
                    .and_then(|state_files| {
                        start_listener(state_files, self.adapter.clone(), self.access.clone())
                    })
            },
        );
    }
    /// Builds the supervisor and reads the persisted on/off choice. Starting is
    /// a separate, fallible step so its failure can reach the UI.
    pub fn new(
        app: AppHandle,
        adapter: Arc<dyn DeviceAdapter>,
        access: watch::Receiver<AccessMode>,
    ) -> Self {
        // Before anything can accept an announcement, so the first status read
        // of this launch already knows which hosts have worked here before.
        // Only the durable fact is restored; no instant is.
        if let Ok(path) = attach_history_path(&app) {
            restore_attach_history(&path);
        }
        let enabled = preference_path(&app)
            .map(|path| load_enabled(&path))
            .unwrap_or(true);
        Self {
            app,
            adapter,
            access,
            running: Mutex::new(None),
            enabled: AtomicBool::new(enabled),
            last_error: Mutex::new(None),
            served_by_another_instance: AtomicBool::new(false),
        }
    }

    /// Starts the loopback listener unless this user turned local MCP off.
    pub fn start_if_enabled(&self) -> Result<(), String> {
        if !self.enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        self.start_now()
    }

    fn start_now(&self) -> Result<(), String> {
        listener_policy_transaction(&self.running, |running| self.start_now_locked(running))
    }

    fn start_now_locked(&self, running: &mut Option<RunningLocalAccess>) -> Result<(), String> {
        start_listener_slot_locked(
            running,
            &self.last_error,
            &self.served_by_another_instance,
            || {
                self.enabled.load(Ordering::Acquire)
                    && !crate::external_host::stopping()
                    && !app_local_data_directory(&self.app)
                        .is_ok_and(|directory| crate::external_host::delegated(&directory))
            },
            || {
                state_file_paths(&self.app)
                    .map_err(LocalAccessFailure::other)
                    .and_then(|state_files| {
                        start_listener(state_files, self.adapter.clone(), self.access.clone())
                    })
            },
        )
    }

    fn stop_now(&self) {
        stop_listener_slot(&self.running, &self.last_error);
    }

    /// Turns the loopback MCP surface back on and remembers the choice.
    pub fn enable(&self) -> Result<LocalAccessStatus, String> {
        let result = listener_policy_result_transaction(
            &self.running,
            |running| {
                self.enabled.store(true, Ordering::Release);
                let stored = preference_path(&self.app).and_then(|path| store_enabled(&path, true));
                let started = self.start_now_locked(running);
                stored.and(started)
            },
            |error| *lock_recover(&self.last_error) = Some(error.to_owned()),
        );
        match result {
            Ok(()) => Ok(self.status()),
            Err(error) => Err(error),
        }
    }

    /// Stops the listener, cancels live sessions and removes every descriptor
    /// this process published, then remembers that the user asked for it.
    pub fn disable(&self) -> Result<LocalAccessStatus, String> {
        let stored = listener_policy_result_transaction(
            &self.running,
            |running| {
                self.enabled.store(false, Ordering::Release);
                stop_listener_slot_locked(running, &self.last_error);
                // Off by request is not displacement by another copy.
                self.served_by_another_instance
                    .store(false, Ordering::Release);
                preference_path(&self.app).and_then(|path| store_enabled(&path, false))
            },
            |error| *lock_recover(&self.last_error) = Some(error.to_owned()),
        );
        match stored {
            Ok(()) => Ok(self.status()),
            Err(error) => Err(error),
        }
    }

    /// The exit path. Stops without touching the persisted choice, so the
    /// surface comes back on the next launch.
    pub fn shutdown_for_exit(&self) {
        self.stop_now();
        // The throttle defers most announcements, so the newest timestamps are
        // usually only in memory when the process ends. This is the one place
        // a blocking write is what the caller wants.
        flush_attach_history();
    }

    pub fn status(&self) -> LocalAccessStatus {
        let running = lock_recover(&self.running);
        if running.is_none() && self.enabled.load(Ordering::Acquire) {
            if let Some(external) = crate::external_host::status() {
                if let Some(endpoint) = external["endpoint"]
                    .as_str()
                    .and_then(|raw| reqwest::Url::parse(raw).ok())
                {
                    return LocalAccessStatus {
                        enabled: true,
                        running: true,
                        port: endpoint.port(),
                        instance_id: external["instanceId"].as_str().map(str::to_owned),
                        descriptor_paths: state_file_paths(&self.app)
                            .unwrap_or_default()
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect(),
                        last_error: None,
                        served_by_another_instance: false,
                    };
                }
            }
        }
        LocalAccessStatus {
            enabled: self.enabled.load(Ordering::Acquire),
            running: running.is_some(),
            port: running.as_ref().map(|listener| listener.port),
            instance_id: running
                .as_ref()
                .map(|listener| listener.instance_id.clone()),
            descriptor_paths: running
                .as_ref()
                .map(|listener| {
                    listener
                        .state_files
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
            last_error: lock_recover(&self.last_error).clone(),
            served_by_another_instance: self.served_by_another_instance.load(Ordering::Acquire),
        }
    }
}

/// A poisoned lock here means a previous caller panicked while holding it. The
/// state behind it is a listener handle and a preference flag, neither of which
/// a panic can leave half-written - and refusing to clean up on the exit path
/// because of one is strictly worse than proceeding.
fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for LocalAccess {
    fn drop(&mut self) {
        // `tauri::App::run` ends in `std::process::exit`, so this does not run
        // on the ordinary exit path; `RunEvent::Exit` in `lib.rs` is what does.
        // Kept for `run_return`, for tests, and for any future caller that owns
        // a `LocalAccess` by value.
        self.stop_now();
    }
}

static PUBLICATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub use crate::broker_paths::unix_time_ms;

fn native_bridge_is_recent(last_seen: u64, now: u64) -> bool {
    last_seen != 0
        && now.saturating_sub(last_seen) <= NATIVE_BRIDGE_ACTIVITY_WINDOW.as_millis() as u64
}

/// Records that heartbeats can arrive from this instant, so the start-up
/// window can be distinguished from a host that is absent.
///
/// Called on every successful listener start, including a restart after the
/// user switched the surface off and on: while it was off no heartbeat could
/// be accepted, so the clock has to run again from when it could.
fn begin_observing_local_clients() {
    LOCAL_CLIENT_OBSERVATION_SINCE.store(unix_time_ms(), Ordering::Release);
}

/// What this app can honestly say about one host's bridge.
///
/// Three values, not two booleans, because "no heartbeat" meant two different
/// things and the panel said the more damaging one for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalClientLiveness {
    /// A heartbeat carrying this client id arrived inside the activity window.
    /// The only state that licenses the word "Connected".
    Connected,
    /// No heartbeat yet, and a healthy bridge has not had time to send one
    /// since this app started accepting them. Nothing is known; nothing is
    /// claimed either way.
    Settling,
    /// No heartbeat, and long enough has passed that a running bridge would
    /// have announced itself.
    Absent,
}

/// Everything known about one host's bridge, read in one pass so a status
/// document cannot mix two instants, and so no provider can invent its own
/// rule out of the parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalClientEvidence {
    pub liveness: LocalClientLiveness,
    /// A bridge with this client id has announced itself at least once - in
    /// this run, or in an earlier one this app still has on record. It is
    /// history, never liveness: it says the host's bridge has worked against
    /// this app, and says nothing at all about whether it is running now.
    pub ever_attached: bool,
}

impl LocalClientEvidence {
    pub fn connected(self) -> bool {
        self.liveness == LocalClientLiveness::Connected
    }

    /// True while a healthy bridge may simply not have ticked yet. A row in
    /// this state says it is still checking; it never says Connected.
    pub fn settling(self) -> bool {
        self.liveness == LocalClientLiveness::Settling
    }

    pub fn attached(self) -> bool {
        self.ever_attached
    }

    /// Whether a host has demonstrably read the configuration this app wrote.
    ///
    /// The client id a bridge announces carries this build's own version, so a
    /// bridge that announces it was started from a route naming this version -
    /// which is what a process-start timestamp was standing in for, and got
    /// wrong whenever the host loaded its MCP servers without restarting.
    /// Once seen it is not un-seen: an MCP host may start and stop a bridge
    /// per chat, and the configuration it read does not become unread when the
    /// bridge exits.
    pub fn picked_up_current_config(self) -> bool {
        self.connected() || self.ever_attached
    }
}

/// Per-host bridge liveness keeps first-class desktop integrations honest. A
/// global native heartbeat is sufficient for ChatGPT/Codex's shared config,
/// but must not make LM Studio, Jan, or an arbitrary MCP host appear
/// connected.
pub fn local_client_evidence(client: &str) -> LocalClientEvidence {
    if let Some(external) = crate::external_host::status() {
        let seen = external["bridgeClients"][client]
            .as_u64()
            .unwrap_or_default();
        return local_client_evidence_at(
            seen,
            seen > 0 || local_client_attached_before(client),
            external["observedSince"].as_u64().unwrap_or_default(),
            unix_time_ms(),
        );
    }
    let last_seen = local_client_activity_snapshot()
        .get(client)
        .copied()
        .unwrap_or_default();
    local_client_evidence_at(
        last_seen,
        local_client_attached_before(client),
        LOCAL_CLIENT_OBSERVATION_SINCE.load(Ordering::Acquire),
        unix_time_ms(),
    )
}

/// The rule itself, separated from the statics that feed it so the start-up
/// window can be asserted without a listener or a clock.
///
/// `last_seen` is this process's own observation and is the *only* input to
/// liveness. `attached_before` is the durable record and is the only input
/// that survives a restart; it can make a row calm, and it can never make one
/// green. The two are separate arguments rather than one timestamp precisely
/// so that the second cannot be fed to [`native_bridge_is_recent`] by
/// accident - `last_seen` used to do both jobs, and remembering it across a
/// restart would have turned every host that once worked into a permanent
/// false "Connected".
pub(crate) fn local_client_evidence_at(
    last_seen: u64,
    attached_before: bool,
    observing_since: u64,
    now: u64,
) -> LocalClientEvidence {
    let ever_attached = attached_before || last_seen != 0;
    let liveness = if native_bridge_is_recent(last_seen, now) {
        LocalClientLiveness::Connected
    } else if observing_since != 0
        && now.saturating_sub(observing_since) <= NATIVE_BRIDGE_ACTIVITY_WINDOW.as_millis() as u64
    {
        // The same tolerance the app already grants a bridge that has
        // announced itself. A bridge announces every 5s, so one that has said
        // nothing for the whole window is genuinely not there - and using the
        // one constant keeps the two halves of the rule from drifting.
        LocalClientLiveness::Settling
    } else {
        LocalClientLiveness::Absent
    };
    LocalClientEvidence {
        liveness,
        ever_attached,
    }
}

pub fn local_client_connected(client: &str) -> bool {
    local_client_evidence(client).connected()
}

/// The durable half of a host's bridge record, and the throttle deciding when
/// it reaches disk.
#[derive(Default)]
struct ClientAttachHistory {
    generation: u64,
    /// Where the durable copy lives, once the app has resolved its own local
    /// data directory. `None` before start-up, and in any test that does not
    /// ask for a file: the record is then in-memory only and nothing is
    /// written, which is exactly the behaviour it replaces.
    path: Option<PathBuf>,
    /// Client id -> the unix millisecond at which a bridge carrying it last
    /// announced itself. The value orders eviction and nothing else; only the
    /// *presence* of a key is ever allowed to influence what a row says.
    clients: BTreeMap<String, u64>,
    /// When this process last attempted a durable write, or 0 if it has not.
    last_written: u64,
    /// An announcement moved a timestamp the throttle declined to write, or a
    /// write failed. A clean shutdown flushes on this.
    unwritten: bool,
    /// The last write failure written to stderr, so a disk that has been full
    /// all morning costs one line rather than one every five minutes.
    reported_failure: Option<String>,
}

/// The on-disk form. Small and versioned: a build that learns something new
/// about a client bumps [`CLIENT_HISTORY_SCHEMA_VERSION`], and a build that
/// does not recognise the version it finds starts from nothing rather than
/// reading fields it cannot interpret.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientAttachHistoryFile {
    schema_version: u32,
    #[serde(default)]
    clients: BTreeMap<String, u64>,
}

fn attach_history() -> &'static Mutex<ClientAttachHistory> {
    LOCAL_CLIENT_ATTACH_HISTORY.get_or_init(|| Mutex::new(ClientAttachHistory::default()))
}

struct HistoryWrite {
    path: PathBuf,
    clients: BTreeMap<String, u64>,
    generation: u64,
}
enum HistoryCommand {
    Write(HistoryWrite),
    Stop,
}
struct HistoryWriter {
    sender: HistorySender,
    task: std::thread::JoinHandle<()>,
}
#[derive(Default)]
struct HistoryQueue {
    latest: Option<HistoryWrite>,
    stopping: bool,
}
struct HistorySender {
    wake: mpsc::SyncSender<()>,
    queue: Arc<Mutex<HistoryQueue>>,
}
impl HistorySender {
    fn send(&self, command: HistoryCommand) -> Result<(), String> {
        let mut queue = lock_recover(&self.queue);
        if queue.stopping {
            return Err("The local MCP history writer is stopping".into());
        }
        match command {
            HistoryCommand::Write(snapshot) => queue.latest = Some(snapshot),
            HistoryCommand::Stop => queue.stopping = true,
        }
        match self.wake.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(())) => {
                Err("The local MCP history writer stopped".into())
            }
        }
    }
}
static HISTORY_WRITER: OnceLock<Mutex<Option<HistoryWriter>>> = OnceLock::new();
fn history_writer() -> &'static Mutex<Option<HistoryWriter>> {
    HISTORY_WRITER.get_or_init(|| Mutex::new(None))
}

fn start_history_writer(
    persist: impl Fn(&HistoryWrite) -> Result<(), String> + Send + 'static,
    completed: impl Fn(&HistoryWrite, Result<(), String>) + Send + 'static,
) -> Result<HistoryWriter, String> {
    let (wake, receiver) = mpsc::sync_channel(1);
    let queue = Arc::new(Mutex::new(HistoryQueue::default()));
    let sender = HistorySender {
        wake,
        queue: queue.clone(),
    };
    let task = std::thread::Builder::new()
        .name("qc-local-client-history".into())
        .spawn(move || {
            while receiver.recv().is_ok() {
                let (snapshot, stop) = {
                    let mut queue = lock_recover(&queue);
                    (queue.latest.take(), queue.stopping)
                };
                if let Some(snapshot) = snapshot {
                    let result = persist(&snapshot);
                    completed(&snapshot, result);
                }
                if stop {
                    break;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(HistoryWriter { sender, task })
}

fn enqueue_history(snapshot: HistoryWrite) -> Result<(), String> {
    let mut writer = lock_recover(history_writer());
    if writer.is_none() {
        *writer = Some(start_history_writer(
            |snapshot| store_attach_history(&snapshot.path, &snapshot.clients),
            |snapshot, result| {
                if result.is_ok() {
                    if let Ok(mut history) = attach_history().lock() {
                        if history.generation == snapshot.generation
                            && history.path.as_ref() == Some(&snapshot.path)
                        {
                            history.unwritten = false;
                        }
                    }
                } else if let Err(error) = result {
                    report_history_failure(&error);
                }
            },
        )?);
    }
    writer
        .as_ref()
        .expect("writer started")
        .sender
        .send(HistoryCommand::Write(snapshot))
        .map_err(|error| error.to_string())
}

/// Reads the durable record.
///
/// Absent, unreadable, truncated, not JSON at all, or written under a schema
/// this build does not know are one case with one answer: an empty history,
/// which is what every release before this one had permanently. Nothing here
/// can panic and nothing here can fail a launch.
fn load_attach_history(path: &Path) -> BTreeMap<String, u64> {
    let mut clients = load_attach_history_document(path);
    retain_current_clients(&mut clients);
    evict_oldest_clients(&mut clients);
    clients
}

fn load_attach_history_document(path: &Path) -> BTreeMap<String, u64> {
    let Some(document) = crate::local_stdio::read_local_document(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ClientAttachHistoryFile>(&bytes).ok())
    else {
        return BTreeMap::new();
    };
    if document.schema_version != CLIENT_HISTORY_SCHEMA_VERSION {
        return BTreeMap::new();
    }
    document.clients
}

fn store_attach_history(path: &Path, clients: &BTreeMap<String, u64>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("The local MCP client history path has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the local MCP state directory: {error}"))?;
    let _lease = acquire_document_lease(path, "history", Duration::from_secs(2))?;
    // A different process can have recorded another provider since we loaded
    // our snapshot. Union under the cross-process lease, never overwrite it.
    // A prior build may flush after the successor publishes. It must not
    // apply its current-only consumer filter to the successor's disk facts.
    let mut merged = load_attach_history_document(path);
    for (client, timestamp) in clients {
        merged
            .entry(client.clone())
            .and_modify(|previous| *previous = (*previous).max(*timestamp))
            .or_insert(*timestamp);
    }
    merged.retain(|client, _| {
        client_id_is_current(client) || client_id_is_newer(client, env!("CARGO_PKG_VERSION"))
    });
    evict_oldest_clients(&mut merged);
    let encoded = serde_json::to_vec(&ClientAttachHistoryFile {
        schema_version: CLIENT_HISTORY_SCHEMA_VERSION,
        clients: merged,
    })
    .map_err(|error| format!("Could not encode the local MCP client history: {error}"))?;
    publish_atomically(path, parent, "the local MCP client history", &encoded)
}

/// Whether this build could ever ask about this client id.
///
/// Every id is minted as `<provider>:<CARGO_PKG_VERSION>` and every read is
/// `local_client_evidence` against one of this build's own constants, so an id
/// carrying any other version cannot be looked up by anything - not by a row,
/// not by a heartbeat, not by a test. Keeping those rows cost the record its
/// whole capacity within five releases and told the reader nothing, because
/// the question they answer is one nothing asks.
///
/// This is *not* a claim that the host is running. Liveness stays where it was,
/// per-process and unremembered; this only decides whether an id is reachable.
fn client_id_is_current(client: &str) -> bool {
    client
        .rsplit_once(':')
        .is_some_and(|(_, version)| version == env!("CARGO_PKG_VERSION"))
}

fn client_id_is_newer(client: &str, current: &str) -> bool {
    let Some((provider, version)) = client.rsplit_once(':') else {
        return false;
    };
    if provider.is_empty() || client.len() > 64 || client.chars().any(char::is_control) {
        return false;
    }
    match (
        semver::Version::parse(version),
        semver::Version::parse(current),
    ) {
        (Ok(version), Ok(current)) => version.cmp_precedence(&current).is_gt(),
        _ => false,
    }
}

/// Drops every id this build cannot ask about.
///
/// An id that survives an upgrade is not evidence a host is stale: the route
/// refresh rewrites each paired host's configuration to this build's id, and
/// the bridge that announces afterwards carries it. An old bridge still running
/// from before the refresh announces under its own id, which this drops - the
/// row it would feed reads the new id and would not have seen it either way.
fn retain_current_clients(clients: &mut BTreeMap<String, u64>) {
    clients.retain(|client, _| client_id_is_current(client));
}

/// Holds the record to [`CLIENT_HISTORY_CAPACITY`], dropping least-recently
/// attached first.
///
/// A tie is broken by client id so the same input always produces the same
/// file: two ids that last attached in the same millisecond must not make the
/// record depend on hash order.
fn evict_oldest_clients(clients: &mut BTreeMap<String, u64>) {
    if clients.len() <= CLIENT_HISTORY_CAPACITY {
        return;
    }
    let mut ordered: Vec<(String, u64)> = std::mem::take(clients).into_iter().collect();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ordered.truncate(CLIENT_HISTORY_CAPACITY);
    *clients = ordered.into_iter().collect();
}

/// Applies one announcement to the record and answers whether it is owed a
/// durable write now.
///
/// Pure, and taking its clock and its last-write instant as arguments, so the
/// throttle can be asserted without a disk, a listener or a sleep. The first
/// attach for a client id writes immediately because that is the announcement
/// that changes what the panel says about that host; a repeat only refreshes
/// an eviction key, and waits for [`CLIENT_HISTORY_WRITE_INTERVAL`].
fn note_attach(
    clients: &mut BTreeMap<String, u64>,
    client: &str,
    now: u64,
    last_written: u64,
) -> bool {
    // An announcement from a bridge this build did not write a route for.
    // Recording it would put a row on disk that nothing can read and that the
    // next launch deletes; declining costs the write instead of paying it.
    if !client_id_is_current(client) {
        return false;
    }
    let first_attach = !clients.contains_key(client);
    clients.insert(client.to_string(), now);
    if first_attach {
        evict_oldest_clients(clients);
        return true;
    }
    last_written == 0
        || now.saturating_sub(last_written) >= CLIENT_HISTORY_WRITE_INTERVAL.as_millis() as u64
}

/// Whether a write failure is worth a line on stderr.
///
/// Uses the bridge announcement policy at the smaller scale these paths need:
/// one line when it breaks, one when the reason changes, and nothing while the
/// same reason keeps repeating. A
/// failure that is never reported is the defect that policy exists to close,
/// so silence is not an option either.
///
/// Shared with the start-up route refresh in `client_packages.rs` rather than
/// reimplemented there. The policy has one subtlety and it is fatal when it is
/// got wrong: `failure` is the limiter's KEY, so anything in it that varies per
/// attempt (a timestamp, an age, a staged file name carrying a process id)
/// makes every failure a new key and restores the line-per-attempt flood the
/// limiter exists to prevent.
pub(crate) fn should_report_failure(reported: Option<&str>, failure: &str) -> bool {
    reported != Some(failure)
}

/// Loads the durable record and points it at the file it came from.
///
/// Called once, at start-up, before the listener can accept an announcement.
/// It seeds *only* the durable map: [`LOCAL_CLIENT_LAST_SEEN`] stays empty, so
/// no remembered instant can reach [`native_bridge_is_recent`] and no row can
/// go green on a bridge that is not running now.
fn restore_attach_history(path: &Path) {
    if let Ok(mut history) = attach_history().lock() {
        let clients = load_attach_history(path);
        history.clients = clients;
        history.path = Some(path.to_path_buf());
        history.last_written = 0;
        history.unwritten = false;
        history.generation = history.generation.wrapping_add(1);
    }
}

/// Records that a bridge carrying `client` has announced itself, and writes
/// the record when the throttle says one is owed.
///
/// One owned worker serializes/coalesces commits without holding this map
/// during filesystem IO. Shutdown sends the newest generation and joins it.
fn record_local_client_attach(client: &str, now: u64) {
    let Ok(mut history) = attach_history().lock() else {
        return;
    };
    let last_written = history.last_written;
    let owed = note_attach(&mut history.clients, client, now, last_written);
    history.generation = history.generation.wrapping_add(1);
    if !owed {
        history.unwritten = true;
        return;
    }
    let Some(path) = history.path.clone() else {
        // No file resolved: the record is in-memory only, which is a complete
        // and correct answer for this process's own lifetime.
        history.unwritten = false;
        return;
    };
    history.last_written = now;
    history.unwritten = true;
    // Enqueue while holding the map so another announcement cannot enqueue
    // an older/newer snapshot in the reverse order. No IO is done here.
    let outcome = enqueue_history(HistoryWrite {
        path,
        clients: history.clients.clone(),
        generation: history.generation,
    });
    drop(history);
    if let Err(error) = outcome {
        report_history_failure(&error);
    }
}

fn report_history_failure(failure: &str) {
    if let Ok(mut history) = attach_history().lock() {
        history.unwritten = true;
        if !should_report_failure(history.reported_failure.as_deref(), failure) {
            return;
        }
        history.reported_failure = Some(failure.to_string());
    }
    eprintln!("QC Remote could not record which AI apps have connected: {failure}");
}

/// Writes anything the throttle deferred. Called on the way out, where a
/// blocking write is what the caller is asking for.
fn flush_attach_history() {
    // Even an earlier unwinding producer must not bypass owned-worker cleanup.
    let history = lock_recover(attach_history());
    let outcome = if history.unwritten {
        history
            .path
            .clone()
            .map(|path| {
                enqueue_history(HistoryWrite {
                    path,
                    clients: history.clients.clone(),
                    generation: history.generation,
                })
            })
            .transpose()
            .map(|_| ())
    } else {
        Ok(())
    };
    drop(history);
    if let Err(error) = outcome {
        report_history_failure(&error);
    }
    // Retain ownership through its final commit; no detached writer survives
    // a clean shutdown. As before, exit explicitly requests blocking IO.
    let writer = lock_recover(history_writer()).take();
    if let Some(writer) = writer {
        let _ = writer.sender.send(HistoryCommand::Stop);
        // Disconnect is also a wake/termination path if Stop could not reach
        // a failed worker; retaining its sender while joining can deadlock.
        drop(writer.sender);
        if writer.task.join().is_err() {
            report_history_failure("The local MCP history writer stopped unexpectedly");
        }
    }
}

/// Whether a bridge carrying this client id has ever announced itself to this
/// app - in this run or in any earlier one.
///
/// Deliberately a `bool` and not an instant. Returning the remembered
/// timestamp would put an old value within reach of the recency rule, and one
/// careless call site would then paint a green row for a host that last ran
/// weeks ago.
fn local_client_attached_before(client: &str) -> bool {
    attach_history()
        .lock()
        .map(|history| history.clients.contains_key(client))
        .unwrap_or(false)
}

fn attach_history_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_local_data_directory(app)?.join(CLIENT_HISTORY_FILE_NAME))
}

/// Antigravity's own bridge is an ordinary local client that names itself like
/// every other one. It had a second `AtomicU64` of its own next to the shared
/// map, written from the same heartbeat, which is exactly the per-provider
/// fork of liveness that let one provider miss a correction made to the other
/// five. It reads the shared record now.
pub fn antigravity_bridge_connected() -> bool {
    local_client_connected(ANTIGRAVITY_BRIDGE_CLIENT)
}

pub fn antigravity_bridge_evidence() -> LocalClientEvidence {
    local_client_evidence(ANTIGRAVITY_BRIDGE_CLIENT)
}
const REPLACE_ATTEMPTS: u32 = 10;

/// Persistent sidecars must never be unlinked: replacing a lock inode while a
/// contender holds/waits on it would split ownership between two publishers.
/// The OS releases these locks on process death; no PID guessing or reaping.
pub(crate) fn acquire_document_lease(
    path: &Path,
    role: &str,
    timeout: Duration,
) -> Result<fs::File, String> {
    let parent = path
        .parent()
        .ok_or("The local MCP state path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let lease_path = path.with_file_name(format!(
        "{}.{}.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid local MCP state name")?,
        role
    ));
    if !lease_path.exists() {
        if let Err(error) = owner_only_file::write_staged(&lease_path, b"") {
            // A racing creator is fine; its protection is checked below.
            if !lease_path.exists() {
                return Err(error);
            }
        }
    }
    if !qc_owner_only_file::is_owner_only(&lease_path)? {
        return Err("The local MCP publication lease is not owner-private".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = options
        .open(&lease_path)
        .map_err(|error| error.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(error) if started.elapsed() >= timeout => {
                return Err(format!(
                    "Another process owns the local MCP {role} lease: {error}"
                ));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

struct StagedDocument(Option<qc_owner_only_file::ClaimedFile>);
impl Drop for StagedDocument {
    fn drop(&mut self) {
        if let Some(claim) = self.0.take() {
            let _ = claim.release();
        }
    }
}

/// The bridge re-reads this descriptor on every request, so an in-place
/// truncate-then-write hands concurrent readers a partial document. Writing a
/// sibling temporary file and renaming over the target keeps every read whole.
///
/// The staged file is created with an explicit DACL naming this user and
/// nobody else - see [`owner_only_file`] for what that does and does not
/// defend against - and the rename carries that DACL onto the published copy,
/// because a rename inside one directory moves the file's own security
/// descriptor with it and a protected DACL has nothing left to re-inherit.
///
/// `what` names the document in both failure sentences, because the same
/// mechanism now carries the descriptor and the durable client history and an
/// operator reading a stalled write has to be told which one stalled.
fn publish_atomically(
    path: &Path,
    parent: &Path,
    what: &str,
    contents: &[u8],
) -> Result<(), String> {
    remove_orphaned_stage_files(path, parent);
    let sequence = PUBLICATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.{}.{sequence}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(STATE_FILE_NAME),
        std::process::id(),
    ));
    // These two sentences are rate-limiter KEYS, not just messages:
    // `report_history_failure` reports one line per distinct reason and stays
    // silent while the reason repeats. Only the static `what` and the OS error
    // may appear in them. Do not add `temporary` - it carries this process's
    // id and a per-call sequence number, so every failure would be a new key
    // and the limiter would write a line per attempt for ever. That is exactly
    // how an earlier version of this limiter went wrong: an age was folded
    // into a detail first, and the limiter was built on top of it afterwards.
    let mut stage = StagedDocument(Some(
        qc_owner_only_file::ClaimedFile::try_claim(&temporary)
            .map_err(|error| format!("Could not reserve {what}: {error}"))?,
    ));
    stage
        .0
        .as_mut()
        .expect("stage claim")
        .write(contents)
        .map_err(|error| format!("Could not stage {what}: {error}"))?;
    for attempt in 1..=REPLACE_ATTEMPTS {
        match fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            // Windows can briefly refuse the replace while a reader holds the
            // target open, so give the short read a chance to finish.
            Err(error) if attempt == REPLACE_ATTEMPTS => {
                let _ = fs::remove_file(&temporary);
                return Err(format!("Could not publish {what}: {error}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    unreachable!("the final replace attempt returns")
}

/// A staged descriptor carries the same credential as the published one, so a
/// termination between staging and replacing would leave it behind. Publishing
/// is the app's first act, which makes it the right moment to clear the last run.
fn remove_orphaned_stage_files(path: &Path, parent: &Path) {
    let prefix = format!(
        ".{}.",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(STATE_FILE_NAME)
    );
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix) && name.ends_with(".tmp") {
            // A live publisher retains its claim; only dead/legacy stages
            // can be removed, with the claim pinned through deletion.
            let _ = qc_owner_only_file::release_if_unclaimed(&entry.path());
        }
    }
}

// Only the exact loopback host the descriptor publishes. Accepting `localhost`
// as well was unreachable for the bridge and widened DNS-rebinding exposure.
fn allowed_hosts(port: u16) -> Vec<String> {
    vec![format!("127.0.0.1:{port}")]
}

pub(crate) fn tool_contract_sha256() -> String {
    let normalized = String::from_utf8_lossy(QC_ACTIONS_CONTRACT).replace("\r\n", "\n");
    Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_listener_policy_publishes_result_before_releasing_actual_transaction() {
        let slot = Mutex::new(None);
        let last_error = Mutex::new(None);
        let calls = std::cell::Cell::new(0);
        // This is the same result transaction called by enable and disable,
        // not a recreation of their preference or listener policy.
        let old_result: Result<(), String> = listener_policy_result_transaction(
            &slot,
            |_| Err("older preference write failed".to_owned()),
            |error| {
                assert!(
                    matches!(slot.try_lock(), Err(std::sync::TryLockError::WouldBlock)),
                    "result publication escaped the actual listener transaction"
                );
                calls.set(calls.get() + 1);
                *lock_recover(&last_error) = Some(error.to_owned());
            },
        );
        assert_eq!(
            old_result.as_ref().map_err(String::as_str),
            Err("older preference write failed")
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(
            lock_recover(&last_error).as_deref(),
            Some("older preference write failed")
        );

        // A later successful transaction clears the diagnostic. Merely handling
        // the old returned error cannot publish it again after this boundary.
        let healthy: Result<(), String> = listener_policy_result_transaction(
            &slot,
            |_| {
                assert!(matches!(
                    slot.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ));
                *lock_recover(&last_error) = None;
                Ok(())
            },
            |_| panic!("successful transaction must not publish an error"),
        );
        assert!(healthy.is_ok());
        assert!(old_result.is_err());
        assert_eq!(calls.get(), 1);
        assert!(lock_recover(&last_error).is_none());
        assert!(slot.try_lock().is_ok());
    }

    #[test]
    fn local_listener_policy_refuses_stale_start_after_completed_disable() {
        let slot = Mutex::new(None);
        let last_error = Mutex::new(Some("preserved diagnostic".to_string()));
        let served = AtomicBool::new(true);
        let enabled = AtomicBool::new(true);
        let factories = std::sync::atomic::AtomicUsize::new(0);
        // Deterministic scheduling: the contender passed the old outer preflight,
        // then a complete disable transaction linearized before its slot entry.
        assert!(enabled.load(Ordering::Acquire));
        listener_policy_transaction(&slot, |running| {
            enabled.store(false, Ordering::Release);
            stop_listener_slot_locked_for_policy_fixture(running, &last_error);
        });
        let result = listener_policy_transaction(&slot, |running| {
            start_listener_slot_locked(
                running,
                &last_error,
                &served,
                || enabled.load(Ordering::Acquire),
                || {
                    factories.fetch_add(1, Ordering::AcqRel);
                    Err(LocalAccessFailure::other("obsolete factory ran"))
                },
            )
        });
        assert_eq!(
            factories.load(Ordering::Acquire),
            0,
            "completed disable must refuse the stale factory before any listener effects"
        );
        assert!(result.is_ok());
        assert!(!enabled.load(Ordering::Acquire));
        assert!(lock_recover(&slot).is_none());
        assert_eq!(
            lock_recover(&last_error).as_deref(),
            Some("preserved diagnostic")
        );
        assert!(served.load(Ordering::Acquire));

        // A permitted fresh start still executes the real factory/error path.
        enabled.store(true, Ordering::Release);
        let result = listener_policy_transaction(&slot, |running| {
            start_listener_slot_locked(
                running,
                &last_error,
                &served,
                || true,
                || {
                    factories.fetch_add(1, Ordering::AcqRel);
                    Err(LocalAccessFailure::other("healthy admitted factory"))
                },
            )
        });
        assert_eq!(factories.load(Ordering::Acquire), 1);
        assert_eq!(result.unwrap_err(), "healthy admitted factory");
        assert_eq!(
            lock_recover(&last_error).as_deref(),
            Some("healthy admitted factory")
        );
        assert!(!served.load(Ordering::Acquire));
    }

    // Keep the fixture's disable step on the existing exact stop transaction on
    // the baseline; production source supplies its same locked core afterward.
    fn stop_listener_slot_locked_for_policy_fixture(
        running: &mut Option<RunningLocalAccess>,
        last_error: &Mutex<Option<String>>,
    ) {
        let slot = Mutex::new(running.take());
        stop_listener_slot(&slot, last_error);
        *running = lock_recover(&slot).take();
    }

    #[test]
    fn local_listener_policy_reads_current_preference_inside_serialized_slot() {
        let slot = Mutex::new(None);
        let enabled = AtomicBool::new(true);
        let persisted = AtomicBool::new(true);
        let served = AtomicBool::new(false);
        let last_error = Mutex::new(None);
        let factories = std::sync::atomic::AtomicUsize::new(0);
        // A previously scheduled observer saw "on"; it must not retain that
        // value across a completed disable/persistence transaction.
        assert!(persisted.load(Ordering::Acquire));
        listener_policy_transaction(&slot, |_| {
            enabled.store(false, Ordering::Release);
            persisted.store(false, Ordering::Release);
        });
        reconcile_listener_slot(
            &slot, &last_error, &served, &enabled,
            || {
                assert!(matches!(slot.try_lock(), Err(std::sync::TryLockError::WouldBlock)),
                    "the ACTUAL reconciliation core must acquire the running-slot lock before reading policy");
                (persisted.load(Ordering::Acquire), true)
            },
            || {
                factories.fetch_add(1, Ordering::AcqRel);
                Err(LocalAccessFailure::other("stale reconcile factory"))
            },
        ).unwrap();
        assert_eq!(factories.load(Ordering::Acquire), 0);
        assert!(!enabled.load(Ordering::Acquire));
        assert!(!persisted.load(Ordering::Acquire));
        assert!(lock_recover(&slot).is_none());
    }

    #[test]
    fn local_http_connection_headroom_is_bounded_owned_and_recovers_after_completion() {
        use qc_native_platform::thread_owner::{Budget, Owned};
        use std::io::{Read, Write};
        static BUDGET: Budget = Budget::new(1);
        struct Finish {
            stop: watch::Sender<bool>,
            release: watch::Sender<bool>,
            original: Option<Owned>,
        }
        impl Drop for Finish {
            fn drop(&mut self) {
                let _ = self.release.send(true);
                let _ = self.stop.send(true);
                if self
                    .original
                    .as_mut()
                    .is_some_and(|actor| !actor.wait_within(Duration::from_secs(3)))
                {
                    eprintln!("HTTP connection fixture exact original remains retained");
                }
            }
        }
        let permit = BUDGET.reserve().unwrap();
        let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, mut stopped) = watch::channel(false);
        let (release, released) = watch::channel(false);
        let (entered, observed) = mpsc::channel();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_actor = completed.clone();
        let router = Router::new()
            .route(
                "/held",
                get(move || {
                    let entered = entered.clone();
                    let mut released = released.clone();
                    async move {
                        let _ = entered.send(());
                        while !*released.borrow_and_update() {
                            if released.changed().await.is_err() {
                                break;
                            }
                        }
                        "released"
                    }
                }),
            )
            .route("/ready", get(|| async { "ready" }));
        let original =
            Owned::spawn_reserved("local-http-headroom-fixture", permit, (), move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    serve_loopback_with_limit(
                        listener,
                        router,
                        Duration::from_secs(10),
                        async move {
                            while !*stopped.borrow_and_update() {
                                if stopped.changed().await.is_err() {
                                    break;
                                }
                            }
                        },
                        2,
                    )
                    .await
                    .unwrap();
                });
                completed_actor.store(true, Ordering::Release);
            })
            .unwrap();
        // Own the original and both release gates before any fallible wait.
        let finish = Finish {
            stop,
            release,
            original: Some(original),
        };
        let connect = || {
            let stream =
                std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
        };
        let mut first = connect();
        first
            .write_all(b"GET /held HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut second = connect();
        second
            .write_all(b"GET /held HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut excess = connect();
        let mut byte = [0_u8; 1];
        let closed = excess.read(&mut byte);
        assert!(
            matches!(&closed, Ok(0))
                || matches!(
                    &closed, Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset
                ),
            "excess connection must close, not retain a third task"
        );
        assert!(observed.try_recv().is_err());
        finish.release.send(true).unwrap();
        for stream in [&mut first, &mut second] {
            let mut response = Vec::new();
            stream.take(4096).read_to_end(&mut response).unwrap();
            assert!(String::from_utf8_lossy(&response).contains("released"));
        }
        // FIN observation can precede the connection task's final return.
        // Poll only this harmless read until actual capacity is released;
        // this does not retry a write or reset the observation deadline.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let mut healthy = connect();
            let mut response = Vec::new();
            let answered = healthy
                .write_all(b"GET /ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .is_ok()
                && healthy.take(4096).read_to_end(&mut response).is_ok()
                && String::from_utf8_lossy(&response).contains("ready");
            if answered {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "completed connections did not release headroom"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(finish);
        assert!(completed.load(Ordering::Acquire));
        assert_eq!(
            BUDGET.count(),
            0,
            "actual listener original and its owned connection tasks retired"
        );
    }
    use axum::{body::to_bytes, http::Request};
    use qc_relay_client::DeviceError;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tower::ServiceExt;

    struct OwnedTestDirectory(PathBuf);
    type ListenerExitHook = Arc<dyn Fn() + Send + Sync>;
    static LISTENER_EXIT_HOOKS: Mutex<Vec<(PathBuf, ListenerExitHook)>> = Mutex::new(Vec::new());
    pub(super) fn run_listener_exit_hook(path: Option<&Path>) {
        let hook = lock_recover(&LISTENER_EXIT_HOOKS)
            .iter()
            .find(|(owned, _)| Some(owned.as_path()) == path)
            .map(|(_, hook)| hook.clone());
        if let Some(hook) = hook {
            hook();
        }
    }
    #[cfg(windows)]
    struct ListenerExitHookGuard(PathBuf);
    #[cfg(windows)]
    impl Drop for ListenerExitHookGuard {
        fn drop(&mut self) {
            lock_recover(&LISTENER_EXIT_HOOKS).retain(|(path, _)| path != &self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn actual_listener_native_tls_retains_exact_original_publication_lease_and_capacity_until_join()
    {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        static ROOT_BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        static RETAINED: Mutex<
            Vec<(
                RunningLocalAccess,
                PathBuf,
                qc_native_platform::thread_owner::Reservation,
            )>,
        > = Mutex::new(Vec::new());
        struct TlsGate {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Drop for TlsGate {
            fn drop(&mut self) {
                let _ = self.entered.send(());
                let _ = self.release.recv_timeout(Duration::from_secs(30));
            }
        }
        thread_local! { static TLS: std::cell::RefCell<Option<TlsGate>> = const { std::cell::RefCell::new(None) }; }
        struct Cleanup {
            root: PathBuf,
            release: Option<mpsc::Sender<()>>,
            hook: Option<ListenerExitHookGuard>,
            listener: Option<RunningLocalAccess>,
            supervisor_slot: Option<Arc<Mutex<Option<RunningLocalAccess>>>>,
            root_permit: Option<qc_native_platform::thread_owner::Reservation>,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                self.hook.take();
                if let Some(slot) = self.supervisor_slot.take() {
                    self.listener = lock_recover(&slot).take();
                }
                if let Some(listener) = self.listener.take() {
                    let _ = listener.stop();
                    let deadline = std::time::Instant::now() + Duration::from_secs(3);
                    while !listener.native_finished() && std::time::Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    if listener.native_finished() {
                        let _ = listener.join_original();
                    } else {
                        lock_recover(&RETAINED).push((
                            listener,
                            self.root.clone(),
                            self.root_permit.take().unwrap(),
                        ));
                        eprintln!(
                            "retaining exact listener fixture original/root/admission after unknown native retirement: {}",
                            self.root.display()
                        );
                        return;
                    }
                }
                let _ = fs::remove_dir_all(&self.root);
                self.root_permit.take();
            }
        }
        let retained = std::mem::take(&mut *lock_recover(&RETAINED));
        for (listener, root, permit) in retained {
            if listener.native_finished() {
                let _ = listener.join_original();
                let _ = fs::remove_dir_all(root);
                drop(permit);
            } else {
                lock_recover(&RETAINED).push((listener, root, permit));
            }
        }
        let root_permit = ROOT_BUDGET.reserve().unwrap();
        let root = scratch_directory("actual-listener-tls");
        let path = root.join(STATE_FILE_NAME);
        let (entered, observed) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let held = Mutex::new(Some(held));
        let hook: ListenerExitHook = Arc::new(move || {
            TLS.with(|slot| {
                *slot.borrow_mut() = Some(TlsGate {
                    entered: entered.clone(),
                    release: held.lock().unwrap().take().unwrap(),
                })
            });
        });
        let mut cleanup = Cleanup {
            root: root.clone(),
            release: Some(release),
            hook: None,
            listener: None,
            supervisor_slot: None,
            root_permit: Some(root_permit),
        };
        lock_recover(&LISTENER_EXIT_HOOKS).push((path.clone(), hook));
        cleanup.hook = Some(ListenerExitHookGuard(path.clone()));
        cleanup.listener = Some(
            super::start_listener_with_budget(
                vec![path.clone()],
                Arc::new(FakeAdapter {
                    ready: AtomicBool::new(false),
                }),
                full_access(),
                &BUDGET,
            )
            .unwrap(),
        );
        let started = std::time::Instant::now();
        assert!(
            !cleanup.listener.as_ref().unwrap().stop(),
            "runtime acknowledgement alone cannot certify native retirement"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!cleanup.listener.as_ref().unwrap().native_finished());
        assert_eq!(BUDGET.count(), 1);
        assert!(
            acquire_document_lease(&path, "publisher", Duration::ZERO).is_err(),
            "exact old publisher retains cross-process lease"
        );
        let refused = root.join("refused").join(STATE_FILE_NAME);
        assert!(super::start_listener_with_budget(
            vec![refused.clone()],
            Arc::new(FakeAdapter {
                ready: AtomicBool::new(false)
            }),
            full_access(),
            &BUDGET,
        )
        .is_err());
        assert!(
            !refused.parent().unwrap().exists(),
            "refusal precedes every path/bind effect"
        );
        let slot = Arc::new(Mutex::new(cleanup.listener.take()));
        cleanup.supervisor_slot = Some(slot.clone());
        let last_error = Mutex::new(None);
        let served_by_other = AtomicBool::new(true);
        let factories = AtomicU64::new(0);
        // Exact production transactions called by LocalAccess::stop_now and
        // start_now; no Wry app or copied test-only state machine is created.
        stop_listener_slot(&slot, &last_error);
        assert!(lock_recover(&slot).is_some());
        assert!(lock_recover(&last_error)
            .as_deref()
            .unwrap()
            .contains("not acknowledged"));
        let error = start_listener_slot(&slot, &last_error, &served_by_other, || {
            factories.fetch_add(1, Ordering::AcqRel);
            panic!("a retiring original must refuse before factory admission")
        })
        .unwrap_err();
        assert!(error.contains("still retiring"));
        assert_eq!(factories.load(Ordering::Acquire), 0);
        assert!(!served_by_other.load(Ordering::Acquire));
        cleanup.release.take().unwrap().send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !lock_recover(&slot).as_ref().unwrap().native_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "exact native TLS original did not retire"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        cleanup.hook.take();
        let error = start_listener_slot(&slot, &last_error, &served_by_other, || {
            assert!(
                slot.try_lock().is_err(),
                "startup factory remains serialized under its original slot lock"
            );
            factories.fetch_add(1, Ordering::AcqRel);
            Err(LocalAccessFailure {
                message: "actual factory refusal fixture".into(),
                served_by_another_instance: true,
            })
        })
        .unwrap_err();
        assert_eq!(error, "actual factory refusal fixture");
        assert_eq!(
            lock_recover(&last_error).as_deref(),
            Some("actual factory refusal fixture")
        );
        assert!(served_by_other.load(Ordering::Acquire));
        assert!(lock_recover(&slot).is_none());
        assert_eq!(
            BUDGET.count(),
            0,
            "finished predecessor was actually joined before replacement factory"
        );
        start_listener_slot(&slot, &last_error, &served_by_other, || {
            assert!(slot.try_lock().is_err());
            factories.fetch_add(1, Ordering::AcqRel);
            super::start_listener_with_budget(
                vec![path],
                Arc::new(FakeAdapter {
                    ready: AtomicBool::new(false),
                }),
                full_access(),
                &BUDGET,
            )
        })
        .unwrap();
        assert!(lock_recover(&last_error).is_none());
        assert!(!served_by_other.load(Ordering::Acquire));
        assert_eq!(factories.load(Ordering::Acquire), 2);
        start_listener_slot(&slot, &last_error, &served_by_other, || {
            panic!("healthy existing listener does not run another factory")
        })
        .unwrap();
        stop_listener_slot(&slot, &last_error);
        assert!(lock_recover(&slot).is_none());
        assert_eq!(BUDGET.count(), 0);
    }
    impl Drop for OwnedTestDirectory {
        fn drop(&mut self) {
            if stop_fixture_listeners_beneath(&self.0) && fixture_readers_retired_beneath(&self.0) {
                let _ = fs::remove_dir_all(&self.0);
            } else {
                eprintln!(
                    "local MCP fixture retained its exact listener and root after unknown retirement: {}",
                    self.0.display()
                );
            }
        }
    }
    // Test-only strong ownership is acquired before the caller can assert.
    // An unwound caller cannot detach a live publisher/JoinHandle.
    static FIXTURE_LISTENERS_IN_USE: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    struct FixtureListenerPermit;
    impl Drop for FixtureListenerPermit {
        fn drop(&mut self) {
            FIXTURE_LISTENERS_IN_USE.fetch_sub(1, Ordering::AcqRel);
        }
    }
    struct TestRunningLocalAccess {
        running: RunningLocalAccess,
        _permit: FixtureListenerPermit,
    }
    impl std::ops::Deref for TestRunningLocalAccess {
        type Target = RunningLocalAccess;
        fn deref(&self) -> &RunningLocalAccess {
            &self.running
        }
    }
    static TEST_LISTENERS: Mutex<Vec<Arc<TestRunningLocalAccess>>> = Mutex::new(Vec::new());
    fn reap_fixture_listeners() {
        let mut finished = Vec::new();
        let mut listeners = TEST_LISTENERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut index = 0;
        while index < listeners.len() {
            if listeners[index].native_finished() && Arc::strong_count(&listeners[index]) == 1 {
                let listener = listeners.swap_remove(index);
                finished.push(listener);
            } else {
                index += 1;
            }
        }
        drop(listeners);
        for listener in finished {
            let TestRunningLocalAccess { running, _permit } = Arc::try_unwrap(listener)
                .ok()
                .expect("unique finished fixture listener");
            let _ = running.join_original();
            drop(_permit);
        }
    }
    fn stop_fixture_listeners_beneath(root: &Path) -> bool {
        let acknowledged = {
            let listeners: Vec<_> = TEST_LISTENERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter(|listener| {
                    listener
                        .state_files
                        .iter()
                        .any(|path| path.starts_with(root))
                })
                .cloned()
                .collect();
            // Visit every listener even when an earlier one cannot acknowledge.
            listeners.iter().fold(true, |all, listener| {
                let stopped = listener.stop() || listener.native_finished();
                stopped && all
            })
        };
        reap_fixture_listeners();
        acknowledged
    }
    // Shadows the production name ONLY within this test module. All calls
    // still invoke the real production listener, then retain its exact owner.
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    fn start_listener(
        state_files: Vec<PathBuf>,
        adapter: Arc<dyn DeviceAdapter>,
        access: watch::Receiver<AccessMode>,
    ) -> Result<Arc<TestRunningLocalAccess>, LocalAccessFailure> {
        reap_fixture_listeners();
        if FIXTURE_LISTENERS_IN_USE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 64).then_some(count + 1)
            })
            .is_err()
        {
            return Err(LocalAccessFailure {
                message: "Fixture listener capacity reached; no new listener was admitted".into(),
                served_by_another_instance: false,
            });
        }
        let permit = FixtureListenerPermit;
        let listener = Arc::new(TestRunningLocalAccess {
            running: super::start_listener(state_files, adapter, access)?,
            _permit: permit,
        });
        TEST_LISTENERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(listener.clone());
        Ok(listener)
    }
    struct OwnedTestListener(Arc<TestRunningLocalAccess>);
    impl Drop for OwnedTestListener {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    struct FixtureStartRelease(Arc<AtomicBool>);
    impl Drop for FixtureStartRelease {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    type ReaderAnswer = (u32, u32, u32);
    static FIXTURE_READERS_IN_USE: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    struct FixtureReaderPermit;
    impl FixtureReaderPermit {
        #[allow(
            deprecated,
            reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
        )]
        fn acquire() -> Self {
            assert!(
                FIXTURE_READERS_IN_USE
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| (count < 4)
                        .then_some(count + 1))
                    .is_ok(),
                "fixture reader capacity reached; no new reader admitted"
            );
            Self
        }
    }
    impl Drop for FixtureReaderPermit {
        fn drop(&mut self) {
            FIXTURE_READERS_IN_USE.fetch_sub(1, Ordering::AcqRel);
        }
    }
    static RETAINED_FIXTURE_READERS: Mutex<
        Vec<(
            PathBuf,
            std::thread::JoinHandle<ReaderAnswer>,
            FixtureReaderPermit,
        )>,
    > = Mutex::new(Vec::new());
    fn fixture_readers_retired_beneath(root: &Path) -> bool {
        let mut finished = Vec::new();
        let mut readers = RETAINED_FIXTURE_READERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut index = 0;
        while index < readers.len() {
            if qc_native_platform::process::thread_signalled(&readers[index].1) {
                finished.push(readers.swap_remove(index));
            } else {
                index += 1;
            }
        }
        let retired = !readers.iter().any(|(directory, _, _)| directory == root);
        drop(readers);
        for (_, reader, permit) in finished {
            let _ = reader.join();
            drop(permit);
        }
        retired
    }
    struct FixtureReader {
        root: PathBuf,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<ReaderAnswer>>,
        permit: Option<FixtureReaderPermit>,
    }
    impl FixtureReader {
        fn finish(&mut self) -> Result<ReaderAnswer, String> {
            self.stop.store(true, Ordering::Release);
            let Some(worker) = self.worker.take() else {
                return Err("reader already settled".into());
            };
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !qc_native_platform::process::thread_signalled(&worker)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            if qc_native_platform::process::thread_signalled(&worker) {
                let result = worker
                    .join()
                    .map_err(|_| "descriptor reader panicked".into());
                self.permit.take();
                result
            } else {
                RETAINED_FIXTURE_READERS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((
                        self.root.clone(),
                        worker,
                        self.permit.take().expect("owned reader permit"),
                    ));
                Err("descriptor reader retirement unknown; exact thread and root retained".into())
            }
        }
    }
    impl Drop for FixtureReader {
        fn drop(&mut self) {
            if self.worker.is_some() {
                if let Err(error) = self.finish() {
                    eprintln!("local MCP fixture cleanup: {error}");
                }
            }
        }
    }

    #[test]
    fn live_staging_claim_survives_other_publisher_sweep_and_failure_cleans_ours() {
        let directory = scratch_directory("live-stage");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(STATE_FILE_NAME);
        let stage_path = directory.join(format!(".{STATE_FILE_NAME}.123.0.tmp"));
        let mut stage = StagedDocument(Some(
            qc_owner_only_file::ClaimedFile::try_claim(&stage_path).unwrap(),
        ));
        stage.0.as_mut().unwrap().write(b"live credential").unwrap();
        test_descriptor("new").publish(&path).unwrap();
        assert_eq!(fs::read(&stage_path).unwrap(), b"live credential");
        drop(stage);
        assert!(!stage_path.exists());
        assert!(!qc_owner_only_file::claim_path(&stage_path).exists());
        let bad_target = directory.join("not-a-file");
        fs::create_dir(&bad_target).unwrap();
        assert!(publish_atomically(&bad_target, &directory, "test", b"payload").is_err());
        assert!(!fs::read_dir(&directory).unwrap().flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(".not-a-file.")
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    struct TestHistoryWorker {
        writer: Option<HistoryWriter>,
        release: Option<mpsc::SyncSender<()>>,
    }
    impl Drop for TestHistoryWorker {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
            if let Some(writer) = self.writer.take() {
                let _ = writer.sender.send(HistoryCommand::Stop);
                drop(writer.sender);
                let _ = writer.task.join();
            }
        }
    }

    #[test]
    fn owned_history_worker_coalesces_without_blocking_state_and_joins_latest_on_exit() {
        let (entered, observed) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let blocked = Mutex::new(blocked);
        let writes = Arc::new(Mutex::new(Vec::new()));
        let records = writes.clone();
        let state = Arc::new(Mutex::new((1_u64, true)));
        let completion = state.clone();
        let writer = start_history_writer(
            move |snapshot| {
                if snapshot.generation == 1 {
                    entered.send(()).unwrap();
                    blocked.lock().unwrap().recv().unwrap();
                }
                records.lock().unwrap().push(snapshot.generation);
                Ok(())
            },
            move |snapshot, result| {
                let mut state = completion.lock().unwrap();
                if result.is_ok() && state.0 == snapshot.generation {
                    state.1 = false;
                }
            },
        )
        .unwrap();
        let mut owned = TestHistoryWorker {
            writer: Some(writer),
            release: Some(release),
        };
        let send = |generation| {
            HistoryCommand::Write(HistoryWrite {
                path: PathBuf::from("unused"),
                clients: BTreeMap::new(),
                generation,
            })
        };
        owned.writer.as_ref().unwrap().sender.send(send(1)).unwrap();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        // A stalled disk worker owns no state/status mutex or listener thread.
        *state
            .try_lock()
            .expect("heartbeat/status remains responsive") = (3, true);
        owned.writer.as_ref().unwrap().sender.send(send(2)).unwrap();
        owned.writer.as_ref().unwrap().sender.send(send(3)).unwrap();
        owned.release.take().unwrap().send(()).unwrap();
        let writer = owned.writer.take().unwrap();
        writer.sender.send(HistoryCommand::Stop).unwrap();
        writer.task.join().unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![1, 3]);
        assert_eq!(*state.lock().unwrap(), (3, false));
    }

    #[test]
    fn owned_history_worker_failed_commit_remains_owed_and_stops_cleanly() {
        let state = Arc::new(Mutex::new(true));
        let completion = state.clone();
        let writer = start_history_writer(
            |_| Err("disk unavailable".into()),
            move |_, result| {
                *completion.lock().unwrap() = result.is_err();
            },
        )
        .unwrap();
        let mut owned = TestHistoryWorker {
            writer: Some(writer),
            release: None,
        };
        let writer = owned.writer.take().unwrap();
        writer
            .sender
            .send(HistoryCommand::Write(HistoryWrite {
                path: PathBuf::from("unused"),
                clients: BTreeMap::new(),
                generation: 1,
            }))
            .unwrap();
        writer.sender.send(HistoryCommand::Stop).unwrap();
        writer.task.join().unwrap();
        assert!(*state.lock().unwrap());
    }

    #[test]
    fn poisoned_history_queue_recovers_latest_flush_and_joins_owned_worker() {
        let (completed, observed) = mpsc::sync_channel(1);
        let writer = start_history_writer(
            |_| Ok(()),
            move |snapshot, result| {
                completed
                    .send((snapshot.generation, result.is_ok()))
                    .unwrap();
            },
        )
        .unwrap();
        let mut owned = TestHistoryWorker {
            writer: Some(writer),
            release: None,
        };
        let queue = owned.writer.as_ref().unwrap().sender.queue.clone();
        assert!(std::panic::catch_unwind(move || {
            let mut queue = queue.lock().unwrap();
            queue.latest = Some(HistoryWrite {
                path: PathBuf::from("unused"),
                clients: BTreeMap::new(),
                generation: 1,
            });
            panic!("injected producer panic");
        })
        .is_err());
        let writer = owned.writer.as_ref().unwrap();
        writer
            .sender
            .send(HistoryCommand::Write(HistoryWrite {
                path: PathBuf::from("unused"),
                clients: BTreeMap::new(),
                generation: 2,
            }))
            .unwrap();
        writer.sender.send(HistoryCommand::Stop).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(2)).unwrap(),
            (2, true)
        );
        let writer = owned.writer.take().unwrap();
        drop(writer.sender);
        writer.task.join().unwrap();
    }

    #[test]
    fn history_forward_version_fence_uses_semver_precedence_not_lexical_or_build_metadata() {
        assert!(client_id_is_newer("claude:1.0.0", "1.0.0-rc.9"));
        assert!(client_id_is_newer("claude:1.0.0-rc.10", "1.0.0-rc.9"));
        assert!(client_id_is_newer("claude:1.10.0", "1.9.0"));
        assert!(!client_id_is_newer("claude:1.0.0-rc.1", "1.0.0"));
        assert!(!client_id_is_newer(
            "claude:1.0.0+newer-build",
            "1.0.0+older-build"
        ));
        for invalid in [
            "claude:garbage",
            "claude:1.02.0",
            "claude:1.0",
            ":2.0.0",
            "claude\n:2.0.0",
        ] {
            assert!(!client_id_is_newer(invalid, "1.0.0"));
        }
        assert!(!client_id_is_newer("claude:2.0.0", "invalid-current"));
    }

    #[test]
    fn old_history_writer_preserves_successor_disk_ids_without_exposing_them_as_current() {
        let directory = scratch_directory("successor-history");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(CLIENT_HISTORY_FILE_NAME);
        let mut future = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        future.major += 1;
        future.pre = semver::Prerelease::EMPTY;
        future.build = semver::BuildMetadata::EMPTY;
        let successor = format!("claude:{future}");
        fs::write(
            &path,
            serde_json::to_vec(&ClientAttachHistoryFile {
                schema_version: CLIENT_HISTORY_SCHEMA_VERSION,
                clients: BTreeMap::from([
                    (successor.clone(), 500),
                    ("claude:0.0.1".into(), 99),
                    ("claude:invalid".into(), 999),
                ]),
            })
            .unwrap(),
        )
        .unwrap();
        let current = current_client_id("jan");
        store_attach_history(&path, &BTreeMap::from([(current.clone(), 300)])).unwrap();
        let durable = load_attach_history_document(&path);
        assert_eq!(durable[&successor], 500);
        assert_eq!(durable[&current], 300);
        assert!(!durable.contains_key("claude:0.0.1"));
        assert!(!durable.contains_key("claude:invalid"));
        let reader = load_attach_history(&path);
        assert_eq!(reader, BTreeMap::from([(current, 300)]));
        assert!(
            !client_id_is_current(&successor),
            "future history cannot feed current liveness"
        );
    }

    #[test]
    #[ignore = "owned subprocess fixture, invoked only by document_leases_and_history_merge_are_cross_process"]
    fn document_lease_process_fixture() {
        let path = PathBuf::from(std::env::var_os("QC_LOCAL_LEASE_FIXTURE").unwrap());
        if std::env::var_os("QC_LOCAL_LEASE_BUSY").is_some() {
            assert!(acquire_document_lease(&path, "publisher", Duration::ZERO).is_err());
        } else {
            let clients = BTreeMap::from([(current_client_id("jan"), 200)]);
            store_attach_history(&path, &clients).unwrap();
        }
    }

    #[test]
    fn document_leases_and_history_merge_are_cross_process() {
        let directory = scratch_directory("multiprocess-lease");
        let directory_owner = Arc::new(OwnedTestDirectory(directory.clone()));
        let path = directory.join(STATE_FILE_NAME);
        let invoke = |busy| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "local_access::tests::document_lease_process_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("QC_LOCAL_LEASE_FIXTURE", &path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
            }
            if busy {
                command.env("QC_LOCAL_LEASE_BUSY", "1");
            } else {
                command.env_remove("QC_LOCAL_LEASE_BUSY");
            }
            let output = crate::tests::run_owned_test_binary_fixture_with_resource(
                &mut command,
                directory_owner.clone(),
            )
            .expect("owned lease fixture completed within its budget");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let lease = acquire_document_lease(&path, "publisher", Duration::ZERO).unwrap();
        invoke(true);
        drop(lease);
        let after_death = acquire_document_lease(&path, "publisher", Duration::ZERO).unwrap();
        drop(after_death);
        store_attach_history(&path, &BTreeMap::from([(current_client_id("claude"), 300)])).unwrap();
        invoke(false);
        // A delayed old snapshot cannot erase another process's newer facts.
        store_attach_history(&path, &BTreeMap::from([(current_client_id("claude"), 100)])).unwrap();
        let merged = load_attach_history(&path);
        assert_eq!(merged[&current_client_id("claude")], 300);
        assert_eq!(merged[&current_client_id("jan")], 200);
        drop(directory_owner);
        assert!(
            !directory.exists(),
            "acknowledged fixture originals released the root"
        );
    }

    struct FakeAdapter {
        ready: AtomicBool,
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn local_http_managed_presence_requires_initialize_and_is_per_session_bounded_activity() {
        use qc_remote_mcp::ClientPresence as _;
        let first = DirectCodexPresence::new();
        let second = DirectCodexPresence::new();
        first.active();
        assert!(!DIRECT_CODEX_ACTIVITY
            .get()
            .is_some_and(|rows| lock_recover(rows).contains_key(&first.id)));
        let first_id = first.id.clone();
        let second_id = second.id.clone();
        first.initialized(&rmcp::model::Implementation::new(
            "managed-route-fixture-a",
            "0",
        ));
        second.initialized(&rmcp::model::Implementation::new(
            "managed-route-fixture-b",
            "0",
        ));
        assert!(lock_recover(DIRECT_CODEX_ACTIVITY.get().unwrap()).contains_key(&first_id));
        drop(first);
        let rows = lock_recover(DIRECT_CODEX_ACTIVITY.get().unwrap());
        assert!(!rows.contains_key(&first_id));
        assert!(rows.contains_key(&second_id));
        drop(rows);
        *lock_recover(DIRECT_CODEX_ACTIVITY.get().unwrap())
            .get_mut(&second_id)
            .unwrap() = unix_time_ms().saturating_sub(13_000);
        local_client_activity_snapshot();
        assert!(
            !lock_recover(DIRECT_CODEX_ACTIVITY.get().unwrap()).contains_key(&second_id),
            "idle HTTP is not a fabricated persistent heartbeat"
        );
        assert!(
            local_client_attached_before(concat!("codex:", env!("CARGO_PKG_VERSION"))),
            "history survives session expiry"
        );
        second.active();
        assert!(lock_recover(DIRECT_CODEX_ACTIVITY.get().unwrap()).contains_key(&second_id));
        drop(second);
        assert!(!lock_recover(DIRECT_CODEX_ACTIVITY.get().unwrap()).contains_key(&second_id));
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn local_http_activity_metadata_is_capped_even_when_one_connection_initializes_many_sessions() {
        // Isolate the production insertion policy; flooding the global map
        // could evict concurrently-running lifecycle fixture rows.
        let mut rows = HashMap::new();
        for index in 0..MAX_LOCAL_HTTP_CONNECTIONS + 1 {
            record_direct_activity(&mut rows, &format!("fixture:{index}"), 1000 + index as u64);
            assert!(rows.len() <= MAX_LOCAL_HTTP_CONNECTIONS);
        }
        assert!(!rows.contains_key("fixture:0"));
        assert_eq!(rows.len(), MAX_LOCAL_HTTP_CONNECTIONS);
        record_direct_activity(&mut rows, "fixture:1", 2000);
        assert_eq!(
            rows.len(),
            MAX_LOCAL_HTTP_CONNECTIONS,
            "existing activity updates don't evict another session"
        );
        record_direct_activity(&mut rows, "fresh", 20_000);
        assert_eq!(
            rows.len(),
            1,
            "expired metadata pruned, without affecting MCP/session state"
        );
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_http_late_pair_refusal_keeps_legacy_and_explicit_retry_rearms_only_direct() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        let directory = scratch_directory("direct-late-pair");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![directory.join(STATE_FILE_NAME)];
        let running = super::start_listener_inner(
            paths.clone(),
            Arc::new(FakeAdapter {
                ready: AtomicBool::new(false),
            }),
            full_access(),
            &BUDGET,
            Some(directory.clone()),
        )
        .unwrap();
        let endpoint = crate::local_stdio::descriptor_from_paths(&paths).unwrap();
        let stranger = StdTcpListener::bind("127.0.0.1:0").unwrap();
        let port = stranger.local_addr().unwrap().port();
        let record = directory.join(crate::local_http_helper::PORT_FILE);
        qc_owner_only_file::publish_atomically(
            &record,
            serde_json::to_vec(
                &serde_json::json!({"schemaVersion":1,"port":port,"pairingRevision":1}),
            )
            .unwrap()
            .as_slice(),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let client = crate::loopback_http::loopback_client_builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        endpoint
            .prove_identity(&client)
            .await
            .expect("late Pair failure did not stop legacy audience");
        assert!(crate::local_http_helper::verified_headers(&directory)
            .await
            .is_err());
        drop(stranger);
        // No timer retry after failure: a new explicit Pair is required.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!directory
            .join(crate::local_http_helper::DESCRIPTOR_FILE)
            .exists());
        crate::local_http_binding::StablePort::select_for_pairing(&record).unwrap();
        let direct_headers = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(headers) = crate::local_http_helper::verified_headers(&directory).await {
                    break headers;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let direct_token = direct_headers["Authorization"].as_str().unwrap();
        let refused = client
            .get(&endpoint.health_url)
            .header(AUTHORIZATION, direct_token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            refused.status(),
            StatusCode::UNAUTHORIZED,
            "new token cannot authenticate to legacy audience"
        );
        let refused = client
            .post(format!("http://127.0.0.1:{port}/mcp"))
            .bearer_auth(&endpoint.bearer_token)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(
            refused.status(),
            StatusCode::UNAUTHORIZED,
            "legacy token cannot authenticate to new audience"
        );
        endpoint.prove_identity(&client).await.unwrap();
        assert!(running.stop());
        assert!(!directory
            .join(crate::local_http_helper::DESCRIPTOR_FILE)
            .exists());
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_http_startup_refusal_preserves_legacy_for_malformed_and_occupied_pairing() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        for malformed in [true, false] {
            let directory = scratch_directory("direct-startup-refusal");
            let _directory = OwnedTestDirectory(directory.clone());
            let stranger = StdTcpListener::bind("127.0.0.1:0").unwrap();
            let address = stranger.local_addr().unwrap();
            let bytes = if malformed {
                b"not valid port metadata".to_vec()
            } else {
                serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"port":address.port(),"pairingRevision":1})).unwrap()
            };
            qc_owner_only_file::publish_atomically(
                &directory.join(crate::local_http_helper::PORT_FILE),
                &bytes,
            )
            .unwrap();
            let paths = vec![directory.join(STATE_FILE_NAME)];
            let running = super::start_listener_inner(
                paths.clone(),
                Arc::new(FakeAdapter {
                    ready: AtomicBool::new(false),
                }),
                full_access(),
                &BUDGET,
                Some(directory.clone()),
            )
            .expect("compatibility host still starts");
            let endpoint = crate::local_stdio::descriptor_from_paths(&paths).unwrap();
            let client = crate::loopback_http::loopback_client_builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            endpoint
                .prove_identity(&client)
                .await
                .expect("legacy authentication remains healthy");
            assert!(!directory
                .join(crate::local_http_helper::DESCRIPTOR_FILE)
                .exists());
            assert_eq!(
                stranger.local_addr().unwrap(),
                address,
                "foreign listener never displaced"
            );
            assert!(running.stop());
        }
    }

    #[async_trait]
    impl DeviceAdapter for FakeAdapter {
        async fn ready(&self) -> bool {
            self.ready.load(Ordering::Relaxed)
        }

        async fn invoke(&self, _method: &str, _params: Value) -> Result<Value, DeviceError> {
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    struct ErrorAdapter {
        error: DeviceError,
    }

    #[async_trait]
    impl DeviceAdapter for ErrorAdapter {
        async fn ready(&self) -> bool {
            true
        }

        async fn invoke(&self, _method: &str, _params: Value) -> Result<Value, DeviceError> {
            Err(self.error.clone())
        }
    }

    /// Records what the round trip actually asked the device for, so
    /// "read-only, one request" is proved rather than asserted in a comment.
    struct RecordingAdapter {
        device_connected: AtomicBool,
        invoked: Mutex<Vec<String>>,
    }

    impl RecordingAdapter {
        fn new(device_connected: bool) -> Self {
            Self {
                device_connected: AtomicBool::new(device_connected),
                invoked: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.invoked.lock().expect("recorded calls").clone()
        }
    }

    struct EffectivePolicyAdapter {
        mode: std::sync::atomic::AtomicUsize,
        invoked: Mutex<Vec<String>>,
    }

    impl EffectivePolicyAdapter {
        fn new(mode: AccessMode) -> Self {
            Self {
                mode: std::sync::atomic::AtomicUsize::new(Self::encode(mode)),
                invoked: Mutex::new(Vec::new()),
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
    }

    #[async_trait]
    impl DeviceAdapter for EffectivePolicyAdapter {
        async fn ready(&self) -> bool {
            true
        }

        async fn invoke(&self, method: &str, _params: Value) -> Result<Value, DeviceError> {
            self.invoked.lock().unwrap().push(method.to_owned());
            Ok(serde_json::json!({"accepted": true}))
        }

        fn effective_access_mode(&self) -> Option<AccessMode> {
            Some(match self.mode.load(Ordering::SeqCst) {
                0 => AccessMode::Full,
                1 => AccessMode::Modify,
                2 => AccessMode::Performance,
                _ => AccessMode::ReadOnly,
            })
        }
    }

    #[async_trait]
    impl DeviceAdapter for RecordingAdapter {
        async fn ready(&self) -> bool {
            self.device_connected.load(Ordering::Relaxed)
        }

        async fn invoke(&self, method: &str, _params: Value) -> Result<Value, DeviceError> {
            self.invoked
                .lock()
                .expect("recorded calls")
                .push(method.to_owned());
            if !self.device_connected.load(Ordering::Relaxed) {
                return Err(DeviceError::new(
                    "device_unavailable",
                    "No Quad Cortex is connected",
                    true,
                ));
            }
            Ok(serde_json::json!({
                "platform": "Rust device gateway",
                "gatewayAvailable": true,
                "usbDiagnostics": {
                    "phase": "ready",
                    "detail": "synchronized",
                    "connected": true,
                    "synchronized": true,
                    "activePresetName": "Rehearsal Clean"
                }
            }))
        }
    }

    /// The access mode never changes in these tests, and a dropped sender
    /// leaves `borrow` returning the last value, which is all the backend reads.
    fn full_access() -> watch::Receiver<AccessMode> {
        watch::channel(AccessMode::Full).1
    }

    fn running_status(instance_id: &str) -> LocalAccessStatus {
        LocalAccessStatus {
            enabled: true,
            running: true,
            port: None,
            instance_id: Some(instance_id.to_owned()),
            descriptor_paths: Vec::new(),
            last_error: None,
            served_by_another_instance: false,
        }
    }

    /// A descriptor naming a port this process is not serving, used to stand in
    /// for the copy of QC Remote a test cannot start a second time.
    fn descriptor_for_port(port: u16, instance_id: &str) -> LocalDescriptor {
        LocalDescriptor {
            url: format!("http://127.0.0.1:{port}/mcp"),
            health_url: format!("http://127.0.0.1:{port}/health"),
            rpc_url: format!("http://127.0.0.1:{port}/rpc"),
            bearer_token: "T".repeat(43),
            instance_id: instance_id.into(),
            tool_contract_sha256: tool_contract_sha256(),
        }
    }

    fn published_instance(path: &Path) -> String {
        let document: Value =
            serde_json::from_slice(&fs::read(path).expect("descriptor bytes")).expect("descriptor");
        document["instanceId"]
            .as_str()
            .expect("instanceId")
            .to_owned()
    }

    #[tokio::test]
    async fn a_connection_that_never_finishes_its_request_head_is_closed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = Router::new().route("/health", get(|| async { "ok" }));
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let server = tokio::spawn(serve_loopback(
            listener,
            router,
            Duration::from_millis(300),
            async move { wait_for_shutdown(&mut stop_rx).await },
        ));

        // A trickling client is cut off once the head deadline passes.
        let mut slow = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        slow.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n")
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let mut buffer = Vec::new();
        let _closed = tokio::time::timeout(Duration::from_secs(5), slow.read_to_end(&mut buffer))
            .await
            .expect("the server kept an unfinished request head open");
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert!(
            !String::from_utf8_lossy(&buffer).contains("200 OK"),
            "an unfinished request was answered"
        );

        // A complete request is still served.
        let mut prompt = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        prompt
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), prompt.read_to_end(&mut answer))
            .await
            .unwrap()
            .unwrap();
        let answer = String::from_utf8_lossy(&answer);
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.ends_with("ok"), "{answer}");

        stop_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("shutdown was not honoured")
            .unwrap()
            .unwrap();
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err(),
            "the listener outlived shutdown"
        );
    }

    fn test_router(ready: bool) -> Router {
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(ready),
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access: access_rx,
        });
        local_router(
            backend,
            LocalHealth {
                adapter,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        )
    }

    #[test]
    fn internal_mcp_resource_reads_are_allowed_at_every_access_tier() {
        for access in [
            AccessMode::ReadOnly,
            AccessMode::Performance,
            AccessMode::Modify,
            AccessMode::Full,
        ] {
            assert!(local_access_permits(access, "system.status"));
            assert!(local_access_permits(access, "device.snapshot"));
        }
        assert!(!local_access_permits(
            AccessMode::ReadOnly,
            "device.setTempo"
        ));
    }

    #[tokio::test]
    async fn authoritative_policy_revokes_and_relaxes_while_watch_is_stale() {
        let adapter = Arc::new(EffectivePolicyAdapter::new(AccessMode::ReadOnly));
        let adapter_trait: Arc<dyn DeviceAdapter> = adapter.clone();
        let (access_tx, access_rx) = watch::channel(AccessMode::Full);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter_trait.clone(),
            access: access_rx,
        });
        let router = local_router(
            backend,
            LocalHealth {
                adapter: adapter_trait,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        );

        let forbidden = router
            .clone()
            .oneshot(rpc_request_with_params(
                "device.setTempo",
                Some("test-token"),
                serde_json::json!({ "bpm": 120 }),
            ))
            .await
            .expect("restricted RPC response");
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        assert!(adapter.invoked.lock().unwrap().is_empty());

        // The persisted authoritative choice relaxes while the old watch still
        // says ReadOnly; the serving listener must not retain that stale denial.
        adapter.set_mode(AccessMode::Full);
        access_tx.send_replace(AccessMode::ReadOnly);
        let allowed = router
            .oneshot(rpc_request_with_params(
                "device.setTempo",
                Some("test-token"),
                serde_json::json!({ "bpm": 120 }),
            ))
            .await
            .expect("relaxed RPC response");
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(
            *adapter.invoked.lock().unwrap(),
            vec!["device.setTempo".to_owned()]
        );
    }

    fn test_descriptor(token: &str) -> LocalDescriptor {
        LocalDescriptor {
            url: "http://127.0.0.1:41000/mcp".into(),
            health_url: "http://127.0.0.1:41000/health".into(),
            rpc_url: "http://127.0.0.1:41000/rpc".into(),
            bearer_token: token.into(),
            instance_id: "instance".into(),
            tool_contract_sha256: tool_contract_sha256(),
        }
    }

    /// A directory of this test's own, so a failing run cannot take another
    /// test's descriptors with it.
    fn scratch_directory(name: &str) -> PathBuf {
        crate::tests::unique_fixture_directory(&format!("qc-remote-local-access-{name}"))
    }

    async fn error_body(response: Response) -> Value {
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.starts_with("application/json")),
            Some(true),
            "both bridges parse a failure body as JSON before anything else"
        );
        serde_json::from_slice(
            &to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("error body"),
        )
        .expect("error JSON")
    }

    fn health_request(token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .uri("/health")
            .header(HOST, "127.0.0.1:41000");
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::empty()).expect("health request")
    }

    fn rpc_request(rpc: &str, token: Option<&str>) -> Request<Body> {
        rpc_request_with_params(rpc, token, serde_json::json!({}))
    }

    fn rpc_request_with_params(rpc: &str, token: Option<&str>, params: Value) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/rpc")
            .header(HOST, "127.0.0.1:41000")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        builder
            .body(Body::from(
                serde_json::json!({ "rpc": rpc, "params": params }).to_string(),
            ))
            .expect("RPC request")
    }

    #[tokio::test]
    async fn bridge_lifecycle_is_authenticated_passive_and_uses_the_publishers_store() {
        struct NoDeviceCalls;
        #[async_trait]
        impl DeviceAdapter for NoDeviceCalls {
            async fn ready(&self) -> bool {
                panic!("lifecycle must not probe USB");
            }
            async fn invoke(&self, _: &str, _: Value) -> Result<Value, DeviceError> {
                panic!("lifecycle must not invoke a device");
            }
        }
        let root = scratch_directory("bridge-lifecycle-views");
        let _root = crate::tests::RemoveDirOnDrop::new(root.clone());
        let installed = root.join("installed-view");
        let packaged = root.join("packaged-view");
        std::fs::create_dir(&installed).unwrap();
        std::fs::create_dir(&packaged).unwrap();
        let descriptor = installed.join("device-broker.json");
        let created =
            qc_native_platform::process::os_process_created_at(std::process::id()).unwrap();
        std::fs::write(&descriptor, serde_json::json!({
            "port":41001, "bearerToken":"DO-NOT-RETURN-THIS-SECRET", "instanceId":"publisher-broker",
            "processId":std::process::id(), "processCreatedAt":created, "publishedAtUnixMs":unix_time_ms(),
        }).to_string()).unwrap();
        // A stale Quit in the launcher's other view must not override the
        // publisher that this authenticated connection actually reaches.
        std::fs::write(
            crate::device_broker_descriptor::user_quit_path(&packaged.join("device-broker.json")),
            r#"{"instanceId":"unrelated","source":"tray"}"#,
        )
        .unwrap();
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(NoDeviceCalls);
        let (_access, access) = watch::channel(AccessMode::ReadOnly);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access,
        });
        let router = local_router_with_lifecycle(
            backend,
            LocalHealth {
                adapter,
                instance_id: Arc::from("publisher"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
            Duration::from_secs(15),
            Some(descriptor.clone()),
        );
        let denied = router
            .clone()
            .oneshot(rpc_request("local.bridgeLifecycle", None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        for method in ["local.bridgeLifecycle", "local.bridgeHeartbeat"] {
            let response = router
                .clone()
                .oneshot(rpc_request(method, Some("test-token")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("DO-NOT-RETURN-THIS-SECRET"));
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["result"]["brokerLifecycle"]["userQuit"], false);
            let lifecycle = body["result"]["brokerLifecycle"].as_object().unwrap();
            assert_eq!(
                lifecycle.len(),
                4,
                "only owner and explicit Quit evidence may leave the publisher"
            );
            let owner = &body["result"]["brokerLifecycle"]["owner"];
            assert_eq!(
                owner.as_object().unwrap().len(),
                3,
                "only processId, processCreatedAt and instanceId may leave the publisher"
            );
            assert_eq!(owner["processId"], std::process::id());
            assert_eq!(owner["processCreatedAt"], created);
            assert_eq!(owner["instanceId"], "publisher-broker");
            assert!(owner.get("bearerToken").is_none() && owner.get("port").is_none());
        }
        std::fs::write(
            crate::device_broker_descriptor::user_quit_path(&descriptor),
            r#"{"instanceId":"publisher-broker","source":"tray"}"#,
        )
        .unwrap();
        let post_quit_client = format!("post-quit-client-{:032x}", rand::random::<u128>());
        for method in ["local.bridgeLifecycle", "local.bridgeHeartbeat"] {
            let response = router
                .clone()
                .oneshot(rpc_request_with_params(
                    method,
                    Some("test-token"),
                    serde_json::json!({"client":post_quit_client,"sessionId":post_quit_client,"clientName":post_quit_client}),
                ))
                .await
                .unwrap();
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["result"]["brokerLifecycle"]["userQuit"], true);
            if method == "local.bridgeHeartbeat" {
                assert_eq!(body["result"]["connected"], false);
                assert!(local_client_evidence(&post_quit_client).connected());
                assert!(local_client_evidence(&post_quit_client).attached());
                let response = router
                    .clone()
                    .oneshot(rpc_request("host.status", Some("test-token")))
                    .await
                    .unwrap();
                let body = error_body(response).await;
                assert!(body["result"]["clients"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["name"] == post_quit_client));
                crate::host_presence::bridge_closed(&post_quit_client);
            }
        }
    }

    #[tokio::test]
    async fn host_status_requires_auth_and_counts_distinct_live_mcp_sessions() {
        let router = test_router(false);
        let denied = router
            .clone()
            .oneshot(rpc_request("host.status", None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let name = format!("presence-test-{:032x}", rand::random::<u128>());
        let ids = [format!("{name}-1"), format!("{name}-2")];
        assert!(!ids.is_empty(), "the presence exercise needs live sessions");
        // Two independent sessions share one client name.
        assert_eq!(ids.len(), 2);
        for id in &ids {
            let response = router
                .clone()
                .oneshot(rpc_request_with_params(
                    "local.bridgeHeartbeat",
                    Some("test-token"),
                    serde_json::json!({"sessionId":id,"clientName":name}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = router
            .clone()
            .oneshot(rpc_request("host.status", Some("test-token")))
            .await
            .unwrap();
        let document = error_body(response).await;
        assert_eq!(document["ok"], true);
        assert_eq!(document["result"]["endpoint"], "http://127.0.0.1:41000/mcp");
        assert_eq!(
            document["result"]["clients"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["name"] == name)
                .count(),
            2
        );
        assert!(!document.to_string().contains("test-token"));
        for id in &ids {
            let response = router
                .clone()
                .oneshot(rpc_request_with_params(
                    "local.bridgeClosed",
                    Some("test-token"),
                    serde_json::json!({"sessionId":id}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = router
            .oneshot(rpc_request("host.status", Some("test-token")))
            .await
            .unwrap();
        assert!(error_body(response).await["result"]["clients"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["name"] != name));
    }

    #[tokio::test]
    async fn direct_http_initialize_reports_real_client_and_delete_removes_it() {
        let router = test_router(false);
        let name = format!("http-init-{:032x}", rand::random::<u128>());
        let request = |body: Value, session: Option<&str>| {
            let mut builder = Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(HOST, "127.0.0.1:41000")
                .header(AUTHORIZATION, "Bearer test-token")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream");
            if let Some(session) = session {
                builder = builder.header("mcp-session-id", session);
            }
            builder.body(Body::from(body.to_string())).unwrap()
        };
        let response=router.clone().oneshot(request(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":name,"version":"1"}}}),None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let session = response
            .headers()
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        drop(response);
        let response = router
            .clone()
            .oneshot(request(
                serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&session),
            ))
            .await
            .unwrap();
        assert!(response.status().is_success());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !crate::host_presence::snapshot(None)
                .clients
                .iter()
                .any(|row| row.name == name)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("initialize is observed");
        let delete = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(HOST, "127.0.0.1:41000")
            .header(AUTHORIZATION, "Bearer test-token")
            .header("mcp-session-id", session)
            .body(Body::empty())
            .unwrap();
        assert!(router.oneshot(delete).await.unwrap().status().is_success());
        tokio::time::timeout(Duration::from_secs(1), async {
            while crate::host_presence::snapshot(None)
                .clients
                .iter()
                .any(|row| row.name == name)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("session deletion removes presence");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_published_listener_proves_it_holds_the_token_without_receiving_it() {
        let directory = scratch_directory("identity-proof");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![directory.join(STATE_FILE_NAME)];
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let listener = start_listener(paths.clone(), adapter, full_access()).expect("listener");
        let endpoint = crate::local_stdio::descriptor_from_paths(&paths).expect("descriptor");
        let identity_url = endpoint
            .identity_url
            .clone()
            .expect("a current descriptor names its identity check");
        assert!(crate::local_identity::identity_url_matches(
            &endpoint.rpc_url,
            &identity_url
        ));
        let client = crate::loopback_http::loopback_client_builder()
            .build()
            .unwrap();
        endpoint
            .prove_identity(&client)
            .await
            .expect("the publishing listener proves itself");
        // A client holding another token, or expecting another instance,
        // is told this is not the listener it is looking for - and which of
        // the two it was, because the LOCAL panel words them differently.
        let wrong_token = crate::local_identity::verify(
            &client,
            &identity_url,
            &endpoint.instance_id,
            &"x".repeat(43),
        )
        .await
        .expect_err("another token cannot verify");
        assert!(wrong_token.credential_mismatch);
        let wrong_instance = crate::local_identity::verify(
            &client,
            &identity_url,
            "another-instance",
            &endpoint.bearer_token,
        )
        .await
        .expect_err("another instance cannot verify");
        assert!(!wrong_instance.credential_mismatch);
        // The check itself is unauthenticated, but a browser cannot use it.
        let browser = client
            .post(&identity_url)
            .header("origin", "https://example.test")
            .json(&serde_json::json!({"nonce": "n"}))
            .send()
            .await
            .unwrap();
        assert_eq!(browser.status(), StatusCode::FORBIDDEN);
        listener.stop();
        drop(_directory);
    }

    #[test]
    fn publisher_handoff_survives_delayed_cleanup_from_the_previous_owner() {
        let directory = scratch_directory("publisher-handoff");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![directory.join(STATE_FILE_NAME)];
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(false),
        });
        let first = start_listener(paths.clone(), adapter.clone(), full_access()).unwrap();
        assert!(
            start_listener(paths.clone(), adapter.clone(), full_access()).is_err(),
            "live GUI publisher is not displaced"
        );
        first.stop();
        let second = start_listener(paths.clone(), adapter, full_access()).unwrap();
        first.stop();
        let descriptor = crate::local_stdio::descriptor_from_paths(&paths).unwrap();
        assert_eq!(descriptor.instance_id, second.instance_id());
        second.stop();
        assert!(!paths[0].exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn simultaneous_listener_start_has_one_publisher_and_releases_lease_on_stop_or_error() {
        let directory = scratch_directory("simultaneous-listeners");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![directory.join(STATE_FILE_NAME)];
        let (first, second) = std::thread::scope(|scope| {
            let gate = Arc::new(AtomicBool::new(false));
            let arrived = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let _release_on_unwind = FixtureStartRelease(gate.clone());
            let spawn = || {
                let paths = paths.clone();
                let gate = gate.clone();
                let arrived = arrived.clone();
                scope.spawn(move || {
                    arrived.fetch_add(1, Ordering::AcqRel);
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    while !gate.load(Ordering::Acquire) {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "listener fixture gate not released"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    start_listener(
                        paths,
                        Arc::new(FakeAdapter {
                            ready: AtomicBool::new(false),
                        }),
                        full_access(),
                    )
                })
            };
            let first = spawn();
            let second = spawn();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while arrived.load(Ordering::Acquire) != 2 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "listener fixture workers did not arrive"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            gate.store(true, Ordering::Release);
            (
                first.join().unwrap().map(OwnedTestListener),
                second.join().unwrap().map(OwnedTestListener),
            )
        });
        assert_ne!(
            first.is_ok(),
            second.is_ok(),
            "exactly one process/thread may publish"
        );
        let winner = first.ok().or_else(|| second.ok()).unwrap();
        assert_eq!(published_instance(&paths[0]), winner.0.instance_id());
        assert!(winner.0.stop());
        let successor = OwnedTestListener(
            start_listener(
                paths.clone(),
                Arc::new(FakeAdapter {
                    ready: AtomicBool::new(false),
                }),
                full_access(),
            )
            .unwrap(),
        );
        assert!(successor.0.stop());
        let blocked_path = directory.join("blocked");
        fs::create_dir(&blocked_path).unwrap();
        assert!(start_listener(
            vec![blocked_path.clone()],
            Arc::new(FakeAdapter {
                ready: AtomicBool::new(false)
            }),
            full_access()
        )
        .is_err());
        let released = acquire_document_lease(&blocked_path, "publisher", Duration::ZERO).unwrap();
        drop(released);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn simultaneous_publishers_with_overlapping_descriptor_sets_elect_one_owner() {
        let directory = scratch_directory("overlapping-publisher-sets");
        let _directory = OwnedTestDirectory(directory.clone());
        let first_path = directory.join("a-host.json");
        let second_path = directory.join("b-host.json");
        let shared_path = directory.join("shared-mirror.json");
        let (first, second) = std::thread::scope(|scope| {
            let gate = Arc::new(AtomicBool::new(false));
            let arrived = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let _release_on_unwind = FixtureStartRelease(Arc::clone(&gate));
            let spawn = |paths: Vec<PathBuf>| {
                let gate = Arc::clone(&gate);
                let arrived = Arc::clone(&arrived);
                scope.spawn(move || {
                    arrived.fetch_add(1, Ordering::AcqRel);
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    while !gate.load(Ordering::Acquire) {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "overlapping publisher fixture gate not released"
                        );
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    start_listener(
                        paths,
                        Arc::new(FakeAdapter {
                            ready: AtomicBool::new(false),
                        }),
                        full_access(),
                    )
                })
            };
            let first = spawn(vec![first_path.clone(), shared_path.clone()]);
            let second = spawn(vec![second_path.clone(), shared_path.clone()]);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while arrived.load(Ordering::Acquire) != 2 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "overlapping publisher workers did not arrive"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            gate.store(true, Ordering::Release);
            (
                first.join().unwrap().map(OwnedTestListener),
                second.join().unwrap().map(OwnedTestListener),
            )
        });

        assert_ne!(
            first.is_ok(),
            second.is_ok(),
            "the shared mirror lease must elect exactly one publisher"
        );
        let first_won = first.is_ok();
        let winner = first.ok().or_else(|| second.ok()).unwrap();
        let descriptors = [&first_path, &second_path, &shared_path]
            .into_iter()
            .filter(|path| path.exists())
            .map(|path| {
                crate::local_stdio::descriptor_from_paths(std::slice::from_ref(path)).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            descriptors.len(),
            2,
            "only the winner's private path and the shared mirror are published"
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| descriptor.instance_id == winner.0.instance_id()),
            "all published views must name the elected owner"
        );

        assert!(winner.0.stop());
        let successor_paths = if first_won {
            vec![second_path, shared_path]
        } else {
            vec![first_path, shared_path]
        };
        let successor = OwnedTestListener(
            start_listener(
                successor_paths,
                Arc::new(FakeAdapter {
                    ready: AtomicBool::new(false),
                }),
                full_access(),
            )
            .unwrap(),
        );
        assert!(successor.0.stop());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_partial_publisher_lease_acquisition_releases_earlier_paths() {
        let directory = scratch_directory("partial-publisher-lease");
        let _directory = OwnedTestDirectory(directory.clone());
        let first_path = directory.join("a-host.json");
        let blocked_path = directory.join("z-shared-mirror.json");
        // start_listener sorts paths, so this held lease is acquired second;
        // failure there must drop the lease already acquired for first_path.
        let blocker = acquire_document_lease(&blocked_path, "publisher", Duration::ZERO).unwrap();

        let error = match start_listener(
            vec![blocked_path.clone(), first_path.clone()],
            Arc::new(FakeAdapter {
                ready: AtomicBool::new(false),
            }),
            full_access(),
        ) {
            Ok(listener) => {
                listener.stop();
                panic!("a publisher must not pass the held shared-mirror lease");
            }
            Err(error) => error,
        };
        assert!(error.served_by_another_instance);
        assert!(error.message.contains("Another process owns"));

        let first_lease = acquire_document_lease(&first_path, "publisher", Duration::ZERO)
            .expect("the earlier lease is released when a later acquisition fails");
        drop(first_lease);
        drop(blocker);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn actual_listener_fixture_unwind_retires_publisher_before_its_owned_root() {
        let directory = scratch_directory("listener-unwind");
        let port = AtomicU64::new(0);
        let failed = std::panic::catch_unwind(|| {
            let _directory = OwnedTestDirectory(directory.clone());
            let listener = start_listener(
                vec![directory.join(STATE_FILE_NAME)],
                Arc::new(FakeAdapter {
                    ready: AtomicBool::new(false),
                }),
                full_access(),
            )
            .unwrap();
            port.store(listener.port.into(), Ordering::Release);
            panic!("injected failure after production listener acquisition");
        });
        assert!(failed.is_err());
        assert!(
            !directory.exists(),
            "root removed only after listener retirement"
        );
        let port = u16::try_from(port.load(Ordering::Acquire)).unwrap();
        assert!(port != 0);
        assert!(
            std::net::TcpStream::connect_timeout(
                &([127, 0, 0, 1], port).into(),
                Duration::from_millis(100),
            )
            .is_err(),
            "the actual listener still serves after fixture unwind"
        );
        reap_fixture_listeners();
    }

    struct PendingReceiptBody {
        polled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }
    impl hyper::body::Body for PendingReceiptBody {
        type Data = axum::body::Bytes;
        type Error = std::convert::Infallible;
        fn poll_frame(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
            self.polled.store(true, Ordering::Release);
            std::task::Poll::Pending
        }
    }
    impl Drop for PendingReceiptBody {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    fn bounded_body_router(adapter: Arc<RecordingAdapter>) -> Router {
        let adapter_trait: Arc<dyn DeviceAdapter> = adapter.clone();
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter_trait.clone(),
            access: full_access(),
        });
        local_router_with_body_timeout(
            backend,
            LocalHealth {
                adapter: adapter_trait,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
            Duration::from_millis(40),
        )
    }
    fn body_request(path: &str, token: Option<&str>, body: Body) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header(HOST, "127.0.0.1:41000")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        request.body(body).unwrap()
    }
    #[tokio::test]
    async fn identity_rpc_and_mcp_receipt_deadlines_drop_pending_bodies_before_device_dispatch() {
        for path in ["/identity", "/rpc", "/mcp"] {
            let adapter = Arc::new(RecordingAdapter::new(true));
            let polled = Arc::new(AtomicBool::new(false));
            let dropped = Arc::new(AtomicBool::new(false));
            let body = Body::new(PendingReceiptBody {
                polled: polled.clone(),
                dropped: dropped.clone(),
            });
            // No spawned task: timeout/unwind drops the exact owned service
            // future and its body inline, with observable destructor receipt.
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                bounded_body_router(adapter.clone()).oneshot(body_request(
                    path,
                    Some("test-token"),
                    body,
                )),
            )
            .await
            .expect("body receipt exceeded its own deadline")
            .unwrap();
            assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT, "{path}");
            let error = error_body(response).await;
            assert_eq!(error["code"], "LOCAL_REQUEST_TIMEOUT");
            assert!(polled.load(Ordering::Acquire));
            assert!(
                dropped.load(Ordering::Acquire),
                "pending body remains owned after refusal"
            );
            assert!(
                adapter.calls().is_empty(),
                "incomplete receipt dispatched device IO"
            );
        }
    }
    #[tokio::test]
    async fn receipt_budget_preserves_auth_precedence_byte_limits_and_healthy_dispatch() {
        let adapter = Arc::new(RecordingAdapter::new(true));
        let polled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let denied = bounded_body_router(adapter.clone())
            .oneshot(body_request(
                "/rpc",
                None,
                Body::new(PendingReceiptBody {
                    polled: polled.clone(),
                    dropped: dropped.clone(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert!(
            !polled.load(Ordering::Acquire),
            "unauthorized body must not be consumed"
        );
        assert!(dropped.load(Ordering::Acquire));
        for (path, limit) in [
            ("/identity", 1024),
            ("/rpc", 2 * 1024 * 1024),
            ("/mcp", 4 * 1024 * 1024),
        ] {
            let refused = bounded_body_router(adapter.clone())
                .oneshot(body_request(
                    path,
                    Some("test-token"),
                    Body::from(vec![b'x'; limit + 1]),
                ))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE, "{path}");
            assert_eq!(error_body(refused).await["code"], "LOCAL_REQUEST_TOO_LARGE");
        }
        assert!(adapter.calls().is_empty());
        let identity = bounded_body_router(adapter.clone())
            .oneshot(body_request(
                "/identity",
                None,
                Body::from(r#"{"nonce":"complete-fixture"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(identity.status(), StatusCode::OK);
        let healthy = bounded_body_router(adapter.clone())
            .oneshot(rpc_request("system.status", Some("test-token")))
            .await
            .unwrap();
        assert_eq!(healthy.status(), StatusCode::OK);
        assert_eq!(adapter.calls(), vec!["system.status"]);
    }

    #[tokio::test]
    async fn authenticated_health_reports_ready_identity() {
        let response = test_router(true)
            .oneshot(health_request(Some("test-token")))
            .await
            .expect("health response");
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("health body"),
        )
        .expect("health JSON");
        assert_eq!(body["schemaVersion"], 1);
        assert_eq!(body["instanceId"], "test-instance");
        assert_eq!(body["appReady"], true);
        assert_eq!(body["deviceReady"], true);
        assert_eq!(body["processId"], 42);
        assert_eq!(body["toolContractSha256"], tool_contract_sha256());
    }

    #[tokio::test]
    async fn unavailable_health_is_typed_and_authentication_fails_closed() {
        let response = test_router(false)
            .oneshot(health_request(Some("test-token")))
            .await
            .expect("health response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("health body"),
        )
        .expect("health JSON");
        assert_eq!(body["code"], "LOCAL_UNAVAILABLE");
        assert_eq!(body["reason"], "device_not_ready");

        let unauthorized = test_router(true)
            .oneshot(health_request(None))
            .await
            .expect("unauthorized response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let origin = test_router(true)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(HOST, "127.0.0.1:41000")
                    .header(AUTHORIZATION, "Bearer test-token")
                    .header("origin", "https://example.com")
                    .body(Body::empty())
                    .expect("origin request"),
            )
            .await
            .expect("origin response");
        assert_eq!(origin.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn every_authentication_failure_is_typed_json_rather_than_a_bare_string() {
        // Regression: these three were returned as `text/plain`. Both clients
        // parse a failure body as JSON, fail to find a typed code, and fall
        // back to the health endpoint for a reason - so a wrong bearer token, a
        // rebinding attempt and a browser-issued request were each reported to
        // the user as "the Quad Cortex is not connected".
        let unauthorized = test_router(true)
            .oneshot(health_request(None))
            .await
            .expect("unauthorized response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let body = error_body(unauthorized).await;
        assert_eq!(body["code"], "LOCAL_UNAUTHORIZED");
        assert_eq!(body["reason"], "unauthorized");

        let invalid_host = test_router(true)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(HOST, "localhost:41000")
                    .header(AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .expect("invalid host request"),
            )
            .await
            .expect("invalid host response");
        assert_eq!(invalid_host.status(), StatusCode::BAD_REQUEST);
        let body = error_body(invalid_host).await;
        assert_eq!(body["code"], "LOCAL_INVALID_HOST");
        assert_eq!(body["reason"], "invalid_host");

        let origin = test_router(true)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(HOST, "127.0.0.1:41000")
                    .header(AUTHORIZATION, "Bearer test-token")
                    .header("origin", "https://example.com")
                    .body(Body::empty())
                    .expect("origin request"),
            )
            .await
            .expect("origin response");
        assert_eq!(origin.status(), StatusCode::FORBIDDEN);
        let body = error_body(origin).await;
        assert_eq!(body["code"], "LOCAL_FORBIDDEN_ORIGIN");
        assert_eq!(body["reason"], "browser_origin_forbidden");
    }

    #[tokio::test]
    async fn an_rpc_failure_carries_the_typed_code_without_breaking_the_native_envelope() {
        let unknown = test_router(true)
            .oneshot(rpc_request("not.a.real.action", Some("test-token")))
            .await
            .expect("unknown RPC response");
        assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
        let body = error_body(unknown).await;
        assert_eq!(body["code"], "LOCAL_UNKNOWN_ACTION");
        assert_eq!(body["reason"], "unknown_action");
        // `local_stdio.rs` deserializes this envelope; the typed fields are
        // additive and must not disturb it.
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "unknown_action");
        assert_eq!(body["error"]["retryable"], false);
        assert!(body["error"]["message"].is_string());

        // The access mode is the other rejection a client sees routinely.
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::ReadOnly);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access: access_rx,
        });
        let router = local_router(
            backend,
            LocalHealth {
                adapter,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        );
        let forbidden = router
            .oneshot(rpc_request_with_params(
                "device.setTempo",
                Some("test-token"),
                serde_json::json!({ "bpm": 120 }),
            ))
            .await
            .expect("forbidden RPC response");
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let body = error_body(forbidden).await;
        assert_eq!(body["code"], "LOCAL_FORBIDDEN");
        assert_eq!(body["reason"], "forbidden");
        assert_eq!(body["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn an_rpc_device_failure_preserves_domain_code_separately_from_retry_class() {
        let mut device_error =
            DeviceError::new("-32013", "DEVICE_SUSPENDED: A backup is in progress", false);
        device_error.details = Some(Box::new(serde_json::json!({
            "code": "DEVICE_SUSPENDED",
            "retryable": false,
            "requestSent": false,
            "privatePayload": "must not be forwarded"
        })));
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(ErrorAdapter {
            error: device_error,
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access: access_rx,
        });
        let response = local_router(
            backend,
            LocalHealth {
                adapter,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        )
        .oneshot(rpc_request("system.status", Some("test-token")))
        .await
        .expect("typed device error response");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = error_body(response).await;
        assert_eq!(body["code"], "LOCAL_DEVICE_ERROR");
        assert_eq!(body["reason"], "device_error");
        assert_eq!(body["error"]["code"], "device_error");
        assert_eq!(body["error"]["deviceCode"], "DEVICE_SUSPENDED");
        assert_eq!(body["error"]["retryable"], false);
        assert_eq!(
            body["error"]["message"],
            "DEVICE_SUSPENDED: A backup is in progress"
        );
        assert_eq!(body["error"]["details"]["requestSent"], false);
        assert!(body["error"]["details"]["privatePayload"].is_null());
    }

    #[tokio::test]
    async fn actual_adapter_to_mcp_preserves_only_bounded_quarantine_facts() {
        for code in ["WRITE_QUARANTINED", "WRITE_OUTCOME_AMBIGUOUS"] {
            for nested_code in [None, Some(code)] {
                let mut error = DeviceError::new(code, "fixture", false);
                let mut details = serde_json::json!({
                    "retryable":false,"writesQuarantined":true,
                    "quarantine":{"scope":"device","sinceUnixMs":5,"sessionEpoch":2,
                        "reason":"SECRET_FIXTURE","clearsBy":"SECRET_FIXTURE","method":"SECRET_FIXTURE"},
                    "credential":"SECRET_FIXTURE","userActionRequired":true
                });
                if let Some(code) = nested_code {
                    details["code"] = serde_json::json!(code);
                }
                if code == "WRITE_QUARANTINED" {
                    details["requestSent"] = serde_json::json!(false);
                }
                error.details = Some(Box::new(details));
                let adapter: Arc<dyn DeviceAdapter> = Arc::new(ErrorAdapter { error });
                let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
                let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
                    adapter,
                    access: access_rx,
                });
                let server = QcMcp::new(backend);
                let text = server
                    .execute(
                        &PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE),
                        "list_models",
                        Some(Map::from_iter([("query".into(), Value::Null)])),
                    )
                    .await
                    .unwrap_err();
                let wire: Value = serde_json::from_str(&text).expect("actual MCP text error JSON");
                assert_eq!(wire["code"], "device_error");
                assert_eq!(wire["deviceCode"], code);
                assert_eq!(wire["retryable"], false);
                assert!(wire.get("userActionRequired").is_none());
                assert_eq!(wire["deviceDetails"]["writesQuarantined"], true);
                assert_eq!(
                    wire["deviceDetails"]["quarantine"],
                    serde_json::json!({"scope":"device","sinceUnixMs":5,"sessionEpoch":2})
                );
                if code == "WRITE_QUARANTINED" {
                    assert_eq!(wire["deviceDetails"]["requestSent"], false);
                } else {
                    assert!(wire["deviceDetails"].get("requestSent").is_none());
                }
                assert!(!text.contains("SECRET_FIXTURE"));
            }
        }
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(ErrorAdapter {
            error: DeviceError::new("DEVICE_TRANSPORT", "legacy fixture", true),
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
        let server = QcMcp::new(Arc::new(LocalBackend {
            adapter,
            access: access_rx,
        }));
        let text = server
            .execute(
                &PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE),
                "list_models",
                Some(Map::from_iter([("query".into(), Value::Null)])),
            )
            .await
            .unwrap_err();
        let wire: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(wire["code"], "device_unavailable");
        assert_eq!(wire["retryable"], true);
        assert!(wire.get("deviceDetails").is_none());
        assert!(wire.get("userActionRequired").is_none());
    }

    #[tokio::test]
    async fn actual_adapter_mcp_drops_only_quarantine_contradictions() {
        for code in [
            "WRITE_QUARANTINED",
            "WRITE_OUTCOME_AMBIGUOUS",
            "DEVICE_OUTCOME_UNKNOWN",
            "OTHER",
        ] {
            for nested_code in [None, Some(code)] {
                let mut error = DeviceError::new(code, "fixture", false);
                let mut details = serde_json::json!({"requestSent":true,"writesQuarantined":false});
                if let Some(code) = nested_code {
                    details["code"] = serde_json::json!(code);
                }
                error.details = Some(Box::new(details));
                let adapter: Arc<dyn DeviceAdapter> = Arc::new(ErrorAdapter { error });
                let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
                let server = QcMcp::new(Arc::new(LocalBackend {
                    adapter,
                    access: access_rx,
                }));
                let text = server
                    .execute(
                        &PrincipalRoute::new(LOCAL_PRINCIPAL, LOCAL_DEVICE),
                        "list_models",
                        Some(Map::from_iter([("query".into(), Value::Null)])),
                    )
                    .await
                    .unwrap_err();
                let wire: Value = serde_json::from_str(&text).unwrap();
                if code == "WRITE_QUARANTINED" {
                    assert!(wire["deviceDetails"].get("requestSent").is_none());
                    assert!(wire["deviceDetails"].get("writesQuarantined").is_none());
                } else {
                    assert_eq!(wire["deviceDetails"]["requestSent"], true);
                    assert_eq!(wire["deviceDetails"]["writesQuarantined"], false);
                }
                assert!(wire.get("userActionRequired").is_none());
            }
        }
    }

    #[test]
    fn malformed_device_error_codes_are_not_exposed_as_domain_codes() {
        assert_eq!(
            safe_device_error_code("DEVICE_SUSPENDED"),
            Some("DEVICE_SUSPENDED")
        );
        assert_eq!(safe_device_error_code("DEVICE/SECRET"), None);
        assert_eq!(safe_device_error_code(&"A".repeat(65)), None);
    }

    #[tokio::test]
    async fn native_stdio_hop_is_authenticated_and_action_allowlisted() {
        let response = test_router(true)
            .oneshot(rpc_request("system.status", Some("test-token")))
            .await
            .expect("RPC response");
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("RPC body"),
        )
        .expect("RPC JSON");
        assert_eq!(
            body,
            serde_json::json!({ "ok": true, "result": { "ok": true } })
        );

        let unknown = test_router(true)
            .oneshot(rpc_request("not.a.real.action", Some("test-token")))
            .await
            .expect("unknown RPC response");
        assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

        let unauthorized = test_router(true)
            .oneshot(rpc_request("system.status", None))
            .await
            .expect("unauthorized RPC response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_native_heartbeat_records_live_bridge_activity_without_device_io() {
        let router = test_router(false);
        let response = router
            .oneshot(rpc_request_with_params(
                "local.bridgeHeartbeat",
                Some("test-token"),
                serde_json::json!({ "client": ANTIGRAVITY_BRIDGE_CLIENT }),
            ))
            .await
            .expect("heartbeat response");
        assert_eq!(response.status(), StatusCode::OK);
        // Antigravity is read out of the one shared record every other host is
        // read from, so a correction to the rule cannot reach five providers
        // and miss the sixth.
        let evidence = antigravity_bridge_evidence();
        assert!(evidence.connected());
        assert!(evidence.attached());
        assert!(antigravity_bridge_connected());
        assert_eq!(evidence, local_client_evidence(ANTIGRAVITY_BRIDGE_CLIENT));
        // The same announcement also reaches the record that outlives this
        // process, so the next launch can tell this host apart from one that
        // has never run here.
        assert!(local_client_attached_before(ANTIGRAVITY_BRIDGE_CLIENT));
    }

    #[tokio::test]
    async fn an_unlinked_native_bridge_stays_off_the_list_until_its_next_tool_call() {
        let router = test_router(true);
        let name = format!("unlink-test-{:032x}", rand::random::<u128>());
        let session = format!("stdio-{:032x}", rand::random::<u128>());
        let heartbeat = |router: Router| {
            let request = rpc_request_with_params(
                "local.bridgeHeartbeat",
                Some("test-token"),
                serde_json::json!({"sessionId":session,"clientName":name}),
            );
            async move { router.oneshot(request).await.unwrap() }
        };
        let listed = |router: Router| {
            let name = name.clone();
            async move {
                let response = router
                    .oneshot(rpc_request("host.status", Some("test-token")))
                    .await
                    .unwrap();
                error_body(response).await["result"]["clients"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|row| row["name"] == name.as_str())
                    .count()
            }
        };
        let tool_call = |session: Option<&str>| {
            let mut request = rpc_request_with_params(
                "device.setTempo",
                Some("test-token"),
                serde_json::json!({ "bpm": 120 }),
            );
            if let Some(session) = session {
                request.headers_mut().insert(
                    crate::host_presence::BRIDGE_SESSION_HEADER,
                    session.parse().unwrap(),
                );
            }
            request
        };
        assert_eq!(heartbeat(router.clone()).await.status(), StatusCode::OK);
        assert_eq!(listed(router.clone()).await, 1);
        assert!(crate::host_presence::end_local_session(&session));
        // Acknowledged exactly as before, and not recorded.
        let answered = heartbeat(router.clone()).await;
        assert_eq!(answered.status(), StatusCode::OK);
        assert_eq!(
            error_body(answered).await,
            serde_json::json!({"ok":true,"result":{"connected":true}})
        );
        assert_eq!(listed(router.clone()).await, 0);
        // A tool call without the header (an older bridge), or naming another
        // session, works and relinks nothing.
        for other in [None, Some("stdio-someone-else")] {
            let response = router.clone().oneshot(tool_call(other)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(listed(router.clone()).await, 0);
        }
        // A resource read is not a tool call.
        let mut read = rpc_request("system.status", Some("test-token"));
        read.headers_mut().insert(
            crate::host_presence::BRIDGE_SESSION_HEADER,
            session.parse().unwrap(),
        );
        assert_eq!(
            router.clone().oneshot(read).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(listed(router.clone()).await, 0);
        // The chat's next tool call is answered normally and relinks it.
        let response = router
            .clone()
            .oneshot(tool_call(Some(&session)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(listed(router.clone()).await, 1);
        assert_eq!(heartbeat(router.clone()).await.status(), StatusCode::OK);
        assert_eq!(listed(router.clone()).await, 1);
        crate::host_presence::bridge_closed(&session);
        assert_eq!(listed(router).await, 0);
    }

    #[test]
    fn resource_only_methods_never_relink_and_every_tool_method_may() {
        let resources = qc_remote_mcp::resource_rpcs();
        assert_eq!(resources.len(), 3);
        let actions = qc_remote_mcp::ACTIONS;
        assert!(!actions.is_empty());
        let mut resource_only = 0;
        for (uri, rpc) in resources {
            let tool = qc_remote_mcp::ACTIONS
                .iter()
                .any(|action| action.rpc == rpc);
            // A shared method is a tool call unless the bridge marked it as a
            // resource read by omitting its session (see local_stdio).
            assert_eq!(relinks_session(rpc), tool, "{uri} -> {rpc}");
            resource_only += usize::from(!tool);
        }
        assert_eq!(resource_only, 1, "system.status");
        assert!(!relinks_session("device.snapshot"), "internal projection");
        assert!(relinks_session("device.listModels"), "the list_models tool");
        for action in actions {
            assert!(relinks_session(action.rpc), "{}", action.rpc);
        }
    }

    /// The relink happens before the backend is asked, so a call the access
    /// level refuses (or that fails) still lists the chat again: the chat
    /// made a new request, whatever its answer.
    #[tokio::test]
    async fn a_refused_tool_call_still_relinks_its_session() {
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::ReadOnly);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access: access_rx,
        });
        let router = local_router(
            backend,
            LocalHealth {
                adapter,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        );
        let name = format!("refused-relink-{:032x}", rand::random::<u128>());
        let session = format!("stdio-{:032x}", rand::random::<u128>());
        let response = router
            .clone()
            .oneshot(rpc_request_with_params(
                "local.bridgeHeartbeat",
                Some("test-token"),
                serde_json::json!({"sessionId":session,"clientName":name}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(crate::host_presence::end_local_session(&session));
        let listed = || async {
            let response = router
                .clone()
                .oneshot(rpc_request("host.status", Some("test-token")))
                .await
                .unwrap();
            error_body(response).await["result"]["clients"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["name"] == name.as_str())
                .count()
        };
        assert_eq!(listed().await, 0);
        let mut write = rpc_request_with_params(
            "device.setTempo",
            Some("test-token"),
            serde_json::json!({ "bpm": 120 }),
        );
        write.headers_mut().insert(
            crate::host_presence::BRIDGE_SESSION_HEADER,
            session.parse().unwrap(),
        );
        let refused = router.clone().oneshot(write).await.unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(listed().await, 1);
        crate::host_presence::bridge_closed(&session);
        assert_eq!(listed().await, 0);
    }

    #[test]
    fn a_bridge_that_has_not_ticked_yet_is_not_reported_as_absent() {
        let window = NATIVE_BRIDGE_ACTIVITY_WINDOW.as_millis() as u64;
        // The app restarted at 1_000_000 and nothing has announced itself.
        let settling = local_client_evidence_at(0, false, 1_000_000, 1_000_000 + window);
        assert_eq!(settling.liveness, LocalClientLiveness::Settling);
        assert!(settling.settling());
        assert!(
            !settling.connected(),
            "a row must never go green on the absence of a heartbeat"
        );
        assert!(!settling.picked_up_current_config());

        // One tick later the window has closed and silence means what it says.
        let absent = local_client_evidence_at(0, false, 1_000_000, 1_000_000 + window + 1);
        assert_eq!(absent.liveness, LocalClientLiveness::Absent);
        assert!(!absent.settling());
    }

    #[test]
    fn a_bridge_that_heartbeats_after_an_app_restart_reads_as_connected() {
        let window = NATIVE_BRIDGE_ACTIVITY_WINDOW.as_millis() as u64;
        // The listener opened at 1_000_000 with an empty record; a bridge that
        // was already running announces itself one heartbeat later.
        let observing_since = 1_000_000;
        let announced_at = observing_since + 5_000;
        let evidence = local_client_evidence_at(announced_at, false, observing_since, announced_at);
        assert!(
            evidence.connected(),
            "a live announcement is all the evidence a connected row needs"
        );
        assert!(evidence.picked_up_current_config());
        // And it stays connected for the whole activity window without the
        // user doing anything.
        assert!(local_client_evidence_at(
            announced_at,
            false,
            observing_since,
            announced_at + window
        )
        .connected());
    }

    #[test]
    fn liveness_is_unknown_until_the_listener_opens() {
        // No listener has ever started, so there is no start-up window to be
        // inside and nothing to be lenient about.
        assert_eq!(
            local_client_evidence_at(0, false, 0, 9_999_999).liveness,
            LocalClientLiveness::Absent
        );
    }

    /// A client id of the shape this build mints, for a made-up provider.
    ///
    /// The record keeps only ids carrying this build's version, so a test that
    /// wrote a literal one would start failing at the next release bump - and
    /// would be asserting about a row the app could never read anyway.
    fn current_client_id(provider: &str) -> String {
        format!("{provider}:{}", env!("CARGO_PKG_VERSION"))
    }

    #[test]
    fn a_host_that_worked_before_reads_as_idle_after_a_restart_not_as_never_connected() {
        let directory = scratch_directory("client-history");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(CLIENT_HISTORY_FILE_NAME);
        let client = &current_client_id("codex");
        let attached_at = 1_700_000_000_000_u64;

        // The run in which this host's bridge announced itself.
        let mut live = BTreeMap::new();
        let owed = note_attach(&mut live, client, attached_at, 0);
        store_attach_history(&path, &live).expect("store history");

        // The app is closed and opened again. Nothing in memory survives, so
        // `last_seen` is 0, and the listener begins accepting
        // announcements a day later.
        let remembered = load_attach_history(&path);
        let attached_before = remembered.contains_key(client);
        let observing_since = attached_at + 24 * 60 * 60 * 1_000;
        let now = observing_since + NATIVE_BRIDGE_ACTIVITY_WINDOW.as_millis() as u64 + 1;
        let evidence = local_client_evidence_at(0, attached_before, observing_since, now);
        drop(_directory);

        assert!(owed, "the first attach for a client id is written at once");
        assert!(attached_before, "the record did not survive the restart");
        assert!(
            evidence.attached(),
            "a host whose bridge has run here before must never be told it has never connected"
        );
        assert!(
            evidence.picked_up_current_config(),
            "nor asked to restart to re-read a configuration it has demonstrably read"
        );
        assert!(
            !evidence.connected(),
            "history is not a heartbeat: no row may go green on a remembered attach"
        );
        assert!(
            !evidence.settling(),
            "nor may a remembered attach hold a row in the start-up window"
        );
        assert_eq!(evidence.liveness, LocalClientLiveness::Absent);
    }

    #[test]
    fn a_remembered_attach_can_never_become_a_connection() {
        let remembered_at = 1_700_000_000_000_u64;
        // The hazard the two-argument split exists to prevent, stated as an
        // assertion: had the durable record been restored by seeding
        // `last_seen`, the recency rule would have read it as a live bridge
        // and painted a green row for a process that is not running.
        assert!(native_bridge_is_recent(
            remembered_at,
            remembered_at + 1_000
        ));

        // It travels as a fact instead, so liveness is decided entirely by
        // this process's own silence.
        let observing_since = remembered_at + 1_000;
        let window = NATIVE_BRIDGE_ACTIVITY_WINDOW.as_millis() as u64;
        let inside = local_client_evidence_at(0, true, observing_since, observing_since + window);
        let outside =
            local_client_evidence_at(0, true, observing_since, observing_since + window + 1);

        assert_eq!(inside.liveness, LocalClientLiveness::Settling);
        assert!(!inside.connected());
        assert_eq!(outside.liveness, LocalClientLiveness::Absent);
        assert!(!outside.connected());
        assert!(outside.attached());
        assert_eq!(
            outside.liveness,
            local_client_evidence_at(0, false, observing_since, observing_since + window + 1)
                .liveness,
            "the durable record must not move liveness in either direction"
        );
    }

    #[test]
    fn an_unreadable_client_history_degrades_to_no_history() {
        let directory = scratch_directory("client-history-damage");
        let _directory = OwnedTestDirectory(directory.clone());
        let absent = load_attach_history(&directory.join("never-written.json"));

        let garbage = directory.join("garbage.json");
        fs::write(&garbage, b"{ this is not json").expect("write garbage");
        let unparsable = load_attach_history(&garbage);

        let truncated = directory.join("truncated.json");
        fs::write(&truncated, br#"{"schemaVersion":1,"clients":{"codex:0.1"#)
            .expect("write truncated");
        let cut_short = load_attach_history(&truncated);

        let future = directory.join("future.json");
        fs::write(
            &future,
            br#"{"schemaVersion":99,"clients":{"codex:0.1.144":1700000000000}}"#,
        )
        .expect("write future schema");
        let unknown_schema = load_attach_history(&future);
        drop(_directory);

        assert!(absent.is_empty(), "a first launch has no history");
        assert!(unparsable.is_empty());
        assert!(
            cut_short.is_empty(),
            "a torn write must not panic or half-load"
        );
        assert!(
            unknown_schema.is_empty(),
            "a file from a build that knows more than this one is not guessed at"
        );
        // Degrading to empty is degrading to the behaviour every release
        // before this record had - amber rows - and never to a false green.
        let degraded = local_client_evidence_at(0, false, 1_000, 1_000_000);
        assert!(!degraded.attached());
        assert!(!degraded.connected());
    }

    #[test]
    fn the_client_history_evicts_the_least_recently_attached_id() {
        let mut clients = BTreeMap::new();
        let overflow = CLIENT_HISTORY_CAPACITY + 4;
        // Ids attached a second apart, oldest first.
        for index in 0..overflow {
            note_attach(
                &mut clients,
                &current_client_id(&format!("host-{index:03}")),
                1_000 + index as u64 * 1_000,
                0,
            );
        }
        let first_kept = overflow - CLIENT_HISTORY_CAPACITY;

        assert_eq!(clients.len(), CLIENT_HISTORY_CAPACITY);
        assert!(
            !clients.contains_key(&current_client_id("host-000")),
            "the least recently attached id is the one that goes"
        );
        assert!(!clients.contains_key(&current_client_id(&format!("host-{:03}", first_kept - 1))));
        assert!(
            clients.contains_key(&current_client_id(&format!("host-{first_kept:03}"))),
            "and eviction stops the moment the record is back inside its bound"
        );
        assert!(
            clients.contains_key(&current_client_id(&format!("host-{:03}", overflow - 1))),
            "the most recent id is always kept"
        );
    }

    #[test]
    fn a_record_written_by_earlier_releases_keeps_only_the_ids_this_build_reads() {
        let directory = scratch_directory("client-history-upgrade");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(CLIENT_HISTORY_FILE_NAME);
        let live = current_client_id("claude");
        // A legacy record can contain several versions of each supported host.
        let mut carried = BTreeMap::new();
        for (index, version) in ["0.1.144", "0.1.151", "0.1.155", "0.1.158"]
            .into_iter()
            .enumerate()
        {
            for provider in ["claude", "codex", "antigravity", "jan", "lm-studio"] {
                carried.insert(format!("{provider}:{version}"), 1_000 + index as u64);
            }
        }
        carried.insert(live.clone(), 9_000);
        store_attach_history(&path, &carried).expect("store history");

        let restored = load_attach_history(&path);
        drop(_directory);

        assert_eq!(
            restored.keys().collect::<Vec<_>>(),
            vec![&live],
            "an upgraded machine opens holding one row per live host, not one per release"
        );
        assert_eq!(
            restored.get(&live).copied(),
            Some(9_000),
            "and the row that survives keeps the instant it was attached at"
        );
    }

    #[test]
    fn an_announcement_from_a_bridge_this_build_did_not_route_is_not_recorded() {
        let mut clients = BTreeMap::new();
        // A host still running the bridge from before an upgrade. Its id is
        // one `local_client_evidence` is never called with, so a row for it
        // could not reach a panel row even if it were kept.
        let stale = note_attach(&mut clients, "claude:0.1.144", 10_000, 0);
        let unparsable = note_attach(&mut clients, "no-version-at-all", 10_000, 0);
        let current = note_attach(&mut clients, &current_client_id("claude"), 10_000, 0);

        assert!(!stale, "a row nothing can read must not cost a file write");
        assert!(!unparsable);
        assert!(
            current,
            "the id this build routes is still recorded at once"
        );
        assert_eq!(
            clients.keys().collect::<Vec<_>>(),
            vec![&current_client_id("claude")]
        );
    }

    #[test]
    fn only_the_first_attach_for_a_host_is_written_at_once() {
        let mut clients = BTreeMap::new();
        let client = &current_client_id("antigravity");
        let interval = CLIENT_HISTORY_WRITE_INTERVAL.as_millis() as u64;

        let first = note_attach(&mut clients, client, 10_000, 0);
        // The announcement five seconds later carries no new fact.
        let repeat = note_attach(&mut clients, client, 15_000, 10_000);
        let just_inside = note_attach(&mut clients, client, 10_000 + interval - 1, 10_000);
        let due = note_attach(&mut clients, client, 10_000 + interval, 10_000);
        // A second host is a new fact even while the first host's throttle is
        // closed, because it is what decides that host's own row.
        let other_host = note_attach(&mut clients, &current_client_id("jan"), 15_000, 10_000);

        assert!(first, "the fact that changes a row is written immediately");
        assert!(
            !repeat,
            "a five-second announcement must not cost a file write"
        );
        assert!(!just_inside);
        assert!(
            due,
            "the eviction key is refreshed once the floor has passed"
        );
        assert!(other_host);
        assert_eq!(clients.len(), 2);
        assert_eq!(clients.get(client).copied(), Some(10_000 + interval));
    }

    #[test]
    fn a_repeated_write_failure_is_reported_once_per_reason() {
        assert!(should_report_failure(None, "the disk is full"));
        assert!(
            !should_report_failure(Some("the disk is full"), "the disk is full"),
            "a failure that has not changed does not earn a second line"
        );
        assert!(
            should_report_failure(Some("the disk is full"), "access is denied"),
            "a different reason is a different diagnosis and is always reported"
        );
    }

    #[test]
    fn native_bridge_activity_expires_when_the_stdio_process_disappears() {
        assert!(!native_bridge_is_recent(0, 10_000));
        assert!(native_bridge_is_recent(10_000, 21_999));
        assert!(native_bridge_is_recent(10_000, 22_000));
        assert!(!native_bridge_is_recent(10_000, 22_001));
    }

    #[test]
    fn the_published_contract_hash_is_the_normalized_contract_on_disk() {
        let on_disk = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../contracts/public/qc-actions.v1.json"),
        )
        .expect("qc-actions.v1.json");
        let normalized = String::from_utf8_lossy(&on_disk).replace("\r\n", "\n");
        let expected: String = Sha256::digest(normalized.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(tool_contract_sha256(), expected);
    }

    #[tokio::test]
    async fn only_the_exact_loopback_host_is_accepted() {
        let response = test_router(true)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(HOST, "localhost:41000")
                    .header(AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .expect("localhost request"),
            )
            .await
            .expect("localhost response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn publishing_clears_an_orphaned_staged_descriptor() {
        let directory = scratch_directory("stale");
        let _directory = OwnedTestDirectory(directory.clone());
        let orphan = directory.join(format!(".{STATE_FILE_NAME}.999999.0.tmp"));
        fs::write(&orphan, b"{\"bearerToken\":\"live-looking\"}").expect("orphan");
        let path = directory.join(STATE_FILE_NAME);
        test_descriptor("token")
            .publish(&path)
            .expect("endpoint document");
        let remaining = orphan.exists();
        drop(_directory);
        assert!(
            !remaining,
            "an abandoned staged descriptor still holds a live-looking credential"
        );
    }

    #[test]
    fn a_published_descriptor_is_readable_by_this_user_and_nobody_else() {
        // The descriptor carries a bearer token that grants full control of the
        // attached device. Under the inherited ACL it was also readable by
        // `BUILTIN\Administrators` and `NT AUTHORITY\SYSTEM`, and by whatever
        // the parent directory happened to grant - `%APPDATA%` redirected onto
        // a file server, say. What this cannot do is stop a process running as
        // *this* user; see the module comment on `owner_only_file`.
        let directory = scratch_directory("owner-only");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(STATE_FILE_NAME);
        test_descriptor("token")
            .publish(&path)
            .expect("endpoint document");

        let published = owner_only_file::is_owner_only(&path).expect("inspect the published DACL");
        // The neighbour proves the check can say no: without it a predicate
        // that always returned true would pass the assertion above.
        let neighbour = directory.join("ordinary.json");
        fs::write(&neighbour, b"{}").expect("ordinary file");
        let inherited = owner_only_file::is_owner_only(&neighbour).expect("inspect a plain file");
        // A refresh re-publishes over the existing copy every 30 seconds. The
        // replaced file must not come back with the parent's ACEs.
        test_descriptor("token").publish(&path).expect("refresh");
        let refreshed = owner_only_file::is_owner_only(&path).expect("inspect the refreshed DACL");

        drop(_directory);
        assert!(
            published,
            "the published descriptor still inherits Administrators and SYSTEM access"
        );
        assert!(
            !inherited,
            "an ordinary file in the same directory was also reported owner-only, so this proves nothing"
        );
        assert!(
            refreshed,
            "a descriptor refresh republished the credential under the inherited ACL"
        );
    }

    #[test]
    fn descriptor_publication_never_exposes_a_partial_document() {
        let directory = scratch_directory("atomic");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join("local-mcp.json");
        let publish = |token: &str| {
            test_descriptor(token)
                .publish(&path)
                .expect("endpoint document")
        };
        publish("seed-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

        let stop = Arc::new(AtomicBool::new(false));
        let reader_path = path.clone();
        let reader_stop = stop.clone();
        let permit = FixtureReaderPermit::acquire();
        let reader = std::thread::spawn(move || {
            let mut observed = 0_u32;
            let mut torn = 0_u32;
            let mut refused = 0_u32;
            while !reader_stop.load(Ordering::Relaxed) {
                match fs::read(&reader_path) {
                    Ok(bytes) => {
                        observed += 1;
                        if serde_json::from_slice::<Value>(&bytes).is_err() {
                            torn += 1;
                        }
                    }
                    // Replacing a file by rename is atomic for a reader that
                    // already holds a handle, but a reader *opening* during the
                    // swap can be refused. That is a retryable miss, not a
                    // partial document, and counting it as one made this test
                    // fail under load while claiming a tear that never happened.
                    Err(_) => refused += 1,
                }
            }
            (observed, torn, refused)
        });
        let mut reader = FixtureReader {
            root: directory.clone(),
            stop: stop.clone(),
            worker: Some(reader),
            permit: Some(permit),
        };

        for index in 0..600 {
            publish(&format!("rotated-token-{index:0>30}"));
        }
        stop.store(true, Ordering::Relaxed);
        let (observed, torn, refused) = reader.finish().expect("reader thread");

        drop(_directory);
        assert!(observed > 0, "the reader never saw the descriptor");
        // The invariant that matters: a reader that gets bytes never gets a
        // half-written document. The app publishes three copies and renames
        // them one at a time, so a refusal on one still leaves the others.
        assert_eq!(
            torn, 0,
            "the bridge re-reads this file on every request; {torn} of {observed} reads saw a partial document ({refused} opens were refused mid-rename, which is expected)"
        );
        // No assertion on `refused`. A refusal is expected and handled, its rate
        // depends on how much CPU this thread gets against the rest of the
        // suite, and a threshold I cannot justify is exactly how this test came
        // to fail while reporting a tear that never happened. It is reported
        // above so a future failure is diagnosable.
    }

    #[test]
    fn descriptor_is_additive_revision_four() {
        let directory = scratch_directory("revision");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join("local-mcp.json");
        test_descriptor("token")
            .publish(&path)
            .expect("endpoint document");
        let document: Value = serde_json::from_slice(&fs::read(&path).expect("endpoint bytes"))
            .expect("endpoint JSON");
        assert_eq!(document["schemaVersion"], 1);
        assert_eq!(document["descriptorRevision"], 4);
        assert_eq!(document["transport"], "streamable-http");
        assert_eq!(document["healthUrl"], "http://127.0.0.1:41000/health");
        assert_eq!(document["rpcUrl"], "http://127.0.0.1:41000/rpc");
        assert_eq!(document["instanceId"], "instance");
        assert!(document["publishedAtUnixMs"].as_u64().is_some());
        drop(_directory);
    }

    #[test]
    fn a_refresh_restamps_the_timestamp_and_moves_nothing_else() {
        // The whole liveness contract rests on this: a refresh that changed the
        // token, the port or the instance id would read as a restart, and a
        // reader would be right to throw away the session it had.
        let directory = scratch_directory("refresh");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(STATE_FILE_NAME);
        let descriptor = test_descriptor("stable-token");
        descriptor
            .publish_at(&path, 1_700_000_000_000)
            .expect("first publication");
        let first: Value =
            serde_json::from_slice(&fs::read(&path).expect("first bytes")).expect("first document");
        descriptor
            .publish_at(&path, 1_700_000_030_000)
            .expect("refresh");
        let second: Value = serde_json::from_slice(&fs::read(&path).expect("refreshed bytes"))
            .expect("refreshed document");
        drop(_directory);

        assert_eq!(first["publishedAtUnixMs"], 1_700_000_000_000_u64);
        assert_eq!(second["publishedAtUnixMs"], 1_700_000_030_000_u64);
        for field in [
            "schemaVersion",
            "descriptorRevision",
            "transport",
            "url",
            "healthUrl",
            "rpcUrl",
            "bearerToken",
            "processId",
            "instanceId",
            "toolContractSha256",
        ] {
            assert_eq!(
                first[field], second[field],
                "{field} moved across a refresh, which a reader reads as a restart"
            );
        }
        let moved: Vec<&String> = second
            .as_object()
            .expect("refreshed object")
            .keys()
            .filter(|key| first[key.as_str()] != second[key.as_str()])
            .collect();
        assert_eq!(moved, vec!["publishedAtUnixMs"]);
    }

    /// The shipped cadence and the reader's window, run at 1:100 so the ratio
    /// under test is the one that ships. `tokio`'s paused clock would be exact,
    /// but it needs the `test-util` feature this crate does not carry.
    const TEST_TIME_SCALE: u32 = 100;

    #[tokio::test]
    async fn refreshes_fill_the_readers_stale_window_and_stop_on_shutdown() {
        // Readers retire a descriptor 150 s after its last refresh. Several
        // refreshes inside that window is the whole margin: consecutive writes
        // have to fail, not one, before a live app is taken for a dead one.
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let ticks = Arc::new(AtomicU64::new(0));
        let counted = ticks.clone();
        let refreshing = tokio::spawn(async move {
            refresh_until_shutdown(
                DESCRIPTOR_REFRESH_INTERVAL / TEST_TIME_SCALE,
                &mut shutdown_rx,
                move || {
                    counted.fetch_add(1, Ordering::Relaxed);
                },
            )
            .await;
        });
        tokio::time::sleep(DESCRIPTOR_STALE_AFTER / TEST_TIME_SCALE).await;
        let inside_the_window = ticks.load(Ordering::Relaxed);
        assert!(
            (4..=6).contains(&inside_the_window),
            "the 5x ratio puts four to six refreshes inside the reader's window, saw {inside_the_window}"
        );
        shutdown.send(true).expect("shutdown");
        refreshing.await.expect("refresh task");
        let at_shutdown = ticks.load(Ordering::Relaxed);
        // A tick that fired after the stop would republish a descriptor the
        // stop path has already removed: the stale-file bug over again.
        tokio::time::sleep(DESCRIPTOR_REFRESH_INTERVAL / TEST_TIME_SCALE * 3).await;
        let after_shutdown = ticks.load(Ordering::Relaxed);

        assert_eq!(
            after_shutdown, at_shutdown,
            "a refresh fired after shutdown was signalled"
        );
    }

    #[tokio::test]
    async fn a_refresh_never_fires_when_shutdown_is_already_signalled() {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let ticks = Arc::new(AtomicU64::new(0));
        let counted = ticks.clone();
        shutdown.send(true).expect("shutdown before the first tick");
        refresh_until_shutdown(
            DESCRIPTOR_REFRESH_INTERVAL / TEST_TIME_SCALE,
            &mut shutdown_rx,
            move || {
                counted.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await;
        assert_eq!(ticks.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_failed_publication_removes_only_the_copies_this_process_wrote() {
        // Regression, both directions. Returning on the first failed path left
        // a valid-looking descriptor advertising a port that was dropped on the
        // next line; the other branch removed all three copies, including ones
        // a live instance had published to the shared mirror paths.
        let directory = scratch_directory("partial");
        let _directory = OwnedTestDirectory(directory.clone());
        let mine = directory.join(STATE_FILE_NAME);
        // `create_dir_all` cannot make a directory where a file already is.
        let blocker = directory.join("blocked");
        fs::write(&blocker, b"not a directory").expect("blocker");
        let unwritable = blocker.join(STATE_FILE_NAME);
        let another_instance = directory.join("other").join(STATE_FILE_NAME);
        fs::create_dir_all(another_instance.parent().expect("other parent")).expect("other dir");
        fs::write(&another_instance, b"{\"bearerToken\":\"live-elsewhere\"}").expect("other");

        let error = publish_all_or_nothing(
            &test_descriptor("token"),
            &[mine.clone(), unwritable, another_instance.clone()],
        )
        .expect_err("a path that cannot be created must fail the publication");

        let mine_remains = mine.exists();
        let other_remains = fs::read(&another_instance).expect("other bytes");
        drop(_directory);
        assert!(error.contains("local MCP state directory"), "{error}");
        assert!(
            !mine_remains,
            "a half-published descriptor advertises a port nothing will listen on"
        );
        assert_eq!(
            other_remains, b"{\"bearerToken\":\"live-elsewhere\"}",
            "a failed publication deleted a descriptor this process never wrote"
        );
    }

    #[test]
    fn a_complete_publication_writes_every_copy() {
        let directory = scratch_directory("complete");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![
            directory.join(STATE_FILE_NAME),
            directory.join("second").join(STATE_FILE_NAME),
            directory.join("claude").join(STATE_FILE_NAME),
        ];
        publish_all_or_nothing(&test_descriptor("token"), &paths).expect("publication");
        let published: Vec<bool> = paths.iter().map(|path| path.exists()).collect();
        remove_published_descriptors(&paths, "instance");
        let removed: Vec<bool> = paths.iter().map(|path| path.exists()).collect();
        drop(_directory);
        assert_eq!(published, vec![true, true, true]);
        assert_eq!(removed, vec![false, false, false]);
    }

    #[test]
    fn a_stop_closes_the_port_and_removes_every_descriptor_it_published() {
        // The descriptor outliving the process is what makes clients dial dead
        // ports: two were found on disk from separate app runs, because
        // `Drop` never ran and no `RunEvent::Exit` handler existed.
        let directory = scratch_directory("lifecycle");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![
            directory.join(STATE_FILE_NAME),
            directory.join("mirror").join(STATE_FILE_NAME),
        ];
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
        let listener = start_listener(paths.clone(), adapter, access_rx).expect("listener");

        let published: Vec<bool> = paths.iter().map(|path| path.exists()).collect();
        let address = format!("127.0.0.1:{}", listener.port);
        let served = std::net::TcpStream::connect(&address).is_ok();
        listener.stop();
        let remaining: Vec<bool> = paths.iter().map(|path| path.exists()).collect();
        let refused = std::net::TcpStream::connect(&address).is_err();
        drop(_directory);

        assert_eq!(published, vec![true, true]);
        assert!(
            served,
            "the descriptor was published before the port answered"
        );
        assert_eq!(
            remaining,
            vec![false, false],
            "a stopped app left a descriptor pointing at a port it no longer owns"
        );
        assert!(refused, "the listener is still accepting after a stop");
    }

    #[test]
    fn a_second_instance_does_not_take_over_a_live_instances_descriptor() {
        // The GUI now has an application-level single-instance guard, but the
        // listener must still arbitrate races with the headless bridge and
        // direct hosts. A second publisher must not bind its own ephemeral
        // port, mint its own token, or overwrite the live descriptor.
        let directory = scratch_directory("second-instance");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![
            directory.join(STATE_FILE_NAME),
            directory.join("mirror").join(STATE_FILE_NAME),
        ];
        let first_adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let (_first_access_tx, first_access_rx) = watch::channel(AccessMode::Full);
        let first = start_listener(paths.clone(), first_adapter, first_access_rx)
            .expect("the first instance publishes");
        let published = fs::read(&paths[0]).expect("the first instance's descriptor");
        // Each copy carries its own publication timestamp, so the mirror is
        // compared against its own bytes rather than against the primary's.
        let published_mirror = fs::read(&paths[1]).expect("the first instance's mirrored copy");

        let second_adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let (_second_access_tx, second_access_rx) = watch::channel(AccessMode::Full);
        let second = start_listener(paths.clone(), second_adapter, second_access_rx);
        let failure = second.err();

        let after = fs::read(&paths[0]).expect("the descriptor after a second start");
        let mirror = fs::read(&paths[1]).expect("the mirrored descriptor after a second start");
        let first_still_answers =
            std::net::TcpStream::connect(format!("127.0.0.1:{}", first.port)).is_ok();
        first.stop();
        drop(_directory);

        assert_eq!(
            after, published,
            "a second instance overwrote the descriptor a live instance is serving, so every paired client migrates to it"
        );
        assert_eq!(
            mirror, published_mirror,
            "a second instance overwrote a mirrored copy the live instance published"
        );
        let failure = failure.expect(
            "a second instance started serving as the published endpoint while another one already was",
        );
        assert!(
            failure.served_by_another_instance,
            "the refusal is not distinguishable from an ordinary start-up failure: {}",
            failure.message
        );
        assert!(
            failure
                .message
                .contains("Another process owns the local MCP publisher lease"),
            "lastError does not say what happened: {}",
            failure.message
        );
        assert!(
            first_still_answers,
            "the first instance stopped answering, so this test proves nothing about the descriptor"
        );
    }

    #[test]
    fn a_descriptor_whose_owner_is_gone_never_locks_a_fresh_start_out() {
        // The trap in guarding publication: a crashed instance leaves a
        // descriptor that is still inside the reader's 150s window and still
        // names a plausible pid. Refusing on either of those would keep the
        // app from ever starting again for two and a half minutes.
        let directory = scratch_directory("owner-gone");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(STATE_FILE_NAME);
        let dead_port = {
            let bound = StdTcpListener::bind("127.0.0.1:0").expect("a port to abandon");
            bound.local_addr().expect("abandoned address").port()
        };
        descriptor_for_port(dead_port, "an-instance-that-crashed")
            .publish(&path)
            .expect("the crashed instance's descriptor");

        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let listener = start_listener(vec![path.clone()], adapter, full_access());
        let outcome = listener
            .as_ref()
            .err()
            .map(|failure| failure.message.clone());
        let republished = listener
            .as_ref()
            .ok()
            .map(|listener| (listener.instance_id.clone(), published_instance(&path)));
        if let Ok(listener) = listener {
            listener.stop();
        }
        drop(_directory);

        assert_eq!(outcome, None, "a dead owner blocked a fresh start");
        let (started, on_disk) = republished.expect("the fresh instance published");
        assert_eq!(
            on_disk, started,
            "the fresh instance did not take the descriptor over from the dead one"
        );
    }

    #[test]
    fn an_expired_or_unreadable_descriptor_is_republished() {
        let directory = scratch_directory("expired-or-unreadable");
        let _directory = OwnedTestDirectory(directory.clone());
        let expired = directory.join(STATE_FILE_NAME);
        let unreadable = directory.join("unreadable").join(STATE_FILE_NAME);
        fs::create_dir_all(unreadable.parent().expect("parent")).expect("unreadable directory");
        // Published 200s ago against the readers' 150s allowance: a client
        // would already be ignoring it.
        descriptor_for_port(41000, "an-instance-that-stopped-refreshing")
            .publish_at(&expired, unix_time_ms().saturating_sub(200_000))
            .expect("expired descriptor");
        fs::write(&unreadable, b"{ this is not a descriptor").expect("unreadable descriptor");

        let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
            ready: AtomicBool::new(true),
        });
        let listener = start_listener(
            vec![expired.clone(), unreadable.clone()],
            adapter,
            full_access(),
        );
        let refused = listener
            .as_ref()
            .err()
            .map(|failure| failure.message.clone());
        let republished = listener.as_ref().ok().map(|listener| {
            (
                listener.instance_id.clone(),
                published_instance(&expired),
                published_instance(&unreadable),
            )
        });
        if let Ok(listener) = listener {
            listener.stop();
        }
        drop(_directory);

        assert_eq!(refused, None, "a stale file locked a fresh start out");
        let (started, expired_now, unreadable_now) = republished.expect("the fresh instance");
        assert_eq!(expired_now, started);
        assert_eq!(unreadable_now, started);
    }

    #[test]
    fn a_stranger_on_the_recycled_port_does_not_hold_the_descriptor() {
        // Windows hands the ephemeral port out again, so "something is
        // listening there" is not evidence that QC Remote is. Only an answer
        // carrying the descriptor's own instance id is.
        let directory = scratch_directory("recycled-port");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(STATE_FILE_NAME);
        let stranger = StdTcpListener::bind("127.0.0.1:0").expect("a stranger's port");
        let port = stranger.local_addr().expect("stranger address").port();
        stranger.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let accepting = std::thread::scope(|scope| {
            let _release_on_unwind = FixtureStartRelease(stop.clone());
            let stopping = stop.clone();
            let accepting = scope.spawn(move || {
                // Accept and drop: a connection succeeds, no QC Remote answers.
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !stopping.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
                    match stranger.accept() {
                        Ok(_) => return,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(_) => return,
                    }
                }
            });
            descriptor_for_port(port, "an-instance-that-is-gone")
                .publish(&path)
                .expect("descriptor naming the recycled port");

            let adapter: Arc<dyn DeviceAdapter> = Arc::new(FakeAdapter {
                ready: AtomicBool::new(true),
            });
            let listener = start_listener(vec![path.clone()], adapter, full_access());
            let refused = listener
                .as_ref()
                .err()
                .map(|failure| failure.message.clone());
            let republished = listener
                .as_ref()
                .ok()
                .map(|listener| (listener.instance_id.clone(), published_instance(&path)));
            if let Ok(listener) = listener {
                listener.stop();
            }
            stop.store(true, Ordering::Release);
            let _ = accepting.join();
            (refused, republished)
        });
        let (refused, republished) = accepting;
        drop(_directory);

        assert_eq!(
            refused, None,
            "an unrelated listener on the recycled port blocked a fresh start"
        );
        let (started, on_disk) = republished.expect("the fresh instance published");
        assert_eq!(on_disk, started);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_connection_test_makes_one_read_only_call_and_names_what_it_found() {
        let directory = scratch_directory("connection-test");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![directory.join(STATE_FILE_NAME)];
        let adapter = Arc::new(RecordingAdapter::new(true));
        let listener =
            start_listener(paths.clone(), adapter.clone(), full_access()).expect("listener");
        let status = running_status(&listener.instance_id);

        let outcome = probe_local_mcp(&paths, &status, unix_time_ms()).await;
        let calls = adapter.calls();
        listener.stop();
        drop(_directory);

        assert_eq!(outcome.state, LocalMcpTestState::Ready);
        assert_eq!(
            outcome.preset_name.as_deref(),
            Some("Rehearsal Clean"),
            "success has to be specific enough to be believed"
        );
        assert_eq!(
            calls,
            vec!["system.status".to_owned()],
            "a connection test must be one read-only call, and nothing that touches the device"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_connection_test_separates_the_faults_that_have_different_remedies() {
        let directory = scratch_directory("connection-test-faults");
        let _directory = OwnedTestDirectory(directory.clone());
        let paths = vec![directory.join(STATE_FILE_NAME)];

        let off = probe_local_mcp(
            &paths,
            &LocalAccessStatus {
                enabled: false,
                ..running_status("this-instance")
            },
            unix_time_ms(),
        )
        .await;
        let nothing_published =
            probe_local_mcp(&paths, &running_status("this-instance"), unix_time_ms()).await;

        let adapter = Arc::new(RecordingAdapter::new(false));
        let listener =
            start_listener(paths.clone(), adapter.clone(), full_access()).expect("listener");
        let status = running_status(&listener.instance_id);
        let no_device = probe_local_mcp(&paths, &status, unix_time_ms()).await;
        let another_copy = probe_local_mcp(
            &paths,
            &running_status("a-different-copy-of-the-app"),
            unix_time_ms(),
        )
        .await;

        // A descriptor whose credential no longer matches the listener: the
        // shape a rotated token leaves behind.
        let published: Value =
            serde_json::from_slice(&fs::read(&paths[0]).expect("descriptor")).expect("descriptor");
        let mut rotated = published.clone();
        rotated["bearerToken"] = Value::String("R".repeat(43));
        fs::write(&paths[0], serde_json::to_vec(&rotated).expect("rotated")).expect("rotate");
        let rejected = probe_local_mcp(&paths, &status, unix_time_ms()).await;

        // The app is gone but its descriptor is still fresh: what a crash
        // leaves for the next 150 seconds.
        listener.stop();
        fs::write(
            &paths[0],
            serde_json::to_vec(&published).expect("republished"),
        )
        .expect("republish");
        let gone = probe_local_mcp(&paths, &status, unix_time_ms()).await;
        let calls = adapter.calls();
        drop(_directory);

        assert_eq!(off.state, LocalMcpTestState::Disabled);
        assert_eq!(nothing_published.state, LocalMcpTestState::NotPublished);
        assert_eq!(no_device.state, LocalMcpTestState::DeviceNotConnected);
        assert_eq!(another_copy.state, LocalMcpTestState::ServedByAnotherCopy);
        assert_eq!(rejected.state, LocalMcpTestState::Unauthorized);
        assert_eq!(gone.state, LocalMcpTestState::NotAnswering);
        assert!(
            no_device.preset_name.is_none(),
            "a preset name without a device would be a stale reading"
        );
        assert_eq!(
            calls.len(),
            2,
            "only the two tests that reached a running app should have called the device: {calls:?}"
        );
    }

    #[test]
    fn the_connection_test_keeps_the_names_the_panel_switches_on() {
        // The panel maps each of these to a different sentence with a
        // different next action. A renamed variant would fall through to its
        // default branch and report "not answering" for a state that is
        // nothing of the kind, which is how this feature got its reputation.
        let wire: Vec<String> = [
            LocalMcpTestState::Disabled,
            LocalMcpTestState::NotPublished,
            LocalMcpTestState::NotAnswering,
            LocalMcpTestState::Unauthorized,
            LocalMcpTestState::ServedByAnotherCopy,
            LocalMcpTestState::DeviceNotConnected,
            LocalMcpTestState::Ready,
        ]
        .iter()
        .map(|state| {
            serde_json::to_value(state)
                .expect("state")
                .as_str()
                .expect("a string variant")
                .to_owned()
        })
        .collect();
        assert_eq!(
            wire,
            vec![
                "disabled",
                "notPublished",
                "notAnswering",
                "unauthorized",
                "servedByAnotherCopy",
                "deviceNotConnected",
                "ready"
            ]
        );

        let result = serde_json::to_value(local_test_result(
            LocalMcpTestState::Ready,
            Some("Rehearsal Clean".into()),
            Some("a diagnosis".into()),
        ))
        .expect("result");
        assert_eq!(result["state"], "ready");
        assert_eq!(result["presetName"], "Rehearsal Clean");
        assert_eq!(result["detail"], "a diagnosis");
        assert!(
            result["appVersion"]
                .as_str()
                .is_some_and(|version| version.contains('.')),
            "the panel interpolates appVersion into the sentence it shows on success"
        );

        let displaced = serde_json::to_value(LocalAccessStatus {
            served_by_another_instance: true,
            ..LocalAccessStatus::default()
        })
        .expect("status");
        assert_eq!(displaced["servedByAnotherInstance"], true);
    }

    #[test]
    fn local_mcp_is_on_until_this_user_turns_it_off() {
        let directory = scratch_directory("preference");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(ENABLED_FILE_NAME);
        let fresh_install = load_enabled(&path);
        store_enabled(&path, false).expect("store off");
        let after_off = load_enabled(&path);
        store_enabled(&path, true).expect("store on");
        let after_on = load_enabled(&path);
        fs::write(&path, b"{ this is not json").expect("corrupt preference");
        let after_corruption = load_enabled(&path);
        drop(_directory);

        assert!(fresh_install, "local MCP must ship on");
        assert!(!after_off, "the off switch did not survive a reload");
        assert!(after_on);
        assert!(
            after_corruption,
            "an unreadable preference must not silently opt a user out"
        );
    }

    #[test]
    fn failed_preference_publication_preserves_the_durable_disabled_choice() {
        let directory = scratch_directory("preference-publication-failure");
        let _directory = OwnedTestDirectory(directory.clone());
        let path = directory.join(ENABLED_FILE_NAME);
        store_enabled(&path, false).unwrap();
        let previous = fs::read(&path).unwrap();
        let error = store_enabled_with(&path, true, |destination, parent, bytes| {
            assert_eq!(destination, path);
            assert_eq!(parent, directory);
            assert_eq!(
                serde_json::from_slice::<Value>(bytes).unwrap()["enabled"],
                true
            );
            // Drive the real owned publisher through a failed rename, rather
            // than deleting/truncating the live preference to manufacture an error.
            let blocked = parent.join("blocked-preference");
            fs::create_dir(&blocked).unwrap();
            publish_atomically(&blocked, parent, "the local MCP preference", bytes)
        })
        .unwrap_err();
        assert!(error.contains("Could not publish"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert!(!load_enabled(&path));
        assert!(!fs::read_dir(&directory).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".blocked-preference.")
        }));
    }

    /// The background host inside `qc-device-broker.exe`, with no MCP client
    /// attached: its gateway is detached, and its broker is a probe answering
    /// what the in-process broker's cached session would.
    fn detached_host(
        answer: impl Fn() -> crate::local_stdio::BrokerDeviceReadiness + Send + Sync + 'static,
    ) -> crate::local_stdio::HeadlessDeviceAdapter {
        let adapter = crate::local_stdio::HeadlessDeviceAdapter::new(
            std::env::temp_dir().join(format!(
                "qc-health-readiness-unused-{}.json",
                rand::random::<u64>()
            )),
            false,
        )
        .with_broker_readiness(Arc::new(answer));
        // What the background host does on every tick with no client.
        adapter.release_when_unused();
        assert!(!adapter.gateway_attached());
        adapter
    }

    async fn health_of(adapter: &crate::local_stdio::HeadlessDeviceAdapter) -> (StatusCode, Value) {
        let adapter: Arc<dyn DeviceAdapter> = Arc::new(adapter.clone());
        let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access: access_rx,
        });
        let router = local_router(
            backend,
            LocalHealth {
                adapter,
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        );
        let response = router
            .oneshot(health_request(Some("test-token")))
            .await
            .expect("health response");
        let status = response.status();
        let body = serde_json::from_slice(
            &to_bytes(response.into_body(), 16 * 1024)
                .await
                .expect("health body"),
        )
        .expect("health JSON");
        (status, body)
    }

    /// The defect: with no MCP client attached the background host's gateway
    /// is detached, and `/health` answered from its cleared `connected` flag -
    /// 503 `device_not_ready` while the broker held a ready Quad Cortex. It
    /// answers from the broker now, and reading it attaches nothing.
    #[tokio::test]
    async fn a_detached_host_reports_the_brokers_ready_device_as_ready() {
        use crate::local_stdio::{BrokerDeviceReadiness, HostReadiness};
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = reads.clone();
        let adapter = detached_host(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            BrokerDeviceReadiness::Ready
        });
        assert!(adapter.ready().await);
        assert_eq!(adapter.readiness().await, HostReadiness::Ready);
        let (status, body) = health_of(&adapter).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["deviceReady"], true);
        assert_eq!(body["appReady"], true);
        assert!(body.get("reason").is_none(), "{body}");
        assert_eq!(
            reads.load(Ordering::SeqCst),
            3,
            "each probe asked the broker"
        );
        assert!(
            !adapter.gateway_attached(),
            "a readiness read attached the gateway to the broker"
        );
    }

    #[tokio::test]
    async fn a_detached_host_reports_the_brokers_closed_session_as_not_ready() {
        use crate::local_stdio::{BrokerDeviceReadiness, HostReadiness};
        let adapter = detached_host(|| BrokerDeviceReadiness::NotReady {
            phase: "disconnected".into(),
        });
        assert!(!adapter.ready().await);
        let HostReadiness::NotReady(reason) = adapter.readiness().await else {
            panic!("ready");
        };
        assert!(reason.contains("disconnected"), "{reason}");
        let (status, body) = health_of(&adapter).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["deviceReady"], false);
        assert_eq!(body["code"], "LOCAL_UNAVAILABLE");
        assert_eq!(body["reason"], "device_not_ready");
        assert!(!adapter.gateway_attached());
    }

    /// An unreachable broker, a broker that cannot read its device phase, a
    /// wedged read and a failed read each answer not ready with a reason,
    /// promptly, and none of them attaches or opens anything.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreadable_broker_answers_not_ready_promptly_and_opens_nothing() {
        use crate::local_stdio::{BrokerDeviceReadiness, HostReadiness};
        let unavailable = detached_host(|| BrokerDeviceReadiness::Unavailable {
            reason: "the device broker is shutting down".into(),
        });
        assert_eq!(
            unavailable.readiness().await,
            HostReadiness::NotReady("the device broker is shutting down".into())
        );
        let (status, body) = health_of(&unavailable).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason"], "device_not_ready");

        let failing = detached_host(|| panic!("the broker's status could not be read"));
        let HostReadiness::NotReady(reason) = failing.readiness().await else {
            panic!("ready");
        };
        assert!(reason.contains("failed"), "{reason}");
        let (status, _) = health_of(&failing).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        // The first read wedges until released; later reads answer at once.
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(Some(release_rx));
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = probes.clone();
        let wedged = detached_host(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            if let Some(release) = release_rx.lock().unwrap().take() {
                let _ = release.recv_timeout(Duration::from_secs(30));
            }
            BrokerDeviceReadiness::Ready
        });
        let started = std::time::Instant::now();
        let HostReadiness::NotReady(reason) = wedged.readiness().await else {
            panic!("a wedged broker read answered ready");
        };
        assert!(reason.contains("did not answer"), "{reason}");
        assert!(started.elapsed() < Duration::from_secs(5), "the read hung");
        // Later reads join the wedged one, whose budget is already spent, so
        // they answer at once and never start a second blocking read.
        let started = std::time::Instant::now();
        let HostReadiness::NotReady(reason) = wedged.readiness().await else {
            panic!("ready while a read is wedged");
        };
        assert!(reason.contains("did not answer"), "{reason}");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a read joined past the deadline waited"
        );
        let (status, _) = health_of(&wedged).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(probes.load(Ordering::SeqCst), 1, "a wedged read piled up");
        release_tx.send(()).expect("release");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while wedged.readiness().await != HostReadiness::Ready {
            assert!(
                std::time::Instant::now() < deadline,
                "the read never cleared"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for adapter in [&unavailable, &failing, &wedged] {
            assert!(!adapter.gateway_attached(), "a readiness read attached");
        }
    }

    /// The relay's `DeviceFrame::Ready { usb_connected }` is
    /// `timeout(.., adapter.ready())` through the `DeviceAdapter` trait on
    /// every one-second tick (qc-relay-client, pinned there by
    /// `the_ready_frame_follows_readiness_changes_with_no_other_event`). Read
    /// the same way here, a detached host follows the broker both ways with
    /// no client and no event in between.
    #[tokio::test]
    async fn the_relay_readiness_of_a_detached_host_follows_the_broker() {
        use crate::local_stdio::BrokerDeviceReadiness;
        let open = Arc::new(AtomicBool::new(true));
        let session = open.clone();
        let host: Arc<dyn DeviceAdapter> = Arc::new(detached_host(move || {
            if session.load(Ordering::SeqCst) {
                BrokerDeviceReadiness::Ready
            } else {
                BrokerDeviceReadiness::NotReady {
                    phase: "disconnected".into(),
                }
            }
        }));
        for expected in [true, false, true] {
            open.store(expected, Ordering::SeqCst);
            let usb_connected = tokio::time::timeout(Duration::from_secs(10), host.ready())
                .await
                .unwrap_or(false);
            assert_eq!(usb_connected, expected);
        }
    }

    /// Records, for every device call it receives, whether that call was
    /// served as an MCP resource read.
    struct OriginRecordingAdapter {
        origins: Mutex<Vec<(String, bool)>>,
    }

    #[async_trait]
    impl DeviceAdapter for OriginRecordingAdapter {
        async fn ready(&self) -> bool {
            true
        }

        async fn invoke(&self, method: &str, _params: Value) -> Result<Value, DeviceError> {
            self.origins
                .lock()
                .unwrap()
                .push((method.to_owned(), qc_remote_mcp::serving_resource_read()));
            Ok(serde_json::json!({}))
        }
    }

    /// `/rpc` serves a relayed resource read inside the same
    /// scope the host's own resource reads run in, so its gateway uses only
    /// the broker connection it holds. Only behind the bearer check, only
    /// for a method a resource uses, and only with the exact value; anything
    /// else is handled as before, and the marker can only narrow a request.
    #[tokio::test]
    async fn the_resource_read_origin_is_honoured_only_behind_auth_for_resource_methods() {
        let adapter = Arc::new(OriginRecordingAdapter {
            origins: Mutex::new(Vec::new()),
        });
        let (_access_tx, access_rx) = watch::channel(AccessMode::Full);
        let backend: Arc<dyn QcBackend> = Arc::new(LocalBackend {
            adapter: adapter.clone(),
            access: access_rx,
        });
        let router = local_router(
            backend,
            LocalHealth {
                adapter: adapter.clone(),
                instance_id: Arc::from("test-instance"),
                tool_contract_sha256: Arc::from(tool_contract_sha256()),
                process_id: 42,
            },
            Arc::from("test-token"),
            allowed_hosts(41000),
        );
        let marked = |rpc: &str, token: Option<&str>, value: &str| {
            let mut request = rpc_request(rpc, token);
            request
                .headers_mut()
                .insert(REQUEST_ORIGIN_HEADER, value.parse().unwrap());
            request
        };
        let resource_rpc = qc_remote_mcp::resource_rpcs()
            .into_iter()
            .map(|(_, rpc)| rpc)
            .find(|rpc| *rpc != "system.status")
            .expect("a resource rpc that is also a tool's");

        for request in [
            marked(resource_rpc, Some("test-token"), RESOURCE_READ_ORIGIN),
            rpc_request(resource_rpc, Some("test-token")),
            marked("device.setTempo", Some("test-token"), RESOURCE_READ_ORIGIN),
            marked(resource_rpc, Some("test-token"), "tool-call"),
        ] {
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                StatusCode::OK
            );
        }
        let refused = router
            .clone()
            .oneshot(marked(resource_rpc, None, RESOURCE_READ_ORIGIN))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

        assert_eq!(
            *adapter.origins.lock().unwrap(),
            vec![
                (resource_rpc.to_owned(), true),
                (resource_rpc.to_owned(), false),
                ("device.setTempo".to_owned(), false),
                (resource_rpc.to_owned(), false),
            ]
        );
    }
}
