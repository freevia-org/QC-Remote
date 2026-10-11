//! The update-only HTTP relay.
//!
//! A Quad Cortex connected over USB to this broker has no network of its own
//! to reach Neural DSP's firmware server, so it asks its host to fetch for it:
//! type 61 (`UpdaterForward`) carries the HTTP request the unit built, and the
//! reply, in the same message type, carries the response. This module answers
//! those requests, and only those. It carries no other message family: the
//! account, cloud-library, backup and log forwards (types 29, 30, 31) and the
//! cloud login (type 18) ride an encrypted envelope this build does not
//! implement, and nothing here decodes, executes or answers them.
//!
//! The unit supplies the URL, the headers (including its own credentials) and
//! the timeout. Only HTTPS requests to `neuraldsp.com` and its subdomains (and
//! to any extra domain the settings name) are carried, at any path; a request
//! for another domain is refused, answered with an error and recorded by host
//! name. The relay adds no header of its own to the unit's set (the HTTP client
//! adds only `Host` and, when the unit sent no `Accept`, its default
//! `Accept: */*`), stores no credential and inspects nothing in a body. Header
//! values and query values never reach a log, a status field or an error: the
//! audit record is the time, the request type, the host, the path shape, the
//! status and the byte counts. That record goes to the broker's error output
//! and the device flight recorder; the status that AI apps can read carries
//! only counts and the outcome of the last request (see [`RelayStatus`]).
//!
//! The switch is the `updaterRelayEnabled` key of the broker settings file
//! (`tray_settings`). It is on unless it is set to `false`; when it is off the
//! broker behaves as if this module did not exist (type 60 and 61 frames are
//! decoded, counted and dropped) and the status says the unit asked.

pub mod executor;
pub mod fetch;
mod http;
pub mod policy;
pub mod pump;
pub mod request;
pub mod response;

use crate::tray_settings::{self, UpdaterRelaySettings};
use executor::{ChunkSink, ExecutionLimits, Failure, Outcome, SinkClosed};
use fetch::{FetchFailure, HttpFetcher};
use policy::RelayPolicy;
use prost::Message as _;
use quadcortex::commands::OutboundMessage;
use quadcortex::proto::cortex_protobuf_v2 as pa;
use quadcortex::sync::RecoverPoison;
use request::RelayRequest;
use serde::Serialize;
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the settings file is read again.
const SETTINGS_RECHECK: Duration = Duration::from_secs(2);
/// Reply chunks that may wait for the USB writer. The HTTP side blocks when
/// this is full, so the image is never held in memory beyond a few chunks and
/// the server is read no faster than the device takes it.
const CHUNK_QUEUE: usize = 4;
/// How long the HTTP side waits for the USB writer to take a chunk before it
/// gives up. A wedged device lane must not leave a thread running for ever.
const SINK_STALL_LIMIT: Duration = Duration::from_secs(120);
const AUDIT_KEEP: usize = 16;
const EVENTS_KEEP: usize = 32;

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// One finished request, as far as it is ever recorded.
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

impl fmt::Display for AuditEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "updater-relay request={} host={} path={} outcome={} status={} in={} out={} hops={}",
            self.request,
            self.host,
            self.path_shape,
            self.outcome,
            self.status
                .map_or_else(|| "none".to_owned(), |status| status.to_string()),
            self.bytes_from_server,
            self.bytes_to_device,
            self.hops
        )
    }
}

