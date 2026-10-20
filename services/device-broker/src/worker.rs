mod raw_events;
pub use raw_events::RawSubscription;
use raw_events::{fan_out_raw_event, subscribe_raw, RawEventBus};
mod preset_workflow;
mod retired_worker;
use retired_worker::{WorkerLifetime, WorkerRetirement};
mod retention;
mod session_identity;
mod state_journal;
use crate::updater_relay::pump::{RelayPump, RelaySink};
use crate::updater_relay::{CloudForwardRelay, RelayStatus, UpdaterRelay};
#[cfg(test)]
use prost::Message as _;
use quadcortex::commands::{self, DeviceCommand, DeviceOperation, OutboundMessage};
use quadcortex::device::backup::BackupAction;
use quadcortex::device::correlation::{ResponseDecision, ResponseExpectation};
use quadcortex::device::owner::{
    DispatchContext, GatewayReadScope, ObservationTicket, OperationTicket, PresetCatalogAbsence,
    SessionFence, SessionRuntime,
};
use quadcortex::device::read::{GatewayReadRuntime, ReadObservation};
use quadcortex::device::request::{self as runtime_request, GatewayWritePlan};
use quadcortex::device::request::{
    GatewayVerification, GatewayVerificationAction, GatewayVerificationRuntime,
    GatewayWriteVerificationPolicy,
};
use quadcortex::device::touch_guard::TouchGuard;
#[cfg(test)]
use quadcortex::device::transport::TransportRuntime;
use quadcortex::device::transport::{CloudAnswerKey, IngressTime};
use quadcortex::device::{GatewaySnapshot, PresetFolder, PresetList};
#[cfg(test)]
use quadcortex::domain::STATE_EVENT_MAXIMUM_LIMIT;
use quadcortex::inbound_telemetry::InboundTelemetrySnapshot;
use quadcortex::profile;
#[cfg(test)]
use quadcortex::proto::cortex_protobuf_v2 as pa;
use quadcortex::responses::{LibraryEntries, TempoClock as TempoClockFrame};
use quadcortex::state::{parse_model_repo, BlockDetails, ModelCatalog, ModelList, StateUpdate};
use quadcortex::sync::RecoverPoison;
use quadcortex_forwarder::cloud_forward::RequestOrigin;
use quadcortex_hid::{ConnectedQc, IncomingMessage, QcUsb, UsbError};
use retention::{
    budgeted_queue, BudgetedSender, OfferOutcome, QueueKind, RetentionCounters,
    RAW_SUBSCRIBER_QUEUE_MAX_BYTES, STATE_SUBSCRIBER_QUEUE_MAX_BYTES,
};
pub use retention::{BudgetedReceiver, RetentionDiagnostics};
use serde::Serialize;
use state_journal::StateEventJournal;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroize;

const SUBSCRIBER_QUEUE_CAPACITY: usize = quadcortex::domain::STATE_EVENT_DEFAULT_LIMIT;

/// Why a preset-catalog read gave up with nothing to project: the owner's
/// reason and catalog counters, plus this USB connection's wire counters and
/// how long the read waited. Carried as the `-32010` error's `data.catalog`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetCatalogUnpublished {
    /// The gateway method that read the catalog.
    pub method: String,
    /// The setlist key the read looked up: the request's own, or the active
    /// setlist's when it named none. `None` for the folder list, which takes
    /// no key. Echoed so a mangled key diagnoses itself.
    pub setlist_key: Option<String>,
    /// The key names no folder of this session's complete directory, so the
    /// read failed at once instead of waiting out its budget.
    pub setlist_key_unknown: bool,
    /// Which empty-catalog condition applied, with the owner's counters.
    #[serde(flatten)]
    pub absence: PresetCatalogAbsence,
    /// Type-4 (File) frames received on this USB connection, from
    /// `usbDiagnostics.messagesReceivedByType`.
    pub file_frames_received: u64,
    /// Type-4 (File) messages this connection sent.
    pub file_messages_sent: u64,
    /// Type-49 (Connection) frames received on this USB connection.
    pub connection_frames_received: u64,
    /// `usbDiagnostics.sessionEpoch` when the read gave up.
    pub session_epoch: u64,
    /// How long the read waited before giving up.
    pub waited_ms: u64,
    /// The read's budget.
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerStatus {
    pub phase: String,
    pub detail: String,
    pub connected: bool,
    pub synchronized: bool,
    pub active_preset_name: Option<String>,
    pub active_preset_location: Option<String>,
    /// Display name read once per connected session; status polling is cache-only.
    pub device_name: Option<String>,
    pub active_scene: Option<u32>,
    pub connected_at_unix_ms: Option<u128>,
    pub handshake_ms: Option<u128>,
    pub messages_received: u64,
    pub messages_sent: u64,
    pub write_delivery: quadcortex_hid::WriteDeliveryCounts,
    pub messages_received_by_type: HashMap<u16, u64>,
    pub messages_sent_by_type: HashMap<u16, u64>,
    /// Inbound frames this connection received and could not act on, by
    /// reason and by message type, plus the per-type tally of the envelope's
    /// two device trailer bytes. `messagesReceivedByType` counts what the
    /// host understood; this counts what it did not. See
    /// `quadcortex::inbound_telemetry`.
    pub inbound_telemetry: InboundTelemetrySnapshot,
    /// Frames the broker did not keep or deliver under its memory bounds:
    /// slow-subscriber drops, latest-message cache refusals and raw-history
    /// omissions. Process-lifetime counters; see `worker::retention`.
    pub retention: RetentionDiagnostics,
    /// Set after a mutation reached the wire but its outcome could not be
    /// established. Reads remain available; later writes fail closed until a
    /// fresh device session has synchronized.
    pub writes_quarantined: bool,
    pub write_quarantine_reason: Option<String>,
    /// Structured form of the quarantine: which write, how far it reaches,
    /// since when, and exactly how it clears. `None` whenever
    /// `writes_quarantined` is false.
    pub write_quarantine: Option<crate::write_quarantine::WriteQuarantine>,
    /// The device asked the host to stop driving it (a device backup or a
    /// reset is running on the unit). Requests that would reach the device are
    /// refused with `DEVICE_SUSPENDED` until it resumes or the session ends;
    /// cached reads, status, the keepalive and session control continue.
    pub device_suspended: bool,
    /// The structured form: the reason, since when, how many requests were
    /// refused, and how the last suspension ended. Read from the session
    /// owner on every status call, never cached here.
    pub suspension: quadcortex::device::owner::SuspensionDiagnostics,
    pub expected_write_stalls: u64,
    pub last_hid_write_duration_ms: u64,
    pub max_hid_write_duration_ms: u64,
    pub last_hid_write_completed: bool,
    /// Heuristic risk signal for `usb.rs`'s HID reader thread: how many times
    /// the `session_owner` lock plus the bounded read-queue send held the
    /// reader off the device long enough that the OS/driver-level HID input
    /// queue - which surfaces no overflow flag of its own - could plausibly
    /// have silently dropped a report. See `UsbTelemetry::reader_lock_stall_events`.
    pub reader_lock_stall_events: u64,
    pub max_reader_lock_stall_ms: u64,
    pub last_message_type: Option<u16>,
    /// The update-only HTTP relay: whether it is on, what the unit has asked
    /// it for, what it refused, and the audit record of finished requests. A
    /// unit that asks while the relay is off is counted here and `note` says
    /// so. Always current; read from the relay on every status call.
    pub updater_relay: RelayStatus,
    /// Invalidates queued host admission on connection attempts, completed
    /// transport opens, and in-session state rebuilds. Connection timestamps
    /// alone cannot distinguish an in-session rebuild.
    /// Internal bookkeeping, deliberately absent from the status payload.
    #[serde(skip)]
    pub session_epoch: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedStateFrame {
    pub stream_id: String,
    pub sequence: u64,
    pub observed_at: u128,
    pub states: Vec<StateUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo_clock: Option<TempoClockFrame>,
}

/// A live decoded-state subscription, bounded by entry count and bytes.
pub struct StateSubscription {
    receiver: BudgetedReceiver<DecodedStateFrame>,
    identity: Arc<()>,
    subscribers: std::sync::Weak<Mutex<Vec<StateSubscriber>>>,
}

struct StateSubscriber {
    identity: Arc<()>,
    sender: BudgetedSender<DecodedStateFrame>,
}

impl std::ops::Deref for StateSubscription {
    type Target = BudgetedReceiver<DecodedStateFrame>;
    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl Drop for StateSubscription {
    fn drop(&mut self) {
        // Membership belongs to the waiter, not the next USB publication.
        // A disconnected/silent unit must not retain abandoned TCP queues.
        if let Some(subscribers) = self.subscribers.upgrade() {
            subscribers
                .lock_recover()
                .retain(|subscriber| !Arc::ptr_eq(&subscriber.identity, &self.identity));
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateEventPage {
    pub native: bool,
    pub stream_id: String,
    pub latest_sequence: u64,
    pub frames: Vec<DecodedStateFrame>,
}

pub(crate) struct GatewayVerificationStep {
    pub action: GatewayVerificationAction,
    pub snapshot: Option<GatewaySnapshot>,
}

/// The host owns waiting; the shared owner owns verification and its session.
pub(crate) struct GatewayVerificationWait<'a> {
    controller: &'a DeviceController,
    id: u64,
}

impl GatewayVerificationWait<'_> {
    pub fn advance(&self, observation_sequence: u64) -> Result<GatewayVerificationStep, String> {
        let mut owner = self.controller.session_owner.lock_recover();
        let now_ms = self.controller.session_clock.elapsed().as_millis() as u64;
        let (action, _context, _remaining_ms) = owner.advance_gateway_verification(
            self.id,
            u128::from(observation_sequence),
            now_ms,
        )?;
        let snapshot = matches!(action, GatewayVerificationAction::Verified)
            .then(|| owner.state.snapshot().clone());
        Ok(GatewayVerificationStep { action, snapshot })
    }
}

impl Drop for GatewayVerificationWait<'_> {
    fn drop(&mut self) {
        self.controller
            .session_owner
            .lock_recover()
            .cancel_gateway_verification(self.id);
    }
}

impl Default for BrokerStatus {
    fn default() -> Self {
        Self {
            phase: "searching".into(),
            detail: "Waiting for Quad Cortex USB".into(),
            connected: false,
            synchronized: false,
            active_preset_name: None,
            active_preset_location: None,
            device_name: None,
            active_scene: None,
            connected_at_unix_ms: None,
            handshake_ms: None,
            messages_received: 0,
            messages_sent: 0,
            write_delivery: Default::default(),
            messages_received_by_type: HashMap::new(),
            messages_sent_by_type: HashMap::new(),
            inbound_telemetry: InboundTelemetrySnapshot::default(),
            retention: RetentionDiagnostics::default(),
            writes_quarantined: false,
            write_quarantine_reason: None,
            write_quarantine: None,
            device_suspended: false,
            suspension: Default::default(),
            expected_write_stalls: 0,
            last_hid_write_duration_ms: 0,
            max_hid_write_duration_ms: 0,
            last_hid_write_completed: false,
            reader_lock_stall_events: 0,
            max_reader_lock_stall_ms: 0,
            last_message_type: None,
            updater_relay: RelayStatus::default(),
            session_epoch: 0,
        }
    }
}

enum Command {
    RecordFlight {
        event: String,
        incident: bool,
    },
    ReserveRequestId {
        reply: mpsc::Sender<Result<u64, String>>,
    },
    Reconnect {
        force: bool,
        reply: mpsc::Sender<ReconnectOutcome>,
    },
    Disconnect {
        reply: mpsc::Sender<Result<(), String>>,
        /// A sharing-safe close keeps new participant admission reserved until
        /// the worker actually finishes, even if the caller stops waiting.
        _admission: Option<Box<dyn Send>>,
    },
    Send(
        u16,
        Vec<u8>,
        OperationTicket,
        mpsc::Sender<Result<(), String>>,
    ),
    RetargetCapture(
        OutboundMessage,
        u64,
        OperationTicket,
        mpsc::Sender<Result<(), String>>,
    ),
    SendRealtime(u16, Vec<u8>, OperationTicket),
    SendSequence {
        admission: OperationTicket,
        messages: Vec<OutboundMessage>,
        delay: Duration,
        interval: Duration,
        /// Sent once, without the sequence's admission, when a frame after the
        /// first fails (the touch-down is then known to have been accepted). See
        /// `send_paced_sequence`.
        release_on_failure: Option<OutboundMessage>,
        /// For a hold: the touch guard and the held point, so that a hold whose
        /// touch-up could not be delivered even after the one second attempt is
        /// recorded as a held pointer by the worker itself, whether or not the
        /// caller waits for the result.
        hold: Option<HoldRecord>,
        reply: mpsc::Sender<Result<(), String>>,
    },
    Request {
        message_type: u16,
        payload: Vec<u8>,
        expected_type: u16,
        request_id: Option<u64>,
        admission: RequestAdmission,
        reply: mpsc::Sender<Result<IncomingMessage, String>>,
    },
    SubmitGatewayRead {
        messages: Vec<OutboundMessage>,
        admission: RequestAdmission,
        reply: mpsc::Sender<Result<u64, String>>,
    },
    CreateBackup {
        admission: OperationTicket,
        reply: mpsc::Sender<Result<String, String>>,
    },
    Stop,
}

/// Test-only view of the worker queue of a controller with no worker.
#[cfg(test)]
pub(crate) struct TestCommands(mpsc::Receiver<Command>);

#[cfg(test)]
impl TestCommands {
    /// Queued commands other than best-effort flight records: anything that
    /// would have reached the device or changed the session.
    pub(crate) fn device_work(&self) -> usize {
        self.0
            .try_iter()
            .filter(|command| !matches!(command, Command::RecordFlight { .. }))
            .count()
    }
}

/// Queue admission and wire response share one caller budget. Keeping the
/// native monotonic origin here avoids inventing a second worker-relative
/// deadline after a read has already waited behind other commands.
struct RequestAdmission {
    epoch: u64,
    operation: OperationTicket,
    clock: Instant,
}

impl RequestAdmission {
    fn remaining(&self, current_epoch: u64, now: Instant) -> Result<Duration, String> {
        if self.epoch != current_epoch {
            return Err("Device session changed before the queued QC read was submitted".into());
        }
        let remaining = Duration::from_millis(
            self.operation.remaining_ms(
                now.saturating_duration_since(self.clock)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            ),
        );
        if remaining.is_zero() {
            Err("QC read expired before USB submission".into())
        } else {
            Ok(remaining)
        }
    }
}

fn queue_request(commands: &mpsc::SyncSender<Command>, command: Command) -> Result<(), String> {
    commands.try_send(command).map_err(|error| match error {
        mpsc::TrySendError::Full(_) => {
            "QC command queue is full; the read was not submitted".into()
        }
        mpsc::TrySendError::Disconnected(_) => "Native QC worker is not available".into(),
    })
}

fn submit_read_batch(
    messages: Vec<OutboundMessage>,
    admission: &RequestAdmission,
    epoch: u64,
    mut now: impl FnMut() -> Instant,
    mut write: impl FnMut(OutboundMessage) -> Result<(), String>,
) -> Result<(), String> {
    for message in messages {
        admission.remaining(epoch, now())?;
        write(message)?;
    }
    Ok(())
}

// Subscribers are notification aids, not authoritative storage. The bounded
// history above is the catch-up source, so a stalled UI/client must not retain
// an unbounded clone of every device frame. Each queue is bounded by this
// entry count and by a byte budget (`retention`), and a drop is logged and
// counted in the broker status.
// Bound both internal producer/consumer seams. Realtime commands acknowledge
// queue admission rather than physical completion, so an unbounded channel
// would let a stalled USB device turn concurrent remote calls into unbounded
// process memory. The decoder queue is larger because one legitimate catalog
// burst can contain more than a thousand frames.
const DEVICE_COMMAND_QUEUE_CAPACITY: usize = quadcortex::domain::STATE_EVENT_DEFAULT_LIMIT;
// A bounded queue limits retained commands, not a continuously replenished drain.
// Yield to inbound state, relay progress and lifecycle deadlines after each batch.
const DEVICE_COMMAND_BATCH_LIMIT: usize = 32;

fn next_command_in_batch<T>(
    commands: &mpsc::Receiver<T>,
    remaining: &mut usize,
    not_before: Instant,
) -> Option<T> {
    if *remaining == 0 || Instant::now() < not_before {
        return None;
    }
    let command = commands.try_recv().ok()?;
    *remaining -= 1;
    Some(command)
}
const STATE_DECODER_QUEUE_CAPACITY: usize = quadcortex::domain::STATE_EVENT_MAXIMUM_LIMIT;
// The USB reader itself blocks on a permanent native RX thread. This short
// broker-side receive poll only multiplexes completed reports with outbound
// commands; keeping the old 50 ms wait added a full UI frame (and sometimes
// more) before realtime HID commands could even be submitted.
const CONNECTED_IO_POLL_MS: i32 = 5;
/// Raw File frames reach subscribers before the state decoder has ingested
/// them, so a wait that only wakes on the *next* frame can sleep through the
/// moment the last listing of a burst becomes readable. Anything waiting for
/// the session's directory publication re-checks the cache at least this often.
const PRESET_PUBLICATION_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// Scoped operations cannot spend a second retry window reserving an ID.
fn reserve_request_id_before(
    commands: &mpsc::SyncSender<Command>,
    deadline: Instant,
) -> Result<u64, String> {
    if Instant::now() >= deadline {
        return Err("QC request budget expired before request ID reservation".into());
    }
    let (reply, response) = mpsc::channel();
    commands
        .try_send(Command::ReserveRequestId { reply })
        .map_err(|_| "Native QC worker cannot admit the scoped request".to_string())?;
    response
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "QC request budget expired while reserving a request ID".to_string())?
}

/// Discovery belongs to this controller and its worker, never a process-wide
/// test override. Production instances always use the native HID enumerator.
#[cfg(test)]
type DiscoveryFixture =
    Arc<dyn Fn() -> Result<Vec<quadcortex_hid::usb::QcUsbDevice>, String> + Send + Sync>;

#[derive(Clone, Default)]
struct DeviceDiscovery {
    #[cfg(test)]
    enumerate: Option<DiscoveryFixture>,
}

impl DeviceDiscovery {
    fn devices(&self) -> Result<Vec<quadcortex_hid::usb::QcUsbDevice>, String> {
        #[cfg(test)]
        if let Some(enumerate) = &self.enumerate {
            return enumerate();
        }
        quadcortex_hid::usb::connected_devices().map_err(|error| error.to_string())
    }
}

pub struct DeviceController {
    discovery: DeviceDiscovery,
    device_id: Arc<Mutex<Option<String>>>,
    identity_observation_enabled: Arc<AtomicBool>,
    state_stream_id: String,
    state: Arc<Mutex<BrokerStatus>>,
    latest_messages: Arc<Mutex<HashMap<u16, IncomingMessage>>>,
    raw_events: Arc<RawEventBus>,
    state_event_log: Arc<Mutex<StateEventJournal>>,
    latest_tempo_clock: Arc<Mutex<Option<DecodedStateFrame>>>,
    latest_master_volume: Arc<Mutex<Option<DecodedStateFrame>>>,
    state_event_sequence: Arc<AtomicU64>,
    state_subscribers: Arc<Mutex<Vec<StateSubscriber>>>,
    session_owner: Arc<Mutex<SessionRuntime>>,
    session_clock: Instant,
    commands: mpsc::SyncSender<Command>,
    /// Answers the unit's type-61 update requests over this PC's internet
    /// connection; see [`crate::updater_relay`].
    updater_relay: Arc<UpdaterRelay>,
    /// The pointer a raw `press` mouse frame (or a hold whose touch-up could not
    /// be delivered) left down on the unit, if any. Every other screen gesture is
    /// refused while it is held, and the host never releases it by itself; see
    /// [`quadcortex::device::touch_guard`].
    touch_guard: Arc<Mutex<TouchGuard>>,
    /// Bumped by every request that must reach a connect attempt already
    /// running on the worker (session reset, disconnect, stop). The worker
    /// snapshots it when a connect starts and abandons that connect as soon as
    /// it changes; see [`ConnectInterrupt`].
    connect_interrupt: ConnectInterrupt,
    /// The `qc-native-usb` thread and how far its shutdown has got, so
    /// shutdown can wait for it exactly once. See [`WorkerSlot`].
    worker: WorkerThread,
}

/// Where the USB worker thread is in its life, shared with every
/// [`WorkerShutdown`]. Explicit so a worker one shutdown has joined, or given
/// up on, is never waited on again by another. Timed-out original handles
/// remain owned and may be acknowledged after actual completion: a controller that
/// `finish_shutdown` already stopped returns at once from its own Drop.
#[derive(Default)]
enum WorkerSlot {
    /// No worker thread was started (test controllers).
    #[default]
    Absent,
    /// Running, and nobody has started waiting for it.
    Running(thread::JoinHandle<()>),
    /// Another caller holds the handle and is waiting for it right now.
    Joining,
    /// A shutdown already ended. Never wait again; exact retained completion
    /// may upgrade this outcome after the original handle has actually joined.
    Stopped(WorkerExit),
}

/// The worker slot, shared between a controller and its shutdown handles.
type WorkerThread = Arc<WorkerThreadState>;

struct WorkerThreadState {
    slot: Mutex<WorkerSlot>,
    retirement: Arc<WorkerRetirement>,
    lifetime: Mutex<Option<Arc<WorkerLifetime>>>,
    teardown_verified: Arc<AtomicBool>,
}

impl std::ops::Deref for WorkerThreadState {
    type Target = Mutex<WorkerSlot>;
    fn deref(&self) -> &Self::Target {
        &self.slot
    }
}

impl Default for WorkerThreadState {
    fn default() -> Self {
        Self {
            slot: Mutex::new(WorkerSlot::Absent),
            retirement: WorkerRetirement::production(),
            lifetime: Mutex::new(None),
            teardown_verified: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl WorkerThreadState {
    fn running(
        handle: thread::JoinHandle<()>,
        retirement: Arc<WorkerRetirement>,
        lifetime: Arc<WorkerLifetime>,
        teardown_verified: Arc<AtomicBool>,
    ) -> Self {
        Self {
            slot: Mutex::new(WorkerSlot::Running(handle)),
            retirement,
            lifetime: Mutex::new(Some(lifetime)),
            teardown_verified,
        }
    }
}

impl WorkerThreadState {
    fn joined_exit(&self) -> WorkerExit {
        if self.teardown_verified.load(Ordering::Acquire) {
            WorkerExit::Joined
        } else {
            WorkerExit::TeardownUnverified
        }
    }
}

/// ONE shared budget for stopping and joining the device workers: how long
/// a shutdown waits, in total across all of them, for the USB workers to
/// return after `Stop`. It is not a bound on total shutdown time. The
/// pre-existing per-device disconnect waits that run before it (each
/// `DeviceController::disconnect`: queue admission plus a 1 s ack, device by
/// device) are outside this budget.
///
/// The worker's last act is dropping its connection, and `QcUsb::drop` is
/// what writes the orderly `connection(false)` teardown and then joins the HID
/// writer for up to its own 2 s budget (`FINISH_WRITES_JOIN_BUDGET`). This
/// covers that budget plus the worker reaching `Stop` at all. A process that
/// exits sooner than the teardown write leaves the QC holding a session for a
/// dead host (memory: stale-session-disconnect-livelocks-the-handshake).
pub const WORKER_EXIT_BUDGET: Duration = Duration::from_secs(3);

/// How stopping one USB worker ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExit {
    /// The worker returned and native teardown delivery was verified.
    /// This does not promise a separate acknowledgement from the device.
    Joined,
    /// The worker returned, but a native writer was retained, failed or canceled.
    /// Its original native handle and host lease may still remain owned.
    TeardownUnverified,
    /// No worker thread was ever started; there was no device to release.
    NoWorker,
    /// The worker did not return by the deadline; it was left running and
    /// its disconnect is not verified.
    TimedOut,
    /// Called on the worker thread itself, where joining would deadlock.
    OnWorkerThread,
    /// The worker panicked; it is gone, but nothing proves it disconnected.
    Panicked,
    /// Another caller is waiting for this worker right now; this one did not.
    JoinInProgress,
}

impl WorkerExit {
    /// Whether this outcome proves the worker returned and so wrote its
    /// orderly disconnect. Every other outcome with a worker behind it is
    /// "clean disconnect not verified", never success.
    pub fn verified_clean_disconnect(self) -> bool {
        matches!(self, WorkerExit::Joined)
    }
}

/// What one shutdown of several USB workers found, controller by controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerShutdownReport {
    /// Each controller's selected device id (if any) and how its worker ended.
    pub outcomes: Vec<(Option<String>, WorkerExit)>,
}

impl WorkerShutdownReport {
    /// Controllers that had a worker whose clean disconnect is not verified.
    pub fn unverified(&self) -> Vec<&(Option<String>, WorkerExit)> {
        self.outcomes
            .iter()
            .filter(|(_, exit)| *exit != WorkerExit::NoWorker && !exit.verified_clean_disconnect())
            .collect()
    }

    /// Every worker there was returned, so every disconnect was written.
    pub fn all_verified(&self) -> bool {
        self.unverified().is_empty()
    }

    /// One line for logs and the tray: which disconnects are not verified.
    pub fn summary(&self) -> String {
        let verified = self
            .outcomes
            .iter()
            .filter(|(_, exit)| exit.verified_clean_disconnect())
            .count();
        let unverified = self.unverified();
        if unverified.is_empty() {
            return format!("clean disconnect verified for {verified} device worker(s)");
        }
        let detail = unverified
            .iter()
            .map(|(device, exit)| format!("{} {exit:?}", device.as_deref().unwrap_or("default")))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "clean disconnect NOT verified for {} device worker(s) ({detail}); verified for {verified}",
            unverified.len()
        )
    }
}

/// Stop every controller's USB worker against ONE deadline, `budget` from
/// now: `Stop` goes to all of them first, then each is joined only for the
/// time left until that shared deadline, so this worker stop-and-join phase
/// is bounded by one budget however many devices there are (not counting the
/// `Stop` admission waits, which the same deadline also caps). It does not
/// bound the whole shutdown: the per-device disconnect waits before it are
/// separate. Controllers with no worker
/// thread get nothing queued. No lock is held while waiting.
pub fn stop_workers_within<'a>(
    controllers: impl IntoIterator<Item = &'a DeviceController>,
    budget: Duration,
) -> WorkerShutdownReport {
    let deadline = Instant::now() + budget;
    let stopping: Vec<_> = controllers
        .into_iter()
        .map(|controller| (controller.device_id(), controller.shutdown_handle()))
        .collect();
    for (_, shutdown) in &stopping {
        shutdown.request_stop(deadline, false);
    }
    WorkerShutdownReport {
        outcomes: stopping
            .into_iter()
            .map(|(device, shutdown)| (device, shutdown.join_until(deadline)))
            .collect(),
    }
}

