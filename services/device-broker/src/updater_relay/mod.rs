//! Transparent QC HTTP forwarding. Protocol adapters preserve device requests
//! and upstream responses; the USB pump only frames and paces those bytes.
mod cloud_forward;
pub mod executor;
pub mod fetch;
pub(crate) use cloud_forward::CloudForwardRelay;
pub(crate) mod http;
pub mod policy;
pub mod pump;
pub mod request;
pub(crate) mod resolver_helper;
pub mod response;
use crate::tray_settings::{self, UpdaterRelaySettings};
use executor::{ChunkSink, Failure, Outcome, SinkClosed};
use fetch::HttpFetcher;
use prost::Message as _;
use quadcortex::{proto::cortex_protobuf_v2 as pa, sync::RecoverPoison};
use quadcortex_forwarder::ForwardedMessage;
use request::RelayRequest;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
const SETTINGS_RECHECK: Duration = Duration::from_secs(2);
/// Legacy status schema; production forwarding does not populate history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub at_unix_ms: u128,
    pub request: String,
    pub host: String,
    pub path_shape: String,
    pub outcome: String,
    pub status: Option<u16>,
    pub bytes_from_server: u64,
    pub bytes_to_device: u64,
    pub hops: u8,
}

/// Relay switch and aggregate transfer status for existing clients.
/// Legacy request-history fields remain empty and are not serialized.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    /// Whether the relay is switched on. On unless the settings file says off.
    pub enabled: bool,
    /// Type-61 frames received, whether or not they were acted on.
    pub requests_seen: u64,
    /// Requests the unit made while the relay was off. They were dropped.
    pub requests_while_disabled: u64,
    #[serde(skip_serializing)]
    pub last_disabled_request_at_unix_ms: Option<u128>,
    #[serde(skip_serializing)]
    pub last_disabled_request_kind: Option<String>,
    #[serde(skip_serializing)]
    pub last_disabled_request_host: Option<String>,
    pub requests_completed: u64,
    pub requests_failed: u64,
    pub requests_refused: u64,
    #[serde(skip_serializing)]
    pub last_request_at_unix_ms: Option<u128>,
    /// Legacy status counter; transparent forwarding never refuses a domain.
    pub refused_domain_requests: u64,
    /// Legacy status fields retained for compatibility; always empty.
    #[serde(skip_serializing)]
    pub last_refused_host: Option<String>,
    #[serde(skip_serializing)]
    pub last_refused_at_unix_ms: Option<u128>,
    pub bytes_from_server: u64,
    pub bytes_to_device: u64,
    pub in_flight: bool,
    /// How the latest finished request ended, as the audit's outcome word
    /// (`completed`, `network`, `http-status`, ...). Never a
    /// host, a path or a value.
    pub last_outcome: Option<String>,
    /// Legacy status field; empty means no destination allowlist.
    #[serde(skip_serializing)]
    pub allowed_domains: Vec<String>,
    /// Set while the relay is off and the unit has asked for it.
    #[serde(skip_serializing)]
    pub note: Option<String>,
    #[serde(skip_serializing)]
    pub recent: Vec<AuditEntry>,
}

impl Default for RelayStatus {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_seen: 0,
            requests_while_disabled: 0,
            last_disabled_request_at_unix_ms: None,
            last_disabled_request_kind: None,
            last_disabled_request_host: None,
            requests_completed: 0,
            requests_failed: 0,
            requests_refused: 0,
            last_request_at_unix_ms: None,
            refused_domain_requests: 0,
            last_refused_host: None,
            last_refused_at_unix_ms: None,
            bytes_from_server: 0,
            bytes_to_device: 0,
            in_flight: false,
            last_outcome: None,
            allowed_domains: Vec::new(),
            note: None,
            recent: Vec::new(),
        }
    }
}

/// What [`UpdaterRelay::on_forward_frame`] did with a type-61 frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// The relay is off: counted, noted, dropped.
    Disabled,
    /// A request is running for it.
    Started,
    /// The same request id as the one already running.
    Duplicate,
    /// Another request is running; the unit was answered with an error.
    Busy,
    /// Not a decodable request; there is nobody to answer.
    Undecodable,
    /// No thread could be started; the unit was answered with an error.
    SpawnFailed,
}