/// The relay's part of the broker status (`usbDiagnostics.updaterRelay`).
///
/// `system.status` carries this object, and `system.status` is readable by the
/// AI apps attached to the broker (it backs the `qc://status` resource). What
/// it may show them is therefore limited to what the Settings row needs: the
/// switch, counts, whether a request is running, and the outcome word of the
/// last finished one. Everything that names a place or a time is marked
/// `skip_serializing`: host names (the unit's, the refused one), the allowed
/// list the user edited, the audit list and its path shapes, the free-text note
/// and the timestamps. They stay in the struct for the broker's own code and
/// tests, and reach the user through the broker's log and the device flight
/// recorder. No QC Remote tool or resource serves that recorder, but it is a
/// plain local file that any program running as the user can read.
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
    /// Requests for a domain that is not allowed. They were not carried.
    pub refused_domain_requests: u64,
    /// The host name (never a path or query) of the most recent request for a
    /// domain that is not allowed, and when it was made. A download served from
    /// another domain shows up here first; add the domain to
    /// `updaterRelayExtraDomains` to allow it. Not in `system.status`; it is in
    /// the broker's log and the flight recorder.
    #[serde(skip_serializing)]
    pub last_refused_host: Option<String>,
    #[serde(skip_serializing)]
    pub last_refused_at_unix_ms: Option<u128>,
    pub bytes_from_server: u64,
    pub bytes_to_device: u64,
    pub in_flight: bool,
    /// How the latest finished request ended, as the audit's outcome word
    /// (`completed`, `timeout`, `refused:domain-not-allowed`, ...). Never a
    /// host, a path or a value.
    pub last_outcome: Option<String>,
    /// The domains the relay will connect to, each with its subdomains.
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
            allowed_domains: vec![policy::NEURALDSP_DOMAIN.to_owned()],
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

/// Where the relay reads its switch from.
enum SettingsSource {
    /// No settings directory is known: the defaults apply.
    Defaults,
    Directory(PathBuf),
    /// Tests only: a fixed value, so a test can switch the relay without a file.
    #[cfg(test)]
    Fixed(UpdaterRelaySettings),
}

struct SettingsCache {
    source: SettingsSource,
    /// The last value read, or the defaults before the first read succeeds.
    settings: UpdaterRelaySettings,
    read_at: Option<Instant>,
    /// Whether the latest read of the settings file failed. Logged once per
    /// run of failures, not once per read.
    read_failing: bool,
}

struct Active {
    updater_request_id: u64,
    cancel: Arc<AtomicBool>,
    /// Set by the request's thread, after it has given the permit back, once
    /// its last frame is queued or it has ended: from then on the request is
    /// over, whatever frames are still queued.
    done: Arc<AtomicBool>,
    replies: mpsc::Receiver<OutboundMessage>,
}

struct State {
    stats: RelayStatus,
    audit: VecDeque<AuditEntry>,
    events: Vec<String>,
    /// Replies that need no HTTP work (a busy answer), sent before any chunk.
    immediate: VecDeque<OutboundMessage>,
    /// The queued frames of requests that are over but not yet written to the
    /// unit, oldest first. They go out before anything a newer request makes.
    draining: VecDeque<mpsc::Receiver<OutboundMessage>>,
    active: Option<Active>,
}

impl State {
    fn push_event(&mut self, event: String) {
        if self.events.len() >= EVENTS_KEEP {
            self.events.remove(0);
        }
        self.events.push(event);
    }
}

/// Holds the process-wide right to run one request. Dropping it gives the
/// right back, on every path out of the request.
struct Permit(Arc<AtomicBool>);

