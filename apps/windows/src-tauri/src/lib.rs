use broker_attach::{attach_to_device_broker, read_frame, write_frame, BrokerAttachFailure};
#[cfg(test)]
use broker_attach::{
    published_broker_process_is_alive, published_process_is_alive_with,
    DEVICE_BROKER_AUTHENTICATE_METHOD,
};
#[cfg(test)]
use qc_native_platform::process::{os_process_created_at, os_process_is_alive};
use qc_relay_client::{DeviceAdapter, DeviceError};
use quadcortex::device::{generated_gateway, generated_gateway::rpc};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufReader, Read, Write};
#[cfg(test)]
use std::net::SocketAddr;
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

mod access_policy;
mod access_policy_store;
mod background_host;
mod broker_attach;
mod broker_installations;
pub mod broker_paths;
pub mod broker_tray_control;
pub mod broker_tray_settings;
mod client_packages;
mod credential_io;
mod credential_store;
mod demo_provider;
mod device_broker_descriptor;
mod external_host;
mod external_links;
#[allow(dead_code)]
// Shared surface: this host uses descriptor/participant; the broker draws the
// tray readings itself.
mod generated_broker_surface;
mod host_presence;
mod host_runtime_status;
mod local_access;
mod local_identity;
mod local_stdio;
mod loopback_http;
mod os_version;
mod owned_blocking;
mod owner_only_file;
mod relay;
mod relay_task;
mod remote_backup;
mod remote_grants;
mod remote_unlink;
mod system_tools;
#[cfg(test)]
mod test_health_dir;
#[cfg(windows)]
mod tray_native;
#[allow(dead_code)] // The hosts publish these types; the broker icon reads the published file.
mod tray_presence;
mod virtual_device_store;
mod workspace;

#[derive(Default)]
struct MacMenuEventState {
    frontend_ready: bool,
    pending: Vec<&'static str>,
}

#[derive(Default)]
struct MacMenuEventBridge(Mutex<MacMenuEventState>);

impl MacMenuEventBridge {
    #[cfg(any(target_os = "macos", test))]
    fn dispatch(&self, event: &'static str) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.frontend_ready {
            true
        } else {
            state.pending.push(event);
            false
        }
    }

    fn frontend_ready(&self) -> Vec<&'static str> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.frontend_ready = true;
        std::mem::take(&mut state.pending)
    }
}

#[cfg(test)]
mod mac_menu_event_bridge_tests {
    use super::MacMenuEventBridge;

    #[test]
    fn queues_menu_events_until_frontend_listeners_are_ready() {
        let bridge = MacMenuEventBridge::default();

        assert!(!bridge.dispatch("macos-menu-connect"));
        assert!(!bridge.dispatch("macos-menu-disconnect"));
        assert_eq!(
            bridge.frontend_ready(),
            ["macos-menu-connect", "macos-menu-disconnect"]
        );
        assert!(bridge.frontend_ready().is_empty());
        assert!(bridge.dispatch("macos-menu-connect"));
    }
}

use client_packages::{
    antigravity_local_integration_status, chatgpt_local_integration_status,
    claude_desktop_extension_status, install_antigravity_local_integration,
    install_chatgpt_local_integration, install_claude_desktop_extension,
    install_jan_local_integration, install_lm_studio_local_integration,
    jan_local_integration_status, lm_studio_local_integration_status, open_antigravity,
    open_chatgpt_configuration, open_chatgpt_desktop, open_claude_desktop, open_jan,
    open_lm_studio, other_mcp_stdio_setup, refresh_managed_routes_on_start,
    remove_antigravity_local_integration, remove_chatgpt_local_integration,
    remove_jan_local_integration, remove_lm_studio_local_integration, save_local_client_package,
};
// Microsoft Store policy 10.1.5: the Windows build never opens a third-party
// download page, so these commands are compiled on the other platforms only.
#[cfg(not(windows))]
use client_packages::{
    open_antigravity_download, open_chatgpt_desktop_download, open_claude_desktop_download,
    open_jan_download, open_lm_studio_download,
};

#[tauri::command]
fn macos_menu_events_ready(app: AppHandle) -> Result<(), String> {
    let Some(bridge) = app.try_state::<MacMenuEventBridge>() else {
        return Ok(());
    };
    for event in bridge.frontend_ready() {
        if let Err(error) = app.emit(event, ()) {
            eprintln!("QC Remote could not deliver queued macOS menu action: {error}");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn emit_macos_menu_event(app: &tauri::AppHandle, event: &'static str) {
    let bridge = app.state::<MacMenuEventBridge>();
    if bridge.dispatch(event) {
        if let Err(error) = app.emit(event, ()) {
            eprintln!("QC Remote could not deliver macOS menu action: {error}");
        }
    }
}
use local_access::{test_local_mcp, LocalAccess, LocalAccessStatus};
use relay::{BrowserPairingProgress, RelayBridge, RelayStatus};
use virtual_device_store::{read_virtual_device_store, write_virtual_device_store};
#[cfg(test)]
use workspace::validate_native_backup;
use workspace::{
    backup_file_stem, safe_workspace_name, validate_backup_name, validate_workspace,
    validate_workspace_path, write_native_backup, write_workspace,
};

pub fn run_local_mcp_bridge() -> Result<(), String> {
    local_stdio::run()
}
pub use background_host::BackgroundMcpHost;
pub use local_stdio::{BrokerDeviceReadiness, BrokerReadinessProbe};
/// `host_token` is the token the broker in this process generated
/// (`qc_device_broker::resident::host_token`); the host presents it in its
/// `broker.hello`. It never leaves the process. `readiness` reads that
/// broker's device session without a request, for `/health` and the relay
/// while the host has no MCP client attached.
pub fn start_background_mcp_host(
    directory: PathBuf,
    host_token: String,
    readiness: BrokerReadinessProbe,
) -> Result<BackgroundMcpHost, String> {
    background_host::start(directory, host_token, readiness)
}

/// The broker icon's Unlink on a local MCP client, called in process by
/// `qc-device-broker.exe`: ends the one session this process's host published
/// under `session_id` (a local bridge's lease or a loopback HTTP session).
/// Returns false when there is no such session. It adds no block: a native
/// bridge's heartbeat no longer lists the session, and that chat's next tool
/// call records it afresh. Nothing about it outlives this process.
pub fn end_local_mcp_session(session_id: &str) -> bool {
    host_presence::end_local_session(session_id)
}

pub use remote_unlink::RemoteMcpUnlink;

/// The broker icon's Unlink on a remote MCP client, called in process by
/// `qc-device-broker.exe` on its action thread (blocking, bounded): revokes the
/// OAuth grant this process's live relay presence holds for `session_id`, with
/// the Remote panel's own revoke call, and nothing else. The grant never comes
/// from the caller.
pub fn unlink_remote_mcp_session(session_id: &str) -> Result<RemoteMcpUnlink, String> {
    remote_unlink::unlink_remote_session(session_id)
}

/// The broker icon's Unlink on a linked remote app (one OAuth grant), called
/// in process by `qc-device-broker.exe` on its action thread (blocking,
/// bounded): revokes the grant this process's own linked-apps list holds for
/// the opaque `entry_id`, whether or not a session is live, then refreshes
/// that list. The grant never comes from the caller.
pub fn unlink_linked_remote_app(entry_id: &str) -> Result<RemoteMcpUnlink, String> {
    remote_unlink::unlink_linked_app(entry_id)
}

/// Set once by `qc-device-broker.exe`, the process that *is* the broker: no
/// gateway in it may enter the election or spawn a broker. It only attaches to
/// the descriptor its own broker publishes, and reports the broker unavailable
/// while there is none (starting, or exiting). Without this the in-process MCP
/// host would find no descriptor during that window and start a second
/// `qc-device-broker.exe` - a second process, and a second USB owner.
static BROKER_SPAWN_FORBIDDEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn forbid_broker_spawn() {
    BROKER_SPAWN_FORBIDDEN.store(true, std::sync::atomic::Ordering::Release);
}

fn broker_spawn_forbidden() -> bool {
    BROKER_SPAWN_FORBIDDEN.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// How this host reaches its `qc-device-broker`.
///
/// Every `GatewayTransport` is torn down by its own `Drop`, so a transport
/// abandoned half way through `GatewayProcess::start` (a failed API version
/// check, a reader thread that could not start) is cleaned up exactly like a
/// finished one.
enum GatewayTransport {
    /// A private broker spoken to over its stdio pipes. Used only when the
    /// shared descriptor path was never resolved, which is the pre-shared
    /// behaviour kept intact for that case.
    OwnedChild { child: Child, stdin: ChildStdin },
    /// A connection to the one broker every QC Remote process on this host
    /// shares.
    SharedBroker {
        stream: TcpStream,
        /// Identity is captured from the authenticated descriptor, never
        /// re-read from a possibly newer broker during failure recovery.
        instance_id: String,
        release_marker_path: PathBuf,
        /// `Some` only in the process that won the election and spawned the
        /// broker itself; `None` in every process that merely attached to a
        /// broker someone else is hosting.
        ///
        /// Even when `Some`, the broker is never killed through this handle: by
        /// the time a transport holds it the broker has published itself, and
        /// any other QC Remote window may already be attached. Its lifetime is
        /// its own zero-client grace period, nothing else. The handle is kept
        /// because an open process handle stops Windows from reusing the pid,
        /// so the `broker_pid` this host reports keeps naming that broker. It is
        /// held for that effect alone; outside tests nothing reads it.
        #[cfg_attr(not(test), allow(dead_code))]
        hosted_child: Option<Child>,
    },
}

impl Drop for GatewayTransport {
    fn drop(&mut self) {
        match self {
            Self::OwnedChild { child, .. } => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Self::SharedBroker { stream, .. } => {
                // `shutdown` rather than relying on the drop of `stream`: the
                // reader thread holds a `try_clone` of this socket, and the
                // connection only closes once every handle is gone. Without
                // this, a dropped client would stay counted by the broker for
                // as long as the reader thread lived - which is forever, since
                // it is blocked reading that very socket - and the broker
                // could never reach its zero-client exit.
                let _ = stream.shutdown(Shutdown::Both);
                // Deliberately no kill, not even of a broker this process
                // spawned. Every other QC Remote window on the machine may be
                // talking to that same process, and this transport is dropped
                // not only when a window closes but on every transport failure
                // and every `Gateway::restart` (a session reset) - none of
                // which says the broker is dead. Killing it here would drop
                // every other window's device session to recover this one
                // connection. The broker exits on its own once its last client
                // has been gone for its grace period.
            }
        }
    }
}

struct GatewayProcess {
    transport: GatewayTransport,
    responses: mpsc::Receiver<GatewayReaderMessage>,
    reader_completed: mpsc::Receiver<()>,
    next_id: u64,
    /// The broker's process id, for the runtime health record. For an attached
    /// client it is the id the broker published, not a process this one owns.
    broker_pid: u32,
    intentional_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    participant_announced: bool,
    /// Envelope metadata also covers array results, which cannot carry fields.
    last_broker_target: Option<qc_relay_client::BrokerTarget>,
    /// The second way onto this connection, for the reads the broker answers
    /// beside a running request. `None` for a private stdio child, whose
    /// broker loop answers strictly in order.
    lock_free_reads: Option<std::sync::Arc<LockFreeReads>>,
}

/// Ids at or above this mark a lock-free read sent beside a running request.
/// The reader hands their replies to `LockFreeReads`, never to the request
/// holding the gateway, so the two can never answer each other.
/// `GatewayProcess::next_id` counts up from 1 and does not come near it.
const LOCK_FREE_READ_ID_BASE: u64 = 1 << 48;
/// How long a lock-free read waits before the caller falls back to queueing
/// for the gateway as before. A current broker answers in milliseconds; only
/// a broker that still serves a connection strictly in order runs this out.
const LOCK_FREE_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A test's stand-in for `GatewayProcess::start`, given `auto_connect`.
#[cfg(test)]
type GatewayStarter = std::sync::Arc<dyn Fn(bool) -> Result<GatewayProcess, String> + Send + Sync>;

/// Test starters, each keyed by the descriptor path of the fixture it applies
/// to, so tests running in parallel never see each other's.
#[cfg(test)]
static GATEWAY_START_SEAMS: Mutex<Vec<(PathBuf, GatewayStarter)>> = Mutex::new(Vec::new());

#[cfg(test)]
fn gateway_start_seam(descriptor_path: Option<&Path>) -> Option<GatewayStarter> {
    let descriptor_path = descriptor_path?;
    GATEWAY_START_SEAMS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(path, _)| path == descriptor_path)
        .map(|(_, starter)| starter.clone())
}

/// Replaces `GatewayProcess::start` for one descriptor path for the life of
/// the guard, and counts every start that reaches it. The starter it installs
/// launches nothing: it refuses, so a counted start leaves no process behind.
#[cfg(test)]
pub(crate) struct GatewayStartSeam {
    descriptor_path: PathBuf,
    starts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl GatewayStartSeam {
    /// The refusal a counted start answers with.
    pub(crate) const REFUSAL: &'static str = "test gateway starter: no process is started";

    pub(crate) fn install(descriptor_path: &Path) -> Self {
        let starts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = starts.clone();
        GATEWAY_START_SEAMS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                descriptor_path.to_path_buf(),
                std::sync::Arc::new(move |_auto_connect| {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(Self::REFUSAL.into())
                }),
            ));
        Self {
            descriptor_path: descriptor_path.to_path_buf(),
            starts,
        }
    }

    pub(crate) fn starts(&self) -> usize {
        self.starts.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Drop for GatewayStartSeam {
    fn drop(&mut self) {
        GATEWAY_START_SEAMS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(path, _)| *path != self.descriptor_path);
    }
}

/// Test-only: what a test receives about every broker `host_device_broker`
/// spawns for one descriptor path, at the instant of the spawn - before the
/// attach, before `GatewayProcess::start` returns, before anything that could
/// panic or fail.
#[cfg(all(test, windows))]
pub(crate) enum SpawnedBroker {
    /// A duplicate of the spawned `Child`'s own handle (`try_clone_to_owned`):
    /// it names exactly that process object, never a pid.
    Owned(std::os::windows::io::OwnedHandle),
    /// The duplicate could not be made, so the broker was killed in place,
    /// through the original `Child`, and its exit was observed within
    /// `SPAWN_REAP_BOUND`. Holds why, and the exit status.
    ReapedInPlace(String),
    /// The in-place reap was not acknowledged within its bound (the
    /// kill failed, or the exit was not observed), so ownership is kept: a
    /// handle duplicated after the reap attempt, held here for later cleanup
    /// and the test end. Holds why. Removal stays blocked while it lives.
    Retained {
        handle: std::os::windows::io::OwnedHandle,
        why: String,
    },
    /// The duplicate could not be made, the in-place reap was not
    /// acknowledged, and no handle could be retained either: not owned by
    /// the test (the gateway's own `Child` still holds it), never terminated
    /// by anything else.
    Unowned(String),
    /// The original `Child` itself, handed to the fixture by the
    /// Published-failure arm, where production lets it drop (shared
    /// lifetime). The fixture owns its scratch broker, so it may end it.
    OriginalChild(Child),
}

/// Test-only: the Published-failure arm gives the fixture that
/// installed a spawn sink for this path the original `Child`; without a sink
/// it drops exactly as in production.
#[cfg(all(test, windows))]
fn fixture_takes_published_child(descriptor_path: &Path, child: Child) {
    let sink = BROKER_SPAWN_SINKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(path, _, _)| path == descriptor_path)
        .map(|(_, sink, _)| sink.clone());
    if let Some(sink) = sink {
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(SpawnedBroker::OriginalChild(child));
    }
}

#[cfg(all(test, windows))]
pub(crate) type BrokerSpawnSink = std::sync::Arc<Mutex<Vec<SpawnedBroker>>>;

/// How long the in-place reap at the spawn point waits for the exit.
#[cfg(all(test, windows))]
const SPAWN_REAP_BOUND: Duration = Duration::from_secs(5);

/// What a test injects at the spawn point.
#[cfg(all(test, windows))]
#[derive(Clone, Copy, Default)]
pub(crate) struct SpawnInjection {
    /// Panic right after the handle is stored - only ever once the broker
    /// is owned (the injection point inside `GatewayProcess::start`).
    pub(crate) panic_after_owned: bool,
    /// Make the first handle duplication fail, to prove the broker is still
    /// reaped (or retained).
    pub(crate) fail_clone: bool,
    /// Make the in-place kill fail and not take effect, to prove the
    /// reap is bounded and ownership is kept. It also applies to the
    /// kill in `host_device_broker`'s Unpublished arm.
    pub(crate) fail_kill: bool,
    /// Make the second duplicate (the one that would retain the
    /// broker after an unacknowledged in-place reap) fail too.
    pub(crate) fail_retain_clone: bool,
    /// Replace the real attach outcome, to drive each arm of
    /// `host_device_broker` end to end.
    pub(crate) attach_outcome: AttachOverride,
    /// Every `try_wait` in the Unpublished cleanup fails.
    pub(crate) fail_wait: bool,
    /// Panic right after the child is in its start slot.
    pub(crate) panic_after_spawn: bool,
    /// Record nothing in the spawn sink, so the test holds no
    /// auxiliary duplicate and only the production start slot owns the child.
    pub(crate) no_sink_duplicate: bool,
}

/// Which attach outcome a test forces in `host_device_broker`.
#[cfg(all(test, windows))]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AttachOverride {
    /// The real outcome.
    #[default]
    Real,
    /// `SpawnedBrokerFailure::Unpublished`: the arm that kills the broker.
    Unpublished,
    /// `SpawnedBrokerFailure::Published`: the arm that returns `Occupied`
    /// and drops its local `Child` without killing it.
    PublishedFailure,
}

/// The injection installed for `descriptor_path`, if any.
#[cfg(all(test, windows))]
fn spawn_injection(descriptor_path: &Path) -> Option<SpawnInjection> {
    BROKER_SPAWN_SINKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(path, _, _)| path == descriptor_path)
        .map(|(_, _, injection)| *injection)
}

/// The attach outcome, replaced when a test asked for another arm. A real
/// successful attach that is replaced normally drops its stream: the broker
/// then has no client and ends through its own grace period, or through the
/// test's handle. A live-retention fixture can explicitly own that client
/// through `BrokerAttachHoldGuard` without duplicating process ownership.
#[cfg(all(test, windows))]
fn injected_attach_outcome(
    descriptor_path: &Path,
    real: Result<(TcpStream, String), SpawnedBrokerFailure>,
) -> Result<(TcpStream, String), SpawnedBrokerFailure> {
    match spawn_injection(descriptor_path).map(|injection| injection.attach_outcome) {
        Some(AttachOverride::Unpublished) => {
            // A live-retention fixture may retain this actual client without
            // owning any duplicate Child or process handle. Otherwise retain
            // the normal behavior: dropping the connection starts idle grace.
            if let Ok((stream, _)) = real {
                let held = BROKER_ATTACH_HOLDS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .find(|(path, _)| path == descriptor_path)
                    .map(|(_, held)| held.clone());
                if let Some(held) = held {
                    *held
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stream);
                }
            }
            Err(SpawnedBrokerFailure::Unpublished(
                "injected: the broker did not publish".into(),
            ))
        }
        Some(AttachOverride::PublishedFailure) => Err(SpawnedBrokerFailure::Published(
            BrokerAttachFailure::Unresponsive(
                "injected: the published broker did not answer".into(),
            ),
        )),
        _ => real,
    }
}

/// Sinks keyed by descriptor path, with their injection.
#[cfg(all(test, windows))]
static BROKER_SPAWN_SINKS: Mutex<Vec<(PathBuf, BrokerSpawnSink, SpawnInjection)>> =
    Mutex::new(Vec::new());

#[cfg(all(test, windows))]
type FixtureAttachHold = std::sync::Arc<Mutex<Option<TcpStream>>>;

#[cfg(all(test, windows))]
static BROKER_ATTACH_HOLDS: Mutex<Vec<(PathBuf, FixtureAttachHold)>> = Mutex::new(Vec::new());

/// Retain a real client through fixture unwind and final same-Child cleanup.
/// This does not duplicate ownership of the production slot's process.
#[cfg(all(test, windows))]
pub(crate) struct BrokerAttachHoldGuard {
    path: PathBuf,
    held: FixtureAttachHold,
}

#[cfg(all(test, windows))]
impl BrokerAttachHoldGuard {
    pub(crate) fn install(path: &Path) -> Self {
        let held = FixtureAttachHold::default();
        BROKER_ATTACH_HOLDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((path.to_owned(), held.clone()));
        Self {
            path: path.to_owned(),
            held,
        }
    }

    pub(crate) fn attached(&self) -> bool {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

#[cfg(all(test, windows))]
impl Drop for BrokerAttachHoldGuard {
    fn drop(&mut self) {
        BROKER_ATTACH_HOLDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(path, _)| path != &self.path);
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// Called by `host_device_broker` on the very next line after the spawn.
///
/// When the handle cannot be duplicated, the original `Child` - still held
/// here, mutably - is killed in place and its exit is polled with `try_wait`
/// for at most `SPAWN_REAP_BOUND` (never an unbounded `wait`).
/// Only `Ok(Some(status))` is a reap. Otherwise ownership is kept: a handle
/// is duplicated again and retained; only if that also fails is the entry
/// `Unowned`, and the gateway's `Child` is left exactly as it was, never
/// dropped here. The injected panic runs only when the broker is owned.
#[cfg(all(test, windows))]
fn register_spawned_broker(descriptor_path: &Path, child: &mut Child) {
    use std::os::windows::io::AsHandle as _;
    let entry = BROKER_SPAWN_SINKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(path, _, _)| path == descriptor_path)
        .map(|(_, sink, injection)| (sink.clone(), *injection));
    let Some((sink, injection)) = entry else {
        return;
    };
    if injection.no_sink_duplicate {
        return;
    }
    let duplicated = if injection.fail_clone {
        Err(std::io::Error::other("injected handle duplication failure"))
    } else {
        child.as_handle().try_clone_to_owned()
    };
    let spawned = match duplicated {
        Ok(handle) => SpawnedBroker::Owned(handle),
        Err(error) => {
            let killed = if injection.fail_kill {
                Err(std::io::Error::other("injected kill failure"))
            } else {
                child.kill()
            };
            let deadline = Instant::now() + SPAWN_REAP_BOUND;
            let observed = loop {
                match child.try_wait() {
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    other => break other,
                }
            };
            let killed = killed.map_or_else(|error| error.to_string(), |()| "ok".into());
            match observed {
                Ok(Some(status)) => SpawnedBroker::ReapedInPlace(format!(
                    "duplicating its handle failed ({error}); killed in place (kill: {killed}) \
                     and its exit observed ({status})"
                )),
                unacknowledged => {
                    let status = match unacknowledged {
                        Ok(_) => format!("still running after {} ms", SPAWN_REAP_BOUND.as_millis()),
                        Err(wait) => format!("exit status unknown: {wait}"),
                    };
                    let why = format!(
                        "duplicating its handle failed ({error}); in-place reap not \
                         acknowledged (kill: {killed}; {status})"
                    );
                    let retained = if injection.fail_retain_clone {
                        Err(std::io::Error::other("injected second duplication failure"))
                    } else {
                        child.as_handle().try_clone_to_owned()
                    };
                    match retained {
                        Ok(handle) => SpawnedBroker::Retained { handle, why },
                        Err(again) => SpawnedBroker::Unowned(format!(
                            "{why}; no handle could be retained ({again})"
                        )),
                    }
                }
            }
        }
    };
    let owned = matches!(spawned, SpawnedBroker::Owned(_));
    sink.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(spawned);
    if owned && injection.panic_after_owned {
        panic!("injected panic inside GatewayProcess::start after the broker spawned");
    }
}

/// Installs a spawn sink for one descriptor path for the life of the guard.
#[cfg(all(test, windows))]
pub(crate) struct BrokerSpawnSinkGuard(PathBuf);

#[cfg(all(test, windows))]
impl BrokerSpawnSinkGuard {
    pub(crate) fn install(
        descriptor_path: &Path,
        injection: SpawnInjection,
    ) -> (Self, BrokerSpawnSink) {
        let sink = BrokerSpawnSink::default();
        BROKER_SPAWN_SINKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((descriptor_path.to_path_buf(), sink.clone(), injection));
        (Self(descriptor_path.to_path_buf()), sink)
    }
}

#[cfg(all(test, windows))]
impl Drop for BrokerSpawnSinkGuard {
    fn drop(&mut self) {
        BROKER_SPAWN_SINKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(path, _, _)| *path != self.0);
    }
}

/// A test's stand-in for `GatewayProcess::start` on one `Gateway`, given
/// `auto_connect` and the descriptor path `start` would have been given
/// (`None`: the private `--stdio` child).
#[cfg(test)]
type GatewayInstanceStarter =
    std::sync::Arc<dyn Fn(bool, Option<&Path>) -> Result<GatewayProcess, String> + Send + Sync>;

#[cfg(test)]
impl Gateway {
    /// Replaces `GatewayProcess::start` for this gateway alone with a starter
    /// that launches nothing and refuses with [`GatewayStartSeam::REFUSAL`].
    /// Returns every descriptor path a start was asked for, in order. The
    /// starter lives in this value, not in any registry, so no other test's
    /// gateway - with a `None` descriptor or any other - can reach it.
    pub(crate) fn count_starts_refusing(&mut self) -> std::sync::Arc<Mutex<Vec<Option<PathBuf>>>> {
        let starts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = starts.clone();
        self.start_seam = Some(std::sync::Arc::new(
            move |_auto_connect, descriptor_path| {
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(descriptor_path.map(Path::to_path_buf));
                Err(GatewayStartSeam::REFUSAL.into())
            },
        ));
        starts
    }

    /// `count_starts_refusing`, recording each start's `auto_connect`.
    pub(crate) fn record_start_auto_connect_refusing(
        &mut self,
    ) -> std::sync::Arc<Mutex<Vec<bool>>> {
        let starts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = starts.clone();
        self.start_seam = Some(std::sync::Arc::new(
            move |auto_connect, _descriptor_path| {
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(auto_connect);
                Err(GatewayStartSeam::REFUSAL.into())
            },
        ));
        starts
    }
}

/// The read that may travel on a broker connection while another request on
/// it is still waiting for its answer: `system.status`.
///
/// `Gateway` is held for the whole of a request, so a 28-second
/// `device.createBackup` used to hold every `system.status` behind it - and
/// with it the one field, `deviceSuspended`, that says why the device is not
/// answering. The broker answers
/// `quadcortex::device::answers_beside_running_request` methods - only
/// `system.status` - straight from its connection reader, without its request
/// lock, without an idle-release reopen and without waiting behind the
/// connection's queue, so this path sends exactly those, under its own id
/// range, and waits only for its own reply. The other cached reads can start
/// a USB reopen and stay in the gateway's line.
///
/// It never sends anything else, and it shares the gateway's writer lock so
/// two frames cannot interleave on the wire. Nothing that writes to the
/// device, and nothing that orders writes, goes through here.
struct LockFreeReads {
    writer: Mutex<TcpStream>,
    pending: Mutex<std::collections::HashMap<u64, mpsc::SyncSender<Value>>>,
    next_id: std::sync::atomic::AtomicU64,
    intentional_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct GatewayWriteFailure {
    message: String,
    attempted: bool,
}

impl GatewayWriteFailure {
    fn request_failure(self, method: &str) -> GatewayRequestFailure {
        let failure = GatewayRequestFailure::Transport(self.message);
        if self.attempted {
            failure.after_dispatch(method)
        } else {
            failure
        }
    }
}

impl LockFreeReads {
    fn new(
        writer: TcpStream,
        intentional_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            writer: Mutex::new(writer),
            pending: Mutex::new(std::collections::HashMap::new()),
            next_id: std::sync::atomic::AtomicU64::new(0),
            intentional_release,
        }
    }

    /// Every frame this connection sends goes through here, whichever path
    /// it came from.
    fn write(&self, payload: &[u8]) -> Result<(), GatewayWriteFailure> {
        self.write_before(payload, std::time::Instant::now() + GATEWAY_WRITE_TIMEOUT)
    }

