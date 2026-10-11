//! The background MCP services (loopback MCP listener, remote relay,
//! prompts, `tray-host-status.json`). Since 0.1.178 they run inside
//! `qc-device-broker.exe`, supervised by the broker's resident host; that
//! process forbids broker spawning, so the device adapter below only ever
//! attaches to the broker it lives in. This entry never constructs a Tauri
//! application.
use crate::{host_presence, local_access, local_stdio, relay};
use qc_relay_client::{Client, ConnectionState, DeviceAdapter};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// The provider name this host's broker connection presents
/// (services/device-broker/src/rpc.rs `IN_PROCESS_HOST_PROVIDER`). The icon
/// leaves this connection out of its client list and lists the AI apps the
/// host serves instead. Display only: the broker recognises its own host by
/// the per-process token passed to [`start`], which the host presents as
/// `hostToken`.
pub const IN_PROCESS_HOST_PROVIDER: &str = "QC Remote background host";

pub struct BackgroundMcpHost {
    stop: watch::Sender<bool>,
    done: mpsc::Receiver<()>,
    finished: Arc<AtomicBool>,
    presence: Arc<Mutex<Option<serde_json::Value>>>,
}
impl BackgroundMcpHost {
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    pub fn presence(&self) -> Option<serde_json::Value> {
        self.presence.lock().ok()?.clone()
    }
    pub fn stop(&mut self) {
        self.stop_within(Duration::from_secs(7));
    }
    /// Ask the host to stop and wait at most `within`. The resident broker
    /// bounds every wait it makes on the host, so a wedged host can never hold
    /// back process exit or an installer's stop.
    pub fn stop_within(&mut self, within: Duration) {
        self.stop.send_replace(true);
        let _ = self.done.recv_timeout(within);
    }
}
impl Drop for BackgroundMcpHost {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

pub fn start(
    directory: PathBuf,
    host_token: String,
    readiness: local_stdio::BrokerReadinessProbe,
) -> Result<BackgroundMcpHost, String> {
    let lease = HostLease::try_acquire(&directory, "background-mcp-host.lock")?
        .ok_or("Another process already owns the background MCP host")?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let (stop, receiver) = watch::channel(false);
    let (done_tx, done) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let presence = Arc::new(Mutex::new(None));
    let (finish_flag, view) = (finished.clone(), presence.clone());
    std::thread::Builder::new()
        .name("qc-background-mcp-host".into())
        .spawn(move || {
            // The broker and the USB worker share this process: a panic in the
            // host must end only the host, and must still report it finished
            // so the broker's supervisor restarts it.
            let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(run(directory, host_token, readiness, receiver, view));
                runtime.shutdown_timeout(Duration::from_secs(2));
            }));
            if served.is_err() {
                eprintln!("The background MCP host panicked; the broker restarts it");
            }
            drop(lease);
            finish_flag.store(true, Ordering::Release);
            let _ = done_tx.send(());
        })
        .map_err(|e| e.to_string())?;
    Ok(BackgroundMcpHost {
        stop,
        done,
        finished,
        presence,
    })
}

pub(crate) struct HostLease {
    _file: File,
}
impl HostLease {
    pub(crate) fn try_acquire(directory: &Path, name: &str) -> Result<Option<Self>, String> {
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(name))
            .map_err(|e| e.to_string())?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error.to_string()),
        }
    }
}

struct BackgroundRelay {
    task: tokio::task::JoinHandle<()>,
    stop: watch::Sender<bool>,
    _lease: HostLease,
    credential: String,
    state: watch::Sender<ConnectionState>,
}
impl Drop for BackgroundRelay {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        self.state.send_replace(ConnectionState::Stopped);
        self.task.abort();
    }
}

/// Where the background host publishes its bearer token: exactly the set the
/// GUI publisher writes, from the same two directories. It used to start from
/// `local_stdio::state_paths`, which honours `QC_REMOTE_LOCAL_MCP_STATE` in
/// every build, so a release broker wrote a live token wherever the
/// environment pointed - and the two publishers wrote different sets.
fn publication_paths(directory: &Path, roaming: Option<PathBuf>) -> Vec<PathBuf> {
    local_access::descriptor_paths(directory, roaming)
}

