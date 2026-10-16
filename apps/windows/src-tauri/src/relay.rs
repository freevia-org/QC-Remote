use qc_relay_client::{
    begin_host_pairing, pair_replacing, poll_host_pairing, revoke, AccessMode, Client,
    ConnectionState, Credentials, DeviceAdapter, HostPairingSession, PairedDevice, PairingMetadata,
    RemoteConnector, Usage,
};
use serde::Serialize;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

const CREDENTIAL_SERVICE: &str = "QC Remote";
const LEGACY_CREDENTIAL_SERVICE: &str = "QC Control";
const CREDENTIAL_ACCOUNT: &str = "outbound-public-relay-v1";
const PENDING_REVOCATIONS_ACCOUNT: &str = "outbound-public-relay-pending-revocations-v1";
const WITHDRAWAL_TIMEOUT: Duration = Duration::from_secs(20);
const PENDING_WITHDRAWAL_DETAIL: &str =
    "This computer is unpaired locally. Cloud withdrawal is pending; retry withdrawal when online.";
const ACCESS_MODE_ACCOUNT: &str = "outbound-public-relay-access-v1";
const AUTO_APPROVE_REMOTE_ACTIONS_ACCOUNT: &str = "outbound-public-relay-auto-approve-actions-v1";
/// The name this computer registers under, and declares when it opens a
/// browser pairing session so the relay's confirmation page can show it.
pub(crate) const RELAY_DEVICE_NAME: &str = if cfg!(target_os = "linux") {
    "QC Remote on Linux"
} else if cfg!(target_os = "macos") {
    "QC Remote on macOS"
} else if cfg!(target_os = "windows") {
    "QC Remote on Windows"
} else {
    "QC Remote"
};
const RELAY_PLATFORM: &str = if cfg!(target_os = "linux") {
    "linux"
} else if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(target_os = "windows") {
    "windows"
} else {
    "unknown"
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    pub paired: bool,
    /// Whether this installation is currently allowed to open the outbound
    /// relay session. Credentials can remain stored after a failed keyring
    /// clear; the persisted latch is authoritative in that case.
    pub remote_enabled: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending_revocation: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_repair: bool,
    pub state: ConnectionState,
    pub access_mode: AccessMode,
    pub endpoint: Option<String>,
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing: Option<PairingMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug)]
pub struct BrowserPairingOffer {
    pub pair_url: String,
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPairingProgress {
    pub state: String,
    pub status: Option<RelayStatus>,
}

#[derive(Default)]
struct BrowserPairingState {
    generation: u64,
    session: Option<HostPairingSession>,
}

impl BrowserPairingState {
    fn invalidate(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.session = None;
        self.generation
    }

    fn publish(&mut self, generation: u64, session: HostPairingSession) -> bool {
        if self.generation != generation {
            return false;
        }
        self.session = Some(session);
        true
    }

    fn claim(&mut self, generation: u64) -> bool {
        self.generation == generation && self.session.take().is_some()
    }
}

pub struct RelayBridge {
    lifecycle: Mutex<()>,
    lifecycle_generation: AtomicU64,
    task: Mutex<Option<crate::relay_task::Owned>>,
    stop: Mutex<Option<watch::Sender<bool>>>,
    state: Mutex<watch::Receiver<ConnectionState>>,
    access: watch::Sender<AccessMode>,
    access_generation: Arc<AtomicU64>,
    access_write: tokio::sync::Mutex<()>,
    credential_cache: Mutex<Option<Option<Credentials>>>,
    credential_generation: Arc<AtomicU64>,
    credential_cache_updated_at: Mutex<Option<Instant>>,
    credential_refresh_pending: Arc<std::sync::atomic::AtomicBool>,
    start_pending: std::sync::atomic::AtomicBool,
    storage_ready: std::sync::atomic::AtomicBool,
    reconcile_pending: Arc<std::sync::atomic::AtomicBool>,
    last_error: Mutex<Option<String>>,
    pending_browser_pairing: Arc<Mutex<BrowserPairingState>>,
    pending_revocation: std::sync::atomic::AtomicBool,
}

impl Default for RelayBridge {
    fn default() -> Self {
        let (_, state) = watch::channel(ConnectionState::Stopped);
        let (access, _) = watch::channel(AccessMode::ReadOnly);
        Self {
            lifecycle: Mutex::new(()),
            lifecycle_generation: AtomicU64::new(0),
            task: Mutex::new(None),
            stop: Mutex::new(None),
            state: Mutex::new(state),
            access,
            access_generation: Arc::new(AtomicU64::new(0)),
            access_write: tokio::sync::Mutex::new(()),
            credential_cache: Mutex::new(None),
            credential_generation: Arc::new(AtomicU64::new(0)),
            credential_cache_updated_at: Mutex::new(None),
            credential_refresh_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            start_pending: std::sync::atomic::AtomicBool::new(false),
            storage_ready: std::sync::atomic::AtomicBool::new(false),
            reconcile_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_error: Mutex::new(None),
            pending_browser_pairing: Arc::new(Mutex::new(BrowserPairingState::default())),
            pending_revocation: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl RelayBridge {
    pub(crate) fn schedule_reconcile(app: tauri::AppHandle, delegated: bool) {
        use tauri::Manager;
        let pending = app.state::<Self>().reconcile_pending.clone();
        if pending.swap(true, Ordering::AcqRel) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            use tauri::Manager;
            app.state::<Self>()
                .reconcile_ownership(delegated, || crate::relay_adapter(&app))
                .await;
            pending.store(false, Ordering::Release);
        });
    }

    pub(crate) async fn reconcile_ownership(
        &self,
        delegated: bool,
        adapter: impl FnOnce() -> Arc<dyn DeviceAdapter>,
    ) {
        // The canonical owner-only policy file is authoritative across GUI,
        // delegated, and headless processes. Keep this process's status/watch
        // view current too; request admission independently reads the file.
        let policy = effective_access_mode();
        if *self.access.borrow() != policy {
            self.access.send_replace(policy);
        }
        if !self.storage_ready.load(Ordering::Acquire) {
            let _ = self.refresh_credentials_cache().await;
            self.storage_ready.store(true, Ordering::Release);
        } else {
            let stale = self
                .credential_cache_updated_at
                .lock()
                .ok()
                .and_then(|time| *time)
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(2));
            if stale {
                let _ = self.refresh_credentials_cache().await;
            }
        }
        if delegated {
            self.stop();
            return;
        }
        let running = self.task.lock().map(|task| task.is_some()).unwrap_or(false);
        if !running && self.status().is_ok_and(|status| status.paired) {
            if let Err(error) = self.start(adapter()).await {
                if let Ok(mut detail) = self.last_error.lock() {
                    *detail = Some(error);
                }
            }
        }
    }
    /// What the relay last said about the credential this host holds.
    ///
    /// `PairingRequired` is the relay having refused it outright - a 401 or 403
    /// on the socket upgrade - rather than any transport failure, which lands in
    /// `Reconnecting`.
    fn connection_state(&self) -> Result<ConnectionState, String> {
        if let Some(external) = crate::external_host::status() {
            if let Ok(state) = serde_json::from_value(external["remoteState"].clone()) {
                return Ok(state);
            }
        }
        Ok(*self
            .state
            .lock()
            .map_err(|_| "Relay state lock was poisoned")?
            .borrow())
    }

    pub fn status(&self) -> Result<RelayStatus, String> {
        let credential = self
            .credential_cache
            .lock()
            .map_err(|_| "Relay credential cache lock was poisoned")?
            .clone()
            .flatten();
        let needs_repair = credential
            .as_ref()
            .is_some_and(|value| !value.uses_current_identity_provider());
        let state = self.connection_state()?;
        let remote_enabled =
            crate::access_policy_store::effective_remote_enabled(&access_policy_directory());
        let pending_revocation = self.pending_revocation.load(Ordering::Acquire);
        Ok(RelayStatus {
            paired: credential.is_some() && !needs_repair,
            remote_enabled,
            pending_revocation,
            needs_repair,
            state,
            access_mode: *self.access.borrow(),
            endpoint: credential.as_ref().map(|value| value.endpoint.clone()),
            device_id: credential.as_ref().map(|value| value.device_id.clone()),
            pairing: credential.as_ref().and_then(|value| value.pairing.clone()),
            detail: if needs_repair {
                Some(
                    "Remote access now uses Google sign-in. Pair this device again to continue."
                        .into(),
                )
            } else if pending_revocation {
                Some(if credential.is_none() {
                    PENDING_WITHDRAWAL_DETAIL.into()
                } else {
                    "A previous cloud authorization still needs to be withdrawn; retry withdrawal when online.".into()
                })
            } else if !remote_enabled && credential.is_some() {
                Some("This computer is unpaired for remote use. Pair it again to enable the remote route.".into())
            } else {
                self.last_error.lock().ok().and_then(|value| value.clone())
            },
        })
    }

    fn cache_credentials(&self, credentials: Option<Credentials>) -> Result<(), String> {
        *self
            .credential_cache
            .lock()
            .map_err(|_| "Relay credential cache lock was poisoned")? = Some(credentials);
        *self
            .credential_cache_updated_at
            .lock()
            .map_err(|_| "Relay credential cache timestamp lock was poisoned")? =
            Some(Instant::now());
        Ok(())
    }