impl Permit {
    fn acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn process_limiter() -> Arc<AtomicBool> {
    static LIMITER: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    Arc::clone(LIMITER.get_or_init(|| Arc::new(AtomicBool::new(false))))
}

/// The relay for one device session. Cheap to create: no thread starts and no
/// network stack loads until the unit sends a request while the relay is on.
pub struct UpdaterRelay {
    settings: Mutex<SettingsCache>,
    fetcher: Arc<dyn HttpFetcher>,
    limiter: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Default for UpdaterRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdaterRelay {
    pub fn new() -> Self {
        Self::with_parts(Arc::new(http::ReqwestFetcher::default()), process_limiter())
    }

    /// A relay over another fetcher and its own concurrency limiter. The
    /// limiter is shared by every relay of a process in production, so two
    /// device sessions never run two requests at once.
    pub fn with_parts(fetcher: Arc<dyn HttpFetcher>, limiter: Arc<AtomicBool>) -> Self {
        Self {
            settings: Mutex::new(SettingsCache {
                source: SettingsSource::Defaults,
                settings: UpdaterRelaySettings::default(),
                read_at: None,
                read_failing: false,
            }),
            fetcher,
            limiter,
            state: Arc::new(Mutex::new(State {
                stats: RelayStatus::default(),
                audit: VecDeque::new(),
                events: Vec::new(),
                immediate: VecDeque::new(),
                draining: VecDeque::new(),
                active: None,
            })),
        }
    }

    /// Read the switch from `directory`'s broker settings file from now on.
    /// This is where a settings screen's toggle takes effect: it writes
    /// `updaterRelayEnabled` through `tray_settings::set_updater_relay_enabled_in`
    /// and the relay notices within [`SETTINGS_RECHECK`].
    pub fn set_settings_directory(&self, directory: PathBuf) {
        let mut cache = self.settings.lock_recover();
        cache.source = SettingsSource::Directory(directory);
        cache.read_at = None;
        cache.read_failing = false;
    }

    #[cfg(test)]
    pub(crate) fn use_fixed_settings(&self, settings: UpdaterRelaySettings) {
        let mut cache = self.settings.lock_recover();
        cache.source = SettingsSource::Fixed(settings);
        cache.read_at = None;
        cache.read_failing = false;
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
            match read {
                Ok(fresh) => {
                    cache.settings = fresh;
                    cache.read_failing = false;
                }
                // A file that is there and cannot be used (locked for a moment
                // while the app replaces it, or damaged) says nothing about
                // the switch, so the last value stands: a cleared switch must
                // not come back on because a read failed at the wrong moment.
                // Only an absent file is the default, and that is not an error.
                Err(error) => {
                    if !cache.read_failing {
                        cache.read_failing = true;
                        eprintln!(
                            "updater-relay: the broker settings file could not be used ({error:?}); \
                             keeping the last known setting, update relay {}",
                            if cache.settings.enabled { "on" } else { "off" }
                        );
                    }
                }
            }
            cache.read_at = Some(now);
        }
        cache.settings.clone()
    }

    pub fn status(&self) -> RelayStatus {
        let settings = self.settings(Instant::now());
        let state = self.state.lock_recover();
        let mut status = state.stats.clone();
        status.enabled = settings.enabled;
        status.allowed_domains = RelayPolicy::new(&settings.extra_domains).domains().to_vec();
        status.in_flight = state
            .active
            .as_ref()
            .is_some_and(|active| !active.done.load(Ordering::Acquire));
        status.recent = state.audit.iter().cloned().collect();
        status.last_outcome = state.audit.back().map(|entry| entry.outcome.clone());
        status.note = (!settings.enabled && status.requests_while_disabled > 0).then(|| {
            format!(
                "The Quad Cortex asked {} time(s) to use this PC's internet connection for a \
                 firmware update, and the update relay is turned off; the requests were dropped",
                status.requests_while_disabled
            )
        });
        status
    }