async fn run(
    directory: PathBuf,
    host_token: String,
    readiness: local_stdio::BrokerReadinessProbe,
    mut stop: watch::Receiver<bool>,
    view: Arc<Mutex<Option<serde_json::Value>>>,
) {
    let device = local_stdio::HeadlessDeviceAdapter::with_identity(
        crate::device_broker_descriptor::device_broker_descriptor_path(&directory),
        true,
        IN_PROCESS_HOST_PROVIDER,
        Some(host_token),
    )
    // With no MCP client attached the gateway is detached; /health and the
    // relay then answer from this process's broker, which a readiness read
    // never opens, reopens or joins.
    .with_broker_readiness(readiness);
    let adapter: Arc<dyn DeviceAdapter> = Arc::new(device.clone());
    let (access, access_rx) = watch::channel(relay::load_access_mode());
    let mut listener: Option<local_access::RunningLocalAccess> = None;
    let mut relay_host: Option<BackgroundRelay> = None;
    let mut last_settings = Instant::now() - Duration::from_secs(5);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut maintenance: Option<tokio::task::JoinHandle<()>> = None;
    // The linked-apps list is fetched when this relay connects (and after a
    // remote Unlink, in remote_unlink.rs), never on a timer.
    let mut relay_was_connected = false;
    let mut linked_apps_fetch: Option<tokio::task::JoinHandle<()>> = None;
    // A connect that arrives while a read is still running gets one read of
    // its own once that one ends.
    let mut linked_apps_pending = false;
    loop {
        tokio::select! {
            changed = stop.changed() => { if changed.is_err() || *stop.borrow() { break; } }
            _ = interval.tick() => {}
        }
        if *stop.borrow()
            || !crate::broker_tray_settings::broker_tray_settings_in(&directory).tray_enabled
        {
            break;
        }
        let local_enabled = local_access::local_mcp_enabled_in(&directory);
        if !local_enabled {
            if let Some(old) = listener.take() {
                old.stop();
            }
        } else if listener.is_none() {
            #[cfg(target_os = "linux")]
            let paths = publication_paths(&directory, None);
            #[cfg(not(target_os = "linux"))]
            let paths =
                publication_paths(&directory, std::env::var_os("APPDATA").map(PathBuf::from));
            // The existing publisher guard authenticates any GUI owner. It
            // never displaces that owner; after GUI exit the next tick wins.
            listener = local_access::start_listener(paths, adapter.clone(), access_rx.clone()).ok();
        }
        if last_settings.elapsed() >= Duration::from_secs(5) {
            last_settings = Instant::now();
            access.send_replace(relay::load_access_mode());
            let credential = relay::load_credentials()
                .ok()
                .flatten()
                .filter(|c| c.uses_current_identity_provider());
            host_presence::set_remote_endpoint(credential.as_ref().map(|c| c.endpoint.as_str()));
            let identity = credential
                .as_ref()
                .and_then(|c| {
                    serde_json::to_string(&(&c.endpoint, &c.device_id, &c.device_credential)).ok()
                })
                .unwrap_or_default();
            if relay_host
                .as_ref()
                .is_some_and(|host| host.credential != identity || host.task.is_finished())
            {
                relay_host = None;
                // The list belongs to the relay (and pairing) just dropped.
                crate::remote_grants::clear();
            }
            if relay_host.is_none() {
                if let Some(credentials) = credential {
                    if let Ok(Some(lease)) =
                        HostLease::try_acquire(&directory, "remote-mcp-host.lock")
                    {
                        let (state, state_rx) = watch::channel(ConnectionState::Stopped);
                        host_presence::set_remote_state(state_rx);
                        let (halt, halt_rx) = watch::channel(false);
                        let (presence, presence_rx) = watch::channel(None);
                        host_presence::set_remote(presence_rx);
                        // No remote method waits for a host-side approval
                        // (HOST_APPROVAL_METHODS is empty): the user
                        // confirms once, in the AI app that asked.
                        if let Ok(credentials) = relay::begin_credential_connection(credentials) {
                            let client = Client::new(
                                credentials,
                                adapter.clone(),
                                state.clone(),
                                access_rx.clone(),
                                halt_rx,
                            )
                            .with_gateway_api_version(
                                quadcortex::device::generated_gateway::API_VERSION,
                            )
                            .with_mcp_presence(presence)
                            .with_credential_status(Arc::new(relay::record_credential_status))
                            .with_replacement_ack(Arc::new(relay::acknowledge_replacement));
                            relay_host = Some(BackgroundRelay {
                                task: tokio::spawn(client.run()),
                                stop: halt,
                                _lease: lease,
                                credential: identity,
                                state,
                            });
                        }
                    }
                }
            }
        }
        let relay_connected = relay_host
            .as_ref()
            .is_some_and(|host| *host.state.borrow() == ConnectionState::Connected);
        if relay_connected && !relay_was_connected {
            linked_apps_pending = true;
        }
        if linked_apps_pending
            && linked_apps_fetch
                .as_ref()
                .is_none_or(|task| task.is_finished())
        {
            linked_apps_pending = false;
            linked_apps_fetch = Some(tokio::spawn(crate::remote_grants::refresh_on_connect(
                || host_presence::remote_state() == ConnectionState::Connected,
            )));
        }
        if relay_host.is_none() {
            crate::remote_grants::clear();
        }
        relay_was_connected = relay_connected;
        let endpoint = listener.as_ref().map(|running| running.endpoint());
        let document = local_access::host_status_document(
            endpoint,
            listener.as_ref().map(|running| running.instance_id()),
        );
        let snapshot = host_presence::snapshot(None);
        if maintenance.as_ref().is_none_or(|task| task.is_finished()) {
            let device = device.clone();
            let used = host_presence::host_in_use(&snapshot.clients);
            // Broker election/response waits must not delay status publication,
            // settings changes or listener shutdown.
            maintenance = Some(tokio::task::spawn_blocking(move || {
                if used {
                    device.retain_while_used();
                } else {
                    device.release_when_unused();
                }
            }));
        }
        // While a GUI publishes, its registry is authoritative; the tray uses
        // its authenticated reader instead of replacing it with an empty list.
        *view.lock().unwrap_or_else(|p| p.into_inner()) = if listener.is_some() || !local_enabled {
            if let Err(error) = crate::host_runtime_status::publish(&directory, document.clone()) {
                eprintln!("Could not publish tray host status: {error}");
            }
            Some(document)
        } else {
            None
        };
    }
    drop(relay_host);
    host_presence::set_remote_endpoint(None);
    device.stop();
    if let Some(listener) = listener {
        listener.stop();
    }
    crate::host_runtime_status::remove(&directory);
    *view.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_background_host_publishes_exactly_the_gui_set() {
        // Never an environment-chosen path: the set is a function of the two
        // directories alone, and it is the GUI publisher's set.
        let directory = PathBuf::from(r"C:\Users\someone\AppData\Local\org.freevia.qcremote");
        let roaming = PathBuf::from(r"C:\Users\someone\AppData\Roaming");
        let paths = publication_paths(&directory, Some(roaming.clone()));
        assert_eq!(
            paths,
            vec![
                directory.join("local-mcp.json"),
                roaming
                    .join("Claude")
                    .join("qc-remote")
                    .join("local-mcp.json"),
            ]
        );
        assert_eq!(
            paths,
            local_access::descriptor_paths(&directory, Some(roaming))
        );
        assert_eq!(
            publication_paths(&directory, None),
            vec![directory.join("local-mcp.json")]
        );
    }
    #[test]
    fn the_host_declares_the_provider_its_broker_excludes() {
        // services/device-broker/src/rpc.rs IN_PROCESS_HOST_PROVIDER.
        assert_eq!(IN_PROCESS_HOST_PROVIDER, "QC Remote background host");
        assert_ne!(
            IN_PROCESS_HOST_PROVIDER, "QC Remote MCP host",
            "bridges stay participants"
        );
    }

    #[test]
    fn publisher_lease_is_exclusive_and_released_with_owner() {
        let directory =
            std::env::temp_dir().join(format!("qc-mcp-lease-{:032x}", rand::random::<u128>()));
        let first = HostLease::try_acquire(&directory, "lease")
            .unwrap()
            .unwrap();
        assert!(HostLease::try_acquire(&directory, "lease")
            .unwrap()
            .is_none());
        drop(first);
        assert!(HostLease::try_acquire(&directory, "lease")
            .unwrap()
            .is_some());
        fs::remove_dir_all(directory).unwrap();
    }
}