    async fn refresh_credentials_cache(&self) -> Result<(), String> {
        use std::sync::atomic::AtomicBool;
        if self.credential_refresh_pending.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reset = Reset(&self.credential_refresh_pending);
        let generation = self.credential_generation.load(Ordering::Acquire);
        *self
            .credential_cache_updated_at
            .lock()
            .map_err(|_| "Relay credential cache timestamp lock was poisoned")? =
            Some(Instant::now());
        let credentials = load_credentials().await?;
        let pending_revocations = load_pending_revocations_async().await?;
        if self.credential_generation.load(Ordering::Acquire) == generation {
            self.cache_credentials(credentials)?;
            self.pending_revocation
                .store(!pending_revocations.is_empty(), Ordering::Release);
        }
        Ok(())
    }

    pub(crate) async fn initialize(&self, adapter: Arc<dyn DeviceAdapter>) {
        self.restore_access_mode();
        if let Err(error) = self.refresh_credentials_cache().await {
            if let Ok(mut detail) = self.last_error.lock() {
                *detail = Some(error);
            }
        }
        self.storage_ready.store(true, Ordering::Release);
        if self.status().is_ok_and(|status| status.paired) {
            if let Err(error) = self.start(adapter).await {
                if let Ok(mut detail) = self.last_error.lock() {
                    *detail = Some(error);
                }
            }
        }
    }

    pub async fn usage(&self) -> Result<Usage, String> {
        let credentials = load_credentials()
            .await?
            .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
        qc_relay_client::usage(&credentials).await
    }

    /// Every device paired to this account.
    ///
    /// Empty rather than an error when this computer is not paired, and empty
    /// rather than an error against a relay too old to answer: the panel reads
    /// this whenever it is open, including before anything has been paired,
    /// and it already holds this computer's own pairing to fall back on. A
    /// failure that a person could act on - a credential the relay has
    /// forgotten - is still reported.
    pub async fn roster(&self) -> Result<Vec<PairedDevice>, String> {
        let Some(credentials) = load_credentials().await? else {
            return Ok(Vec::new());
        };
        match qc_relay_client::roster(&credentials).await {
            Ok(devices) => Ok(devices),
            Err(reason) if reason == qc_relay_client::ROSTER_UNSUPPORTED => Ok(Vec::new()),
            Err(reason) => Err(reason),
        }
    }

    pub async fn connectors(&self) -> Result<Vec<RemoteConnector>, String> {
        let credentials = load_credentials()
            .await?
            .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
        qc_relay_client::connectors(&credentials).await
    }

    pub async fn unlink_connector(&self, grant_id: &str) -> Result<Vec<RemoteConnector>, String> {
        let (credentials, _) = revoke_connector_grant(grant_id).await?;
        qc_relay_client::connectors(&credentials).await
    }

    /// Unpair one of the other devices on this account and report what is left.
    ///
    /// This computer is refused here on purpose: retiring itself also has to
    /// clear the local credential store, which `unpair` does and this does not.
    pub async fn unpair_device(&self, device_id: &str) -> Result<Vec<PairedDevice>, String> {
        let credentials = load_credentials()
            .await?
            .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
        qc_relay_client::revoke_paired_device(&credentials, device_id).await?;
        self.roster().await
    }

    pub async fn start(&self, adapter: Arc<dyn DeviceAdapter>) -> Result<(), String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        self.start_inner(adapter).await
    }

    /// `pair`, browser pairing, and the public start command all serialize on
    /// `relay-pairing-operation.lock`; callers that already hold it use this
    /// inner entrypoint. The lease ends after Client publication, never when
    /// the long-lived Client task ends.
    async fn start_inner(&self, adapter: Arc<dyn DeviceAdapter>) -> Result<(), String> {
        if self.start_pending.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        struct Reset<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reset = Reset(&self.start_pending);
        if crate::external_host::stopping() {
            return Ok(());
        }
        let lifecycle_generation = self.lifecycle_generation.load(Ordering::Acquire);
        let generation = self.credential_generation.load(Ordering::Acquire);
        let credentials = match load_credentials()
            .await?
            .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())
        {
            Ok(value) => value,
            Err(error) => {
                if let Ok(mut detail) = self.last_error.lock() {
                    *detail = Some(error.clone());
                }
                return Err(error);
            }
        };
        if self.credential_generation.load(Ordering::Acquire) != generation {
            return Err("Pairing changed while the relay was starting; retry the start".into());
        }
        self.cache_credentials(Some(credentials.clone()))?;
        if !credentials.uses_current_identity_provider() {
            let error =
                "Remote access now uses Google sign-in. Pair this device again to continue."
                    .to_string();
            if let Ok(mut detail) = self.last_error.lock() {
                *detail = Some(error.clone());
            }
            return Err(error);
        }
        let directory =
            crate::local_stdio::app_local_data_dir().ok_or("Local app directory is unavailable")?;
        if crate::external_host::delegated(&directory) {
            return Ok(());
        }
        if !crate::access_policy_store::effective_remote_enabled(&directory) {
            return Err("Remote hosting is disabled. Pair this device again to enable it.".into());
        }
        let permit = crate::relay_task::Permit::acquire()?;
        let lease =
            crate::background_host::HostLease::try_acquire(&directory, "remote-mcp-host.lock")?
                .ok_or(
                    "Remote MCP is hosted by the background tray or a previous host is retiring",
                )?;
        let credentials = begin_credential_connection(credentials).await?;
        self.publish_start_if_current(lifecycle_generation, generation, &directory, move || {
            self.stop_inner();
            let (state_tx, state_rx) = watch::channel(ConnectionState::Stopped);
            crate::host_presence::set_remote_state(state_rx.clone());
            let (stop_tx, stop_rx) = watch::channel(false);
            *self
                .state
                .lock()
                .map_err(|_| "Relay state lock was poisoned")? = state_rx;
            *self
                .stop
                .lock()
                .map_err(|_| "Relay stop lock was poisoned")? = Some(stop_tx);
            let (presence_tx, presence_rx) = watch::channel(None);
            crate::host_presence::set_remote(presence_rx);
            let client = Client::new(
                credentials,
                adapter,
                state_tx,
                self.access.subscribe(),
                stop_rx,
            )
            .with_gateway_api_version(quadcortex::device::generated_gateway::API_VERSION)
            .with_mcp_presence(presence_tx)
            .with_credential_status(Arc::new(record_credential_status))
            .with_replacement_ack(Arc::new(acknowledge_replacement));
            let task = tauri::async_runtime::spawn(client.run());
            let task = crate::relay_task::Owned::new(task, lease, permit);
            *self
                .task
                .lock()
                .map_err(|_| "Relay task lock was poisoned")? = Some(task);
            if let Ok(mut detail) = self.last_error.lock() {
                *detail = None;
            }
            Ok(())
        })
    }

    fn publish_start_if_current(
        &self,
        lifecycle_generation: u64,
        credential_generation: u64,
        directory: &std::path::Path,
        publish: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "Relay lifecycle lock was poisoned")?;
        if self.lifecycle_generation.load(Ordering::Acquire) != lifecycle_generation
            || self.credential_generation.load(Ordering::Acquire) != credential_generation
            || crate::external_host::stopping()
            || crate::external_host::delegated(directory)
        {
            return Err("Relay startup was superseded before the host could start".into());
        }
        publish()
    }

    pub fn stop(&self) {
        if let Ok(_lifecycle) = self.lifecycle.lock() {
            self.lifecycle_generation.fetch_add(1, Ordering::AcqRel);
            self.stop_inner();
        }
    }

    fn stop_inner(&self) {
        if let Ok(mut stop) = self.stop.lock() {
            if let Some(sender) = stop.take() {
                sender.send_replace(true);
            }
        }
        if let Ok(mut task) = self.task.lock() {
            if let Some(handle) = task.take() {
                handle.retire();
            }
        }
        crate::relay_task::reap();
        if let Ok(mut state) = self.state.lock() {
            *state = watch::channel(ConnectionState::Stopped).1;
        }
    }

    pub async fn set_access_mode(&self, mode: &str) -> Result<AccessMode, String> {
        let mode = AccessMode::parse(mode)?;
        self.persist_access_mode(mode, store_access_mode).await?;
        Ok(mode)
    }

    async fn persist_access_mode<F, Fut>(&self, mode: AccessMode, persist: F) -> Result<(), String>
    where
        F: FnOnce(AccessMode) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        // Serialize same-process writes before choosing a generation. Without
        // this lock, an older Full write could publish after a newer ReadOnly
        // write even though the older command reports itself superseded.
        let _write = self.access_write.lock().await;
        self.access_generation.fetch_add(1, Ordering::AcqRel);
        persist(mode).await?;
        self.access.send_replace(mode);
        Ok(())
    }

    pub fn restore_access_mode(&self) {
        let generation = self.access_generation.load(Ordering::Acquire);
        let current = self.access_generation.clone();
        let access = self.access.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match initialize_access_policy().await {
                    Ok(record) => {
                        if current.load(Ordering::Acquire) == generation {
                            access.send_replace(record.mode);
                        }
                        return;
                    }
                    Err(_) if current.load(Ordering::Acquire) == generation => {
                        access.send_replace(AccessMode::ReadOnly);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Err(_) => return,
                }
            }
        });
    }

    pub fn access_receiver(&self) -> watch::Receiver<AccessMode> {
        self.access.subscribe()
    }

    pub async fn pair(
        &self,
        endpoint: &str,
        pairing_code: &str,
        device_name: &str,
        adapter: Arc<dyn DeviceAdapter>,
    ) -> Result<RelayStatus, String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        self.credential_generation.fetch_add(1, Ordering::AcqRel);
        let held = load_credentials().await?;
        let credentials =
            pair_replacing(endpoint, pairing_code, device_name, held.as_ref()).await?;
        adopt_credentials(credentials, held.as_ref()).await?;
        set_remote_session_enabled(true).await?;
        self.start_inner(adapter).await?;
        self.status()
    }

    pub async fn begin_browser_pairing(
        &self,
        endpoint: &str,
    ) -> Result<BrowserPairingOffer, String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let generation = self.try_pairing_state()?.invalidate();
        let session = begin_host_pairing(endpoint, RELAY_DEVICE_NAME, RELAY_PLATFORM).await?;
        let offer = BrowserPairingOffer {
            pair_url: session.pair_url.clone(),
            expires_at: session.expires_at,
        };
        if !self.try_pairing_state()?.publish(generation, session) {
            return Err("Browser pairing was cancelled or replaced".into());
        }
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = Some("Finish secure sign-in in your browser".into());
        }
        Ok(offer)
    }

    pub async fn poll_browser_pairing(
        &self,
        device_name: &str,
        adapter: Arc<dyn DeviceAdapter>,
    ) -> Result<BrowserPairingProgress, String> {
        let (generation, session) = {
            let pending = self.try_pairing_state()?;
            (
                pending.generation,
                pending
                    .session
                    .clone()
                    .ok_or("Start browser pairing first")?,
            )
        };
        if session.expires_at < now_millis() {
            let mut pending = self.try_pairing_state()?;
            if pending.generation == generation {
                pending.invalidate();
            }
            return Err(
                "The browser pairing session expired. Start it again from QC Remote.".into(),
            );
        }
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let held = load_credentials().await?;
        let Some(code) = poll_host_pairing(&session.endpoint, &session).await? else {
            if !self.browser_pairing_is_pending(generation)? {
                return Err("Browser pairing was canceled before sign-in completed".into());
            }
            return Ok(BrowserPairingProgress {
                state: "waiting".into(),
                status: None,
            });
        };
        // Claim makes redemption one-shot, not uncancellable. The owned
        // credential worker checks this generation again at the store commit;
        // a cancelled mint is retired through the durable withdrawal journal.
        if !self.claim_browser_pairing_redemption(generation)? {
            return Err("Browser pairing was canceled before its code could be redeemed".into());
        }
        // Redeeming a code is one-shot, including an ambiguous transport failure.
        self.credential_generation.fetch_add(1, Ordering::AcqRel);
        let credentials =
            pair_replacing(&session.endpoint, &code, device_name, held.as_ref()).await?;
        adopt_credentials_bound(
            credentials,
            held.as_ref(),
            Some((self.pending_browser_pairing.clone(), generation)),
        )
        .await?;
        set_remote_session_enabled(true).await?;
        self.start_inner(adapter).await?;
        Ok(BrowserPairingProgress {
            state: "paired".into(),
            status: Some(self.status()?),
        })
    }

    fn try_pairing_state(&self) -> Result<std::sync::MutexGuard<'_, BrowserPairingState>, String> {
        self.pending_browser_pairing.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => "Browser pairing is committing or busy. Try again after the current operation finishes; it has not been cancelled.".into(),
            std::sync::TryLockError::Poisoned(_) => "Browser pairing lock was poisoned; no pairing was changed".into(),
        })
    }

    pub fn cancel_browser_pairing(&self) -> Result<(), String> {
        self.invalidate_pending_browser_pairing()?;
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = None;
        }
        Ok(())
    }

    fn browser_pairing_is_pending(&self, generation: u64) -> Result<bool, String> {
        let pending = self.try_pairing_state()?;
        Ok(pending.generation == generation && pending.session.is_some())
    }

    fn claim_browser_pairing_redemption(&self, generation: u64) -> Result<bool, String> {
        Ok(self.try_pairing_state()?.claim(generation))
    }

    fn invalidate_pending_browser_pairing(&self) -> Result<(), String> {
        self.try_pairing_state()?.invalidate();
        Ok(())
    }

    /// Disable request admission before retaining and retiring cloud receipts.
    pub async fn unpair(&self) -> Result<RelayStatus, String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let expected = load_credentials().await?;
        self.credential_generation.fetch_add(1, Ordering::AcqRel);
        self.stop();
        set_remote_session_enabled(false).await?;
        self.invalidate_pending_browser_pairing()?;
        let pending = prepare_withdrawal(true, expected).await?;
        self.cache_credentials(None)?;
        self.pending_revocation
            .store(!pending.is_empty(), Ordering::Release);
        let remains = retire_journal(pending).await?;
        self.pending_revocation.store(remains, Ordering::Release);
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = remains.then(|| PENDING_WITHDRAWAL_DETAIL.into());
        }
        self.status()
    }

    /// Retry only withdrawn receipts; never clear or stop a new active pairing.
    pub async fn retry_withdrawal(&self) -> Result<RelayStatus, String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let pending = prepare_withdrawal(false, None).await?;
        let remains = retire_journal(pending).await?;
        self.pending_revocation.store(remains, Ordering::Release);
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = remains.then(|| PENDING_WITHDRAWAL_DETAIL.into());
        }
        self.status()
    }
}

