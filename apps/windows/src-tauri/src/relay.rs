use qc_relay_client::{
    begin_host_pairing, pair_replacing, poll_host_pairing, revoke, AccessMode, Client,
    ConnectionState, Credentials, DeviceAdapter, HostPairingSession, PairedDevice, PairingMetadata,
    RemoteConnector, Usage,
};
use serde::Serialize;
use std::sync::{Arc, Mutex};
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
} else if cfg!(target_os = "windows") {
    "QC Remote on Windows"
} else {
    "QC Remote"
};
const RELAY_PLATFORM: &str = if cfg!(target_os = "linux") {
    "linux"
} else if cfg!(target_os = "windows") {
    "windows"
} else {
    "unknown"
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    pub paired: bool,
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
    lease: Mutex<Option<crate::background_host::HostLease>>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    stop: Mutex<Option<watch::Sender<bool>>>,
    state: Mutex<watch::Receiver<ConnectionState>>,
    access: watch::Sender<AccessMode>,
    last_error: Mutex<Option<String>>,
    pending_browser_pairing: Mutex<Option<HostPairingSession>>,
}

impl Default for RelayBridge {
    fn default() -> Self {
        let (_, state) = watch::channel(ConnectionState::Stopped);
        let (access, _) = watch::channel(AccessMode::Full);
        Self {
            lease: Mutex::new(None),
            task: Mutex::new(None),
            stop: Mutex::new(None),
            state: Mutex::new(state),
            access,
            last_error: Mutex::new(None),
            pending_browser_pairing: Mutex::new(None),
        }
    }
}

