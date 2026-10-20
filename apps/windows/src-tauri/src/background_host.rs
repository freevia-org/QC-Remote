//! The background MCP services (loopback MCP listener, remote relay,
//! prompts, `tray-host-status.json`). Since 0.1.178 they run inside
//! `qc-device-broker.exe`, supervised by the broker's resident host; that
//! process forbids broker spawning, so the device adapter below only ever
//! attaches to the broker it lives in. This entry never constructs a Tauri
//! application.
use crate::{host_presence, local_access, local_stdio, relay};
use qc_relay_client::{Client, ConnectionState, Credentials, DeviceAdapter};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    original: Mutex<qc_native_platform::thread_owner::Owned>,
    presence: Arc<Mutex<Option<serde_json::Value>>>,
}
static HOST_BUDGET: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(4);
impl BackgroundMcpHost {
    pub fn finished(&self) -> bool {
        match self.original.try_lock() {
            Ok(mut original) => original.finished(),
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner().finished(),
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
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
        let original = self
            .original
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        original.wait_within(within);
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
    // Reserve before runtime creation starts any native pool workers.
    let admission = HOST_BUDGET.reserve()?;
    let lease = HostLease::try_acquire(&directory, "background-mcp-host.lock")?
        .ok_or("Another process already owns the background MCP host")?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let (stop, receiver) = watch::channel(false);
    let presence = Arc::new(Mutex::new(None));
    let view = presence.clone();
    let original = qc_native_platform::thread_owner::Owned::spawn_reserved(
        "qc-background-mcp-host",
        admission,
        lease,
        move || {
            // The broker and the USB worker share this process: a panic in the
            // host must end only the host, and must still report it finished
            // so the broker's supervisor restarts it.
            let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(run(directory, host_token, readiness, receiver, view));
                // Runtime Drop retains started blocking work until it ends.
                // This wait is inside the owned host original, never a GUI
                // finished()/Drop call. A stall keeps the lease and permit.
                drop(runtime);
            }));
            // Runtime teardown above has retired this host's async tasks.
            // Release their acknowledged relay leases before reporting;
            // diagnostic output must not strand a completed original.
            crate::relay_task::reap();
            if served.is_err() {
                let _ = std::panic::catch_unwind(|| {
                    eprintln!("The background MCP host panicked; the broker restarts it")
                });
            }
        },
    )?;
    Ok(BackgroundMcpHost {
        stop,
        original: Mutex::new(original),
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
    task: crate::relay_task::Owned,
    stop: watch::Sender<bool>,
    credential: String,
    state: watch::Sender<ConnectionState>,
}
impl Drop for BackgroundRelay {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        self.state.send_replace(ConnectionState::Stopped);
        // Owned's Drop retains original task+lease+permit until actual join.
    }
}