/// Journal before removing active access, then retire each credential once.
/// A failed or interrupted request leaves the journal retryable; revocation
/// accepts already-withdrawn credentials, so ambiguous outcomes are safe to retry.
#[cfg(test)]
async fn withdraw_pairing<S, C, R, RF>(
    stored: Option<Credentials>,
    mut pending: Vec<Credentials>,
    mut save: S,
    clear_active: C,
    mut revoke_one: R,
    timeout: Duration,
) -> Result<bool, String>
where
    S: FnMut(&[Credentials]) -> Result<(), String>,
    C: FnOnce() -> Result<(), String>,
    R: FnMut(Credentials) -> RF,
    RF: Future<Output = Result<(), String>>,
{
    let mut arriving = stored;
    while let Some(mut credential) = arriving {
        arriving = credential.previous_credential.take().map(|value| *value);
        if !pending
            .iter()
            .any(|entry| same_credential(Some(entry), Some(&credential)))
        {
            pending.push(credential);
        }
    }
    save(&pending)?;
    clear_active()?;
    let deadline = tokio::time::Instant::now() + timeout;
    while let Some(credential) = pending.first().cloned() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero()
            || !matches!(
                tokio::time::timeout(remaining, revoke_one(credential)).await,
                Ok(Ok(()))
            )
        {
            return Ok(true);
        }
        pending.remove(0);
        save(&pending)?;
    }
    Ok(false)
}

fn pending_revocations_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, PENDING_REVOCATIONS_ACCOUNT)
        .map_err(|_| "The system credential store is unavailable".to_string())
}

fn load_pending_revocations() -> Result<Vec<Credentials>, String> {
    match pending_revocations_entry()?.get_password() {
        Ok(value) => serde_json::from_str(&value)
            .map_err(|_| "The pending cloud withdrawal record is invalid".into()),
        Err(keyring::Error::NoEntry) => Ok(Vec::new()),
        Err(_) => Err("Could not read the pending cloud withdrawal record".into()),
    }
}

fn store_pending_revocations(entries: &[Credentials]) -> Result<(), String> {
    let entry = pending_revocations_entry()?;
    if entries.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Could not clear the pending cloud withdrawal record".into()),
        };
    }
    let encoded =
        serde_json::to_string(entries).map_err(|_| "Could not encode pending cloud withdrawals")?;
    entry
        .set_password(&encoded)
        .map_err(|_| "Could not store pending cloud withdrawals".into())
}

fn now_millis() -> u64 {
    quadcortex::time::unix_time_ms()
}

impl Drop for RelayBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

fn credential_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
        .map_err(|_| "The system credential store is unavailable".to_string())
}

fn legacy_credential_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(LEGACY_CREDENTIAL_SERVICE, CREDENTIAL_ACCOUNT)
        .map_err(|_| "The system credential store is unavailable".to_string())
}

fn access_mode_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, ACCESS_MODE_ACCOUNT)
        .map_err(|_| "The system credential store is unavailable".to_string())
}

fn legacy_access_mode_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(LEGACY_CREDENTIAL_SERVICE, ACCESS_MODE_ACCOUNT)
        .map_err(|_| "The system credential store is unavailable".to_string())
}

fn auto_approve_remote_actions_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, AUTO_APPROVE_REMOTE_ACTIONS_ACCOUNT)
        .map_err(|_| "The system credential store is unavailable".to_string())
}

fn clear_auto_approve_remote_actions_sync() -> Result<(), String> {
    match auto_approve_remote_actions_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(
            "Could not clear the remote approval preference from the system credential store"
                .into(),
        ),
    }
}