enum SettingsSource {
    Defaults,
    Directory(PathBuf),
    #[cfg(test)]
    Fixed(UpdaterRelaySettings),
}
struct SettingsCache {
    source: SettingsSource,
    settings: UpdaterRelaySettings,
    read_at: Option<Instant>,
}
#[derive(Default)]
struct State {
    stats: RelayStatus,
}
struct Task {
    cancel: Arc<AtomicBool>,
    replies: mpsc::Receiver<ForwardedMessage>,
    thread: thread::JoinHandle<()>,
}
fn retired() -> &'static Mutex<Vec<thread::JoinHandle<()>>> {
    static TASKS: OnceLock<Mutex<Vec<thread::JoinHandle<()>>>> = OnceLock::new();
    TASKS.get_or_init(|| Mutex::new(Vec::new()))
}
fn reap() {
    let mut tasks = retired().lock_recover();
    let mut i = 0;
    while i < tasks.len() {
        if crate::platform::process::thread_signalled(&tasks[i]) {
            let _ = tasks.swap_remove(i).join();
        } else {
            i += 1;
        }
    }
}
/// A CorOS update is a series of Type 61 requests with gaps between them, so
/// an empty task list alone does not mean no update is running: the unit is
/// taken to be updating for this long after its latest request.
pub const UPDATE_QUIET_AFTER: Duration = Duration::from_secs(60);

/// What the unit has shown of an update, beyond the requests in flight.
#[derive(Debug, Default)]
struct UpdateActivity {
    /// When the unit last sent a Type 61 request.
    last_request_at: Option<Instant>,
    /// Its own updater state (Type 68) last said an update is under way.
    device_updating: bool,
}

pub struct UpdaterRelay {
    settings: Mutex<SettingsCache>,
    state: Arc<Mutex<State>>,
    tasks: Mutex<Vec<Task>>,
    response_cancellations: Mutex<Vec<std::sync::Weak<AtomicBool>>>,
    fetcher: Arc<dyn HttpFetcher>,
    activity: Mutex<UpdateActivity>,
}
impl Default for UpdaterRelay {
    fn default() -> Self {
        Self::new()
    }
}
impl UpdaterRelay {
    pub fn new() -> Self {
        Self::with_parts(
            Arc::new(http::direct_fetcher()),
            Arc::new(AtomicBool::new(false)),
        )
    }
    pub fn with_parts(fetcher: Arc<dyn HttpFetcher>, _legacy_limiter: Arc<AtomicBool>) -> Self {
        Self {
            settings: Mutex::new(SettingsCache {
                source: SettingsSource::Defaults,
                settings: UpdaterRelaySettings::default(),
                read_at: None,
            }),
            state: Arc::new(Mutex::new(State::default())),
            tasks: Mutex::new(Vec::new()),
            response_cancellations: Mutex::new(Vec::new()),
            fetcher,
            activity: Mutex::new(UpdateActivity::default()),
        }
    }
    pub fn set_settings_directory(&self, directory: PathBuf) {
        let mut settings = self.settings.lock_recover();
        settings.source = SettingsSource::Directory(directory);
        settings.read_at = None;
    }
    #[cfg(test)]
    pub(crate) fn use_fixed_settings(&self, fixed: UpdaterRelaySettings) {
        let mut settings = self.settings.lock_recover();
        settings.source = SettingsSource::Fixed(fixed);
        settings.read_at = None;
    }
    fn settings(&self, now: Instant) -> UpdaterRelaySettings {
        let mut cache = self.settings.lock_recover();
        let stale = cache
            .read_at
            .is_none_or(|read| now.saturating_duration_since(read) >= SETTINGS_RECHECK);
        if stale {
            let read = match &cache.source {
                SettingsSource::Defaults => Ok(UpdaterRelaySettings::default()),
                SettingsSource::Directory(directory) => {
                    tray_settings::try_read_updater_relay_in(directory)
                }
                #[cfg(test)]
                SettingsSource::Fixed(settings) => Ok(settings.clone()),
            };
            // A file that is there and cannot be used (locked for a moment
            // while the app replaces it, or damaged) says nothing about
            // the switch, so the last value stands: a cleared switch must
            // not come back on because a read failed at the wrong moment.
            // Only an absent file is the default, and that is not an error.
            if let Ok(fresh) = read {
                cache.settings = fresh;
            }
            cache.read_at = Some(now);
        }
        cache.settings.clone()
    }

