use qc_relay_client::{
    begin_host_pairing, pair_replacing, poll_host_pairing, revoke, AccessMode, Client,
    ConnectionState, Credentials, DeviceAdapter, HostPairingSession, PairedDevice, PairingMetadata,
    RemoteConnector, Usage,
};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

const CREDENTIAL_SERVICE: &str = "QC Remote";
const LEGACY_CREDENTIAL_SERVICE: &str = "QC Control";
const CREDENTIAL_ACCOUNT: &str = "outbound-public-relay-v1";
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

pub struct RelayBridge {
    lifecycle: Mutex<()>,
    lifecycle_generation: AtomicU64,
    lease: Mutex<Option<crate::background_host::HostLease>>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
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
    pending_browser_pairing: Mutex<Option<HostPairingSession>>,
    browser_pairing_generation: AtomicU64,
}

impl Default for RelayBridge {
    fn default() -> Self {
        let (_, state) = watch::channel(ConnectionState::Stopped);
        let (access, _) = watch::channel(AccessMode::ReadOnly);
        Self {
            lifecycle: Mutex::new(()),
            lifecycle_generation: AtomicU64::new(0),
            lease: Mutex::new(None),
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
            pending_browser_pairing: Mutex::new(None),
            browser_pairing_generation: AtomicU64::new(0),
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
        Ok(RelayStatus {
            paired: credential.is_some() && !needs_repair,
            remote_enabled,
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
        if self.credential_generation.load(Ordering::Acquire) == generation {
            self.cache_credentials(credentials)?;
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
        let lease =
            crate::background_host::HostLease::try_acquire(&directory, "remote-mcp-host.lock")?
                .ok_or("Remote MCP is hosted by the background tray")?;
        let credentials = begin_credential_connection(credentials).await?;
        self.publish_start_if_current(lifecycle_generation, generation, &directory, move || {
            self.stop_inner();
            *self
                .lease
                .lock()
                .map_err(|_| "Relay lease lock was poisoned")? = Some(lease);
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
                handle.abort();
            }
        }
        if let Ok(mut lease) = self.lease.lock() {
            lease.take();
        }
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
        // Starting another flow invalidates any poll already in flight. The
        // slot lock makes this generation change atomic with its pending value.
        let generation = {
            let mut pending = self
                .pending_browser_pairing
                .lock()
                .map_err(|_| "Browser pairing lock was poisoned")?;
            *pending = None;
            self.browser_pairing_generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1)
        };
        let session = begin_host_pairing(endpoint, RELAY_DEVICE_NAME, RELAY_PLATFORM).await?;
        let offer = BrowserPairingOffer {
            pair_url: session.pair_url.clone(),
            expires_at: session.expires_at,
        };
        let mut pending = self
            .pending_browser_pairing
            .lock()
            .map_err(|_| "Browser pairing lock was poisoned")?;
        if self.browser_pairing_generation.load(Ordering::Acquire) != generation {
            return Err("Browser pairing was canceled or replaced before it could start".into());
        }
        *pending = Some(session);
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
        let (session, generation) = {
            let pending = self
                .pending_browser_pairing
                .lock()
                .map_err(|_| "Browser pairing lock was poisoned")?;
            (
                pending.clone().ok_or("Start browser pairing first")?,
                self.browser_pairing_generation.load(Ordering::Acquire),
            )
        };
        if session.expires_at < now_millis() {
            self.discard_browser_pairing(generation);
            return Err(
                "The browser pairing session expired. Start it again from QC Remote.".into(),
            );
        }
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let held = load_credentials().await?;
        let Some(code) = poll_host_pairing(&session.endpoint, &session).await? else {
            if !self.browser_pairing_is_pending(generation) {
                return Err("Browser pairing was canceled before sign-in completed".into());
            }
            return Ok(BrowserPairingProgress {
                state: "waiting".into(),
                status: None,
            });
        };
        // This is the cancellation boundary for the one-shot redemption. Once
        // claimed, redemption and local adoption form one transaction; a later
        // Cancel cannot discard a receipt that may already have been created.
        if !self.claim_browser_pairing_redemption(generation) {
            return Err("Browser pairing was canceled before its code could be redeemed".into());
        }
        // Redeeming a code is one-shot, including an ambiguous transport failure.
        self.credential_generation.fetch_add(1, Ordering::AcqRel);
        let credentials =
            pair_replacing(&session.endpoint, &code, device_name, held.as_ref()).await?;
        adopt_credentials(credentials, held.as_ref()).await?;
        set_remote_session_enabled(true).await?;
        self.start_inner(adapter).await?;
        *self
            .pending_browser_pairing
            .lock()
            .map_err(|_| "Browser pairing lock was poisoned")? = None;
        Ok(BrowserPairingProgress {
            state: "paired".into(),
            status: Some(self.status()?),
        })
    }

    pub fn cancel_browser_pairing(&self) {
        self.invalidate_pending_browser_pairing();
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = None;
        }
    }

    fn browser_pairing_is_pending(&self, generation: u64) -> bool {
        self.pending_browser_pairing.lock().is_ok_and(|pending| {
            pending.is_some()
                && self.browser_pairing_generation.load(Ordering::Acquire) == generation
        })
    }

    fn claim_browser_pairing_redemption(&self, generation: u64) -> bool {
        let Ok(mut pending) = self.pending_browser_pairing.lock() else {
            return false;
        };
        if pending.is_none()
            || self.browser_pairing_generation.load(Ordering::Acquire) != generation
        {
            return false;
        }
        *pending = None;
        self.browser_pairing_generation
            .fetch_add(1, Ordering::AcqRel);
        true
    }

    fn discard_browser_pairing(&self, generation: u64) {
        if let Ok(mut pending) = self.pending_browser_pairing.lock() {
            if self.browser_pairing_generation.load(Ordering::Acquire) == generation {
                *pending = None;
                self.browser_pairing_generation
                    .fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    fn invalidate_pending_browser_pairing(&self) {
        if let Ok(mut pending) = self.pending_browser_pairing.lock() {
            *pending = None;
            self.browser_pairing_generation
                .fetch_add(1, Ordering::AcqRel);
        } else {
            // Poisoning must still prevent a poll already holding an older
            // token from crossing the adoption boundary.
            self.browser_pairing_generation
                .fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Disable this host locally before withdrawing the pairing at the relay.
    ///
    /// The persisted session latch blocks resident hosts before either network
    /// or keyring I/O begins. Clearing only the local store leaves the relay
    /// honouring a credential the user believes they have revoked. The local
    /// clear still happens if the relay cannot be reached, and the reason is
    /// surfaced so they can retry while the relay still holds it.
    pub async fn unpair(&self) -> Result<RelayStatus, String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        self.credential_generation.fetch_add(1, Ordering::AcqRel);
        // Invalidate any startup that is awaiting keyring I/O before starting
        // network withdrawal or local deletion.
        self.stop();
        self.invalidate_pending_browser_pairing();
        // Persist the remote-session revocation before any network or keyring
        // operation. If either later operation fails, resident/headless hosts
        // still deny every new remote invocation.
        if let Err(error) = set_remote_session_enabled(false).await {
            if let Ok(mut detail) = self.last_error.lock() {
                *detail = Some(format!("Remote hosting was stopped, but its disabled state could not be saved: {error}"));
            }
            return Err(error);
        }
        let stored = load_credentials().await?;
        let withdrawal = match stored.as_ref() {
            Some(credentials) => revoke(credentials).await,
            None => Ok(()),
        };
        clear_credentials_if_current(stored).await?;
        self.cache_credentials(None)?;
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = match &withdrawal {
                Ok(()) => None,
                Err(_) => Some(
                    "This computer was unpaired locally, but QC Remote Cloud could not be reached to withdraw the pairing. Unpair again while online."
                        .to_string(),
                ),
            };
        }
        self.status()
    }
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
                held.previous_credential = None;
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
    let expected = expected.cloned();
    let mut stored = incoming.clone();
    let refusal = crate::credential_io::shared()?
        .write(move || {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            let held = load_credentials_sync()?;
            if !same_credential(held.as_ref(), expected.as_ref()) {
                return Ok(Some(
                    "Pairing changed while the new receipt was pending; no credential was overwritten"
                        .into(),
                ));
            }
            let refusal = owner_change_refusal(
                held.as_ref().and_then(|value| value.pairing.as_ref()),
                stored.pairing.as_ref(),
                held.as_ref().is_some_and(|value| value.credential_rejected),
                held.is_some(),
            );
            if refusal.is_none() {
                stored.previous_credential = held.map(|mut previous| {
                    previous.previous_credential = None;
                    Box::new(previous)
                });
                store_credentials_sync(&stored)?;
            }
            Ok(refusal)
        })
        .await?;
    if let Some(reason) = refusal {
        let _ = revoke(&incoming).await;
        return Err(reason);
    }
    Ok(())
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

async fn clear_credentials_if_current(expected: Option<Credentials>) -> Result<(), String> {
    crate::credential_io::shared()?
        .write(move || {
            // Keep both the cross-process lease and compare-and-clear inside
            // the worker transaction. Dropping the async caller cannot release
            // the lease while the native keyring deletion is still running.
            let _mutation = credential_lease("relay-credential-state.lock")?;
            let held = load_credentials_sync()?;
            if !same_credential(held.as_ref(), expected.as_ref()) {
                return Err(
                    "Pairing changed while unpairing; the replacement was left untouched".into(),
                );
            }
            clear_credentials_sync()
        })
        .await
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
        *bridge.pending_browser_pairing.lock().unwrap() = Some(HostPairingSession {
            endpoint: "https://pair.example.invalid".into(),
            session_id: "fixture-session".into(),
            receiver_secret: "fixture-secret".into(),
            pair_url: "https://pair.example.invalid/fixture".into(),
            expires_at: u64::MAX,
            protocol: "fixture".into(),
        });
        bridge
            .browser_pairing_generation
            .store(generation, Ordering::Release);

        // Simulate a network poll that has started but whose returned code
        // has not yet reached the local one-shot redemption boundary.
        let (poll_started_tx, poll_started_rx) = tokio::sync::oneshot::channel();
        let (receipt_tx, receipt_rx) = tokio::sync::oneshot::channel();
        let polling_bridge = bridge.clone();
        let poll = tokio::spawn(async move {
            let _ = poll_started_tx.send(());
            receipt_rx.await.unwrap();
            polling_bridge.claim_browser_pairing_redemption(generation)
        });
        poll_started_rx.await.unwrap();

        bridge.cancel_browser_pairing();
        let _ = receipt_tx.send(());

        assert!(!poll.await.unwrap());
        assert!(bridge.pending_browser_pairing.lock().unwrap().is_none());

        // A valid, current poll must be able to claim the adoption boundary,
        // and that one-shot claim cannot be reused.
        let current = RelayBridge::default();
        *current.pending_browser_pairing.lock().unwrap() = Some(session());
        current
            .browser_pairing_generation
            .store(7, Ordering::Release);
        assert!(current.claim_browser_pairing_redemption(7));
        assert!(!current.claim_browser_pairing_redemption(7));

        // A stale poll cannot clear a replacement session while refusing its
        // claim; the replacement remains available to its own generation.
        let replacement = RelayBridge::default();
        *replacement.pending_browser_pairing.lock().unwrap() = Some(session());
        replacement
            .browser_pairing_generation
            .store(9, Ordering::Release);
        assert!(!replacement.claim_browser_pairing_redemption(8));
        assert!(replacement
            .pending_browser_pairing
            .lock()
            .unwrap()
            .is_some());
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