    fn write_before(
        &self,
        payload: &[u8],
        deadline: std::time::Instant,
    ) -> Result<(), GatewayWriteFailure> {
        // Acquiring the shared frame writer is part of the same output budget.
        // A refusal here proves no bytes from this request were dispatched.
        let mut writer = loop {
            if std::time::Instant::now() >= deadline {
                return Err(GatewayWriteFailure {
                    message: "Gateway frame writer admission deadline elapsed".into(),
                    attempted: false,
                });
            }
            match self.writer.try_lock() {
                Ok(writer) => break writer,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(GatewayWriteFailure {
                        message: "Gateway writer lock was poisoned".into(),
                        attempted: false,
                    })
                }
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(
                    deadline
                        .saturating_duration_since(std::time::Instant::now())
                        .min(std::time::Duration::from_millis(2)),
                ),
            }
        };
        qc_broker_framing::write_tcp_frame_before_deadline(
            &mut writer,
            payload,
            quadcortex::domain::IPC_MAX_FRAME_BYTES,
            deadline,
        )
        .map_err(|error| {
            // A partial frame cannot safely be followed by another request.
            // Retire this socket, never replay an admitted mutation.
            let _ = writer.shutdown(Shutdown::Both);
            GatewayWriteFailure {
                message: error.to_string(),
                attempted: true,
            }
        })
    }

    fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<u64, mpsc::SyncSender<Value>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A reply in this path's id range. One nobody waits for any more (the
    /// read gave up and the caller queued for the gateway instead) is
    /// dropped: it is a read, and it must never reach the request in flight.
    fn deliver(&self, id: u64, response: Value) {
        if let Some(waiter) = self.pending().remove(&id) {
            let _ = waiter.try_send(response);
        }
    }

    /// The connection is gone: wake every waiter now rather than at its
    /// timeout.
    fn close(&self) {
        self.pending().clear();
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, GatewayRequestFailure> {
        if !quadcortex::device::answers_beside_running_request(method) {
            return Err(GatewayRequestFailure::Transport(format!(
                "{method} is not answered beside a running request"
            )));
        }
        if self
            .intentional_release
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(GatewayRequestFailure::Transport(
                "The broker connection is being released".into(),
            ));
        }
        let id = LOCK_FREE_READ_ID_BASE
            + self
                .next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % LOCK_FREE_READ_ID_BASE;
        let payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))
        .map_err(|error| GatewayRequestFailure::Transport(error.to_string()))?;
        if payload.len() > quadcortex::domain::IPC_MAX_FRAME_BYTES {
            return Err(GatewayRequestFailure::Transport(
                "Gateway request exceeds the IPC limit".into(),
            ));
        }
        let (waiter, reply) = mpsc::sync_channel(1);
        let deadline = std::time::Instant::now() + LOCK_FREE_READ_TIMEOUT;
        self.pending().insert(id, waiter);
        if let Err(error) = self.write_before(&payload, deadline) {
            self.pending().remove(&id);
            return Err(error.request_failure(method));
        }
        let response = match reply
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            Ok(response) => response,
            Err(_) => {
                self.pending().remove(&id);
                return Err(GatewayRequestFailure::Transport(format!(
                    "The broker did not answer {method} beside the running request"
                )));
            }
        };
        let result = decode_gateway_response(response, id)?;
        validate_gateway_result(method, &result)?;
        Ok(result)
    }
}

/// Where the gateway publishes its current connection's `LockFreeReads`, for
/// callers that find the gateway busy. Cloned out of the `Gateway` before it
/// is put behind its mutex, so reading it never waits for that mutex.
#[derive(Clone, Default)]
pub(crate) struct LockFreeReadSlot(std::sync::Arc<Mutex<Option<std::sync::Arc<LockFreeReads>>>>);

impl LockFreeReadSlot {
    fn publish(&self, reads: Option<std::sync::Arc<LockFreeReads>>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = reads;
    }

    fn current(&self) -> Option<std::sync::Arc<LockFreeReads>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// What a request does when the gateway is busy and it was not answered
/// beside the running request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WhenGatewayBusy {
    /// Queue for the gateway, as every request did before.
    Wait,
    /// Report the transport busy, as the window's cached reads always have.
    Refuse,
}

/// Whether a gateway request may bring a broker connection up.
///
/// Decided by the request's origin and acted on only inside the gateway lock,
/// by [`Gateway::request_detailed_as`]: the same critical section that
/// attaches to, elects, spawns or recovers a broker. A look taken before the
/// lock (`gateway_attachment`) can be overtaken by a request that holds it and
/// loses its connection, so it never decides this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionUse {
    /// Attach, elect, spawn or recover as needed: a tool call, a window's own
    /// request, presence upkeep. Every request before this type existed.
    MayConnect,
    /// Use the broker connection the gateway already holds, and send device
    /// demand only while the device session on it is open. Never attach,
    /// elect, spawn or recover a broker, and never start the broker's idle
    /// reopen: an MCP resource read, which is the client reading context and
    /// not the chat asking for the device.
    ExistingOnly,
}

impl ConnectionUse {
    /// The current task's, by origin: `ExistingOnly` while it serves an MCP
    /// resource read ([`qc_remote_mcp::serving_resource_read`]). Read on the
    /// async side, before a gateway call moves to a blocking thread, which
    /// carries no task-local.
    pub(crate) fn of_current_task() -> Self {
        if qc_remote_mcp::serving_resource_read() {
            Self::ExistingOnly
        } else {
            Self::MayConnect
        }
    }
}

/// What an `ExistingOnly` request is told when it could only be served by
/// bringing a broker connection or a device session up: the structured
/// offline answer a bridge with nobody to relay to gives. A transport failure,
/// so every adapter reports it as `device_unavailable`, retryable.
fn existing_connection_unavailable() -> GatewayRequestFailure {
    GatewayRequestFailure::Transport(local_stdio::OFFLINE_ADVICE.into())
}

/// Whether a `system.status` reply reports the device session open. Only
/// then may an `ExistingOnly` request send device demand: the broker answers
/// demand on a released session by reopening USB (`prepare_idle_released_target`
/// in `services/device-broker/src/rpc.rs`), and never answers `system.status`
/// that way.
fn device_session_open(status: &Value) -> bool {
    if let Some(ready) = status.get("targetReady").and_then(Value::as_bool) {
        return ready;
    }
    status
        .get("usbDiagnostics")
        .and_then(|diagnostics| diagnostics.get("connected"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// The envelope `origin` that tells the broker a request is an MCP resource
/// read, and the `system.status` capability of a broker that honours it.
/// Mirror `RESOURCE_READ_ORIGIN` and `RESOURCE_READ_ORIGIN_CAPABILITY` in
/// `services/device-broker/src/rpc/wire.rs`.
const BROKER_RESOURCE_READ_ORIGIN: &str = "resourceRead";
const BROKER_RESOURCE_READ_ORIGIN_CAPABILITY: &str = "resourceReadOrigin";
// Internal serialization marker; the wire carries a separate boolean so it
// composes with resourceRead. It is sent only after capability negotiation.
const PHYSICAL_TARGET_ORIGIN: &str = "physicalTargetOrigin";
const PHYSICAL_RESOURCE_READ_ORIGIN: &str = "physicalResourceRead";
/// A device-backed resource read refused because the attached broker does not
/// list [`BROKER_RESOURCE_READ_ORIGIN_CAPABILITY`]. It has the offline shape
/// (a retryable transport failure: restarting QC Remote replaces the broker),
/// and says why.
const BROKER_TOO_OLD_FOR_RESOURCE_READS: &str = "The Quad Cortex was not read: the QC Remote \
device broker running now is too old to guarantee that a resource read never reopens the \
device. Restart QC Remote to update it, or use the matching tool.";

/// Whether the broker that sent this `system.status` reply enforces a
/// resource read's narrowing itself, under its own request lock. Only such a
/// broker closes the gap between the status read and the device read: an
/// idle release, or another client's disconnect, can otherwise land between
/// the two, and the broker would then reopen USB for the read.
fn broker_narrows_resource_reads(status: &Value) -> bool {
    status
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability == BROKER_RESOURCE_READ_ORIGIN_CAPABILITY)
        })
}

/// The one way a host's callers reach its gateway.
///
/// Every request takes the gateway mutex and runs `locked`, exactly as
/// before: writes are serialized by that mutex and then by the broker, in the
/// same order, and are refused by the broker on the same terms. Two cases
/// differ only while the gateway is busy (a backup is running):
/// - `system.status` asks the broker directly over `LockFreeReads` and
///   returns the broker's own answer, falling back to waiting only when that
///   answer does not come;
/// - a caller passing `WhenGatewayBusy::Refuse` is told the transport is
///   busy instead of waiting, as the window's cached reads always were.
pub(crate) fn request_gateway(
    gateway: &Mutex<Gateway>,
    reads: &LockFreeReadSlot,
    method: &str,
    params: Value,
    when_busy: WhenGatewayBusy,
    locked: impl FnOnce(&mut Gateway, Value) -> Result<Value, GatewayRequestFailure>,
) -> Result<Value, GatewayRequestFailure> {
    let poisoned = || GatewayRequestFailure::Transport("Gateway session lock was poisoned".into());
    let beside = quadcortex::device::answers_beside_running_request(method);
    if beside || when_busy == WhenGatewayBusy::Refuse {
        match gateway.try_lock() {
            Ok(mut gateway) => return locked(&mut gateway, params),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(poisoned()),
            Err(std::sync::TryLockError::WouldBlock) => {
                if let Some(reads) = reads.current().filter(|_| beside) {
                    match reads.request(method, params.clone()) {
                        Ok(result) => return Ok(result),
                        Err(failure @ GatewayRequestFailure::Remote { .. }) => return Err(failure),
                        Err(GatewayRequestFailure::Transport(_)) => {}
                        Err(failure @ GatewayRequestFailure::UnansweredMutation(_)) => {
                            return Err(failure)
                        }
                    }
                }
                if when_busy == WhenGatewayBusy::Refuse {
                    return Err(GatewayRequestFailure::Transport(
                        "Device transport is busy".into(),
                    ));
                }
            }
        }
    }
    let mut gateway = gateway.lock().map_err(|_| poisoned())?;
    locked(&mut gateway, params)
}

/// One relay call on a host's gateway, in the connection mode its adapter
/// chose. Both `WindowsRelayAdapter::invoke_as` and
/// `local_stdio::HeadlessDeviceAdapter::invoke_gateway` run exactly this, so a
/// test of it covers the gateway half of either. `stopped` is a headless
/// host's own stop flag: set, the call is refused before it starts and the
/// connection is retired after it. A window has none and passes `None`.
#[cfg(test)]
pub(crate) fn relay_gateway_request(
    gateway: &Mutex<Gateway>,
    lock_free_reads: &LockFreeReadSlot,
    method: &str,
    params: Value,
    connection: ConnectionUse,
    stopped: Option<&std::sync::atomic::AtomicBool>,
    admission: Option<&owned_blocking::Context>,
) -> Result<Value, GatewayRequestFailure> {
    relay_gateway_request_targeted(
        gateway,
        lock_free_reads,
        method,
        params,
        connection,
        RelayGatewayContext { stopped, admission },
        None,
    )
}

pub(crate) struct RelayGatewayContext<'a> {
    pub(crate) stopped: Option<&'a std::sync::atomic::AtomicBool>,
    pub(crate) admission: Option<&'a owned_blocking::Context>,
}

pub(crate) fn relay_gateway_request_targeted(
    gateway: &Mutex<Gateway>,
    lock_free_reads: &LockFreeReadSlot,
    method: &str,
    params: Value,
    connection: ConnectionUse,
    context: RelayGatewayContext<'_>,
    expected: Option<Value>,
) -> Result<Value, GatewayRequestFailure> {
    let is_stopped = || {
        context
            .stopped
            .is_some_and(|stopped| stopped.load(std::sync::atomic::Ordering::Acquire))
    };
    let stopping = || GatewayRequestFailure::Transport("The MCP host is stopping".into());
    if is_stopped() {
        return Err(stopping());
    }
    // A status read reaches the broker even while a backup holds the
    // gateway; everything else queues for it.
    // The lock-free envelope has no target guard. A guarded call must pass
    // through the locked serializer even when it is a status/resource read.
    let no_unfenced_reads = LockFreeReadSlot::default();
    let reads = if expected.is_some() {
        &no_unfenced_reads
    } else {
        lock_free_reads
    };
    request_gateway(
        gateway,
        reads,
        method,
        params,
        WhenGatewayBusy::Wait,
        |gateway, params| {
            if is_stopped() {
                return Err(stopping());
            }
            // Cancellation and dispatch compete under the gateway's own lock.
            // Queued requests must not dispatch after their caller has left.
            if context
                .admission
                .is_some_and(|admission| !admission.begin_dispatch())
            {
                return Err(GatewayRequestFailure::Transport(
                    "The request ended before gateway dispatch".into(),
                ));
            }
            let result = gateway.request_targeted_as(method, params, connection, expected);
            if is_stopped() {
                gateway.retire_current_process();
            }
            result
        },
    )
}

/// Retained native gateway work has a software admission ceiling, not a
/// firmware/device limit. The relay's existing default invocation deadline is
/// five minutes; a narrower outer caller deadline cancels this await sooner.
static RELAY_GATEWAY_NATIVE_BUDGET: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(8);
const RELAY_GATEWAY_NATIVE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub(crate) async fn await_relay_gateway(
    method: String,
    budget: &'static qc_native_platform::thread_owner::Budget,
    within: Duration,
    action: impl FnOnce(owned_blocking::Context) -> Result<Value, GatewayRequestFailure>
        + Send
        + 'static,
) -> Result<Value, DeviceError> {
    match owned_blocking::run("qc-relay-gateway", budget, Instant::now() + within, action).await {
        Ok(result) => result.map_err(device_error_from_gateway_failure),
        Err(failure) => {
            let message = format!("{} ({:?})", failure.message, failure.kind);
            let failure = if failure.admitted && generated_gateway::is_mutation_method(&method) {
                GatewayRequestFailure::UnansweredMutation(format!(
                    "{message}. The admitted mutation may have reached the device; it was not replayed. Inspect current state before another mutation."
                ))
            } else {
                GatewayRequestFailure::Transport(message)
            };
            Err(device_error_from_gateway_failure(failure))
        }
    }
}

/// Where the shared broker's descriptor lives, resolved from the Tauri
/// `app_local_data_dir` during setup. A window's `Gateway` reads it; a headless
/// `--local-mcp-bridge` process never runs that setup and gives its `Gateway`
/// the same path directly (`Gateway::descriptor_path`). With neither, no
/// descriptor path could be resolved, and `GatewayProcess::start` keeps its
/// private-child behaviour.
static DEVICE_BROKER_DESCRIPTOR_PATH: OnceLock<PathBuf> = OnceLock::new();

/// How long a freshly spawned broker may take to bind and publish its
/// descriptor. The broker writes the descriptor itself once bound, so the host
/// that spawned it waits for that write rather than writing a second copy that
/// could disagree about the real port.
///
/// It covers process start, the bind, the descriptor claim and one file
/// write - nothing that waits on hardware. MIDI warm-up runs in the broker's
/// background after publication, and the USB handshake of an `--auto-connect`
/// broker runs on its own worker thread, so a slow MIDI service can no longer
/// hold publication past this and get a broker killed mid-handshake.
const DEVICE_BROKER_PUBLISH_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a process that lost the election waits for the winner to publish
/// before giving up on the shared broker altogether.
const DEVICE_BROKER_ELECTION_TIMEOUT: Duration = Duration::from_secs(10);
/// An election lock older than this belongs to a process that died while
/// holding it: the holder never keeps it longer than the publish wait plus two
/// handshakes.
const DEVICE_BROKER_ELECTION_LOCK_STALE_AFTER: Duration = Duration::from_secs(15);
/// Poll interval for the descriptor and election-lock files.
const DEVICE_BROKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// How long to wait before re-checking a `Gone` verdict against a published
/// descriptor. Long enough to let a transient refusal under genuine machine
/// load clear; short enough not to meaningfully slow down the common case
/// (no descriptor published at all, which never reaches this check).
const DEVICE_BROKER_GONE_CONFIRMATION_DELAY: Duration = Duration::from_millis(200);
/// Answered by the broker itself, outside the gateway contract: how many
/// clients it is serving. Mirrors `BROKER_CLIENT_COUNT_METHOD` in
/// `services/device-broker/src/rpc.rs`.
const DEVICE_BROKER_CLIENT_COUNT_METHOD: &str = "broker.clientCount";
const DEVICE_BROKER_DISCONNECT_METHOD: &str = "broker.disconnectDevice";
/// What `device.disconnect` answers when it leaves a shared session open for
/// the other clients still using it.
const SHARED_SESSION_LEFT_OPEN_DETAIL: &str = "Disconnected this window. The Quad Cortex session \
     stays open while another QC Remote window or MCP client is still using it, and only closes \
     once all of them - not just this window - have disconnected.";

enum GatewayReaderMessage {
    Response(Value),
    Failed(String),
}

enum GatewayRequestFailure {
    Transport(String),
    /// Mutation dispatch was attempted, but no trustworthy reply established
    /// its outcome. This transport loss must not advertise a safe replay.
    UnansweredMutation(String),
    Remote {
        code: i64,
        message: String,
        retryable: bool,
        details: Option<Value>,
    },
}