    /// Feed one received type-61 frame. Called from the device loop; it never
    /// waits on the network.
    pub fn on_forward_frame(&self, payload: &[u8], now: Instant) -> Disposition {
        let settings = self.settings(now);
        let decoded = request::decode_request(payload);
        let mut state = self.state.lock_recover();
        state.stats.requests_seen += 1;
        state.stats.last_request_at_unix_ms = Some(unix_ms());
        if !settings.enabled {
            state.stats.requests_while_disabled += 1;
            state.stats.last_disabled_request_at_unix_ms = Some(unix_ms());
            if let Ok(request) = &decoded {
                state.stats.last_disabled_request_kind = request
                    .kind
                    .map(|kind| kind.label().to_owned())
                    .or_else(|| Some("unknown".to_owned()));
                state.stats.last_disabled_request_host = reqwest::Url::parse(&request.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned));
            }
            return Disposition::Disabled;
        }
        let request = match decoded {
            Ok(request) => request,
            Err(error) => {
                state.stats.requests_failed += 1;
                state.push_event(format!("updater-relay undecodable request: {error:?}"));
                return Disposition::Undecodable;
            }
        };
        if state.active.as_ref().is_some_and(|active| {
            active.updater_request_id == request.updater_request_id
                && !active.done.load(Ordering::Acquire)
        }) {
            return Disposition::Duplicate;
        }
        // The process-wide permit says whether a request is running. The
        // request's thread gives it back when its network work is over (as it
        // starts to queue its last frame), and marks itself done once that frame
        // is queued, so a unit that answers a reply with its next request as
        // soon as that frame is on the wire finds the right to run free and is
        // not turned away as busy.
        let Some(permit) = Permit::acquire(&self.limiter) else {
            state.stats.requests_refused += 1;
            state.push_event("updater-relay busy: a request is already running".to_owned());
            state.immediate.push_back(response::forward_response(
                request.request_id,
                request.updater_request_id,
                Vec::new(),
                true,
                Some(response::error_code::BUSY),
            ));
            return Disposition::Busy;
        };
        // The previous request of this relay, if it still has an entry, has
        // given the permit back and is over. Its frames that the loop has not
        // written yet stay queued, in order, ahead of anything this one makes.
        if let Some(finished) = state.active.take() {
            state.draining.push_back(finished.replies);
        }

        let (replies, queue) = mpsc::sync_channel(CHUNK_QUEUE);
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let limits = ExecutionLimits {
            max_image_bytes: settings.max_download_bytes,
            ..ExecutionLimits::default()
        };
        let job = Job {
            policy: RelayPolicy::new(&settings.extra_domains),
            limits,
            fetcher: Arc::clone(&self.fetcher),
            cancel: Arc::clone(&cancel),
            done: Arc::clone(&done),
            shared: Arc::clone(&self.state),
            permit,
            request: request.clone(),
        };
        let kind = request.kind.map_or("unknown", |kind| kind.label());
        let spawned = thread::Builder::new()
            .name("qc-updater-relay".into())
            .spawn(move || job.run(replies));
        match spawned {
            Ok(_) => {
                state.active = Some(Active {
                    updater_request_id: request.updater_request_id,
                    cancel,
                    done,
                    replies: queue,
                });
                state.push_event(format!("updater-relay started request={kind}"));
                Disposition::Started
            }
            Err(_) => {
                state.stats.requests_failed += 1;
                state.push_event("updater-relay could not start its worker thread".to_owned());
                state.immediate.push_back(response::forward_response(
                    request.request_id,
                    request.updater_request_id,
                    Vec::new(),
                    true,
                    Some(response::error_code::NETWORK),
                ));
                Disposition::SpawnFailed
            }
        }
    }

    /// Feed one received type-60 (`Updater`) frame. A unit that reports its
    /// updater failed has abandoned whatever it asked for.
    pub fn on_updater_state(&self, payload: &[u8]) {
        let Ok(message) = pa::UpdaterMessage::decode(payload) else {
            return;
        };
        let failed = matches!(
            message.state,
            Some(pa::updater_message::State::State(value))
                if pa::updater_state::Enum::try_from(value).ok()
                    == Some(pa::updater_state::Enum::Failed)
        );
        if failed {
            self.cancel("the unit reported its updater failed");
        }
    }

    /// Switch-off and housekeeping, once per device-loop pass.
    pub fn maintain(&self, now: Instant) {
        if !self.settings(now).enabled && self.is_active() {
            self.cancel("the update relay was switched off");
        }
    }