    pub fn status(&self) -> RelayStatus {
        let enabled = self.settings(Instant::now()).enabled;
        let mut status = self.state.lock_recover().stats.clone();
        status.enabled = enabled;
        status.in_flight = self.is_active();
        status
    }
    /// Last observed relay diagnostics without settings I/O or lock waits.
    /// An unread directory setting is unknown, not the default enabled value.
    pub(crate) fn try_cached_status(&self) -> Option<RelayStatus> {
        let settings = match self.settings.try_lock() {
            Ok(settings) => settings,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        let enabled = match &settings.source {
            SettingsSource::Directory(_) if settings.read_at.is_none() => return None,
            #[cfg(test)]
            SettingsSource::Fixed(fixed) if settings.read_at.is_none() => fixed.enabled,
            _ => settings.settings.enabled,
        };
        let mut status = match self.state.try_lock() {
            Ok(state) => state.stats.clone(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().stats.clone(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        status.enabled = enabled;
        status.in_flight = match self.tasks.try_lock() {
            Ok(tasks) => !tasks.is_empty(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => !poisoned.into_inner().is_empty(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        Some(status)
    }
    pub fn on_forward_frame(&self, payload: &[u8], now: Instant) -> Disposition {
        reap();
        self.activity.lock_recover().last_request_at = Some(now);
        let settings = self.settings(now);
        {
            let mut state = self.state.lock_recover();
            state.stats.requests_seen += 1;
            if !settings.enabled {
                state.stats.requests_while_disabled += 1;
                return Disposition::Disabled;
            }
        }
        let request = match request::decode_request(payload) {
            Ok(r) => r,
            Err(_) => {
                self.state.lock_recover().stats.requests_failed += 1;
                return Disposition::Undecodable;
            }
        };
        let (sender, replies) = mpsc::sync_channel(4); // USB backpressure; never a refusal.
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut cancellations = self.response_cancellations.lock_recover();
            cancellations.retain(|cancel| cancel.strong_count() > 0);
            cancellations.push(Arc::downgrade(&cancel));
        }
        let task_cancel = Arc::clone(&cancel);
        let fetcher = Arc::clone(&self.fetcher);
        let state = Arc::clone(&self.state);
        let spawned = thread::Builder::new()
            .name("qc-http-updater".into())
            .spawn(move || {
                let mut sink = UsbSink {
                    sender,
                    request_id: request.request_id,
                    updater_request_id: request.updater_request_id,
                    cancel: Arc::clone(&task_cancel),
                    deadline: None,
                };
                let outcome =
                    executor::execute_at(fetcher.as_ref(), &request, &task_cancel, &mut sink, now);
                record_finish(&state, &request, &outcome);
            });
        match spawned {
            Ok(thread) => {
                self.tasks.lock_recover().push(Task {
                    cancel,
                    replies,
                    thread,
                });
                Disposition::Started
            }
            Err(_) => {
                self.state.lock_recover().stats.requests_failed += 1;
                Disposition::SpawnFailed
            }
        }
    }
    pub fn poll_outbound(&self) -> Option<ForwardedMessage> {
        let mut tasks = self.tasks.lock_recover();
        let mut i = 0;
        while i < tasks.len() {
            match tasks[i].replies.try_recv() {
                Ok(message) => {
                    if message.current() {
                        return Some(message);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => i += 1,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let task = tasks.remove(i);
                    if crate::platform::process::thread_signalled(&task.thread) {
                        let _ = task.thread.join();
                    } else {
                        retired().lock_recover().push(task.thread);
                    }
                }
            }
        }
        None
    }
    pub fn is_active(&self) -> bool {
        !self.tasks.lock_recover().is_empty()
    }
    /// Whether an update may be under way, for work that must not interrupt
    /// one: a request in flight, a request within `UPDATE_QUIET_AFTER`, or
    /// the unit's own updater state between requesting data and rebooting.
    pub fn update_in_progress(&self, now: Instant) -> bool {
        if self.is_active() {
            return true;
        }
        let activity = self.activity.lock_recover();
        activity.device_updating
            || activity
                .last_request_at
                .is_some_and(|at| now.saturating_duration_since(at) < UPDATE_QUIET_AFTER)
    }
    pub fn maintain(&self, now: Instant) {
        reap();
        if !self.settings(now).enabled {
            self.cancel_if_active("relay disabled");
        }
    }
    pub fn cancel(&self, _reason: &str) {
        {
            let mut cancellations = self.response_cancellations.lock_recover();
            for cancel in cancellations.iter().filter_map(std::sync::Weak::upgrade) {
                cancel.store(true, Ordering::Release);
            }
            cancellations.retain(|cancel| cancel.strong_count() > 0);
        }
        let tasks = std::mem::take(&mut *self.tasks.lock_recover());
        for task in &tasks {
            task.cancel.store(true, Ordering::Release);
        }
        for task in tasks {
            drop(task.replies);
            if crate::platform::process::thread_signalled(&task.thread) {
                let _ = task.thread.join();
            } else {
                retired().lock_recover().push(task.thread);
            }
        }
        reap();
    }
    pub fn cancel_if_active(&self, reason: &str) {
        self.cancel(reason)
    }
    pub fn on_updater_state(&self, payload: &[u8]) {
        if let Ok(message) = pa::UpdaterMessage::decode(payload) {
            if let Some(pa::updater_message::State::State(state)) = message.state.as_ref() {
                let updating = matches!(
                    pa::updater_state::Enum::try_from(*state),
                    Ok(pa::updater_state::Enum::RequestingData
                        | pa::updater_state::Enum::DownloadingData
                        | pa::updater_state::Enum::Updating
                        | pa::updater_state::Enum::Reboot)
                );
                self.activity.lock_recover().device_updating = updating;
            }
            if matches!(message.state,Some(pa::updater_message::State::State(state)) if pa::updater_state::Enum::try_from(state).ok()==Some(pa::updater_state::Enum::Failed))
            {
                self.cancel_if_active("device abandoned update");
            }
        }
    }
    pub fn take_events(&self) -> Vec<String> {
        Vec::new()
    }
}
impl Drop for UpdaterRelay {
    fn drop(&mut self) {
        self.cancel("relay dropped");
    }
}
struct UsbSink {
    sender: mpsc::SyncSender<ForwardedMessage>,
    request_id: Option<u64>,
    updater_request_id: u64,
    cancel: Arc<AtomicBool>,
    deadline: Option<Instant>,
}
impl ChunkSink for UsbSink {
    fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }
    fn emit(
        &mut self,
        payload: Vec<u8>,
        last: bool,
        status: Option<i32>,
    ) -> Result<(), SinkClosed> {
        let mut message = ForwardedMessage {
            message: response::forward_response(
                self.request_id,
                self.updater_request_id,
                payload,
                last,
                status,
            ),
            deadline: self.deadline,
            cancel: Arc::clone(&self.cancel),
        };
        loop {
            if self.cancel.load(Ordering::Acquire)
                || self.deadline.is_some_and(|end| Instant::now() >= end)
            {
                return Err(SinkClosed);
            }
            match self.sender.try_send(message) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(SinkClosed),
                Err(mpsc::TrySendError::Full(pending)) => {
                    message = pending;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}
fn record_finish(shared: &Mutex<State>, _request: &RelayRequest, outcome: &Outcome) {
    let label = match outcome.failure {
        None => "completed".to_owned(),
        Some(Failure::HttpStatus(_)) => "http-status".to_owned(),
        Some(Failure::Network(_)) => "network".to_owned(),
        Some(Failure::Cancelled) => "cancelled".to_owned(),
        Some(Failure::DeviceGone) => "device-gone".to_owned(),
        Some(Failure::BadRequest) => "protocol".to_owned(),
    };
    let mut state = shared.lock_recover();
    if outcome.failure.is_none() {
        state.stats.requests_completed += 1
    } else {
        state.stats.requests_failed += 1
    }
    state.stats.bytes_from_server += outcome.bytes_from_server;
    state.stats.bytes_to_device += outcome.bytes_to_device;
    state.stats.last_outcome = Some(label);
}
#[cfg(test)]
mod tests;