impl GatewayRequestFailure {
    fn after_dispatch(self, method: &str) -> Self {
        match self {
            Self::Transport(message)
                if generated_gateway::is_mutation_method(method)
                    || method == "broker.createDeviceBackupBound" =>
            {
                Self::UnansweredMutation(message)
            }
            other => other,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GatewayUiError {
    code: String,
    message: String,
    retryable: bool,
}

impl From<GatewayRequestFailure> for GatewayUiError {
    fn from(failure: GatewayRequestFailure) -> Self {
        match failure {
            GatewayRequestFailure::Remote {
                code,
                message,
                retryable,
                ..
            } => Self {
                code: code.to_string(),
                message,
                retryable,
            },
            GatewayRequestFailure::Transport(message) => Self {
                code: "DEVICE_TRANSPORT".into(),
                message,
                retryable: true,
            },
            GatewayRequestFailure::UnansweredMutation(message) => Self {
                code: "WRITE_OUTCOME_AMBIGUOUS".into(),
                message,
                retryable: false,
            },
        }
    }
}

// Library mutations can contain several verified stages and backup is the
// longest single RPC (three minutes). Reply waiting has a software deadline;
// synchronous OS operations do not imply a hard kernel cancellation guarantee.
const GATEWAY_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);
// Local IPC output headroom, not a device/firmware limit. Both frame-writer
// admission and all partial TCP writes share one absolute deadline.
const GATEWAY_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Reported when a broker answer does not satisfy its generated result contract.
/// Distinct from a transport fault so the session is not torn down for it.
const GATEWAY_CONTRACT_ERROR_CODE: i64 = -32020;
/// Compatibility with older brokers that reported a failed reservation retry
/// as -32011. Current brokers reserve once inside the caller's scope and do not
/// emit this legacy error. Keep this value aligned with the broker RPC mapping.
const RESERVE_REQUEST_ID_TIMEOUT_ERROR_CODE: i64 = -32011;
/// Cap on the retained broker stderr log so a restart loop cannot fill the disk.
const BROKER_STDERR_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// A broker notification bound for the webview: the Tauri event name and its
/// payload.
type DeviceEvent = (&'static str, Value);

/// The webview event a broker notification is forwarded as, or `None` for
/// anything that is not a notification this host relays (every RPC response).
fn device_event_name(message: &Value) -> Option<&'static str> {
    match message.get("method").and_then(Value::as_str)? {
        "device.stateFrame" => Some("qc-state-frame"),
        "device.phase" => Some("qc-device-phase"),
        _ => None,
    }
}

impl GatewayProcess {
    fn has_durable_forced_release(&self) -> bool {
        let GatewayTransport::SharedBroker {
            instance_id,
            release_marker_path,
            ..
        } = &self.transport
        else {
            return false;
        };
        read_forced_release_marker(release_marker_path, instance_id)
    }
    /// `auto_connect` is set for a broker replacing a failed one, so it
    /// restores the device session the client already had without waiting to be
    /// asked, and for the first broker of a headless MCP host, which has no
    /// connection flow of its own (`Gateway::auto_connect_first_broker`). A
    /// window's first broker stays idle and lets the window's connection flow
    /// own the handshake. It only matters to a broker this call spawns: one
    /// that is already running keeps whatever session it has.
    ///
    /// `descriptor_path` is where the shared broker is met. `None` keeps the
    /// private child, which is what a host gets when no directory for the
    /// descriptor could be resolved at all.
    fn start(
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        auto_connect: bool,
        descriptor_path: Option<&Path>,
    ) -> Result<Self, String> {
        // The one place every attach, election, shared spawn and private
        // `--stdio` child begins. Tests count it here; see `GatewayStartSeam`.
        #[cfg(all(test, windows))]
        require_native_test_state_root()?;
        #[cfg(test)]
        if let Some(starter) = gateway_start_seam(descriptor_path) {
            return starter(auto_connect);
        }
        match descriptor_path {
            Some(descriptor_path) => Self::start_shared(event_tx, auto_connect, descriptor_path),
            // A private `--stdio` broker is a second USB owner too.
            None if broker_spawn_forbidden() => {
                Err("No shared broker descriptor in the broker's own process".into())
            }
            None => Self::start_owned_child(event_tx, auto_connect),
        }
    }

    /// `start`, against an explicit descriptor path instead of the one resolved
    /// from the Tauri-managed `app_local_data_dir`, which a plain `cargo test`
    /// does not have.
    #[cfg(test)]
    fn start_with_descriptor_path(
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        auto_connect: bool,
        descriptor_path: &Path,
    ) -> Result<Self, String> {
        #[cfg(windows)]
        require_native_test_state_root()?;
        Self::start_shared(event_tx, auto_connect, descriptor_path)
    }

    /// Which transport this process ended up on.
    #[cfg(test)]
    fn uses_shared_broker(&self) -> bool {
        matches!(self.transport, GatewayTransport::SharedBroker { .. })
    }

    /// The broker's process id when this instance spawned it; `None` when it
    /// attached to a broker another process hosts.
    #[cfg(test)]
    fn hosted_broker_pid(&self) -> Option<u32> {
        match &self.transport {
            GatewayTransport::SharedBroker {
                hosted_child: Some(child),
                ..
            } => Some(child.id()),
            _ => None,
        }
    }

    /// Test-only ownership seam: hands the `Child` of the broker this gateway
    /// spawned to the test that caused the spawn, so the test waits for it and,
    /// if it must, terminates it through that handle and never by pid.
    /// Production keeps the handle only to pin the pid and never kills through
    /// it (see `hosted_child`), so taking it changes no lifetime: the broker
    /// still ends through its own grace period.
    #[cfg(test)]
    fn take_hosted_broker_child(&mut self) -> Option<Child> {
        match &mut self.transport {
            GatewayTransport::SharedBroker { hosted_child, .. } => hosted_child.take(),
            GatewayTransport::OwnedChild { .. } => None,
        }
    }

    /// The shared broker: attach to the published one, or win the election,
    /// spawn it, and attach to that.
    fn start_shared(
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        auto_connect: bool,
        descriptor_path: &Path,
    ) -> Result<Self, String> {
        let (transport, broker_pid) = attach_or_host_device_broker(descriptor_path, auto_connect)?;
        let GatewayTransport::SharedBroker { stream, .. } = &transport else {
            unreachable!("attach_or_host_device_broker only produces shared transports");
        };
        let reader = stream
            .try_clone()
            .map_err(|error| format!("Could not start gateway event reader: {error}"))?;
        let lock_free_writer = stream
            .try_clone()
            .map_err(|error| format!("Could not start gateway lock-free reads: {error}"))?;
        // From here on `transport` owns the cleanup: any early return drops it,
        // closing this connection. The broker itself is left to its own
        // zero-client grace period, even when this call spawned it.
        Self::with_transport(
            transport,
            broker_pid,
            reader,
            event_tx,
            Some(lock_free_writer),
        )
    }

    /// Attach to exactly this published broker: no election, never a spawn.
    /// Used only by `Gateway::attach_published_candidate`, which has already
    /// checked the descriptor; the authenticated handshake proves the
    /// connection reached that same instance.
    fn attach_exact(
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        descriptor: &device_broker_descriptor::DeviceBrokerDescriptor,
        release_marker_path: PathBuf,
        deadline: Instant,
    ) -> Result<Self, String> {
        // The connect and the authenticated handshake are bounded by their
        // own timeouts (0.5 s and 2 s, `broker_attach`); the version check
        // gets whatever is left of `deadline`, and nothing at all past it.
        let stream =
            attach_to_device_broker(descriptor).map_err(|failure| failure.message().to_owned())?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("The newer broker did not finish attaching in time".into());
        }
        let reader = stream
            .try_clone()
            .map_err(|error| format!("Could not start gateway event reader: {error}"))?;
        let lock_free_writer = stream
            .try_clone()
            .map_err(|error| format!("Could not start gateway lock-free reads: {error}"))?;
        Self::with_transport_within(
            GatewayTransport::SharedBroker {
                stream,
                hosted_child: None,
                instance_id: descriptor.instance_id.clone(),
                release_marker_path,
            },
            descriptor.process_id,
            reader,
            event_tx,
            Some(lock_free_writer),
            remaining,
        )
    }

    /// A private broker on stdio pipes, as every host used before the broker
    /// was shared.
    ///
    /// Reachable only when `descriptor_path` is `None` in `start`, which in
    /// production only happens if `DEVICE_BROKER_DESCRIPTOR_PATH` was never
    /// set - and Tauri `.setup()` (in `run()`, below) now sets it
    /// unconditionally, falling back to `std::env::temp_dir()` rather than
    /// leaving it unset on `app_local_data_dir()` failure, specifically so
    /// this path stays unreachable: it duplicates `start_shared`'s MIDI-
    /// warm-up and orphan-reaping logic (`orphan_guard.rs`) independently,
    /// and that duplication has drifted out of sync more than once. Do not "simplify" that fallback back into leaving the
    /// path unset without knowing this is what keeps this function dead in
    /// practice - see `a_missing_descriptor_directory_falls_back_to_temp_dir`
    /// below for the test guarding it.
    fn start_owned_child(
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        auto_connect: bool,
    ) -> Result<Self, String> {
        let mut command = native_broker_command()?;
        command.arg("--stdio");
        add_broker_session_arguments(&mut command, auto_connect);
        // Keep the broker's own diagnostics. Discarding them meant a panic, an
        // abort, or any startup complaint left no trace anywhere on the system,
        // so a broker that died was indistinguishable from one the host killed.
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = spawn_with_broker_stderr(&mut command)
            .map_err(|error| format!("Could not start device gateway: {error}"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Gateway stdio was not created".into());
        };
        let broker_pid = child.id();
        Self::with_transport(
            GatewayTransport::OwnedChild { child, stdin },
            broker_pid,
            stdout,
            event_tx,
            None,
        )
    }

    fn with_transport(
        transport: GatewayTransport,
        broker_pid: u32,
        reader: impl Read + Send + 'static,
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        lock_free_writer: Option<TcpStream>,
    ) -> Result<Self, String> {
        Self::with_transport_within(
            transport,
            broker_pid,
            reader,
            event_tx,
            lock_free_writer,
            GATEWAY_RESPONSE_TIMEOUT,
        )
    }

    /// `with_transport`, with its version check answered within `within`.
    fn with_transport_within(
        transport: GatewayTransport,
        broker_pid: u32,
        reader: impl Read + Send + 'static,
        event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
        lock_free_writer: Option<TcpStream>,
        within: Duration,
    ) -> Result<Self, String> {
        let intentional_release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lock_free_reads = lock_free_writer.map(|writer| {
            std::sync::Arc::new(LockFreeReads::new(writer, intentional_release.clone()))
        });
        let GatewayReader {
            responses,
            completed: reader_completed,
        } = spawn_gateway_reader_with(
            reader,
            event_tx,
            intentional_release.clone(),
            lock_free_reads.clone(),
        )?;
        let mut process = Self {
            transport,
            responses,
            reader_completed,
            next_id: 1,
            broker_pid,
            intentional_release,
            participant_announced: false,
            last_broker_target: None,
            lock_free_reads,
        };
        process.verify_gateway_api_version(within)?;
        Ok(process)
    }

    /// A stale `qc-device-broker.exe` left over from a previous install links
    /// its own older copy of the generated gateway contract and answers every
    /// call under it - including ones whose argument or result shape has
    /// since changed on this side. `system.status` has always reported
    /// `gatewayApiVersion`, but nothing ever compared it against what this
    /// running build expects: the drift this is meant to catch instead
    /// surfaced only indirectly, as a confusing device error several calls
    /// later (the `add_block` deadlock was exactly this shape). Checking once
    /// here, right after spawn and before anything else can use the process,
    /// turns that into one immediate, specific failure naming both versions.
    fn verify_gateway_api_version(&mut self, within: Duration) -> Result<(), String> {
        let status = self
            .request_as_within(rpc::RUNTIME_STATUS, json!({}), None, within)
            .map_err(|failure| match failure {
                GatewayRequestFailure::Transport(message)
                | GatewayRequestFailure::UnansweredMutation(message) => message,
                GatewayRequestFailure::Remote { message, .. } => message,
            })?;
        check_gateway_api_version(&status, self.broker_pid)
    }

    /// How many clients the shared broker is serving, this one included.
    /// `None` for a private child, which by definition serves only this host,
    /// and for a broker too old to answer.
    fn shared_broker_client_count(&mut self) -> Option<u64> {
        if !matches!(self.transport, GatewayTransport::SharedBroker { .. }) {
            return None;
        }
        self.request_unchecked(DEVICE_BROKER_CLIENT_COUNT_METHOD, json!({}))
            .ok()?
            .get("clients")
            .and_then(Value::as_u64)
    }

    /// Name this socket's actual owner only when the optional tray is enabled.
    /// Older brokers remain usable; a method-not-found response is not a
    /// transport failure and must not restart them.
    #[cfg(test)]
    fn announce_participant(&mut self, provider: &str) -> Result<(), GatewayRequestFailure> {
        self.announce_participant_with(provider, None)
    }

    /// `host_token` is presented only by the background MCP host inside
    /// `qc-device-broker.exe`: the token its own broker generated, which is
    /// how that broker tells its host from every other client.
    fn announce_participant_with(
        &mut self,
        provider: &str,
        host_token: Option<&str>,
    ) -> Result<(), GatewayRequestFailure> {
        if self.participant_announced
            || !matches!(self.transport, GatewayTransport::SharedBroker { .. })
        {
            return Ok(());
        }
        let mut hello = json!({"role":generated_broker_surface::ConnectionRole::Participant.wire_name(), "provider":provider});
        if let Some(token) = host_token {
            hello["hostToken"] = json!(token);
        }
        match self.request_unchecked("broker.hello", hello) {
            Ok(_) | Err(GatewayRequestFailure::Remote { code: -32601, .. }) => {
                self.participant_announced = true;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, GatewayRequestFailure> {
        self.request_as(method, params, None)
    }

    /// `request`, with the envelope's `origin` set when given. Sent only to a
    /// broker that lists [`BROKER_RESOURCE_READ_ORIGIN_CAPABILITY`]: an older
    /// broker rejects an envelope field it does not know.
    fn request_as(
        &mut self,
        method: &str,
        params: Value,
        origin: Option<&str>,
    ) -> Result<Value, GatewayRequestFailure> {
        self.request_as_within(method, params, origin, GATEWAY_RESPONSE_TIMEOUT)
    }

    /// `request_as`, giving up on the answer after `within`.
    fn request_as_within(
        &mut self,
        method: &str,
        params: Value,
        origin: Option<&str>,
        within: Duration,
    ) -> Result<Value, GatewayRequestFailure> {
        self.request_targeted_as_within(method, params, origin, None, within)
    }

    fn request_targeted_as_within(
        &mut self,
        method: &str,
        params: Value,
        origin: Option<&str>,
        expected: Option<&Value>,
        within: Duration,
    ) -> Result<Value, GatewayRequestFailure> {
        let result =
            self.request_unchecked_targeted_within(method, params, origin, expected, within)?;
        if matches!(method, "broker.devices" | "broker.selectDevice") {
            validate_broker_control_result(method, &result)?;
        } else if method == "broker.backupTarget" {
            serde_json::from_value::<quadcortex::device::owner::DispatchContext>(result.clone())
                .map_err(|_| {
                    GatewayRequestFailure::Transport(
                        "Broker returned an invalid backup session context".into(),
                    )
                })?;
        } else if method == DEVICE_BROKER_DISCONNECT_METHOD {
            validate_gateway_result(rpc::DISCONNECT, &result)?;
        } else if method == "broker.createDeviceBackupBound" {
            validate_gateway_result(rpc::CREATE_DEVICE_BACKUP, &result)?;
        } else {
            validate_gateway_result(method, &result)?;
        }
        Ok(result)
    }

    /// `request` without the generated result contract, which only covers the
    /// gateway's own methods: broker-level methods have no entry in it.
    fn request_unchecked(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, GatewayRequestFailure> {
        self.request_unchecked_as(method, params, None)
    }

    fn request_unchecked_as(
        &mut self,
        method: &str,
        params: Value,
        origin: Option<&str>,
    ) -> Result<Value, GatewayRequestFailure> {
        self.request_unchecked_as_within(method, params, origin, GATEWAY_RESPONSE_TIMEOUT)
    }

    fn request_unchecked_as_within(
        &mut self,
        method: &str,
        params: Value,
        origin: Option<&str>,
        within: Duration,
    ) -> Result<Value, GatewayRequestFailure> {
        self.request_unchecked_targeted_within(method, params, origin, None, within)
    }

    fn request_unchecked_targeted_within(
        &mut self,
        method: &str,
        params: Value,
        origin: Option<&str>,
        expected: Option<&Value>,
        within: Duration,
    ) -> Result<Value, GatewayRequestFailure> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        let mut envelope = json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        });
        if let Some(expected) = expected {
            envelope["expectedBrokerTarget"] = expected.clone();
        }
        if let Some(origin) = origin {
            if matches!(
                origin,
                PHYSICAL_TARGET_ORIGIN | PHYSICAL_RESOURCE_READ_ORIGIN
            ) {
                envelope["physicalOnly"] = json!(true);
            }
            if origin == PHYSICAL_RESOURCE_READ_ORIGIN {
                envelope["origin"] = json!(BROKER_RESOURCE_READ_ORIGIN);
            } else if origin != PHYSICAL_TARGET_ORIGIN {
                envelope["origin"] = json!(origin);
            }
        }
        let payload = serde_json::to_vec(&envelope)
            .map_err(|error| GatewayRequestFailure::Transport(error.to_string()))?;
        if payload.len() > quadcortex::domain::IPC_MAX_FRAME_BYTES {
            return Err(GatewayRequestFailure::Transport(format!(
                "Gateway request exceeds the {} byte IPC limit",
                quadcortex::domain::IPC_MAX_FRAME_BYTES
            )));
        }
        // Through the lock-free reads' writer when there is one: it is the
        // same socket, and sharing its lock keeps a read sent beside this
        // request from interleaving with this frame.
        match &self.lock_free_reads {
            Some(reads) => reads.write(&payload),
            None => match &mut self.transport {
                GatewayTransport::SharedBroker { stream, .. } => {
                    qc_broker_framing::write_tcp_frame_before_deadline(
                        stream,
                        &payload,
                        quadcortex::domain::IPC_MAX_FRAME_BYTES,
                        std::time::Instant::now() + GATEWAY_WRITE_TIMEOUT,
                    )
                    .map_err(|error| {
                        let _ = stream.shutdown(Shutdown::Both);
                        GatewayWriteFailure {
                            message: error.to_string(),
                            attempted: true,
                        }
                    })
                }
                // Legacy private-child pipe path retains its existing policy;
                // the TCP timeout does not claim cancellable Windows pipe I/O.
                GatewayTransport::OwnedChild { stdin, .. } => {
                    write_frame(stdin, &payload).map_err(|message| GatewayWriteFailure {
                        message,
                        attempted: true,
                    })
                }
            },
        }
        .map_err(|failure| failure.request_failure(method))?;

        let response = match self.responses.recv_timeout(within) {
            Ok(GatewayReaderMessage::Response(value)) => value,
            Ok(GatewayReaderMessage::Failed(error)) => {
                return Err(GatewayRequestFailure::Transport(error).after_dispatch(method));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Whole seconds as before; a bounded attach's remainder in ms.
                let waited = if within.as_secs() > 0 && within.subsec_millis() == 0 {
                    format!("{} seconds", within.as_secs())
                } else {
                    format!("{} ms", within.as_millis())
                };
                return Err(GatewayRequestFailure::Transport(format!(
                    "Gateway did not answer {method} within {waited}"
                ))
                .after_dispatch(method));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(GatewayRequestFailure::Transport(
                    "Gateway response channel closed".into(),
                )
                .after_dispatch(method));
            }
        };
        self.last_broker_target = response_target_metadata(&response)
            .map_err(|failure| failure.after_dispatch(method))?;
        decode_gateway_response(response, id).map_err(|failure| failure.after_dispatch(method))
    }
}

fn read_forced_release_marker(path: &Path, instance_id: &str) -> bool {
    // Same tiny local metadata contract as descriptors and user-Quit records:
    // one held file, 128 KiB software ceiling, no firmware limit.
    local_stdio::read_local_document(path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<generated_broker_surface::BrokerReleaseMarker>(&bytes).ok()
        })
        .is_some_and(|marker| marker.forced && marker.instance_id == instance_id)
}

/// One broker reply to request `id`: its result, or the broker's error. Shared
/// by the gateway's own requests and its lock-free reads, so both hold a reply
/// to the same standard.
fn response_target_metadata(
    response: &Value,
) -> Result<Option<qc_relay_client::BrokerTarget>, GatewayRequestFailure> {
    let invalid =
        || GatewayRequestFailure::Transport("The broker returned invalid target metadata".into());
    let envelope = response.get("brokerTarget");
    let nested = response
        .get("result")
        .and_then(|result| result.get("brokerTarget"));
    if envelope.is_some() && nested.is_some() && envelope != nested {
        return Err(invalid());
    }
    let Some(value) = envelope.or(nested) else {
        return Ok(None);
    };
    if value.as_object().is_none_or(|object| object.len() != 3) {
        return Err(invalid());
    }
    let target: qc_relay_client::BrokerTarget =
        serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if target.session_id.trim().is_empty()
        || target.session_id.len() > 256
        || target.generation == 0
        || target.generation > 9_007_199_254_740_991
    {
        return Err(invalid());
    }
    Ok(Some(target))
}

fn decode_gateway_response(response: Value, id: u64) -> Result<Value, GatewayRequestFailure> {
    let Some(object) = response.as_object() else {
        return Err(GatewayRequestFailure::Transport(
            "Gateway response was not an object".into(),
        ));
    };
    if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || response.get("id").and_then(Value::as_u64) != Some(id)
        || object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "jsonrpc" | "id" | "result" | "error" | "brokerTarget"
            )
        })
    {
        return Err(GatewayRequestFailure::Transport(
            "Gateway response did not match the request".into(),
        ));
    }
    response_target_metadata(&response)?;
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_result == has_error {
        return Err(GatewayRequestFailure::Transport(
            "Gateway response must contain exactly one result or error".into(),
        ));
    }
    if has_result {
        return Ok(response["result"].clone());
    }
    let error = response["error"].as_object().ok_or_else(|| {
        GatewayRequestFailure::Transport("Gateway returned a malformed error".into())
    })?;
    if error
        .keys()
        .any(|key| !matches!(key.as_str(), "code" | "message" | "data"))
    {
        return Err(GatewayRequestFailure::Transport(
            "Gateway returned a malformed error".into(),
        ));
    }
    let code = error.get("code").and_then(Value::as_i64).ok_or_else(|| {
        GatewayRequestFailure::Transport("Gateway returned a malformed error code".into())
    })?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            GatewayRequestFailure::Transport("Gateway returned a malformed error message".into())
        })?;
    let details = safe_gateway_failure_details(code, error.get("data"));
    let retryable = details
        .as_ref()
        .and_then(|details| details.get("retryable"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Err(GatewayRequestFailure::Remote {
        code,
        message: message.into(),
        retryable,
        details,
    })
}

/// Keep only the device error facts the host protocol understands. In
/// particular, never forward arbitrary `error.data` that could contain a
/// request payload or credential from a malformed broker.
fn safe_gateway_failure_details(code: i64, data: Option<&Value>) -> Option<Value> {
    let mut details = safe_gateway_error_details(data);
    // This legacy broker code is emitted only by its pre-dispatch gate.
    if code == -32012 {
        let value = details.get_or_insert_with(|| serde_json::json!({}));
        let object = value.as_object_mut().expect("sanitized object");
        object.insert("code".into(), serde_json::json!("WRITE_QUARANTINED"));
        object.insert("retryable".into(), Value::Bool(false));
        object.insert("requestSent".into(), Value::Bool(false));
        object.insert("writesQuarantined".into(), Value::Bool(true));
        object.remove("userActionRequired");
    } else if let Some(value) = details.as_mut() {
        if value.get("code").and_then(Value::as_str) == Some("WRITE_OUTCOME_AMBIGUOUS") {
            let object = value.as_object_mut().expect("sanitized object");
            object.remove("userActionRequired");
            object.insert("retryable".into(), Value::Bool(false));
        }
    }
    details
}

fn safe_gateway_error_details(data: Option<&Value>) -> Option<Value> {
    let source = data?.as_object()?;
    let mut details = serde_json::Map::new();
    // The broker's typed idle-reopen refusal is stable readiness evidence.
    // Do not forward arbitrary categories or infer this from overloaded -32002.
    if source.get("category").and_then(Value::as_str) == Some("notConnected") {
        details.insert("category".into(), Value::String("notConnected".into()));
    }
    if let Some(code) = source.get("code").and_then(Value::as_str).filter(|code| {
        !code.is_empty()
            && code.len() <= 64
            && code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    }) {
        details.insert("code".into(), Value::String(code.to_owned()));
    }
    for field in ["retryable", "requestSent"] {
        if let Some(value) = source.get(field).and_then(Value::as_bool) {
            details.insert(field.into(), Value::Bool(value));
        }
    }
    qc_relay_client::project_dispatch_error_details(
        source.get("code").and_then(Value::as_str),
        source,
        &mut details,
    );
    // This policy follows the exact machine code, never an untrusted flag or message.
    let action_required = match details.get("code").and_then(Value::as_str) {
        Some("UNSAVED_STATE_UNCONFIRMED") => Some(true),
        Some("UNSAVED_STATE_NOT_YET_OBSERVED") => Some(false),
        _ => None,
    };
    if let Some(required) = action_required {
        details.insert("userActionRequired".into(), Value::Bool(required));
        details.insert("retryable".into(), Value::Bool(!required));
    }
    (!details.is_empty()).then_some(Value::Object(details))
}

fn device_error_from_gateway_failure(failure: GatewayRequestFailure) -> DeviceError {
    match failure {
        GatewayRequestFailure::Remote {
            code,
            message,
            retryable,
            details,
        } => DeviceError {
            code: match details
                .as_ref()
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
            {
                Some("WRITE_QUARANTINED") => "WRITE_QUARANTINED".into(),
                Some("WRITE_OUTCOME_AMBIGUOUS") => "WRITE_OUTCOME_AMBIGUOUS".into(),
                Some("UNSAVED_STATE_UNCONFIRMED") => "UNSAVED_STATE_UNCONFIRMED".into(),
                Some("UNSAVED_STATE_NOT_YET_OBSERVED") => "UNSAVED_STATE_NOT_YET_OBSERVED".into(),
                _ => code.to_string(),
            },
            message,
            retryable,
            user_action_required: details
                .as_ref()
                .and_then(|value| value.get("userActionRequired"))
                .and_then(Value::as_bool),
            details: details.map(Box::new),
        },
        GatewayRequestFailure::Transport(message) => {
            DeviceError::new("DEVICE_TRANSPORT", message, true)
        }
        GatewayRequestFailure::UnansweredMutation(message) => DeviceError {
            code: "WRITE_OUTCOME_AMBIGUOUS".into(),
            message,
            retryable: false,
            user_action_required: Some(true),
            details: Some(Box::new(
                json!({"code":"WRITE_OUTCOME_AMBIGUOUS", "requestSent":true,
                "retryable":false,"userActionRequired":true,"verification":"unknown"}),
            )),
        },
    }
}

/// A result that does not satisfy its contract is a protocol fault in a broker
/// that is otherwise alive, answering, and correctly framed. Reporting it as a
/// transport failure made the host kill and respawn a healthy process on every
/// occurrence, turning one malformed field into a dropped device session.
fn validate_gateway_result(method: &str, result: &Value) -> Result<(), GatewayRequestFailure> {
    generated_gateway::validate_result(method, result).map_err(|message| {
        GatewayRequestFailure::Remote {
            code: GATEWAY_CONTRACT_ERROR_CODE,
            message,
            retryable: false,
            details: None,
        }
    })
}

/// The shared broker has a small control surface outside the device gateway
/// contract. Validate those two UI-facing replies explicitly instead of
/// sending broker methods through the generated `device.*` result catalog.
fn validate_broker_control_result(
    method: &str,
    result: &Value,
) -> Result<(), GatewayRequestFailure> {
    let malformed = |detail: &str| GatewayRequestFailure::Remote {
        code: GATEWAY_CONTRACT_ERROR_CODE,
        message: format!("{method} returned a malformed broker-control result: {detail}"),
        retryable: false,
        details: None,
    };
    let Some(object) = result.as_object() else {
        return Err(malformed("expected an object"));
    };
    match method {
        "broker.devices" => {
            let Some(devices) = object.get("devices").and_then(Value::as_array) else {
                return Err(malformed("devices must be an array"));
            };
            let selected = object.get("selectedDeviceId");
            if !selected.is_some_and(|value| value.is_null() || value.as_str().is_some()) {
                return Err(malformed("selectedDeviceId must be a string or null"));
            }
            for device in devices {
                let Some(device) = device.as_object() else {
                    return Err(malformed("each device must be an object"));
                };
                if !device
                    .get("id")
                    .is_some_and(|value| value.as_str().is_some_and(|id| !id.is_empty()))
                    || !device.get("name").is_some_and(Value::is_string)
                    || !device.get("connected").is_some_and(Value::is_boolean)
                    || !device.get("phase").is_some_and(Value::is_string)
                {
                    return Err(malformed(
                        "each device needs id, name, connected, and phase fields",
                    ));
                }
            }
            Ok(())
        }
        "broker.selectDevice" => {
            if object
                .get("selectedDeviceId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty())
            {
                Ok(())
            } else {
                Err(malformed("selectedDeviceId must be a non-empty string"))
            }
        }
        _ => Err(malformed("method is not an exposed broker-control method")),
    }
}

/// Preserve readiness handling for explicit session loss and the legacy -32011
/// broker code. Current scoped deadline errors are ordinary remote failures;
/// they do not prove disconnection and must not change readiness. This check
/// neither kills the broker nor initiates a USB reconnect.
fn remote_failure_marks_disconnected(code: i64, message: &str) -> bool {
    message.contains("No Quad Cortex session") || code == RESERVE_REQUEST_ID_TIMEOUT_ERROR_CODE
}

/// Compare the broker's self-reported `gatewayApiVersion` (from a
/// `system.status` result) against the version this build was compiled
/// against. Kept separate from `GatewayProcess::verify_gateway_api_version`
/// so the three outcomes - match, mismatch, missing - are each testable
/// without spawning a real broker process.
///
/// The message names the broker's process id. A shared broker outlives any one
/// window: after an upgrade, a broker started by the old version keeps running
/// for as long as any client - another window, a headless MCP bridge - stays
/// attached, and a new window meets it here. Nothing can replace a broker that
/// is alive and answering, so the person reading this has to know which
/// process to close.
fn check_gateway_api_version(status: &Value, broker_pid: u32) -> Result<(), String> {
    match status.get("gatewayApiVersion").and_then(Value::as_u64) {
        Some(version) if version == generated_gateway::API_VERSION => Ok(()),
        Some(version) => Err(format!(
            "Gateway API version mismatch: this QC Remote build expects broker API version {}, \
             but the running qc-device-broker (process {broker_pid}) reports version {version}. \
             A broker left running by an older install is the usual cause. Close every QC Remote \
             window and MCP client so it exits on its own, or end qc-device-broker.exe process \
             {broker_pid} in Task Manager, then try again.",
            generated_gateway::API_VERSION
        )),
        None => Err(format!(
            "Gateway API version mismatch: the running qc-device-broker (process {broker_pid}) \
             did not report a gatewayApiVersion at all (this QC Remote build expects version {}). \
             Close every QC Remote window and MCP client, or end qc-device-broker.exe process \
             {broker_pid} in Task Manager, then try again.",
            generated_gateway::API_VERSION
        )),
    }
}

/// The broker executable: beside this app, or in a debug build from
/// `QC_GATEWAY_EXECUTABLE`.
fn native_broker_command() -> Result<Command, String> {
    #[allow(unused_mut)] // Only Windows creation flags and test env mutate it.
    let mut command = match broker_executable_override(
        cfg!(debug_assertions),
        std::env::var_os("QC_GATEWAY_EXECUTABLE"),
    ) {
        Some(executable) => Command::new(executable),
        None => Command::new(default_broker_executable()?),
    };
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    // A broker draws its own tray icon on an interactive desktop. Tests that
    // reach this spawn path must not put icons on a developer's taskbar.
    #[cfg(test)]
    command.env("QC_DEVICE_BROKER_NO_TRAY", "1");
    Ok(command)
}

/// Which broker to run instead of the installed one, if any.
///
/// Development and the shared-broker integration harness point
/// `QC_GATEWAY_EXECUTABLE` at a freshly built broker. A release build ignores
/// it: the broker owns the USB device and mints the bearer token every local
/// client uses, so anything able to set this user's environment could
/// otherwise have every QC Remote launch start its own program in that role.
fn broker_executable_override(
    debug_build: bool,
    configured: Option<std::ffi::OsString>,
) -> Option<std::ffi::OsString> {
    configured.filter(|value| debug_build && !value.is_empty())
}

#[cfg(not(test))]
fn default_broker_executable() -> Result<PathBuf, String> {
    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    locate_native_broker(executable_directory.as_deref()).ok_or_else(|| {
        format!(
            "The QC Remote device broker ({}) is missing beside QC Remote. Reinstall QC Remote.",
            native_broker_filename()
        )
    })
}

/// The test runner prepares the broker before starting the app tests. A test
/// must never run Cargo recursively while another Cargo test holds the build
/// lock; the explicit override lets the gate name the artifact it just built.
#[cfg(test)]
fn default_broker_executable() -> Result<PathBuf, String> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("..")
                .join("target")
        });
    let executable_name = format!("qc-device-broker{}", std::env::consts::EXE_SUFFIX);
    let executable = std::env::var_os("QC_GATEWAY_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| target.join("debug").join(executable_name));
    if executable.is_file() {
        Ok(executable)
    } else {
        Err(format!(
            "The prebuilt QC device broker is missing at {}; build it before running app tests or set QC_GATEWAY_EXECUTABLE to its path.",
            executable.display()
        ))
    }
}

/// The flags both broker modes take.
fn add_broker_session_arguments(command: &mut Command, auto_connect: bool) {
    // Lets the connection badge stay gray until a Quad Cortex actually
    // answers: device.reconnect holds this gateway for the whole handshake,
    // so the phase stream is the only way to see detection while it runs.
    // A broker too old to know the flag ignores it, and the UI then waits
    // for the reconnect result instead.
    command.arg("--phase-events");
    if auto_connect {
        command.arg("--auto-connect");
    }
}

/// Reads every frame the broker sends: notifications go to the webview,
/// responses to the request waiting on the returned receiver.
// Software process-wide headroom, including originals still retiring after a
// reconnect. A body-completion notification does not release native admission.
static GATEWAY_READER_THREADS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(16);

struct GatewayReader {
    responses: mpsc::Receiver<GatewayReaderMessage>,
    completed: mpsc::Receiver<()>,
}

/// Completion is published after the reader has consumed its last buffered
/// notification, including during an unwind. A failed writer must not guess
/// whether the independent reader already saw a deliberate release.
struct GatewayReaderCompletion(mpsc::Sender<()>);
impl Drop for GatewayReaderCompletion {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[cfg(test)]
fn spawn_gateway_reader(
    reader: impl Read + Send + 'static,
    event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
    intentional_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<GatewayReader, String> {
    spawn_gateway_reader_with(reader, event_tx, intentional_release, None)
}

/// `lock_free_reads` receives every reply in its own id range; everything
/// else is routed exactly as before.
fn spawn_gateway_reader_with(
    reader: impl Read + Send + 'static,
    event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
    intentional_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    lock_free_reads: Option<std::sync::Arc<LockFreeReads>>,
) -> Result<GatewayReader, String> {
    spawn_gateway_reader_with_budget(
        reader,
        event_tx,
        intentional_release,
        lock_free_reads,
        &GATEWAY_READER_THREADS,
    )
}

fn spawn_gateway_reader_with_budget(
    reader: impl Read + Send + 'static,
    event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
    intentional_release: std::sync::Arc<std::sync::atomic::AtomicBool>,
    lock_free_reads: Option<std::sync::Arc<LockFreeReads>>,
    budget: &'static qc_native_platform::thread_owner::Budget,
) -> Result<GatewayReader, String> {
    let (response_tx, responses) = mpsc::channel();
    let (completion_tx, completed) = mpsc::channel();
    let original = qc_native_platform::thread_owner::Owned::spawn(
        "qc-gateway-events",
        budget,
        (),
        move || {
            let _completion = GatewayReaderCompletion(completion_tx);
            let mut reader = BufReader::new(reader);
            loop {
                match read_frame(&mut reader) {
                    Ok(value) if value["method"] == "broker.shuttingDown" => {
                        if value["params"]["forced"] == true {
                            // Set before reading EOF. Even a poll racing the
                            // close must preserve the user's deliberate release.
                            intentional_release.store(true, std::sync::atomic::Ordering::Release);
                        }
                    }
                    Ok(value) if value["method"] == "broker.clientsChanged" => {}
                    Ok(value) if device_event_name(&value).is_some() => {
                        if let (Some(sender), Some(name), Some(params)) = (
                            event_tx.as_ref(),
                            device_event_name(&value),
                            value.get("params"),
                        ) {
                            // State events carry sequence numbers and the UI can
                            // reconcile from a fresh snapshot, and a phase event is
                            // superseded by the next one. Never let a stalled
                            // renderer block the response reader needed by commands.
                            let _ = sender.try_send((name, params.clone()));
                        }
                    }
                    Ok(value)
                        if value
                            .get("id")
                            .and_then(Value::as_u64)
                            .is_some_and(|id| id >= LOCK_FREE_READ_ID_BASE) =>
                    {
                        if let (Some(reads), Some(id)) = (
                            lock_free_reads.as_ref(),
                            value.get("id").and_then(Value::as_u64),
                        ) {
                            reads.deliver(id, value);
                        }
                    }
                    Ok(value) => {
                        if response_tx
                            .send(GatewayReaderMessage::Response(value))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if let Some(reads) = lock_free_reads.as_ref() {
                            reads.close();
                        }
                        let _ = response_tx.send(GatewayReaderMessage::Failed(error));
                        return;
                    }
                }
            }
        },
    )
    .map_err(|error| format!("Could not start gateway event reader: {error}"))?;
    // Transfer the exact original to the existing shared owner family. Future
    // admissions reap acknowledged originals; active/unknown readers retain
    // their permits through native TLS exit and successful original join.
    // No GUI caller blocks waiting for a synchronous reader to retire.
    drop(original);
    Ok(GatewayReader {
        responses,
        completed,
    })
}

/// Where the shared broker's descriptor directory should live, given what
/// Tauri's own resolver answered.
///
/// Falls back to the OS temp directory, joined with the bundle identifier
/// rather than a hardcoded display name, when `app_local_data_dir` errors -
/// `local_stdio.rs`'s headless host resolves this exact directory
/// independently for the same descriptor, under its own `APP_DIRECTORY`
/// constant pinned to this same identifier by a test, so the two must agree
/// on the joined name or a window and a headless bridge degrading on the
/// same machine would each land in a different temp directory and never
/// find each other's broker. `app_local_data_dir` failing at all is not
/// expected to happen in practice, and `std::env::temp_dir` cannot fail the
/// same way - it names a directory without creating or verifying it - so
/// this keeps every window on the one, better-tested shared-broker hosting
/// mode (`GatewayProcess::start_shared`) instead of silently degrading to
/// the private, unshared one (`start_owned_child`) on a resolution error.
#[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
fn resolve_descriptor_directory(
    app_local_data_dir: Result<PathBuf, impl std::fmt::Display>,
    bundle_identifier: &str,
) -> PathBuf {
    app_local_data_dir.unwrap_or_else(|error| {
        eprintln!(
            "QC Remote could not resolve the shared device broker directory: {error}; falling \
             back to the OS temp directory rather than a private, unshared broker."
        );
        std::env::temp_dir().join(bundle_identifier)
    })
}

/// A failed setup hook otherwise ends in an opaque Tauri panic. Say what
/// went wrong on stderr and, when a desktop dialog tool exists, in a dialog.
#[cfg(target_os = "linux")]
fn report_startup_failure(error: &str) {
    let message = format!("QC Remote could not start.\n\n{error}");
    eprintln!("{message}");
    let attempts: [(&str, Vec<&str>); 2] = [
        (
            "zenity",
            vec![
                "--error",
                "--no-markup",
                "--title=QC Remote",
                "--text",
                &message,
            ],
        ),
        ("kdialog", vec!["--title", "QC Remote", "--error", &message]),
    ];
    for (program, arguments) in attempts {
        if std::process::Command::new(program)
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return;
        }
    }
}

fn app_descriptor_directory(
    app_local_data_dir: Result<PathBuf, impl std::fmt::Display>,
    bundle_identifier: &str,
) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = (app_local_data_dir, bundle_identifier);
        let directory = broker_paths::app_local_data_dir();
        qc_native_platform::paths::prepare_directory(&directory)?;
        Ok(directory)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (app_local_data_dir, bundle_identifier);
        let directory = broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&directory)?;
        Ok(directory)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(resolve_descriptor_directory(
            app_local_data_dir,
            bundle_identifier,
        ))
    }
}