/// At most one active and one stopping local listener. Do not admit a
/// successor while the previous original is still in native TLS teardown.
/// On unwind, the owned background thread waits for these exact originals;
/// its outer bounded stop can refuse without losing lease or attribution.
#[derive(Default)]
struct BackgroundListeners {
    active: Option<local_access::RunningLocalAccess>,
    retiring: Vec<local_access::RunningLocalAccess>,
}
impl BackgroundListeners {
    fn stop_active(&mut self) {
        if let Some(old) = self.active.take() {
            old.stop();
            self.retiring.push(old);
        }
    }
    fn reap(&mut self) {
        let mut index = 0;
        while index < self.retiring.len() {
            if self.retiring[index].native_finished() {
                let _ = self.retiring.swap_remove(index).join_original();
            } else {
                index += 1;
            }
        }
    }
}
impl Drop for BackgroundListeners {
    fn drop(&mut self) {
        self.stop_active();
        for old in self.retiring.drain(..) {
            let _ = old.join_original();
        }
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
    .with_access_policy_directory(directory.clone())
    // With no MCP client attached the gateway is detached; /health and the
    // relay then answer from this process's broker, which a readiness read
    // never opens, reopens or joins.
    .with_broker_readiness(readiness);
    let adapter: Arc<dyn DeviceAdapter> = Arc::new(device.clone());
    let (access, access_rx) = watch::channel(qc_relay_client::AccessMode::ReadOnly);
    let mut credential_snapshot = None;
    let mut listener = BackgroundListeners::default();
    let mut relay_host: Option<BackgroundRelay> = None;
    let mut last_settings = Instant::now() - Duration::from_secs(5);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut maintenance: Option<tokio::task::JoinHandle<()>> = None;
    let (credential_tx, mut credential_rx) =
        watch::channel(None::<Result<Option<Credentials>, String>>);
    let mut refresh_stop = stop.clone();
    let refresh_access = access.clone();
    let credential_refresh = tokio::spawn(async move {
        let mut refresh = tokio::time::interval(Duration::from_secs(5));
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = refresh_stop.changed() => {
                    if changed.is_err() || *refresh_stop.borrow() { break; }
                }
                _ = refresh.tick() => {}
            }
            let previous = *refresh_access.borrow();
            let mode = tokio::select! {
                changed = refresh_stop.changed() => {
                    if changed.is_err() || *refresh_stop.borrow() { break; }
                    continue;
                }
                mode = relay::refresh_access_mode(previous) => mode,
            };
            refresh_access.send_replace(mode);
            let credentials = tokio::select! {
                changed = refresh_stop.changed() => {
                    if changed.is_err() || *refresh_stop.borrow() { break; }
                    continue;
                }
                credentials = relay::load_credentials() => credentials,
            };
            credential_tx.send_replace(Some(credentials));
        }
    });
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
            changed = credential_rx.changed() => {
                if changed.is_err() { break; }
                match credential_rx.borrow_and_update().clone() {
                    Some(Ok(credentials)) => credential_snapshot = credentials,
                    Some(Err(error)) => eprintln!("QC Remote background host could not refresh pairing: {error}"),
                    None => {}
                }
                last_settings = Instant::now() - Duration::from_secs(5);
            }
        }
        if *stop.borrow()
            || !crate::broker_tray_settings::broker_tray_settings_in(&directory).tray_enabled
        {
            break;
        }
        let local_enabled = local_access::local_mcp_enabled_in(&directory);
        listener.reap();
        crate::relay_task::reap();
        let remote_enabled = crate::access_policy_store::effective_remote_enabled(&directory);
        if !remote_enabled && relay_host.take().is_some() {
            crate::remote_grants::clear();
            host_presence::set_remote_endpoint(None);
            host_presence::set_remote_state(watch::channel(ConnectionState::Stopped).1);
        }
        if !local_enabled {
            listener.stop_active();
        } else if listener.active.is_none() && listener.retiring.is_empty() {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let paths = publication_paths(&directory, None);
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let paths =
                publication_paths(&directory, std::env::var_os("APPDATA").map(PathBuf::from));
            // The existing publisher guard authenticates any GUI owner. It
            // never displaces that owner; after GUI exit the next tick wins.
            listener.active =
                local_access::start_listener(paths, adapter.clone(), access_rx.clone()).ok();
        }
        if last_settings.elapsed() >= Duration::from_secs(5) {
            last_settings = Instant::now();
            let credential = credential_snapshot
                .as_ref()
                .filter(|c| c.uses_current_identity_provider())
                .cloned();
            host_presence::set_remote_endpoint(if remote_enabled {
                credential.as_ref().map(|c| c.endpoint.as_str())
            } else {
                None
            });
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
            if remote_enabled && relay_host.is_none() {
                if let Some(credentials) = credential {
                    let permit = match crate::relay_task::Permit::acquire() {
                        Ok(permit) => permit,
                        Err(error) => {
                            eprintln!("Background relay admission refused: {error}");
                            continue;
                        }
                    };
                    if let Ok(Some(lease)) =
                        HostLease::try_acquire(&directory, "remote-mcp-host.lock")
                    {
                        // Serialize credential refresh and publication against
                        // pair/unpair. Release this before the long-lived run;
                        // the per-invocation latch check closes the remaining
                        // race after publication.
                        let operation = match HostLease::try_acquire(
                            &directory,
                            "relay-pairing-operation.lock",
                        ) {
                            Ok(Some(operation)) => operation,
                            Ok(None) => continue,
                            Err(error) => {
                                eprintln!("Could not acquire relay pairing lease: {error}");
                                continue;
                            }
                        };
                        let (state, state_rx) = watch::channel(ConnectionState::Stopped);
                        host_presence::set_remote_state(state_rx);
                        let (halt, halt_rx) = watch::channel(false);
                        let (presence, presence_rx) = watch::channel(None);
                        host_presence::set_remote(presence_rx);
                        // No remote method waits for a host-side approval
                        // (HOST_APPROVAL_METHODS is empty): the user
                        // confirms once, in the AI app that asked.
                        let connection_state = state.clone();
                        let host_adapter = adapter.clone();
                        let policy = access_rx.clone();
                        let task = tokio::spawn(async move {
                            let Ok(credentials) =
                                relay::begin_credential_connection(credentials).await
                            else {
                                return;
                            };
                            if !host_adapter.remote_session_enabled() {
                                return;
                            }
                            let client = Client::new(
                                credentials,
                                host_adapter,
                                connection_state,
                                policy,
                                halt_rx,
                            )
                            .with_gateway_api_version(
                                quadcortex::device::generated_gateway::API_VERSION,
                            )
                            .with_mcp_presence(presence)
                            .with_credential_status(Arc::new(relay::record_credential_status))
                            .with_replacement_ack(Arc::new(relay::acknowledge_replacement));
                            drop(operation);
                            client.run().await;
                        });
                        relay_host = Some(BackgroundRelay {
                            task: crate::relay_task::Owned::new(
                                tauri::async_runtime::JoinHandle::Tokio(task),
                                lease,
                                permit,
                            ),
                            stop: halt,
                            credential: identity,
                            state,
                        });
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
            if let Some(previous) = linked_apps_fetch.take() {
                let _ = previous.await;
            }
            linked_apps_fetch = Some(tokio::spawn(crate::remote_grants::refresh_on_connect(
                || host_presence::remote_state() == ConnectionState::Connected,
            )));
        }
        if relay_host.is_none() {
            crate::remote_grants::clear();
        }
        relay_was_connected = relay_connected;
        let endpoint = listener.active.as_ref().map(|running| running.endpoint());
        let document = local_access::host_status_document(
            endpoint,
            listener
                .active
                .as_ref()
                .map(|running| running.instance_id()),
        );
        let snapshot = host_presence::snapshot(None);
        if maintenance.as_ref().is_none_or(|task| task.is_finished()) {
            if let Some(previous) = maintenance.take() {
                let _ = previous.await;
            }
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
        *view.lock().unwrap_or_else(|p| p.into_inner()) = if listener.active.is_some()
            || !local_enabled
        {
            if let Err(error) = crate::host_runtime_status::publish(&directory, document.clone()) {
                eprintln!("Could not publish tray host status: {error}");
            }
            Some(document)
        } else {
            None
        };
    }
    credential_refresh.abort();
    let _ = credential_refresh.await;
    if let Some(fetch) = linked_apps_fetch.take() {
        fetch.abort();
        let _ = fetch.await;
    }
    drop(relay_host);
    host_presence::set_remote_endpoint(None);
    device.stop();
    drop(listener); // Exact local original joins stay inside owned host.
    if let Some(maintenance) = maintenance.take() {
        // A started blocking maintenance operation cannot be force-cancelled.
        // Keep the host original/lease while waiting for its real outcome.
        let _ = maintenance.await;
    }
    crate::relay_task::reap();
    crate::host_runtime_status::remove(&directory);
    *view.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_reacquire_host_lease_until(
        directory: &Path,
        timeout: Duration,
    ) -> Result<Option<HostLease>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(lease) = HostLease::try_acquire(directory, "host")? {
                return Ok(Some(lease));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn bounded_host_lease_reacquire_waits_for_a_held_lease_to_drop() {
        let directory = crate::tests::unique_fixture_directory("bounded-host-lease");
        let root = crate::tests::RemoveDirOnDrop::new(directory.clone());
        let lease = HostLease::try_acquire(&directory, "host").unwrap().unwrap();
        assert!(
            try_reacquire_host_lease_until(&directory, Duration::from_millis(50))
                .unwrap()
                .is_none()
        );
        drop(lease);
        let reacquired =
            try_reacquire_host_lease_until(&directory, Duration::from_secs(2)).unwrap();
        assert!(reacquired.is_some());
        drop(reacquired);
        drop(root);
        assert!(!directory.exists());
    }
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
    fn a_background_host_keeps_its_lease_until_the_exact_native_tls_original_retires() {
        use std::cell::RefCell;
        use std::sync::mpsc;
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Tls(mpsc::Sender<()>, mpsc::Receiver<()>);
        impl Drop for Tls {
            fn drop(&mut self) {
                let _ = self.0.send(());
                let _ = self.1.recv_timeout(Duration::from_secs(30));
            }
        }
        thread_local! { static TLS: RefCell<Option<Tls>> = const { RefCell::new(None) }; }
        struct Release(Option<mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let directory = crate::tests::unique_fixture_directory("background-original-tls");
        let root = Arc::new(crate::tests::RemoveDirOnDrop::new(directory.clone()));
        let lease = HostLease::try_acquire(&directory, "host").unwrap().unwrap();
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release));
        let original = qc_native_platform::thread_owner::Owned::spawn(
            "background-tls-fixture",
            &BUDGET,
            (lease, root.clone()),
            move || {
                TLS.with(|slot| *slot.borrow_mut() = Some(Tls(entered, release_rx)));
            },
        )
        .unwrap();
        let (stop, _receiver) = watch::channel(false);
        let mut host = BackgroundMcpHost {
            stop,
            original: Mutex::new(original),
            presence: Arc::new(Mutex::new(None)),
        };
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!host.finished());
        host.stop_within(Duration::ZERO);
        assert!(!host.finished());
        assert!(HostLease::try_acquire(&directory, "host")
            .unwrap()
            .is_none());
        drop(host);
        qc_native_platform::thread_owner::reap();
        assert_eq!(BUDGET.count(), 1);
        assert!(HostLease::try_acquire(&directory, "host")
            .unwrap()
            .is_none());
        drop(release);
        let until = Instant::now() + Duration::from_secs(2);
        while BUDGET.count() != 0 {
            qc_native_platform::thread_owner::reap();
            assert!(
                Instant::now() < until,
                "native TLS fixture original did not retire"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(HostLease::try_acquire(&directory, "host")
            .unwrap()
            .is_some());
        drop(root);
        assert!(!directory.exists());
    }

    #[test]
    fn background_runtime_teardown_keeps_the_host_lease_while_started_blocking_work_is_alive() {
        use std::sync::mpsc;
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Release(Option<mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let directory = crate::tests::unique_fixture_directory("background-runtime-drop");
        let root = Arc::new(crate::tests::RemoveDirOnDrop::new(directory.clone()));
        let lease = HostLease::try_acquire(&directory, "host").unwrap().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release));
        let original = qc_native_platform::thread_owner::Owned::spawn(
            "background-runtime-fixture",
            &BUDGET,
            (lease, root.clone()),
            move || {
                let _started = runtime.spawn_blocking(move || {
                    let _ = entered.send(());
                    let _ = release_rx.recv_timeout(Duration::from_secs(30));
                });
                drop(runtime);
            },
        )
        .unwrap();
        let (stop, _) = watch::channel(false);
        let mut host = BackgroundMcpHost {
            stop,
            original: Mutex::new(original),
            presence: Arc::new(Mutex::new(None)),
        };
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        host.stop_within(Duration::from_millis(10));
        assert!(!host.finished());
        assert!(HostLease::try_acquire(&directory, "host")
            .unwrap()
            .is_none());
        drop(release);
        host.stop_within(Duration::from_secs(2));
        assert!(host.finished());
        // A concurrently forked pre-exec child can briefly retain the flock's
        // file description after the original actor has actually retired.
        // Observe OS lock release without weakening either retirement check.
        let reacquired =
            try_reacquire_host_lease_until(&directory, Duration::from_secs(2)).unwrap();
        assert!(reacquired.is_some());
        drop(reacquired);
        drop(host);
        drop(root);
        assert!(!directory.exists());
    }

    #[test]
    fn an_aborted_background_relay_keeps_the_lease_until_cancelled_future_drop_finishes() {
        let _serial = crate::relay_task::fixture_lock();
        use std::sync::mpsc;
        struct DropGate(mpsc::Sender<()>, mpsc::Receiver<()>);
        impl Drop for DropGate {
            fn drop(&mut self) {
                let _ = self.0.send(());
                let _ = self.1.recv_timeout(Duration::from_secs(30));
            }
        }
        struct Release(Option<mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let directory = crate::tests::unique_fixture_directory("background-relay-cancel");
        let root = Arc::new(crate::tests::RemoveDirOnDrop::new(directory.clone()));
        let lease = HostLease::try_acquire(&directory, "relay")
            .unwrap()
            .unwrap();
        let permit = crate::relay_task::Permit::acquire().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (polled, polled_rx) = mpsc::channel();
        let (dropping, dropping_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release));
        let task = runtime.spawn(async move {
            let _gate = DropGate(dropping, release_rx);
            let _ = polled.send(());
            std::future::pending::<()>().await;
        });
        let (stop, _) = watch::channel(false);
        let (state, _) = watch::channel(ConnectionState::Stopped);
        let relay = BackgroundRelay {
            task: crate::relay_task::Owned::new(
                tauri::async_runtime::JoinHandle::Tokio(task),
                (lease, root.clone()),
                permit,
            ),
            stop,
            credential: "fixture-only".into(),
            state,
        };
        polled_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(relay);
        dropping_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        crate::relay_task::reap();
        assert!(HostLease::try_acquire(&directory, "relay")
            .unwrap()
            .is_none());
        drop(release);
        drop(runtime);
        crate::relay_task::reap();
        assert!(HostLease::try_acquire(&directory, "relay")
            .unwrap()
            .is_some());
        drop(root);
        assert!(!directory.exists());
    }

    #[test]
    fn publisher_lease_is_exclusive_and_released_with_owner() {
        let directory = crate::tests::unique_fixture_directory("mcp-lease");
        let _cleanup = crate::tests::RemoveDirOnDrop::new(directory.clone());
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
    }
}