fn read_access_mode_value_sync(
    store: crate::access_policy::Store,
) -> Result<Option<String>, String> {
    let entry = match store {
        crate::access_policy::Store::Current => access_mode_entry(),
        crate::access_policy::Store::Legacy => legacy_access_mode_entry(),
    }?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => {
            Err("Could not read the relay access mode from the system credential store".into())
        }
    }
}

fn read_access_mode_sync() -> Result<Option<AccessMode>, String> {
    crate::access_policy::read_with(read_access_mode_value_sync)
}

fn access_policy_directory() -> std::path::PathBuf {
    crate::broker_paths::app_local_data_dir()
}

async fn set_remote_session_enabled(enabled: bool) -> Result<(), String> {
    let directory = access_policy_directory();
    tokio::task::spawn_blocking(move || {
        crate::access_policy_store::update_remote_enabled(&directory, enabled).map(|_| ())
    })
    .await
    .map_err(|error| format!("Could not save the remote-session state: {error}"))?
}

async fn initialize_access_policy() -> Result<crate::access_policy_store::Record, String> {
    let directory = access_policy_directory();
    let checked_directory = directory.clone();
    let existing =
        tokio::task::spawn_blocking(move || crate::access_policy_store::read(&checked_directory))
            .await
            .map_err(|error| format!("Could not inspect the relay access policy: {error}"))??;
    if let Some(record) = existing {
        return Ok(record);
    }

    // No policy has ever been migrated. Only this one-time path consults the
    // legacy keyring preference; all serving reads use the canonical file.
    // Keyring errors leave policy absent (and therefore ReadOnly) for a retry.
    let migrated = crate::credential_io::shared()?
        .read(Duration::from_secs(3), read_access_mode_sync)
        .await?
        .unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        crate::access_policy_store::initialize_if_absent(&directory, migrated)
    })
    .await
    .map_err(|error| format!("Could not migrate the relay access policy: {error}"))?
}

pub(crate) async fn load_access_mode_initial() -> AccessMode {
    initialize_access_policy()
        .await
        .map(|record| record.mode)
        .unwrap_or(AccessMode::ReadOnly)
}

pub(crate) async fn refresh_access_mode(_previous: AccessMode) -> AccessMode {
    load_access_mode_initial().await
}

pub(crate) fn effective_access_mode() -> AccessMode {
    crate::access_policy_store::effective_mode(&access_policy_directory())
}

async fn store_access_mode(mode: AccessMode) -> Result<(), String> {
    let directory = access_policy_directory();
    tokio::task::spawn_blocking(move || crate::access_policy_store::write(&directory, mode))
        .await
        .map_err(|error| format!("Could not save the relay access policy: {error}"))?
        .map(|_| ())
}

/// Revoke one AI app's OAuth grant with this computer's device credential:
/// the Remote panel's Unlink and the broker icon's remote Unlink both end
/// here. Ends that app's authorization only; the account and this pairing are
/// untouched. Returns the credential used, for a caller that reads on, and
/// what the relay said (a 404 is not a confirmed revoke).
pub(crate) async fn revoke_connector_grant(
    grant_id: &str,
) -> Result<(Credentials, qc_relay_client::ConnectorRevoke), String> {
    let credentials = load_credentials()
        .await?
        .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
    let outcome = qc_relay_client::revoke_connector(&credentials, grant_id).await?;
    Ok((credentials, outcome))
}

pub(crate) async fn load_credentials() -> Result<Option<Credentials>, String> {
    crate::credential_io::shared()?
        .read(Duration::from_secs(3), load_credentials_sync)
        .await
}

fn load_credentials_sync() -> Result<Option<Credentials>, String> {
    match credential_entry()?.get_password() {
        Ok(value) => serde_json::from_str(&value).map(Some).map_err(|_| {
            "The stored public-relay credential is invalid; unpair and pair again".into()
        }),
        Err(keyring::Error::NoEntry) => match legacy_credential_entry()?.get_password() {
            Ok(value) => serde_json::from_str(&value).map(Some).map_err(|_| {
                "The stored public-relay credential is invalid; unpair and pair again".into()
            }),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(
                "Could not read the public-relay credential from the system credential store"
                    .into(),
            ),
        },
        Err(_) => Err(
            "Could not read the public-relay credential from the system credential store".into(),
        ),
    }
}

/// Why this host will not take on an arriving credential, if it will not.
///
/// Pairing was "last sign-in wins". The relay mints its offer for whichever
/// Google account completes the sign-in, and this host stored whatever came
/// back without ever comparing it to what it already held - so a pairing link
/// that reached the wrong person was enough to move the Quad Cortex onto their
/// account, and the previous owner's record was left orphaned under an account
/// they no longer controlled.
///
/// Changing hands is now something done deliberately at this machine: unpair,
/// then pair. A held pairing with missing owner metadata is refused until
/// backfilled or explicitly unpaired. Only exact held-credential rejection
/// evidence permits the shared bypass.
fn owner_change_refusal(
    current: Option<&PairingMetadata>,
    arriving: Option<&PairingMetadata>,
    current_rejected: bool,
    already_paired: bool,
) -> Option<String> {
    let held = current.map_or("", |value| value.email.as_str());
    let incoming = arriving.map_or("", |value| value.email.as_str());
    qc_relay_client::pairing_owner_refusal(already_paired, held, incoming, current_rejected)
        .map(|code| format!("{code}: This computer's current pairing cannot be replaced by this account receipt. Unpair it here before pairing again."))
}

/// Same identity, regardless of a concurrently recorded rejection annotation.
fn same_credential(left: Option<&Credentials>, right: Option<&Credentials>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.endpoint == b.endpoint
                && a.device_id == b.device_id
                && a.device_credential == b.device_credential
        }
        _ => false,
    }
}

fn credential_lease(name: &str) -> Result<crate::background_host::HostLease, String> {
    let directory =
        crate::local_stdio::app_local_data_dir().ok_or("Local app directory is unavailable")?;
    crate::background_host::HostLease::try_acquire(&directory, name)?.ok_or_else(|| {
        "A pairing credential operation is already in progress; no credential was changed".into()
    })
}

/// The callback receives the precise Client identity that got a connect rejection.
/// A stale socket cannot mark a newly adopted credential. A busy store fails closed.
pub(crate) async fn begin_credential_connection(
    expected: Credentials,
) -> Result<Credentials, String> {
    crate::credential_io::shared()?
        .write(move || {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            let held = load_credentials_sync()?;
            if !same_credential(held.as_ref(), Some(&expected)) {
                return Err(
                    "Pairing changed before relay connection; no connection started".into(),
                );
            }
            let mut current = held.ok_or("Pairing disappeared before relay connection")?;
            current.connection_generation = current
                .connection_generation
                .checked_add(1)
                .ok_or("Relay connection generation exhausted; reconnect refused")?;
            store_credentials_sync(&current)?;
            Ok(current)
        })
        .await
}

fn same_connection(expected: &Credentials, held: &Credentials) -> bool {
    same_credential(Some(expected), Some(held))
        && expected.connection_generation == held.connection_generation
}

pub(crate) fn acknowledge_replacement(expected: &Credentials, previous_device_id: &str) {
    let expected = expected.clone();
    let previous_device_id = previous_device_id.to_owned();
    let result = crate::credential_io::shared().and_then(|worker| {
        worker.try_write(move || {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            let mut held = match load_credentials_sync()? {
                Some(held) => held,
                None => return Ok(()),
            };
            if same_connection(&expected, &held)
                && held
                    .previous_credential
                    .as_ref()
                    .is_some_and(|previous| previous.device_id == previous_device_id)
            {
                retain_older_replacement_receipts(
                    &mut held,
                    load_pending_revocations,
                    store_pending_revocations,
                )?;
                store_credentials_sync(&held)?;
            }
            Ok(())
        })
    });
    if let Err(error) = result {
        eprintln!("QC Remote could not queue replacement acknowledgement: {error}");
    }
}

pub(crate) fn record_credential_status(expected: &Credentials, rejected: bool) {
    let expected = expected.clone();
    let result = crate::credential_io::shared().and_then(|worker| {
        worker.try_write(move || {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            let mut held = match load_credentials_sync()? {
                Some(held) => held,
                None => return Ok(()),
            };
            if same_connection(&expected, &held) {
                held.credential_rejected = rejected;
                store_credentials_sync(&held)?;
            }
            Ok(())
        })
    });
    if let Err(error) = result {
        eprintln!("QC Remote could not queue credential status update: {error}");
    }
}

async fn adopt_credentials(
    incoming: Credentials,
    expected: Option<&Credentials>,
) -> Result<(), String> {
    adopt_credentials_bound(incoming, expected, None).await
}

type PairingFence = (Arc<Mutex<BrowserPairingState>>, u64);

async fn adopt_credentials_bound(
    incoming: Credentials,
    expected: Option<&Credentials>,
    fence: Option<PairingFence>,
) -> Result<(), String> {
    let expected = expected.cloned();
    let mut stored = incoming.clone();
    // Keep the complete outcome, including worker admission/read/store errors.
    // Every failure after minting must retire the incoming-only capability.
    let adoption = match crate::credential_io::shared() {
        Err(error) => Err(error),
        Ok(worker) => {
            worker
                .write(move || {
                    with_pairing_fence(fence.as_ref(), || {
                        store_adopted_credentials(&mut stored, expected.as_ref())
                    })
                })
                .await
        }
    };
    finish_adoption(incoming, adoption).await
}