/// Record where the shared broker's descriptor lives. Called once from Tauri
/// setup, before any command can reach the gateway.
fn set_device_broker_descriptor_directory(app_local_data_dir: &Path) {
    let _ = DEVICE_BROKER_DESCRIPTOR_PATH.set(
        device_broker_descriptor::device_broker_descriptor_path(app_local_data_dir),
    );
}

/// Serialises the processes deciding who hosts the broker. It sits beside the
/// descriptor rather than being the descriptor itself because the broker
/// claims the descriptor path on its own, with `create_new`, once it has bound
/// its port; a parent holding that same path would make every child it spawns
/// lose its own claim and exit. The broker also starts its device controller -
/// with `--auto-connect`, the USB session - before that claim, so the election
/// has to be settled before a broker is spawned, not by the broker.
fn device_broker_election_path(descriptor_path: &Path) -> PathBuf {
    descriptor_path.with_extension("election")
}

/// A shared-broker transport and the broker's process id.
type SharedBrokerAttachment = (GatewayTransport, u32);

/// The published broker, if a descriptor names one.
///
/// `now_ms` is the clock the descriptor's staleness is judged against. The
/// attach loop passes the real time, so an expired descriptor is not worth a
/// connect. The election winner passes `0`, which accepts a descriptor of any
/// age: before it may delete a descriptor, even an expired one, it has to have
/// proved the broker behind it dead.
fn attach_to_published_device_broker_with_admission(
    descriptor_path: &Path,
    now_ms: u64,
    admission: &qc_native_platform::owner_identity::RecoveryAdmission,
) -> Option<Result<SharedBrokerAttachment, BrokerAttachFailure>> {
    attach_to_published_device_broker_assessed(descriptor_path, now_ms, &|pid, created| {
        admission.assess(pid, created)
    })
}

fn attach_to_published_device_broker_assessed(
    descriptor_path: &Path,
    now_ms: u64,
    assess: &impl Fn(u32, Option<u64>) -> qc_native_platform::owner_identity::Assessment,
) -> Option<Result<SharedBrokerAttachment, BrokerAttachFailure>> {
    let descriptor = device_broker_descriptor::read_descriptor(descriptor_path, now_ms)?;
    use qc_native_platform::owner_identity::Assessment;
    let verdict = assess(descriptor.process_id, descriptor.process_created_at);
    if verdict == Assessment::GoneOrDifferent {
        return Some(Err(BrokerAttachFailure::Gone(
            "The published broker process generation is gone or different".into(),
        )));
    }
    Some(
        attach_to_device_broker(&descriptor)
            .map_err(|error| {
                if verdict == Assessment::Unknown {
                    BrokerAttachFailure::Unresponsive(format!(
                        "Broker owner identity is unavailable; its records were preserved: {}",
                        error.message()
                    ))
                } else {
                    error
                }
            })
            .map(|stream| {
                (
                    GatewayTransport::SharedBroker {
                        stream,
                        hosted_child: None,
                        instance_id: descriptor.instance_id.clone(),
                        release_marker_path:
                            generated_broker_surface::BrokerReleaseMarker::path_for(
                                descriptor_path,
                                &descriptor.instance_id,
                            ),
                    },
                    descriptor.process_id,
                )
            }),
    )
}

/// Attach to the machine's shared broker, electing and spawning one if none
/// answers.
///
/// Only a process that holds the election lock ever spawns a broker or deletes
/// a descriptor. Everyone else keeps trying to attach until the holder's broker
/// is published, so two windows opened together converge on one broker rather
/// than two brokers fighting over the USB device.
fn attach_or_host_device_broker(
    descriptor_path: &Path,
    auto_connect: bool,
) -> Result<SharedBrokerAttachment, String> {
    #[cfg(windows)]
    if !broker_spawn_forbidden() {
        if let Some(refusal) = hidden_resident_refusal(
            descriptor_path,
            session_marker_present(descriptor_path),
            device_broker_descriptor::unix_time_ms(),
            &broker_attach::published_broker_process_is_alive,
        ) {
            return Err(refusal);
        }
    }
    // After a user's Quit only the user brings QC Remote back: autonomous
    // demand (a tool call, an open window's automatic connect or timer, a
    // gateway's recovery) neither elects nor spawns. An explicit user action
    // clears the record before it gets here (`acknowledge_user_connect_in`).
    if !broker_spawn_forbidden() {
        if let Some(refusal) = user_quit_refusal(descriptor_path) {
            return Err(refusal);
        }
    }
    attach_or_host_device_broker_within(
        descriptor_path,
        auto_connect,
        DEVICE_BROKER_ELECTION_TIMEOUT,
        !broker_spawn_forbidden(),
    )
}

/// Refuses new hosting while the session's resident host is running where
/// this process cannot attach to it: its marker (a session-wide named mutex)
/// is held and no attachable descriptor is published in this directory. The
/// same predicate as the CLI's `residentNotVisible` (marker without a visible
/// live descriptor). A live descriptor here is attached as before.
/// Conservative: the marker is also held briefly by a resident that is exiting
/// and by a pre-0.1.178 tray; either refuses until it is gone.
#[cfg_attr(not(windows), allow(dead_code))]
fn hidden_resident_refusal(
    descriptor_path: &Path,
    session_marker: bool,
    now_ms: u64,
    alive: &dyn Fn(&device_broker_descriptor::DeviceBrokerDescriptor) -> bool,
) -> Option<String> {
    let attachable = device_broker_descriptor::read_descriptor(descriptor_path, now_ms)
        .is_some_and(|descriptor| alive(&descriptor));
    (session_marker && !attachable).then(|| {
        if cfg!(target_os = "linux") {
            "No new broker was started. Quit QC Remote from its tray icon, then open QC Remote again from your applications menu."
        } else {
            "No new broker was started. Quit QC Remote from its notification-area icon, then open QC Remote again from the Start menu."
        }
        .to_owned()
    })
}

/// The session marker as this process sees it. Under test it is only ever
/// what a test pinned for its own descriptor path (unpinned: absent), so a
/// resident running on the developer's machine changes no other test.
fn session_marker_present(descriptor_path: &Path) -> bool {
    #[cfg(test)]
    {
        PINNED_SESSION_MARKERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|(path, present)| path == descriptor_path && *present)
    }
    #[cfg(all(not(test), windows))]
    {
        let _ = descriptor_path;
        crate::tray_native::is_running()
    }
    #[cfg(all(not(test), not(windows)))]
    {
        #[cfg(target_os = "linux")]
        {
            descriptor_path
                .parent()
                .is_some_and(qc_native_platform::resident_marker::is_running)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = descriptor_path;
            false
        }
    }
}

/// Test-only: the session marker per descriptor path (see above).
#[cfg(test)]
static PINNED_SESSION_MARKERS: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::new());

/// `Some(advice)` while a user's Quit recorded for this directory stands
/// (`device_broker_descriptor::read_user_quit`, written by the broker that was
/// quit): no broker may be elected or spawned and no bridge may host for
/// autonomous demand. The answer is recoverable and says how to reconnect
/// ([`local_stdio::POST_QUIT_ADVICE`]).
///
/// The record is bound to the quit broker's instance, never to a time. A live
/// broker under another instance id was started after the Quit by a user (it
/// cleared the record) or by anything else that published here; it is
/// attachable, and attaching starts nothing, so the record does not block it.
/// The quit generation's own descriptor, while it is still on its way out,
/// is not attachable. A record that cannot be read stands and names no
/// generation. Whether anything here is attachable is the same predicate
/// [`hidden_resident_refusal`] asks of the session marker, with the record as
/// the condition and the quit generation excluded.
pub(crate) fn user_quit_refusal(descriptor_path: &Path) -> Option<String> {
    user_quit_refusal_at(
        descriptor_path,
        device_broker_descriptor::unix_time_ms(),
        &broker_attach::published_broker_process_is_alive,
    )
}

fn user_quit_refusal_at(
    descriptor_path: &Path,
    now_ms: u64,
    alive: &dyn Fn(&device_broker_descriptor::DeviceBrokerDescriptor) -> bool,
) -> Option<String> {
    // An unreadable record names no generation, so it cannot tell a newer
    // broker from the quit one: nothing is attachable and it refuses, live
    // descriptor or not, until a user action clears it.
    let quit_instance = match device_broker_descriptor::read_user_quit(descriptor_path)? {
        Ok(record) => record.instance_id,
        Err(_) => return Some(local_stdio::POST_QUIT_ADVICE.to_owned()),
    };
    let attachable = |descriptor: &device_broker_descriptor::DeviceBrokerDescriptor| {
        descriptor.instance_id != quit_instance && alive(descriptor)
    };
    hidden_resident_refusal(descriptor_path, true, now_ms, &attachable)
        .map(|_| local_stdio::POST_QUIT_ADVICE.to_owned())
}

/// How long a user action's clear waits for the election lock; the broker's
/// writer waits as long (`user_quit::LOCK_BUDGET`).
const USER_QUIT_LOCK_BUDGET: Duration = Duration::from_secs(2);

/// The user's own Connect, USB or device click in a window, or an actual app
/// launch (setup, and a second launch the single-instance callback receives),
/// supersedes a recorded Quit before the connect it starts reaches the
/// gateway. Never called on mount, a timer, device detection or a webview
/// reload. Each such action calls it exactly once.
pub(crate) fn acknowledge_user_connect_in(descriptor_path: &Path) -> Result<bool, String> {
    acknowledge_user_connect_within(descriptor_path, USER_QUIT_LOCK_BUDGET)
}

/// `acknowledge_user_connect_in` under the election lock
/// (`<descriptor>.election`): the lock every election, the broker's record
/// writer and its compare-and-remove hold, so a clear never lands in the
/// middle of one and never interleaves with a Quit being recorded. A lock that
/// cannot be had within `budget` is an explicit failure and the record stands.
fn acknowledge_user_connect_within(
    descriptor_path: &Path,
    budget: Duration,
) -> Result<bool, String> {
    let election_path = device_broker_election_path(descriptor_path);
    let deadline = Instant::now() + budget;
    let election = loop {
        match device_broker_descriptor::try_claim_descriptor_slot(&election_path) {
            Ok(election) => break election,
            Err(()) if Instant::now() >= deadline => {
                return Err(
                    "Could not clear the recorded Quit: the election lock is held; try again"
                        .into(),
                )
            }
            Err(()) => {
                remove_abandoned_election_lock(&election_path);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let cleared = device_broker_descriptor::clear_user_quit(descriptor_path);
    drop(election);
    device_broker_descriptor::release_descriptor_slot(&election_path);
    cleared
}

#[tauri::command]
async fn acknowledge_user_connect() -> Result<(), String> {
    // Off the async runtime: the clear may wait for the election lock.
    tauri::async_runtime::spawn_blocking(|| match DEVICE_BROKER_DESCRIPTOR_PATH.get() {
        Some(descriptor_path) => acknowledge_user_connect_in(descriptor_path).map(|_| ()),
        // No shared descriptor: no shared broker, so no recorded Quit either.
        None => Ok(()),
    })
    .await
    .map_err(|error| error.to_string())?
}

/// An actual app launch supersedes a recorded Quit. Its clear failing is
/// reported where the Settings window shows the background host's errors
/// (and on stderr), never swallowed: the record then still stands and the
/// window's connects are refused until a click clears it.
#[cfg_attr(any(test, not(windows)), allow(dead_code))]
fn acknowledge_app_launch(app: &AppHandle) {
    let Some(descriptor_path) = DEVICE_BROKER_DESCRIPTOR_PATH.get() else {
        return;
    };
    if let Err(error) = acknowledge_user_connect_in(descriptor_path) {
        eprintln!("QC Remote: {error}");
        if let Some(tray_control) = app.try_state::<broker_tray_control::BrokerTrayControl>() {
            tray_control.report_error(error);
        }
    }
}

// Software headroom for explicit GUI relaunch acknowledgments, not device
// capacity. A stuck original retains admission and its AppHandle until join.
#[cfg(all(any(windows, target_os = "linux", target_os = "macos"), not(test)))]
static RELAUNCH_ACKNOWLEDGMENTS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(4);

#[cfg(any(windows, all(any(target_os = "linux", target_os = "macos"), not(test))))]
fn start_relaunch_acknowledgment(
    budget: &'static qc_native_platform::thread_owner::Budget,
    resource: impl Send + 'static,
    action: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let original = qc_native_platform::thread_owner::Owned::spawn(
        "qc-user-quit-relaunch",
        budget,
        resource,
        action,
    )
    .map_err(|error| error.to_string())?;
    // Same shared owner family as the native reader/await work. Drop retains
    // the exact original nonblockingly; later admissions reap actual joins.
    drop(original);
    Ok(())
}

/// `attach_or_host_device_broker` with an explicit overall budget.
///
/// A published broker that is still running but does not answer is waited
/// for, never replaced: this process does not even enter the election for it.
/// If it never answers within `budget` the caller gets an error and no second
/// broker is started. That costs availability while a broker is wedged, which
/// is the right side to err on - two brokers on one USB device is a
/// correctness failure.
///
/// `may_host` is false in the broker's own process (`forbid_broker_spawn`):
/// there it only waits for its own broker's descriptor and never elects,
/// deletes a descriptor or spawns.
fn attach_or_host_device_broker_within(
    descriptor_path: &Path,
    auto_connect: bool,
    budget: Duration,
    may_host: bool,
) -> Result<SharedBrokerAttachment, String> {
    if let Some(directory) = descriptor_path.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create the device broker directory: {error}"))?;
    }
    let election_path = device_broker_election_path(descriptor_path);
    let deadline = Instant::now() + budget;
    let admission = prepare_broker_recovery_admission(descriptor_path, may_host);
    qc_native_platform::owner_identity::after_preparation(deadline, Instant::now(), |_| {
        attach_or_elect_until(
            deadline,
            may_host,
            || {
                // Every attach retry keeps the same-generation refusal: while a
                // user's Quit stands and nothing here is attachable but the quit
                // generation on its way out (or nothing at all), this is "no
                // broker", so the next step is the election, whose spawn
                // admission refuses with the Quit's advice. A newer live broker is
                // attached as before.
                if may_host && user_quit_refusal(descriptor_path).is_some() {
                    return None;
                }
                attach_to_published_device_broker_with_admission(
                    descriptor_path,
                    device_broker_descriptor::unix_time_ms(),
                    &admission,
                )
            },
            |last_error| match device_broker_descriptor::try_claim_descriptor_slot(&election_path) {
                Ok(election) => {
                    let outcome = host_device_broker_with_admission(
                        descriptor_path,
                        auto_connect,
                        &admission,
                    );
                    drop(election);
                    device_broker_descriptor::release_descriptor_slot(&election_path);
                    match outcome? {
                        HostAttempt::Attached(attachment) => Ok(Some(attachment)),
                        HostAttempt::Occupied(error) => {
                            *last_error = error;
                            Ok(None)
                        }
                    }
                }
                Err(()) => {
                    remove_abandoned_election_lock(&election_path);
                    Ok(None)
                }
            },
            Instant::now,
            std::thread::sleep,
        )
    })
    .map_err(str::to_owned)?
}

/// The attach-or-elect decision of `attach_or_host_device_broker_within`,
/// with its effects injected so it can be tested without sockets or a clock.
///
/// `attach` makes one attach attempt against the published descriptor.
/// `elect` runs one election; it answers `Some` with the attachment it
/// hosted, or `None` (having updated the last error) when the caller must
/// keep waiting. `now` and `sleep` are the clock.
fn attach_or_elect_until<T>(
    deadline: Instant,
    may_host: bool,
    mut attach: impl FnMut() -> Option<Result<T, BrokerAttachFailure>>,
    mut elect: impl FnMut(&mut String) -> Result<Option<T>, String>,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<T, String> {
    let mut last_error = String::from("no broker was published");
    loop {
        let may_elect = match attach() {
            Some(Ok(attachment)) => return Ok(attachment),
            Some(Err(BrokerAttachFailure::Unresponsive(error))) => {
                last_error = error;
                false
            }
            Some(Err(BrokerAttachFailure::Gone(error))) => {
                last_error = error;
                // A live broker's listener can refuse a single connection
                // under genuine, severe machine load (a full accept backlog,
                // or the accept thread briefly starved of CPU) without the
                // process actually being dead. A single Gone verdict is not
                // enough to justify deleting a published broker's descriptor
                // and spawning a second one on the same USB device - pause
                // briefly, giving a transient condition (not this process's
                // own long-running work - it holds nothing else during this
                // wait) a moment to clear, then confirm the verdict holds
                // before acting on it.
                sleep(DEVICE_BROKER_GONE_CONFIRMATION_DELAY);
                match attach() {
                    Some(Ok(attachment)) => return Ok(attachment),
                    Some(Err(BrokerAttachFailure::Gone(confirmed))) => {
                        last_error = confirmed;
                        true
                    }
                    Some(Err(BrokerAttachFailure::Unresponsive(recovered))) => {
                        last_error = recovered;
                        false
                    }
                    None => false,
                }
            }
            None => true,
        };
        // In the broker's own process only its own broker may serve: wait for
        // it to publish (or report it unavailable), never elect.
        if may_elect && !may_host {
            last_error = "the broker in this process has not published its connection".into();
        } else if may_elect {
            if let Some(attachment) = elect(&mut last_error)? {
                return Ok(attachment);
            }
        }
        if now() >= deadline {
            return Err(format!(
                "Could not reach the shared QC device broker: {last_error}"
            ));
        }
        sleep(DEVICE_BROKER_POLL_INTERVAL);
    }
}

/// A lock whose holder died mid-election would otherwise block every later
/// election for good.
fn remove_abandoned_election_lock(election_path: &Path) {
    let abandoned = fs::metadata(election_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > DEVICE_BROKER_ELECTION_LOCK_STALE_AFTER);
    if abandoned {
        device_broker_descriptor::release_descriptor_slot(election_path);
    }
}

/// What the election winner achieved, short of an outright failure.
enum HostAttempt {
    Attached(SharedBrokerAttachment),
    /// A broker that is still running holds the descriptor but did not
    /// answer, so it was left alone and nothing was spawned. The caller keeps
    /// waiting for it.
    Occupied(String),
}

/// Run by the election winner only.
fn prepare_broker_recovery_admission(
    descriptor_path: &Path,
    may_host: bool,
) -> qc_native_platform::owner_identity::RecoveryAdmission {
    use qc_native_platform::owner_identity::RecoveryAdmission;
    if !may_host {
        return RecoveryAdmission::none();
    }
    let Some(record) = device_broker_descriptor::read_descriptor(descriptor_path, 0) else {
        return RecoveryAdmission::none();
    };
    let helper = qc_native_platform::owner_identity::current_helper();
    RecoveryAdmission::prepare(
        record.process_id,
        record.process_created_at,
        helper.as_deref(),
    )
}

#[cfg(test)]
fn host_device_broker(descriptor_path: &Path, auto_connect: bool) -> Result<HostAttempt, String> {
    host_device_broker_with_admission(
        descriptor_path,
        auto_connect,
        &qc_native_platform::owner_identity::RecoveryAdmission::none(),
    )
}

fn host_device_broker_with_admission(
    descriptor_path: &Path,
    auto_connect: bool,
    admission: &qc_native_platform::owner_identity::RecoveryAdmission,
) -> Result<HostAttempt, String> {
    host_device_broker_assessed(
        descriptor_path,
        &|pid, created| admission.assess(pid, created),
        || start_device_broker(descriptor_path, auto_connect),
    )
}

/// The real hosting decision with injectable observations and spawn boundary.
/// Tests exercise retirement and start admission without launching a broker.
fn host_device_broker_assessed(
    descriptor_path: &Path,
    assess: &impl Fn(u32, Option<u64>) -> qc_native_platform::owner_identity::Assessment,
    start: impl FnOnce() -> Result<HostAttempt, String>,
) -> Result<HostAttempt, String> {
    #[cfg(all(test, windows))]
    require_native_test_state_root()?;
    // A broker may have been published between the failed attach and winning
    // the lock: the previous holder finished just before this one claimed it.
    // A descriptor that cannot be read is given one more chance after a pause,
    // because a live broker atomically republishes its descriptor at each
    // refresh and this process may have read the previous generation.
    //
    // Staleness is ignored here (`now_ms` 0): the descriptor is about to be
    // deleted, and only positive process-generation evidence justifies that,
    // never a refused connection or the age of the file.
    for pause in [Duration::ZERO, Duration::from_millis(100)] {
        std::thread::sleep(pause);
        match attach_to_published_device_broker_assessed(descriptor_path, 0, assess) {
            Some(Ok(attachment)) => return Ok(HostAttempt::Attached(attachment)),
            Some(Err(BrokerAttachFailure::Unresponsive(error))) => {
                return Ok(HostAttempt::Occupied(error));
            }
            Some(Err(BrokerAttachFailure::Gone(_))) | None => {}
        }
    }
    // Spawn admission: the marker may have appeared since the check before
    // the election. Refused before anything is deleted, reserved or spawned.
    if let Some(refusal) = hidden_resident_refusal(
        descriptor_path,
        session_marker_present(descriptor_path),
        device_broker_descriptor::unix_time_ms(),
        &broker_attach::published_broker_process_is_alive,
    ) {
        return Err(refusal);
    }
    // And a user's Quit recorded since then (`user_quit_refusal`), refused
    // the same way, before anything is deleted, reserved or spawned.
    if let Some(refusal) = user_quit_refusal(descriptor_path) {
        return Err(refusal);
    }
    // Whatever is still at the descriptor path names a broker proven dead, or
    // is not a descriptor at all. The new broker's own `create_new` claim
    // needs the path free.
    let expected = match crate::local_stdio::read_local_document(descriptor_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "Could not revalidate the broker descriptor: {error}"
            ))
        }
    };
    if let Some(bytes) = &expected {
        if let Ok(record) =
            serde_json::from_slice::<device_broker_descriptor::DeviceBrokerDescriptor>(bytes)
        {
            if assess(record.process_id, record.process_created_at)
                != qc_native_platform::owner_identity::Assessment::GoneOrDifferent
            {
                return Ok(HostAttempt::Occupied(
                    "Broker owner changed or is unverifiable; its descriptor was preserved".into(),
                ));
            }
        }
    }
    qc_owner_only_file::release_if_unclaimed_matching(descriptor_path, expected.as_deref())
        .map_err(|error| format!("Broker publication changed or remains owned: {error}"))?;

    start()
}

fn start_device_broker(descriptor_path: &Path, auto_connect: bool) -> Result<HostAttempt, String> {
    // The owner slot
    // for the broker about to be spawned is reserved first. If this domain
    // still holds a broker whose exit was never acknowledged, or every slot
    // is taken, nothing is spawned (`StartSlotGuard::reserve`).
    let slot = StartSlotGuard::reserve(descriptor_path)?;
    let mut command = native_broker_command()?;
    command
        .arg("--broker-serve")
        .arg("--descriptor-path")
        .arg(descriptor_path);
    // This process won the shared election before spawning. The broker must
    // not wait on the same lock while its parent waits for publication.
    #[cfg(unix)]
    command.arg("--host-election-held");
    add_broker_session_arguments(&mut command, auto_connect);
    command.stdin(Stdio::null()).stdout(Stdio::null());
    let child = spawn_with_broker_stderr(&mut command)
        .map_err(|error| format!("Could not start device gateway: {error}"))?;
    // The original `Child` moves into the
    // process-lifetime slot at once, before anything fallible, and is only
    // borrowed from there. A panic from here on leaves it in the slot.
    let mut held = slot.hold(child);
    let child = held
        .child
        .as_mut()
        .expect("the slot holds the child just spawned");
    #[cfg(all(test, windows))]
    register_spawned_broker(descriptor_path, child);
    #[cfg(all(test, windows))]
    panic_after_spawn_if_injected(descriptor_path);
    let attached = attach_to_spawned_device_broker(descriptor_path, child);
    #[cfg(all(test, windows))]
    let attached = injected_attach_outcome(descriptor_path, attached);
    match attached {
        Ok((stream, instance_id)) => {
            // Transferred to the normal owner, the gateway transport.
            let child = held.child.take().expect("the slot holds the child");
            drop(held);
            let broker_pid = child.id();
            Ok(HostAttempt::Attached((
                GatewayTransport::SharedBroker {
                    stream,
                    hosted_child: Some(child),
                    release_marker_path: generated_broker_surface::BrokerReleaseMarker::path_for(
                        descriptor_path,
                        &instance_id,
                    ),
                    instance_id,
                },
                broker_pid,
            )))
        }
        Err(SpawnedBrokerFailure::Unpublished(error)) => {
            // The one place a broker is ever killed by this host: it never
            // published, so no other client can have attached to it.
            // Bounded, borrowed, reported. The
            // child stays in its slot unless its exit is observed.
            #[cfg(all(test, windows))]
            let reaped = {
                let mut process = InjectedProcess::new(descriptor_path, child);
                reap_unpublished(&mut process, UNPUBLISHED_REAP_BOUND)
            };
            #[cfg(not(all(test, windows)))]
            let reaped = reap_unpublished(child, UNPUBLISHED_REAP_BOUND);
            let cleanup = reaped.describe(UNPUBLISHED_REAP_BOUND);
            if reaped.exited {
                held.child = None;
                Err(format!("{error}; cleanup: {cleanup}"))
            } else {
                let pid = held.child.as_ref().map_or(0, Child::id);
                held.why = Some(cleanup.clone());
                Err(format!(
                    "{error}; cleanup: {cleanup}; the broker (pid {pid}) is retained in its \
                     start slot and no new broker is started for this directory until its \
                     exit is observed"
                ))
            }
        }
        Err(SpawnedBrokerFailure::Published(failure)) => {
            // Published, so another window may already be attached: it is not
            // ours to kill any more. Retrying goes through the ordinary attach
            // path, which judges it like any other published broker.
            // Production unchanged: the `Child` leaves the slot and drops.
            // Test-only: a fixture that owns this scratch path takes it.
            let child = held.child.take().expect("the slot holds the child");
            drop(held);
            #[cfg(all(test, windows))]
            fixture_takes_published_child(descriptor_path, child);
            #[cfg(not(all(test, windows)))]
            drop(child);
            Ok(HostAttempt::Occupied(failure.message().to_owned()))
        }
    }
}

/// How long `host_device_broker` waits for a broker it killed because it
/// never published.
const UNPUBLISHED_REAP_BOUND: Duration = Duration::from_secs(5);

/// The process operations the unpublished-broker cleanup borrows (a
/// production seam). The cleanup only borrows: the `Child` is owned by its
/// start slot throughout, so a borrow can never lose it.
pub(crate) trait SpawnedProcess {
    fn kill(&mut self) -> std::io::Result<()>;
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl SpawnedProcess for Child {
    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Child::try_wait(self)
    }
}