/// Stops the USB worker and waits, bounded, for it to finish. Cloneable so an
/// exit path that does not own the controller (the stdio orphan guard) can
/// still release the device before `process::exit`.
#[derive(Clone)]
pub struct WorkerShutdown {
    commands: mpsc::SyncSender<Command>,
    connect_interrupt: ConnectInterrupt,
    worker: WorkerThread,
}

impl WorkerShutdown {
    #[cfg(test)]
    fn fixture_original_acknowledged(&self) -> bool {
        self.worker.retirement.reap();
        matches!(
            *self.worker.lock_recover(),
            WorkerSlot::Absent
                | WorkerSlot::Stopped(
                    WorkerExit::Joined
                        | WorkerExit::TeardownUnverified
                        | WorkerExit::Panicked
                        | WorkerExit::NoWorker
                )
        )
    }

    /// Stop this one worker: queue `Stop`, then wait for it until `budget`
    /// from now. A worker another shutdown already stopped, or is joining, is
    /// not stopped or waited on again. Never holds a lock while it waits.
    pub fn stop_and_join(&self, budget: Duration) -> WorkerExit {
        let deadline = Instant::now() + budget;
        self.request_stop(deadline, true);
        self.join_until(deadline)
    }

    /// Phase one: interrupt any connect and queue `Stop`, waiting for queue
    /// room no later than `deadline` (one attempt once it has passed). Skipped
    /// for a worker already stopped or being joined, and for a controller with
    /// no worker thread unless `without_worker` (Drop keeps queueing `Stop`
    /// there, as it always did).
    fn request_stop(&self, deadline: Instant, without_worker: bool) {
        let live = match &*self.worker.lock_recover() {
            WorkerSlot::Running(_) => true,
            WorkerSlot::Absent => without_worker,
            WorkerSlot::Joining | WorkerSlot::Stopped(_) => false,
        };
        if !live {
            return;
        }
        self.connect_interrupt.raise();
        let admission = deadline
            .saturating_duration_since(Instant::now())
            .min(CONTROL_COMMAND_ADMISSION);
        // Already gone is fine: the join then returns at once.
        let _ = send_control(
            &self.commands,
            Command::Stop,
            admission,
            Some(&self.connect_interrupt),
        );
    }

    /// Phase two: wait for the worker until `deadline`, not at all once it
    /// has passed. The handle is taken out under the slot lock and waited on
    /// after releasing it; the outcome is recorded so nobody waits again.
    fn join_until(&self, deadline: Instant) -> WorkerExit {
        self.worker.retirement.reap();
        let handle = {
            let mut slot = self.worker.lock_recover();
            match std::mem::replace(&mut *slot, WorkerSlot::Joining) {
                WorkerSlot::Running(handle) => handle,
                previous => {
                    let exit = match &previous {
                        WorkerSlot::Absent => WorkerExit::NoWorker,
                        WorkerSlot::Joining => WorkerExit::JoinInProgress,
                        WorkerSlot::Stopped(exit) => *exit,
                        WorkerSlot::Running(_) => unreachable!("matched above"),
                    };
                    *slot = previous;
                    return exit;
                }
            }
        };
        let (exit, retained) = wait_for_worker(handle, deadline);
        let exit = if exit == WorkerExit::Joined {
            self.worker.joined_exit()
        } else {
            exit
        };
        *self.worker.lock_recover() = WorkerSlot::Stopped(exit);
        if let Some(handle) = retained {
            let lifetime = self
                .worker
                .lifetime
                .lock_recover()
                .take()
                .expect("original worker owns its admission");
            self.worker
                .retirement
                .retain(handle, Arc::downgrade(&self.worker), lifetime);
        } else {
            self.worker.lifetime.lock_recover().take();
        }
        exit
    }
}

pub(crate) fn thread_signalled<T>(handle: &thread::JoinHandle<T>) -> bool {
    crate::platform::process::thread_signalled(handle)
}

/// Join `handle` if it returns by `deadline`; otherwise return its exact original
/// handle for retained ownership while it keeps running, and
/// say so. Logs every outcome that is not a verified clean disconnect.
fn wait_for_worker(
    handle: thread::JoinHandle<()>,
    deadline: Instant,
) -> (WorkerExit, Option<thread::JoinHandle<()>>) {
    if handle.thread().id() == thread::current().id() {
        // The worker can never own a controller, but if it ever drops one it
        // must not wait for itself. It is already on its way out.
        eprintln!(
            "QC device broker: worker shutdown requested from the worker itself; not waiting \
             (clean disconnect not verified)"
        );
        return (WorkerExit::OnWorkerThread, Some(handle));
    }
    while !thread_signalled(&handle) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            eprintln!(
                "QC device broker: the USB worker did not stop by the shutdown deadline; retaining \
                 its original thread (clean disconnect not verified)"
            );
            return (WorkerExit::TimedOut, Some(handle));
        }
        thread::sleep(remaining.min(Duration::from_millis(5)));
    }
    match handle.join() {
        Ok(()) => (WorkerExit::Joined, None),
        Err(_) => {
            eprintln!(
                "QC device broker: the USB worker panicked during shutdown (clean disconnect \
                 not verified)"
            );
            (WorkerExit::Panicked, None)
        }
    }
}

/// How a control request gets past a USB connect that is still running.
///
/// The worker is one thread, and `QcUsb::connect_device` blocks it for the
/// whole handshake and staged startup. A unit that never completes that
/// startup (2026-10-03: rebuilt every ~1.1 s for 13 minutes) used to leave the
/// worker inside the connect for good, so a reset, a disconnect and shutdown
/// all queued behind it and nothing could end the wedge short of killing the
/// process. A generation counter rather than a flag: it cannot be left set by a
/// request that arrived while no connect was running and so cancel the next,
/// unrelated one.
#[derive(Clone, Default)]
struct ConnectInterrupt(Arc<AtomicU64>);

impl ConnectInterrupt {
    fn raise(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }

    /// A predicate that turns true once [`Self::raise`] runs after this call.
    fn watch(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let counter = Arc::clone(&self.0);
        let generation = counter.load(Ordering::Acquire);
        move || counter.load(Ordering::Acquire) != generation
    }
}

/// How long a control command waits for room in the worker's queue.
const CONTROL_COMMAND_ADMISSION: Duration = Duration::from_secs(5);