fn with_pairing_fence<F>(fence: Option<&PairingFence>, commit: F) -> Result<Option<String>, String>
where
    F: FnOnce() -> Result<Option<String>, String>,
{
    let pending = fence
        .map(|(state, _)| {
            state
                .lock()
                .map_err(|_| "Browser pairing lock was poisoned".to_owned())
        })
        .transpose()?;
    if let (Some((_, generation)), Some(pending)) = (fence, pending.as_ref()) {
        if pending.generation != *generation {
            return Ok(Some("Browser pairing was cancelled or replaced".into()));
        }
    }
    // This guard lives through the synchronous store commit inside the owned
    // worker. No MutexGuard crosses an await; Cancel shares this exact lock.
    commit()
}

fn store_adopted_credentials(
    incoming: &mut Credentials,
    expected: Option<&Credentials>,
) -> Result<Option<String>, String> {
    let _mutation = credential_lease("relay-credential-state.lock")?;
    let held = load_credentials_sync()?;
    if !same_credential(held.as_ref(), expected) {
        return Ok(Some(
            "Pairing changed while the new receipt was pending; no credential was overwritten"
                .into(),
        ));
    }
    let refusal = owner_change_refusal(
        held.as_ref().and_then(|value| value.pairing.as_ref()),
        incoming.pairing.as_ref(),
        held.as_ref().is_some_and(|value| value.credential_rejected),
        held.is_some(),
    );
    if refusal.is_none() {
        incoming.previous_credential = replacement_predecessor(held)?;
        store_credentials_sync(incoming)?;
    }
    Ok(refusal)
}

fn replacement_predecessor(held: Option<Credentials>) -> Result<Option<Box<Credentials>>, String> {
    // Bound serialized nesting without ever dropping an unacknowledged receipt.
    // The active pairing stays unchanged if its history needs withdrawal first.
    let mut cursor = held.as_ref();
    let mut depth = 0;
    while let Some(previous) = cursor {
        depth += 1;
        if depth > 32 {
            return Err(
                "Pairing repair history needs withdrawal; unpair before pairing again.".into(),
            );
        }
        cursor = previous.previous_credential.as_deref();
    }
    Ok(held.map(Box::new))
}

/// The relay acknowledges only the immediate predecessor, not its ancestors.
/// Journal every older receipt before removing its withdrawal capability from
/// active state. An interrupted journal/store sequence is idempotently retryable.
fn retain_older_replacement_receipts<L, S>(
    held: &mut Credentials,
    load: L,
    save: S,
) -> Result<(), String>
where
    L: FnOnce() -> Result<Vec<Credentials>, String>,
    S: FnOnce(&[Credentials]) -> Result<(), String>,
{
    let mut arriving = held
        .previous_credential
        .as_ref()
        .and_then(|previous| previous.previous_credential.as_deref())
        .cloned();
    if arriving.is_some() {
        let mut pending = load()?;
        while let Some(mut credential) = arriving {
            arriving = credential.previous_credential.take().map(|value| *value);
            if !pending
                .iter()
                .any(|entry| same_credential(Some(entry), Some(&credential)))
            {
                pending.push(credential);
            }
        }
        save(&pending)?;
    }
    held.previous_credential = None;
    Ok(())
}

async fn finish_adoption(
    incoming: Credentials,
    adoption: Result<Option<String>, String>,
) -> Result<(), String> {
    finish_adoption_with(incoming, adoption, retire_refused_credential).await
}

async fn finish_adoption_with<R, RF>(
    mut incoming: Credentials,
    adoption: Result<Option<String>, String>,
    retire: R,
) -> Result<(), String>
where
    R: FnOnce(Credentials) -> RF,
    RF: Future<Output = Result<(), String>>,
{
    let reason = match adoption {
        Ok(None) => return Ok(()),
        Ok(Some(reason)) | Err(reason) => reason,
    };
    incoming.previous_credential = None;
    match retire(incoming).await {
        Ok(()) => Err(reason),
        Err(withdrawal) => Err(format!("{reason}. {withdrawal}")),
    }
}

async fn retire_refused_credential(incoming: Credentials) -> Result<(), String> {
    let recorded = update_pending_receipt(incoming.clone(), true).await;
    let withdrawal = tokio::time::timeout(WITHDRAWAL_TIMEOUT, revoke(&incoming)).await;
    match withdrawal {
        Ok(Ok(())) if recorded.is_ok() => update_pending_receipt(incoming, false).await.map(|_| ()),
        Ok(Ok(())) => Ok(()),
        failed => {
            let reason = match failed {
                Err(_) => "Cloud withdrawal timed out".to_owned(),
                Ok(Err(reason)) => reason,
                _ => unreachable!(),
            };
            Err(match recorded {
                Ok(_) => format!("{reason}; cloud withdrawal remains pending"),
                Err(store) => format!("{reason}; the withdrawal could not be recorded ({store})"),
            })
        }
    }
}

// The production journal operations run on the serialized credential worker.
// Each mutation reloads and merges under the cross-process state lease.
async fn update_pending_receipt(credential: Credentials, keep: bool) -> Result<bool, String> {
    crate::credential_io::shared()?
        .write(move || {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            let mut entries = load_pending_revocations()?;
            entries.retain(|entry| !same_credential(Some(entry), Some(&credential)));
            if keep {
                entries.push(credential);
            }
            store_pending_revocations(&entries)?;
            Ok(!entries.is_empty())
        })
        .await
}

async fn prepare_withdrawal(
    clear: bool,
    expected: Option<Credentials>,
) -> Result<Vec<Credentials>, String> {
    crate::credential_io::shared()?
        .write(move || {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            prepare_withdrawal_with(
                clear,
                expected.as_ref(),
                load_credentials_sync,
                load_pending_revocations,
                store_pending_revocations,
                clear_credentials_sync,
            )
        })
        .await
}

fn prepare_withdrawal_with<L, P, S, C>(
    clear: bool,
    expected: Option<&Credentials>,
    load: L,
    pending: P,
    save: S,
    clear_active: C,
) -> Result<Vec<Credentials>, String>
where
    L: FnOnce() -> Result<Option<Credentials>, String>,
    P: FnOnce() -> Result<Vec<Credentials>, String>,
    S: FnOnce(&[Credentials]) -> Result<(), String>,
    C: FnOnce() -> Result<(), String>,
{
    let mut pending = pending()?;
    if clear {
        let mut arriving = load()?;
        // The async operation lease can retire when its caller is dropped.
        // Compare inside this worker's state lease before journaling or clearing,
        // so a replacement adopted by another process is never withdrawn.
        if !same_credential(arriving.as_ref(), expected) {
            return Err(
                "Pairing changed while unpairing; the replacement was left untouched".into(),
            );
        }
        while let Some(mut credential) = arriving {
            arriving = credential.previous_credential.take().map(|value| *value);
            if !pending
                .iter()
                .any(|entry| same_credential(Some(entry), Some(&credential)))
            {
                pending.push(credential);
            }
        }
        // The production caller holds one lease through journaling and clear.
        save(&pending)?;
        clear_active()?;
    }
    Ok(pending)
}

async fn retire_journal(pending: Vec<Credentials>) -> Result<bool, String> {
    for credential in pending {
        if matches!(
            tokio::time::timeout(WITHDRAWAL_TIMEOUT, revoke(&credential)).await,
            Ok(Ok(()))
        ) {
            update_pending_receipt(credential, false).await?;
        }
    }
    load_pending_revocations_async()
        .await
        .map(|entries| !entries.is_empty())
}

async fn load_pending_revocations_async() -> Result<Vec<Credentials>, String> {
    crate::credential_io::shared()?
        .read(Duration::from_secs(5), load_pending_revocations)
        .await
}

/// Unlike active unpairing, a newly minted receipt may be retired directly if
/// the credential store is unavailable: it was never the active local route.
#[cfg(test)]
async fn retire_unadopted<J, R, RF>(
    credential: Credentials,
    mut journal: J,
    revoke_one: R,
    timeout: Duration,
) -> Result<(), String>
where
    J: FnMut(&Credentials, bool) -> Result<(), String>,
    R: FnOnce(Credentials) -> RF,
    RF: Future<Output = Result<(), String>>,
{
    let recorded = journal(&credential, true);
    let withdrawal = tokio::time::timeout(timeout, revoke_one(credential.clone())).await;
    match withdrawal {
        Ok(Ok(())) if recorded.is_ok() => journal(&credential, false),
        Ok(Ok(())) => Ok(()),
        failed => {
            let reason = match failed {
                Err(_) => "Cloud withdrawal timed out".to_owned(),
                Ok(Err(reason)) => reason,
                Ok(Ok(())) => unreachable!(),
            };
            Err(match recorded {
                Ok(()) => format!("{reason}; cloud withdrawal remains pending"),
                Err(store) => format!("{reason}; the withdrawal could not be recorded ({store})"),
            })
        }
    }
}