/// What `reap_unpublished` saw.
pub(crate) struct UnpublishedReap {
    pub(crate) killed: Result<(), String>,
    /// Exited (`Ok(Some)`), still running (`Ok(None)`), or unknown (`Err`).
    pub(crate) status: Result<Option<std::process::ExitStatus>, String>,
    pub(crate) elapsed: Duration,
    pub(crate) exited: bool,
}

impl UnpublishedReap {
    /// The kill result, the status and the elapsed time against the bound.
    pub(crate) fn describe(&self, bound: Duration) -> String {
        let killed = match &self.killed {
            Ok(()) => "kill: ok".to_owned(),
            Err(error) => format!("kill failed: {error}"),
        };
        let status = match &self.status {
            Ok(Some(status)) => format!("exited ({status})"),
            Ok(None) => "still running".to_owned(),
            Err(error) => format!("exit status unknown: {error}"),
        };
        format!(
            "{killed}; {status} after {} ms of {} ms",
            self.elapsed.as_millis(),
            bound.as_millis()
        )
    }
}

/// Kills the borrowed process and polls `try_wait` for at most `bound`
/// (production). Never waits without bound and never takes ownership.
pub(crate) fn reap_unpublished<P: SpawnedProcess + ?Sized>(
    process: &mut P,
    bound: Duration,
) -> UnpublishedReap {
    let started = Instant::now();
    let killed = process.kill().map_err(|error| error.to_string());
    let status = loop {
        match process.try_wait() {
            Ok(Some(status)) => break Ok(Some(status)),
            Ok(None) if started.elapsed() < bound => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => break Ok(None),
            Err(error) => break Err(error.to_string()),
        }
    };
    let exited = matches!(status, Ok(Some(_)));
    UnpublishedReap {
        killed,
        status,
        elapsed: started.elapsed(),
        exited,
    }
}

/// What a start slot holds.
#[derive(Default)]
struct StartSlotContent {
    /// The original `Child`, from the instant after spawn.
    child: Option<Child>,
    /// Why it is retained, once a cleanup could not acknowledge its exit.
    why: Option<String>,
}

/// One reserved start slot: a descriptor-path domain and its content.
struct StartSlot {
    descriptor_path: PathBuf,
    content: std::sync::Arc<Mutex<StartSlotContent>>,
}

/// The process-lifetime owner of spawned brokers until they are handed on.
///
/// - Bound: at most `MAX_START_SLOTS` slots process-wide, and one per
///   descriptor-path domain (starts in a domain are serialized by the
///   election lock).
/// - Lifetime: a static; it holds a child through the return of
///   `host_device_broker`, through any unwind above it, through `Gateway`
///   retirement and drop, until the same handle observes the exit or this
///   process ends. Process end is the OS handle boundary, not a cleanup.
/// - No eviction, no replacement, no retry thread, no abandonment timeout: a
///   retained child leaves only when a later `reserve` observes its exit.
/// - Evidence: every retention and refusal is in the error returned to the
///   gateway, which records it in its runtime health record.
static START_SLOTS: Mutex<Vec<StartSlot>> = Mutex::new(Vec::new());

/// The start-slot bound.
const MAX_START_SLOTS: usize = 4;

fn lock_content(content: &Mutex<StartSlotContent>) -> std::sync::MutexGuard<'_, StartSlotContent> {
    content
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The reservation `host_device_broker` holds while it starts a broker. If
/// it is dropped with no child in the slot (spawn failed, or the child was
/// handed on), the slot is released; with a child in it (a retained
/// unpublished broker, or an unwind), the slot keeps it.
struct StartSlotGuard {
    content: std::sync::Arc<Mutex<StartSlotContent>>,
}

impl StartSlotGuard {
    /// Before spawning: one bounded observation (a single `try_wait`, never
    /// blocking) of every retained child, releasing those that exited; then
    /// a slot for `descriptor_path`, or a refusal naming what is retained.
    fn reserve(descriptor_path: &Path) -> Result<Self, String> {
        let mut slots = START_SLOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots.retain(|slot| {
            let content = match slot.content.try_lock() {
                Ok(content) => content,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                // A start in progress holds it.
                Err(std::sync::TryLockError::WouldBlock) => return true,
            };
            let mut content = content;
            match content.child.as_mut().map(Child::try_wait) {
                Some(Ok(Some(_))) => {
                    content.child = None;
                    false
                }
                // Reserved and still empty: a start is about to fill it.
                None => true,
                Some(_) => true,
            }
        });
        let refusal = |slot: &StartSlot| {
            let content = match slot.content.try_lock() {
                Ok(content) => Some(content),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => None,
            };
            match content {
                Some(content) => format!(
                    "{} (pid {}: {})",
                    slot.descriptor_path.display(),
                    content.child.as_ref().map_or(0, Child::id),
                    content
                        .why
                        .as_deref()
                        .unwrap_or("held since a start that did not finish")
                ),
                None => format!("{} (a start in progress)", slot.descriptor_path.display()),
            }
        };
        if let Some(slot) = slots
            .iter()
            .find(|slot| slot.descriptor_path == descriptor_path)
        {
            return Err(format!(
                "Not starting another device broker: an earlier one is still held and not \
                 acknowledged exited: {}",
                refusal(slot)
            ));
        }
        if slots.len() >= MAX_START_SLOTS {
            let held: Vec<String> = slots.iter().map(refusal).collect();
            return Err(format!(
                "Not starting another device broker: all {MAX_START_SLOTS} start slots are \
                 held: {}",
                held.join("; ")
            ));
        }
        let content = std::sync::Arc::new(Mutex::new(StartSlotContent::default()));
        slots.push(StartSlot {
            descriptor_path: descriptor_path.to_path_buf(),
            content: content.clone(),
        });
        Ok(Self { content })
    }

    /// Moves the just-spawned child into the slot and borrows the slot.
    fn hold(&self, child: Child) -> std::sync::MutexGuard<'_, StartSlotContent> {
        let mut content = lock_content(&self.content);
        content.child = Some(child);
        content
    }
}

impl Drop for StartSlotGuard {
    fn drop(&mut self) {
        if lock_content(&self.content).child.is_some() {
            // Owned by the slot from now on, through any unwind.
            return;
        }
        START_SLOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|slot| !std::sync::Arc::ptr_eq(&slot.content, &self.content));
    }
}

/// Test-only: the start slot of `descriptor_path` - the pid of the child it
/// holds, if any, and why - without changing it.
#[cfg(all(test, windows))]
pub(crate) fn start_slot_state(descriptor_path: &Path) -> Option<(Option<u32>, Option<String>)> {
    START_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|slot| slot.descriptor_path == descriptor_path)
        .map(|slot| {
            let content = lock_content(&slot.content);
            (content.child.as_ref().map(Child::id), content.why.clone())
        })
}

/// Test-only: hands the fixture that owns `descriptor_path` the original
/// child its start slot holds, and releases the slot.
#[cfg(all(test, windows))]
pub(crate) fn take_start_slot_child(descriptor_path: &Path) -> Option<Child> {
    let mut slots = START_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = slots
        .iter()
        .position(|slot| slot.descriptor_path == descriptor_path)?;
    let child = lock_content(&slots[index].content).child.take();
    slots.remove(index);
    child
}

/// Test-only: occupy a start slot with a child the test spawned (capacity
/// tests), and try a reservation without spawning anything.
#[cfg(all(test, windows))]
pub(crate) fn occupy_start_slot_for_test(
    descriptor_path: &Path,
    child: Child,
) -> Result<(), (String, Child)> {
    // A refused reservation hands the child back, so
    // the caller still owns it.
    let slot = match StartSlotGuard::reserve(descriptor_path) {
        Ok(slot) => slot,
        Err(refusal) => return Err((refusal, child)),
    };
    drop(slot.hold(child));
    Ok(())
}

/// Test-only: a duplicate of the handle of the child the start
/// slot of `descriptor_path` holds, made after the slot owns it.
#[cfg(all(test, windows))]
pub(crate) fn duplicate_start_slot_child(
    descriptor_path: &Path,
) -> Option<std::io::Result<std::os::windows::io::OwnedHandle>> {
    use std::os::windows::io::AsHandle as _;
    START_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|slot| slot.descriptor_path == descriptor_path)
        .and_then(|slot| {
            lock_content(&slot.content)
                .child
                .as_ref()
                .map(|child| child.as_handle().try_clone_to_owned())
        })
}

#[cfg(all(test, windows))]
pub(crate) fn reserve_start_slot_for_test(descriptor_path: &Path) -> Result<(), String> {
    StartSlotGuard::reserve(descriptor_path).map(drop)
}

/// Test-only: a borrowed child whose first kill and/or every `try_wait`
/// fail, as injected for `descriptor_path`, so the real `reap_unpublished`
/// and the real retention run.
#[cfg(all(test, windows))]
struct InjectedProcess<'a> {
    child: &'a mut Child,
    fail_kill: bool,
    fail_wait: bool,
}

#[cfg(all(test, windows))]
impl<'a> InjectedProcess<'a> {
    fn new(descriptor_path: &Path, child: &'a mut Child) -> Self {
        let injection = spawn_injection(descriptor_path).unwrap_or_default();
        Self {
            child,
            fail_kill: injection.fail_kill,
            fail_wait: injection.fail_wait,
        }
    }
}

#[cfg(all(test, windows))]
impl SpawnedProcess for InjectedProcess<'_> {
    fn kill(&mut self) -> std::io::Result<()> {
        if self.fail_kill {
            Err(std::io::Error::other("injected kill failure"))
        } else {
            self.child.kill()
        }
    }
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        if self.fail_wait {
            Err(std::io::Error::other("injected wait failure"))
        } else {
            self.child.try_wait()
        }
    }
}

/// Test-only: panic right after the spawned child is in its slot,
/// before the attach, when injected.
#[cfg(all(test, windows))]
fn panic_after_spawn_if_injected(descriptor_path: &Path) {
    if spawn_injection(descriptor_path).is_some_and(|injection| injection.panic_after_spawn) {
        panic!("injected panic right after the broker was spawned");
    }
}

enum SpawnedBrokerFailure {
    /// It exited, or never published within the publish timeout.
    Unpublished(String),
    /// It published, and then the attach to it failed.
    Published(BrokerAttachFailure),
}

fn attach_to_spawned_device_broker(
    descriptor_path: &Path,
    child: &mut Child,
) -> Result<(TcpStream, String), SpawnedBrokerFailure> {
    let deadline = Instant::now() + DEVICE_BROKER_PUBLISH_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| SpawnedBrokerFailure::Unpublished(error.to_string()))?
        {
            return Err(SpawnedBrokerFailure::Unpublished(format!(
                "The QC device broker exited before publishing its connection ({status})"
            )));
        }
        if let Some(descriptor) = device_broker_descriptor::read_descriptor(
            descriptor_path,
            device_broker_descriptor::unix_time_ms(),
        ) {
            if descriptor.process_id == child.id() {
                return attach_to_device_broker(&descriptor)
                    .map(|stream| (stream, descriptor.instance_id))
                    .map_err(SpawnedBrokerFailure::Published);
            }
        }
        if Instant::now() >= deadline {
            #[cfg(test)]
            if let Ok(stderr) = fs::read(broker_stderr_path()) {
                let tail = stderr.iter().rev().take(2_000).copied().collect::<Vec<_>>();
                let tail = tail.into_iter().rev().collect::<Vec<_>>();
                eprintln!("broker stderr tail: {}", String::from_utf8_lossy(&tail));
            }
            return Err(SpawnedBrokerFailure::Unpublished(format!(
                "The QC device broker did not publish its connection within {} seconds",
                DEVICE_BROKER_PUBLISH_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(DEVICE_BROKER_POLL_INTERVAL);
    }
}

/// The string the shipped `qc-device-broker.exe` embeds
/// (`RESIDENT_HOST_BUILD_MARKER` in src/bin/qc-device-broker.rs; the release
/// checks read the same one). Both come from this crate, so they share
/// `CARGO_PKG_VERSION`.
fn resident_host_build_marker() -> String {
    format!(
        "qc-remote-resident-host-build:v1:{}:end",
        env!("CARGO_PKG_VERSION")
    )
}

/// Whether the file at `path` embeds this version's build marker. Reads the
/// file; never runs it.
fn embeds_resident_host_marker(path: &Path) -> bool {
    let marker = resident_host_build_marker();
    std::fs::File::open(path)
        .and_then(|mut file| reader_contains_resident_marker(&mut file, marker.as_bytes()))
        .unwrap_or(false)
}

fn reader_contains_resident_marker(reader: &mut impl Read, marker: &[u8]) -> std::io::Result<bool> {
    if marker.is_empty() {
        return Ok(true);
    }
    // A binary has no JSON/document size limit. Keep only the fixed read
    // window plus a marker-length overlap so split markers remain visible.
    let mut bytes = vec![0u8; 8192 + marker.len() - 1];
    let mut overlap = 0;
    loop {
        let read = match reader.read(&mut bytes[overlap..overlap + 8192]) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(false);
        }
        let available = overlap + read;
        if bytes[..available]
            .windows(marker.len())
            .any(|window| window == marker)
        {
            return Ok(true);
        }
        overlap = available.min(marker.len() - 1);
        bytes.copy_within(available - overlap..available, 0);
    }
}

/// The broker to spawn: the sidecar installed beside this executable, and in
/// a release build nothing else. A missing sidecar fails closed rather than
/// running some other `qc-device-broker.exe` found by name.
///
/// A debug build may also use a checkout's build output, but only one that
/// embeds this version's resident-host marker, read from the file before
/// anything runs it: a broker-only developer build or a stale one of the same
/// name (which would treat unknown arguments as a live device probe) is never
/// started.
// Tests build their own broker instead; see `default_broker_executable`.
#[cfg_attr(test, allow(dead_code))]
fn locate_native_broker(executable_directory: Option<&Path>) -> Option<PathBuf> {
    // Where the dev build script and the installer build put the app-host
    // broker: `<target>/app-host-broker`, under the target directory this
    // build used, else this crate's own.
    let target = option_env!("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let fallbacks = ["debug", "release"].map(|profile| {
        target
            .join("app-host-broker")
            .join(profile)
            .join(native_broker_filename())
    });
    locate_native_broker_in(executable_directory, &fallbacks, cfg!(debug_assertions))
}

fn native_broker_filename() -> &'static str {
    if cfg!(windows) {
        "qc-device-broker.exe"
    } else {
        "qc-device-broker"
    }
}

fn native_broker_sidecar_names() -> Vec<String> {
    let base = native_broker_filename();
    let mut names = vec![base.to_owned()];
    if cfg!(windows) {
        names.extend([
            "qc-device-broker-x86_64-pc-windows-msvc.exe".to_owned(),
            "qc-device-broker-x86_64-pc-windows-gnu.exe".to_owned(),
        ]);
    } else if let Some(target) = native_linux_target_triple() {
        names.push(format!("qc-device-broker-{target}"));
    } else if let Some(target) = native_macos_target_triple() {
        names.push(format!("qc-device-broker-{target}"));
    }
    names
}

fn native_linux_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    return Some("x86_64-unknown-linux-gnu");
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
    return Some("x86_64-unknown-linux-musl");
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    return Some("aarch64-unknown-linux-gnu");
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
    return Some("aarch64-unknown-linux-musl");
    #[allow(unreachable_code)]
    None
}

fn native_macos_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some("x86_64-apple-darwin");
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some("aarch64-apple-darwin");
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod native_broker_filename_tests {
    use super::*;

    #[test]
    fn broker_sidecar_names_match_the_current_tauri_target() {
        let names = native_broker_sidecar_names();
        if cfg!(windows) {
            assert_eq!(native_broker_filename(), "qc-device-broker.exe");
            assert!(names.contains(&"qc-device-broker-x86_64-pc-windows-msvc.exe".into()));
        } else {
            assert_eq!(native_broker_filename(), "qc-device-broker");
            #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
            assert!(names.contains(&"qc-device-broker-x86_64-unknown-linux-gnu".into()));
            #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
            assert!(names.contains(&"qc-device-broker-aarch64-unknown-linux-gnu".into()));
            #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
            assert!(names.contains(&"qc-device-broker-x86_64-apple-darwin".into()));
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            assert!(names.contains(&"qc-device-broker-aarch64-apple-darwin".into()));
        }
    }
}

fn locate_native_broker_in(
    executable_directory: Option<&Path>,
    development_fallbacks: &[PathBuf],
    allow_development_fallback: bool,
) -> Option<PathBuf> {
    if let Some(directory) = executable_directory {
        let installed = native_broker_sidecar_names()
            .iter()
            .map(|name| directory.join(name))
            .find(|path| path.is_file());
        if installed.is_some() {
            return installed;
        }
    }
    if !allow_development_fallback {
        return None;
    }
    development_fallbacks
        .iter()
        .find(|path| path.is_file() && embeds_resident_host_marker(path))
        .cloned()
}

/// Authoritative session health carried by `usbDiagnostics`.
///
/// `Gateway::connected` below is a host-side latch over `device.*` RPC
/// outcomes and nothing else: `system.status` never writes to it. A device that
/// answers calls while remaining in `phase: "syncing"` with
/// `synchronized: false` must not be reported as healthy. These fields make a
/// session stuck during startup visible to the health record.
#[derive(Default)]
struct DeviceSession {
    phase: Option<String>,
    synchronized: Option<bool>,
    /// The device's own answer to "is there a USB session", as distinct from
    /// [`Gateway::connected`], which is what the host last inferred from an RPC
    /// outcome. Every `system.status` reply carries it.
    connected: Option<bool>,
    /// Set once a mutation reached the wire but its outcome could not be
    /// established; only `system.status` (via `controller.status()`) carries
    /// this today. Persisted so the health record - and the frontend's own
    /// poll of the same field - can show it instead of a window sitting on a
    /// stale "connected" read until a mutation happens to be attempted and
    /// gets the dedicated -32012 error back.
    writes_quarantined: Option<bool>,
    write_quarantine_reason: Option<String>,
    /// Without this, a phase is just another latch: a reader cannot tell a
    /// session that is ready now from one that was ready ten minutes ago.
    observed_at_unix: Option<u64>,
}

#[derive(Default)]
struct Gateway {
    /// Scoped to a UI invoke under the gateway mutex, never a global default:
    /// this same Gateway also serves MCP requests for the effective target.
    physical_only: bool,
    expected_broker_target: Option<Value>,
    selected_device_id: Option<String>,
    process: Option<GatewayProcess>,
    /// A failed writer may race a delayed notification reader. Keep that
    /// generation and its flag until the reader acknowledges completion.
    retiring_process: Option<GatewayProcess>,
    /// True once a broker has been spawned, so a later spawn is known to be
    /// replacing one rather than starting the session for the first time.
    started_once: bool,
    connected: bool,
    device: DeviceSession,
    event_tx: Option<mpsc::SyncSender<DeviceEvent>>,
    /// Where this gateway meets the shared broker. `None` defers to
    /// `DEVICE_BROKER_DESCRIPTOR_PATH`, which Tauri setup resolves for a
    /// window. A headless `--local-mcp-bridge` process never runs that setup,
    /// so it names the path here instead - the same file every window uses.
    descriptor_path: Option<PathBuf>,
    /// Whether the very first broker this gateway spawns opens the device
    /// session on its own. A window leaves it off: its mount-time connect asks
    /// for the session. A headless MCP host has no such step, so without this
    /// a broker it spawned would answer every call "not connected".
    auto_connect_first_broker: bool,
    /// Survives dropping the failed transport after a deliberate tray release.
    /// Explicit reconnect clears it, and so does a newer broker generation
    /// published after the released one, once attached (`attach_published_candidate`); ordinary
    /// crash recovery is unchanged.
    intentional_release: bool,
    /// The broker instance the standing release belongs to, when known. A
    /// release binds that generation only, never a broker started after it.
    released_instance_id: Option<String>,
    /// Set when a window gateway attached to a newer broker while a release
    /// stood (`attach_published_candidate`): until its own reconnect, only
    /// `forwarded_before_reconnect` requests reach that broker.
    reconnect_required: bool,
    /// The last candidate attach that failed (`attach_published_candidate`).
    failed_attach: Option<FailedAttach>,
    /// Attaches `attach_published_candidate` attempted, for tests.
    #[cfg(test)]
    supersede_attach_attempts: usize,
    participant_provider: Option<&'static str>,
    /// Set only for the background MCP host inside `qc-device-broker.exe`:
    /// its broker's per-process host token, presented in `broker.hello`.
    participant_host_token: Option<String>,
    announcement_settings: AnnouncementSettingsCache,
    /// This gateway's connection, for lock-free reads that find it busy.
    /// Kept current wherever `process` changes; see `request_gateway`.
    lock_free_reads: LockFreeReadSlot,
    /// A test's stand-in for `GatewayProcess::start`, owned by this gateway
    /// alone (`Gateway::count_starts_refusing`).
    #[cfg(test)]
    start_seam: Option<GatewayInstanceStarter>,
}

/// The broker instance a connection was attached to, if it is a shared one.
fn gateway_process_instance_id(process: &GatewayProcess) -> Option<String> {
    match &process.transport {
        GatewayTransport::SharedBroker { instance_id, .. } => Some(instance_id.clone()),
        GatewayTransport::OwnedChild { .. } => None,
    }
}

/// Whether a published broker may be attached to without an election or a
/// spawn: it is live, it is not `excluded_instance_id` (the released
/// generation, when there is one), and nothing excludes it (`excluded`: its
/// own forced-release marker, or a user Quit record naming it or unreadable,
/// `user_quit_excludes`). This only selects a candidate: an attach to exactly
/// this descriptor must still succeed (`Gateway::attach_published_candidate`).
fn broker_attach_candidate(
    excluded_instance_id: Option<&str>,
    published: Option<&device_broker_descriptor::DeviceBrokerDescriptor>,
    excluded: bool,
    alive: &dyn Fn(&device_broker_descriptor::DeviceBrokerDescriptor) -> bool,
) -> bool {
    published.is_some_and(|descriptor| {
        excluded_instance_id != Some(descriptor.instance_id.as_str())
            && !excluded
            && alive(descriptor)
    })
}

/// Whether the user Quit record excludes this broker instance: it names it,
/// or it cannot be read (an unreadable record names no generation, so it
/// excludes every one), the same predicate `user_quit_refusal_at` applies.
/// A Quit in the short gap before its shutdown commits is never attached.
fn user_quit_excludes(descriptor_path: &Path, instance_id: &str) -> bool {
    match device_broker_descriptor::read_user_quit(descriptor_path) {
        None => false,
        Some(Ok(record)) => record.instance_id == instance_id,
        Some(Err(_)) => true,
    }
}

/// How long a candidate whose attach failed is left alone, unless the
/// descriptor names another instance or process sooner.
const SUPERSEDE_ATTACH_RETRY_AFTER: Duration = Duration::from_secs(30);
/// The whole of one candidate attach, version check and device selection
/// included. It runs under the gateway lock.
const SUPERSEDE_ATTACH_DEADLINE: Duration = Duration::from_secs(5);

/// A candidate attach that failed, so the probes do not repeat it.
struct FailedAttach {
    instance_id: String,
    process_id: u32,
    at: Instant,
}

/// The reads a window gateway may use, while a release stands, to find and
/// attach to a newer broker generation: its status heartbeat and its
/// inventory poll. Nothing else ever triggers that attach.
fn supersede_probe_method(method: &str) -> bool {
    matches!(method, rpc::RUNTIME_STATUS | "broker.devices")
}

/// What a window gateway forwards to a broker it attached to after a release
/// (`Gateway::reconnect_required`) before its own reconnect: the probes, the
/// reconnect itself, a device selection made by the user, and a disconnect.
/// Every other request, any `device.*` mutation included, is refused with the
/// release's -32002, so nothing but that reconnect can reopen the device
/// session the release closed.
fn forwarded_before_reconnect(method: &str) -> bool {
    supersede_probe_method(method)
        || matches!(
            method,
            rpc::RECONNECT
                | rpc::DISCONNECT
                | DEVICE_BROKER_DISCONNECT_METHOD
                | "broker.selectDevice"
        )
}

fn released_from_tray() -> GatewayRequestFailure {
    GatewayRequestFailure::Remote {
        code: -32002,
        message: "The device was released from the tray. Connect explicitly to use it again."
            .into(),
        retryable: false,
        details: None,
    }
}

#[derive(Default)]
struct AnnouncementSettingsCache {
    last_read: Option<Instant>,
    revision: u64,
    enabled: bool,
}
impl AnnouncementSettingsCache {
    fn should_announce(
        &mut self,
        announced: bool,
        revision: u64,
        now: Instant,
        read: impl FnOnce() -> bool,
    ) -> bool {
        if announced {
            return false;
        }
        if self.revision != revision
            || self
                .last_read
                .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(1))
        {
            self.enabled = read();
            self.revision = revision;
            self.last_read = Some(now);
        }
        self.enabled
    }
}

impl Gateway {
    /// A busy gateway's callers reach its connection through this slot, so
    /// it must name the current process, or none.
    fn publish_lock_free_reads(&self) {
        // While `reconnect_required` stands no read bypasses the gate: a busy
        // gateway's callers then wait for the lock (or are refused as busy)
        // and meet `forwarded_before_reconnect` like any other request. The
        // next request after the reconnect republishes the slot.
        self.lock_free_reads.publish(
            self.process
                .as_ref()
                .filter(|_| !self.reconnect_required)
                .and_then(|process| process.lock_free_reads.clone()),
        );
    }

    /// The slot a host keeps outside the gateway mutex.
    pub(crate) fn lock_free_read_slot(&self) -> LockFreeReadSlot {
        self.lock_free_reads.clone()
    }

    fn retire_current_process(&mut self) {
        if let Some(mut process) = self.process.take() {
            match &mut process.transport {
                // FIN closes this client's request stream without discarding
                // unread release notifications. The broker then closes its
                // response stream once any current request has finished.
                GatewayTransport::SharedBroker { stream, .. } => {
                    let _ = stream.shutdown(Shutdown::Write);
                }
                GatewayTransport::OwnedChild { child, .. } => {
                    let _ = child.kill();
                }
            }
            self.retiring_process = Some(process);
        }
        self.publish_lock_free_reads();
    }

    fn respect_intentional_release(&mut self, method: &str) -> Result<bool, GatewayRequestFailure> {
        if let Some(process) = self.retiring_process.take() {
            if method == rpc::RECONNECT {
                // This user request explicitly supersedes the failed
                // generation. Its reader owns a different Arc and cannot
                // contaminate the new connection's release state.
                drop(process);
                self.intentional_release = false;
                self.released_instance_id = None;
                return Ok(true);
            }
            let completed = match process
                .reader_completed
                // The broker joins its phase/state push threads after FIN;
                // their ordinary poll intervals can exceed 100 milliseconds.
                .recv_timeout(Duration::from_secs(1))
            {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                Err(mpsc::RecvTimeoutError::Timeout) => false,
            };
            self.intentional_release |= process
                .intentional_release
                .load(std::sync::atomic::Ordering::Acquire);
            // The notification broadcast is deliberately bounded. A stalled
            // writer may miss it, so inspect the durable marker exactly at
            // failed-generation recovery, never on ordinary hot RPCs.
            self.intentional_release |= process.has_durable_forced_release();
            if self.intentional_release && self.released_instance_id.is_none() {
                self.released_instance_id = gateway_process_instance_id(&process);
            }
            if !completed && !self.intentional_release {
                self.retiring_process = Some(process);
                return Err(GatewayRequestFailure::Transport(
                    "The previous broker connection is still draining; automatic recovery is waiting for its final notifications.".into(),
                ));
            }
        }
        if let Some(process) = self.process.as_ref().filter(|process| {
            process
                .intentional_release
                .load(std::sync::atomic::Ordering::Acquire)
        }) {
            self.intentional_release = true;
            if self.released_instance_id.is_none() {
                self.released_instance_id = gateway_process_instance_id(process);
            }
        }
        if !self.intentional_release {
            return Ok(false);
        }
        self.process = None;
        self.publish_lock_free_reads();
        self.connected = false;
        self.device = DeviceSession::default();
        publish_gateway_readiness(false);
        if method == rpc::RECONNECT {
            self.intentional_release = false;
            self.released_instance_id = None;
            return Ok(true);
        }
        // The release bound the generation it released. A window whose status
        // or inventory probe finds a newer live broker published (started by
        // the user, or by a client's own demand when no Quit stands) attaches
        // to exactly that broker so it can mirror what it reports. Only the
        // window's own gateway does this: a headless bridge stays released
        // until an explicit reconnect. The release is cleared only once that
        // attach has succeeded; on any failure it stands. The attached broker
        // then receives only the probe that found it, and later only
        // `forwarded_before_reconnect` requests, until the window reconnects.
        if self.participant_provider.is_none() && supersede_probe_method(method) {
            let released = self.released_instance_id.clone();
            if let Some(process) = released
                .as_deref()
                .and_then(|released| self.attach_published_candidate(Some(released)))
            {
                self.process = Some(process);
                self.intentional_release = false;
                self.released_instance_id = None;
                self.reconnect_required = true;
                self.publish_lock_free_reads();
                return Ok(true);
            }
        }
        Err(released_from_tray())
    }