/// Queue a command that must not be lost without ever blocking the caller
/// indefinitely: the worker may be inside a connect and draining nothing, in
/// which case a plain `send` on a full bounded queue parks the calling thread
/// for as long as the connect lasts.
fn send_control(
    commands: &mpsc::SyncSender<Command>,
    mut command: Command,
    budget: Duration,
    interrupt: Option<&ConnectInterrupt>,
) -> Result<(), String> {
    let deadline = Instant::now() + budget;
    loop {
        match commands.try_send(command) {
            Ok(()) => {
                // The initial raise makes room in a full queue. Repeat only
                // after the command is visible, so a worker that registered
                // its watch between that raise and queue insertion cannot
                // start an uncancellable handshake ahead of the command.
                if let Some(interrupt) = interrupt {
                    interrupt.raise();
                }
                return Ok(());
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("Native QC worker is not available".into())
            }
            Err(mpsc::TrySendError::Full(returned)) => {
                if Instant::now() >= deadline {
                    return Err("Native QC worker is busy and did not accept the request".into());
                }
                command = returned;
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Diagnostics are best effort. Dropping one when the queue is full is correct;
/// blocking a request thread on it is not - `handle` records `rpc-start` for
/// every call, `system.status` included, so with the worker inside a connect
/// the 257th status poll parked its connection thread for good. That thread is
/// what held each abandoned tray observer on the broker's client list.
fn queue_flight_event(commands: &mpsc::SyncSender<Command>, event: String, incident: bool) {
    let _ = commands.try_send(Command::RecordFlight { event, incident });
}

/// What a screen operation does to the held-pointer record once it is sent.
enum TouchEffect {
    /// Nothing to record.
    None,
    /// A raw release is going out: the pointer is lifted if it was delivered.
    Released,
}

/// A hold's handle on the touch guard, carried by its command so the worker can
/// record a pointer the hold could not lift.
struct HoldRecord {
    guard: Arc<Mutex<TouchGuard>>,
    x: f32,
    y: f32,
}

/// Why a paced touch sequence's error reports a pointer that may still be down:
/// its touch-up could not be sent, even after the one second attempt.
/// [`send_paced_sequence`] writes it and the hold bookkeeping looks for it.
const TOUCH_UP_UNSENT: &str = "the touch-up could not be sent";

/// Record in the touch guard a pointer that a hold could not lift. Only a hold
/// whose own touch-up failed, even after the one second attempt, is recorded: a
/// failure of the touch-down means nothing is known to be pressed, and a
/// release that went out means nothing is.
fn note_hold_outcome(hold: Option<&HoldRecord>, result: &Result<(), String>, now_ms: u64) {
    if let (Some(record), Err(error)) = (hold, result) {
        if error.contains(TOUCH_UP_UNSENT) {
            record.guard.lock_recover().note_unreleased_hold(
                record.x,
                record.y,
                now_ms,
                format!("the last hold's release is unconfirmed: {error}"),
            );
        }
    }
}

impl DeviceController {
    pub fn start() -> Result<Self, String> {
        Self::start_with_auto_connect(true)
    }

    /// Starts the stdio broker without racing its client's explicit reconnect.
    pub fn start_disconnected() -> Result<Self, String> {
        Self::start_with_auto_connect(false)
    }

    fn start_with_auto_connect(auto_connect: bool) -> Result<Self, String> {
        Self::start_for_device(None, auto_connect)
    }

    pub fn device_id(&self) -> Option<String> {
        self.device_id.lock_recover().clone()
    }

    pub fn claim_device(&self, id: &str) -> bool {
        let mut selected = self.device_id.lock_recover();
        selected.get_or_insert_with(|| id.to_string()) == id
    }

    pub fn start_for_device(id: Option<String>, auto_connect: bool) -> Result<Self, String> {
        Self::start_for_device_with_lease(id, auto_connect, crate::device_lease::default_path())
    }

    /// Like [`Self::start_for_device`] with the ownership record at `lease_path`
    /// (tests and unusual installs).
    pub fn start_for_device_with_lease(
        id: Option<String>,
        auto_connect: bool,
        lease_path: std::path::PathBuf,
    ) -> Result<Self, String> {
        Self::start_for_device_with_discovery(
            id,
            auto_connect,
            lease_path,
            DeviceDiscovery::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn start_for_device_with_test_discovery(
        id: Option<String>,
        auto_connect: bool,
        lease_path: std::path::PathBuf,
        enumerate: impl Fn() -> Result<Vec<quadcortex_hid::usb::QcUsbDevice>, String>
            + Send
            + Sync
            + 'static,
    ) -> Result<Self, String> {
        Self::start_for_device_with_discovery(
            id,
            auto_connect,
            lease_path,
            DeviceDiscovery {
                enumerate: Some(Arc::new(enumerate)),
            },
        )
    }

    pub(crate) fn connected_devices(
        &self,
    ) -> Result<Vec<quadcortex_hid::usb::QcUsbDevice>, String> {
        self.discovery.devices()
    }

    fn start_for_device_with_discovery(
        id: Option<String>,
        auto_connect: bool,
        lease_path: std::path::PathBuf,
        discovery: DeviceDiscovery,
    ) -> Result<Self, String> {
        let retirement = WorkerRetirement::production();
        // Refuse before creating decoder/USB threads or performing device effects.
        let lifetime = retirement.reserve()?;
        let decoder_lifetime = Arc::clone(&lifetime);
        let metadata_lifetime = Arc::clone(&lifetime);
        let device_id = Arc::new(Mutex::new(id));
        let worker_device_id = Arc::clone(&device_id);
        let identity_observation_enabled = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(BrokerStatus::default()));
        if !auto_connect {
            let mut status = state.lock_recover();
            status.phase = "disconnected".into();
            status.detail = "Waiting for a reconnect request".into();
        }
        let latest_messages = Arc::new(Mutex::new(HashMap::new()));
        let raw_events = Arc::new(RawEventBus::default());
        let state_event_log = Arc::new(Mutex::new(StateEventJournal::default()));
        let latest_tempo_clock = Arc::new(Mutex::new(None));
        let latest_master_volume = Arc::new(Mutex::new(None));
        let state_event_sequence = Arc::new(AtomicU64::new(0));
        let state_stream_id = uuid::Uuid::new_v4().to_string();
        let state_subscribers = Arc::new(Mutex::new(Vec::new()));
        let session_owner = Arc::new(Mutex::new(SessionRuntime::new(0)));
        let (state_messages, state_receiver) = mpsc::sync_channel(STATE_DECODER_QUEUE_CAPACITY);
        let state_catalogs = state_messages.clone();
        let session_clock = Instant::now();
        let decoded = StateDecoderShared {
            stream_id: state_stream_id.clone(),
            event_log: Arc::clone(&state_event_log),
            latest_tempo_clock: Arc::clone(&latest_tempo_clock),
            latest_master_volume: Arc::clone(&latest_master_volume),
            latest_sequence: Arc::clone(&state_event_sequence),
            session_owner: Arc::clone(&session_owner),
            subscribers: Arc::clone(&state_subscribers),
        };
        let decoder =
            start_owned_decoder_with_lifetime("qc-native-state", decoder_lifetime, move |stop| {
                run_state_decoder_with_lifetime(
                    decoded,
                    state_catalogs,
                    state_receiver,
                    session_clock,
                    stop,
                    metadata_lifetime,
                )
            })
            .map_err(|error| format!("Could not start native QC state decoder: {error}"))?;
        let (commands, receiver) = mpsc::sync_channel(DEVICE_COMMAND_QUEUE_CAPACITY);
        let connect_interrupt = ConnectInterrupt::default();
        let worker_interrupt = connect_interrupt.clone();
        let worker_state = Arc::clone(&state);
        let worker_messages = Arc::clone(&latest_messages);
        let worker_raw_events = Arc::clone(&raw_events);
        let worker_owner = Arc::clone(&session_owner);
        let worker_identity_observation = Arc::clone(&identity_observation_enabled);
        let updater_relay = Arc::new(UpdaterRelay::new());
        let worker_updater_relay = Arc::clone(&updater_relay);
        let worker_lifetime = Arc::clone(&lifetime);
        let teardown_verified = Arc::new(AtomicBool::new(true));
        let worker_teardown_verified = Arc::clone(&teardown_verified);
        let worker_discovery = discovery.clone();
        let worker = thread::Builder::new()
            .name("qc-native-usb".into())
            .spawn(move || {
                let _lifetime = lifetime;
                run(
                    WorkerContext {
                        discovery: worker_discovery,
                        _decoder: decoder,
                        device_id: worker_device_id,
                        identity_observation_enabled: worker_identity_observation,
                        session: worker_owner,
                        session_clock,
                        state: worker_state,
                        latest_messages: worker_messages,
                        raw_events: worker_raw_events,
                        state_messages,
                        connect_interrupt: worker_interrupt,
                        lease_path,
                        updater_relay: worker_updater_relay,
                        teardown_verified: worker_teardown_verified,
                    },
                    receiver,
                    auto_connect,
                )
            })
            .map_err(|error| format!("Could not start native QC USB worker: {error}"))?;
        Ok(Self {
            discovery,
            worker: Arc::new(WorkerThreadState::running(
                worker,
                retirement,
                worker_lifetime,
                teardown_verified,
            )),
            device_id,
            identity_observation_enabled,
            state_stream_id,
            state,
            latest_messages,
            raw_events,
            state_event_log,
            latest_tempo_clock,
            latest_master_volume,
            state_event_sequence,
            state_subscribers,
            session_owner,
            session_clock,
            commands,
            updater_relay,
            touch_guard: Arc::new(Mutex::new(TouchGuard::default())),
            connect_interrupt,
        })
    }

    /// Whether the session is open, and its phase, from the worker's cached
    /// status alone. Takes only the short status lock: no command reaches the
    /// worker, so it opens, reopens and waits for nothing. See
    /// [`crate::rpc::BrokerHandle::device_readiness`].
    pub fn connection_readiness(&self) -> (bool, String) {
        let status = self.state.lock_recover();
        (status.connected, status.phase.clone())
    }

    /// Test-only: what the worker would record for an opened or closed
    /// session, with no worker or USB behind it.
    #[cfg(test)]
    pub(crate) fn set_connection_for_test(&self, connected: bool, phase: &str) {
        let mut status = self.state.lock_recover();
        status.connected = connected;
        status.phase = phase.to_owned();
    }

    /// Test-only: hold the worker's status lock, as a worker thread updating
    /// the status would, until the guard drops.
    #[cfg(test)]
    pub(crate) fn hold_status_lock_for_test(&self) -> std::sync::MutexGuard<'_, BrokerStatus> {
        self.state.lock_recover()
    }

    #[cfg(test)]
    pub(crate) fn set_connected_for_test(&self) {
        let mut status = self.state.lock_recover();
        status.connected = true;
        status.phase = "ready".into();
    }

    /// Test-only: hold the retained session state exactly as a long device
    /// operation can while cached status and USB keepalives remain healthy.
    #[cfg(test)]
    pub(crate) fn hold_session_lock_for_test(&self) -> std::sync::MutexGuard<'_, SessionRuntime> {
        self.session_owner.lock_recover()
    }

    pub fn status(&self) -> BrokerStatus {
        let mut status = self.state.lock_recover().clone();
        // Before the session-owner lock is taken, not under it: this may read
        // the settings file, and the HID reader thread takes that lock for
        // every report it receives.
        let updater_relay = self.updater_relay.status();
        let owner = self.session_owner.lock_recover();
        if status.synchronized {
            if let Some(preset) = owner.current_preset() {
                status.active_preset_location = Some(preset.preset_location.clone());
                status.active_preset_name = Some(preset.preset_name.clone());
            }
        }
        // The owner is authoritative and session-scoped: a disconnect or a new
        // epoch has already cleared it there.
        status.suspension = owner.suspension();
        status.device_suspended = status.suspension.active.is_some();
        status.retention = self.raw_events.retention.snapshot();
        status.updater_relay = updater_relay;
        status
    }

    /// Cache-only RPC diagnostics. Suspension is authoritative in the session
    /// owner, not the short worker record. If either cache is busy, omit the
    /// diagnostics rather than reporting a suspended unit as unsuspended.
    /// Unlike `status`, this performs no settings I/O and never waits for a lock.
    pub(crate) fn try_cached_status(&self) -> Option<BrokerStatus> {
        let updater_relay = self.updater_relay.try_cached_status()?;
        let mut status = match self.state.try_lock() {
            Ok(status) => status.clone(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        let owner = match self.session_owner.try_lock() {
            Ok(owner) => owner,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if status.synchronized {
            if let Some(preset) = owner.current_preset() {
                status.active_preset_location = Some(preset.preset_location.clone());
                status.active_preset_name = Some(preset.preset_name.clone());
            }
        }
        status.suspension = owner.suspension();
        status.device_suspended = status.suspension.active.is_some();
        status.retention = self.raw_events.retention.snapshot();
        status.updater_relay = updater_relay;
        Some(status)
    }

    /// The tray's display projection. Unlike [`Self::status`], this never
    /// waits for either worker lock: the tray's own liveness indicator must
    /// not call a healthy broker unresponsive merely because a device
    /// operation is updating retained session state.
    ///
    /// The short status record already carries every field the tray renders.
    /// When the session owner is immediately available, use its authoritative
    /// preset address and name too. When either lock is busy, return promptly;
    /// the tray state loop retains its last projection and tries again on its
    /// next 250 ms tick.
    pub(crate) fn try_tray_status(&self) -> Option<BrokerStatus> {
        let mut status = match self.state.try_lock() {
            Ok(status) => status.clone(),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if status.synchronized {
            match self.session_owner.try_lock() {
                Ok(owner) => {
                    if let Some(preset) = owner.current_preset() {
                        status.active_preset_location = Some(preset.preset_location.clone());
                        status.active_preset_name = Some(preset.preset_name.clone());
                    }
                }
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    if let Some(preset) = poisoned.into_inner().current_preset() {
                        status.active_preset_location = Some(preset.preset_location.clone());
                        status.active_preset_name = Some(preset.preset_name.clone());
                    }
                }
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
        }
        Some(status)
    }

    /// Read the update relay's switch from the broker settings in `directory`
    /// (the app's local data directory, where the broker descriptor sits).
    /// Without this the relay runs on its defaults and nothing can turn it
    /// off; every broker mode that can carry the unit's requests calls it.
    pub fn configure_updater_relay(&self, directory: std::path::PathBuf) {
        self.updater_relay.set_settings_directory(directory);
    }

    /// The refusal for a client request that would reach the device, while the
    /// device holds the session suspended. Counted as a refused request.
    pub fn suspension_refusal(&self) -> Option<String> {
        self.session_owner.lock_recover().suspension_refusal()
    }

    /// Only an attached observer opts into the one-time display-name read.
    /// Disabling does not erase the session cache or reset its attempt latch.
    pub fn set_identity_observation_enabled(&self, enabled: bool) {
        self.identity_observation_enabled
            .store(enabled, Ordering::Release);
    }

    pub fn record_flight_event(&self, event: impl Into<String>) {
        queue_flight_event(&self.commands, event.into(), false);
    }

    /// Quarantine with no known originating RPC (legacy callers and tests):
    /// the widest scope, which gates every mutation.
    pub fn quarantine_writes(&self, reason: impl Into<String>) {
        self.quarantine_writes_for("unknown", reason);
    }

    /// Quarantine after `method` was transmitted and could not be settled.
    /// The method fixes the quarantine's scope and therefore what it still
    /// lets through; see [`crate::write_quarantine`].
    pub fn quarantine_writes_for(&self, method: &str, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut status = self.state.lock_recover();
            let since = quadcortex::time::unix_time_ms();
            status.writes_quarantined = true;
            status.write_quarantine_reason = Some(reason.clone());
            status.write_quarantine = Some(crate::write_quarantine::WriteQuarantine::new(
                method,
                reason,
                since,
                status.session_epoch,
            ));
        }
        queue_flight_event(&self.commands, format!("write-quarantined {method}"), true);
    }

    pub fn write_quarantine_reason(&self) -> Option<String> {
        self.state.lock_recover().write_quarantine_reason.clone()
    }

    pub fn write_quarantine(&self) -> Option<crate::write_quarantine::WriteQuarantine> {
        let status = self.state.lock_recover();
        status.writes_quarantined.then(|| {
            status.write_quarantine.clone().unwrap_or_else(|| {
                // A quarantine set through a path that predates the structured
                // record: gate everything, as it always did.
                crate::write_quarantine::WriteQuarantine::new(
                    "unknown",
                    status.write_quarantine_reason.clone().unwrap_or_default(),
                    0,
                    status.session_epoch,
                )
            })
        })
    }

    /// Clear the quarantine, but only the exact record that was found stale (set in an
    /// earlier session epoch): a newer ambiguous write must survive. No read is taken.
    pub fn clear_stale_write_quarantine(&self, since_unix_ms: u64) -> bool {
        let cleared = {
            let mut status = self.state.lock_recover();
            let same = status.write_quarantine.as_ref().is_some_and(|quarantine| {
                quarantine.since_unix_ms == since_unix_ms
                    && crate::write_quarantine::is_stale(quarantine, status.session_epoch)
            });
            if same {
                status.writes_quarantined = false;
                status.write_quarantine_reason = None;
                status.write_quarantine = None;
            }
            same
        };
        if cleared {
            queue_flight_event(
                &self.commands,
                "write-quarantine-cleared-stale-epoch".into(),
                false,
            );
        }
        cleared
    }

    pub fn session_epoch(&self) -> u64 {
        self.state.lock_recover().session_epoch
    }

    /// Target projection must not acquire the retained device/session tree.
    /// A busy status writer leaves the previous projection intact; it must
    /// never turn an unknown physical fact into permission to select demo.
    pub(crate) fn try_target_readiness(&self) -> Option<(bool, bool, u64)> {
        let state = match self.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        Some((state.connected, state.synchronized, state.session_epoch))
    }

    /// Opaque per-controller UUID, unrelated to any hardware serial or path.
    pub(crate) fn target_stream_id(&self) -> &str {
        &self.state_stream_id
    }

    /// Request a reconnect that leaves a healthy session alone. The returned
    /// outcome says which of the two happened; see [`ReconnectOutcome`].
    pub fn reconnect(&self) -> Result<ReconnectOutcome, String> {
        self.request_reconnect(false)
    }
    /// Queue an unforced reconnect behind any accepted close already in the
    /// worker's FIFO. Unlike [`Self::reconnect`], this does not trust cached
    /// status to return early; the worker checks its live session when it
    /// reaches the command, making its reply a barrier behind the close.
    pub(crate) fn reconnect_after_pending_close(&self) -> Result<ReconnectOutcome, String> {
        self.request_reconnect_inner(false, true)
    }
    /// The same command with the teardown forced. Always
    /// [`ReconnectOutcome::Restarted`] when it returns at all.
    pub fn reset_session(&self) -> Result<ReconnectOutcome, String> {
        self.request_reconnect(true)
    }
    fn request_reconnect(&self, force: bool) -> Result<ReconnectOutcome, String> {
        self.request_reconnect_inner(force, false)
    }
    fn request_reconnect_inner(
        &self,
        force: bool,
        enqueue_when_cached_ready: bool,
    ) -> Result<ReconnectOutcome, String> {
        if force {
            // A forced reset must reach a connect that is still running: the
            // worker answers no command until it leaves it, and a wedged
            // startup never does on its own.
            self.connect_interrupt.raise();
        } else if !enqueue_when_cached_ready {
            // An unforced reconnect leaves a live or forming session alone, so
            // there is nothing to queue behind a connect in progress: answer
            // from the published phase exactly as the worker would.
            let phase = self.state.lock_recover().phase.clone();
            if reconnect_outcome(false, false, &phase) == ReconnectOutcome::AlreadyConnected {
                return Ok(ReconnectOutcome::AlreadyConnected);
            }
        }
        let (reply, response) = mpsc::channel();
        send_control(
            &self.commands,
            Command::Reconnect { force, reply },
            CONTROL_COMMAND_ADMISSION,
            force.then_some(&self.connect_interrupt),
        )?;
        response
            .recv_timeout(Duration::from_millis(
                quadcortex::profile::READY_WAIT_TIMEOUT_MS,
            ))
            .map_err(|_| "Native QC worker did not acknowledge the reconnect request".to_string())
    }
    pub fn disconnect(&self) -> Result<(), String> {
        self.disconnect_with_admission(None)
    }

    pub(crate) fn disconnect_with_admission(
        &self,
        admission: Option<Box<dyn Send>>,
    ) -> Result<(), String> {
        let (_, result) = self.disconnect_with_admission_report(admission);
        result
    }

    /// Reports whether the close command entered the native worker queue
    /// separately from whether the worker acknowledged it. A timeout after
    /// queue admission can still be followed by a successful close, so callers
    /// that manage reconnect state must not treat it like a rejected command.
    pub(crate) fn disconnect_with_admission_report(
        &self,
        admission: Option<Box<dyn Send>>,
    ) -> (bool, Result<(), String>) {
        self.connect_interrupt.raise();
        let (reply, response) = mpsc::channel();
        if let Err(error) = send_control(
            &self.commands,
            Command::Disconnect {
                reply,
                _admission: admission,
            },
            CONTROL_COMMAND_ADMISSION,
            Some(&self.connect_interrupt),
        ) {
            return (false, Err(error));
        }
        let result = response
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "Native QC worker did not acknowledge the disconnect request".to_string())
            .and_then(|result| result);
        (true, result)
    }

    pub fn latest_message(&self, message_type: u16) -> Option<IncomingMessage> {
        self.latest_messages
            .lock_recover()
            .get(&message_type)
            .cloned()
    }

    pub fn state_events_since(&self, sequence: u64, limit: usize) -> Vec<DecodedStateFrame> {
        self.state_event_page(sequence, limit).frames
    }

    pub fn state_event_page(&self, sequence: u64, limit: usize) -> StateEventPage {
        // The journal append and high-water update use this same lock. Returning
        // one coherent page prevents a concurrent append from advertising a
        // tail that was never considered when selecting its frames.
        let log = self.state_event_log.lock_recover();
        let frames = log.page(sequence, limit);
        StateEventPage {
            native: true,
            stream_id: self.state_stream_id.clone(),
            latest_sequence: self.state_event_sequence.load(Ordering::Acquire),
            frames,
        }
    }

    pub fn latest_state_sequence(&self) -> u64 {
        self.state_event_sequence.load(Ordering::Acquire)
    }

    pub fn latest_tempo_clock(&self) -> Option<DecodedStateFrame> {
        self.latest_tempo_clock.lock_recover().clone()
    }

    pub fn latest_master_volume(&self) -> Option<DecodedStateFrame> {
        self.latest_master_volume.lock_recover().clone()
    }

    pub fn subscribe_state_events(&self) -> StateSubscription {
        let (sender, receiver) = budgeted_queue(
            QueueKind::State,
            SUBSCRIBER_QUEUE_CAPACITY,
            STATE_SUBSCRIBER_QUEUE_MAX_BYTES,
            Arc::clone(&self.raw_events.retention),
        );
        let identity = Arc::new(());
        self.state_subscribers.lock_recover().push(StateSubscriber {
            identity: Arc::clone(&identity),
            sender,
        });
        StateSubscription {
            receiver,
            identity,
            subscribers: Arc::downgrade(&self.state_subscribers),
        }
    }

    /// Test-only hook that fans a synthetic frame out to every subscriber
    /// exactly as `publish_state_frame` does for a real USB-decoded one.
    ///
    /// There is no fake USB adapter wired in at this layer, so integration
    /// tests that need a `device.stateFrame` push to arrive on a live
    /// subscription (e.g. `rpc.rs`'s TCP multi-client tests) go through this
    /// instead of driving the real decoder.
    #[cfg(test)]
    pub(crate) fn inject_state_frame_for_test(&self, frame: DecodedStateFrame) {
        publish_state_frame(
            &self.state_event_log,
            &self.state_event_sequence,
            &self.state_subscribers,
            frame,
        );
    }

    /// Test-only: a controller holding a live, unsynchronized session with no
    /// worker behind it. Commands queue to the returned receiver.
    #[cfg(test)]
    pub(crate) fn live_session_for_test() -> (Self, TestCommands) {
        let (controller, commands) = tests::refresh_controller();
        (controller, TestCommands(commands))
    }

    /// Test-only: a live session whose staged startup reached `Connected`
    /// and whose retained directory holds exactly `folders`, each a complete
    /// 256-slot preset setlist.
    #[cfg(test)]
    pub(crate) fn connected_catalog_session_for_test(folders: &[&str]) -> (Self, TestCommands) {
        let (controller, commands) = tests::connected_catalog_controller(folders);
        (controller, TestCommands(commands))
    }

    /// Test-only: reduce one received frame through the session owner exactly
    /// as the USB worker does for a decoded inbound message.
    #[cfg(test)]
    pub(crate) fn observe_device_frame_for_test(&self, message_type: u16, payload: &[u8]) {
        let mut owner = self.session_owner.lock_recover();
        let connection = owner
            .session_fence()
            .map(|fence| {
                serde_json::to_value(fence).unwrap()["connection"]
                    .as_u64()
                    .unwrap()
            })
            .expect("live test session");
        let ticket = owner.capture_observation(connection).unwrap();
        owner.observe_state(&ticket, message_type, payload).unwrap();
    }

    pub fn subscribe_raw_events(&self) -> RawSubscription {
        subscribe_raw(&self.raw_events)
    }

    pub fn block_details(&self, row: u32, column: u32) -> Result<Option<BlockDetails>, String> {
        self.guarded_block_details(row, column, &serde_json::Value::Null)
    }

    pub fn guarded_block_details(
        &self,
        row: u32,
        column: u32,
        params: &serde_json::Value,
    ) -> Result<Option<BlockDetails>, String> {
        if row > 3 || column > 9 {
            return Err("Block coordinates must address rows 0-3 and columns 0-9".into());
        }
        let owner = self.session_owner.lock_recover();
        let current = owner
            .current_preset()
            .ok_or_else(|| "No Quad Cortex preset has been synchronized yet".to_owned())?;
        quadcortex::device::request::assert_expected_preset(Some(current), params)?;
        if !current.grid_layout_complete {
            return Err(format!(
                "{}The retained block details are not authoritative while Grid topology is incomplete",
                "STATE_UNAVAILABLE: "
            ));
        }
        Ok(owner.state.block_details(row, column))
    }

    pub fn lane_control_details(
        &self,
        row: u32,
        control: &str,
    ) -> Result<Option<BlockDetails>, String> {
        self.guarded_lane_control_details(row, control, &serde_json::Value::Null)
    }

    pub fn guarded_lane_control_details(
        &self,
        row: u32,
        control: &str,
        params: &serde_json::Value,
    ) -> Result<Option<BlockDetails>, String> {
        if row > 3 || !matches!(control, "inputGate" | "laneOutput") {
            return Err("Lane control must be inputGate or laneOutput on rows 0-3".into());
        }
        let owner = self.session_owner.lock_recover();
        quadcortex::device::request::assert_expected_preset(owner.current_preset(), params)?;
        Ok(owner.state.lane_control_details(row, control))
    }

    /// Flight-recorder description of the retained grid as a move frame will
    /// see it; see `StateDecoder::move_grid_layout_diagnostic`.
    pub fn move_grid_layout_diagnostic(&self) -> String {
        self.session_owner
            .lock_recover()
            .state
            .move_grid_layout_diagnostic()
    }

    pub fn gateway_snapshot(&self) -> Option<GatewaySnapshot> {
        self.session_owner.lock_recover().current_preset().cloned()
    }

    /// Project details only from the exact native revision certified by a write.
    /// The comparison and projection share the session-owner lock: a reconnect,
    /// preset change, or later update cannot be combined with an older result.
    pub(crate) fn verified_write_details(
        &self,
        result: &serde_json::Value,
        row: u32,
        column: Option<u32>,
        control: Option<&str>,
    ) -> Result<Option<BlockDetails>, String> {
        let owner = self.session_owner.lock_recover();
        let Some(current) = owner.current_preset() else {
            return Ok(None);
        };
        if validate_verified_projection_version(current, result).is_err() {
            // Keep the already-verified evidence, but do not supplement it with
            // details from a later revision or claim the landed write failed.
            return Ok(None);
        }
        match (column, control) {
            (Some(column), None) => Ok(owner.state.block_details(row, column)),
            (None, Some(control)) => Ok(owner.state.lane_control_details(row, control)),
            _ => Err("A verified projection requires one block or lane target".into()),
        }
    }

    pub fn gateway_preflight_observation(&self, method: &str) -> Option<serde_json::Value> {
        self.session_owner
            .lock_recover()
            .decoded_preflight_observation(method)
    }

    pub fn plan_gateway_write(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<(GatewayWritePlan, DispatchContext), String> {
        let started_ms = self.session_clock.elapsed().as_millis() as u64;
        let owner = self.session_owner.lock_recover();
        let state = &owner.state;
        let plan = runtime_request::plan_gateway_write_with_state(method, params, state)?;
        let context = owner.context_for_gateway_write(method, params, &plan.write, started_ms)?;
        Ok((plan, context))
    }

    /// Revalidates a host-transport write's original session fence immediately
    /// before its intent enters the external MIDI worker queue.
    pub fn validate_gateway_write_fence(&self, fence: &DispatchContext) -> Result<(), String> {
        let now_ms = self.session_clock.elapsed().as_millis() as u64;
        let mut owner = self.session_owner.lock_recover();
        owner
            .plan_operation_in_context(fence, now_ms, 2_000)
            .map(|_| ())
    }

    /// Validate a no-op plan and copy its observation in the same owner lease.
    /// A separate validation followed by an unfenced snapshot could certify a
    /// replacement session's state as this original plan's preflight result.
    pub(crate) fn gateway_snapshot_for_plan(
        &self,
        fence: &DispatchContext,
    ) -> Result<GatewaySnapshot, String> {
        let mut owner = self.session_owner.lock_recover();
        let now_ms = self.session_clock.elapsed().as_millis() as u64;
        owner.plan_operation_in_context(fence, now_ms, 2_000)?;
        owner
            .current_preset()
            .cloned()
            .ok_or_else(|| "No retained snapshot for no-op preflight".into())
    }

    /// Capture a session fence for revalidation by the host-MIDI worker just
    /// before it enters the native endpoint. The owner lock is released when
    /// this callback returns, before the potentially blocking native send.
    pub fn gateway_write_fence_validator(
        &self,
        fence: DispatchContext,
    ) -> impl FnOnce() -> Result<(), String> + Send + 'static {
        let owner = Arc::clone(&self.session_owner);
        let clock = self.session_clock;
        move || {
            let now_ms = clock.elapsed().as_millis() as u64;
            owner
                .lock_recover()
                .plan_operation_in_context(&fence, now_ms, 2_000)
                .map(|_| ())
        }
    }

    /// Wait for a synchronized snapshot that satisfies `predicate`.
    ///
    /// Both exits answer the same question, through [`matching_snapshot`]: the
    /// deadline path used to `return Some(snapshot)` with the predicate false,
    /// while the channel-closed path applied it. `rpc.rs` treats `Some` as proof
    /// and never re-checks, so an expired wait handed a caller a snapshot it had
    /// already rejected and called it a verified readback. A timeout must not
    /// return the success path's value.
    pub fn wait_for_gateway_snapshot(
        &self,
        timeout: Duration,
        predicate: impl Fn(&GatewaySnapshot) -> bool,
    ) -> Option<GatewaySnapshot> {
        let events = self.subscribe_state_events();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(matched) = matching_snapshot(self.gateway_snapshot(), &predicate) {
                return Some(matched);
            }
            if Instant::now() >= deadline {
                return None;
            }
            if events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_err()
            {
                return matching_snapshot(self.gateway_snapshot(), &predicate);
            }
        }
    }

    pub fn list_models(&self) -> Result<ModelList, String> {
        self.session_owner.lock_recover().state.model_list()
    }

    pub fn preset_folders(&self) -> Vec<PresetFolder> {
        self.session_owner.lock_recover().state.preset_folders()
    }

    pub fn observed_preset_list(&self, key: &str) -> Option<PresetList> {
        let owner = self.session_owner.lock_recover();
        owner
            .state
            .preset_library()
            .observed_list(key, owner.state.snapshot())
    }

    /// Wait for *this* USB session's own directory publication to include the
    /// capture/IR folder `key`, without asking the device for anything.
    ///
    /// `device.captures` used to send `readFolders(false)` and wait 30 s for a
    /// reply (two early recorded sessions saw no inbound File traffic after such
    /// a read; the unit does answer one, ESTABLISHED on CorOS 4.1.0 on
    /// 2026-10-07, so this wait is a design choice, not a device limit). The device can republish its whole directory on its own
    /// in-session state rebuilds (`syncing` -> `ready`), independently of any
    /// read request, so a read that happens to return during one of those
    /// rebuilds is satisfied by an unrelated republication rather than an
    /// answer to its own request - the same false success the preset catalog
    /// reads were showing. So the only honest wait is for the publication this
    /// session already paid for.
    ///
    /// The raw event bus is a wake-up hint, not the cache: a frame reaches
    /// subscribers before the state decoder ingests it, so the per-iteration
    /// wait is bounded rather than trusting one more frame to arrive.
    pub fn wait_for_library_folder(
        &self,
        kind: quadcortex::device::native_library::LibraryKind,
        key: &str,
        timeout: Duration,
    ) -> Result<Option<LibraryEntries>, String> {
        let deadline = Instant::now() + timeout;
        let fence = self.session_owner.lock_recover().session_fence()?;
        let events = self.subscribe_raw_events();
        loop {
            {
                let owner = self.session_owner.lock_recover();
                if !owner.accepts_fence(&fence) {
                    return Err("QC session changed during library listing".into());
                }
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                if let Some(entries) = owner.retained_library_catalog(
                    &fence,
                    kind,
                    key,
                    self.session_clock.elapsed().as_millis() as u64,
                )? {
                    return Ok((Instant::now() < deadline).then_some(entries));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(PRESET_PUBLICATION_POLL_INTERVAL);
            match events.recv_timeout(wait) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    pub fn plan_preset_recall(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<(runtime_request::PresetRecallPlan, DispatchContext), String> {
        let started_ms = self.session_clock.elapsed().as_millis() as u64;
        let owner = self.session_owner.lock_recover();
        let plan = runtime_request::plan_preset_recall(method, params, owner.current_preset())?;
        let context = owner.context_for_gateway_write(
            method,
            params,
            &runtime_request::PlannedWrite::HidCommand(plan.command.clone()),
            started_ms,
        )?;
        Ok((plan, context))
    }

    /// Read retained native evidence in the original USB session. Host wakeups
    /// only recheck availability; they do not request or infer a new generation.
    ///
    /// `Ok(Err(_))` is the old `Ok(None)`: nothing to project within `timeout`.
    /// It says why, from the same owner state under the same lock that found
    /// nothing, so the caller's error can diagnose itself.
    pub fn wait_for_preset_catalog(
        &self,
        method: &str,
        requested_key: Option<&str>,
        timeout: Duration,
    ) -> Result<Result<serde_json::Value, Box<PresetCatalogUnpublished>>, String> {
        let started = Instant::now();
        let deadline = started + timeout;
        let (fence, key) = {
            let owner = self.session_owner.lock_recover();
            let key = if method == "device.listPresetFolders" {
                String::new()
            } else {
                requested_key
                    .or_else(|| {
                        owner
                            .current_preset()
                            .map(|state| state.setlist_key.as_str())
                    })
                    .filter(|key| !key.is_empty())
                    .ok_or("No active preset setlist has been synchronized")?
                    .to_owned()
            };
            (owner.session_fence()?, key)
        };
        let events = self.subscribe_raw_events();
        let absence = loop {
            {
                let owner = self.session_owner.lock_recover();
                if !owner.accepts_fence(&fence) {
                    return Err("QC session changed during catalog listing".into());
                }
                let now_ms = self.session_clock.elapsed().as_millis() as u64;
                if Instant::now() >= deadline {
                    break owner.preset_catalog_absence(&fence, method, &key, now_ms)?;
                }
                if let Some(value) = owner.retained_preset_catalog(&fence, method, &key, now_ms)? {
                    if Instant::now() < deadline {
                        return Ok(Ok(value));
                    }
                    break owner.preset_catalog_absence(&fence, method, &key, now_ms)?;
                }
                // The directory is published once per USB session. When it is
                // complete and the key is not in it, no wakeup can change the
                // answer, so say so now rather than at the deadline.
                if let Some(absence) =
                    owner.settled_preset_catalog_absence(&fence, method, &key, now_ms)?
                {
                    break absence;
                }
            }
            let now = Instant::now();
            let wait = deadline
                .saturating_duration_since(now)
                .min(PRESET_PUBLICATION_POLL_INTERVAL);
            if wait.is_zero() {
                continue;
            }
            match events.recv_timeout(wait) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let owner = self.session_owner.lock_recover();
                    let now_ms = self.session_clock.elapsed().as_millis() as u64;
                    break owner.preset_catalog_absence(&fence, method, &key, now_ms)?;
                }
            }
        };
        // The owner lock is released before the status lock is taken:
        // `status()` takes them in the other order.
        let (file_frames_received, file_messages_sent, connection_frames_received, session_epoch) = {
            let status = self.state.lock_recover();
            let received = |kind: u16| {
                status
                    .messages_received_by_type
                    .get(&kind)
                    .copied()
                    .unwrap_or(0)
            };
            (
                received(profile::MESSAGE_TYPE_FILE),
                status
                    .messages_sent_by_type
                    .get(&profile::MESSAGE_TYPE_FILE)
                    .copied()
                    .unwrap_or(0),
                received(profile::MESSAGE_TYPE_CONNECTION),
                status.session_epoch,
            )
        };
        Ok(Err(Box::new(PresetCatalogUnpublished {
            method: method.to_owned(),
            setlist_key: (!key.is_empty()).then_some(key),
            setlist_key_unknown: absence.setlist_key_unknown(),
            absence,
            file_frames_received,
            file_messages_sent,
            connection_frames_received,
            session_epoch,
            waited_ms: started.elapsed().as_millis() as u64,
            timeout_ms: timeout.as_millis() as u64,
        })))
    }

    fn write_admission_for(
        &self,
        timeout: Duration,
        fence: Option<&DispatchContext>,
    ) -> Result<OperationTicket, String> {
        let mut owner = self.session_owner.lock_recover();
        let now_ms = self.session_clock.elapsed().as_millis() as u64;
        let timeout_ms = timeout.as_millis().min(u128::from(u64::MAX)) as u64;
        match fence {
            Some(fence) => owner.plan_operation_in_context(fence, now_ms, timeout_ms),
            None => owner.plan_operation(now_ms, timeout_ms),
        }
    }

    fn read_admission(
        &self,
        epoch: u64,
        fence: &SessionFence,
        started: Instant,
        budget: Duration,
    ) -> Result<RequestAdmission, String> {
        let mut owner = self.session_owner.lock_recover();
        let operation = owner.plan_operation_for(
            fence,
            started
                .saturating_duration_since(self.session_clock)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            budget.as_millis().min(u128::from(u64::MAX)) as u64,
        )?;
        Ok(RequestAdmission {
            epoch,
            operation,
            clock: self.session_clock,
        })
    }

    pub fn send(&self, message_type: u16, payload: Vec<u8>) -> Result<(), String> {
        self.send_for(message_type, payload, None, true)
    }

    pub(crate) fn begin_gateway_verification(
        &self,
        context: &DispatchContext,
        verification: GatewayVerification,
        after_sequence: u64,
        policy: &GatewayWriteVerificationPolicy,
    ) -> Result<GatewayVerificationWait<'_>, String> {
        let mut owner = self.session_owner.lock_recover();
        let runtime = GatewayVerificationRuntime::new(
            verification,
            u128::from(after_sequence),
            self.session_clock.elapsed().as_millis() as u64,
            policy,
        );
        let id = owner.begin_gateway_verification(context, runtime)?;
        Ok(GatewayVerificationWait {
            controller: self,
            id,
        })
    }

    fn wait_for_native_write(
        &self,
        operation: &OperationTicket,
        frame_count: usize,
        admission: mpsc::Receiver<Result<(), String>>,
        delivery_events: mpsc::Receiver<()>,
    ) -> Result<(), String> {
        let remaining = || {
            Duration::from_millis(
                operation.remaining_ms(self.session_clock.elapsed().as_millis() as u64),
            )
        };
        admission.recv_timeout(remaining())
            .map_err(|error| format!("QC write admission was not confirmed within its original budget: {error}; device effect may be uncertain"))??;
        loop {
            let wait = remaining();
            if wait.is_zero() {
                return Err("QC native delivery was not confirmed within its original budget; device effect may be uncertain".into());
            }
            {
                let owner = self.session_owner.lock_recover();
                owner.validate_operation(
                    operation,
                    self.session_clock.elapsed().as_millis() as u64,
                )?;
                if owner.operation_delivery_complete(operation, frame_count)? {
                    return Ok(());
                }
            }
            delivery_events.recv_timeout(wait).map_err(|error| {
                format!(
                    "QC native delivery was not confirmed: {error}; device effect may be uncertain"
                )
            })?;
        }
    }

    fn send_for(
        &self,
        message_type: u16,
        payload: Vec<u8>,
        fence: Option<&DispatchContext>,
        wait: bool,
    ) -> Result<(), String> {
        self.send_for_with_budget(message_type, payload, fence, wait, Duration::from_secs(2))
    }

    fn send_for_with_budget(
        &self,
        message_type: u16,
        payload: Vec<u8>,
        fence: Option<&DispatchContext>,
        wait: bool,
        budget: Duration,
    ) -> Result<(), String> {
        self.send_for_with_budget_and_admission(message_type, payload, fence, wait, budget, || {})
    }

    fn send_for_with_budget_and_admission(
        &self,
        message_type: u16,
        payload: Vec<u8>,
        fence: Option<&DispatchContext>,
        wait: bool,
        budget: Duration,
        on_admitted: impl FnOnce(),
    ) -> Result<(), String> {
        let admission = self.write_admission_for(budget, fence)?;
        let delivery_events = if wait {
            Some(self.session_owner.lock_recover().watch_deliveries()?)
        } else {
            None
        };
        let (sender, receiver) = mpsc::channel();
        self.commands
            .try_send(Command::Send(
                message_type,
                payload,
                admission.clone(),
                sender,
            ))
            .map_err(|error| error.to_string())?;
        on_admitted();
        if !wait {
            return Ok(());
        }
        self.wait_for_native_write(
            &admission,
            1,
            receiver,
            delivery_events.expect("waiting caller has delivery observer"),
        )
    }

    pub fn send_command(&self, message: OutboundMessage) -> Result<(), String> {
        self.send(message.message_type, message.payload)
    }

    pub fn send_command_for_plan(
        &self,
        message: OutboundMessage,
        fence: &DispatchContext,
    ) -> Result<(), String> {
        self.send_for(message.message_type, message.payload, Some(fence), true)
    }

    /// Queue a realtime intent without waiting for native delivery. The worker
    /// and writer still enforce its original session identity and caller budget.
    pub fn send_realtime_command(&self, message: OutboundMessage) -> Result<(), String> {
        self.send_realtime_for(message, None)
    }

    pub fn send_realtime_command_for_plan(
        &self,
        message: OutboundMessage,
        fence: &DispatchContext,
    ) -> Result<(), String> {
        self.send_realtime_for(message, Some(fence))
    }

    fn send_realtime_for(
        &self,
        message: OutboundMessage,
        fence: Option<&DispatchContext>,
    ) -> Result<(), String> {
        if !self.state.lock_recover().connected {
            return Err("Quad Cortex is not connected".into());
        }
        let admission = self.write_admission_for(Duration::from_secs(2), fence)?;
        match self.commands.try_send(Command::SendRealtime(
            message.message_type,
            message.payload,
            admission,
        )) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => {
                Err("Native QC command queue is busy; retry the realtime control".into())
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err("Native QC worker is not available".into())
            }
        }
    }

    // One internal entry point for every sequence: plain, hold-and-release and fenced writes
    // differ only in these arguments, so a parameter struct would add a type for a single caller.
    #[allow(clippy::too_many_arguments)]
    fn send_sequence_for(
        &self,
        messages: Vec<OutboundMessage>,
        delay: Duration,
        interval: Duration,
        release_on_failure: Option<OutboundMessage>,
        hold: Option<(f32, f32)>,
        fence: Option<&DispatchContext>,
        wait: bool,
    ) -> Result<OperationTicket, String> {
        if messages.is_empty() {
            return Err("QC write sequence must contain a frame".into());
        }
        let frame_count = messages.len();
        let (sender, receiver) = mpsc::channel();
        let message_count = u32::try_from(messages.len()).unwrap_or(u32::MAX);
        let admission = self.write_admission_for(
            delay + interval.saturating_mul(message_count) + Duration::from_secs(2),
            fence,
        )?;
        let delivery_events = if wait {
            Some(self.session_owner.lock_recover().watch_deliveries()?)
        } else {
            None
        };
        self.commands
            .try_send(Command::SendSequence {
                admission: admission.clone(),
                messages,
                delay,
                interval,
                release_on_failure,
                hold: hold.map(|(x, y)| HoldRecord {
                    guard: Arc::clone(&self.touch_guard),
                    x,
                    y,
                }),
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        if !wait {
            return Ok(admission);
        }
        self.wait_for_native_write(
            &admission,
            frame_count,
            receiver,
            delivery_events.expect("waiting caller has delivery observer"),
        )?;
        Ok(admission)
    }

    /// Settings writes retain their delivery identity so pushed evidence can be
    /// qualified against native report start, rather than RPC queue time.
    pub(crate) fn send_settings_for_plan(
        &self,
        messages: Vec<OutboundMessage>,
        interval: Duration,
        fence: &DispatchContext,
    ) -> Result<OperationTicket, String> {
        self.send_sequence_for(
            messages,
            Duration::ZERO,
            interval,
            None,
            None,
            Some(fence),
            true,
        )
    }

    pub(crate) fn operation_observation_is_fresh(
        &self,
        operation: &OperationTicket,
        observation: Option<&ObservationTicket>,
    ) -> bool {
        observation.is_some_and(|observation| {
            self.session_owner
                .lock_recover()
                .operation_observation_is_fresh(operation, observation)
        })
    }

    pub fn send_operation(&self, operation: DeviceOperation) -> Result<(), String> {
        self.send_operation_for(operation, None, true)
    }

    pub fn send_operation_for_plan(
        &self,
        operation: DeviceOperation,
        fence: &DispatchContext,
    ) -> Result<(), String> {
        self.send_operation_for(operation, Some(fence), true)
    }

    pub fn queue_operation_for_plan(
        &self,
        operation: DeviceOperation,
        fence: &DispatchContext,
    ) -> Result<(), String> {
        self.send_operation_for(operation, Some(fence), false)
    }

    fn send_operation_for(
        &self,
        operation: DeviceOperation,
        fence: Option<&DispatchContext>,
        wait: bool,
    ) -> Result<(), String> {
        if let DeviceOperation::ScreenMouseFrame {
            kind: commands::ScreenMouseType::Press,
            x,
            y,
            ..
        } = &operation
        {
            let (x, y) = (*x, *y);
            let mut messages = operation
                .clone()
                .try_encode()
                .map_err(|error| error.to_string())?;
            if messages.len() != 1 {
                return Err("Raw screen press must encode one frame".into());
            }
            let message = messages.remove(0);
            let mut guard = self.touch_guard.lock_recover();
            guard.admit(&operation, self.session_clock.elapsed().as_millis() as u64)?;
            return self.send_for_with_budget_and_admission(
                message.message_type,
                message.payload,
                fence,
                wait,
                Duration::from_secs(2),
                move || {
                    // Before successful queue admission, no bytes can have
                    // been sent: preserve the prior guard on any refusal.
                    // After admission, even an uncertain native outcome must
                    // retain the held-pointer protection. Release this mutex
                    // before waiting for native delivery.
                    guard.note_press(x, y, self.session_clock.elapsed().as_millis() as u64);
                    drop(guard);
                },
            );
        }
        let touch = self.admit_touch(&operation)?;
        let result = self.send_operation_unguarded(operation, fence, wait);
        self.settle_touch(touch, &result);
        result
    }

    /// Refuse a screen gesture while a pointer is still down, and note a raw
    /// press or release that is about to go out. Nothing is retried and nothing
    /// is sent here.
    fn admit_touch(&self, operation: &DeviceOperation) -> Result<TouchEffect, String> {
        let now_ms = self.session_clock.elapsed().as_millis() as u64;
        let guard = self.touch_guard.lock_recover();
        guard.admit(operation, now_ms)?;
        match operation {
            // A release lifts the pointer. So does a raw tap: it is a pointer-position
            // update plus a release in one frame (observed on CorOS 4.1.0,
            // 2026-10-06), which is how a long press on a Grid block is finished.
            DeviceOperation::ScreenMouseFrame {
                kind: commands::ScreenMouseType::Release | commands::ScreenMouseType::Tap,
                ..
            } => Ok(TouchEffect::Released),
            _ => Ok(TouchEffect::None),
        }
    }

    /// Finish what [`Self::admit_touch`] started: a release that was delivered
    /// clears the guard (a failed one leaves the pointer held).
    fn settle_touch(&self, effect: TouchEffect, result: &Result<(), String>) {
        match effect {
            TouchEffect::None => {}
            TouchEffect::Released => {
                if result.is_ok() {
                    self.touch_guard.lock_recover().note_release_sent();
                }
            }
        }
    }

    fn send_operation_unguarded(
        &self,
        operation: DeviceOperation,
        fence: Option<&DispatchContext>,
        wait: bool,
    ) -> Result<(), String> {
        if let DeviceOperation::RetargetCapture {
            source_revision, ..
        } = &operation
        {
            let source_revision = *source_revision;
            let mut messages = operation.try_encode().map_err(|error| error.to_string())?;
            if messages.len() != 1 {
                return Err("Capture retarget must encode one frame".into());
            }
            let (reply, receiver) = mpsc::channel();
            let admission = self.write_admission_for(Duration::from_secs(2), fence)?;
            let delivery_events = if wait {
                Some(self.session_owner.lock_recover().watch_deliveries()?)
            } else {
                None
            };
            self.commands
                .try_send(Command::RetargetCapture(
                    messages.remove(0),
                    source_revision,
                    admission.clone(),
                    reply,
                ))
                .map_err(|error| error.to_string())?;
            if !wait {
                return Ok(());
            }
            return self.wait_for_native_write(
                &admission,
                1,
                receiver,
                delivery_events.expect("waiting caller has delivery observer"),
            );
        }
        let interval_ms =
            quadcortex::device::request::operation_inter_message_interval_ms(&operation);
        let release_on_failure = operation.abort_release_frame();
        let hold = match &operation {
            DeviceOperation::ScreenHold { x, y } => Some((*x, *y)),
            _ => None,
        };
        let messages = operation.try_encode().map_err(|error| error.to_string())?;
        if interval_ms > 0 || messages.len() > 1 {
            return self
                .send_sequence_for(
                    messages,
                    Duration::ZERO,
                    Duration::from_millis(interval_ms),
                    release_on_failure,
                    hold,
                    fence,
                    wait,
                )
                .map(|_| ());
        }
        for message in messages {
            self.send_for(message.message_type, message.payload, fence, wait)?;
        }
        Ok(())
    }

    pub fn request(
        &self,
        message_type: u16,
        payload: Vec<u8>,
        expected_type: u16,
        request_id: Option<u64>,
        timeout: Duration,
    ) -> Result<IncomingMessage, String> {
        let started = Instant::now();
        let epoch = self.state.lock_recover().session_epoch;
        let fence = self.session_owner.lock_recover().session_fence()?;
        let admission = self.read_admission(epoch, &fence, started, timeout)?;
        let (sender, receiver) = mpsc::channel();
        queue_request(
            &self.commands,
            Command::Request {
                message_type,
                payload,
                expected_type,
                request_id,
                admission,
                reply: sender,
            },
        )?;
        receiver
            .recv_timeout(timeout + Duration::from_secs(1))
            .map_err(|error| error.to_string())?
    }

    /// Execute one projected read with one admission and completion budget.
    /// Projection decoding stays on the caller's operation thread. The worker
    /// submits an ordered message batch and returns its observation boundary.
    pub fn begin_gateway_read_scope(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<GatewayReadScope, String> {
        let started_ms = self.session_clock.elapsed().as_millis() as u64;
        // Validate arguments and obtain the method budget before reserving an ID.
        let plan = runtime_request::plan_gateway_read(method, params, 0)?;
        self.session_owner
            .lock_recover()
            .begin_gateway_read_scope(started_ms, plan.timeout_ms)
    }

    pub fn gateway_read_scope_remaining(&self, scope: &GatewayReadScope) -> Result<u64, String> {
        self.session_owner
            .lock_recover()
            .gateway_read_scope_remaining(scope, self.session_clock.elapsed().as_millis() as u64)
    }

    pub fn begin_gateway_read_scope_for(
        &self,
        context: &DispatchContext,
        timeout_ms: u64,
    ) -> Result<GatewayReadScope, String> {
        let started_ms = self.session_clock.elapsed().as_millis() as u64;
        self.session_owner
            .lock_recover()
            .begin_gateway_read_scope_for(context, started_ms, timeout_ms)
    }

    pub fn bind_gateway_preflight(
        &self,
        context: &DispatchContext,
        method: &str,
        params: &serde_json::Value,
        response: &serde_json::Value,
    ) -> Result<
        (
            DispatchContext,
            runtime_request::GatewayWritePreflightDecision,
        ),
        String,
    > {
        self.session_owner.lock_recover().bind_write_preflight(
            context,
            method,
            params,
            response,
            self.session_clock.elapsed().as_millis() as u64,
        )
    }

    /// Copy authoritative I/O settings only within the original read session.
    pub fn retained_io_settings_in_scope(
        &self,
        scope: &GatewayReadScope,
    ) -> Result<Option<Vec<u8>>, String> {
        let owner = self.session_owner.lock_recover();
        owner
            .gateway_read_scope_remaining(scope, self.session_clock.elapsed().as_millis() as u64)?;
        owner.state.assert_io_settings_healthy()?;
        Ok(owner
            .state
            .published_settings(profile::MESSAGE_TYPE_IO_SETTINGS)
            .map(<[u8]>::to_vec))
    }

    pub fn validate_io_settings_read(&self, scope: &GatewayReadScope) -> Result<(), String> {
        let owner = self.session_owner.lock_recover();
        owner
            .gateway_read_scope_remaining(scope, self.session_clock.elapsed().as_millis() as u64)?;
        owner.state.assert_io_settings_healthy()
    }

    /// Read a session publication without allowing a replacement cache to
    /// certify the original write. Decode outside the lock; revalidate below.
    pub fn published_gateway_read_in_scope(
        &self,
        scope: &GatewayReadScope,
        message_type: u16,
        projection: runtime_request::GatewayResponseProjection,
    ) -> Result<serde_json::Value, String> {
        self.published_gateway_read_in_scope_with(scope, message_type, |payload| {
            projection.decode(payload)
        })
    }

    pub(crate) fn published_gateway_read_in_scope_with(
        &self,
        scope: &GatewayReadScope,
        message_type: u16,
        decode: impl FnOnce(&[u8]) -> Result<serde_json::Value, String>,
    ) -> Result<serde_json::Value, String> {
        let message = self
            .latest_message(message_type)
            .ok_or("QC session has not published the requested settings")?;
        let value = decode(&message.payload)?;
        let owner = self.session_owner.lock_recover();
        owner
            .gateway_read_scope_remaining(scope, self.session_clock.elapsed().as_millis() as u64)?;
        if message_type == profile::MESSAGE_TYPE_IO_SETTINGS {
            owner.state.assert_io_settings_healthy()?;
        }
        if !message.observation.as_ref().is_some_and(|ticket| {
            owner.accepts_observation_for(scope.context().session_fence(), ticket)
        }) {
            return Err(
                "QC settings publication does not belong to the original write session".into(),
            );
        }
        Ok(value)
    }

    pub fn gateway_read(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let scope = self.begin_gateway_read_scope(method, params)?;
        self.gateway_read_in_scope(&scope, method, params)
    }

    pub fn gateway_read_in_scope(
        &self,
        scope: &GatewayReadScope,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let started = Instant::now();
        let epoch = self.state.lock_recover().session_epoch;
        let fence = scope.context().session_fence().clone();
        let remaining = self.gateway_read_scope_remaining(scope)?;
        let request_id = self.reserve_request_id_in_scope(scope)?;
        let mut plan = runtime_request::plan_gateway_read(method, params, request_id)?;
        plan.timeout_ms = plan.timeout_ms.min(remaining);
        self.gateway_read_scope_remaining(scope)?;
        let budget = Duration::from_millis(plan.timeout_ms);
        let timeout_error = || {
            format!(
                "The Quad Cortex did not return a valid {method} reply within {} seconds",
                budget.as_secs()
            )
        };
        let started_ms = started
            .saturating_duration_since(self.session_clock)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let runtime = GatewayReadRuntime::new(
            plan.response_type,
            plan.projection,
            started_ms,
            plan.timeout_ms,
        );
        let events = self.subscribe_raw_events();
        let (reply, submitted) = mpsc::channel();
        let admission = self.read_admission(epoch, &fence, started, budget)?;
        let remaining =
            admission.remaining(self.state.lock_recover().session_epoch, Instant::now())?;
        queue_request(
            &self.commands,
            Command::SubmitGatewayRead {
                messages: plan
                    .operation
                    .try_encode()
                    .map_err(|error| error.to_string())?,
                admission,
                reply,
            },
        )?;
        // This acknowledgement proves admission only. The shared read runtime
        // still requires a timely, correlated, valid device response.
        let after_sequence = submitted
            .recv_timeout(remaining)
            .map_err(|_| timeout_error())??;
        loop {
            if self.state.lock_recover().session_epoch != epoch {
                return Err("Device session changed while awaiting a QC read".into());
            }
            let now_ms = self
                .session_clock
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            let remaining_ms = runtime.remaining_ms(now_ms);
            if remaining_ms == 0 {
                return Err(timeout_error());
            }
            let message = events.recv_timeout(Duration::from_millis(remaining_ms.min(100)));
            let message = match message {
                Ok(message) if message.sequence > after_sequence => message,
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("QC read observation stream closed".into())
                }
            };
            if !message.observation.as_ref().is_some_and(|ticket| {
                self.session_owner
                    .lock_recover()
                    .accepts_observation_for(&fence, ticket)
            }) || !runtime.accepts_ingress(
                message
                    .observation
                    .as_ref()
                    .and_then(ObservationTicket::ingress),
                self.session_clock.elapsed().as_millis() as u64,
            ) {
                continue;
            }
            let observation =
                runtime.observe_with_clock(message.message_type, &message.payload, || {
                    self.session_clock
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64
                });
            // The unit states its CorOS version in its update request, not in
            // the reply a read decodes; the session holds what it stated.
            let observation = {
                let owner = self.session_owner.lock_recover();
                runtime.with_session_facts(observation, owner.observed_cor_os_version())
            };
            match observation {
                ReadObservation::Matched { value } => {
                    if self.state.lock_recover().session_epoch != epoch {
                        return Err("Device session changed while decoding a QC read".into());
                    }
                    let owner = self.session_owner.lock_recover();
                    if !message
                        .observation
                        .as_ref()
                        .is_some_and(|ticket| owner.accepts_observation_for(&fence, ticket))
                    {
                        return Err("QC observation session changed before read completion".into());
                    }
                    if runtime.remaining_ms(self.session_clock.elapsed().as_millis() as u64) == 0 {
                        return Err(timeout_error());
                    }
                    owner.gateway_read_scope_remaining(
                        scope,
                        self.session_clock.elapsed().as_millis() as u64,
                    )?;
                    return Ok(value);
                }
                ReadObservation::Expired => return Err(timeout_error()),
                ReadObservation::Unrelated | ReadObservation::Rejected { .. } => {}
            }
        }
    }

    /// Reserve a correlation id from the same retained session sequence used
    /// by staged startup and post-boot synchronization.
    pub fn reserve_request_id_in_scope(&self, scope: &GatewayReadScope) -> Result<u64, String> {
        let started = Instant::now();
        let remaining = self.gateway_read_scope_remaining(scope)?;
        let request_id =
            reserve_request_id_before(&self.commands, started + Duration::from_millis(remaining))?;
        self.gateway_read_scope_remaining(scope)?;
        Ok(request_id)
    }

    /// Capture the physical session before a host opens a save dialog. This is
    /// an expected-state fence, not authorization, and does not open USB.
    pub fn backup_context(&self) -> Result<DispatchContext, String> {
        self.session_owner.lock_recover().session_context()
    }

    pub fn create_backup(&self, timeout: Duration) -> Result<String, String> {
        self.create_backup_in_context(timeout, None)
    }

    pub fn create_backup_in_context(
        &self,
        timeout: Duration,
        expected: Option<&DispatchContext>,
    ) -> Result<String, String> {
        let started = Instant::now();
        let started_ms = started
            .saturating_duration_since(self.session_clock)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        // Preserve the existing worker-response allowance and recovery bound,
        // but start their combined budget before queueing and retain the session
        // that owns the export. A new ready session cannot certify this backup.
        let worker_budget = timeout.saturating_add(Duration::from_secs(20));
        let recovery_budget = Duration::from_millis(profile::READY_WAIT_TIMEOUT_MS);
        let (admission, scope) = {
            let mut owner = self.session_owner.lock_recover();
            // Planning and validation share the owner lock. The worker checks
            // this same ticket again immediately before its USB effect.
            let context = match expected {
                Some(context) => context.clone(),
                None => owner.session_context()?,
            };
            let scope = owner.begin_gateway_read_scope_for(
                &context,
                started_ms,
                worker_budget
                    .saturating_add(recovery_budget)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )?;
            let admission = owner.plan_operation_in_context(
                &context,
                started_ms,
                timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            )?;
            (admission, scope)
        };
        let (reply, response) = mpsc::channel();
        self.commands
            .try_send(Command::CreateBackup { admission, reply })
            .map_err(|error| error.to_string())?;
        let document = response
            .recv_timeout(worker_budget.saturating_sub(started.elapsed()))
            // By this point `Command::CreateBackup` has already been admitted
            // into the worker's queue and may already be writing to the
            // device, so a timeout here is the same "sent but unconfirmed"
            // shape `wait_for_native_write` returns - carrying the same
            // phrase keeps it recognized by rpc.rs's
            // error_has_ambiguous_write_outcome, which quarantines further
            // writes until a session reset. Without it this timeout looked
            // like an ordinary, safely-retryable failure.
            .map_err(|_| {
                "Native QC backup worker timed out; device effect may be uncertain".to_string()
            })??;
        let status = self.wait_for_ready_in_scope(&scope, recovery_budget)?;
        if status.phase != "ready" {
            return Err(format!(
                "The backup completed, but the Quad Cortex session did not recover: {}",
                status.detail
            ));
        }
        // The shared initialization runtime's ready state already requires
        // Master Volume and the complete authoritative control seed.
        Ok(document)
    }

    pub fn switch_scene(&self, scene: u32) -> Result<(), String> {
        if scene > 7 {
            return Err("Scene must be between 0 and 7".into());
        }
        self.send_operation(DeviceOperation::Command(DeviceCommand::SelectScene(scene)))
    }

    pub fn wait_for_scene(&self, scene: u32, timeout: Duration) -> BrokerStatus {
        let events = self.subscribe_state_events();
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.status();
            if status.active_scene == Some(scene)
                || status.phase == "error"
                || Instant::now() >= deadline
            {
                return status;
            }
            if events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_err()
            {
                return self.status();
            }
        }
    }

    fn wait_for_ready_in_scope(
        &self,
        scope: &GatewayReadScope,
        timeout: Duration,
    ) -> Result<BrokerStatus, String> {
        let events = self.subscribe_state_events();
        let started = Instant::now();
        loop {
            let status = self.status();
            let remaining = self.gateway_read_scope_remaining(scope)?;
            let wait = timeout
                .saturating_sub(started.elapsed())
                .min(Duration::from_millis(remaining));
            if status.phase == "ready" || status.phase == "error" || wait.is_zero() {
                return Ok(status);
            }
            if events.recv_timeout(wait).is_err() {
                // Re-sample through the same session/deadline check even when
                // the notification channel closes or a wait expires.
                let status = self.status();
                self.gateway_read_scope_remaining(scope)?;
                return Ok(status);
            }
        }
    }

    pub fn wait_for_ready(&self, timeout: Duration) -> BrokerStatus {
        let events = self.subscribe_state_events();
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.status();
            if status.phase == "ready" || status.phase == "error" || Instant::now() >= deadline {
                return status;
            }
            if events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_err()
            {
                return self.status();
            }
        }
    }
}

impl DeviceController {
    /// A handle that stops this controller's USB worker and waits for it.
    pub fn shutdown_handle(&self) -> WorkerShutdown {
        WorkerShutdown {
            commands: self.commands.clone(),
            connect_interrupt: self.connect_interrupt.clone(),
            worker: Arc::clone(&self.worker),
        }
    }

    /// Before the process exits: stop this controller's USB worker and wait,
    /// bounded by `budget`, for its connection teardown. Nothing is queued
    /// for a controller with no worker thread (tests), and a worker already
    /// stopped reports its recorded outcome without waiting again.
    pub fn stop_worker_for_exit(&self, budget: Duration) -> WorkerExit {
        stop_workers_within([self], budget).outcomes[0].1
    }

    /// Test-only: a controller whose USB worker is `worker`, on its own
    /// thread, exactly where `start_for_device_with_lease` keeps the real one.
    #[cfg(test)]
    fn with_worker_for_test(worker: impl FnOnce(mpsc::Receiver<Command>) + Send + 'static) -> Self {
        let (mut controller, receiver) = tests::refresh_controller();
        let retirement = WorkerRetirement::new(retired_worker::MAX_WORKER_LIFETIMES);
        let lifetime = retirement.reserve().unwrap();
        let worker_lifetime = Arc::clone(&lifetime);
        let handle = thread::Builder::new()
            .name("qc-native-usb-fake".into())
            .spawn(move || {
                let _lifetime = lifetime;
                worker(receiver)
            })
            .unwrap();
        controller.worker = Arc::new(WorkerThreadState::running(
            handle,
            retirement,
            worker_lifetime,
            Arc::new(AtomicBool::new(true)),
        ));
        controller
    }

    /// Test-only: a controller whose USB worker never reads `Stop` and
    /// returns only once `release` is set: a worker wedged inside the OS.
    #[cfg(test)]
    pub(crate) fn with_stuck_worker_for_test(release: Arc<AtomicBool>) -> Self {
        Self::with_worker_for_test(move |_receiver| {
            while !release.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(10));
            }
        })
    }

    /// Reap and inspect the exact worker original in retirement tests.
    #[cfg(test)]
    pub(crate) fn fixture_worker_acknowledged(&self) -> bool {
        self.shutdown_handle().fixture_original_acknowledged()
    }
}

impl Drop for DeviceController {
    /// Stop the worker and wait, bounded, for it to return, so the orderly
    /// disconnect `QcUsb::drop` writes is on the wire before a `--stdio`
    /// broker's `main` returns. Without the wait the detached worker was
    /// killed with the process and the QC kept a stale session.
    ///
    /// Deadlock safety: the worker never owns a controller (its context holds
    /// only the shared state `Arc`s), and `stop_and_join` refuses to wait on
    /// its own thread anyway. Drop takes no lock while it waits, and the
    /// worker needs no broker-level lock (`request_lock`, `clients`) to reach
    /// `Stop` and return. The join is bounded by [`WORKER_EXIT_BUDGET`] and the
    /// `Stop` admission by `CONTROL_COMMAND_ADMISSION`, so the worst case is a
    /// slow exit, never a hang. A worker that
    /// `finish_shutdown` already joined or gave up on is not waited on again,
    /// so Drop never adds a second budget to a shutdown.
    fn drop(&mut self) {
        let _ = self.shutdown_handle().stop_and_join(WORKER_EXIT_BUDGET);
    }
}

struct PendingRequest {
    epoch: u64,
    expectation: ResponseExpectation,
    reply: mpsc::Sender<Result<IncomingMessage, String>>,
}

/// The connected unit as the update relay's pump sees it.
struct ConnectedRelaySink<'a>(&'a mut ConnectedQc);

impl RelaySink for ConnectedRelaySink<'_> {
    fn send_forwarded(
        &mut self,
        frame: quadcortex_forwarder::ForwardedMessage,
    ) -> Result<(), String> {
        if !frame.current() {
            return Ok(());
        }
        let (message, deadline, cancel) = frame.into_parts();
        match self.0.usb.send_forwarded_command(message, deadline, cancel) {
            Ok(()) | Err(UsbError::ForwardingRetired) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
    fn send(&mut self, message: OutboundMessage) -> Result<(), String> {
        self.0
            .send_command(message)
            .map_err(|error| error.to_string())
    }
}

/// A LocalBackup document collected on the main device loop.
///
/// LocalBackup replies are an uncorrelated stream, so collection is a state
/// machine advanced by the ordinary read path rather than a nested loop. A
/// backup can legitimately take three minutes; running it inline used to stop
/// live state, keepalive and every other command for that whole time, and
/// forced pending requests to be failed before it started.
struct BackupInProgress {
    operation: OperationTicket,
    reply: mpsc::Sender<Result<String, String>>,
}

impl BackupInProgress {
    fn absorb(
        &self,
        owner: &mut SessionRuntime,
        observation: &ObservationTicket,
        now_ms: u64,
        payload: &[u8],
    ) -> Option<Result<String, String>> {
        match owner.consume_backup(&self.operation, observation, payload, now_ms) {
            Ok(Some(progress)) => progress.document.map(Ok),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

enum StateDecoderCommand {
    Reset,
    StateWake,
    MetadataWake,
    Catalog(ObservationTicket, Result<ModelCatalog, String>),
    Stop,
}

/// An independently cancellable decoder owner. It is armed before the USB
/// worker is spawned, so rejected startup cannot strand a sender/receiver cycle.
/// A stalled parser/lock is retained by exact JoinHandle until it acknowledges;
/// the caller's deadline does not claim it stopped or detach its ownership.
struct DecoderThreadOwner {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    lifetime: Option<Arc<WorkerLifetime>>,
}

impl DecoderThreadOwner {
    fn request_stop(&self, messages: &mpsc::SyncSender<StateDecoderCommand>) {
        self.stop.store(true, Ordering::Release);
        let _ = messages.try_send(StateDecoderCommand::Stop);
    }
}

#[cfg(test)]
fn start_owned_decoder(
    name: &str,
    run: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
) -> std::io::Result<DecoderThreadOwner> {
    let lifetime = WorkerRetirement::production()
        .reserve()
        .map_err(std::io::Error::other)?;
    start_owned_decoder_with_lifetime(name, lifetime, run)
}

fn start_owned_decoder_with_lifetime(
    name: &str,
    lifetime: Arc<WorkerLifetime>,
    run: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
) -> std::io::Result<DecoderThreadOwner> {
    let stop = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || run(signal))?;
    Ok(DecoderThreadOwner {
        stop,
        thread: Some(thread),
        lifetime: Some(lifetime),
    })
}

impl Drop for DecoderThreadOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + WORKER_EXIT_BUDGET;
        while !thread_signalled(&thread) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let lifetime = self
            .lifetime
            .take()
            .expect("original decoder owns its admission");
        if thread_signalled(&thread) {
            let _ = thread.join();
            drop(lifetime);
        } else {
            let retirement = Arc::clone(&lifetime.0);
            retirement.retain(thread, std::sync::Weak::new(), lifetime);
            eprintln!("QC device broker: retaining a decoder that did not acknowledge shutdown");
        }
    }
}

struct StateDecoderShared {
    stream_id: String,
    event_log: Arc<Mutex<StateEventJournal>>,
    latest_tempo_clock: Arc<Mutex<Option<DecodedStateFrame>>>,
    latest_master_volume: Arc<Mutex<Option<DecodedStateFrame>>>,
    latest_sequence: Arc<AtomicU64>,
    session_owner: Arc<Mutex<SessionRuntime>>,
    subscribers: Arc<Mutex<Vec<StateSubscriber>>>,
}

fn clear_revoked_state_publications(
    shared: &StateDecoderShared,
    owner: &SessionRuntime,
    published: &mut Option<ObservationTicket>,
) {
    if published
        .as_ref()
        .is_some_and(|ticket| owner.accepts_observation(ticket))
    {
        return;
    }
    shared.event_log.lock_recover().clear();
    *shared.latest_tempo_clock.lock_recover() = None;
    *shared.latest_master_volume.lock_recover() = None;
    *published = None;
}

fn queue_state_observation(
    sender: &mpsc::SyncSender<StateDecoderCommand>,
    owner: &Arc<Mutex<SessionRuntime>>,
    message: IncomingMessage,
) -> Result<(), String> {
    if message.message_type == profile::MESSAGE_TYPE_MODEL_REPO {
        let Some(ticket) = message.observation else {
            return Ok(());
        };
        if owner
            .lock_recover()
            .queue_model_repo(ticket, message.payload)
        {
            sender
                .send(StateDecoderCommand::MetadataWake)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    } else {
        let Some(ticket) = message.observation else {
            return Ok(());
        };
        // Reduce ordinary state before the USB worker can dispatch its next
        // command. Publishing may wait behind UI work; native admission must
        // already see the state carried by these received bytes.
        let (result, ready) = {
            let mut owner = owner.lock_recover();
            let result = owner.observe_state(&ticket, message.message_type, &message.payload);
            (result, owner.state_publications_ready())
        };
        if ready {
            sender
                .send(StateDecoderCommand::StateWake)
                .map_err(|error| error.to_string())?;
        }
        result.map(|_| ())
    }
}

#[cfg(test)]
fn run_state_decoder(
    shared: StateDecoderShared,
    sender: mpsc::SyncSender<StateDecoderCommand>,
    messages: mpsc::Receiver<StateDecoderCommand>,
    session_clock: Instant,
) {
    run_state_decoder_with_stop(
        shared,
        sender,
        messages,
        session_clock,
        Arc::new(AtomicBool::new(false)),
    );
}

#[cfg(test)]
fn run_state_decoder_with_stop(
    shared: StateDecoderShared,
    sender: mpsc::SyncSender<StateDecoderCommand>,
    messages: mpsc::Receiver<StateDecoderCommand>,
    session_clock: Instant,
    stop: Arc<AtomicBool>,
) {
    let lifetime = WorkerRetirement::production()
        .reserve()
        .expect("fixture decoder admission");
    run_state_decoder_with_lifetime(shared, sender, messages, session_clock, stop, lifetime);
}

fn zeroize_private_cloud_payload(message: &mut IncomingMessage) -> bool {
    if quadcortex_hid::latest_cache::is_private_cloud_message(message.message_type) {
        message.payload.zeroize();
        true
    } else {
        false
    }
}

fn retain_message_for_current_epoch(message: &mut IncomingMessage, current: bool) -> bool {
    if !current {
        zeroize_private_cloud_payload(message);
    }
    current
}

fn run_state_decoder_with_lifetime(
    shared: StateDecoderShared,
    sender: mpsc::SyncSender<StateDecoderCommand>,
    messages: mpsc::Receiver<StateDecoderCommand>,
    session_clock: Instant,
    stop: Arc<AtomicBool>,
    lifetime: Arc<WorkerLifetime>,
) {
    let (metadata_tx, metadata_rx) = mpsc::sync_channel::<()>(1);
    let metadata_results = sender.clone();
    let metadata_owner = Arc::clone(&shared.session_owner);
    let metadata_worker =
        start_owned_decoder_with_lifetime("qc-native-metadata", lifetime, move |stop| {
            while !stop.load(Ordering::Acquire) {
                match metadata_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                let work = metadata_owner.lock_recover().take_model_repo();
                let Some(work) = work else {
                    continue;
                };
                let catalog = parse_model_repo(&work.payload).map_err(|error| error.to_string());
                if metadata_results
                    .send(StateDecoderCommand::Catalog(work.ticket, catalog))
                    .is_err()
                {
                    return;
                }
            }
        })
        .ok();
    let mut next_sequence = 1_u64;
    let mut published = None;
    while !stop.load(Ordering::Acquire) {
        let command = match messages.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match command {
            StateDecoderCommand::Reset => {
                // The owner already revoked the old session. Do not clear its
                // queue here: a delayed host reset may follow newer reductions.
                let owner = shared.session_owner.lock_recover();
                clear_revoked_state_publications(&shared, &owner, &mut published);
                continue;
            }
            StateDecoderCommand::StateWake => {}
            StateDecoderCommand::MetadataWake => {
                if metadata_worker.is_some() {
                    let _ = metadata_tx.try_send(());
                } else {
                    // Even this fallback parses outside the owner lock. State
                    // received while parsing is staged before the installation.
                    let work = shared.session_owner.lock_recover().take_model_repo();
                    if let Some(work) = work {
                        let catalog =
                            parse_model_repo(&work.payload).map_err(|error| error.to_string());
                        let _ = shared.session_owner.lock_recover().complete_model_repo(
                            &work.ticket,
                            catalog,
                            session_clock.elapsed().as_millis() as u64,
                        );
                    }
                }
            }
            StateDecoderCommand::Catalog(ticket, catalog) => {
                let _ = shared.session_owner.lock_recover().complete_model_repo(
                    &ticket,
                    catalog,
                    session_clock.elapsed().as_millis() as u64,
                );
                // The owner releases the active parse even when it fails.
                let _ = metadata_tx.try_send(());
            }
            StateDecoderCommand::Stop => break,
        }
        let publications = shared
            .session_owner
            .lock_recover()
            .take_state_publications();
        let Ok(publications) = publications else {
            // Queue overflow fails the shared lifecycle; the USB worker's
            // existing lifecycle recovery closes and reconnects that session.
            continue;
        };
        for publication in publications {
            let owner = shared.session_owner.lock_recover();
            if !owner.accepts_observation(&publication.observation) {
                continue;
            }
            clear_revoked_state_publications(&shared, &owner, &mut published);
            published = Some(publication.observation);
            let frame = DecodedStateFrame {
                stream_id: shared.stream_id.clone(),
                sequence: next_sequence,
                observed_at: u128::from(publication.observed_at),
                states: publication.states,
                tempo_clock: publication.tempo_clock,
            };
            if frame.tempo_clock.is_some() {
                *shared.latest_tempo_clock.lock_recover() = Some(frame.clone());
            }
            if frame
                .states
                .iter()
                .any(|state| state.master_volume.is_some())
            {
                *shared.latest_master_volume.lock_recover() = Some(frame.clone());
            }
            // Keep the session check and native host publication on one side
            // of reset. This only enqueues bounded messages, never UI callbacks.
            publish_state_frame(
                &shared.event_log,
                &shared.latest_sequence,
                &shared.subscribers,
                frame,
            );
            next_sequence = next_sequence.saturating_add(1);
        }
    }
    // Retire both channel ends before waiting for the metadata owner. A parser
    // finishing into a full result queue must be woken with Disconnected.
    drop(messages);
    drop(sender);
    drop(metadata_tx);
    drop(metadata_worker);
}

fn publish_state_frame(
    event_log: &Arc<Mutex<StateEventJournal>>,
    latest_sequence: &Arc<AtomicU64>,
    subscribers: &Arc<Mutex<Vec<StateSubscriber>>>,
    frame: DecodedStateFrame,
) {
    let retained = {
        let mut log = event_log.lock_recover();
        let retained = log.push_sized(frame.clone());
        latest_sequence.store(frame.sequence, Ordering::Release);
        retained
    };
    let Some(encoded_bytes) = retained else {
        return;
    };
    // Never blocks: a slow subscriber loses this frame (logged and counted)
    // and catches up from the journal, which readers already do on a gap.
    subscribers.lock_recover().retain(|subscriber| {
        subscriber.sender.offer(frame.clone(), encoded_bytes) != OfferOutcome::Disconnected
    });
}

/// Every `run()` teardown branch that disconnects an existing session calls
/// this first: without it, a cached read (`device.generalSettings`,
/// `device.ioSettings`, ...) has no connected/freshness check of its own and
/// keeps answering with the last-known-good values from the session that
/// just failed.
fn clear_session_caches(
    state: &Arc<Mutex<BrokerStatus>>,
    latest_messages: &Arc<Mutex<HashMap<u16, IncomingMessage>>>,
) {
    state.lock_recover().device_name = None;
    latest_messages.lock_recover().clear();
}

/// The identical teardown both `observe_lifecycle` and `advance_lifecycle`
/// failure branches in `run()` used to spell out by hand: record the
/// incident, drop the phase, clear the caches, fail every pending request
/// and any active backup, disconnect the session, and abort the connection.
/// One fix to this ordering (the backup-stranding gap this session already
/// closed once, at a sibling call site) previously had to be applied at
/// each copy by hand.
#[allow(clippy::too_many_arguments)]
fn teardown_after_session_failure(
    error: impl std::fmt::Display,
    now_ms: u64,
    incident: &'static str,
    state: &Arc<Mutex<BrokerStatus>>,
    latest_messages: &Arc<Mutex<HashMap<u16, IncomingMessage>>>,
    pending_requests: &mut Vec<PendingRequest>,
    backup: &mut Option<BackupInProgress>,
    session: &Arc<Mutex<SessionRuntime>>,
    connection: &mut Option<ConnectedQc>,
) {
    let detail = format!("QC session failed: {error}");
    if let Some(connected) = connection.as_mut() {
        connected.usb.record_incident(incident);
    }
    set_phase(state, "searching", &detail, false, false);
    clear_session_caches(state, latest_messages);
    fail_pending(pending_requests, &detail);
    fail_backup(backup, &detail);
    {
        let mut owner = session.lock_recover();
        owner.disconnect(now_ms, true);
        // A lifecycle that failed or stalled earns the same retry backoff as a
        // connect that failed, so a unit that keeps doing it is not hammered.
        owner.transport.connect_failed(now_ms);
    }
    if let Some(mut failed) = connection.take() {
        failed.usb.abort();
    }
}

#[allow(clippy::too_many_arguments)]
fn ingest_incoming(
    session: &Arc<Mutex<SessionRuntime>>,
    state: &Arc<Mutex<BrokerStatus>>,
    connected: &mut ConnectedQc,
    latest_messages: &Arc<Mutex<HashMap<u16, IncomingMessage>>>,
    raw_events: &Arc<RawEventBus>,
    state_messages: &mpsc::SyncSender<StateDecoderCommand>,
    pending_requests: &mut Vec<PendingRequest>,
    mut message: IncomingMessage,
    now_ms: u64,
) {
    if zeroize_private_cloud_payload(&mut message) {
        return;
    }
    update_message(state, &mut connected.latest_messages, message.clone());
    if let Some(bytes) = cache_latest_message(&mut latest_messages.lock_recover(), message.clone())
    {
        raw_events.retention.record_latest_cache_rejection(bytes);
    }
    fan_out_raw_event(raw_events, &message);
    let _ = queue_state_observation(state_messages, session, message.clone());
    deliver_pending(
        pending_requests,
        &message,
        now_ms,
        state.lock_recover().session_epoch,
    );
}

#[cfg(test)]
mod forwarding_privacy_tests {
    use super::*;

    #[test]
    fn account_and_update_requests_are_not_retained_as_readable_state() {
        let mut latest = HashMap::new();
        for message_type in [18, 29, 30, 31, 41, 58, 61] {
            let message = IncomingMessage {
                observation: None,
                sequence: 0,
                message_type,
                payload: b"synthetic-private-cloud-payload".to_vec(),
                received_at_unix_ms: 0,
            };
            latest.insert(message_type, message.clone());
            assert_eq!(cache_latest_message(&mut latest, message), None);
            assert!(!latest.contains_key(&message_type));
        }
    }
}

const RETAINED_PAYLOAD_MAX_BYTES: usize =
    (quadcortex::domain::IPC_MAX_FRAME_BYTES - 64 * 1024) / 4 * 3;

/// "type 51 (13000000 bytes, sha256 0123abcd...)": what is recorded in place
/// of a payload too large to retain.
fn summarize_payload(message: &IncomingMessage) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(&message.payload);
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "type {} ({} bytes, sha256 {hex}...)",
        message.message_type,
        message.payload.len()
    )
}

/// Whether `incoming` may be cached as its type's latest message. An
/// oversized payload is summarised instead, and any older entry of that type
/// is dropped too: it is no longer the latest publication, so leaving it
/// would answer later reads with superseded state.
fn retain_latest_message_payload(
    latest: &mut HashMap<u16, IncomingMessage>,
    incoming: &IncomingMessage,
) -> bool {
    if incoming.payload.len() <= RETAINED_PAYLOAD_MAX_BYTES {
        return true;
    }
    latest.remove(&incoming.message_type);
    eprintln!(
        "Did not cache an oversized QC frame: {}",
        summarize_payload(incoming)
    );
    false
}

/// Keep settings as cumulative snapshots. CorOS publishes full type-3/type-9
/// frames during the handshake but answers writes with sparse settings
/// echo. Replacing the cache with that echo makes every unrelated setting
/// disappear until the next session. The shared reducer preserves absent parent
/// fields and replaces present value groups, including omitted proto3 false bits.
///
/// Returns the payload size of a message the cache refused under its per-entry
/// or total bounds (already logged), so the caller can count it.
fn cache_latest_message(
    latest: &mut HashMap<u16, IncomingMessage>,
    mut incoming: IncomingMessage,
) -> Option<usize> {
    if quadcortex_hid::latest_cache::is_private_cloud_message(incoming.message_type) {
        latest.remove(&incoming.message_type);
        incoming.payload.zeroize();
        return None;
    }
    if !retain_latest_message_payload(latest, &incoming) {
        return Some(incoming.payload.len());
    }
    // A tagged type-3 is not this host's settings: keep the last good one
    // (first or later, live or re-seeded). It is not a bounds rejection.
    if !quadcortex::responses::settings_publication_is_retainable(
        incoming.message_type,
        &incoming.payload,
    ) {
        return None;
    }
    if incoming.message_type == profile::MESSAGE_TYPE_IO_SETTINGS
        && matches!(
            quadcortex::responses::validate_io_settings_retention(&incoming.payload),
            Err(quadcortex::responses::ResponseDecodeError::RetainedIoBudget)
        )
    {
        latest.remove(&incoming.message_type);
        return Some(incoming.payload.len());
    }
    if matches!(
        incoming.message_type,
        profile::MESSAGE_TYPE_GENERAL_SETTINGS | profile::MESSAGE_TYPE_IO_SETTINGS
    ) {
        if let Some(previous) = latest.get(&incoming.message_type).filter(|previous| {
            previous
                .observation
                .as_ref()
                .zip(incoming.observation.as_ref())
                .is_some_and(|(old, new)| old.same_session_as(new))
        }) {
            let merge = if incoming.message_type == profile::MESSAGE_TYPE_GENERAL_SETTINGS {
                quadcortex::responses::merge_general_settings_publication
            } else {
                quadcortex::responses::merge_io_settings_publication
            };
            let merged = match merge(&previous.payload, &incoming.payload) {
                Ok(merged) => merged,
                Err(quadcortex::responses::ResponseDecodeError::RetainedIoBudget) => {
                    let bytes = incoming.payload.len();
                    latest.remove(&incoming.message_type);
                    return Some(bytes);
                }
                Err(_) => return None,
            };
            incoming.payload = merged;
        }
    }
    let (message_type, bytes) = (incoming.message_type, incoming.payload.len());
    match quadcortex_hid::insert_latest_message(latest, incoming) {
        Ok(()) => None,
        Err(rejection) => {
            eprintln!("Did not cache a {bytes}-byte QC frame of type {message_type}: {rejection}");
            Some(bytes)
        }
    }
}

struct WorkerContext {
    discovery: DeviceDiscovery,
    _decoder: DecoderThreadOwner,
    device_id: Arc<Mutex<Option<String>>>,
    identity_observation_enabled: Arc<AtomicBool>,
    session: Arc<Mutex<SessionRuntime>>,
    session_clock: Instant,
    state: Arc<Mutex<BrokerStatus>>,
    latest_messages: Arc<Mutex<HashMap<u16, IncomingMessage>>>,
    raw_events: Arc<RawEventBus>,
    state_messages: mpsc::SyncSender<StateDecoderCommand>,
    connect_interrupt: ConnectInterrupt,
    lease_path: std::path::PathBuf,
    updater_relay: Arc<UpdaterRelay>,
    teardown_verified: Arc<AtomicBool>,
}

fn run(context: WorkerContext, commands: mpsc::Receiver<Command>, initial_auto_connect: bool) {
    // Keep ownership outside the transport's entire scope. A ConnectedQc drop
    // may still be closing native HID handles when the caller's bounded join
    // expires; another opener must not receive the lease before that finishes.
    let lease = if context.lease_path == crate::device_lease::default_path()
        && std::env::var_os("QC_DEVICE_LEASE_PATH").is_none()
    {
        crate::device_lease::LeaseHolder::with_locations(
            crate::device_lease::Locations::from_environment(),
            crate::device_lease::own_role(),
            true,
        )
    } else {
        crate::device_lease::LeaseHolder::new(
            context.lease_path.clone(),
            crate::device_lease::own_role(),
        )
    };
    with_transport_lease(lease, |lease| {
        run_leased(context, commands, initial_auto_connect, lease)
    });
}

fn native_disconnect_result(verified: bool) -> Result<(), String> {
    if verified {
        Ok(())
    } else {
        Err("Device session closed; native teardown delivery/release is not verified; original native work remains owned until retirement".into())
    }
}

fn with_transport_lease<L>(mut lease: L, run: impl FnOnce(&mut L)) {
    run(&mut lease);
}

fn run_leased(
    context: WorkerContext,
    commands: mpsc::Receiver<Command>,
    initial_auto_connect: bool,
    lease: &mut crate::device_lease::LeaseHolder,
) {
    let WorkerContext {
        discovery,
        _decoder,
        device_id,
        identity_observation_enabled,
        session,
        session_clock,
        state,
        latest_messages,
        raw_events,
        state_messages,
        connect_interrupt,
        lease_path: _,
        updater_relay,
        teardown_verified,
    } = context;
    let mut connection: Option<ConnectedQc> = None;
    let mut identity_read =
        session_identity::SessionIdentityRead::new(Arc::clone(&identity_observation_enabled));
    let mut auto_connect = initial_auto_connect;
    let mut pending_requests: Vec<PendingRequest> = Vec::new();
    let mut backup: Option<BackupInProgress> = None;
    let mut relay_pump = RelayPump::new();
    let mut cloud_forward =
        CloudForwardRelay::with_fetcher(crate::updater_relay::http::direct_fetcher());
    let mut cloud_key_watch = CloudKeyWatch::default();
    let mut command_not_before = Instant::now();
    // The inbound-telemetry revision this loop has already published. A
    // connection epoch resets the session's tally to zero, which is a change
    // like any other, so no separate reset is needed here.
    let mut published_inbound_revision = 0_u64;
    let mut startup_watch = StartupWatch::default();
    loop {
        // Register before lease maintenance, command draining and HID
        // enumeration: a control request during any pre-open work must still
        // cancel this attempt, rather than become its new baseline.
        let interrupted = connect_interrupt.watch();
        // ProductForward credentials use the same user-visible internet-relay
        // switch as the plaintext firmware relay: it alone decides whether an
        // account request reaches the network. Set this before a new USB
        // connection enters its version handshake so the session key is
        // installed before any encrypted account request can arrive. With the
        // switch off, CloudLogin and ProductForward are still decrypted, in
        // memory and only far enough (ids, timeout, action) to answer or log
        // them, then zeroized: a request left unanswered makes the unit drop
        // its account link. The other account families stay undecrypted.
        let internet_relay_enabled = updater_relay.status().enabled;
        cloud_forward.set_enabled(internet_relay_enabled);
        {
            let mut owner = session.lock_recover();
            owner
                .transport
                .set_cloud_forward_enabled(internet_relay_enabled);
            owner.transport.set_cloud_account_answers_enabled(true);
        }
        if connection.is_some() {
            lease.tick();
        } else {
            lease.release();
        }
        // The post-handshake settle window holds commands without sleeping on
        // this thread. Sleeping here used to stall the read path for the whole
        // window, which is exactly when the QC pushes its initial state burst.
        let mut commands_remaining = DEVICE_COMMAND_BATCH_LIMIT;
        while let Some(command) =
            next_command_in_batch(&commands, &mut commands_remaining, command_not_before)
        {
            match command {
                Command::RecordFlight { event, incident } => {
                    if let Some(connected) = connection.as_mut() {
                        if incident {
                            connected.usb.record_incident(event);
                        } else {
                            connected.usb.record_event(event);
                        }
                    }
                }
                Command::ReserveRequestId { reply } => {
                    let result = connection
                        .as_mut()
                        .map(ConnectedQc::reserve_request_id)
                        .ok_or_else(|| "Quad Cortex is not connected".to_string());
                    let _ = reply.send(result);
                }
                Command::Reconnect { force, reply } => {
                    let phase = state.lock_recover().phase.clone();
                    if reconnect_outcome(force, connection.is_some(), &phase)
                        == ReconnectOutcome::AlreadyConnected
                    {
                        let _ = reply.send(ReconnectOutcome::AlreadyConnected);
                        continue;
                    }
                    lease.begin_recovery_admission();
                    // Before the connection goes: the reconnect below can be due
                    // at once, so the no-session branch may never get a pass.
                    updater_relay.cancel("the device session was restarted");
                    answer_cloud_requests_before_teardown(
                        &mut connection,
                        &mut cloud_forward,
                        &session,
                        &session_clock,
                        &state,
                        "device-session-restarted",
                    );
                    connection.take();
                    fail_pending(&mut pending_requests, "Device session restarted");
                    fail_backup(&mut backup, "Device session restarted");
                    clear_session_caches(&state, &latest_messages);
                    auto_connect = true;
                    session
                        .lock_recover()
                        .request_reconnect(session_clock.elapsed().as_millis() as u64);
                    let _ = state_messages.send(StateDecoderCommand::Reset);
                    set_phase(&state, "searching", "Reconnect requested", false, false);
                    let _ = reply.send(ReconnectOutcome::Restarted);
                }
                Command::Disconnect { reply, _admission } => {
                    updater_relay.cancel("the device session was closed");
                    answer_cloud_requests_before_teardown(
                        &mut connection,
                        &mut cloud_forward,
                        &session,
                        &session_clock,
                        &state,
                        "device-session-closed",
                    );
                    let verified = if let Some(mut connected) = connection.take() {
                        let request_id = connected.reserve_request_id();
                        connected.usb.disconnect_exact_checked(request_id)
                    } else {
                        teardown_verified.load(Ordering::Acquire)
                    };
                    fail_pending(&mut pending_requests, "Device session closed");
                    fail_backup(&mut backup, "Device session closed");
                    clear_session_caches(&state, &latest_messages);
                    auto_connect = false;
                    session
                        .lock_recover()
                        .disconnect(session_clock.elapsed().as_millis() as u64, false);
                    let _ = state_messages.send(StateDecoderCommand::Reset);
                    set_phase(
                        &state,
                        "disconnected",
                        if verified {
                            "Device session closed"
                        } else {
                            "Device session closed; native teardown delivery/release is not verified"
                        },
                        false,
                        false,
                    );
                    let _ = reply.send(native_disconnect_result(verified));
                }
                Command::Send(message_type, payload, admission, reply) => {
                    let result = if let Some(connected) = connection.as_mut() {
                        let result = connected
                            .send_command_checked(
                                OutboundMessage {
                                    message_type,
                                    payload,
                                },
                                None,
                                Some((&admission, &session_clock)),
                            )
                            .map_err(|error| error.to_string());
                        if result.is_ok() {
                            update_usb_telemetry(&state, connected);
                        }
                        result
                    } else {
                        Err("Quad Cortex is not connected".into())
                    };
                    let _ = reply.send(result);
                }
                Command::RetargetCapture(message, revision, admission, reply) => {
                    let result = connection
                        .as_mut()
                        .ok_or_else(|| "Quad Cortex is not connected".to_string())
                        .and_then(|connected| {
                            connected
                                .send_command_checked(
                                    message,
                                    Some(revision),
                                    Some((&admission, &session_clock)),
                                )
                                .map_err(|error| error.to_string())
                        });
                    if result.is_ok() {
                        if let Some(connected) = connection.as_ref() {
                            update_usb_telemetry(&state, connected);
                        }
                    }
                    let _ = reply.send(result);
                }
                Command::SendRealtime(message_type, payload, admission) => {
                    if let Some(connected) = connection.as_mut() {
                        if let Err(error) = connected.send_command_checked(
                            OutboundMessage {
                                message_type,
                                payload,
                            },
                            None,
                            Some((&admission, &session_clock)),
                        ) {
                            if matches!(error, UsbError::Admission(_)) {
                                connected.usb.record_event("realtime-admission-rejected");
                                continue;
                            }
                            let detail = format!("QC realtime frame encoding failed: {error}");
                            connected.usb.record_incident("realtime-write-failure");
                            set_phase(&state, "searching", &detail, false, false);
                            clear_session_caches(&state, &latest_messages);
                            session
                                .lock_recover()
                                .disconnect(session_clock.elapsed().as_millis() as u64, true);
                            if let Some(mut failed) = connection.take() {
                                failed.usb.abort();
                            }
                            continue;
                        }
                        update_usb_telemetry(&state, connected);
                    }
                }
                Command::SendSequence {
                    admission,
                    messages,
                    delay,
                    interval,
                    release_on_failure,
                    hold,
                    reply,
                } => {
                    let result = if let Some(connected) = connection.as_mut() {
                        thread::sleep(delay);
                        send_paced_sequence(
                            &messages,
                            release_on_failure.as_ref(),
                            interval,
                            |message, release| {
                                // The release frame is the cleanup of a gesture
                                // this same loop already started on this same
                                // connection, so it is not fenced by the
                                // admission that the failure may have exhausted.
                                let sent = connected
                                    .send_command_checked(
                                        message.clone(),
                                        None,
                                        (!release).then_some((&admission, &session_clock)),
                                    )
                                    .map_err(|error| error.to_string());
                                if sent.is_ok() {
                                    update_usb_telemetry(&state, connected);
                                }
                                sent
                            },
                            thread::sleep,
                        )
                    } else {
                        Err("Quad Cortex is not connected".into())
                    };
                    // A hold whose touch-up could not be delivered, even after the
                    // one second attempt, leaves the pointer down on the unit: record
                    // it so the next gesture is refused with the reason instead of
                    // the failure being silent. Recorded here, by the worker that
                    // sent the frames, so it does not depend on the caller waiting.
                    note_hold_outcome(
                        hold.as_ref(),
                        &result,
                        session_clock.elapsed().as_millis() as u64,
                    );
                    let _ = reply.send(result);
                }
                Command::Request {
                    message_type,
                    payload,
                    expected_type,
                    request_id,
                    admission,
                    reply,
                } => {
                    let timeout = match admission
                        .remaining(state.lock_recover().session_epoch, Instant::now())
                    {
                        Ok(remaining) => remaining,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    if let Some(connected) = connection.as_mut() {
                        pending_requests.push(PendingRequest {
                            epoch: admission.epoch,
                            expectation: ResponseExpectation::new(
                                expected_type,
                                request_id,
                                session_clock.elapsed().as_millis() as u64,
                                timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                            ),
                            reply,
                        });
                        if let Err(error) = connected.send_command_checked(
                            OutboundMessage {
                                message_type,
                                payload,
                            },
                            None,
                            Some((&admission.operation, &session_clock)),
                        ) {
                            if let Some(pending) = pending_requests.pop() {
                                let _ = pending.reply.send(Err(error.to_string()));
                            }
                            continue;
                        }
                        update_usb_telemetry(&state, connected);
                    } else {
                        let _ = reply.send(Err("Quad Cortex is not connected".into()));
                    }
                }
                Command::SubmitGatewayRead {
                    messages,
                    admission,
                    reply,
                } => {
                    let result = (|| {
                        let connected = connection
                            .as_mut()
                            .ok_or_else(|| "Quad Cortex is not connected".to_string())?;
                        let after_sequence = connected.usb.latest_inbound_sequence();
                        let current_epoch = state.lock_recover().session_epoch;
                        submit_read_batch(
                            messages,
                            &admission,
                            current_epoch,
                            Instant::now,
                            |message| {
                                connected
                                    .send_command_checked(
                                        message,
                                        None,
                                        Some((&admission.operation, &session_clock)),
                                    )
                                    .map_err(|error| error.to_string())?;
                                Ok(())
                            },
                        )?;
                        update_usb_telemetry(&state, connected);
                        Ok(after_sequence)
                    })();
                    let _ = reply.send(result);
                }
                Command::CreateBackup {
                    mut admission,
                    reply,
                } => {
                    if backup.is_some() {
                        let _ = reply.send(Err("A device backup is already in progress".into()));
                    } else if let Some(connected) = connection.as_mut() {
                        // Pending requests are deliberately not failed: the read
                        // path keeps serving them while the document streams.
                        if let Err(error) = session.lock_recover().begin_backup(
                            &mut admission,
                            session_clock.elapsed().as_millis() as u64,
                        ) {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                        if let Err(error) = connected.send_command_checked(
                            commands::create_local_backup(),
                            None,
                            Some((&admission, &session_clock)),
                        ) {
                            session.lock_recover().cancel_backup(&admission);
                            let _ = reply.send(Err(error.to_string()));
                            continue;
                        }
                        update_usb_telemetry(&state, connected);
                        // Backup traffic owns an unconditional keepalive clock,
                        // so restart the ordinary idle deadline here.
                        session
                            .lock_recover()
                            .transport
                            .defer_keepalive(session_clock.elapsed().as_millis() as u64);
                        backup = Some(BackupInProgress {
                            operation: admission,
                            reply,
                        });
                        set_phase(
                            &state,
                            "syncing",
                            "Creating device backup",
                            true,
                            session.lock_recover().transport.synchronized(),
                        );
                    } else {
                        let _ = reply.send(Err("Quad Cortex is not connected".into()));
                    }
                }
                Command::Stop => {
                    fail_pending(&mut pending_requests, "Native broker stopped");
                    fail_backup(&mut backup, "Native broker stopped");
                    updater_relay.cancel("the broker is stopping");
                    answer_cloud_requests_before_teardown(
                        &mut connection,
                        &mut cloud_forward,
                        &session,
                        &session_clock,
                        &state,
                        "broker-stopping",
                    );
                    // A full publication queue must not park the USB worker
                    // before its decoder owner can signal cancellation.
                    _decoder.request_stop(&state_messages);
                    return;
                }
            }
        }

        let now_ms = session_clock.elapsed().as_millis() as u64;
        if connection.is_none()
            && auto_connect
            && session.lock_recover().transport.reconnect_due(now_ms)
        {
            if interrupted() {
                continue;
            }
            session.lock_recover().transport.reconnect_attempted(now_ms);
            // Another live process owns the unit: do not open it. Opening a
            // device a hardware run holds made both openers fight for ~45 s on
            // the real unit, so wait on the normal retry cadence instead and
            // take over by itself once the owner exits or its record is stale.
            if let Err(holder) = lease.acquire() {
                session.lock_recover().disconnect(now_ms, true);
                set_phase(
                    &state,
                    "searching",
                    &crate::device_lease::waiting_detail(&holder),
                    false,
                    false,
                );
                continue;
            }
            // Enumerate before publishing `connecting`. A machine with no QC
            // used to flash an opening phase every retry even though HID had
            // already proved there was nothing to open. Hosts may now treat
            // `connecting` as evidence that a matching USB device exists.
            let devices = discovery.devices().unwrap_or_default();
            if interrupted() {
                continue;
            }
            let selected = {
                let mut selected = device_id.lock_recover();
                let detected = devices
                    .into_iter()
                    .find(|device| selected.as_ref().is_none_or(|id| id == &device.id))
                    .map(|device| device.id);
                if selected.is_none() {
                    *selected = detected.clone();
                }
                detected
            };
            let Some(selected) = selected else {
                session
                    .lock_recover()
                    .disconnect(session_clock.elapsed().as_millis() as u64, true);
                set_phase(&state, "searching", &lease.busy_detail(), false, false);
                continue;
            };
            begin_connection_epoch(&state, "Quad Cortex found; opening its USB session");
            let started = Instant::now();
            let _ = state_messages.send(StateDecoderCommand::Reset);
            // Once chosen, a missing unit never falls through to another one.
            let prior_failures = session.lock_recover().transport.connect_failures();
            let io_owner = match lease.io_owner() {
                Ok(owner) => owner,
                Err(holder) => {
                    set_phase(
                        &state,
                        "searching",
                        &crate::device_lease::waiting_detail(&holder),
                        false,
                        false,
                    );
                    continue;
                }
            };
            if interrupted() {
                continue;
            }
            // The outer startup watchdog cannot run while connect is inside
            // staged initialization. Bound that entire call too, including
            // repeated startup rebuilds, using its existing cancellation lane.
            let attempt_budget = ConnectAttemptBudget::new(started, STARTUP_PROGRESS_STALL_LIMIT);
            let cancelled = || {
                if interrupted() {
                    return true;
                }
                let now = Instant::now();
                // This watchdog must never wait for the lock it is watching.
                let phase = match session.try_lock() {
                    Ok(owner) => owner.lifecycle.as_ref().map(|lifecycle| lifecycle.phase()),
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned
                        .into_inner()
                        .lifecycle
                        .as_ref()
                        .map(|lifecycle| lifecycle.phase()),
                    Err(std::sync::TryLockError::WouldBlock) => None,
                };
                if let Some(phase) = phase {
                    attempt_budget.observe_phase(phase, now);
                }
                attempt_budget.expired(now)
            };
            let opened = QcUsb::connect_device_cancellable_with_io_owner_and_teardown(
                Arc::clone(&session),
                &session_clock,
                Some(&selected),
                |attempt, telemetry| {
                    let mut status = state.lock_recover();
                    status.phase = "handshaking".into();
                    status.detail = handshake_detail(attempt, prior_failures);
                    status.connected = false;
                    status.synchronized = false;
                    status.messages_sent = telemetry.messages_sent;
                    status.write_delivery = telemetry.write_delivery;
                    status.expected_write_stalls = telemetry.expected_write_stalls;
                    status.last_hid_write_duration_ms = telemetry.last_hid_write_duration_ms;
                    status.max_hid_write_duration_ms = telemetry.max_hid_write_duration_ms;
                    status.last_hid_write_completed = telemetry.last_hid_write_completed;
                    status.reader_lock_stall_events = telemetry.reader_lock_stall_events;
                    status.max_reader_lock_stall_ms = telemetry.max_reader_lock_stall_ms;
                    status.messages_sent_by_type = telemetry.messages_sent_by_type;
                },
                |message| {
                    let _ = queue_state_observation(&state_messages, &session, message);
                    refresh_startup_detail(&state, &session);
                },
                &cancelled,
                Some(io_owner),
                Some(Arc::clone(&teardown_verified)),
            );
            // A watchdog cancellation is a failed attempt, not a user request:
            // route it through the normal teardown/backoff/error reporting.
            let opened = attempt_budget.classify(opened, Instant::now(), interrupted());
            match opened {
                Ok(connected) => {
                    session.lock_recover().transport.connect_succeeded();
                    let handshake_ms = started.elapsed().as_millis();
                    install_connection_status(
                        &state,
                        &connected,
                        session.lock_recover().transport.synchronized(),
                        handshake_ms,
                    );
                    {
                        // The handshake filled this map outside update_message,
                        // so apply the same per-entry budget to the copy.
                        let mut latest = HashMap::new();
                        for message in connected.latest_messages.values() {
                            if let Some(bytes) = cache_latest_message(&mut latest, message.clone())
                            {
                                raw_events.retention.record_latest_cache_rejection(bytes);
                            }
                        }
                        *latest_messages.lock_recover() = latest;
                    }
                    // The QC reports its initial preset before its control loop
                    // is always ready to accept the first host mutation. Hold
                    // only that first post-handshake write briefly; subsequent
                    // commands remain on the zero-debounce realtime lane.
                    command_not_before = Instant::now()
                        + Duration::from_millis(
                            quadcortex::profile::POST_INITIALIZATION_WRITE_DELAY_MS,
                        );
                    identity_read = session_identity::SessionIdentityRead::new(Arc::clone(
                        &identity_observation_enabled,
                    ));
                    // Whatever the relay still holds belongs to a session that
                    // ended; none of it may be written into this one.
                    updater_relay.cancel("a new device session started");
                    cloud_forward.abandon_all("new-device-session");
                    cloud_key_watch.new_session();
                    // Those lines describe the session that ended, not this one.
                    let _ = cloud_forward.take_events();
                    connection = Some(connected);
                }
                Err(UsbError::NotAvailable) => {
                    session
                        .lock_recover()
                        .disconnect(session_clock.elapsed().as_millis() as u64, true);
                    set_phase(&state, "searching", &lease.busy_detail(), false, false);
                }
                Err(UsbError::Cancelled) => {
                    // A reset, disconnect or shutdown asked for this attempt to
                    // stop. The command that did so is next in the queue; this
                    // is not a device fault and earns no backoff.
                    session
                        .lock_recover()
                        .disconnect(session_clock.elapsed().as_millis() as u64, true);
                    set_phase(
                        &state,
                        "searching",
                        "The connection attempt was interrupted by a session request",
                        false,
                        false,
                    );
                }
                Err(error) => {
                    let now_ms = session_clock.elapsed().as_millis() as u64;
                    let failures = {
                        let mut owner = session.lock_recover();
                        owner.disconnect(now_ms, true);
                        owner.transport.connect_failed(now_ms);
                        owner.transport.connect_failures()
                    };
                    set_phase(
                        &state,
                        "error",
                        &failed_connect_detail(&error.to_string(), failures),
                        false,
                        false,
                    );
                }
            }
        }

        // A session key that is missing or wrong leaves every account request
        // unanswerable. Two such frames used to end the link, and the
        // reconnect brought a fresh key; now they are only dropped, so ask for
        // that fresh handshake explicitly: only on repeated evidence, on a
        // settled key state, never over an update or a backup, at most once
        // per cooldown.
        if let Some(connected) = connection.as_mut() {
            let now = Instant::now();
            let answer_key = session
                .lock_recover()
                .transport
                .cloud_answer_key(session_clock.elapsed().as_millis() as u64);
            cloud_key_watch.observe(answer_key, now);
            let unusable = connected
                .usb
                .take_unusable_account_frames()
                .saturating_add(u64::from(cloud_forward.take_undecodable()));
            if unusable > 0 {
                cloud_key_watch.note_unusable(unusable, now);
                connected
                    .usb
                    .record_event(format!("cloud-key-unusable: {unusable} account frame(s)"));
            }
            // A CorOS download pauses between its requests; a restart in such
            // a gap would still abort the update.
            let busy = updater_relay.update_in_progress(now) || backup.is_some();
            match cloud_key_watch.decide(now, busy) {
                KeyRenewal::Wait => {}
                KeyRenewal::Deferred => connected.usb.record_incident(
                    "cloud-key-renewal-deferred: an update or backup is in progress",
                ),
                KeyRenewal::Renew => {
                    connected
                        .usb
                        .record_incident("cloud-key-renewal: restarting the session");
                    restart_session(
                        "Quad Cortex account frames could not be decrypted; renewing the session",
                        &mut connection,
                        &mut pending_requests,
                        &mut backup,
                        &state,
                        &latest_messages,
                        &session,
                        &session_clock,
                        &state_messages,
                        &updater_relay,
                        &mut cloud_forward,
                    );
                    auto_connect = true;
                    continue;
                }
            }
        }

        if let Some(connected) = connection.as_mut() {
            let session_epoch = state.lock_recover().session_epoch;
            // An answer is encrypted only under a finished key: never before
            // this session's version exchange completes, never between its
            // fragments (writing one then would also fail and abort the link).
            let answer_key = session
                .lock_recover()
                .transport
                .cloud_answer_key(session_clock.elapsed().as_millis() as u64);
            let replies = cloud_forward.take_response_when(session_epoch, answer_key);
            for event in cloud_forward.take_events() {
                connected.usb.record_event(event);
            }
            if let Some(mut replies) = replies {
                let mut write_failed = false;
                for reply in std::mem::take(&mut replies.messages) {
                    match connected.usb.send_forwarded_command(
                        reply,
                        replies.deadline,
                        Arc::clone(&replies.cancel),
                    ) {
                        Ok(()) => {}
                        Err(UsbError::ForwardingRetired) => {
                            connected
                                .usb
                                .record_event("cloud-reply-retired: deadline or cancellation");
                            continue;
                        }
                        Err(_) => {
                            connected.usb.record_incident("cloud-forward-write-failure");
                            connected.usb.abort();
                            write_failed = true;
                            break;
                        }
                    }
                    update_usb_telemetry(&state, connected);
                }
                if write_failed {
                    continue;
                }
            }
            // Preserve staged startup and its settle window. This latch is reset
            // only after a successful transport open, never by a status poll.
            if Instant::now() >= command_not_before
                && session.lock_recover().transport.synchronized()
            {
                let epoch = state.lock_recover().session_epoch;
                let _ = identity_read.start_once(
                    &session,
                    epoch,
                    session_clock.elapsed().as_millis() as u64,
                    connected.usb.latest_inbound_sequence(),
                    |message, operation| {
                        let result = connected
                            .send_command_checked(message, None, Some((operation, &session_clock)))
                            .map_err(|error| error.to_string());
                        update_usb_telemetry(&state, connected);
                        result
                    },
                );
            }
            let read = connected.usb.read_message(CONNECTED_IO_POLL_MS);
            // One inbound read made: the update relay writes its next reply
            // frame only after enough of these while inbound is waiting, so a
            // long transfer cannot leave the unit's own traffic unread; a read
            // that found the queue empty ends that debt (see
            // `pump::READS_PER_FRAME`).
            relay_pump.note_read(connected.usb.last_read_was_empty());
            // Before the match, because several of its arms tear the
            // connection down and `continue`, and the frame that could not be
            // acted on is often the reason they do.
            if let Some(reason) =
                refresh_inbound_telemetry(&state, &session, &mut published_inbound_revision)
            {
                connected
                    .usb
                    .record_event(format!("inbound-unacted: {reason}"));
            }
            match read {
                Ok(Some(mut message)) => {
                    let now_ms = session_clock.elapsed().as_millis() as u64;
                    let was_synchronized = session.lock_recover().transport.synchronized();
                    match connected.observe_lifecycle(&message, now_ms) {
                        Ok(_) => {}
                        Err(error) => {
                            teardown_after_session_failure(
                                error,
                                now_ms,
                                "session-lifecycle-failure",
                                &state,
                                &latest_messages,
                                &mut pending_requests,
                                &mut backup,
                                &session,
                                &mut connection,
                            );
                            continue;
                        }
                    }
                    update_lifecycle_status(
                        &state,
                        was_synchronized,
                        session.lock_recover().transport.synchronized(),
                    );
                    let current = message
                        .observation
                        .as_ref()
                        .is_some_and(|ticket| session.lock_recover().accepts_observation(ticket));
                    if !current && message.message_type == profile::MESSAGE_TYPE_PRODUCT_FORWARD {
                        // Not forwarded into a session it does not belong to,
                        // but the unit still waits for an answer to it.
                        let origin = request_origin(&message, &state, &session, &session_clock);
                        cloud_forward.refuse_product_forward(
                            &mut message.payload,
                            "stale-epoch",
                            origin,
                        );
                    }
                    if !retain_message_for_current_epoch(&mut message, current) {
                        clear_session_caches(&state, &latest_messages);
                        connected.latest_messages.clear();
                        let _ = state_messages.send(StateDecoderCommand::Reset);
                        fail_pending(&mut pending_requests, "QC lifecycle epoch changed");
                        fail_backup(&mut backup, "QC lifecycle epoch changed");
                        // The device announced a rebuild of its session (the only
                        // way the epoch moves inside a live connection, see
                        // `SessionDriver::observe`) or the session failed: the
                        // unit has dropped what it asked for and, as the capture
                        // shows, asks again with a fresh id after its own timeout.
                        // Stop streaming to it, as the pending requests and the
                        // backup above are stopped.
                        updater_relay.cancel("the QC lifecycle epoch changed");
                        // The link stays up, so what is retired is answered.
                        cloud_forward.refuse_outstanding("epoch-changed");
                        continue;
                    }

                    // These encrypted cloud frames contain account credentials
                    // or credential-lifecycle state. They never enter the
                    // general cache, raw-event stream, or state decoder.
                    if message.message_type == profile::MESSAGE_TYPE_PRODUCT_FORWARD {
                        let origin = request_origin(&message, &state, &session, &session_clock);
                        cloud_forward.on_product_forward(&mut message.payload, origin);
                        continue;
                    }
                    if message.message_type == profile::MESSAGE_TYPE_CLOUD_LOGIN {
                        cloud_forward.on_cloud_login(&mut message.payload);
                        continue;
                    }
                    if matches!(message.message_type, 30 | 31 | 41 | 58) {
                        // These credential-bearing operations have no broker
                        // handler and must not enter general state publication.
                        message.payload.zeroize();
                        connected.usb.record_event(format!(
                            "cloud-frame-dropped: type {} has no handler",
                            message.message_type
                        ));
                        continue;
                    }

                    identity_read.observe(&session, &state, &message, now_ms);
                    // Signed updater URLs and headers stay within the relay.
                    if message.message_type == profile::MESSAGE_TYPE_UPDATER_FORWARD {
                        updater_relay.on_forward_frame(&message.payload, Instant::now());
                        message.payload.zeroize();
                        continue;
                    } else if message.message_type == profile::MESSAGE_TYPE_UPDATER {
                        updater_relay.on_updater_state(&message.payload);
                    }
                    if message.message_type == quadcortex::profile::MESSAGE_TYPE_LOCAL_BACKUP
                        && backup.is_some()
                    {
                        let outcome = backup.as_mut().and_then(|active| {
                            active.absorb(
                                &mut session.lock_recover(),
                                message.observation.as_ref().expect("validated observation"),
                                session_clock.elapsed().as_millis() as u64,
                                &message.payload,
                            )
                        });
                        if let Some(outcome) = outcome {
                            finish_backup(
                                &mut backup,
                                &state,
                                session.lock_recover().transport.synchronized(),
                                outcome,
                            );
                        }
                    } else {
                        ingest_incoming(
                            &session,
                            &state,
                            connected,
                            &latest_messages,
                            &raw_events,
                            &state_messages,
                            &mut pending_requests,
                            message,
                            session_clock.elapsed().as_millis() as u64,
                        );
                    }
                    // A suspension that ended in a reset leaves every cached
                    // device state suspect: rebuild the whole session, exactly
                    // as a session reset does.
                    let resync = session.lock_recover().take_suspension_resync();
                    if let Some(reason) = resync {
                        connected
                            .usb
                            .record_event(format!("suspension-resync {reason}"));
                        restart_session(
                            &format!(
                                "Quad Cortex resumed after {reason}; resynchronizing the session"
                            ),
                            &mut connection,
                            &mut pending_requests,
                            &mut backup,
                            &state,
                            &latest_messages,
                            &session,
                            &session_clock,
                            &state_messages,
                            &updater_relay,
                            &mut cloud_forward,
                        );
                        auto_connect = true;
                        continue;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    connected.usb.record_incident("usb-link-lost");
                    set_phase(
                        &state,
                        "searching",
                        &format!("USB link lost: {error}"),
                        false,
                        false,
                    );
                    fail_pending(&mut pending_requests, &format!("USB link lost: {error}"));
                    fail_backup(&mut backup, &format!("USB link lost: {error}"));
                    clear_session_caches(&state, &latest_messages);
                    session
                        .lock_recover()
                        .disconnect(session_clock.elapsed().as_millis() as u64, true);
                    if let Some(mut failed) = connection.take() {
                        failed.usb.abort();
                    }
                    continue;
                }
            }
            let backup_action = backup.as_ref().map(|active| {
                session
                    .lock_recover()
                    .advance_backup(
                        &active.operation,
                        session_clock.elapsed().as_millis() as u64,
                    )
                    .map(|(action, _)| action)
                    .unwrap_or_else(BackupAction::Failed)
            });
            match backup_action {
                Some(BackupAction::Keepalive) => {
                    if connected
                        .send_command_checked(
                            commands::keepalive(),
                            None,
                            Some((
                                &backup.as_ref().expect("active backup").operation,
                                &session_clock,
                            )),
                        )
                        .is_err()
                    {
                        teardown_after_session_failure(
                            "Backup keepalive write failed",
                            session_clock.elapsed().as_millis() as u64,
                            "backup-keepalive-write-failure",
                            &state,
                            &latest_messages,
                            &mut pending_requests,
                            &mut backup,
                            &session,
                            &mut connection,
                        );
                        continue;
                    }
                    update_usb_telemetry(&state, connected);
                    session
                        .lock_recover()
                        .transport
                        .defer_keepalive(session_clock.elapsed().as_millis() as u64);
                }
                Some(BackupAction::Failed(error)) => {
                    finish_backup(
                        &mut backup,
                        &state,
                        session.lock_recover().transport.synchronized(),
                        Err(error),
                    );
                }
                Some(BackupAction::Wait) | None => {}
            }
            // The update relay's replies and its unconditional keepalive clock.
            // The clock runs only while a request is active; a failed write
            // ends the session as a failed backup keepalive does.
            let relay_report = relay_pump.tick(
                &updater_relay,
                Instant::now(),
                &mut ConnectedRelaySink(&mut *connected),
            );
            for event in updater_relay.take_events() {
                connected.usb.record_event(event);
            }
            if relay_report.keepalives > 0 || relay_report.frames > 0 {
                update_usb_telemetry(&state, connected);
            }
            if relay_report.keepalives > 0 {
                session
                    .lock_recover()
                    .transport
                    .defer_keepalive(session_clock.elapsed().as_millis() as u64);
            }
            if relay_report.write_error.is_some() {
                updater_relay.cancel("a write to the unit failed");
                cloud_forward.abandon_all("write-failure");
                for event in cloud_forward.take_events() {
                    connected.usb.record_event(event);
                }
                teardown_after_session_failure(
                    "Update relay write failed",
                    session_clock.elapsed().as_millis() as u64,
                    "updater-relay-write-failure",
                    &state,
                    &latest_messages,
                    &mut pending_requests,
                    &mut backup,
                    &session,
                    &mut connection,
                );
                continue;
            }
            let now_ms = session_clock.elapsed().as_millis() as u64;
            let was_synchronized = session.lock_recover().transport.synchronized();
            match connected.advance_lifecycle(now_ms) {
                Ok(_) => {}
                Err(error) => {
                    teardown_after_session_failure(
                        error,
                        now_ms,
                        "session-lifecycle-failure",
                        &state,
                        &latest_messages,
                        &mut pending_requests,
                        &mut backup,
                        &session,
                        &mut connection,
                    );
                    continue;
                }
            }
            // A session that stays out of `Connected` this long is not
            // starting, whatever the device is sending: leave and redo the
            // whole handshake. The manual equivalent - closing the app and
            // opening it again - is what healed the wedge.
            let startup_connected = session
                .lock_recover()
                .lifecycle
                .as_ref()
                .is_some_and(|lifecycle| lifecycle.is_connected());
            if startup_watch.stalled(startup_connected, Instant::now(), STARTUP_STALL_LIMIT) {
                teardown_after_session_failure(
                    format!(
                        "startup did not reach Connected within {} s; restarting the session",
                        STARTUP_STALL_LIMIT.as_secs()
                    ),
                    now_ms,
                    "session-startup-stalled",
                    &state,
                    &latest_messages,
                    &mut pending_requests,
                    &mut backup,
                    &session,
                    &mut connection,
                );
                continue;
            }
            update_lifecycle_status(
                &state,
                was_synchronized,
                session.lock_recover().transport.synchronized(),
            );
            // Device loss is detected by read errors, as in the reference
            // client: a write carries no information because every QC write
            // stalls its status stage. The old Version-probe teardown is gone
            // with the probe itself - the session is now held open by the
            // dedicated KeepAlive below rather than proven by a correlated read.
            // A running backup owns the keepalive on its own unconditional clock,
            // and so does a running update relay, so the idle probe stands down
            // for the whole transfer.
            if backup.is_none() && !relay_pump.is_running() {
                let keepalive = session.lock_recover().transport.take_keepalive(now_ms);
                if let Some(keepalive) = keepalive {
                    // The QC needs its dedicated KeepAlive on the fixed cadence
                    // `profile::KEEPALIVE_INTERVAL_MS` declares. Do not restate
                    // the interval in prose here: this comment said "every
                    // five seconds" while the constant was 1000, and a doc
                    // line carrying the same error was already corrected
                    // once. A Version READ is answered, so the link looks
                    // alive, but it does not hold the session open.
                    if connected.usb.send_command(keepalive).is_err() {
                        teardown_after_session_failure(
                            "Keepalive write failed",
                            now_ms,
                            "keepalive-write-failure",
                            &state,
                            &latest_messages,
                            &mut pending_requests,
                            &mut backup,
                            &session,
                            &mut connection,
                        );
                        continue;
                    }
                    update_usb_telemetry(&state, connected);
                }
            }
        } else {
            // Whatever the relay was running belonged to a session that is gone.
            updater_relay.cancel_if_active("no device session");
            cloud_forward.abandon_all("no-device-session");
            // No recorder is open; none of this may land in the next session's.
            let _ = cloud_forward.take_events();
            thread::sleep(Duration::from_millis(100));
        }
        expire_pending(
            &mut pending_requests,
            session_clock.elapsed().as_millis() as u64,
            state.lock_recover().session_epoch,
        );
    }
}

/// The one answer both exits of [`DeviceController::wait_for_gateway_snapshot`]
/// give. Keeping it in one place is what stops a timeout path from returning
/// the success path's value: a deadline is never itself an answer.
pub(crate) fn validate_verified_projection_version(
    current: &GatewaySnapshot,
    result: &serde_json::Value,
) -> Result<(), String> {
    let expected = result
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("stateVersion"))
        .filter(|version| version.is_object())
        .ok_or_else(|| {
            "The verified write has no native revision for its detail projection".to_owned()
        })?;
    let actual = current
        .state_version
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    if actual.as_ref() != Some(expected) {
        return Err(
            "The verified write belongs to an earlier native revision; refresh device details"
                .into(),
        );
    }
    Ok(())
}

fn matching_snapshot(
    snapshot: Option<GatewaySnapshot>,
    predicate: &impl Fn(&GatewaySnapshot) -> bool,
) -> Option<GatewaySnapshot> {
    snapshot.filter(|snapshot| predicate(snapshot))
}

fn update_lifecycle_status(
    state: &Arc<Mutex<BrokerStatus>>,
    was_synchronized: bool,
    synchronized: bool,
) {
    if synchronized == was_synchronized {
        return;
    }
    // An in-session rebuild is the one catalog invalidation that does not
    // reopen the transport, so nothing else here advances the refresh key.
    // Retire it on both edges: the rebuild starts by discarding the seed and
    // ends by republishing the whole directory.
    advance_session_epoch(state);
    set_phase(
        state,
        if synchronized { "ready" } else { "syncing" },
        if synchronized {
            "Active preset synchronized"
        } else {
            "QC requested an in-session state rebuild"
        },
        true,
        synchronized,
    );
    if synchronized {
        let mut status = state.lock_recover();
        status.writes_quarantined = false;
        status.write_quarantine_reason = None;
        status.write_quarantine = None;
    }
}

/// Tear the current session down and ask for a fresh one, as the forced
/// `Command::Reconnect` arm does, after a suspension that ended in a reset.
/// Dropping the connection sends the device its orderly disconnect.
#[allow(clippy::too_many_arguments)]
fn restart_session(
    detail: &str,
    connection: &mut Option<ConnectedQc>,
    pending_requests: &mut Vec<PendingRequest>,
    backup: &mut Option<BackupInProgress>,
    state: &Arc<Mutex<BrokerStatus>>,
    latest_messages: &Arc<Mutex<HashMap<u16, IncomingMessage>>>,
    session: &Arc<Mutex<SessionRuntime>>,
    session_clock: &Instant,
    state_messages: &mpsc::SyncSender<StateDecoderCommand>,
    updater_relay: &UpdaterRelay,
    cloud_forward: &mut CloudForwardRelay,
) {
    // The reconnect is due at once, so the loop's no-session branch may never
    // run: end the relay's request here, before the old session is dropped.
    updater_relay.cancel("the device session was restarted");
    answer_cloud_requests_before_teardown(
        connection,
        cloud_forward,
        session,
        session_clock,
        state,
        "device-session-restarted",
    );
    connection.take();
    fail_pending(pending_requests, "Device session restarted");
    fail_backup(backup, "Device session restarted");
    clear_session_caches(state, latest_messages);
    session
        .lock_recover()
        .request_reconnect(session_clock.elapsed().as_millis() as u64);
    let _ = state_messages.send(StateDecoderCommand::Reset);
    set_phase(state, "searching", detail, false, false);
}

/// The fewest minutes after a first session restart asked for because the
/// account-frame key was missing or wrong. One restart normally restores
/// the key; a unit that keeps failing must not be reconnected in a loop, so
/// each further restart doubles the wait: 10, 20, then 40 minutes, capped.
const CLOUD_KEY_RENEW_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// The doublings `CLOUD_KEY_RENEW_COOLDOWN` stops at (40 minutes).
const CLOUD_KEY_RENEW_MAX_DOUBLINGS: u32 = 2;
/// Unusable-frame evidence kept at most, also while a renewal is deferred.
const CLOUD_KEY_EVIDENCE_MAX: usize = 8;
/// Unusable account frames needed within `CLOUD_KEY_RENEW_WINDOW` before a
/// renewal: one can be a frame caught across a key change.
const CLOUD_KEY_RENEW_FRAMES: usize = 2;
const CLOUD_KEY_RENEW_WINDOW: Duration = Duration::from_secs(60);
/// How long the answer-key state must hold before its frames count: a comms
/// reset clears and rebuilds the key within this.
const CLOUD_KEY_SETTLED_FOR: Duration = Duration::from_secs(5);

/// What `CloudKeyWatch::decide` asks of the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyRenewal {
    Wait,
    /// Renewal is due but would interrupt an update or a backup; reported
    /// once, and the evidence is kept until it can run.
    Deferred,
    Renew,
}

/// Decides when unusable account frames justify a fresh handshake.
#[derive(Debug, Default)]
struct CloudKeyWatch {
    unusable: std::collections::VecDeque<Instant>,
    key_state: Option<(CloudAnswerKey, Instant)>,
    last_renewal: Option<Instant>,
    renewals: u32,
    deferred: bool,
}

impl CloudKeyWatch {
    /// Track the answer-key state; a change restarts its settling time.
    fn observe(&mut self, key: CloudAnswerKey, now: Instant) {
        if self.key_state.is_none_or(|(state, _)| state != key) {
            self.key_state = Some((key, now));
        }
    }

    /// The key is either usable or missing, and has been for a while. Frames
    /// dropped while it is still being built are expected and never count.
    fn settled(&self, now: Instant) -> bool {
        self.key_state.is_some_and(|(state, since)| {
            state != CloudAnswerKey::NotReady
                && now.saturating_duration_since(since) >= CLOUD_KEY_SETTLED_FOR
        })
    }

    fn note_unusable(&mut self, frames: u64, now: Instant) {
        if !self.settled(now) {
            return;
        }
        for _ in 0..frames.min(CLOUD_KEY_EVIDENCE_MAX as u64) {
            if self.unusable.len() >= CLOUD_KEY_EVIDENCE_MAX {
                self.unusable.pop_front();
            }
            self.unusable.push_back(now);
        }
    }

    fn decide(&mut self, now: Instant, busy: bool) -> KeyRenewal {
        if !self.deferred {
            while self
                .unusable
                .front()
                .is_some_and(|at| now.saturating_duration_since(*at) > CLOUD_KEY_RENEW_WINDOW)
            {
                self.unusable.pop_front();
            }
        }
        let cooling = self
            .last_renewal
            .is_some_and(|at| now.saturating_duration_since(at) < self.cooldown());
        if self.unusable.len() < CLOUD_KEY_RENEW_FRAMES || cooling || !self.settled(now) {
            return KeyRenewal::Wait;
        }
        if busy {
            let first = !self.deferred;
            self.deferred = true;
            return if first {
                KeyRenewal::Deferred
            } else {
                KeyRenewal::Wait
            };
        }
        self.unusable.clear();
        self.deferred = false;
        self.last_renewal = Some(now);
        self.renewals = self.renewals.saturating_add(1);
        KeyRenewal::Renew
    }

    /// The wait after the latest renewal: 10, 20, then 40 minutes.
    fn cooldown(&self) -> Duration {
        let doublings = self
            .renewals
            .saturating_sub(1)
            .min(CLOUD_KEY_RENEW_MAX_DOUBLINGS);
        CLOUD_KEY_RENEW_COOLDOWN * (1_u32 << doublings)
    }

    /// A new USB session: its key starts over; the cooldown does not.
    fn new_session(&mut self) {
        self.unusable.clear();
        self.key_state = None;
        self.deferred = false;
    }
}

/// When the unit's clock for a frame started: at its first HID report, in
/// the session clock both the HID reader and this worker use; for a
/// multi-report frame that is earlier than its assembly.
fn frame_received_at(
    ingress: Option<IngressTime>,
    session_clock: &Instant,
    now: Instant,
) -> Instant {
    ingress
        .map(|time| *session_clock + Duration::from_millis(time.first_report_at_ms))
        .filter(|at| *at <= now)
        .unwrap_or(now)
}

fn request_origin(
    message: &IncomingMessage,
    state: &Arc<Mutex<BrokerStatus>>,
    session: &Arc<Mutex<SessionRuntime>>,
    session_clock: &Instant,
) -> RequestOrigin {
    RequestOrigin {
        epoch: state.lock_recover().session_epoch,
        key_generation: session.lock_recover().transport.cloud_key_generation(),
        received_at: frame_received_at(
            message
                .observation
                .as_ref()
                .and_then(|ticket| ticket.ingress()),
            session_clock,
            Instant::now(),
        ),
    }
}

/// The longest a teardown waits for TX admission of its Type 29 answers.
const TEARDOWN_CLOUD_ANSWER_BUDGET: Duration = Duration::from_millis(500);

/// Where teardown answers are written: the USB session's forwarding lane.
type CloudAnswerSink<'a> =
    dyn FnMut(OutboundMessage, Option<Instant>, Arc<AtomicBool>) -> Result<(), String> + 'a;

/// Before this broker ends a USB session itself (stop, disconnect, restart),
/// answer every Type 29 the unit is still waiting for: otherwise the request
/// times out on the unit and it drops its account link about 30 s later. The
/// answers are queued ahead of the teardown on the same ordered TX lane, and
/// the orderly close writes them. Any that cannot be written (no usable key,
/// no connection, admission refused) is named in the flight record.
fn answer_cloud_requests_before_teardown(
    connection: &mut Option<ConnectedQc>,
    cloud_forward: &mut CloudForwardRelay,
    session: &Arc<Mutex<SessionRuntime>>,
    session_clock: &Instant,
    state: &Arc<Mutex<BrokerStatus>>,
    reason: &str,
) {
    let Some(connected) = connection.as_mut() else {
        cloud_forward.abandon_all(reason);
        // No recorder is open; none of this may land in the next session's.
        let _ = cloud_forward.take_events();
        return;
    };
    let epoch = state.lock_recover().session_epoch;
    let answer_key = session
        .lock_recover()
        .transport
        .cloud_answer_key(session_clock.elapsed().as_millis() as u64);
    let usb = &mut connected.usb;
    let lines = flush_cloud_answers_for_teardown(
        cloud_forward,
        epoch,
        answer_key,
        reason,
        &mut |message, deadline, cancel| {
            usb.send_forwarded_command(message, deadline, cancel)
                .map_err(|error| error.to_string())
        },
    );
    for line in lines {
        connected.usb.record_event(line);
    }
}

/// Write every answer owed before a live-link teardown, fetched ones first,
/// then retire the rest without cancelling what was written: the answers
/// are in the TX lane, and the orderly close flushes them. The flight-record
/// lines to keep are returned.
fn flush_cloud_answers_for_teardown(
    cloud_forward: &mut CloudForwardRelay,
    epoch: u64,
    answer_key: CloudAnswerKey,
    reason: &str,
    send: &mut CloudAnswerSink<'_>,
) -> Vec<String> {
    // Answers already fetched go first; only exchanges still running are
    // refused.
    cloud_forward.refuse_outstanding(reason);
    // A teardown must not wait out a unit deadline for TX admission.
    let budget = Instant::now() + TEARDOWN_CLOUD_ANSWER_BUDGET;
    let mut failures = Vec::new();
    while let Some(mut replies) = cloud_forward.take_response_when(epoch, answer_key) {
        let deadline = Some(replies.deadline.map_or(budget, |end| end.min(budget)));
        for reply in std::mem::take(&mut replies.messages) {
            if let Err(error) = send(reply, deadline, Arc::clone(&replies.cancel)) {
                failures.push(format!("cloud-reply-not-queued: {error}"));
            }
        }
    }
    cloud_forward.retire_for_teardown(reason);
    let mut lines = cloud_forward.take_events();
    lines.extend(failures);
    lines
}

/// Deliver a backup's terminal outcome and restore the session phase.
fn finish_backup(
    backup: &mut Option<BackupInProgress>,
    state: &Arc<Mutex<BrokerStatus>>,
    synchronized: bool,
    outcome: Result<String, String>,
) {
    let Some(active) = backup.take() else {
        return;
    };
    let succeeded = outcome.is_ok();
    let _ = active.reply.send(outcome);
    set_phase(
        state,
        if synchronized { "ready" } else { "syncing" },
        if succeeded {
            "Device backup complete"
        } else {
            "Device backup failed; USB session remains open"
        },
        true,
        synchronized,
    );
}

/// Abandon an in-flight backup because the session it belonged to is gone.
fn fail_backup(backup: &mut Option<BackupInProgress>, detail: &str) {
    if let Some(active) = backup.take() {
        let _ = active.reply.send(Err(detail.to_string()));
    }
}

/// What a reconnect request did, which is not always what it was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectOutcome {
    /// The session was torn down: every pending request failed, the decoded
    /// device state was dropped behind a new generation fence, and a fresh
    /// handshake was requested.
    Restarted,
    /// The transport was already connected or on its way, so nothing was
    /// touched. `reset_device_session` is the path that forces a teardown.
    AlreadyConnected,
}

/// An unforced reconnect deliberately leaves a live or forming session alone:
/// a client's routine reconnect must not tear down a healthy one, and the
/// teardown costs more than a cache -- it fails every in-flight request and
/// resets the unit's own screen. Callers are told which happened rather than
/// being left to assume a reconnect occurred; see [`ReconnectOutcome`].
fn reconnect_outcome(force: bool, connected: bool, phase: &str) -> ReconnectOutcome {
    if !force && (connected || matches!(phase, "connecting" | "handshaking" | "syncing" | "ready"))
    {
        ReconnectOutcome::AlreadyConnected
    } else {
        ReconnectOutcome::Restarted
    }
}

fn deliver_pending(
    pending: &mut Vec<PendingRequest>,
    message: &IncomingMessage,
    now_ms: u64,
    epoch: u64,
) {
    // An expired waiter must not steal a matching reply from a still-live one.
    // Timer scheduling never extends a response budget.
    expire_pending(pending, now_ms, epoch);
    if let Some(index) = pending.iter().position(|request| {
        request.expectation.observe_received(
            message.message_type,
            &message.payload,
            message
                .observation
                .as_ref()
                .and_then(ObservationTicket::ingress),
            now_ms,
        ) == ResponseDecision::Matched
    }) {
        let request = pending.remove(index);
        let _ = request.reply.send(Ok(message.clone()));
    }
}

fn expire_pending(pending: &mut Vec<PendingRequest>, now_ms: u64, epoch: u64) {
    let mut index = 0;
    while index < pending.len() {
        if pending[index].epoch != epoch || pending[index].expectation.expired(now_ms) {
            let request = pending.remove(index);
            let error = if request.epoch != epoch {
                "Device session changed while awaiting a QC read".into()
            } else {
                request.expectation.timeout_message()
            };
            let _ = request.reply.send(Err(error));
        } else {
            index += 1;
        }
    }
}

fn fail_pending(pending: &mut Vec<PendingRequest>, detail: &str) {
    for request in pending.drain(..) {
        let _ = request.reply.send(Err(detail.into()));
    }
}

/// Retire the preset catalog's coalescing key. Every caller is a point after
/// which a cached listing may no longer describe the device.
fn advance_session_epoch(state: &Arc<Mutex<BrokerStatus>>) {
    let mut status = state.lock_recover();
    status.session_epoch = status.session_epoch.wrapping_add(1);
}

/// The handshake line of `phase: "handshaking"`. Says what the attempt is
/// waiting for and, once connects have started failing, how many already did,
/// so a session that keeps retrying cannot read like a first attempt.
fn handshake_detail(attempt: u32, prior_failures: u32) -> String {
    if prior_failures == 0 {
        format!("Waiting for Quad Cortex ResetCommsBuffers reply (attempt {attempt})")
    } else {
        format!(
            "Waiting for Quad Cortex ResetCommsBuffers reply (attempt {attempt}; \
             {prior_failures} earlier connect {} failed)",
            if prior_failures == 1 {
                "attempt"
            } else {
                "attempts"
            }
        )
    }
}

/// The error line after a connect failed, with the retry it earned.
fn failed_connect_detail(error: &str, failures: u32) -> String {
    let seconds = quadcortex::device::transport::reconnect_backoff_ms(failures).div_ceil(1000);
    format!(
        "{error}. Connect attempt {failures} failed; retrying in {seconds} s. \
         Resetting the session retries immediately."
    )
}

/// While the connect is past the reset gate the status used to keep saying
/// "Waiting for ResetCommsBuffers reply (attempt 1)" for the whole staged
/// startup, which is where a unit that keeps rebuilding spends its time. Say
/// which stage it is in and how many rebuilds the device has forced.
fn refresh_startup_detail(state: &Arc<Mutex<BrokerStatus>>, session: &Arc<Mutex<SessionRuntime>>) {
    let progress = {
        let owner = session.lock_recover();
        owner
            .lifecycle
            .as_ref()
            .filter(|lifecycle| lifecycle.session_validated())
            .map(|lifecycle| (lifecycle.phase(), lifecycle.startup_rebuilds()))
    };
    let Some((phase, rebuilds)) = progress else {
        return;
    };
    let detail = startup_progress_detail(phase.as_str(), rebuilds);
    let mut status = state.lock_recover();
    if status.phase == "handshaking" && status.detail != detail {
        status.detail = detail;
    }
}

fn startup_progress_detail(phase: &str, rebuilds: u32) -> String {
    if rebuilds == 0 {
        format!("Starting the Quad Cortex session ({phase})")
    } else {
        format!(
            "Starting the Quad Cortex session ({phase}; the device has restarted startup \
             {rebuilds} of {} allowed times)",
            quadcortex::device::initialization::MAX_STARTUP_REBUILDS
        )
    }
}

/// How long an open session may stay short of `Connected` before the broker
/// abandons it and handshakes again. Staged startup is bounded at
/// `READY_WAIT_TIMEOUT_MS` per rebuild; this is the ceiling over all of them.
/// Host policy (not a Cortex Control frame); the 60 s figure is BELIEVED, no
/// measurement in the repository.
const STARTUP_STALL_LIMIT: Duration = Duration::from_secs(60);

// Host policy, not protocol timers. A live successful startup measured 220,561
// ms on 2026-10-14 under machine contention, so 60 s over the entire attempt
// incorrectly resets progressing sessions. Allow time per forward phase while
// retaining an absolute ceiling against rebuild loops. Neither value claims a
// measured firmware limit.
const STARTUP_PROGRESS_STALL_LIMIT: Duration = Duration::from_secs(120);
const STARTUP_ATTEMPT_LIMIT: Duration = Duration::from_secs(600);

/// Watch one open/handshake/initialization attempt. Only a new highest startup
/// phase renews the progress clock: traffic, retry attempts and repeated rebuild
/// phases do not. A ready session's backup or update never uses this watchdog.
struct ConnectAttemptBudget {
    deadline: Instant,
    last_progress: std::cell::Cell<Instant>,
    highest_phase: std::cell::Cell<u8>,
    stall_limit: Duration,
}

impl ConnectAttemptBudget {
    fn new(started: Instant, stall_limit: Duration) -> Self {
        Self {
            deadline: started + STARTUP_ATTEMPT_LIMIT,
            last_progress: std::cell::Cell::new(started),
            highest_phase: std::cell::Cell::new(0),
            stall_limit,
        }
    }

    fn observe_phase(
        &self,
        phase: quadcortex::device::initialization::DeviceStartupPhase,
        now: Instant,
    ) {
        use quadcortex::device::initialization::DeviceStartupPhase;
        let rank = match phase {
            DeviceStartupPhase::SessionValidating => 0,
            DeviceStartupPhase::VersionValidating => 1,
            DeviceStartupPhase::Disconnected => 2,
            DeviceStartupPhase::Building => 3,
            DeviceStartupPhase::Initializing => 4,
            DeviceStartupPhase::Booting => 5,
            DeviceStartupPhase::Connected => 6,
            DeviceStartupPhase::Invalid | DeviceStartupPhase::Failed => 0,
        };
        if rank > self.highest_phase.get() {
            self.highest_phase.set(rank);
            self.last_progress.set(now);
        }
    }

    fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
            || now.saturating_duration_since(self.last_progress.get()) >= self.stall_limit
    }

    fn classify<T>(
        &self,
        result: Result<T, UsbError>,
        now: Instant,
        interrupted: bool,
    ) -> Result<T, UsbError> {
        match result {
            Err(UsbError::Cancelled) if !interrupted && self.expired(now) => {
                let reason = if now >= self.deadline {
                    format!(
                        "connection startup exceeded {} s",
                        STARTUP_ATTEMPT_LIMIT.as_secs()
                    )
                } else {
                    format!(
                        "no forward startup progress for {} s",
                        self.stall_limit.as_secs()
                    )
                };
                Err(UsbError::Initialization(format!(
                    "{reason}; automatically reopening the USB session"
                )))
            }
            other => other,
        }
    }
}

/// Measures how long a connection has gone without reaching `Connected`.
#[derive(Default)]
struct StartupWatch {
    since: Option<Instant>,
}

impl StartupWatch {
    /// True once the connection has been out of `Connected` for `limit`.
    /// Resets itself when `Connected` is observed or after it fires.
    fn stalled(&mut self, connected: bool, now: Instant, limit: Duration) -> bool {
        if connected {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) >= limit {
            self.since = None;
            return true;
        }
        false
    }
}

fn set_phase(
    state: &Arc<Mutex<BrokerStatus>>,
    phase: &str,
    detail: &str,
    connected: bool,
    synchronized: bool,
) {
    let mut status = state.lock_recover();
    status.phase = phase.into();
    status.detail = detail.into();
    status.connected = connected;
    status.synchronized = synchronized;
    if !connected {
        status.active_preset_name = None;
        status.active_preset_location = None;
        status.device_name = None;
        status.active_scene = None;
        status.connected_at_unix_ms = None;
    }
}

/// Publish the session's inbound telemetry into the status, and report the
/// newest reason when it changed so the caller can put it in the flight
/// recorder -- the one inbound record that outlives a connection, since the
/// counters reset with every other receive total.
///
/// The revision is a single integer read under the session lock; the tally is
/// copied only when something was actually counted, so a link carrying only
/// frames the host understands never reaches the allocation path.
fn refresh_inbound_telemetry(
    state: &Arc<Mutex<BrokerStatus>>,
    session: &Arc<Mutex<SessionRuntime>>,
    published_revision: &mut u64,
) -> Option<String> {
    let snapshot = {
        let owner = session.lock_recover();
        let revision = owner.inbound_telemetry_revision();
        if revision == *published_revision {
            return None;
        }
        *published_revision = revision;
        owner.inbound_telemetry()
    };
    let reason = snapshot.last_unacted.clone();
    let mut status = state.lock_recover();
    let unchanged = status.inbound_telemetry.last_unacted == reason;
    status.inbound_telemetry = snapshot;
    if unchanged {
        return None;
    }
    reason
}

/// Start a new transport telemetry epoch before opening HID. Status consumers
/// must never combine a prior connection's receive totals or successful
/// handshake duration with the current handshaking/error phase.
fn begin_connection_epoch(state: &Arc<Mutex<BrokerStatus>>, detail: &str) {
    let mut status = state.lock_recover();
    status.phase = "connecting".into();
    status.detail = detail.into();
    status.connected = false;
    status.synchronized = false;
    status.active_preset_name = None;
    status.active_preset_location = None;
    status.device_name = None;
    status.active_scene = None;
    status.connected_at_unix_ms = None;
    status.handshake_ms = None;
    status.messages_received = 0;
    status.messages_sent = 0;
    status.write_delivery = Default::default();
    status.device_suspended = false;
    status.messages_received_by_type.clear();
    status.messages_sent_by_type.clear();
    status.inbound_telemetry = InboundTelemetrySnapshot::default();
    status.expected_write_stalls = 0;
    status.last_hid_write_duration_ms = 0;
    status.max_hid_write_duration_ms = 0;
    status.last_hid_write_completed = false;
    status.reader_lock_stall_events = 0;
    status.max_reader_lock_stall_ms = 0;
    status.last_message_type = None;
    status.session_epoch = status.session_epoch.wrapping_add(1);
}

fn install_connection_status(
    state: &Arc<Mutex<BrokerStatus>>,
    connection: &ConnectedQc,
    synchronized: bool,
    handshake_ms: u128,
) {
    let mut status = state.lock_recover();
    status.session_epoch = status.session_epoch.wrapping_add(1);
    status.phase = if synchronized { "ready" } else { "syncing" }.into();
    status.detail = if synchronized {
        "Active preset synchronized"
    } else {
        "Handshake complete; waiting for active preset"
    }
    .into();
    status.connected = true;
    status.synchronized = synchronized;
    status.active_preset_name = connection
        .latest_messages
        .get(&quadcortex::profile::MESSAGE_TYPE_RECALL_PRESET)
        .and_then(|message| quadcortex_hid::preset_name(&message.payload));
    status.active_scene = connection
        .latest_messages
        .get(&quadcortex::profile::MESSAGE_TYPE_SCENE)
        .and_then(|message| quadcortex_hid::scene_value(&message.payload));
    status.connected_at_unix_ms = Some(u128::from(quadcortex::time::unix_time_ms()));
    status.handshake_ms = Some(handshake_ms);
    status.messages_received = connection.message_counts.values().sum::<usize>() as u64;
    status.messages_received_by_type = connection
        .message_counts
        .iter()
        .map(|(message_type, count)| (*message_type, *count as u64))
        .collect();
    let telemetry = connection.usb.telemetry();
    status.messages_sent = telemetry.messages_sent;
    status.write_delivery = telemetry.write_delivery;
    status.expected_write_stalls = telemetry.expected_write_stalls;
    status.last_hid_write_duration_ms = telemetry.last_hid_write_duration_ms;
    status.max_hid_write_duration_ms = telemetry.max_hid_write_duration_ms;
    status.last_hid_write_completed = telemetry.last_hid_write_completed;
    status.reader_lock_stall_events = telemetry.reader_lock_stall_events;
    status.max_reader_lock_stall_ms = telemetry.max_reader_lock_stall_ms;
    status.messages_sent_by_type = telemetry.messages_sent_by_type;
    status.last_message_type = connection
        .latest_messages
        .values()
        .max_by_key(|message| message.sequence)
        .map(|message| message.message_type);
    if synchronized {
        status.writes_quarantined = false;
        status.write_quarantine_reason = None;
        status.write_quarantine = None;
    }
}

fn update_usb_telemetry(state: &Arc<Mutex<BrokerStatus>>, connection: &ConnectedQc) {
    let telemetry = connection.usb.telemetry();
    let mut status = state.lock_recover();
    status.messages_sent = telemetry.messages_sent;
    status.write_delivery = telemetry.write_delivery;
    status.expected_write_stalls = telemetry.expected_write_stalls;
    status.last_hid_write_duration_ms = telemetry.last_hid_write_duration_ms;
    status.max_hid_write_duration_ms = telemetry.max_hid_write_duration_ms;
    status.last_hid_write_completed = telemetry.last_hid_write_completed;
    status.reader_lock_stall_events = telemetry.reader_lock_stall_events;
    status.max_reader_lock_stall_ms = telemetry.max_reader_lock_stall_ms;
    status.messages_sent_by_type = telemetry.messages_sent_by_type;
}

fn update_message(
    state: &Arc<Mutex<BrokerStatus>>,
    latest: &mut HashMap<u16, IncomingMessage>,
    message: IncomingMessage,
) {
    let message_type = message.message_type;
    let preset_name = if message_type == quadcortex::profile::MESSAGE_TYPE_RECALL_PRESET {
        quadcortex_hid::preset_name(&message.payload)
    } else {
        None
    };
    let active_scene = if message_type == quadcortex::profile::MESSAGE_TYPE_SCENE {
        quadcortex_hid::scene_value(&message.payload)
    } else {
        None
    };
    if retain_latest_message_payload(latest, &message) {
        if let Err(rejection) = quadcortex_hid::insert_latest_message(latest, message) {
            eprintln!(
                "Did not keep QC frame type {message_type} as the session's latest: {rejection}"
            );
        }
    }
    let mut status = state.lock_recover();
    status.messages_received = status.messages_received.saturating_add(1);
    let count = status
        .messages_received_by_type
        .entry(message_type)
        .or_default();
    *count = count.saturating_add(1);
    status.last_message_type = Some(message_type);
    if message_type == quadcortex::profile::MESSAGE_TYPE_RECALL_PRESET {
        status.active_preset_name = preset_name;
    }
    if let Some(scene) = active_scene {
        status.active_scene = Some(scene)
    }
}

/// Send `messages` in order, `interval` apart, and stop at the first failure.
/// A failed frame is never replayed, with one exception described below.
///
/// A touch gesture that fails after its touch-down was accepted and before it
/// was lifted would leave the device with a pressed pointer, which makes it
/// refuse a backup stream and can click or assign whatever is under it. So, when
/// `release` is given and a frame after the first one fails, the release frame
/// is sent once, best effort, and the returned error says whether it went out.
/// For a stepped drag the failed frame is a MOVE (or the touch-up itself); for
/// a two-frame hold it can only be the touch-up itself, so the release is a
/// second attempt at the identical `RELEASE`, which is the one frame that is
/// given one: a `RELEASE` with nothing pending did nothing on a menu button or
/// row (observed on CorOS 4.1.0, 2026-10-06), so a duplicate costs nothing when
/// the first did arrive. A failure of the FIRST frame gets no release: nothing is
/// known to be pressed, and a release is itself a click on some screens. `send`
/// receives `true` for the release frame. If the release cannot be sent either,
/// the error carries [`TOUCH_UP_UNSENT`] so a hold can record the pointer as held.
fn send_paced_sequence(
    messages: &[OutboundMessage],
    release: Option<&OutboundMessage>,
    interval: Duration,
    mut send: impl FnMut(&OutboundMessage, bool) -> Result<(), String>,
    mut pause: impl FnMut(Duration),
) -> Result<(), String> {
    for (index, message) in messages.iter().enumerate() {
        if let Err(error) = send(message, false) {
            let touch_down_accepted = index > 0;
            return Err(match release.filter(|_| touch_down_accepted) {
                Some(frame) => match send(frame, true) {
                    Ok(()) => format!("{error}; the touch was released"),
                    Err(release_error) => {
                        format!("{error}; {TOUCH_UP_UNSENT}: {release_error}")
                    }
                },
                None => error,
            });
        }
        if index + 1 < messages.len() {
            pause(interval);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