    /// The next frame to send the unit, if one is ready. Never blocks.
    pub fn poll_outbound(&self) -> Option<OutboundMessage> {
        let mut state = self.state.lock_recover();
        if let Some(message) = state.immediate.pop_front() {
            return Some(message);
        }
        // Frames of requests that are over go first, in order.
        loop {
            let next = state.draining.front().map(|queue| queue.try_recv());
            match next {
                None => break,
                Some(Ok(message)) => return Some(message),
                Some(Err(mpsc::TryRecvError::Empty)) => return None,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    state.draining.pop_front();
                }
            }
        }
        let active = state.active.as_ref()?;
        match active.replies.try_recv() {
            Ok(message) => Some(message),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                state.active = None;
                None
            }
        }
    }

    /// Whether a request is running or a reply is waiting to be sent. The
    /// device loop keeps its own unconditional keepalive clock while this is true.
    pub fn is_active(&self) -> bool {
        let state = self.state.lock_recover();
        state.active.is_some() || !state.draining.is_empty() || !state.immediate.is_empty()
    }

    /// End whatever is running: the session it belonged to is gone, the unit
    /// gave up, or the switch went off. Nothing is retried.
    pub fn cancel(&self, reason: &str) {
        let mut state = self.state.lock_recover();
        state.immediate.clear();
        state.draining.clear();
        if let Some(active) = state.active.take() {
            active.cancel.store(true, Ordering::Release);
            state.push_event(format!("updater-relay cancelled: {reason}"));
        }
    }

    /// Cancel only if something is running; for the loop's no-session branch.
    pub fn cancel_if_active(&self, reason: &str) {
        if self.is_active() {
            self.cancel(reason);
        }
    }

    /// Lines for the device flight recorder, oldest first.
    pub fn take_events(&self) -> Vec<String> {
        std::mem::take(&mut self.state.lock_recover().events)
    }
}

/// Everything one request's thread owns.
struct Job {
    policy: RelayPolicy,
    limits: ExecutionLimits,
    fetcher: Arc<dyn HttpFetcher>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    shared: Arc<Mutex<State>>,
    permit: Permit,
    request: RelayRequest,
}

impl Job {
    fn run(self, replies: mpsc::SyncSender<OutboundMessage>) {
        let Job {
            policy,
            limits,
            fetcher,
            cancel,
            done,
            shared,
            permit,
            request,
        } = self;
        let mut sink = ChannelSink {
            replies,
            request_id: request.request_id,
            updater_request_id: request.updater_request_id,
            cancel: Arc::clone(&cancel),
            done,
            permit: Some(permit),
        };
        let outcome = executor::execute(
            fetcher.as_ref(),
            &policy,
            &request,
            &limits,
            &cancel,
            &mut sink,
        );
        // Over, whichever way it ended; any frames still queued are drained.
        // The right to run is given back before the audit line is written
        // (stderr can block), so the log never holds up the unit's next request.
        sink.finish();
        record_finish(&shared, &request, &outcome);
        drop(sink);
    }
}

/// Delivers reply chunks to the device loop through the bounded queue.
struct ChannelSink {
    replies: mpsc::SyncSender<OutboundMessage>,
    request_id: Option<u64>,
    updater_request_id: u64,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    /// The process-wide right to run, held until the request is over.
    permit: Option<Permit>,
}

impl ChannelSink {
    /// The request is over: give the right to run back (a no-op if the last
    /// frame already did), then say so. In that order, so that whoever sees
    /// `done` finds the right already free.
    fn finish(&mut self) {
        self.permit = None;
        self.done.store(true, Ordering::Release);
    }
}