    /// One attach to the published broker, never an election or a spawn, and
    /// only when `broker_attach_candidate` accepts it. The connection is
    /// proven to be that very instance (`attach_to_device_broker` requires the
    /// authenticated answer to carry the descriptor's instance id, and the
    /// transport is checked again here), and a selected device is re-selected
    /// on it without asking the broker to open the session. The whole attach
    /// is bounded by `SUPERSEDE_ATTACH_DEADLINE`, and a candidate that failed
    /// is not tried again for `SUPERSEDE_ATTACH_RETRY_AFTER` unless the
    /// descriptor names another instance or process. `None` on any failure;
    /// the caller then keeps the release.
    fn attach_published_candidate(
        &mut self,
        excluded_instance_id: Option<&str>,
    ) -> Option<GatewayProcess> {
        let descriptor_path = self.shared_descriptor_path()?.to_path_buf();
        let descriptor = device_broker_descriptor::read_descriptor(
            &descriptor_path,
            device_broker_descriptor::unix_time_ms(),
        )?;
        if self.failed_attach.as_ref().is_some_and(|failed| {
            failed.instance_id == descriptor.instance_id
                && failed.process_id == descriptor.process_id
                && failed.at.elapsed() < SUPERSEDE_ATTACH_RETRY_AFTER
        }) {
            return None;
        }
        let marker = generated_broker_surface::BrokerReleaseMarker::path_for(
            &descriptor_path,
            &descriptor.instance_id,
        );
        let excluded = read_forced_release_marker(&marker, &descriptor.instance_id)
            || user_quit_excludes(&descriptor_path, &descriptor.instance_id);
        if !broker_attach_candidate(
            excluded_instance_id,
            Some(&descriptor),
            excluded,
            &broker_attach::published_broker_process_is_alive,
        ) {
            return None;
        }
        #[cfg(test)]
        {
            self.supersede_attach_attempts += 1;
        }
        let deadline = Instant::now() + SUPERSEDE_ATTACH_DEADLINE;
        let attached =
            GatewayProcess::attach_exact(self.event_tx.clone(), &descriptor, marker, deadline)
                .ok()
                .filter(|process| {
                    gateway_process_instance_id(process).as_deref()
                        == Some(descriptor.instance_id.as_str())
                })
                .and_then(|mut process| match &self.selected_device_id {
                    None => Some(process),
                    Some(device_id) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        (!remaining.is_zero()
                            && process
                                .request_as_within(
                                    "broker.selectDevice",
                                    json!({"deviceId": device_id, "autoConnect": false}),
                                    None,
                                    remaining,
                                )
                                .is_ok())
                        .then_some(process)
                    }
                });
        self.failed_attach = if attached.is_some() {
            None
        } else {
            Some(FailedAttach {
                instance_id: descriptor.instance_id.clone(),
                process_id: descriptor.process_id,
                at: Instant::now(),
            })
        };
        attached
    }

    /// Copy the device truth out of any reply that carries `usbDiagnostics`.
    /// Keyed on the payload rather than on the method name so every reply that
    /// reports the session - `system.status`, `device.reconnect`,
    /// `device.resetSession` - refreshes it.
    fn observe_device_session(&mut self, value: &Value) {
        let Some(diagnostics) = value.get("usbDiagnostics") else {
            return;
        };
        let phase = diagnostics
            .get("phase")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let synchronized = diagnostics.get("synchronized").and_then(Value::as_bool);
        let connected = diagnostics.get("connected").and_then(Value::as_bool);
        let writes_quarantined = diagnostics
            .get("writesQuarantined")
            .and_then(Value::as_bool);
        let write_quarantine_reason = diagnostics
            .get("writeQuarantineReason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if phase.is_none() && synchronized.is_none() && connected.is_none() {
            return;
        }
        self.device = DeviceSession {
            phase,
            synchronized,
            connected,
            writes_quarantined,
            write_quarantine_reason,
            observed_at_unix: Some(unix_seconds()),
        };
        // The device's own account of the session outranks the host-side latch.
        //
        // `connected` used to move only when a `device.`-prefixed method
        // succeeded or failed, so with only the 1 Hz `system.status` poll
        // running - the app's idle state - unplugging the Quad Cortex changed
        // nothing: `/health` kept answering 200 with `deviceReady: true`, and
        // the relay kept publishing `usb_connected: true`, until something
        // happened to call a `device.*` RPC. The poll was carrying the answer
        // in its payload the whole time.
        if let Some(connected) = connected {
            self.connected = connected;
        }
    }

    /// A session that is gone cannot still be "ready". Dropping the observed
    /// truth with the latch keeps the record from reporting a phase belonging
    /// to a session that no longer exists.
    ///
    /// The exception is a disconnection the device itself just reported: that
    /// observation *is* the current truth, and discarding it would throw away
    /// the phase and detail explaining what happened.
    fn forget_device_session_if_disconnected(&mut self) {
        if !self.connected && self.device.connected != Some(false) {
            self.device = DeviceSession::default();
        }
    }

    /// The descriptor path `GatewayProcess::start` is given.
    fn shared_descriptor_path(&self) -> Option<&Path> {
        self.descriptor_path
            .as_deref()
            .or_else(|| DEVICE_BROKER_DESCRIPTOR_PATH.get().map(PathBuf::as_path))
    }

    /// `GatewayProcess::start` for this gateway: the one call that attaches
    /// to, elects, spawns or privately starts a broker. A test may replace it
    /// for this gateway alone (`start_seam`).
    fn start_gateway_process(&self, auto_connect: bool) -> Result<GatewayProcess, String> {
        #[cfg(test)]
        if let Some(starter) = &self.start_seam {
            return starter(auto_connect, self.shared_descriptor_path());
        }
        GatewayProcess::start(
            self.event_tx.clone(),
            auto_connect,
            self.shared_descriptor_path(),
        )
    }

    /// Whether a broker spawned now opens the device session by itself.
    fn broker_auto_connect(&self) -> bool {
        self.started_once || self.auto_connect_first_broker
    }

    /// `device.disconnect` on a broker other clients are still using.
    ///
    /// The broker owns one USB session for every process attached to it, so
    /// forwarding the call would end the session under every other window and
    /// every MCP client too - a window switching to the demo device would
    /// drop them all. So a disconnect only reaches the broker from its sole
    /// client. Anyone else is disconnected on their own side and told the
    /// session stays open; the broker closes it once its last client leaves.
    ///
    /// `device.resetSession` is deliberately not gated: it is the recovery
    /// for a wedged session, and a wedged session is wedged for every client.
    fn leave_shared_session_open(&mut self) -> Option<Value> {
        let others_attached = self
            .process
            .as_mut()?
            .shared_broker_client_count()
            .is_some_and(|clients| clients > 1);
        if !others_attached {
            return None;
        }
        self.connected = false;
        self.device = DeviceSession::default();
        publish_gateway_readiness(false);
        write_runtime_health(
            self,
            rpc::DISCONNECT,
            "ok",
            Some("shared session left open"),
        );
        Some(json!({
            "phase": "disconnected",
            "detail": SHARED_SESSION_LEFT_OPEN_DETAIL,
            "lastSync": null,
            "demo": false
        }))
    }

    fn with_physical_target<T>(&mut self, request: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.physical_only;
        self.physical_only = true;
        let result = request(self);
        self.physical_only = previous;
        result
    }

    fn request_targeted_as(
        &mut self,
        method: &str,
        params: Value,
        connection: ConnectionUse,
        expected: Option<Value>,
    ) -> Result<Value, GatewayRequestFailure> {
        let previous = std::mem::replace(&mut self.expected_broker_target, expected);
        let result = self.request_detailed_as(method, params, connection);
        self.expected_broker_target = previous;
        result
    }

    fn request_detailed(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, GatewayRequestFailure> {
        self.request_detailed_as(method, params, ConnectionUse::MayConnect)
    }

    /// `request_detailed` for a request of the given origin. The caller holds
    /// the gateway lock, so whether an `ExistingOnly` request may proceed is
    /// decided here against the connection as it is now, never as an earlier
    /// look found it.
    fn request_detailed_as(
        &mut self,
        method: &str,
        params: Value,
        connection: ConnectionUse,
    ) -> Result<Value, GatewayRequestFailure> {
        let existing_only = connection == ConnectionUse::ExistingOnly;
        #[cfg(all(test, windows))]
        require_native_test_state_root().map_err(GatewayRequestFailure::Transport)?;
        // Before `respect_intentional_release`, which settles a failed
        // generation: that is recovery, and not a resource read's to start.
        if existing_only && self.process.is_none() {
            return Err(existing_connection_unavailable());
        }
        // A broker this window attached to while a release stood
        // (`attach_published_candidate`) gets nothing that could reopen the
        // released session until the window's own reconnect or reset.
        if self.reconnect_required {
            if matches!(method, rpc::RECONNECT | rpc::RESET_SESSION) {
                self.reconnect_required = false;
            } else if !forwarded_before_reconnect(method) {
                return Err(if existing_only {
                    existing_connection_unavailable()
                } else {
                    released_from_tray()
                });
            }
        }
        // While the gate stands, any recovery attaches the way a reconnect
        // after a release does: it never asks a broker to open the session.
        let reconnect_after_release = match self.respect_intentional_release(method) {
            Ok(reconnect) => reconnect || self.reconnect_required,
            Err(_) if existing_only => return Err(existing_connection_unavailable()),
            Err(failure) => return Err(failure),
        };
        if self.process.is_none() {
            // The one place a gateway attaches to, elects, spawns or replaces
            // a broker.
            if existing_only {
                return Err(existing_connection_unavailable());
            }
            // Anything after the first start follows a transport failure or a
            // restart. With a shared broker that usually means re-attaching to
            // the same, still-running broker, which keeps whatever session it
            // has and ignores this flag. It matters only when the broker really
            // is gone and this call spawns its replacement. The caller that
            // triggers it is an
            // ordinary device poll, not a connection step, so nothing would
            // ever ask the replacement to open a session: it would answer every
            // device call with "not connected" while the UI showed the device
            // offline and the broker stayed perfectly healthy. Reconnecting is
            // left to the replacement itself because device.reconnect blocks
            // for up to the ready-wait timeout and must not stall this request.
            let auto_connect = self.selected_device_id.is_none()
                && !reconnect_after_release
                && method != DEVICE_BROKER_DISCONNECT_METHOD
                && self.broker_auto_connect();
            let mut reselected = false;
            if self.reconnect_required {
                // The broker this window attached to after a release went
                // away before the window's own reconnect. Recovery is the
                // same bounded attach to a published broker, never an
                // election or a spawn, so nothing here can start a broker
                // that opens the released device session.
                //
                // Nothing to attach answers with the release itself, never
                // a transport failure: the window must not read this as an
                // ordinary lost session and issue the automatic reconnect
                // that would lift the gate.
                let attached = self
                    .attach_published_candidate(None)
                    .ok_or_else(released_from_tray)?;
                self.process = Some(attached);
                // `attach_published_candidate` already re-selected the
                // device, within its own deadline.
                reselected = true;
            } else {
                self.process = Some(
                    self.start_gateway_process(auto_connect)
                        .map_err(GatewayRequestFailure::Transport)?,
                );
                self.started_once = true;
            }
            if let Some(device_id) = self.selected_device_id.as_ref().filter(|_| !reselected) {
                if let Err(error) = self.process.as_mut().expect("process started").request("broker.selectDevice", json!({"deviceId":device_id, "autoConnect": !reconnect_after_release && !matches!(method, rpc::RECONNECT | rpc::RESET_SESSION | rpc::DISCONNECT | DEVICE_BROKER_DISCONNECT_METHOD)})) {
                    self.retire_current_process();
                    self.connected = false;
                    self.device = DeviceSession::default();
                    publish_gateway_readiness(false);
                    return Err(error);
                }
            }
            // Before this request's own wait, which may be a backup's.
            self.publish_lock_free_reads();
        }
        let mut origin = None;
        if let Some(expected) = &self.expected_broker_target {
            let status = self
                .process
                .as_mut()
                .ok_or_else(existing_connection_unavailable)?
                .request_unchecked(rpc::RUNTIME_STATUS, json!({}))?;
            let supported = status
                .get("capabilities")
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().any(|value| value == "expectedBrokerTarget"));
            if !supported
                || status.get("brokerTarget") != Some(expected)
                || status.get("targetReady") != Some(&json!(true))
            {
                return Err(GatewayRequestFailure::Remote { code:-32002, message:"The admitted broker target is no longer available, or this broker cannot enforce its identity".into(), retryable:false,
                    details:Some(json!({"code":"TARGET_SUPERSEDED","requestSent":false})) });
            }
        }
        if self.physical_only && method != rpc::RUNTIME_STATUS {
            let status = self
                .process
                .as_mut()
                .ok_or_else(existing_connection_unavailable)?
                .request_unchecked(rpc::RUNTIME_STATUS, json!({}))?;
            let supported = status
                .get("capabilities")
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().any(|value| value == PHYSICAL_TARGET_ORIGIN));
            if supported {
                origin = Some(PHYSICAL_TARGET_ORIGIN);
            } else if status.get("brokerTarget").is_some() || status.get("targetReady").is_some() {
                return Err(GatewayRequestFailure::Transport(
                    "The broker cannot guarantee a physical-only UI request".into(),
                ));
            }
            // Older brokers have no demo target and receive their original
            // envelope. A refused write is never retried without its fence.
        }
        if existing_only && method != rpc::RUNTIME_STATUS {
            // Device demand on a session the broker closed would reopen USB.
            // Asked under the same lock, on the connection this request would
            // use; a status read is never demand. A failure here retires a
            // broken connection like any other, and nothing replaces it.
            //
            // This gateway's lock is not the broker's: an idle release or
            // another client's disconnect can still close the session after
            // this answer. A broker that enforces the narrowing itself is
            // therefore told the read's origin, and decides under its own
            // request lock; the status read then only learns that it can, and
            // spares a round trip for a session already closed. An older
            // broker cannot be told (it rejects an unknown envelope field)
            // and cannot close that gap, so a resource read fails closed
            // against it: no device request is sent at all. Tool calls are
            // never `ExistingOnly` and reach an older broker as before.
            let status = self
                .request_detailed_as(rpc::RUNTIME_STATUS, json!({}), ConnectionUse::ExistingOnly)
                .ok();
            let open = status.as_ref().is_some_and(|status| {
                if self.physical_only {
                    status
                        .pointer("/usbDiagnostics/connected")
                        .and_then(Value::as_bool)
                        == Some(true)
                } else {
                    device_session_open(status)
                }
            });
            if !open || self.process.is_none() {
                return Err(existing_connection_unavailable());
            }
            if !status.as_ref().is_some_and(broker_narrows_resource_reads) {
                return Err(GatewayRequestFailure::Transport(
                    BROKER_TOO_OLD_FOR_RESOURCE_READS.into(),
                ));
            }
            origin = Some(if origin == Some(PHYSICAL_TARGET_ORIGIN) {
                PHYSICAL_RESOURCE_READ_ORIGIN
            } else {
                BROKER_RESOURCE_READ_ORIGIN
            });
        }
        if method == rpc::DISCONNECT {
            if let Some(state) = self.leave_shared_session_open() {
                return Ok(state);
            }
        }
        let announcement_directory = self
            .shared_descriptor_path()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let announced = self
            .process
            .as_ref()
            .is_none_or(|process| process.participant_announced);
        let announce = announcement_directory.is_some_and(|directory| {
            self.announcement_settings.should_announce(
                announced,
                broker_tray_settings::settings_revision(),
                Instant::now(),
                || broker_tray_settings::broker_tray_settings_in(&directory).tray_enabled,
            )
        });
        let provider = self.participant_provider.unwrap_or("QC Remote window");
        let process = self.process.as_mut().ok_or_else(|| {
            GatewayRequestFailure::Transport(
                "Gateway process initialization did not produce a process".into(),
            )
        })?;
        let result = (|| {
            if announce {
                process
                    .announce_participant_with(provider, self.participant_host_token.as_deref())?;
            }
            process.request_targeted_as_within(
                method,
                params,
                origin,
                self.expected_broker_target.as_ref(),
                GATEWAY_RESPONSE_TIMEOUT,
            )
        })();
        // The broker's own refusal of a narrowed read (`dispatch_resource_read`
        // in `services/device-broker/src/rpc.rs`: -32002, `notConnected`) is
        // the same offline answer this gateway gives when it has nothing to
        // use. The session it found closed is closed here too.
        if origin.is_some()
            && matches!(
                result,
                Err(GatewayRequestFailure::Remote { code: -32002, .. })
            )
        {
            self.connected = false;
            return Err(existing_connection_unavailable());
        }
        // The reason a call failed is the single most useful thing to keep: the
        // health record previously stored only a status, so a failure left
        // nothing on disk saying what went wrong.
        let (output, status, detail) = match result {
            Ok(value) => {
                if matches!(method, rpc::DISCONNECT | DEVICE_BROKER_DISCONNECT_METHOD) {
                    self.connected = false;
                } else if method.starts_with("device.")
                    && value
                        .pointer("/brokerTarget/source")
                        .and_then(Value::as_str)
                        != Some("demo")
                    && !self
                        .process
                        .as_ref()
                        .and_then(|process| process.last_broker_target.as_ref())
                        .is_some_and(|target| {
                            target.source == qc_relay_client::BrokerTargetSource::Demo
                        })
                {
                    self.connected = true;
                }
                self.observe_device_session(&value);
                if method == "broker.devices" && self.selected_device_id.is_none() {
                    self.selected_device_id = value
                        .get("selectedDeviceId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                (Ok(value), "ok", None)
            }
            Err(GatewayRequestFailure::Remote {
                code,
                message,
                retryable,
                details,
            }) => {
                if remote_failure_marks_disconnected(code, &message)
                    || details
                        .as_ref()
                        .and_then(|value| value.get("category"))
                        .and_then(Value::as_str)
                        == Some("notConnected")
                {
                    self.connected = false;
                }
                let detail = format!("[{code}] {message}");
                (
                    Err(GatewayRequestFailure::Remote {
                        code,
                        message,
                        retryable,
                        details,
                    }),
                    "remote-error",
                    Some(detail),
                )
            }
            Err(
                failure @ (GatewayRequestFailure::Transport(_)
                | GatewayRequestFailure::UnansweredMutation(_)),
            ) => {
                let (message, ambiguous) = match failure {
                    GatewayRequestFailure::Transport(message) => (message, false),
                    GatewayRequestFailure::UnansweredMutation(message) => (message, true),
                    _ => unreachable!(),
                };
                self.intentional_release |= self.process.as_ref().is_some_and(|process| {
                    process
                        .intentional_release
                        .load(std::sync::atomic::Ordering::Acquire)
                });
                self.retire_current_process();
                self.connected = false;
                let detail = message.clone();
                (
                    Err(if ambiguous {
                        GatewayRequestFailure::UnansweredMutation(format!("{message}. The mutation may have reached the device; it was not replayed. Reconnect, inspect the current device state, and decide how to recover before another mutation."))
                    } else {
                        GatewayRequestFailure::Transport(format!("{message}. The failed command was not replayed; the communication session was cleared for a safe reconnect."))
                    }),
                    "transport-error",
                    Some(detail),
                )
            }
        };
        self.forget_device_session_if_disconnected();
        // Published from inside the lock, where `connected` is authoritative,
        // so a readiness probe that arrives while the next call holds the lock
        // still has the real answer to fall back on.
        publish_gateway_readiness(self.connected);
        self.publish_lock_free_reads();
        write_runtime_health(self, method, status, detail.as_deref());
        output
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request_detailed(method, params)
            .map_err(|failure| match failure {
                GatewayRequestFailure::Transport(message)
                | GatewayRequestFailure::UnansweredMutation(message)
                | GatewayRequestFailure::Remote { message, .. } => message,
            })
    }

    fn restart(&mut self, method: &str) -> Result<Value, String> {
        self.intentional_release |= self.process.as_ref().is_some_and(|process| {
            process
                .intentional_release
                .load(std::sync::atomic::Ordering::Acquire)
        });
        self.retire_current_process();
        self.request(method, json!({}))
    }
}

/// Where the broker's stderr is kept, beside the runtime health record.
fn broker_stderr_path() -> PathBuf {
    runtime_health_path().with_file_name("broker-stderr.log")
}

#[cfg(unix)]
fn prepare_native_runtime_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        qc_native_platform::paths::prepare_directory(directory).map_err(std::io::Error::other)
    }
    #[cfg(target_os = "macos")]
    {
        qc_native_platform::paths_macos::prepare_directory(directory).map_err(std::io::Error::other)
    }
}

fn prepare_runtime_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(all(unix, not(test)))]
    {
        prepare_native_runtime_directory(directory)
    }
    // Preserve Windows behavior and the existing owned native-test roots.
    #[cfg(any(not(unix), test))]
    {
        fs::create_dir_all(directory)
    }
}

#[cfg(all(test, unix))]
mod runtime_health_directory_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn first_runtime_directory_is_private_and_accepted_by_the_broker() {
        let root = crate::tests::unique_fixture_directory("runtime-private-directory");
        let _cleanup = crate::tests::RemoveDirOnDrop::new(root.clone());
        let directory = root.join("new-state");
        prepare_native_runtime_directory(&directory).unwrap();
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        prepare_native_runtime_directory(&directory).unwrap();
        let link = root.join("linked-state");
        std::os::unix::fs::symlink(&directory, &link).unwrap();
        assert!(prepare_native_runtime_directory(&link).is_err());
        assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
    }
}

/// Append-only stderr sink for the broker, falling back to discarding output
/// only when the log cannot be opened at all.
fn broker_stderr_sink() -> Stdio {
    let path = broker_stderr_path();
    if let Some(directory) = path.parent() {
        let prepared = prepare_runtime_directory(directory);
        #[cfg(all(unix, not(test)))]
        if prepared.is_err() {
            return Stdio::null();
        }
        #[cfg(any(not(unix), test))]
        let _ = prepared;
    }
    // A broker restart loop must not grow this without bound.
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > BROKER_STDERR_MAX_BYTES) {
        let _ = fs::remove_file(&path);
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null())
}

/// Spawns `command` with its stderr on [`broker_stderr_sink`]. Under test the
/// child is also recorded as a user of this test process's health directory,
/// so a later run never sweeps that directory while the child may still hold
/// the log open (`test_health_dir`).
fn spawn_with_broker_stderr(command: &mut Command) -> std::io::Result<Child> {
    #[cfg(test)]
    let pending = test_health_dir::PendingUse::begin();
    let child = command.stderr(broker_stderr_sink()).spawn()?;
    #[cfg(test)]
    pending.record(child.id());
    Ok(child)
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(all(test, windows))]
fn runtime_health_path() -> PathBuf {
    // Both health and inherited broker stderr belong to the original test
    // PROCESS TREE, not to a request thread that may exit before its broker.
    // The canonical producer removes this root only after its actual native
    // descendants AND original host have acknowledged retirement.
    require_native_test_state_root()
        .expect("native test state requires the canonical owned Windows test launcher")
        .join(format!("process-{}", std::process::id()))
        .join("runtime-health.json")
}

// Non-Windows test launch/retirement policy is not changed by this Windows
// original-producer correction; it requires its own portable owner review.
#[cfg(all(test, not(windows)))]
fn runtime_health_path() -> PathBuf {
    // Tests drive real gateways, and each request rewrites this record. The
    // installed app's own record - and its broker stderr log beside it - is
    // not theirs to overwrite. Every test in this process shares one
    // directory, which a later test process sweeps once this one and every
    // broker spawned into it are proven gone (`test_health_dir`).
    test_health_dir::directory().join("runtime-health.json")
}

#[cfg(all(test, windows))]
thread_local! {
    // Pure per-thread failure injection; never mutates the parallel process
    // environment and never enables a production executable behavior.
    static TEST_STATE_ROOT_OVERRIDE: std::cell::RefCell<Option<Option<PathBuf>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, windows))]
fn require_native_test_state_root() -> Result<PathBuf, String> {
    let configured = TEST_STATE_ROOT_OVERRIDE
        .with(|root| root.borrow().clone())
        .unwrap_or_else(|| std::env::var_os("QC_REMOTE_TEST_STATE_ROOT").map(PathBuf::from));
    validate_native_test_state_root(configured.as_deref())
}

#[cfg(all(test, windows))]
fn validate_native_test_state_root(root: Option<&Path>) -> Result<PathBuf, String> {
    const GUIDANCE: &str = "Native tests requesting gateway health/stderr require the existing owned test producer: node scripts/test-windows-native.mjs --root . (private checkout: node .vendor/qc-remote-public/scripts/test-windows-native.mjs --root .). Standalone focused runners must opt in to nativeTestState on that same original-owned producer.";
    let root = root
        .filter(|root| root.is_absolute())
        .ok_or_else(|| GUIDANCE.to_string())?;
    let metadata = fs::symlink_metadata(root).map_err(|error| format!("{GUIDANCE} {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{GUIDANCE} Test state must be an existing non-link directory."
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "{GUIDANCE} Test state must not be a reparse point."
            ));
        }
    }
    // This is a test-launch contract, not an OS security boundary. Only the
    // launcher's internally allocated original-owner root authorizes deletion.
    // Rust never deletes a caller-configured root or any historical directory.
    Ok(root.to_path_buf())
}

#[cfg(all(not(test), unix))]
fn runtime_health_path() -> PathBuf {
    crate::broker_paths::app_local_data_dir().join("runtime-health.json")
}

#[cfg(all(not(test), not(unix)))]
fn runtime_health_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("QC Remote")
        .join("runtime-health.json")
}

use broker_installations::{detect_duplicate_broker_installations, DuplicateBrokerInstallation};

fn runtime_health_document(
    gateway: &Gateway,
    method: &str,
    status: &str,
    detail: Option<&str>,
    duplicate_broker_installations: &[DuplicateBrokerInstallation],
) -> Value {
    let mut document = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "appPid": std::process::id(),
        "gatewayPid": gateway.process.as_ref().map(|process| process.broker_pid),
        // A host-side latch over `device.*` outcomes only. Kept for existing
        // readers, but it cannot answer "is the session usable" on its own -
        // the three device fields below are what can.
        "connected": gateway.connected,
        "devicePhase": gateway.device.phase,
        "deviceSynchronized": gateway.device.synchronized,
        "deviceObservedAtUnix": gateway.device.observed_at_unix,
        "deviceWritesQuarantined": gateway.device.writes_quarantined,
        "deviceWriteQuarantineReason": gateway.device.write_quarantine_reason,
        "lastMethod": method,
        "lastStatus": status,
        "lastDetail": detail,
        "brokerStderrLog": broker_stderr_path().to_string_lossy(),
        "lastRequestAtUnix": unix_seconds()
    });
    // Only present when there is something to report: an ordinary, single
    // install keeps the existing closed set of health keys.
    if duplicate_broker_installations.len() > 1 {
        document["duplicateBrokerInstallations"] = json!(duplicate_broker_installations);
    }
    document
}

/// The last failure `write_runtime_health` hit, carried into the next record
/// that does get written.
///
/// A health record whose writes are failing looks exactly like one nothing is
/// updating: same bytes, same mtime, no complaint anywhere. Both of this
/// function's failure paths used to be silent - an early `return` when the
/// directory could not be created, and a discarded `fs::write` - so a stale
/// file was indistinguishable from a current one, and reading one as
/// authoritative is a mistake this record has already caused. Recording the
/// failure means the first record that lands afterwards says what went wrong
/// and when, and `write_runtime_health` reports its own state rather than
/// leaving the reader to infer it from a timestamp.
static LAST_RUNTIME_HEALTH_FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// The last health-record write failure, for `/health` to report.
///
/// Carrying it into the next record that lands covers a failure that clears.
/// One that does not clear needs a surface that is still working, because the
/// only place the record itself could say so is the file nothing can write -
/// and the app's stderr, unlike the broker's, is attached to nothing under
/// Tauri. `/health` is answered by the running process over a socket, so it
/// stays reachable exactly when the file does not.
pub(crate) fn last_runtime_health_failure() -> Option<String> {
    LAST_RUNTIME_HEALTH_FAILURE
        .lock()
        .ok()
        .and_then(|last| last.clone())
}