impl RelayBridge {
    pub(crate) fn reconcile_ownership(
        &self,
        delegated: bool,
        adapter: impl FnOnce() -> Arc<dyn DeviceAdapter>,
    ) {
        if delegated {
            self.stop();
            return;
        }
        let running = self.task.lock().map(|task| task.is_some()).unwrap_or(false);
        if !running && self.status().is_ok_and(|status| status.paired) {
            self.restore_access_mode();
            if let Err(error) = self.start(adapter()) {
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
        // One-line migration: remove the retired device-side approval setting
        // from the system credential store. Confirmation belongs to the AI client.
        let _ = clear_auto_approve_remote_actions();
        let credential = load_credentials()?;
        let needs_repair = credential
            .as_ref()
            .is_some_and(|value| !value.uses_current_identity_provider());
        let state = self.connection_state()?;
        Ok(RelayStatus {
            paired: credential.is_some() && !needs_repair,
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
            } else {
                self.last_error.lock().ok().and_then(|value| value.clone())
            },
        })
    }

    pub async fn usage(&self) -> Result<Usage, String> {
        let credentials = load_credentials()?
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
        let Some(credentials) = load_credentials()? else {
            return Ok(Vec::new());
        };
        match qc_relay_client::roster(&credentials).await {
            Ok(devices) => Ok(devices),
            Err(reason) if reason == qc_relay_client::ROSTER_UNSUPPORTED => Ok(Vec::new()),
            Err(reason) => Err(reason),
        }
    }

    pub async fn connectors(&self) -> Result<Vec<RemoteConnector>, String> {
        let credentials = load_credentials()?
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
        let credentials = load_credentials()?
            .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
        qc_relay_client::revoke_paired_device(&credentials, device_id).await?;
        self.roster().await
    }

    pub fn start(&self, adapter: Arc<dyn DeviceAdapter>) -> Result<(), String> {
        if crate::external_host::stopping() {
            return Ok(());
        }
        let credentials = match load_credentials()?
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
        if !credentials.uses_current_identity_provider() {
            let error =
                "Remote access now uses Google sign-in. Pair this device again to continue."
                    .to_string();
            if let Ok(mut detail) = self.last_error.lock() {
                *detail = Some(error.clone());
            }
            return Err(error);
        }
        self.stop();
        let directory =
            crate::local_stdio::app_local_data_dir().ok_or("Local app directory is unavailable")?;
        if crate::external_host::delegated(&directory) {
            return Ok(());
        }
        let lease =
            crate::background_host::HostLease::try_acquire(&directory, "remote-mcp-host.lock")?
                .ok_or("Remote MCP is hosted by the background tray")?;
        let credentials = begin_credential_connection(credentials)?;
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
    }

    pub fn stop(&self) {
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

    pub fn set_access_mode(&self, mode: &str) -> Result<AccessMode, String> {
        let mode = AccessMode::parse(mode)?;
        store_access_mode(mode)?;
        self.access.send_replace(mode);
        Ok(mode)
    }

    pub fn restore_access_mode(&self) {
        self.access.send_replace(load_access_mode());
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
        let held = load_credentials()?;
        let credentials =
            pair_replacing(endpoint, pairing_code, device_name, held.as_ref()).await?;
        adopt_credentials(credentials, held.as_ref()).await?;
        self.start(adapter)?;
        self.status()
    }

    pub async fn begin_browser_pairing(
        &self,
        endpoint: &str,
    ) -> Result<BrowserPairingOffer, String> {
        let session = begin_host_pairing(endpoint, RELAY_DEVICE_NAME, RELAY_PLATFORM).await?;
        let offer = BrowserPairingOffer {
            pair_url: session.pair_url.clone(),
            expires_at: session.expires_at,
        };
        *self
            .pending_browser_pairing
            .lock()
            .map_err(|_| "Browser pairing lock was poisoned")? = Some(session);
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
        let session = self
            .pending_browser_pairing
            .lock()
            .map_err(|_| "Browser pairing lock was poisoned")?
            .clone()
            .ok_or("Start browser pairing first")?;
        if session.expires_at < now_millis() {
            *self
                .pending_browser_pairing
                .lock()
                .map_err(|_| "Browser pairing lock was poisoned")? = None;
            return Err(
                "The browser pairing session expired. Start it again from QC Remote.".into(),
            );
        }
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let held = load_credentials()?;
        let Some(code) = poll_host_pairing(&session.endpoint, &session).await? else {
            return Ok(BrowserPairingProgress {
                state: "waiting".into(),
                status: None,
            });
        };
        // Redeeming a code is one-shot, including an ambiguous transport failure.
        *self
            .pending_browser_pairing
            .lock()
            .map_err(|_| "Browser pairing lock was poisoned")? = None;
        let credentials =
            pair_replacing(&session.endpoint, &code, device_name, held.as_ref()).await?;
        adopt_credentials(credentials, held.as_ref()).await?;
        self.start(adapter)?;
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
        if let Ok(mut pending) = self.pending_browser_pairing.lock() {
            *pending = None;
        }
        if let Ok(mut detail) = self.last_error.lock() {
            *detail = None;
        }
    }

    /// Withdraw the pairing at the relay first, then locally.
    ///
    /// Clearing only the local store leaves the relay honouring a credential
    /// the user believes they have revoked. The local clear still happens if
    /// the relay cannot be reached, because a person who asked to unpair must
    /// never be left paired by a network failure; the reason is surfaced so
    /// they can retry while the relay still holds it.
    pub async fn unpair(&self) -> Result<RelayStatus, String> {
        let _operation = credential_lease("relay-pairing-operation.lock")?;
        let stored = load_credentials()?;
        self.stop();
        if let Ok(mut pending) = self.pending_browser_pairing.lock() {
            *pending = None;
        }
        let withdrawal = match stored {
            Some(credentials) => revoke(&credentials).await,
            None => Ok(()),
        };
        {
            let _mutation = credential_lease("relay-credential-state.lock")?;
            clear_credentials()?;
        }
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

fn clear_auto_approve_remote_actions() -> Result<(), String> {
    match auto_approve_remote_actions_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(
            "Could not clear the remote approval preference from the system credential store"
                .into(),
        ),
    }
}

/// The access level the user chose, as persisted in Windows Credential
/// Manager. `pub(crate)` for a headless `--local-mcp-bridge` host, which has no
/// [`RelayBridge`] and must enforce the same choice a window restores with
/// [`RelayBridge::restore_access_mode`].
pub(crate) fn load_access_mode() -> AccessMode {
    access_mode_entry()
        .and_then(|entry| {
            entry
                .get_password()
                .map_err(|_| "Relay access mode is not stored".to_string())
        })
        .or_else(|_| {
            legacy_access_mode_entry().and_then(|entry| {
                entry
                    .get_password()
                    .map_err(|_| "Relay access mode is not stored".to_string())
            })
        })
        .ok()
        .and_then(|value| AccessMode::parse(&value).ok())
        .unwrap_or_default()
}

fn store_access_mode(mode: AccessMode) -> Result<(), String> {
    let value = mode.as_str();
    access_mode_entry()?
        .set_password(value)
        .map_err(|_| "Could not store the relay access mode in the system credential store".into())
}

/// Revoke one AI app's OAuth grant with this computer's device credential:
/// the Remote panel's Unlink and the broker icon's remote Unlink both end
/// here. Ends that app's authorization only; the account and this pairing are
/// untouched. Returns the credential used, for a caller that reads on, and
/// what the relay said (a 404 is not a confirmed revoke).
pub(crate) async fn revoke_connector_grant(
    grant_id: &str,
) -> Result<(Credentials, qc_relay_client::ConnectorRevoke), String> {
    let credentials = load_credentials()?
        .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
    let outcome = qc_relay_client::revoke_connector(&credentials, grant_id).await?;
    Ok((credentials, outcome))
}

pub(crate) fn load_credentials() -> Result<Option<Credentials>, String> {
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
pub(crate) fn begin_credential_connection(expected: Credentials) -> Result<Credentials, String> {
    let _mutation = credential_lease("relay-credential-state.lock")?;
    let held = load_credentials()?;
    if !same_credential(held.as_ref(), Some(&expected)) {
        return Err("Pairing changed before relay connection; no connection started".into());
    }
    let mut current = held.ok_or("Pairing disappeared before relay connection")?;
    current.connection_generation = current
        .connection_generation
        .checked_add(1)
        .ok_or("Relay connection generation exhausted; reconnect refused")?;
    store_credentials(&current)?;
    Ok(current)
}

fn same_connection(expected: &Credentials, held: &Credentials) -> bool {
    same_credential(Some(expected), Some(held))
        && expected.connection_generation == held.connection_generation
}

pub(crate) fn acknowledge_replacement(expected: &Credentials, previous_device_id: &str) {
    let Ok(_mutation) = credential_lease("relay-credential-state.lock") else {
        return;
    };
    let Ok(Some(mut held)) = load_credentials() else {
        return;
    };
    if same_connection(expected, &held)
        && held
            .previous_credential
            .as_ref()
            .is_some_and(|previous| previous.device_id == previous_device_id)
    {
        held.previous_credential = None;
        let _ = store_credentials(&held);
    }
}

pub(crate) fn record_credential_status(expected: &Credentials, rejected: bool) {
    let Ok(_mutation) = credential_lease("relay-credential-state.lock") else {
        return;
    };
    let Ok(Some(mut held)) = load_credentials() else {
        return;
    };
    if same_connection(expected, &held) {
        held.credential_rejected = rejected;
        let _ = store_credentials(&held);
    }
}

async fn adopt_credentials(
    mut incoming: Credentials,
    expected: Option<&Credentials>,
) -> Result<(), String> {
    let refusal = {
        let _mutation = credential_lease("relay-credential-state.lock")?;
        let held = load_credentials()?;
        if !same_credential(held.as_ref(), expected) {
            Some(
                "Pairing changed while the new receipt was pending; no credential was overwritten"
                    .into(),
            )
        } else {
            let refusal = owner_change_refusal(
                held.as_ref().and_then(|value| value.pairing.as_ref()),
                incoming.pairing.as_ref(),
                held.as_ref().is_some_and(|value| value.credential_rejected),
                held.is_some(),
            );
            if refusal.is_none() {
                incoming.previous_credential = held.map(|mut previous| {
                    previous.previous_credential = None;
                    Box::new(previous)
                });
                store_credentials(&incoming)?;
            }
            refusal
        }
    };
    if let Some(reason) = refusal {
        let _ = revoke(&incoming).await;
        return Err(reason);
    }
    Ok(())
}

fn store_credentials(credentials: &Credentials) -> Result<(), String> {
    let encoded = serde_json::to_string(credentials)
        .map_err(|_| "Could not encode the public-relay credential")?;
    credential_entry()?.set_password(&encoded).map_err(|_| {
        "Could not store the public-relay credential in the system credential store".into()
    })
}

fn clear_credentials() -> Result<(), String> {
    clear_auto_approve_remote_actions()?;
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
    fn relay_defaults_to_full_control() {
        let bridge = RelayBridge::default();
        assert_eq!(*bridge.access.borrow(), AccessMode::Full);
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