fn store_credentials_sync(credentials: &Credentials) -> Result<(), String> {
    let encoded = serde_json::to_string(credentials)
        .map_err(|_| "Could not encode the public-relay credential")?;
    credential_entry()?.set_password(&encoded).map_err(|_| {
        "Could not store the public-relay credential in the system credential store".into()
    })
}

fn clear_credentials_sync() -> Result<(), String> {
    clear_auto_approve_remote_actions_sync()?;
    match credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(_) => {
            return Err(
                "Could not remove the public-relay credential from the system credential store"
                    .into(),
            );
        }
    }
    match legacy_credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(
            "Could not remove the legacy public-relay credential from the system credential store"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_access_updates_commit_in_serialized_admission_order() {
        use std::future::Future;
        use std::sync::atomic::AtomicBool;

        let bridge = Arc::new(RelayBridge::default());
        let committed = Arc::new(Mutex::new(Vec::new()));
        let first_committed = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();

        let first_bridge = bridge.clone();
        let first_log = committed.clone();
        let first_done = first_committed.clone();
        let first = tokio::spawn(async move {
            first_bridge
                .persist_access_mode(AccessMode::Full, move |mode| async move {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    first_log.lock().unwrap().push(mode);
                    first_done.store(true, Ordering::Release);
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();

        let second_log = committed.clone();
        let second = bridge.persist_access_mode(AccessMode::ReadOnly, move |mode| async move {
            second_log.lock().unwrap().push(mode);
            Ok(())
        });
        tokio::pin!(second);
        let second_poll =
            std::future::poll_fn(|cx| std::task::Poll::Ready(second.as_mut().poll(cx))).await;
        assert!(
            second_poll.is_pending(),
            "second update must be waiting behind the first"
        );
        assert!(!first_committed.load(Ordering::Acquire));
        assert!(committed.lock().unwrap().is_empty());

        let _ = release_tx.send(());
        first.await.unwrap().unwrap();
        second.await.unwrap();

        assert_eq!(
            *committed.lock().unwrap(),
            vec![AccessMode::Full, AccessMode::ReadOnly]
        );
        assert_eq!(*bridge.access.borrow(), AccessMode::ReadOnly);
    }

    #[tokio::test]
    async fn unpair_stop_during_blocked_start_prevents_client_publication() {
        use std::sync::atomic::AtomicUsize;

        let bridge = Arc::new(RelayBridge::default());
        let lifecycle_generation = bridge.lifecycle_generation.load(Ordering::Acquire);
        let credential_generation = bridge.credential_generation.load(Ordering::Acquire);
        let published = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let starting_bridge = bridge.clone();
        let start_published = published.clone();
        let directory = crate::tests::unique_fixture_directory("relay-start-race");
        let _directory_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
        crate::broker_tray_settings::set_tray_enabled_in(&directory, false)
            .expect("keep this process as the relay host");
        assert!(
            !crate::external_host::stopping(),
            "the startup fixture requires an active app lifecycle"
        );
        bridge
            .publish_start_if_current(
                lifecycle_generation,
                credential_generation,
                &directory,
                || {
                    published.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                },
            )
            .expect("current startup may publish");
        assert_eq!(published.load(Ordering::Acquire), 1);
        published.store(0, Ordering::Release);
        let startup_directory = directory.clone();
        let start = tokio::spawn(async move {
            // Represents the bounded credential connection transaction holding
            // startup before its final publication point.
            let _ = started_tx.send(());
            let _ = release_rx.await;
            starting_bridge.publish_start_if_current(
                lifecycle_generation,
                credential_generation,
                &startup_directory,
                move || {
                    start_published.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                },
            )
        });
        started_rx.await.unwrap();

        // Unpair invalidates local startup and stops before its network revoke.
        // A revoke failure does not undo that local stop or permit late publish.
        bridge.credential_generation.fetch_add(1, Ordering::AcqRel);
        bridge.stop();
        let _revoke_failure = Err::<(), _>("offline");
        let _ = release_tx.send(());

        assert!(start.await.unwrap().is_err());
        assert_eq!(published.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn cancel_during_browser_poll_prevents_receipt_adoption() {
        let bridge = Arc::new(RelayBridge::default());
        let generation = 1;
        let session = || HostPairingSession {
            endpoint: "https://pair.example.invalid".into(),
            session_id: "fixture-session".into(),
            receiver_secret: "fixture-secret".into(),
            pair_url: "https://pair.example.invalid/fixture".into(),
            expires_at: u64::MAX,
            protocol: "fixture".into(),
        };
        bridge.pending_browser_pairing.lock().unwrap().session = Some(HostPairingSession {
            endpoint: "https://pair.example.invalid".into(),
            session_id: "fixture-session".into(),
            receiver_secret: "fixture-secret".into(),
            pair_url: "https://pair.example.invalid/fixture".into(),
            expires_at: u64::MAX,
            protocol: "fixture".into(),
        });
        bridge.pending_browser_pairing.lock().unwrap().generation = generation;

        // Simulate a network poll that has started but whose returned code
        // has not yet reached the local one-shot redemption boundary.
        let (poll_started_tx, poll_started_rx) = tokio::sync::oneshot::channel();
        let (receipt_tx, receipt_rx) = tokio::sync::oneshot::channel();
        let polling_bridge = bridge.clone();
        let poll = tokio::spawn(async move {
            let _ = poll_started_tx.send(());
            receipt_rx.await.unwrap();
            polling_bridge
                .claim_browser_pairing_redemption(generation)
                .unwrap()
        });
        poll_started_rx.await.unwrap();

        bridge.cancel_browser_pairing().unwrap();
        let _ = receipt_tx.send(());

        assert!(!poll.await.unwrap());
        assert!(bridge
            .pending_browser_pairing
            .lock()
            .unwrap()
            .session
            .is_none());

        // A valid, current poll must be able to claim the adoption boundary,
        // and that one-shot claim cannot be reused.
        let current = RelayBridge::default();
        current.pending_browser_pairing.lock().unwrap().session = Some(session());
        current.pending_browser_pairing.lock().unwrap().generation = 7;
        assert!(current.claim_browser_pairing_redemption(7).unwrap());
        assert!(!current.claim_browser_pairing_redemption(7).unwrap());

        // A stale poll cannot clear a replacement session while refusing its
        // claim; the replacement remains available to its own generation.
        let replacement = RelayBridge::default();
        replacement.pending_browser_pairing.lock().unwrap().session = Some(session());
        replacement
            .pending_browser_pairing
            .lock()
            .unwrap()
            .generation = 9;
        assert!(!replacement.claim_browser_pairing_redemption(8).unwrap());
        assert!(replacement
            .pending_browser_pairing
            .lock()
            .unwrap()
            .session
            .is_some());
    }

    fn pairing_session(id: &str) -> HostPairingSession {
        HostPairingSession {
            endpoint: "https://qcrc.freevia.org".into(),
            session_id: id.into(),
            receiver_secret: "fixture-secret".into(),
            pair_url: "https://qcrc.freevia.org/pair".into(),
            expires_at: u64::MAX,
            protocol: "fixture".into(),
        }
    }

    #[test]
    fn a_cancelled_or_replaced_browser_flow_cannot_publish_or_consume_an_offer() {
        let mut state = BrowserPairingState::default();
        let old = state.invalidate();
        let current = state.invalidate();
        assert!(state.publish(current, pairing_session("new")));
        assert!(!state.publish(old, pairing_session("slow-old")));
        assert!(!state.claim(old));
        assert_eq!(state.session.as_ref().unwrap().session_id, "new");
        assert!(state.claim(current));
        assert!(
            !state.claim(current),
            "one-shot redemption cannot be attempted twice"
        );
        let cancelled = state.invalidate();
        state.invalidate();
        assert!(!state.publish(cancelled, pairing_session("cancelled")));
        assert!(!state.claim(cancelled));
        assert!(state.session.is_none());
    }

    fn withdrawal_credential(id: &str) -> Credentials {
        serde_json::from_value(serde_json::json!({
            "endpoint":"https://qcrc.freevia.org", "deviceId":id,
            "deviceCredential":"fixture-only-withdrawal-credential"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn inside_commit_stall_refuses_cancel_and_poll_without_blocking_callers() {
        struct NeverInvoke;
        #[async_trait::async_trait]
        impl DeviceAdapter for NeverInvoke {
            async fn ready(&self) -> bool {
                panic!("no device readiness probe")
            }
            async fn invoke(
                &self,
                _: &str,
                _: serde_json::Value,
            ) -> Result<serde_json::Value, qc_relay_client::DeviceError> {
                panic!("no device invocation")
            }
        }
        let bridge = Arc::new(RelayBridge::default());
        let generation = bridge.pending_browser_pairing.lock().unwrap().generation;
        let fence = (bridge.pending_browser_pairing.clone(), generation);
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            crate::credential_io::shared()
                .unwrap()
                .write(move || {
                    with_pairing_fence(Some(&fence), || {
                        let _ = started.send(());
                        waiting
                            .recv_timeout(Duration::from_secs(2))
                            .map_err(|error| error.to_string())?;
                        Ok(None)
                    })
                })
                .await
        });
        ready.await.unwrap();
        let before = Instant::now();
        let cancel = bridge.cancel_browser_pairing().unwrap_err();
        assert!(cancel.contains("has not been cancelled"));
        assert!(bridge
            .poll_browser_pairing("fixture", Arc::new(NeverInvoke))
            .await
            .unwrap_err()
            .contains("busy"));
        assert!(bridge
            .claim_browser_pairing_redemption(generation)
            .unwrap_err()
            .contains("busy"));
        assert!(
            before.elapsed() < Duration::from_millis(500),
            "no browser-state caller waits for started OS I/O"
        );
        let _ = release.send(());
        assert!(
            task.await.unwrap().unwrap().is_none(),
            "already-linearized healthy commit completes"
        );
        bridge.cancel_browser_pairing().unwrap();
        assert_ne!(
            bridge.pending_browser_pairing.lock().unwrap().generation,
            generation
        );
    }

    #[tokio::test]
    async fn credential_worker_post_mint_cancel_prevents_store_and_retires_incoming() {
        use std::sync::atomic::AtomicBool;
        let state = Arc::new(Mutex::new(BrowserPairingState::default()));
        let generation = state.lock().unwrap().invalidate();
        let fence = (state.clone(), generation);
        let committed = Arc::new(AtomicBool::new(false));
        let commit_flag = committed.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            crate::credential_io::shared()
                .unwrap()
                .write(move || {
                    let _ = started.send(());
                    waiting
                        .recv_timeout(Duration::from_secs(2))
                        .map_err(|error| error.to_string())?;
                    with_pairing_fence(Some(&fence), || {
                        commit_flag.store(true, Ordering::Release);
                        Ok(None)
                    })
                })
                .await
        });
        ready.await.unwrap();
        state.lock().unwrap().invalidate();
        let _ = release.send(());
        let adoption = task.await.unwrap();
        assert!(!committed.load(Ordering::Acquire));
        let error = finish_adoption_with(withdrawal_credential("minted"), adoption, |receipt| {
            assert_eq!(receipt.device_id, "minted");
            assert!(receipt.previous_credential.is_none());
            std::future::ready(Ok(()))
        })
        .await
        .unwrap_err();
        assert!(error.contains("cancelled or replaced"));
        let generation = state.lock().unwrap().invalidate();
        let fence = (state.clone(), generation);
        let healthy_commit = committed.clone();
        let adoption = crate::credential_io::shared()
            .unwrap()
            .write(move || {
                with_pairing_fence(Some(&fence), || {
                    assert!(
                        fence.0.try_lock().is_err(),
                        "generation mutex remains held through the complete commit"
                    );
                    healthy_commit.store(true, Ordering::Release);
                    Ok(None)
                })
            })
            .await;
        assert!(
            committed.load(Ordering::Acquire),
            "current receipt commits normally"
        );
        let retired = std::cell::Cell::new(false);
        finish_adoption_with(withdrawal_credential("healthy-current"), adoption, |_| {
            retired.set(true);
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();
        assert!(!retired.get(), "healthy adoption does not enter withdrawal");
    }

    #[tokio::test]
    async fn credential_worker_adoption_faults_retire_receipt_and_preserve_error() {
        for fault in ["keyring read failed", "keyring store failed"] {
            let adoption = crate::credential_io::shared()
                .unwrap()
                .write(move || with_pairing_fence(None, || Err(fault.into())))
                .await;
            let retired = Arc::new(Mutex::new(Vec::new()));
            let capture = retired.clone();
            let mut receipt = withdrawal_credential("minted");
            receipt.previous_credential = Some(Box::new(withdrawal_credential("healthy")));
            let error = finish_adoption_with(receipt, adoption, move |receipt| {
                assert!(receipt.previous_credential.is_none());
                capture.lock().unwrap().push(receipt.device_id);
                std::future::ready(Err("withdrawal pending".into()))
            })
            .await
            .unwrap_err();
            assert!(error.contains(fault));
            assert!(error.contains("withdrawal pending"));
            assert_eq!(*retired.lock().unwrap(), vec!["minted"]);
        }
    }

    #[tokio::test]
    async fn credential_worker_journal_failure_preserves_active_and_retry_skips_active_read() {
        use std::sync::atomic::AtomicBool;
        let cleared = Arc::new(AtomicBool::new(false));
        let capture = cleared.clone();
        let error = crate::credential_io::shared()
            .unwrap()
            .write(move || {
                prepare_withdrawal_with(
                    true,
                    Some(&withdrawal_credential("held")),
                    || Ok(Some(withdrawal_credential("held"))),
                    || Ok(Vec::new()),
                    |_| Err("journal write failed".into()),
                    || {
                        capture.store(true, Ordering::Release);
                        Ok(())
                    },
                )
            })
            .await
            .unwrap_err();
        assert_eq!(error, "journal write failed");
        assert!(!cleared.load(Ordering::Acquire));
        let receipts = crate::credential_io::shared()
            .unwrap()
            .write(|| {
                prepare_withdrawal_with(
                    false,
                    None,
                    || panic!("journal-only retry must not load new active pairing"),
                    || Ok(vec![withdrawal_credential("old")]),
                    |_| panic!("no stale full-journal write"),
                    || panic!("no active clear"),
                )
            })
            .await
            .unwrap();
        assert_eq!(receipts[0].device_id, "old");
    }

    #[test]
    fn withdrawal_preserves_a_replacement_pairing_before_journal_or_clear() {
        let old = withdrawal_credential("old");
        for expected in [Some(&old), None] {
            let replacement = withdrawal_credential("replacement");
            let error = prepare_withdrawal_with(
                true,
                expected,
                || Ok(Some(replacement.clone())),
                || Ok(vec![withdrawal_credential("pending")]),
                |_| panic!("a replacement must not be added to the withdrawal journal"),
                || panic!("a replacement must not be cleared"),
            )
            .unwrap_err();
            assert!(error.contains("replacement was left untouched"));
        }
    }

    #[test]
    fn withdrawal_journals_the_expected_pairing_chain_before_clearing() {
        use std::cell::RefCell;
        let mut expected = withdrawal_credential("current");
        expected.previous_credential = Some(Box::new(withdrawal_credential("previous")));
        let operations = RefCell::new(Vec::new());
        let receipts = prepare_withdrawal_with(
            true,
            Some(&expected),
            || Ok(Some(expected.clone())),
            || Ok(vec![withdrawal_credential("pending")]),
            |entries| {
                assert_eq!(entries.len(), 3);
                operations.borrow_mut().push("journal");
                Ok(())
            },
            || {
                operations.borrow_mut().push("clear");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*operations.borrow(), vec!["journal", "clear"]);
        assert_eq!(receipts[1].device_id, "current");
        assert_eq!(receipts[2].device_id, "previous");
    }

    #[tokio::test]
    async fn failed_adoption_retires_only_the_minted_receipt_and_preserves_the_store_error() {
        let mut incoming = withdrawal_credential("minted");
        incoming.previous_credential = Some(Box::new(withdrawal_credential("healthy")));
        let error = finish_adoption_with(incoming, Err("keyring write failed".into()), |retired| {
            assert_eq!(retired.device_id, "minted");
            assert!(
                retired.previous_credential.is_none(),
                "healthy active access is never retired on adoption failure"
            );
            std::future::ready(Err("withdrawal also failed".into()))
        })
        .await
        .unwrap_err();
        assert!(error.contains("keyring write failed"));
        assert!(error.contains("withdrawal also failed"));
    }

    #[tokio::test]
    async fn an_unadopted_receipt_is_withdrawn_even_when_its_journal_cannot_be_written() {
        use std::cell::Cell;
        let sent = Cell::new(false);
        retire_unadopted(
            withdrawal_credential("minted"),
            |_, _| Err("keyring unavailable".into()),
            |credential| {
                assert_eq!(credential.device_id, "minted");
                sent.set(true);
                std::future::ready(Ok(()))
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(sent.get());
    }

    #[tokio::test]
    async fn journal_only_retry_keeps_a_new_active_pairing() {
        use std::cell::RefCell;
        let active = withdrawal_credential("healthy");
        let journal = RefCell::new(vec![withdrawal_credential("older-refused")]);
        let pending = journal.borrow().clone();
        assert!(!withdraw_pairing(
            None,
            pending,
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
            || Ok(()),
            |credential| {
                assert_eq!(credential.device_id, "older-refused");
                std::future::ready(Ok(()))
            },
            Duration::from_secs(1)
        )
        .await
        .unwrap());
        assert_eq!(active.device_id, "healthy");
        assert!(journal.borrow().is_empty());
    }

    #[tokio::test]
    async fn offline_unpair_removes_active_access_and_retries_the_journal_after_restart() {
        use std::cell::RefCell;
        let active = RefCell::new(Some(withdrawal_credential("device-a")));
        let journal = RefCell::new(Vec::<Credentials>::new());
        let stored = active.borrow().clone();
        assert!(withdraw_pairing(
            stored,
            Vec::new(),
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
            || {
                active.borrow_mut().take();
                Ok(())
            },
            |_| {
                assert!(
                    active.borrow().is_none(),
                    "access is removed before the network call"
                );
                std::future::ready(Err("offline".into()))
            },
            Duration::from_secs(1)
        )
        .await
        .unwrap());
        assert!(active.borrow().is_none());
        // A fresh process sees the separate persisted journal, never an active
        // credential. A repeat unpair must still send the original capability.
        let encoded = serde_json::to_string(&*journal.borrow()).unwrap();
        let restored: Vec<Credentials> = serde_json::from_str(&encoded).unwrap();
        let retired = RefCell::new(Vec::new());
        assert!(!withdraw_pairing(
            None,
            restored,
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
            || Ok(()),
            |credential| {
                retired.borrow_mut().push(credential.device_id);
                std::future::ready(Ok(()))
            },
            Duration::from_secs(1)
        )
        .await
        .unwrap());
        assert_eq!(*retired.borrow(), vec!["device-a"]);
        assert!(journal.borrow().is_empty());
    }

    #[tokio::test]
    async fn partial_withdrawal_retains_the_unacknowledged_predecessor_only() {
        use std::cell::RefCell;
        let mut current = withdrawal_credential("new");
        current.previous_credential = Some(Box::new(withdrawal_credential("old")));
        let journal = RefCell::new(Vec::<Credentials>::new());
        assert!(withdraw_pairing(
            Some(current),
            Vec::new(),
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
            || Ok(()),
            |credential| std::future::ready(if credential.device_id == "new" {
                Ok(())
            } else {
                Err("unreachable".into())
            }),
            Duration::from_secs(1)
        )
        .await
        .unwrap());
        assert_eq!(journal.borrow().len(), 1);
        assert_eq!(journal.borrow()[0].device_id, "old");
        assert!(journal.borrow()[0].previous_credential.is_none());
    }

    #[test]
    fn replacement_repair_keeps_all_withdrawal_receipts_until_durable_ack_journaling() {
        use std::cell::RefCell;
        let mut middle = withdrawal_credential("middle");
        middle.previous_credential =
            replacement_predecessor(Some(withdrawal_credential("oldest"))).unwrap();
        let mut newest = withdrawal_credential("newest");
        newest.previous_credential = replacement_predecessor(Some(middle)).unwrap();
        assert_eq!(
            newest
                .previous_credential
                .as_ref()
                .unwrap()
                .previous_credential
                .as_ref()
                .unwrap()
                .device_id,
            "oldest"
        );
        let before = newest.clone();
        assert!(retain_older_replacement_receipts(
            &mut newest,
            || Ok(Vec::new()),
            |_| Err("journal unavailable".into())
        )
        .is_err());
        assert_eq!(
            serde_json::to_value(&newest).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        let journal = RefCell::new(vec![withdrawal_credential("unrelated")]);
        retain_older_replacement_receipts(
            &mut newest,
            || Ok(journal.borrow().clone()),
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
        )
        .unwrap();
        assert!(newest.previous_credential.is_none());
        assert_eq!(
            journal
                .borrow()
                .iter()
                .map(|entry| entry.device_id.as_str())
                .collect::<Vec<_>>(),
            vec!["unrelated", "oldest"]
        );
        // If journaling landed but the active store failed, replay must retain
        // the oldest capability exactly once, alongside unrelated withdrawals.
        let mut replay = before;
        retain_older_replacement_receipts(
            &mut replay,
            || Ok(journal.borrow().clone()),
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(journal.borrow().len(), 2);
        assert!(journal
            .borrow()
            .iter()
            .all(|entry| entry.previous_credential.is_none()));
    }

    #[test]
    fn replacement_repair_excessive_history_refuses_without_discarding_the_active_chain() {
        let mut held = withdrawal_credential("seed");
        for index in 0..32 {
            let mut next = withdrawal_credential(&format!("next-{index}"));
            next.previous_credential = Some(Box::new(held));
            held = next;
        }
        let retained = held.clone();
        assert!(replacement_predecessor(Some(held)).is_err());
        let mut cursor = Some(&retained);
        let mut count = 0;
        while let Some(credential) = cursor {
            count += 1;
            cursor = credential.previous_credential.as_deref();
        }
        assert_eq!(count, 33);
    }

    #[tokio::test]
    async fn a_failed_journal_write_never_discards_the_only_withdrawal_capability() {
        use std::cell::Cell;
        let cleared = Cell::new(false);
        let sent = Cell::new(false);
        assert!(withdraw_pairing(
            Some(withdrawal_credential("held")),
            Vec::new(),
            |_| Err("credential store unavailable".into()),
            || {
                cleared.set(true);
                Ok(())
            },
            |_| {
                sent.set(true);
                std::future::ready(Ok(()))
            },
            Duration::from_secs(1)
        )
        .await
        .is_err());
        assert!(!cleared.get());
        assert!(!sent.get());
    }

    #[tokio::test]
    async fn a_stalled_withdrawal_is_bounded_and_keeps_its_retry_capability() {
        use std::cell::RefCell;
        let journal = RefCell::new(Vec::<Credentials>::new());
        assert!(withdraw_pairing(
            Some(withdrawal_credential("stalled")),
            Vec::new(),
            |entries| {
                *journal.borrow_mut() = entries.to_vec();
                Ok(())
            },
            || Ok(()),
            |_| std::future::pending::<Result<(), String>>(),
            Duration::from_millis(1)
        )
        .await
        .unwrap());
        assert_eq!(journal.borrow()[0].device_id, "stalled");
    }

    #[test]
    fn credential_status_rejects_old_identity_and_old_connection_generation() {
        let held: Credentials = serde_json::from_value(serde_json::json!({
            "endpoint":"https://qcrc.freevia.org", "deviceId":"fixture-device",
            "deviceCredential":"fixture-only-credential", "connectionGeneration":2
        }))
        .unwrap();
        let mut old = held.clone();
        assert!(same_connection(&old, &held));
        old.connection_generation = 1;
        assert!(!same_connection(&old, &held));
        old.connection_generation = 2;
        old.device_credential = "different-fixture-credential".into();
        assert!(!same_connection(&old, &held));
    }

    #[test]
    fn a_browser_pairing_declares_this_computer_the_way_it_registers() {
        assert_eq!(
            qc_relay_client::host_pairing_request_body(RELAY_DEVICE_NAME, RELAY_PLATFORM),
            serde_json::json!({"label": RELAY_DEVICE_NAME, "platform": RELAY_PLATFORM})
        );
    }

    #[test]
    fn relay_starts_restricted_until_access_policy_is_loaded() {
        let bridge = RelayBridge::default();
        assert_eq!(*bridge.access.borrow(), AccessMode::ReadOnly);
        assert_eq!(
            AccessMode::parse("read-only").unwrap(),
            AccessMode::ReadOnly
        );
        assert_eq!(
            AccessMode::parse("performance").unwrap(),
            AccessMode::Performance
        );
        assert_eq!(AccessMode::parse("modify").unwrap(), AccessMode::Modify);
        assert!(AccessMode::parse("write-mostly").is_err());
    }

    fn receipt(email: &str) -> PairingMetadata {
        PairingMetadata {
            email: email.into(),
            device: "Chrome on Windows".into(),
            paired_at: 1,
        }
    }

    #[test]
    fn pairing_cannot_change_hands_without_unpairing_first() {
        let held = receipt("owner-account");

        assert_eq!(
            owner_change_refusal(None, Some(&receipt("owner-account")), false, false),
            None,
            "a host with nothing paired has nothing to protect"
        );
        assert!(owner_change_refusal(Some(&held), None, false, true)
            .unwrap()
            .contains("PAIRING_OWNER_UNVERIFIED"));
        assert!(owner_change_refusal(None, Some(&held), false, true)
            .unwrap()
            .contains("PAIRING_OWNER_UNKNOWN"));
        assert_eq!(
            owner_change_refusal(Some(&held), Some(&receipt("OWNER-ACCOUNT")), false, true),
            None,
            "the same account in different case is the same account"
        );

        let refusal =
            owner_change_refusal(Some(&held), Some(&receipt("stranger-account")), false, true)
                .expect("a different account must be refused");
        assert!(refusal.contains("PAIRING_OWNER_CHANGED"));
        assert!(refusal.contains("Unpair"), "and what to do about it");
    }

    #[test]
    fn a_revoked_host_may_be_paired_to_another_account_directly() {
        let held = receipt("owner-account");

        assert_eq!(
            owner_change_refusal(Some(&held), Some(&receipt("stranger-account")), true, true),
            None,
            "a credential the relay has refused protects nothing, so pairing to another account \
             must not first require pressing Unpair to discard it"
        );
        assert_eq!(
            owner_change_refusal(Some(&held), Some(&receipt("owner-account")), true, true),
            None,
            "and re-pairing to the same account after a revoke is the ordinary repair"
        );
    }

    #[test]
    fn pairing_display_state_is_not_credential_rejection_evidence() {
        for state in [
            ConnectionState::PairingRequired,
            ConnectionState::Reconnecting,
            ConnectionState::InvalidEndpoint,
        ] {
            assert!(
                owner_change_refusal(
                    Some(&receipt("owner")),
                    Some(&receipt("other")),
                    false,
                    true
                )
                .is_some(),
                "{state:?} never bypasses the credential evidence"
            );
        }
    }
}