fn note_runtime_health_failure(what: &str, error: &dyn std::fmt::Display) {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let message = format!("{what}: {error} (at unix ms {at})");
    // Logged on change, not per attempt. `write_runtime_health` runs on every
    // gateway request including the app's own 1 Hz status poll, so a directory
    // that stays unwritable would otherwise emit a line a second - ~86k a day -
    // all saying the same thing. The first occurrence and every change of cause
    // are what carry information; the repeats are what the broker's own stderr
    // sink is size-capped against.
    let cause = strip_health_failure_timestamp(&message);
    if let Ok(mut last) = LAST_RUNTIME_HEALTH_FAILURE.lock() {
        let repeat = last
            .as_deref()
            .is_some_and(|previous| strip_health_failure_timestamp(previous) == cause);
        if !repeat {
            eprintln!("[qc-remote] runtime health record not written - {message}");
        }
        *last = Some(message);
    }
}

/// The cause half of a recorded failure, without its timestamp, so two
/// attempts that failed the same way compare equal.
fn strip_health_failure_timestamp(message: &str) -> &str {
    match message.rfind(" (at unix ms ") {
        Some(at) => &message[..at],
        None => message,
    }
}

/// `detect_duplicate_broker_installations` does several `fs::metadata`/
/// `is_file` stats against fixed paths - a condition that cannot change
/// between one RPC and the next in a running process, but `write_runtime_health`
/// used to re-run it on every single call, including the 1Hz idle
/// `system.status` poll. Computed once and cached for the process's
/// lifetime instead; a duplicate install appearing after this process
/// started is exactly as stale here as `env!("CARGO_PKG_VERSION")` already
/// is above it in the same document.
static DUPLICATE_BROKER_INSTALLATIONS: OnceLock<Vec<DuplicateBrokerInstallation>> = OnceLock::new();

fn cached_duplicate_broker_installations() -> &'static [DuplicateBrokerInstallation] {
    DUPLICATE_BROKER_INSTALLATIONS.get_or_init(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(|local_app_data| {
                detect_duplicate_broker_installations(&PathBuf::from(local_app_data))
            })
            .unwrap_or_default()
    })
}

fn write_runtime_health(gateway: &Gateway, method: &str, status: &str, detail: Option<&str>) {
    let path = runtime_health_path();
    let Some(directory) = path.parent() else {
        return;
    };
    if let Err(error) = prepare_runtime_directory(directory) {
        note_runtime_health_failure("could not create the health record's directory", &error);
        return;
    }
    let mut document = runtime_health_document(
        gateway,
        method,
        status,
        detail,
        cached_duplicate_broker_installations(),
    );
    // Reported before the write that clears it, so the record that finally
    // lands still names the failure that preceded it.
    if let Some(previous) = LAST_RUNTIME_HEALTH_FAILURE
        .lock()
        .ok()
        .and_then(|last| last.clone())
    {
        document["lastWriteFailure"] = Value::String(previous);
    }
    match serde_json::to_vec_pretty(&document) {
        Ok(bytes) => match fs::write(&path, bytes) {
            Ok(()) => {
                if let Ok(mut last) = LAST_RUNTIME_HEALTH_FAILURE.lock() {
                    *last = None;
                }
            }
            Err(error) => note_runtime_health_failure("could not write the health record", &error),
        },
        Err(error) => note_runtime_health_failure("could not encode the health record", &error),
    }
}

fn with_gateway_params(
    state: State<'_, Mutex<Gateway>>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    state
        .lock()
        .map_err(|_| "Gateway session lock was poisoned".to_string())?
        .request(method, params)
}

#[derive(Clone)]
struct WindowsRelayAdapter {
    app: AppHandle,
}

fn status_supports_broker_target_guard(status: &Value) -> bool {
    status
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|value| value == "expectedBrokerTarget"))
}

pub(crate) fn target_readiness_from_status(status: &Value) -> qc_relay_client::TargetReadiness {
    let target_aware = status_supports_broker_target_guard(status);
    let diagnostic_usb_connected = status
        .pointer("/usbDiagnostics/connected")
        .and_then(Value::as_bool);
    let target_ready = status
        .get("targetReady")
        .and_then(Value::as_bool)
        .unwrap_or(!target_aware && diagnostic_usb_connected.unwrap_or(false));
    let broker_target = status
        .get("brokerTarget")
        .cloned()
        .and_then(|value| serde_json::from_value::<qc_relay_client::BrokerTarget>(value).ok())
        .filter(|target| {
            !target.session_id.trim().is_empty()
                && target.session_id.len() <= 256
                && target.generation > 0
                && target.generation <= 9_007_199_254_740_991
        });
    let invalid = status.get("brokerTarget").is_some() && broker_target.is_none();
    // Optional diagnostic caches may be busy while native target selection
    // freshly verifies a connected, synchronized physical controller. Retain
    // that USB fact without inventing one for demo or capability-only status.
    let usb_connected = diagnostic_usb_connected.unwrap_or_else(|| {
        target_ready
            && broker_target.as_ref().is_some_and(|target| {
                target.source == qc_relay_client::BrokerTargetSource::Physical
            })
    });
    qc_relay_client::TargetReadiness {
        target_ready: target_ready && !invalid,
        usb_connected,
        broker_target: if target_ready && !invalid {
            broker_target
        } else {
            None
        },
    }
}

/// Unknown successful status is not a disconnect publication. Effect and
/// completion callers still use the fresh fail-closed mapping above.
pub(crate) fn publication_readiness_from_status(
    status: &Value,
) -> Option<qc_relay_client::TargetReadiness> {
    let target_known = status.get("targetReady").and_then(Value::as_bool).is_some();
    let target_aware = status_supports_broker_target_guard(status);
    // A modern broker can know one controller's USB fact while canonical
    // target selection is still uncertain (including another connected QC).
    // Only legacy physical-only hosts use raw USB as effective readiness.
    if !target_known
        && (target_aware
            || status
                .pointer("/usbDiagnostics/connected")
                .and_then(Value::as_bool)
                .is_none())
    {
        None
    } else {
        Some(target_readiness_from_status(status))
    }
}

/// The last readiness the gateway actually published, readable without taking
/// the gateway mutex. One `Gateway` exists per process, as Tauri managed state.
static PUBLISHED_READINESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn publish_gateway_readiness(connected: bool) {
    PUBLISHED_READINESS.store(connected, std::sync::atomic::Ordering::Release);
}

/// Readiness for `/health` and for the relay's periodic
/// `DeviceFrame::Ready { usb_connected }`.
///
/// Device RPCs hold the gateway mutex while they complete. Reading readiness
/// with `try_lock` alone would therefore report "device not ready" whenever a
/// healthy call happened to be in flight, turning contention into a false
/// disconnection.
///
/// A contended lock says nothing about the device, so fall back to what the
/// gateway last published rather than inventing a disconnection. Poisoning is
/// different: the state behind it is not trustworthy, so that stays unready.
#[cfg(test)]
fn gateway_readiness(gateway: &Mutex<Gateway>) -> bool {
    gateway_readiness_with(gateway, &PUBLISHED_READINESS)
}

/// The probe itself, against a named cell rather than the process-wide one.
///
/// The app has exactly one gateway, so `gateway_readiness` passing the global
/// is the whole of production behaviour and this split changes none of it. The
/// tests are where one cell for the whole process stops being true: the harness
/// runs them on separate threads against gateways of their own, and
/// `Gateway::request_detailed` publishes after every RPC, so any test that
/// makes a request writes the cell every other test is reading. A probe that
/// lands between another test's publication and its own read is then answered
/// for the wrong gateway - which is what made
/// `a_busy_gateway_is_not_reported_as_an_unready_device` fail roughly one run
/// in two on unchanged code.
///
/// A mutex the observing tests take was the first attempt at this and it is
/// not enough: its contract covers tests that *observe* the cell, while the
/// writes come from a dozen tests that only make a request and never mention
/// readiness at all. Giving each test its own cell removes the shared state
/// instead of scheduling around it, so a test added later cannot reintroduce
/// the race by calling `.request(...)` - and the logic under test is this
/// function either way.
#[cfg(test)]
fn gateway_readiness_with(
    gateway: &Mutex<Gateway>,
    published: &std::sync::atomic::AtomicBool,
) -> bool {
    match gateway.try_lock() {
        Ok(gateway) => {
            published.store(gateway.connected, std::sync::atomic::Ordering::Release);
            gateway.connected
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            published.load(std::sync::atomic::Ordering::Acquire)
        }
        Err(std::sync::TryLockError::Poisoned(_)) => false,
    }
}

/// Whether a headless host's gateway is attached to the broker, and if so what
/// it reports. See `local_stdio::HeadlessDeviceAdapter::readiness`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GatewayAttachment {
    /// Connected to the broker; its `connected` flag.
    Attached(bool),
    /// No broker connection: the flag is cleared, not observed.
    Detached,
    /// The lock is held (a request, an attach or a release is running); what
    /// the gateway last published.
    Busy(bool),
    Poisoned,
}

fn gateway_attachment(gateway: &Mutex<Gateway>) -> GatewayAttachment {
    gateway_attachment_with(gateway, &PUBLISHED_READINESS)
}

/// [`gateway_attachment`] against a named cell; see `gateway_readiness_with`
/// for why tests need their own.
fn gateway_attachment_with(
    gateway: &Mutex<Gateway>,
    published: &std::sync::atomic::AtomicBool,
) -> GatewayAttachment {
    match gateway.try_lock() {
        Ok(gateway) if gateway.process.is_some() => {
            published.store(gateway.connected, std::sync::atomic::Ordering::Release);
            GatewayAttachment::Attached(gateway.connected)
        }
        Ok(_) => {
            // Recorded as `gateway_readiness` always did: a host with no
            // broker to ask answers a later busy read from this cell, and a
            // detached gateway must not leave an earlier "ready" in it.
            published.store(false, std::sync::atomic::Ordering::Release);
            GatewayAttachment::Detached
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            GatewayAttachment::Busy(published.load(std::sync::atomic::Ordering::Acquire))
        }
        Err(std::sync::TryLockError::Poisoned(_)) => GatewayAttachment::Poisoned,
    }
}

#[async_trait::async_trait]
impl DeviceAdapter for WindowsRelayAdapter {
    fn effective_access_mode(&self) -> Option<qc_relay_client::AccessMode> {
        Some(crate::access_policy_store::effective_mode(
            &crate::broker_paths::app_local_data_dir(),
        ))
    }

    fn remote_session_enabled(&self) -> bool {
        crate::access_policy_store::effective_remote_enabled(
            &crate::broker_paths::app_local_data_dir(),
        )
    }

    async fn ready(&self) -> bool {
        self.target_readiness().await.target_ready
    }

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
            .map_err(|error| DeviceError {
                code: "INVALID_ARGUMENT".into(),
                message: error.to_string(),
                retryable: false,
                details: None,
                user_action_required: None,
            })?;
        self.invoke_with_target_as(
            method,
            params,
            if resource_read {
                ConnectionUse::ExistingOnly
            } else {
                ConnectionUse::MayConnect
            },
            expected,
        )
        .await
    }

    async fn invoke(&self, method: &str, params: Value) -> Result<Value, DeviceError> {
        // Read here: the blocking thread below has no task-local.
        self.invoke_as(method, params, ConnectionUse::of_current_task())
            .await
    }

    /// A call the paired relay marked as an MCP resource read is served as
    /// this window serves its own: from the broker connection it holds, never
    /// bringing one up. The mode is passed explicitly to the gateway, not
    /// left to a task-local.
    async fn invoke_resource_read(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, DeviceError> {
        qc_remote_mcp::as_resource_read(self.invoke_as(method, params, ConnectionUse::ExistingOnly))
            .await
    }
}

impl WindowsRelayAdapter {
    async fn sample_publication_readiness(&self) -> Option<qc_relay_client::TargetReadiness> {
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let status = request_gateway(
                &app.state::<Mutex<Gateway>>(),
                &app.state::<LockFreeReadSlot>(),
                rpc::RUNTIME_STATUS,
                json!({}),
                WhenGatewayBusy::Refuse,
                |gateway, params| {
                    gateway.request_detailed_as(
                        rpc::RUNTIME_STATUS,
                        params,
                        ConnectionUse::ExistingOnly,
                    )
                },
            );
            status
                .map(|status| publication_readiness_from_status(&status))
                .unwrap_or(Some(Default::default()))
        })
        .await
        .unwrap_or(Some(Default::default()))
    }

    /// One relay call, in the connection mode the caller chose.
    async fn invoke_as(
        &self,
        method: &str,
        params: Value,
        connection: ConnectionUse,
    ) -> Result<Value, DeviceError> {
        self.invoke_with_target_as(method, params, connection, None)
            .await
    }

    async fn invoke_with_target_as(
        &self,
        method: &str,
        params: Value,
        connection: ConnectionUse,
        expected: Option<Value>,
    ) -> Result<Value, DeviceError> {
        // A remote or MCP backup is written to this PC; the caller gets its
        // location, never the document (see `remote_backup`).
        remote_backup::invoke_saving_backups(
            method,
            params,
            remote_backup::default_backup_directory(),
            |params| {
                let app = self.app.clone();
                let method = method.to_owned();
                async move {
                    let method_for_action = method.clone();
                    await_relay_gateway(
                        method,
                        &RELAY_GATEWAY_NATIVE_BUDGET,
                        RELAY_GATEWAY_NATIVE_TIMEOUT,
                        move |admission| {
                            relay_gateway_request_targeted(
                                &app.state::<Mutex<Gateway>>(),
                                &app.state::<LockFreeReadSlot>(),
                                &method_for_action,
                                params,
                                connection,
                                RelayGatewayContext {
                                    stopped: None,
                                    admission: Some(&admission),
                                },
                                expected,
                            )
                        },
                    )
                    .await
                }
            },
        )
        .await
    }
}

fn relay_adapter(app: &AppHandle) -> std::sync::Arc<dyn DeviceAdapter> {
    std::sync::Arc::new(WindowsRelayAdapter { app: app.clone() })
}

#[tauri::command]
fn relay_status(state: State<'_, RelayBridge>) -> Result<RelayStatus, String> {
    state.status()
}

#[tauri::command]
async fn relay_usage(state: State<'_, RelayBridge>) -> Result<qc_relay_client::Usage, String> {
    state.usage().await
}

/// The devices paired to this account, for the REMOTE panel's list.
#[tauri::command]
async fn relay_devices(
    state: State<'_, RelayBridge>,
) -> Result<Vec<qc_relay_client::PairedDevice>, String> {
    state.roster().await
}

#[tauri::command]
async fn relay_connectors(
    state: State<'_, RelayBridge>,
) -> Result<Vec<qc_relay_client::RemoteConnector>, String> {
    state.connectors().await
}

#[tauri::command]
async fn unlink_public_relay_connector(
    state: State<'_, RelayBridge>,
    grant_id: String,
) -> Result<Vec<qc_relay_client::RemoteConnector>, String> {
    state.unlink_connector(&grant_id).await
}

/// Unpair another device of this account from this one. Unpairing this
/// computer stays `unpair_public_relay`, which also clears the local store.
#[tauri::command]
async fn unpair_public_relay_device(
    state: State<'_, RelayBridge>,
    device_id: String,
) -> Result<Vec<qc_relay_client::PairedDevice>, String> {
    state.unpair_device(&device_id).await
}

#[tauri::command]
async fn pair_public_relay(
    app: AppHandle,
    state: State<'_, RelayBridge>,
    endpoint: String,
    pairing_code: String,
    device_name: Option<String>,
) -> Result<RelayStatus, String> {
    state
        .pair(
            &endpoint,
            &pairing_code,
            device_name.as_deref().unwrap_or(relay::RELAY_DEVICE_NAME),
            relay_adapter(&app),
        )
        .await
}

fn relay_pair_url_allowed(endpoint: &str, pair_url: &str) -> bool {
    let Ok(endpoint) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let Ok(pair_url) = reqwest::Url::parse(pair_url) else {
        return false;
    };
    endpoint.scheme() == "https"
        && pair_url.scheme() == "https"
        && endpoint.origin() == pair_url.origin()
        && pair_url.username().is_empty()
        && pair_url.password().is_none()
}

#[tauri::command]
async fn begin_public_relay_browser_pairing(
    state: State<'_, RelayBridge>,
    endpoint: String,
) -> Result<Value, String> {
    let offer = state.begin_browser_pairing(&endpoint).await?;
    if !relay_pair_url_allowed(&endpoint, &offer.pair_url) {
        return Err("QC Remote Cloud returned an unsafe browser pairing address.".into());
    }
    Ok(json!({
        "pairUrl": offer.pair_url,
        "expiresAt": offer.expires_at
    }))
}

/// Open the pairing page in the system browser.
///
/// Signing in on the device host is a legitimate way to pair. The address is
/// checked with the same predicate that guards the QR, so a web layer that asked to open
/// something else gets a refusal rather than a browser window.
#[tauri::command]
fn open_public_relay_pair_url(endpoint: String, pair_url: String) -> Result<(), String> {
    if !relay_pair_url_allowed(&endpoint, &pair_url) {
        return Err("That is not a QC Remote Cloud pairing address.".into());
    }
    webbrowser::open(&pair_url)
        .map(|_| ())
        .map_err(|error| format!("Could not open the pairing page: {error}"))
}

#[tauri::command]
async fn poll_public_relay_browser_pairing(
    app: AppHandle,
    state: State<'_, RelayBridge>,
    device_name: Option<String>,
) -> Result<BrowserPairingProgress, String> {
    state
        .poll_browser_pairing(
            device_name.as_deref().unwrap_or(relay::RELAY_DEVICE_NAME),
            relay_adapter(&app),
        )
        .await
}

#[tauri::command]
fn cancel_public_relay_browser_pairing(state: State<'_, RelayBridge>) -> Result<(), String> {
    state.cancel_browser_pairing()
}

#[tauri::command]
async fn start_public_relay(app: AppHandle, state: State<'_, RelayBridge>) -> Result<(), String> {
    state.start(relay_adapter(&app)).await
}

#[tauri::command]
async fn unpair_public_relay(state: State<'_, RelayBridge>) -> Result<RelayStatus, String> {
    state.unpair().await
}

#[tauri::command]
async fn retry_public_relay_withdrawal(
    state: State<'_, RelayBridge>,
) -> Result<RelayStatus, String> {
    state.retry_withdrawal().await
}

/// Whether the loopback MCP surface is on, whether it is actually listening,
/// and why not when it is not. Read by the local-integration screen.
#[tauri::command]
fn local_mcp_status(state: State<'_, LocalAccess>) -> LocalAccessStatus {
    state.status()
}

#[tauri::command]
fn broker_tray_settings(
    state: State<'_, broker_tray_control::BrokerTrayControl>,
) -> broker_tray_control::BrokerTrayStatus {
    state.status()
}

#[tauri::command]
fn set_broker_tray_enabled(
    app: AppHandle,
    state: State<'_, broker_tray_control::BrokerTrayControl>,
    enabled: bool,
) -> Result<broker_tray_control::BrokerTrayStatus, String> {
    let _ = app;
    let status = state.set_enabled(enabled)?;
    Ok(status)
}

/// The Settings switch "Internet for the Quad Cortex". An app setting only: it
/// is not a Quad Cortex action and no AI tool can reach it.
#[tauri::command]
fn set_broker_updater_relay_enabled(
    state: State<'_, broker_tray_control::BrokerTrayControl>,
    enabled: bool,
) -> Result<broker_tray_control::BrokerTrayStatus, String> {
    state.set_updater_relay_enabled(enabled)
}

#[tauri::command]
fn set_broker_tray_idle_minutes(
    state: State<'_, broker_tray_control::BrokerTrayControl>,
    minutes: Value,
) -> Result<broker_tray_control::BrokerTrayStatus, String> {
    state.set_idle_minutes(&minutes)
}

/// Turns the loopback MCP surface off: the listener stops, live sessions are
/// cancelled, every descriptor this process published is removed, and the
/// choice is remembered for the next launch.
///
/// Async because stopping waits - briefly and with a hard bound - for whatever
/// is still connected, which must not block the UI thread.
#[tauri::command]
async fn disable_local_mcp(state: State<'_, LocalAccess>) -> Result<LocalAccessStatus, String> {
    state.disable()
}

/// Turns it back on and republishes the descriptors. A fresh port and bearer
/// token are minted, so every client re-reads the descriptor.
#[tauri::command]
async fn enable_local_mcp(state: State<'_, LocalAccess>) -> Result<LocalAccessStatus, String> {
    state.enable()
}

#[tauri::command]
async fn set_public_relay_access_mode(
    state: State<'_, RelayBridge>,
    mode: String,
) -> Result<RelayStatus, String> {
    state.set_access_mode(&mode).await?;
    state.status()
}

/// One allowlisted gateway entrypoint shared with Android's native bridge.
///
/// The web clients already build canonical camelCase parameters from the
/// generated gateway manifest. Keeping that envelope intact removes the need
/// for a second hand-written Tauri argument adapter for every QC operation.
/// Local window selection stays on this gateway socket. Other windows and MCP
/// hosts retain their own selection; the broker owns every USB handle.
#[tauri::command]
async fn broker_devices(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<Gateway>>();
        let mut gateway = state
            .lock()
            .map_err(|_| "Gateway session lock was poisoned".to_string())?;
        let mut inventory = gateway.request("broker.devices", json!({}))?;
        // Which broker generation answered, so the window can tell a
        // restarted broker from the same one answering again.
        let instance = gateway
            .process
            .as_ref()
            .and_then(gateway_process_instance_id);
        if let (Some(object), Some(instance)) = (inventory.as_object_mut(), instance) {
            object.insert("brokerInstanceId".into(), Value::String(instance));
        }
        Ok(inventory)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn select_broker_device(app: AppHandle, device_id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<Gateway>>();
        let mut gateway = state
            .lock()
            .map_err(|_| "Gateway session lock was poisoned".to_string())?;
        let selected = gateway.request("broker.selectDevice", json!({"deviceId":device_id}))?;
        gateway.selected_device_id = Some(device_id);
        gateway.connected = false;
        gateway.device = DeviceSession::default();
        publish_gateway_readiness(false);
        Ok(selected)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn gateway_invoke(
    app: AppHandle,
    method: String,
    params: Value,
) -> Result<Value, GatewayUiError> {
    if !generated_gateway::METHODS.contains(&method.as_str()) {
        return Err(GatewayUiError {
            code: "METHOD_NOT_ALLOWED".into(),
            message: "The requested gateway method is not part of the public app contract".into(),
            retryable: false,
        });
    }

    if method == rpc::RESET_SESSION {
        return tauri::async_runtime::spawn_blocking(move || {
            app.state::<Mutex<Gateway>>()
                .lock()
                .map_err(|_| "Gateway session lock was poisoned".to_string())?
                .with_physical_target(|gateway| gateway.restart(rpc::RESET_SESSION))
        })
        .await
        .map_err(|error| GatewayUiError {
            code: "DEVICE_TRANSPORT".into(),
            message: error.to_string(),
            retryable: true,
        })?
        .map_err(|message| GatewayUiError {
            code: "DEVICE_ERROR".into(),
            message,
            retryable: false,
        });
    }

    let nonblocking_read = matches!(
        method.as_str(),
        rpc::CURRENT_STATE_EVENTS | rpc::CURRENT_TEMPO_CLOCK | rpc::CURRENT_MASTER_VOLUME
    );
    tauri::async_runtime::spawn_blocking(move || {
        request_gateway(
            &app.state::<Mutex<Gateway>>(),
            &app.state::<LockFreeReadSlot>(),
            &method,
            params,
            if nonblocking_read {
                WhenGatewayBusy::Refuse
            } else {
                WhenGatewayBusy::Wait
            },
            |gateway, params| {
                gateway.with_physical_target(|gateway| gateway.request_detailed(&method, params))
            },
        )
    })
    .await
    .map_err(|error| GatewayUiError {
        code: "DEVICE_TRANSPORT".into(),
        message: error.to_string(),
        retryable: true,
    })?
    .map_err(GatewayUiError::from)
}

/// The USB panel's explicit Disconnect action closes the broker-owned device
/// session for every local participant. Demo switching continues to use the
/// ordinary per-client `device.disconnect`, which cannot tear USB away from an
/// AI client that is still using it.
#[tauri::command]
async fn disconnect_broker_device(app: AppHandle) -> Result<Value, GatewayUiError> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<Mutex<Gateway>>()
            .lock()
            .map_err(|_| {
                GatewayRequestFailure::Transport("Gateway session lock was poisoned".into())
            })?
            .request_detailed(DEVICE_BROKER_DISCONNECT_METHOD, json!({}))
    })
    .await
    .map_err(|error| GatewayUiError {
        code: "DEVICE_TRANSPORT".into(),
        message: error.to_string(),
        retryable: true,
    })?
    .map_err(GatewayUiError::from)
}