impl ChunkSink for ChannelSink {
    fn emit(
        &mut self,
        payload: Vec<u8>,
        last: bool,
        error_code: Option<i32>,
    ) -> Result<(), SinkClosed> {
        let mut message = Some(response::forward_response(
            self.request_id,
            self.updater_request_id,
            payload,
            last,
            error_code,
        ));
        if last {
            // The network work is over; only the queueing is left. Give the
            // right to run back now, so no moment remains, between queueing
            // this frame and `finish`, in which the unit's next request could
            // be turned away. `done` still waits for the frame to be queued, and
            // the old frames stay ahead of the new request's in `draining`.
            self.permit = None;
        }
        let started = Instant::now();
        loop {
            if self.cancel.load(Ordering::Acquire) || started.elapsed() >= SINK_STALL_LIMIT {
                return Err(SinkClosed);
            }
            let Some(pending) = message.take() else {
                return Err(SinkClosed);
            };
            match self.replies.try_send(pending) {
                Ok(()) => {
                    if last {
                        // Nothing follows: the request is over now, with its
                        // last frame still to be written by the loop.
                        self.finish();
                    }
                    return Ok(());
                }
                Err(mpsc::TrySendError::Full(pending)) => {
                    message = Some(pending);
                    thread::sleep(Duration::from_millis(5));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(SinkClosed),
            }
        }
    }
}

fn failure_label(failure: Option<&Failure>) -> String {
    match failure {
        None => "completed".to_owned(),
        Some(Failure::Refused(rule)) => format!("refused:{rule}"),
        Some(Failure::BadRequest(rule)) => format!("bad-request:{rule}"),
        Some(Failure::TooLarge) => "too-large".to_owned(),
        Some(Failure::Timeout) => "timeout".to_owned(),
        Some(Failure::TooManyRedirects) => "too-many-redirects".to_owned(),
        Some(Failure::Network(kind)) => format!("network:{}", network_label(*kind)),
        Some(Failure::HttpStatus(_)) => "http-status".to_owned(),
        Some(Failure::Cancelled) => "cancelled".to_owned(),
        Some(Failure::DeviceGone) => "device-gone".to_owned(),
    }
}

fn network_label(kind: FetchFailure) -> &'static str {
    match kind {
        FetchFailure::Timeout => "timeout",
        FetchFailure::Dns => "dns",
        FetchFailure::BlockedAddress => "blocked-address",
        FetchFailure::Connect => "connect",
        FetchFailure::Tls => "tls",
        FetchFailure::Protocol => "protocol",
        FetchFailure::Io => "io",
    }
}

fn record_finish(shared: &Mutex<State>, request: &RelayRequest, outcome: &Outcome) {
    let entry = AuditEntry {
        at_unix_ms: unix_ms(),
        request: request
            .kind
            .map_or("unknown", |kind| kind.label())
            .to_owned(),
        host: outcome.host.clone(),
        path_shape: outcome.path_shape.clone(),
        outcome: failure_label(outcome.failure.as_ref()),
        status: outcome.status,
        bytes_from_server: outcome.bytes_from_server,
        bytes_to_device: outcome.bytes_to_device,
        hops: outcome.hops,
    };
    let line = entry.to_string();
    eprintln!("{line}");
    let mut state = shared.lock_recover();
    match &outcome.failure {
        None => state.stats.requests_completed += 1,
        Some(Failure::Refused(rule)) => {
            state.stats.requests_refused += 1;
            if *rule == "domain-not-allowed" {
                state.stats.refused_domain_requests += 1;
                state.stats.last_refused_host = Some(outcome.host.clone());
                state.stats.last_refused_at_unix_ms = Some(entry.at_unix_ms);
                // Prominent on purpose: the owner decides from this line
                // whether the domain belongs in `updaterRelayExtraDomains`.
                let notice = format!(
                    "updater-relay REFUSED a request for domain {} (not in an allowed domain); \
                     add it to updaterRelayExtraDomains to allow it",
                    outcome.host
                );
                eprintln!("{notice}");
                state.push_event(notice);
            }
        }
        Some(_) => state.stats.requests_failed += 1,
    }
    state.stats.bytes_from_server += outcome.bytes_from_server;
    state.stats.bytes_to_device += outcome.bytes_to_device;
    if state.audit.len() >= AUDIT_KEEP {
        state.audit.pop_front();
    }
    state.audit.push_back(entry);
    state.push_event(line);
}

#[cfg(test)]
mod tests;