#[tauri::command]
async fn create_device_backup(app: AppHandle, name: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_device_backup_with(
            &name,
            |method, params| with_gateway_params(app.state::<Mutex<Gateway>>(), method, params),
            |file_name| {
                rfd::FileDialog::new()
                    .add_filter("Quad Cortex Backup", &["json"])
                    .set_file_name(file_name)
                    .save_file()
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn create_device_backup_with(
    name: &str,
    mut request: impl FnMut(&str, Value) -> Result<Value, String>,
    choose_path: impl FnOnce(String) -> Option<PathBuf>,
) -> Result<Value, String> {
    // Refuse an out-of-domain name before the dialog and before the transfer,
    // not after the device has already streamed the whole backup.
    let clean_name = validate_backup_name(name)?;
    // Capture before the modal dialog, not when it eventually closes. The
    // broker validates this session fence under its owner lock and at USB
    // admission; a changed route or reconnect cannot silently export another QC.
    let context = request("broker.backupTarget", json!({})).map_err(|error| format!(
        "Could not bind this backup to the connected Quad Cortex: {error}. If the broker is outdated, restart QC Remote and its broker after updating."
    ))?;
    let Some(path) = choose_path(format!("{}.json", backup_file_stem(&clean_name))) else {
        return Ok(json!({ "cancelled": true }));
    };
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        != Some("json".into())
    {
        return Err("Quad Cortex backups must use the .json extension".into());
    }
    let document = request(
        "broker.createDeviceBackupBound",
        json!({ "name": clean_name, "context": context }),
    )?;
    write_native_backup(&path, &document)?;
    Ok(json!({
        "cancelled": false,
        "path": path.to_string_lossy(),
        "name": path.file_name().and_then(|value| value.to_str()).unwrap_or("QC Device Backup.json")
    }))
}

#[tauri::command]
fn save_workspace_as(document: Value, suggested_name: String) -> Result<Value, String> {
    validate_workspace(&document)?;
    let file_name = format!("{}.qcw", safe_workspace_name(&suggested_name));
    let Some(path) = rfd::FileDialog::new()
        .add_filter("QC Workspace", &["qcw"])
        .set_file_name(file_name)
        .save_file()
    else {
        return Ok(json!({ "cancelled": true }));
    };
    write_workspace(&path, &document)
}

#[tauri::command]
fn save_workspace(path: String, document: Value) -> Result<Value, String> {
    write_workspace(Path::new(&path), &document)
}

#[tauri::command]
fn open_workspace() -> Result<Value, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("QC Workspace", &["qcw"])
        .pick_file()
    else {
        return Ok(json!({ "cancelled": true }));
    };
    validate_workspace_path(&path)?;
    let bytes = workspace::read_workspace_bytes(&path)?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid workspace JSON: {error}"))?;
    validate_workspace(&document)?;
    Ok(json!({
        "cancelled": false,
        "path": path.to_string_lossy(),
        "name": path.file_name().and_then(|value| value.to_str()).unwrap_or("workspace.qcw"),
        "document": document
    }))
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    external_links::open_external_url(&url)
}

/// The host operating system's name and version for a bug report's Platform
/// field, or null when it cannot be read. See `os_version`.
#[tauri::command]
fn host_os_version() -> Option<String> {
    os_version::host_os_version()
}

/// The event that tells an already-open Feedback window which kind of report
/// the Help menu entry just chosen asks for. The webview names the same event.
const FEEDBACK_ISSUE_KIND_EVENT: &str = "feedback-issue-kind";

/// The kinds of report the Feedback window can start on.
fn feedback_issue_kind(kind: &str, issue: Option<&str>) -> Result<Option<&'static str>, String> {
    match (kind, issue) {
        (_, None) => Ok(None),
        ("feedback", Some("bug")) => Ok(Some("bug")),
        ("feedback", Some("feature")) => Ok(Some("feature")),
        ("feedback", Some(other)) => Err(format!("Unknown feedback kind: {other}")),
        (_, Some(_)) => Err(format!("The {kind} window takes no feedback kind")),
    }
}

/// The page a Help or app window loads: `index.html?window=<kind>`, plus
/// `&issue=<bug|feature>` for a Feedback window opened on one kind of report.
fn window_page(kind: &str, issue: Option<&str>) -> String {
    match issue {
        Some(issue) => format!("index.html?window={kind}&issue={issue}"),
        None => format!("index.html?window={kind}"),
    }
}

/// A Help or app window: its label, title, default size, and minimum size.
#[derive(Debug, PartialEq)]
struct WindowSpec {
    label: &'static str,
    title: &'static str,
    width: f64,
    height: f64,
    min_width: f64,
    min_height: f64,
}

/// The three Help windows the Help menu opens, by the `kind` the webview names.
///
/// Each is a top-level window of its own, so a reader can move and resize the
/// guide beside the main window instead of reading it through a modal that
/// covers the controls it describes.
fn help_window_spec(kind: &str) -> Option<WindowSpec> {
    let (label, title, width, height, min_width, min_height) = match kind {
        "guide" => (
            "help-guide",
            "QC Remote — User Guide",
            760.0,
            680.0,
            560.0,
            480.0,
        ),
        "feedback" => (
            "help-feedback",
            "QC Remote — Feedback",
            760.0,
            680.0,
            560.0,
            480.0,
        ),
        "about" => (
            "help-about",
            "QC Remote — About",
            860.0,
            760.0,
            640.0,
            520.0,
        ),
        _ => return None,
    };
    Some(WindowSpec {
        label,
        title,
        width,
        height,
        min_width,
        min_height,
    })
}

/// Open one Help window, or focus it when it is already open.
///
/// `async` on purpose. Tauri runs a synchronous command on the main thread,
/// and on Windows `WebviewWindowBuilder::new` deadlocks there (WebView2 waits
/// on the event loop the command is blocking; see its "Known issues" in tauri
/// 2.11.5). An async command runs on the async runtime instead, so the builder
/// hands creation to the event loop and waits for it without holding it.
///
/// `issue` names the kind of report a Feedback window starts on (Help > Report
/// an Issue… or Send Feedback…). A new window reads it from its page query; an
/// already-open one is told over `FEEDBACK_ISSUE_KIND_EVENT`, so choosing the
/// other menu entry switches the window rather than only focusing it.
#[tauri::command]
async fn open_help_window(
    app: AppHandle,
    kind: String,
    issue: Option<String>,
) -> Result<(), String> {
    let spec =
        help_window_spec(&kind).ok_or_else(|| format!("Unknown help window kind: {kind}"))?;
    let issue = feedback_issue_kind(&kind, issue.as_deref())?;
    if focus_or_build_window(&app, &spec, || window_page(&kind, issue))? {
        if let Some(issue) = issue {
            app.emit_to(spec.label, FEEDBACK_ISSUE_KIND_EVENT, issue)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// The three app windows the main window opens, by the `kind` the webview
/// names: Settings (Edit > Settings…), Device Info (the USB panel's Device
/// info…), and Workspace (what an opened workspace file holds).
///
/// Like the Help windows, each is a movable, resizable window of its own in
/// reading-size type rather than a small modal over the controller. Its page
/// reads the main window's state over the event bus; Settings also sends its
/// two changes back there, where they are applied and remembered.
fn app_window_spec(kind: &str) -> Option<WindowSpec> {
    let (label, title, width, height, min_width, min_height) = match kind {
        "settings" => (
            "app-settings",
            "QC Remote — Settings",
            680.0,
            620.0,
            520.0,
            460.0,
        ),
        "device-info" => (
            "app-device-info",
            "QC Remote — Device Info",
            640.0,
            560.0,
            480.0,
            420.0,
        ),
        "workspace" => (
            "app-workspace",
            "QC Remote — Workspace",
            640.0,
            560.0,
            480.0,
            420.0,
        ),
        _ => return None,
    };
    Some(WindowSpec {
        label,
        title,
        width,
        height,
        min_width,
        min_height,
    })
}

/// Open one app window, or focus it when it is already open.
///
/// `async` for the same reason as `open_help_window`: a synchronous command
/// runs on the main thread, where building a WebView2 window deadlocks.
#[tauri::command]
async fn open_app_window(app: AppHandle, kind: String) -> Result<(), String> {
    let spec = app_window_spec(&kind).ok_or_else(|| format!("Unknown app window kind: {kind}"))?;
    focus_or_build_window(&app, &spec, || window_page(&kind, None))?;
    Ok(())
}

/// Held across one window's focus-or-build, so two opens of the same window in
/// quick succession (F1 pressed twice, or About and then its own "Report an
/// issue") cannot both miss the existing-window check. Async commands run
/// concurrently on the runtime's pool; without this the second one built a
/// duplicate label, which Tauri refuses, and that refusal reached the main
/// window as an error notice although nothing had gone wrong.
static WINDOW_OPEN_LOCK: Mutex<()> = Mutex::new(());

/// Focus the window `spec` names, or build it on `page` when none is open.
/// True when an existing window was focused. The lock is never held across an
/// `.await` (this function has none), so the calling command stays `Send`.
fn focus_or_build_window(
    app: &AppHandle,
    spec: &WindowSpec,
    page: impl FnOnce() -> String,
) -> Result<bool, String> {
    let _opening = WINDOW_OPEN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if focus_open_window(app, spec)? {
        return Ok(true);
    }
    build_window(app, spec, page())?;
    Ok(false)
}

/// Bring an already-open window forward. False when no window has the label,
/// so the caller builds one; one window per label, never a second copy.
fn focus_open_window(app: &AppHandle, spec: &WindowSpec) -> Result<bool, String> {
    let Some(existing) = app.get_webview_window(spec.label) else {
        return Ok(false);
    };
    existing.unminimize().map_err(|error| error.to_string())?;
    existing.set_focus().map_err(|error| error.to_string())?;
    Ok(true)
}

/// Build a new top-level window on `page`, sized by its spec and resizable.
fn build_window(app: &AppHandle, spec: &WindowSpec, page: String) -> Result<(), String> {
    tauri::WebviewWindowBuilder::new(app, spec.label, tauri::WebviewUrl::App(page.into()))
        .title(spec.title)
        .inner_size(spec.width, spec.height)
        .min_inner_size(spec.min_width, spec.min_height)
        .resizable(true)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn diagnostic_string(report: &Value, section: &str, key: &str, maximum: usize) -> String {
    report
        .get(section)
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|character| !character.is_control())
        .take(maximum)
        .collect()
}

fn redacted_diagnostics(report: &Value) -> Value {
    let events: Vec<Value> = report
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(100)
        .filter_map(|entry| {
            let at: String = entry
                .get("at")?
                .as_str()?
                .chars()
                .filter(|character| !character.is_control())
                .take(40)
                .collect();
            let event = entry.get("event")?.as_str()?;
            const ALLOWED_EVENTS: &[&str] = &[
                "app-start",
                "runtime-ready",
                "connect-attempt",
                "connected",
                "connect-failed",
                "disconnected",
                "reset-attempt",
                "diagnostics-exported",
            ];
            ALLOWED_EVENTS
                .contains(&event)
                .then(|| json!({ "at": at, "event": event }))
        })
        .collect();

    json!({
        "format": "qc-remote-diagnostics-v1",
        "generatedAt": report.get("generatedAt").and_then(Value::as_str).unwrap_or(""),
        "appVersion": report.get("appVersion").and_then(Value::as_str).unwrap_or("unknown"),
        "runtime": {
            "platform": diagnostic_string(report, "runtime", "platform", 80),
            "gatewayAvailable": report.get("runtime").and_then(|value| value.get("gatewayAvailable")).and_then(Value::as_bool).unwrap_or(false)
        },
        "connection": {
            "phase": diagnostic_string(report, "connection", "phase", 32),
            "demo": report.get("connection").and_then(|value| value.get("demo")).and_then(Value::as_bool).unwrap_or(true),
            // A demo session reports demo: false so its workflows run; this is
            // what tells a reader the report came from no hardware at all.
            // Only the two known values pass the allowlist.
            "source": match diagnostic_string(report, "connection", "source", 16).as_str() {
                "virtual" => "virtual",
                "device" => "device",
                _ => "unknown",
            }
        },
        "device": {
            "presetLocation": diagnostic_string(report, "device", "presetLocation", 8),
            "presetPosition": report.get("device").and_then(|value| value.get("presetPosition")).and_then(Value::as_u64).unwrap_or(0),
            "mode": diagnostic_string(report, "device", "mode", 16),
            "activeScene": report.get("device").and_then(|value| value.get("activeScene")).and_then(Value::as_u64).unwrap_or(0),
            "tempo": report.get("device").and_then(|value| value.get("tempo")).and_then(Value::as_u64).unwrap_or(0),
            "dirty": report.get("device").and_then(|value| value.get("dirty")).and_then(Value::as_bool).unwrap_or(false),
            "blockCount": report.get("device").and_then(|value| value.get("blockCount")).and_then(Value::as_u64).unwrap_or(0)
        },
        "events": events,
        "redaction": {
            "omitted": ["serial numbers", "MAC addresses", "usernames", "paths", "preset and setlist names"]
        }
    })
}

#[tauri::command]
fn export_diagnostics(report: Value) -> Result<Value, String> {
    let document = redacted_diagnostics(&report);
    let timestamp = chrono_free_timestamp();
    let Some(path) = rfd::FileDialog::new()
        .add_filter("QC Diagnostics", &["json"])
        .set_file_name(format!("QC Remote Diagnostics {timestamp}.json"))
        .save_file()
    else {
        return Ok(json!({ "cancelled": true }));
    };
    let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    write_diagnostics_bytes_with(&path, &bytes, |file, bytes| file.write_all(bytes))
        .map_err(|error| format!("Could not export diagnostics: {error}"))?;
    Ok(json!({
        "cancelled": false,
        "path": path.to_string_lossy(),
        "name": path.file_name().and_then(|value| value.to_str()).unwrap_or("QC Remote Diagnostics.json")
    }))
}

fn write_diagnostics_bytes_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    // Share the existing exclusive stage/sync/rename/disarm transaction.
    // A selected existing export stays intact on partial writes or unwind.
    workspace::write_document_with(path, |file| write(file, bytes))
}

fn chrono_free_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    seconds.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (device_event_tx, device_event_rx) =
        mpsc::sync_channel::<DeviceEvent>(quadcortex::domain::STATE_EVENT_DEFAULT_LIMIT);
    let gateway = Gateway {
        event_tx: Some(device_event_tx),
        ..Gateway::default()
    };
    let builder = tauri::Builder::default();
    // This must remain the first plugin registered. A second GUI process has
    // its own LOCAL connection observations even though the shared publisher
    // correctly refuses descriptor takeover, which can leave two windows
    // showing different client states. Redirect every later launch to the one
    // process that owns the UI and bring that window back from the taskbar.
    #[cfg(all(any(windows, target_os = "linux", target_os = "macos"), not(test)))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        // A second launch is a user reopening QC Remote (bridges never reach
        // this: `--local-mcp-bridge` returns before `run`). It is that
        // launch's one clear point for a recorded Quit, off this thread
        // because the clear may wait for the election lock.
        let launched = app.clone();
        if let Err(error) =
            start_relaunch_acknowledgment(&RELAUNCH_ACKNOWLEDGMENTS, app.clone(), move || {
                acknowledge_app_launch(&launched)
            })
        {
            let error = format!("The reopen could not clear the recorded Quit: {error}");
            eprintln!("QC Remote: {error}");
            if let Some(tray_control) = app.try_state::<broker_tray_control::BrokerTrayControl>() {
                tray_control.report_error(error);
            }
        }
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));
    let lock_free_reads = gateway.lock_free_read_slot();
    builder
        .manage(Mutex::new(gateway))
        .manage(demo_provider::DemoProviderBridge::default())
        .manage(lock_free_reads)
        .manage(RelayBridge::default())
        .manage(MacMenuEventBridge::default())
        .setup(move |app| {
            // Before anything can reach the gateway: every QC Remote process
            // meets the others' shared device broker through this directory.
            // Left unresolved, `GatewayProcess::start` falls back to a private,
            // unshared `--stdio` child instead (`start_owned_child`) - a second
            // hosting mode with its own, independently-maintained MIDI-warm-up
            // and orphan-reaping logic that has drifted out of sync with the
            // shared broker's own fixes more than once. `app_local_data_dir` failing here is not expected to happen
            // in practice, and `std::env::temp_dir` cannot fail the same way -
            // it names a directory without creating or verifying it - so this
            // keeps every window on the one, better-tested hosting mode instead
            // of silently degrading to the other on a resolution error.
            let descriptor_directory =
                app_descriptor_directory(app.path().app_local_data_dir(), &app.config().identifier)
                    .map_err(|error| {
                        #[cfg(target_os = "linux")]
                        report_startup_failure(&error);
                        std::io::Error::other(error)
                    })?;
            set_device_broker_descriptor_directory(&descriptor_directory);
            // An actual app launch is a user action: this launch's one clear
            // point for a recorded Quit (`user_quit_refusal`), before the tray
            // reconcile below and the window's first connect. A headless bridge
            // never runs this setup; a second launch is the single-instance
            // callback's own clear point.
            let launch_clear = acknowledge_user_connect_in(
                &device_broker_descriptor::device_broker_descriptor_path(&descriptor_directory),
            );
            // GUI startup alone reconciles tray/autostart preferences. The
            // headless MCP entrypoint never runs this setup or these effects.
            // Its resident start never clears a recorded Quit itself
            // (`--keep-user-quit`), and does not serve while one stands.
            let tray_control =
                broker_tray_control::BrokerTrayControl::new(descriptor_directory.clone());
            if let Err(error) = tray_control.reconcile() {
                // status().last_error retains this for the Settings window.
                eprintln!("QC Remote tray could not be reconciled: {error}");
            }
            if let Err(error) = launch_clear {
                // After the reconcile, which would overwrite it: reported where
                // the Settings window shows the host's errors, not swallowed.
                eprintln!("QC Remote: {error}");
                tray_control.report_error(error);
            }
            app.manage(tray_control);
            // Keep the native Windows title tied to the compiled package rather
            // than a manually-maintained label. This makes the running build
            // immediately identifiable from the taskbar and title bar.
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) =
                    window.set_title(&format!("QC Remote {}", app.package_info().version))
                {
                    eprintln!("QC Remote window title could not be updated: {error}");
                }
            }
            let handle = app.handle().clone();
            let adapter = relay_adapter(&handle);
            // Managed first, and unconditionally: the supervisor is what the
            // `local_mcp_*` commands and the exit handler reach for, and it has
            // to exist even when the listener refused to start or the user
            // turned local MCP off.
            let local_access = LocalAccess::new(
                handle.clone(),
                adapter.clone(),
                app.state::<RelayBridge>().access_receiver(),
            );
            let local_access_failure = local_access.start_if_enabled().err();
            app.manage(local_access);
            external_host::start(handle.clone(), descriptor_directory.clone());
            if let Some(error) = local_access_failure {
                // The failure is not swallowed here: it is on
                // `local_mcp_status().lastError` for the UI to surface. The
                // console line is for headless and packaging runs.
                eprintln!("QC Remote local access did not start: {error}");
            }
            // Secret Service reads run on the bounded credential worker. Do
            // not make Tauri's setup callback wait for an unlock prompt.
            let app_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                app_handle
                    .state::<RelayBridge>()
                    .initialize(relay_adapter(&app_handle))
                    .await;
            });
            // Deferred, detached, and once per app start. Every release makes
            // every route this app wrote name a version behind, so the app
            // brings its own up to date rather than waiting for the owner to
            // press Update on each row. It reads and writes other
            // applications' configuration files, so it must not sit on the
            // start-up path: `refresh_managed_routes_on_start` spawns its own
            // thread and returns immediately.
            refresh_managed_routes_on_start();
            std::thread::Builder::new()
                .name("qc-window-events".into())
                .spawn(move || {
                    while let Ok((event, payload)) = device_event_rx.recv() {
                        if handle.emit(event, payload).is_err() {
                            return;
                        }
                    }
                })
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(target_os = "macos")]
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    let tray_enabled = window
                        .app_handle()
                        .try_state::<broker_tray_control::BrokerTrayControl>()
                        .is_some_and(|control| control.status().tray_enabled);
                    match macos_main_window_close_action(tray_enabled) {
                        MacosMainWindowCloseAction::Hide => {
                            api.prevent_close();
                            let _ = window.hide();
                        }
                        MacosMainWindowCloseAction::Exit => window.app_handle().exit(0),
                    }
                    return;
                }
            }
            if !main_window_is_gone(window.label(), event) {
                return;
            }
            window
                .app_handle()
                .state::<demo_provider::DemoProviderBridge>()
                .stop();
            // The Help and app windows belong to the main window. Left open,
            // they would keep the process, the device session, and the local
            // MCP listener alive behind windows that ask a main window that no
            // longer exists for diagnostics, device state, and settings.
            for (label, help) in window.app_handle().webview_windows() {
                if label == "main" {
                    continue;
                }
                if let Err(error) = help.close() {
                    eprintln!("QC Remote could not close the {label} window: {error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            macos_menu_events_ready,
            relay_status,
            relay_usage,
            relay_devices,
            relay_connectors,
            unpair_public_relay_device,
            unlink_public_relay_connector,
            pair_public_relay,
            begin_public_relay_browser_pairing,
            poll_public_relay_browser_pairing,
            cancel_public_relay_browser_pairing,
            open_public_relay_pair_url,
            start_public_relay,
            unpair_public_relay,
            retry_public_relay_withdrawal,
            set_public_relay_access_mode,
            gateway_invoke,
            demo_provider::start_demo_provider,
            demo_provider::admit_demo_provider_request,
            demo_provider::complete_demo_provider_request,
            demo_provider::stop_demo_provider,
            acknowledge_user_connect,
            disconnect_broker_device,
            broker_devices,
            select_broker_device,
            create_device_backup,
            save_workspace_as,
            save_workspace,
            open_workspace,
            open_external_url,
            open_help_window,
            open_app_window,
            host_os_version,
            export_diagnostics,
            read_virtual_device_store,
            write_virtual_device_store,
            claude_desktop_extension_status,
            install_claude_desktop_extension,
            open_claude_desktop,
            #[cfg(not(windows))]
            open_claude_desktop_download,
            chatgpt_local_integration_status,
            install_chatgpt_local_integration,
            remove_chatgpt_local_integration,
            open_chatgpt_desktop,
            #[cfg(not(windows))]
            open_chatgpt_desktop_download,
            open_chatgpt_configuration,
            antigravity_local_integration_status,
            install_antigravity_local_integration,
            remove_antigravity_local_integration,
            open_antigravity,
            #[cfg(not(windows))]
            open_antigravity_download,
            lm_studio_local_integration_status,
            install_lm_studio_local_integration,
            remove_lm_studio_local_integration,
            open_lm_studio,
            #[cfg(not(windows))]
            open_lm_studio_download,
            jan_local_integration_status,
            install_jan_local_integration,
            remove_jan_local_integration,
            open_jan,
            #[cfg(not(windows))]
            open_jan_download,
            other_mcp_stdio_setup,
            save_local_client_package,
            local_mcp_status,
            broker_tray_settings,
            set_broker_tray_enabled,
            set_broker_tray_idle_minutes,
            set_broker_updater_relay_enabled,
            enable_local_mcp,
            disable_local_mcp,
            test_local_mcp
        ])
        .build(tauri::generate_context!())
        .expect("error while running QC Remote")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if matches!(
                &event,
                tauri::RunEvent::Reopen {
                    has_visible_windows: false,
                    ..
                }
            ) {
                show_macos_main_window(app);
            }
            if !is_process_exit_event(&event) {
                return;
            }
            external_host::stop();
            if let Some(local_access) = app.try_state::<LocalAccess>() {
                local_access.shutdown_for_exit();
            }
        });
}

#[cfg(target_os = "macos")]
fn show_macos_main_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The events Tauri delivers while the process is on its way out.
///
/// `tauri::App::run` ends in `std::process::exit`, which runs no destructors:
/// `Drop for LocalAccess` never ran, and no `RunEvent::Exit` handler was
/// registered, so the loopback descriptor was never removed. Two of them were
/// found on disk from separate dead app runs (PIDs 27264 and 95624), which is
/// what makes a client dial a port nothing owns any more.
///
/// `RunEvent::Exit` is on the path Tauri actually takes:
/// `make_run_event_loop_callback` (tauri 2.11.5, `src/app.rs`) invokes the
/// callback for `RuntimeRunEvent::Exit` and only then calls
/// `cleanup_before_exit`, and tauri-runtime-wry raises that event from tao's
/// `Event::LoopDestroyed`, before the event loop exits the process.
/// `ExitRequested` arrives earlier, when the last window closes, and is
/// included so the descriptor is gone the moment the user closes the app.
fn is_process_exit_event(event: &tauri::RunEvent) -> bool {
    matches!(
        event,
        tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
    )
}

/// Whether this window event is the main window being destroyed, which is when
/// the Help windows it opened close with it.
fn main_window_is_gone(label: &str, event: &tauri::WindowEvent) -> bool {
    label == "main" && matches!(event, tauri::WindowEvent::Destroyed)
}

/// A closed main window can only be reopened from the Dock if its native
/// window is recreated. In background mode the existing window is retained
/// and hidden; without it, exit so a Dock reopen starts a fresh app instance.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum MacosMainWindowCloseAction {
    Hide,
    Exit,
}

#[cfg(any(target_os = "macos", test))]
fn macos_main_window_close_action(tray_enabled: bool) -> MacosMainWindowCloseAction {
    if tray_enabled {
        MacosMainWindowCloseAction::Hide
    } else {
        MacosMainWindowCloseAction::Exit
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

#[cfg(test)]
mod user_action_metadata_tests {
    use super::*;

    fn serialize_failure(data: Value) -> Value {
        let details = safe_gateway_error_details(Some(&data));
        let retryable = details
            .as_ref()
            .and_then(|d| d.get("retryable"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        serde_json::to_value(device_error_from_gateway_failure(
            GatewayRequestFailure::Remote {
                code: -32000,
                message: "UNSAVED_STATE_UNCONFIRMED message is not policy".into(),
                retryable,
                details,
            },
        ))
        .unwrap()
    }

    #[test]
    fn windows_refusal_frames_match_shared_cloud_contract_fixture() {
        let expected: Vec<Value> = serde_json::from_str(include_str!(
            "../../../../packages/rust/qc-relay-protocol/fixtures/user-action-result-frames.json"
        ))
        .unwrap();
        for frame in expected {
            let code = frame["error"]["code"].as_str().unwrap();
            let details = safe_gateway_error_details(Some(&serde_json::json!({
                "code":code,"retryable":true,"userActionRequired":"untrusted"
            })));
            let retryable = details.as_ref().unwrap()["retryable"].as_bool().unwrap();
            let error = device_error_from_gateway_failure(GatewayRequestFailure::Remote {
                code: -32000,
                message: "fixture".into(),
                retryable,
                details,
            });
            let actual = qc_relay_client::DeviceFrame::Result {
                id: "user-action-contract".into(),
                ok: false,
                result: None,
                error: Some(error),
            };
            assert_eq!(serde_json::to_value(actual).unwrap(), frame);
        }
    }

    #[test]
    fn serialized_relay_failure_derives_action_only_from_exact_machine_code() {
        for (code, required) in [
            ("UNSAVED_STATE_UNCONFIRMED", true),
            ("UNSAVED_STATE_NOT_YET_OBSERVED", false),
        ] {
            for supplied in [
                Value::Bool(required),
                Value::Bool(!required),
                Value::String("true".into()),
                Value::Null,
            ] {
                let wire = serialize_failure(serde_json::json!({
                    "code":code,"retryable":required,"userActionRequired":supplied,"secret":"must drop"
                }));
                assert_eq!(wire["code"], code);
                assert_eq!(wire["userActionRequired"], required);
                assert_eq!(wire["retryable"], !required);
                assert_eq!(wire["details"]["userActionRequired"], required);
                assert!(wire["details"].get("secret").is_none());
            }
        }
        for code in [
            "DEVICE_TRANSPORT",
            "UNSAVED_STATE_UNCONFIRMED_EXTRA",
            "unsaved_state_unconfirmed",
        ] {
            let wire =
                serialize_failure(serde_json::json!({"code":code,"userActionRequired":true}));
            assert_eq!(wire["code"], "-32000");
            assert!(wire.get("userActionRequired").is_none());
            assert!(wire["details"].get("userActionRequired").is_none());
        }
        let wire = serialize_failure(serde_json::json!({"userActionRequired":"true"}));
        assert!(wire.get("userActionRequired").is_none());
        let legacy =
            serde_json::to_value(DeviceError::new("DEVICE_TRANSPORT", "offline", true)).unwrap();
        assert!(legacy.get("userActionRequired").is_none());
    }
}
#[cfg(test)]
mod dispatch_error_projection_tests {
    use super::*;

    fn frame(code: i64, data: Value) -> Value {
        let failure = decode_gateway_response(
            serde_json::json!({
                "jsonrpc":"2.0","id":7,"error":{"code":code,"message":"fixture","data":data}
            }),
            7,
        )
        .unwrap_err();
        serde_json::to_value(qc_relay_client::DeviceFrame::Result {
            id: "fixture".into(),
            ok: false,
            result: None,
            error: Some(device_error_from_gateway_failure(failure)),
        })
        .unwrap()
    }

    #[test]
    fn actual_broker_decode_prioritizes_predispatch_quarantine_code() {
        let wire = frame(
            -32012,
            serde_json::json!({
                "code":"UNSAVED_STATE_UNCONFIRMED","retryable":true,
                "userActionRequired":true,"requestSent":true,"writesQuarantined":false,
                "quarantine":{"scope":"presetGrid","sinceUnixMs":100,"sessionEpoch":2,
                    "reason":"secret","clearsBy":"secret","method":"secret"},
                "credential":"secret"
            }),
        );
        let error = &wire["error"];
        assert_eq!(error["code"], "WRITE_QUARANTINED");
        assert_eq!(error["retryable"], false);
        assert!(error.get("userActionRequired").is_none());
        assert_eq!(
            error["details"],
            serde_json::json!({
                "code":"WRITE_QUARANTINED","retryable":false,"requestSent":false,
                "writesQuarantined":true,"quarantine":{"scope":"presetGrid","sinceUnixMs":100,"sessionEpoch":2}
            })
        );
        assert!(!wire.to_string().contains("secret"));
    }

    #[test]
    fn ambiguous_delivery_preserves_explicit_facts_without_inference_or_user_action() {
        let wire = frame(
            -32010,
            serde_json::json!({"code":"WRITE_OUTCOME_AMBIGUOUS",
            "retryable":true,"writesQuarantined":true,"userActionRequired":true,"requestSent":false}),
        );
        assert_eq!(wire["error"]["code"], "WRITE_OUTCOME_AMBIGUOUS");
        assert!(wire["error"].get("userActionRequired").is_none());
        assert_eq!(wire["error"]["details"]["requestSent"], false);
        let sent = frame(
            -32010,
            serde_json::json!({"code":"WRITE_OUTCOME_AMBIGUOUS","requestSent":true}),
        );
        assert_eq!(sent["error"]["details"]["requestSent"], true);
        let absent = frame(
            -32010,
            serde_json::json!({"code":"WRITE_OUTCOME_AMBIGUOUS"}),
        );
        assert!(absent["error"]["details"].get("requestSent").is_none());
        let legacy = frame(
            -32010,
            serde_json::json!({"code":"OTHER","retryable":false}),
        );
        assert_eq!(legacy["error"]["code"], "-32010");
    }
}
