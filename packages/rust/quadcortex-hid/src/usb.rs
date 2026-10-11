use crate::backend::{write_report_delivered, NativeHid, WriteStallPolicy};
use crate::flight::FlightRecorder;
use hidapi::{HidApi, HidError};
use quadcortex::commands::{self, OutboundMessage};
use quadcortex::device::delivery::{DeliveryStop, DeliveryTicket, ReportOutcome};
use quadcortex::device::initialization::DeviceStartupPhase;
use quadcortex::device::owner::{
    NativeDispatchTicket, ObservationTicket, OperationTicket, SessionRuntime,
};
use quadcortex::device::session::{DispatchOutcome, SessionDriver};
use quadcortex::device::transport::{ReportLayout, TransportRuntime};
use quadcortex::framing;
use quadcortex::profile;
use quadcortex::sync::RecoverPoison;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Wall-clock milliseconds, for the SystemTimeSync the QC is sent at connect.
fn unix_time_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

#[derive(Debug, Error)]
pub enum UsbError {
    #[error("Quad Cortex is not present or its HID interface is owned by another application")]
    NotAvailable,
    #[error("Quad Cortex HID opened, but no ResetCommsBuffers reply arrived within the native handshake timeout")]
    HandshakeTimeout,
    #[error("Quad Cortex initialization failed: {0}")]
    Initialization(String),
    /// The caller asked an in-flight connect to stop (a session reset, a
    /// disconnect, or shutdown). Not a device fault, so nothing is reported as
    /// one and the host may start a fresh attempt at once.
    #[error("Quad Cortex connection attempt was cancelled")]
    Cancelled,
    #[error("USB read failed: {0}")]
    Read(String),
    /// The HID receive queue dropped a report, so the logical frame boundary is
    /// gone. This is not a transient read error and must never be absorbed by
    /// the consecutive-error streak: `read_message` swallowed the overflow
    /// because `read_failed()` wants two errors in a row while
    /// `read_succeeded()` fires on every idle poll, so the latch was consumed,
    /// the error discarded, and the broker went on decoding a discontinuous byte
    /// stream — exactly what setting the latch exists to prevent.
    #[error("USB frame alignment lost: {0}")]
    FrameAlignmentLost(String),
    #[error("USB write failed: {0}")]
    Write(String),
    #[error("USB command was not admitted: {0}")]
    Admission(String),
    #[error("invalid QC frame: {0}")]
    Frame(#[from] framing::FrameError),
    #[error("HID initialization failed: {0}")]
    Hid(String),
}

#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub observation: Option<ObservationTicket>,
    pub sequence: u64,
    pub message_type: u16,
    pub payload: Vec<u8>,
    pub received_at_unix_ms: u128,
}

pub struct ConnectedQc {
    pub usb: QcUsb,
    pub message_counts: HashMap<u16, usize>,
    pub latest_messages: HashMap<u16, IncomingMessage>,
}

impl ConnectedQc {
    pub fn reserve_request_id(&mut self) -> u64 {
        self.usb
            .owner
            .lock_recover()
            .lifecycle
            .as_mut()
            .expect("connected lifecycle")
            .reserve_request_id()
    }

    pub fn send_message(&mut self, message_type: u16, payload: Vec<u8>) -> Result<(), UsbError> {
        self.send_command(OutboundMessage {
            message_type,
            payload,
        })
    }

    pub fn send_command(&mut self, message: OutboundMessage) -> Result<(), UsbError> {
        self.send_command_checked(message, None, None)
    }

    pub fn send_command_checked(
        &mut self,
        mut message: OutboundMessage,
        capture_revision: Option<u64>,
        admission: Option<(&OperationTicket, &Instant)>,
    ) -> Result<(), UsbError> {
        // Stamp and encode under one owner, before handing immutable reports
        // to the bounded native writer. No native I/O occurs under this lock.
        let shared = Arc::clone(&self.usb.owner);
        let mut owner = shared.lock_recover();
        if let Some((ticket, clock)) = admission {
            owner
                .validate_operation(ticket, clock.elapsed().as_millis() as u64)
                .map_err(UsbError::Admission)?;
        }
        if capture_revision
            .is_some_and(|revision| !owner.state.capture_retarget_revision_is_current(revision))
        {
            return Err(UsbError::Initialization("The current model changed while the Neural Capture retarget was queued; refresh and retry".into()));
        }
        owner
            .lifecycle
            .as_mut()
            .filter(|lifecycle| lifecycle.is_active())
            .ok_or_else(|| UsbError::Initialization("no active lifecycle".into()))?
            .stamp_file_mutation_request_id(&mut message)
            .map_err(|error| UsbError::Initialization(error.to_string()))?;
        let reports = encode_session_reports(&owner.transport, &message, self.usb.report_layout)?;
        let guard = NativeWriteGuard::capture(&shared, &owner, admission, capture_revision)?;
        drop(owner);
        self.usb.queue_reports(message.message_type, reports, guard)
    }

    pub fn observe_lifecycle(
        &mut self,
        message: &IncomingMessage,
        now_ms: u64,
    ) -> Result<Option<bool>, UsbError> {
        self.usb.observe_lifecycle(message, now_ms);
        self.usb.drive_lifecycle(now_ms)
    }

    pub fn advance_lifecycle(&mut self, now_ms: u64) -> Result<Option<bool>, UsbError> {
        self.usb.drive_lifecycle(now_ms)
    }
}

/// Record initialization message counts and latest-per-type values. State
/// reduction receives each original observation independently.
fn record_initial(
    message_counts: &mut HashMap<u16, usize>,
    latest_messages: &mut HashMap<u16, IncomingMessage>,
    message: IncomingMessage,
) {
    let count = message_counts.entry(message.message_type).or_default();
    *count = count.saturating_add(1);
    let message_type = message.message_type;
    if let Err(rejection) = crate::latest_cache::insert_latest_message(latest_messages, message) {
        eprintln!("Did not cache initialization message type {message_type}: {rejection}");
    }
}

fn observe_initial(
    message_counts: &mut HashMap<u16, usize>,
    latest_messages: &mut HashMap<u16, IncomingMessage>,
    observe_state: &mut impl FnMut(IncomingMessage),
    message: IncomingMessage,
) {
    // Deliver every startup observation immediately; latest_messages is only
    // the bounded seed for latest-value reads, never a replay source.
    observe_state(message.clone());
    record_initial(message_counts, latest_messages, message);
}

#[derive(Debug, Clone, Default)]
pub struct UsbTelemetry {
    pub write_delivery: WriteDeliveryCounts,
    pub messages_sent: u64,
    pub messages_sent_by_type: HashMap<u16, u64>,
    pub expected_write_stalls: u64,
    pub last_hid_write_duration_ms: u64,
    pub max_hid_write_duration_ms: u64,
    pub last_hid_write_completed: bool,
    /// Count of RX-thread iterations where the `session_owner` lock plus the
    /// bounded channel send took at least [`READER_LOCK_STALL_THRESHOLD_MS`]
    /// between two `read_timeout` calls. This is a heuristic risk signal, not
    /// a direct measurement: the OS/driver-level HID input queue exposes no
    /// overflow flag or counter, so a report dropped there before our reader
    /// thread ever calls `read_timeout` again is invisible to this process.
    /// A rising count means the reader was held off the device long enough,
    /// on the shared `session_owner` mutex, that such an invisible loss
    /// became plausible; it does not prove one occurred.
    pub reader_lock_stall_events: u64,
    pub max_reader_lock_stall_ms: u64,
}

/// Native transport outcomes only. Even a complete frame has no semantic
/// acknowledgement here; mutation verification owns that separate decision.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteDeliveryCounts {
    pub admitted: u64,
    pub started: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
}

impl UsbTelemetry {
    fn record_write(&mut self, duration_ms: u64, completed: bool, expected_stall: bool) {
        self.last_hid_write_duration_ms = duration_ms;
        self.max_hid_write_duration_ms = self.max_hid_write_duration_ms.max(duration_ms);
        self.last_hid_write_completed = completed;
        if expected_stall {
            self.expected_write_stalls = self.expected_write_stalls.saturating_add(1);
        }
    }

    fn record_message(&mut self, message_type: u16) {
        self.messages_sent = self.messages_sent.saturating_add(1);
        let count = self.messages_sent_by_type.entry(message_type).or_default();
        *count = count.saturating_add(1);
    }

    fn record_reader_lock_stall(&mut self, duration_ms: u64) {
        self.max_reader_lock_stall_ms = self.max_reader_lock_stall_ms.max(duration_ms);
        if duration_ms >= READER_LOCK_STALL_THRESHOLD_MS {
            self.reader_lock_stall_events = self.reader_lock_stall_events.saturating_add(1);
        }
    }
}

/// Conservative heuristic for "the RX thread was held off the device long
/// enough on `session_owner` that a report could have been silently dropped
/// by the OS/driver-level HID input queue between two `read_timeout` calls".
/// Not derived from a measured device queue depth or poll interval (neither
/// is available without hardware access); chosen only to separate ordinary
/// scheduler jitter from a hold that plainly reflects lock contention.
const READER_LOCK_STALL_THRESHOLD_MS: u64 = 10;

struct HidWriteMessage {
    message_type: u16,
    reports: Vec<Vec<u8>>,
    guard: Option<NativeWriteGuard>,
}

#[derive(Clone)]
struct NativeWriteGuard {
    owner: Arc<Mutex<SessionRuntime>>,
    ticket: NativeDispatchTicket,
    clock: Option<Instant>,
    delivery: Option<DeliveryTicket>,
}

impl NativeWriteGuard {
    fn capture(
        shared: &Arc<Mutex<SessionRuntime>>,
        owner: &SessionRuntime,
        admission: Option<(&OperationTicket, &Instant)>,
        capture_revision: Option<u64>,
    ) -> Result<Self, UsbError> {
        let now_ms = admission.map_or(0, |(_, clock)| clock.elapsed().as_millis() as u64);
        Ok(Self {
            owner: Arc::clone(shared),
            ticket: owner
                .prepare_dispatch(
                    admission.map(|(ticket, _)| ticket),
                    capture_revision,
                    now_ms,
                )
                .map_err(UsbError::Admission)?,
            clock: admission.map(|(_, clock)| *clock),
            delivery: None,
        })
    }

    fn now_ms(&self) -> u64 {
        self.clock
            .map_or(0, |clock| clock.elapsed().as_millis() as u64)
    }

    fn validate(&self, began: bool) -> Result<(), String> {
        self.owner
            .lock_recover()
            .validate_dispatch(&self.ticket, began, self.now_ms())
    }

    fn begin_report(&self, index: usize) -> Result<(), String> {
        if let Some(delivery) = &self.delivery {
            self.owner
                .lock_recover()
                .begin_native_report(delivery, index, self.now_ms())
        } else {
            self.validate(index > 0)
        }
    }

    fn finish_report(&self, index: usize, outcome: ReportOutcome) -> Result<(), String> {
        if let Some(delivery) = &self.delivery {
            let mut owner = self.owner.lock_recover();
            owner.finish_native_report(delivery, index, outcome, self.now_ms())?;
            if outcome != ReportOutcome::Failed
                && index + 1 == owner.delivery_record(delivery)?.report_count
            {
                owner.require_delivery_complete(delivery)?;
            }
        }
        Ok(())
    }

    fn stop_delivery(&self, reason: DeliveryStop) {
        if let Some(delivery) = &self.delivery {
            let _ = self
                .owner
                .lock_recover()
                .stop_delivery(delivery, reason, self.now_ms());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NativeReadReport {
    bytes: Vec<u8>,
    epoch: u64,
    received_at: Instant,
    received_at_unix_ms: u128,
}

enum HidReadEvent {
    Report(NativeReadReport),
    Idle,
    Error(String),
}

#[derive(Default)]
struct WriteProgress {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl WriteProgress {
    fn snapshot(&self) -> u64 {
        *self.generation.lock_recover()
    }
    fn signal(&self) {
        let mut generation = self.generation.lock_recover();
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
    }
    fn wait(&self, previous: u64, timeout: Duration) {
        let generation = self.generation.lock_recover();
        if *generation == previous {
            drop(
                self.changed
                    .wait_timeout(generation, timeout)
                    .unwrap_or_else(|p| p.into_inner()),
            );
        }
    }
}

fn terminal_write_failure(
    result: &Result<usize, HidError>,
    expected_bytes: usize,
    policy: WriteStallPolicy,
) -> Option<String> {
    if write_report_delivered(result, expected_bytes, policy) {
        return None;
    }
    Some(match result {
        Ok(written) => {
            format!("native HID writer completed a short write ({written}/{expected_bytes} bytes)")
        }
        Err(error) => format!("native HID writer failed: {error}"),
    })
}

fn run_hid_writer<F>(
    receiver: mpsc::Receiver<HidWriteMessage>,
    telemetry: Arc<Mutex<UsbTelemetry>>,
    stopping: Arc<AtomicBool>,
    terminal_error: Arc<Mutex<Option<String>>>,
    progress: Arc<WriteProgress>,
    write_stall_policy: WriteStallPolicy,
    mut write_report: F,
) where
    F: FnMut(&[u8]) -> Result<usize, HidError>,
{
    'messages: while let Ok(message) = receiver.recv() {
        progress.signal();
        let mut began = false;
        for (index, report) in message.reports.into_iter().enumerate() {
            if stopping.load(Ordering::Acquire) {
                if let Some(guard) = &message.guard {
                    guard.stop_delivery(DeliveryStop::Stopped);
                }
                let mut counts = telemetry.lock_recover();
                if began {
                    counts.write_delivery.failed += 1;
                } else {
                    counts.write_delivery.cancelled += 1;
                }
                continue 'messages;
            }
            if let Some(failure) = message
                .guard
                .as_ref()
                .and_then(|guard| guard.begin_report(index).err())
            {
                let mut counts = telemetry.lock().unwrap_or_else(|p| p.into_inner());
                if began {
                    counts.write_delivery.failed += 1;
                    *terminal_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(format!(
                        "native HID frame was partially delivered before admission was revoked: {failure}"
                    ));
                    stopping.store(true, Ordering::Release);
                } else {
                    counts.write_delivery.cancelled += 1;
                }
                continue 'messages;
            }
            if !began {
                telemetry
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .write_delivery
                    .started += 1;
                began = true;
            }
            let started = Instant::now();
            let write_result = write_report(&report);
            let completed = matches!(&write_result, Ok(written) if *written == report.len());
            let failure = terminal_write_failure(&write_result, report.len(), write_stall_policy);
            let expected_stall = !completed && failure.is_none();
            if let Some(guard) = &message.guard {
                let outcome = if failure.is_some() {
                    ReportOutcome::Failed
                } else if expected_stall {
                    ReportOutcome::ExpectedStatusStall
                } else {
                    ReportOutcome::Delivered
                };
                if let Err(error) = guard.finish_report(index, outcome) {
                    telemetry.lock_recover().write_delivery.failed += 1;
                    *terminal_error.lock_recover() = Some(error);
                    stopping.store(true, Ordering::Release);
                    continue 'messages;
                }
            }
            let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            telemetry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record_write(duration_ms, completed, expected_stall);
            if let Some(failure) = failure {
                telemetry
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .write_delivery
                    .failed += 1;
                *terminal_error
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(failure);
                // The accepted queue item may have returned success to its
                // producer already. End the TX lane and wake the session's RX
                // poll so that failure is surfaced asynchronously and the
                // worker reconnects. Drain later queue items as cancelled;
                // no further report reaches the native handle.
                stopping.store(true, Ordering::Release);
                continue 'messages;
            }
            if let Some(delay_ms) = TransportRuntime::report_batch_delay_ms(index + 1) {
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        if began {
            let mut telemetry = telemetry.lock().unwrap_or_else(|p| p.into_inner());
            telemetry.write_delivery.completed += 1;
            telemetry.record_message(message.message_type);
        }
    }
}

struct HidIo {
    receiver: mpsc::Receiver<HidReadEvent>,
    write_sender: Option<mpsc::SyncSender<HidWriteMessage>>,
    write_progress: Arc<WriteProgress>,
    telemetry: Arc<Mutex<UsbTelemetry>>,
    stopping: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    terminal_write_error: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

// A complete maximum-sized logical frame fits in this queue. The queue is
// deliberately finite: a stalled command lane must never turn sustained USB
// input into unbounded process memory.
const HID_READ_QUEUE_CAPACITY: usize = profile::MAX_FRAME_BYTES / framing::CHUNK_SIZE + 2;
// One queue item is a complete logical frame. Keeping reports together prevents
// concurrent producers from interleaving fragments and lets the writer abandon
// the remainder of a frame after the first native write failure.
const HID_WRITE_QUEUE_CAPACITY: usize = 1;

/// How long `finish_writes` waits for the writer thread before handing its
/// join to a detached thread instead. Generous against ordinary write
/// latency (single reports complete in low milliseconds) but short enough
/// that a wedged teardown does not meaningfully delay the shared worker
/// thread from serving every other client.
const FINISH_WRITES_JOIN_BUDGET: Duration = Duration::from_secs(2);

impl HidIo {
    fn start(
        device: NativeHid,
        owner: Arc<Mutex<SessionRuntime>>,
        connection: u64,
    ) -> Result<Self, UsbError> {
        let device = Arc::new(device);
        let write_stall_policy = device.write_stall_policy();
        let stopping = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(HID_READ_QUEUE_CAPACITY);
        let reader_device = Arc::clone(&device);
        let reader_stopping = Arc::clone(&stopping);
        let overflowed = Arc::new(AtomicBool::new(false));
        let reader_overflowed = Arc::clone(&overflowed);
        // Created before the reader thread starts so the reader can record its
        // own lock-contention risk window on the same shared counters the
        // writer thread already reports through.
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let reader_telemetry = Arc::clone(&telemetry);
        let reader = thread::Builder::new()
            .name("qc-native-hid-rx".into())
            .spawn(move || {
                while !reader_stopping.load(Ordering::Acquire) {
                    let mut report = [0_u8; 1024];
                    match reader_device.read_timeout(&mut report, 200) {
                        Ok(0) => match sender.try_send(HidReadEvent::Idle) {
                            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        },
                        Ok(read) => {
                            let received_at = Instant::now();
                            // The OS/driver-level HID input queue exposes no
                            // overflow flag: a report it drops while this
                            // thread is away from `read_timeout` is invisible
                            // to hidapi and to us. Everything between the
                            // previous successful read and the next one -
                            // acquiring `session_owner` and pushing onto the
                            // bounded channel - is exactly that away-from-the-
                            // device window, so time it and surface a risk
                            // signal even though the actual loss cannot be
                            // observed from here.
                            let housekeeping_started = Instant::now();
                            let epoch = match owner.lock_recover().capture_report_epoch(connection)
                            {
                                Ok(epoch) => epoch,
                                Err(error) => {
                                    // Fatal admission errors must reach the consumer even if
                                    // the report queue is already full.
                                    if sender.try_send(HidReadEvent::Error(error)).is_err() {
                                        reader_overflowed.store(true, Ordering::Release);
                                    }
                                    break;
                                }
                            };
                            let received_at_unix_ms = u128::from(quadcortex::time::unix_time_ms());
                            match sender.try_send(HidReadEvent::Report(NativeReadReport {
                                bytes: report[..read].to_vec(),
                                epoch,
                                received_at,
                                received_at_unix_ms,
                            })) {
                                Ok(()) => {}
                                Err(mpsc::TrySendError::Full(_)) => {
                                    // A dropped report invalidates the current logical frame.
                                    // Let the broker tear down and re-open the session rather
                                    // than attempting to decode a discontinuous byte stream.
                                    reader_overflowed.store(true, Ordering::Release);
                                }
                                Err(mpsc::TrySendError::Disconnected(_)) => break,
                            }
                            let housekeeping_ms = housekeeping_started
                                .elapsed()
                                .as_millis()
                                .min(u64::MAX as u128)
                                as u64;
                            reader_telemetry
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .record_reader_lock_stall(housekeeping_ms);
                        }
                        Err(error) => match sender.try_send(HidReadEvent::Error(error)) {
                            Ok(()) => {}
                            Err(mpsc::TrySendError::Full(_)) => {
                                reader_overflowed.store(true, Ordering::Release);
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        },
                    }
                }
            })
            .map_err(|error| UsbError::Hid(format!("could not start HID reader: {error}")))?;
        let (write_sender, write_receiver) =
            mpsc::sync_channel::<HidWriteMessage>(HID_WRITE_QUEUE_CAPACITY);
        let writer_device = Arc::clone(&device);
        let writer_telemetry = Arc::clone(&telemetry);
        let terminal_write_error = Arc::new(Mutex::new(None));
        let writer_terminal_write_error = Arc::clone(&terminal_write_error);
        let writer_stopping = Arc::clone(&stopping);
        let write_progress = Arc::new(WriteProgress::default());
        let writer_progress = Arc::clone(&write_progress);
        let writer = match thread::Builder::new()
            .name("qc-native-hid-tx".into())
            .spawn(move || {
                run_hid_writer(
                    write_receiver,
                    writer_telemetry,
                    writer_stopping,
                    writer_terminal_write_error,
                    writer_progress,
                    write_stall_policy,
                    |report| writer_device.write(report),
                );
            }) {
            Ok(writer) => writer,
            Err(error) => {
                stopping.store(true, Ordering::Release);
                let _ = reader.join();
                return Err(UsbError::Hid(format!(
                    "could not start HID writer: {error}"
                )));
            }
        };
        Ok(Self {
            receiver,
            write_sender: Some(write_sender),
            write_progress,
            telemetry,
            stopping,
            overflowed,
            terminal_write_error,
            reader: Some(reader),
            writer: Some(writer),
        })
    }

    fn queue_message(&self, message_type: u16, reports: Vec<Vec<u8>>) -> Result<(), UsbError> {
        self.queue_message_guarded(message_type, reports, None)
    }

    fn queue_message_guarded(
        &self,
        message_type: u16,
        reports: Vec<Vec<u8>>,
        mut guard: Option<NativeWriteGuard>,
    ) -> Result<(), UsbError> {
        if let Some(error) = self.terminal_write_failure() {
            return Err(UsbError::Write(error));
        }
        if self.stopping.load(Ordering::Acquire) {
            return Err(UsbError::Write("native HID session is stopping".into()));
        }
        if reports.is_empty() {
            return Err(UsbError::Write("cannot submit an empty HID frame".into()));
        }
        let sender = self
            .write_sender
            .as_ref()
            .ok_or_else(|| UsbError::Write("native HID writer stopped".into()))?;
        if let Some(guard) = &mut guard {
            guard.delivery = Some(
                guard
                    .owner
                    .lock_recover()
                    .prepare_delivery(&guard.ticket, message_type, reports.len(), guard.now_ms())
                    .map_err(UsbError::Admission)?,
            );
        }
        let monitor = guard.clone();
        let mut message = HidWriteMessage {
            message_type,
            reports,
            guard,
        };
        // This is a host queue budget, not a firmware transfer deadline. User
        // operations also retain their earlier, usually shorter caller budget.
        let started = Instant::now();
        let wait_budget = Duration::from_millis(profile::COMMAND_CONFIRMATION_TIMEOUT_MS);
        let result = (|| {
            loop {
                if let Some(error) = self.terminal_write_failure() {
                    return Err(UsbError::Write(error));
                }
                if self.stopping.load(Ordering::Acquire) {
                    return Err(UsbError::Write("native HID session is stopping".into()));
                }
                let mut remaining = wait_budget.saturating_sub(started.elapsed());
                if let Some(guard) = &message.guard {
                    guard.validate(false).map_err(UsbError::Admission)?;
                    if let Some(budget_ms) = guard.ticket.remaining_ms(guard.now_ms()) {
                        remaining = remaining.min(Duration::from_millis(budget_ms));
                    }
                }
                if remaining.is_zero() {
                    return Err(UsbError::Admission(
                        "native HID write queue budget expired".into(),
                    ));
                }
                let progress = self.write_progress.snapshot();
                // Publish admission before the consumer can publish completion.
                let mut telemetry = self.telemetry.lock_recover();
                match sender.try_send(message) {
                    Ok(()) => {
                        telemetry.write_delivery.admitted += 1;
                        break;
                    }
                    Err(mpsc::TrySendError::Full(pending)) => message = pending,
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        return Err(UsbError::Write("native HID writer stopped".into()))
                    }
                }
                drop(telemetry);
                self.write_progress.wait(progress, remaining);
            }
            Ok(())
        })();
        if let Some(monitor) = &monitor {
            if result.is_ok() {
                if let Some(delivery) = &monitor.delivery {
                    monitor
                        .owner
                        .lock_recover()
                        .delivery_admitted(delivery)
                        .map_err(UsbError::Admission)?;
                }
            } else {
                monitor.stop_delivery(DeliveryStop::QueueRejected);
            }
        }
        result
    }

    /// Explicit graceful teardown drains the two disconnect frames while RX
    /// remains alive. Drop without this step is cancellation, not a flush.
    ///
    /// The join is bounded: this runs on the single worker thread that also
    /// services every other command and device read, and the writer thread
    /// can be blocked inside an in-flight `HidDevice::write()` with no way
    /// for `stopping` to unblock it (that flag is only checked between
    /// reports, never inside the underlying OS call). A device unplugged
    /// mid-write can leave that call never returning. Past `FINISH_WRITES_JOIN_BUDGET`
    /// this hands the join to a detached thread instead of continuing to
    /// wait on it, so a wedged write degrades to a leaked thread rather than
    /// an unresponsive broker.
    fn finish_writes(&mut self) {
        self.write_sender.take();
        let Some(writer) = self.writer.take() else {
            return;
        };
        let (done_tx, done_rx) = mpsc::channel();
        let watcher = thread::Builder::new()
            .name("qc-hid-writer-join".into())
            .spawn(move || {
                let _ = writer.join();
                let _ = done_tx.send(());
            });
        let Ok(watcher) = watcher else {
            // Could not even start the watcher thread. `writer` was already
            // moved into the closure passed to `spawn`, so a failed spawn
            // drops it along with that closure - there is no handle left
            // here to fall back to a direct join with. This is the same
            // "give up rather than block" outcome as the timeout case below,
            // just reached without a wait; spawn failure this deep into
            // teardown is itself a sign of the resource pressure that would
            // have made the join worth bounding in the first place.
            return;
        };
        if done_rx.recv_timeout(FINISH_WRITES_JOIN_BUDGET).is_err() {
            eprintln!(
                "QC device broker: the HID writer thread did not finish within {:?} of teardown \
                 starting; continuing without it rather than blocking every other client.",
                FINISH_WRITES_JOIN_BUDGET
            );
        }
        // Whether or not it finished in time, this thread's only job was to
        // join `writer` and it is safe to detach: it holds no lock this
        // process needs, and it will end on its own once the OS write call
        // returns (or the process exits).
        drop(watcher);
    }

    fn abort(&self) {
        self.stopping.store(true, Ordering::Release);
        self.write_progress.signal();
    }

    fn telemetry(&self) -> UsbTelemetry {
        self.telemetry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn terminal_write_failure(&self) -> Option<String> {
        self.terminal_write_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn read(&self, timeout_ms: i32) -> Result<(Option<NativeReadReport>, bool), UsbError> {
        if let Some(error) = self.terminal_write_failure() {
            return Err(UsbError::Write(error));
        }
        if self.overflowed.swap(false, Ordering::AcqRel) {
            return Err(UsbError::FrameAlignmentLost(
                "native HID receive queue overflowed; reconnecting to restore frame alignment"
                    .into(),
            ));
        }
        let timeout = if timeout_ms <= 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(timeout_ms as u64)
        };
        let result = match self.receiver.recv_timeout(timeout) {
            Ok(HidReadEvent::Report(report)) => Ok((Some(report), true)),
            Ok(HidReadEvent::Idle) => Ok((None, true)),
            Ok(HidReadEvent::Error(error)) => Err(UsbError::Read(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok((None, false)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(UsbError::Read("native HID reader stopped".into()))
            }
        };
        if let Some(error) = self.terminal_write_failure() {
            return Err(UsbError::Write(error));
        }
        result
    }
}

impl Drop for HidIo {
    fn drop(&mut self) {
        self.abort();
        self.write_sender.take();
        // Linux hidraw has no safe userspace cancellation for a write already
        // inside the kernel. Bound failure teardown too, keeping the handle
        // and advisory ownership lock in the retained writer's Arc. Windows
        // retains its existing teardown policy behind its native backend.
        #[cfg(target_os = "linux")]
        self.finish_writes();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

pub struct QcUsb {
    // Rust drops fields in declaration order. Keep the exclusive I/O owner
    // ahead of HidApi so shutdown joins both I/O lanes and releases USB first.
    io: HidIo,
    owner: Arc<Mutex<SessionRuntime>>,
    connection_identity: u64,
    session_clock: Instant,
    _api: HidApi,
    next_sequence: u64,
    report_layout: ReportLayout,
    flight: FlightRecorder,
    disconnected_state_sent: bool,
    /// The id an implicit teardown (`Drop`) puts on its DELETE subscription.
    /// Reserved from the lifecycle sequence when the session connects, because
    /// `Drop` may run after the shared lifecycle has been invalidated and
    /// the disconnect workflow specifies a dynamic id, not 0.
    teardown_request_id: u64,
}

fn first_supported_path<'a, T>(
    candidates: impl Iterator<Item = (u16, u16, &'a CStr, T)>,
) -> Option<(&'a CStr, T)> {
    candidates
        .filter(|(vendor_id, product_id, _, _)| {
            *vendor_id == profile::VENDOR_ID && profile::supports_product_id(*product_id)
        })
        .map(|(_, _, path, identity)| (path, identity))
        .min_by(|left, right| left.0.to_bytes().cmp(right.0.to_bytes()))
}

/// Local opaque identity of one HID interface. It is matched against a fresh
/// enumeration before opening; callers cannot supply an arbitrary file path.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QcUsbDevice {
    pub id: String,
    pub name: String,
}

pub(crate) fn device_path_id(path: &CStr) -> String {
    path.to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn connected_devices() -> Result<Vec<QcUsbDevice>, UsbError> {
    let api = HidApi::new().map_err(|error| UsbError::Hid(error.to_string()))?;
    let mut devices: Vec<_> = api
        .device_list()
        .filter(|device| {
            device.vendor_id() == profile::VENDOR_ID
                && profile::supports_product_id(device.product_id())
        })
        .map(|device| QcUsbDevice {
            id: crate::backend::device_id(device),
            name: device
                .serial_number()
                .filter(|serial| !serial.is_empty())
                .map(|serial| format!("Quad Cortex · {serial}"))
                .unwrap_or_else(|| "Quad Cortex".into()),
        })
        .collect();
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    devices.dedup_by(|left, right| left.id == right.id);
    if devices.len() > 1 {
        for (index, device) in devices.iter_mut().enumerate() {
            if device.name == "Quad Cortex" {
                device.name = format!("Quad Cortex {}", index + 1);
            }
        }
    }
    Ok(devices)
}

fn selected_supported_path<'a>(
    candidates: impl Iterator<Item = (u16, u16, &'a CStr, String)>,
    device_id: Option<&str>,
) -> Option<(&'a CStr, String)> {
    first_supported_path(
        candidates.filter(|(_, _, _, identity)| device_id.is_none_or(|id| identity == id)),
    )
}

impl QcUsb {
    pub fn latest_inbound_sequence(&self) -> u64 {
        self.next_sequence.saturating_sub(1)
    }

    fn open(
        owner: Arc<Mutex<SessionRuntime>>,
        session_clock: &Instant,
        device_id: Option<&str>,
    ) -> Result<Self, UsbError> {
        let api = HidApi::new().map_err(|error| UsbError::Hid(error.to_string()))?;
        // Enumerate the entire vendor, admit members of the shared
        // product-ID set, then select the lexicographically first HID path.
        // Do not prioritize one product ID or filter by interface use.
        let (path, selected_identity) = selected_supported_path(
            api.device_list().map(|candidate| {
                (
                    candidate.vendor_id(),
                    candidate.product_id(),
                    candidate.path(),
                    crate::backend::device_id(candidate),
                )
            }),
            device_id,
        )
        .ok_or(UsbError::NotAvailable)?;
        let device = NativeHid::open(&api, path, &selected_identity)?;
        let connection_identity = owner
            .lock_recover()
            .opened(session_clock.elapsed().as_millis() as u64)
            .map_err(UsbError::Initialization)?;
        let io = match HidIo::start(device, Arc::clone(&owner), connection_identity) {
            Ok(io) => io,
            Err(error) => {
                owner
                    .lock_recover()
                    .disconnect(session_clock.elapsed().as_millis() as u64, false);
                return Err(error);
            }
        };
        let mut flight = FlightRecorder::open_default();
        flight.event("transport-opened");
        Ok(Self {
            io,
            owner,
            connection_identity,
            session_clock: *session_clock,
            _api: api,
            next_sequence: 1,
            report_layout: ReportLayout::ReportIdPrefixed,
            flight,
            disconnected_state_sent: false,
            teardown_request_id: 0,
        })
    }

    pub fn connect(
        owner: Arc<Mutex<SessionRuntime>>,
        session_clock: &Instant,
        report_handshake_attempt: impl FnMut(u32, UsbTelemetry),
        observe_state: impl FnMut(IncomingMessage),
    ) -> Result<ConnectedQc, UsbError> {
        Self::connect_device(
            owner,
            session_clock,
            None,
            report_handshake_attempt,
            observe_state,
        )
    }

    pub fn connect_device(
        owner: Arc<Mutex<SessionRuntime>>,
        session_clock: &Instant,
        device_id: Option<&str>,
        report_handshake_attempt: impl FnMut(u32, UsbTelemetry),
        observe_state: impl FnMut(IncomingMessage),
    ) -> Result<ConnectedQc, UsbError> {
        Self::connect_device_cancellable(
            owner,
            session_clock,
            device_id,
            report_handshake_attempt,
            observe_state,
            &|| false,
        )
    }

    /// [`Self::connect_device`] that polls `cancelled` between reads (at most
    /// ~200 ms apart) and returns [`UsbError::Cancelled`] once it is true.
    ///
    /// The connect runs on the broker's single worker thread, which services
    /// no command while it is in here; a connect that never finishes (a unit
    /// that keeps rebuilding, one that never echoes the reset) would otherwise
    /// make a requested session reset, a disconnect, and shutdown all wait for
    /// it. `cancelled` is how those requests reach a connect already running.
    pub fn connect_device_cancellable(
        owner: Arc<Mutex<SessionRuntime>>,
        session_clock: &Instant,
        device_id: Option<&str>,
        mut report_handshake_attempt: impl FnMut(u32, UsbTelemetry),
        mut observe_state: impl FnMut(IncomingMessage),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ConnectedQc, UsbError> {
        let mut usb = Self::open(Arc::clone(&owner), session_clock, device_id)?;
        loop {
            if cancelled() {
                // Dropped, not aborted: `Drop` sends the clean disconnect, so
                // the unit is not left holding a session for a host that is
                // about to try again.
                usb.flight.event("handshake-cancelled");
                return Err(UsbError::Cancelled);
            }
            let now_ms = session_clock.elapsed().as_millis() as u64;
            if owner.lock_recover().transport.handshake_timed_out(now_ms) {
                usb.flight.incident("handshake-timeout");
                usb.abort();
                return Err(UsbError::HandshakeTimeout);
            }
            let attempt = {
                let mut owner = owner.lock_recover();
                owner.next_handshake_write(
                    now_ms,
                    uuid::Uuid::new_v4().simple().to_string(),
                    Some(ReportLayout::ReportIdPrefixed),
                )
            };
            let Some(attempt) = attempt else {
                std::thread::yield_now();
                continue;
            };
            usb.flight
                .event(format!("handshake-attempt-{}", attempt.attempt));
            usb.report_layout = attempt.layout;
            usb.drive_lifecycle(now_ms)?;
            report_handshake_attempt(attempt.attempt, usb.telemetry());
            while owner
                .lock_recover()
                .transport
                .awaiting_handshake_reply(session_clock.elapsed().as_millis() as u64)
            {
                if let Some(message) = usb.read_message(200)? {
                    let now_ms = session_clock.elapsed().as_millis() as u64;
                    usb.observe_lifecycle(&message, now_ms);
                    usb.drive_lifecycle(now_ms)?;
                    // Leave as soon as the reset gate is behind us, whatever
                    // phase the lifecycle reached. This loop can only drive
                    // that one gate - it drops ModelRepo in observe_lifecycle
                    // and never calls observe_state - so any later phase that
                    // lands here is stranded until the handshake times out.
                    // finish_hello owns the metadata and state lanes.
                    let past_reset_gate =
                        owner
                            .lock_recover()
                            .lifecycle
                            .as_ref()
                            .is_some_and(|lifecycle| {
                                lifecycle.is_active()
                                    && lifecycle.phase() != DeviceStartupPhase::SessionValidating
                            });
                    if past_reset_gate {
                        usb.flight.event("handshake-reply");
                        return usb.finish_hello(session_clock, &mut observe_state, cancelled);
                    }
                }
                if cancelled() {
                    usb.flight.event("handshake-cancelled");
                    return Err(UsbError::Cancelled);
                }
                if owner
                    .lock_recover()
                    .transport
                    .handshake_timed_out(session_clock.elapsed().as_millis() as u64)
                {
                    usb.flight.incident("handshake-timeout");
                    usb.abort();
                    return Err(UsbError::HandshakeTimeout);
                }
            }
        }
    }

    fn observe_lifecycle(&mut self, message: &IncomingMessage, now_ms: u64) {
        // Metadata returns through the decoder lane after parsing outside the
        // owner; only that typed result may advance Building.
        if message.message_type == profile::MESSAGE_TYPE_MODEL_REPO {
            return;
        }
        if let Some(ticket) = &message.observation {
            self.owner.lock_recover().observe_lifecycle(
                ticket,
                message.message_type,
                &message.payload,
                now_ms,
            );
        }
    }

    /// Pull immutable effects under the shared owner; execute them after
    /// unlocking, then return their identified admission receipt.
    fn drive_lifecycle(&mut self, now_ms: u64) -> Result<Option<bool>, UsbError> {
        loop {
            let batch = {
                let mut owner = self.owner.lock_recover();
                let lifecycle = owner
                    .lifecycle
                    .as_mut()
                    .ok_or_else(|| UsbError::Initialization("no active lifecycle".into()))?;
                lifecycle.next_batch(now_ms, unix_time_ms())
            };
            let Some(batch) = batch else {
                break;
            };
            for message in batch.messages {
                let result = {
                    let shared = Arc::clone(&self.owner);
                    let owner = shared.lock_recover();
                    if !owner
                        .lifecycle
                        .as_ref()
                        .is_some_and(|lifecycle| lifecycle.is_current(&batch.ticket))
                    {
                        break;
                    }
                    let encoded =
                        encode_session_reports(&owner.transport, &message, self.report_layout)
                            .and_then(|reports| {
                                NativeWriteGuard::capture(&shared, &owner, None, None)
                                    .map(|guard| (reports, guard))
                            });
                    drop(owner);
                    encoded.and_then(|(reports, guard)| {
                        self.queue_reports(message.message_type, reports, guard)
                    })
                };
                if let Err(error) = result {
                    // Record before aborting: `abort` consumes the QcUsb that
                    // owns the recorder, so an error not written down first is
                    // lost, leaving an unbroken inbound/outbound trace with no
                    // reason anywhere.
                    self.flight
                        .incident(format!("lifecycle-dispatch-failure: {error}"));
                    if let Some(lifecycle) = self.owner.lock_recover().lifecycle.as_mut() {
                        lifecycle.dispatched(&batch.ticket, DispatchOutcome::Failed);
                    }
                    self.abort();
                    return Err(error);
                }
            }
            if let Some(lifecycle) = self.owner.lock_recover().lifecycle.as_mut() {
                lifecycle.dispatched(&batch.ticket, DispatchOutcome::Accepted);
            }
        }
        let (failure, synchronized) = {
            let mut owner = self.owner.lock_recover();
            let lifecycle = owner
                .lifecycle
                .as_mut()
                .ok_or_else(|| UsbError::Initialization("no active lifecycle".into()))?;
            let failure = lifecycle.failure().cloned();
            let synchronized = lifecycle.take_synchronization_change();
            if failure.is_some() {
                owner.state.reset();
            } else if let Some(synchronized) = synchronized {
                owner
                    .transport
                    .synchronization_completed(now_ms, synchronized);
            }
            (failure, synchronized)
        };
        if let Some(error) = failure {
            self.flight.incident(match error {
                quadcortex::device::session::SessionFailure::Invalid(_) => {
                    "initialization-rejected"
                }
                quadcortex::device::session::SessionFailure::StartupTimeout(_) => {
                    "initialization-timeout"
                }
                quadcortex::device::session::SessionFailure::Protocol(
                    quadcortex::device::initialization::DeviceStartupError::RebuildLoop,
                ) => "initialization-rebuild-loop",
                _ => "initialization-protocol-failure",
            });
            self.abort();
            return Err(UsbError::Initialization(format!(
                "shared lifecycle failed: {error:?}"
            )));
        }
        Ok(synchronized)
    }

    fn finish_hello(
        mut self,
        session_clock: &Instant,
        observe_state: &mut impl FnMut(IncomingMessage),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ConnectedQc, UsbError> {
        self.flight.event("initialization-started");
        let mut message_counts = HashMap::new();
        let mut latest_messages = HashMap::new();
        let synchronized = loop {
            if cancelled() {
                self.flight.event("initialization-cancelled");
                return Err(UsbError::Cancelled);
            }
            self.drive_lifecycle(session_clock.elapsed().as_millis() as u64)?;
            let synchronized = self
                .owner
                .lock_recover()
                .lifecycle
                .as_ref()
                .and_then(SessionDriver::synchronization);
            if let Some(synchronized) = synchronized {
                break synchronized;
            }
            if let Some(message) = self.read_message(100)? {
                self.observe_lifecycle(&message, session_clock.elapsed().as_millis() as u64);
                observe_initial(
                    &mut message_counts,
                    &mut latest_messages,
                    observe_state,
                    message,
                );
            }
        };
        self.flight.event(if synchronized {
            "initialization-synchronized"
        } else {
            "initialization-incomplete"
        });
        self.teardown_request_id = self
            .owner
            .lock_recover()
            .lifecycle
            .as_mut()
            .expect("connected lifecycle")
            .reserve_request_id();
        Ok(ConnectedQc {
            usb: self,
            message_counts,
            latest_messages,
        })
    }

    pub fn send(&mut self, message_type: u16, payload: Vec<u8>) -> Result<(), UsbError> {
        self.send_command(OutboundMessage {
            message_type,
            payload,
        })
    }

    pub fn send_command(&mut self, message: OutboundMessage) -> Result<(), UsbError> {
        self.send_command_with_layout(message, self.report_layout)
    }

    fn send_command_with_layout(
        &mut self,
        message: OutboundMessage,
        layout: ReportLayout,
    ) -> Result<(), UsbError> {
        let message_type = message.message_type;
        let shared = Arc::clone(&self.owner);
        let owner = shared.lock_recover();
        if !owner
            .lifecycle
            .as_ref()
            .is_some_and(SessionDriver::is_active)
        {
            return Err(UsbError::Initialization(
                "no active lifecycle for command admission".into(),
            ));
        }
        let reports = encode_session_reports(&owner.transport, &message, layout)?;
        let guard = NativeWriteGuard::capture(&shared, &owner, None, None)?;
        drop(owner);
        self.queue_reports(message_type, reports, guard)
    }

    fn queue_reports(
        &mut self,
        message_type: u16,
        reports: Vec<Vec<u8>>,
        mut guard: NativeWriteGuard,
    ) -> Result<(), UsbError> {
        guard.clock.get_or_insert(self.session_clock);
        guard.validate(false).map_err(UsbError::Admission)?;
        self.flight.outbound(message_type, reports.len());
        // Windows reports the QC's accepted status-stage STALL only after the
        // data was delivered. Keep the complete frame on the permanent TX lane
        // so fragments cannot interleave and a failed report ends that attempt.
        if let Err(error) = self
            .io
            .queue_message_guarded(message_type, reports, Some(guard))
        {
            self.flight.event("hid-writer-stopped");
            return Err(error);
        }
        Ok(())
    }

    pub fn telemetry(&self) -> UsbTelemetry {
        self.io.telemetry()
    }

    pub fn record_event(&mut self, event: impl Into<String>) {
        self.flight.event(event);
    }

    pub fn record_incident(&mut self, reason: impl Into<String>) {
        self.flight.incident(reason);
    }

    /// Poll one message, and record the reason whenever a read error ends the
    /// session rather than merely being absorbed.
    ///
    /// Recording has to happen before `abort`, because this teardown can occur
    /// during `connect`, where the error consumes the `QcUsb` that owns the
    /// flight recorder - so a reason not written here is lost. Its sibling in
    /// `drive_lifecycle` once lost a fatal fault reason the same way, leaving
    /// an unbroken inbound/outbound trace and no reason anywhere on disk; this
    /// path had the identical gap. The note lives here rather than at the two
    /// call sites because `read_succeeded` and `read_failed` must stay
    /// adjacent: both native hosts share this single policy.
    ///
    /// A single rejected frame the policy tolerates (an over-limit inflation,
    /// a corrupt gzip stream) is dropped without ending the session, but never
    /// silently: the transport's inbound telemetry counts it by type and the
    /// flight record names it as `read-dropped`.
    pub fn read_message(&mut self, timeout_ms: i32) -> Result<Option<IncomingMessage>, UsbError> {
        match self.read_message_poll(timeout_ms) {
            Ok((message, true)) => {
                self.owner.lock_recover().transport.read_succeeded();
                Ok(message)
            }
            Ok((message, false)) => Ok(message),
            // A broken frame stream and a terminal TX result cannot become
            // healthy through a subsequent idle read.
            Err(error @ (UsbError::FrameAlignmentLost(_) | UsbError::Write(_))) => {
                self.flight.incident(format!("read-failure: {error}"));
                self.abort();
                Err(error)
            }
            Err(error) if !self.owner.lock_recover().transport.read_failed() => {
                self.flight.event(format!("read-dropped: {error}"));
                Ok(None)
            }
            Err(error) => {
                self.flight.incident(format!("read-failure: {error}"));
                self.abort();
                Err(error)
            }
        }
    }

    /// Poll one Windows HID event without confusing an empty broker queue with
    /// a successful native read. The shared transport resets its error streak
    /// only for an actual OS idle/report completion.
    fn read_message_poll(
        &mut self,
        timeout_ms: i32,
    ) -> Result<(Option<IncomingMessage>, bool), UsbError> {
        let (report, native_read_succeeded) = self.io.read(timeout_ms)?;
        let Some(report) = report else {
            return Ok((None, native_read_succeeded));
        };
        let (frame, observation) = {
            let mut owner = self.owner.lock_recover();
            if !owner.accepts_connection(self.connection_identity) {
                return Ok((None, true));
            }
            let received_at_ms = report
                .received_at
                .saturating_duration_since(self.session_clock)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            let Some(observed) = owner
                .push_native_report(
                    self.connection_identity,
                    report.epoch,
                    &report.bytes,
                    received_at_ms,
                )
                .map_err(UsbError::Read)?
            else {
                return Ok((None, true));
            };
            observed
        };
        let message_type = frame.message_type;
        let payload = frame.payload;
        self.flight.inbound(message_type, frame.report_count);
        // Preserve the wire payload exactly. Shared quadcortex decoders own
        // bounded gzip handling, matching Android and keeping this adapter at
        // the HID/report boundary.
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        Ok((
            Some(IncomingMessage {
                observation: Some(observation),
                sequence,
                message_type,
                payload,
                received_at_unix_ms: report.received_at_unix_ms,
            }),
            true,
        ))
    }

    pub fn disconnect_exact(&mut self, cpu_load_request_id: u64) {
        if self.disconnected_state_sent {
            return;
        }
        self.disconnected_state_sent = true;
        for message in commands::disconnect_initialization_exact(cpu_load_request_id) {
            if let Ok(reports) = TransportRuntime::encode_reports(&message, self.report_layout) {
                self.flight.outbound(message.message_type, reports.len());
                if self
                    .io
                    .queue_message(message.message_type, reports)
                    .is_err()
                {
                    self.io.abort();
                    break;
                }
            }
        }
        self.io.finish_writes();
    }

    /// A failed session must not flush queued commands or append teardown to
    /// an uncertain frame. The currently executing OS call remains bounded by
    /// its backend timeout; unsent reports are abandoned without replay.
    pub fn abort(&mut self) {
        self.disconnected_state_sent = true;
        self.io.abort();
    }

    pub fn disconnect(&mut self) {
        self.disconnect_exact(self.teardown_request_id);
    }
}

fn encode_session_reports(
    session: &TransportRuntime,
    message: &OutboundMessage,
    layout: ReportLayout,
) -> Result<Vec<Vec<u8>>, UsbError> {
    session
        .encode_reports_with_session(message, layout)
        .map_err(UsbError::from)
}

pub fn preset_name(payload: &[u8]) -> Option<String> {
    quadcortex::responses::decode_recalled_preset_name(payload)
        .ok()
        .flatten()
}

pub fn scene_value(payload: &[u8]) -> Option<u32> {
    quadcortex::responses::decode_selected_scene(payload)
        .ok()
        .flatten()
}

impl Drop for QcUsb {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        encode_session_reports, first_supported_path, observe_initial, HidIo, HidWriteMessage,
        IncomingMessage, ReportLayout, TransportRuntime, UsbError, UsbTelemetry,
        READER_LOCK_STALL_THRESHOLD_MS,
    };
    use hidapi::HidError;
    use quadcortex::device::owner::SessionRuntime;
    #[cfg(windows)]
    use quadcortex::framing;
    use quadcortex::sync::RecoverPoison;
    use quadcortex::{commands::OutboundMessage, profile};
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    };
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;

    #[cfg(windows)]
    use super::write_report_delivered;

    fn message(sequence: u64, message_type: u16, payload: &[u8]) -> IncomingMessage {
        IncomingMessage {
            observation: None,
            sequence,
            message_type,
            payload: payload.to_vec(),
            received_at_unix_ms: u128::from(sequence),
        }
    }

    #[test]
    fn explicit_device_selection_never_falls_back_to_another_attached_unit() {
        let a = CString::new("unit-A").unwrap();
        let b = CString::new("unit-B").unwrap();
        let candidates = [
            (
                profile::VENDOR_ID,
                0x880A,
                a.as_c_str(),
                super::device_path_id(&a),
            ),
            (
                profile::VENDOR_ID,
                0x892F,
                b.as_c_str(),
                super::device_path_id(&b),
            ),
        ];
        let id = super::device_path_id(b.as_c_str());
        assert_eq!(
            super::selected_supported_path(candidates.iter().cloned(), Some(&id)),
            Some((b.as_c_str(), id.clone()))
        );
        assert_eq!(
            super::selected_supported_path(candidates[..1].iter().cloned(), Some(&id)),
            None,
            "unplugging B must not open A, even though A is a supported device"
        );
        assert_eq!(
            super::selected_supported_path(candidates.into_iter(), Some("unknown")),
            None
        );
    }

    #[test]
    fn discovery_filters_the_shared_product_set_and_orders_only_by_hid_path() {
        let rejected_vendor = CString::new("a-rejected-vendor").unwrap();
        let pid_892f = CString::new("z-supported-892f").unwrap();
        let pid_880a = CString::new("m-supported-880a").unwrap();
        let rejected_product = CString::new("b-rejected-product").unwrap();
        let selected = first_supported_path(
            [
                (
                    profile::VENDOR_ID + 1,
                    0x880A,
                    rejected_vendor.as_c_str(),
                    (),
                ),
                (profile::VENDOR_ID, 0x892F, pid_892f.as_c_str(), ()),
                (profile::VENDOR_ID, 0x880A, pid_880a.as_c_str(), ()),
                (profile::VENDOR_ID, 0xFFFF, rejected_product.as_c_str(), ()),
            ]
            .into_iter(),
        );

        assert_eq!(selected, Some((pid_880a.as_c_str(), ())));
    }

    #[test]
    fn selected_node_identity_is_retained_without_a_second_path_lookup() {
        let path = CString::new("/dev/hidraw1").unwrap();
        let expected = "node-generation-before-open";
        let selected = super::selected_supported_path(
            [(
                profile::VENDOR_ID,
                0x880A,
                path.as_c_str(),
                expected.to_owned(),
            )]
            .into_iter(),
            Some(expected),
        )
        .unwrap();
        assert_eq!(selected, (path.as_c_str(), expected.to_owned()));
        assert!(super::selected_supported_path(
            [(
                profile::VENDOR_ID,
                0x880A,
                path.as_c_str(),
                "replacement-generation".to_owned()
            )]
            .into_iter(),
            Some(expected),
        )
        .is_none());
    }

    #[test]
    fn host_encoder_rejects_the_license_message_family_outright() {
        // Neural DSP's own cloud-account/license message families ride on an
        // encrypted trailer this crate does not implement, and no shipped QC
        // Remote feature sends or receives them. Encoding must fail closed
        // regardless of session state.
        let command = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_LICENSE,
            payload: b"license-body".to_vec(),
        };
        let session = TransportRuntime::new(0);
        assert!(
            encode_session_reports(&session, &command, ReportLayout::ReportIdPrefixed).is_err()
        );
    }

    #[test]
    fn initialization_counts_messages_and_caches_latest_by_type() {
        let (mut counts, mut latest) = (HashMap::new(), HashMap::new());
        let delivered = {
            let mut delivered = Vec::new();
            let mut observe = |entry: IncomingMessage| delivered.push(entry);
            for (sequence, folder) in [(1_u64, b"A"), (2, b"B"), (3, b"C")] {
                observe_initial(
                    &mut counts,
                    &mut latest,
                    &mut observe,
                    message(sequence, profile::MESSAGE_TYPE_FILE, folder),
                );
            }
            observe_initial(
                &mut counts,
                &mut latest,
                &mut observe,
                message(4, 15, b"preset"),
            );
            delivered
        };

        assert_eq!(counts.get(&profile::MESSAGE_TYPE_FILE), Some(&3));
        assert_eq!(
            latest
                .get(&profile::MESSAGE_TYPE_FILE)
                .map(|entry| entry.sequence),
            Some(3)
        );
        assert_eq!(
            delivered
                .iter()
                .filter(|entry| entry.message_type == profile::MESSAGE_TYPE_FILE)
                .count(),
            3,
            "every startup folder frame reaches the owner before the connection is published"
        );
        assert_eq!(
            delivered
                .iter()
                .filter(|entry| entry.message_type == profile::MESSAGE_TYPE_FILE)
                .map(|entry| entry.payload.as_slice())
                .collect::<Vec<_>>(),
            vec![b"A".as_slice(), b"B".as_slice(), b"C".as_slice()]
        );
    }

    #[test]
    fn usb_telemetry_tracks_expected_stalls_and_worst_write_latency() {
        let mut telemetry = UsbTelemetry::default();
        telemetry.record_write(3, true, false);
        telemetry.record_write(7, false, true);
        telemetry.record_write(2, true, false);
        telemetry.record_message(10);
        telemetry.record_message(10);
        telemetry.record_message(52);
        assert_eq!(telemetry.expected_write_stalls, 1);
        assert_eq!(telemetry.last_hid_write_duration_ms, 2);
        assert_eq!(telemetry.max_hid_write_duration_ms, 7);
        assert!(telemetry.last_hid_write_completed);
        assert_eq!(telemetry.messages_sent, 3);
        assert_eq!(telemetry.messages_sent_by_type.get(&10), Some(&2));
        assert_eq!(telemetry.messages_sent_by_type.get(&52), Some(&1));
    }

    #[test]
    fn reader_lock_stall_counts_only_holds_at_or_past_the_threshold_but_tracks_every_max() {
        let mut telemetry = UsbTelemetry::default();
        // Below the threshold: an ordinary, momentary lock hold. It still
        // moves the worst-case-latency watermark so a trend is visible before
        // any single hold is long enough to count as a risk event.
        telemetry.record_reader_lock_stall(READER_LOCK_STALL_THRESHOLD_MS - 1);
        assert_eq!(telemetry.reader_lock_stall_events, 0);
        assert_eq!(
            telemetry.max_reader_lock_stall_ms,
            READER_LOCK_STALL_THRESHOLD_MS - 1
        );

        // At the threshold: this is the window in which the OS/driver-level
        // HID input queue - which exposes no overflow counter of its own -
        // could plausibly have dropped a report while the reader thread was
        // held off `read_timeout` by session_owner contention.
        telemetry.record_reader_lock_stall(READER_LOCK_STALL_THRESHOLD_MS);
        assert_eq!(telemetry.reader_lock_stall_events, 1);
        assert_eq!(
            telemetry.max_reader_lock_stall_ms,
            READER_LOCK_STALL_THRESHOLD_MS
        );

        // A smaller stall afterwards must not roll the watermark backwards.
        telemetry.record_reader_lock_stall(1);
        assert_eq!(telemetry.reader_lock_stall_events, 1);
        assert_eq!(
            telemetry.max_reader_lock_stall_ms,
            READER_LOCK_STALL_THRESHOLD_MS
        );

        telemetry.record_reader_lock_stall(READER_LOCK_STALL_THRESHOLD_MS * 5);
        assert_eq!(telemetry.reader_lock_stall_events, 2);
        assert_eq!(
            telemetry.max_reader_lock_stall_ms,
            READER_LOCK_STALL_THRESHOLD_MS * 5
        );
    }

    #[cfg(windows)]
    #[test]
    fn qc_status_stage_stall_is_delivered_but_other_write_errors_are_terminal() {
        let accepted_stall = Err(HidError::IoError {
            error: std::io::Error::from_raw_os_error(31),
        });
        let disconnected = Err(HidError::IoError {
            error: std::io::Error::from_raw_os_error(1167),
        });
        let completed = Ok(129);
        let short_write = Ok(128);

        assert!(write_report_delivered(
            &accepted_stall,
            129,
            WriteStallPolicy::WindowsQcError31
        ));
        assert!(write_report_delivered(
            &completed,
            129,
            WriteStallPolicy::WindowsQcError31
        ));
        assert!(!write_report_delivered(
            &short_write,
            129,
            WriteStallPolicy::WindowsQcError31
        ));
        assert!(!write_report_delivered(
            &disconnected,
            129,
            WriteStallPolicy::WindowsQcError31
        ));
    }

    #[cfg(windows)]
    #[test]
    fn every_fragment_of_a_multi_report_message_continues_after_expected_stall() {
        for message_type in [profile::MESSAGE_TYPE_GRID, profile::MESSAGE_TYPE_FILE, 71] {
            let reports = framing::encode(message_type, &[0x5a; 138]);
            assert_eq!(reports.len(), 2);

            for report in reports {
                let accepted_stall = Err(HidError::IoError {
                    error: std::io::Error::from_raw_os_error(31),
                });
                assert!(write_report_delivered(
                    &accepted_stall,
                    report.len(),
                    WriteStallPolicy::WindowsQcError31
                ));
            }
        }
    }

    #[test]
    fn closed_hid_writer_channel_returns_write_error_without_counting_message() {
        let (write_sender, write_receiver) = mpsc::sync_channel(1);
        drop(write_receiver);
        let (_read_sender, read_receiver) = mpsc::channel();
        let io = HidIo {
            receiver: read_receiver,
            write_sender: Some(write_sender),
            write_progress: Arc::new(super::WriteProgress::default()),
            telemetry: Arc::new(Mutex::new(UsbTelemetry::default())),
            stopping: Arc::new(AtomicBool::new(false)),
            overflowed: Arc::new(AtomicBool::new(false)),
            terminal_write_error: Arc::new(Mutex::new(None)),
            reader: None,
            writer: None,
        };

        let error = io
            .queue_message(profile::MESSAGE_TYPE_KEEP_ALIVE, vec![vec![0_u8; 129]])
            .unwrap_err();

        assert!(matches!(error, UsbError::Write(_)));
        assert_eq!(
            error.to_string(),
            "USB write failed: native HID writer stopped"
        );
        let telemetry = io.telemetry();
        assert_eq!(telemetry.messages_sent, 0);
        assert!(telemetry.messages_sent_by_type.is_empty());
    }

    #[test]
    fn terminal_short_write_is_propagated_and_stops_queued_delivery() {
        let (write_sender, write_receiver) = mpsc::sync_channel(1);
        let (read_sender, read_receiver) = mpsc::channel();
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let terminal_write_error = Arc::new(Mutex::new(None));
        let writes = Arc::new(AtomicUsize::new(0));
        let write_progress = Arc::new(super::WriteProgress::default());
        let writer = {
            let telemetry = Arc::clone(&telemetry);
            let stopping = Arc::clone(&stopping);
            let terminal_write_error = Arc::clone(&terminal_write_error);
            let writes = Arc::clone(&writes);
            let progress = Arc::clone(&write_progress);
            thread::spawn(move || {
                super::run_hid_writer(
                    write_receiver,
                    telemetry,
                    stopping,
                    terminal_write_error,
                    progress,
                    WriteStallPolicy::Strict,
                    move |report| {
                        writes.fetch_add(1, Ordering::SeqCst);
                        Ok(report.len() - 1)
                    },
                )
            })
        };
        let io = HidIo {
            receiver: read_receiver,
            write_sender: Some(write_sender),
            write_progress,
            telemetry,
            stopping,
            overflowed: Arc::new(AtomicBool::new(false)),
            terminal_write_error,
            reader: None,
            writer: Some(writer),
        };

        io.queue_message(4, vec![vec![0_u8; 129], vec![1_u8; 129]])
            .expect("enqueue completes asynchronously");
        let error = loop {
            match io.read(20) {
                Err(error @ UsbError::Write(_)) => break error,
                Ok(_) => continue,
                Err(other) => panic!("unexpected read result: {other}"),
            }
        };
        assert!(error.to_string().contains("short write (128/129 bytes)"));
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert!(matches!(
            io.queue_message(4, vec![vec![2_u8; 129]]),
            Err(UsbError::Write(_))
        ));
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        drop(read_sender);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn measured_qc_epipe_stall_keeps_native_error_then_later_failure_terminal() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(HidWriteMessage {
                message_type: profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                reports: vec![vec![0_u8; 129], vec![1_u8; 129]],
                guard: None,
            })
            .unwrap();
        drop(sender);
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let terminal = Arc::new(Mutex::new(None));
        let writes = AtomicUsize::new(0);
        super::run_hid_writer(
            receiver,
            Arc::clone(&telemetry),
            Arc::clone(&stopping),
            Arc::clone(&terminal),
            Arc::new(super::WriteProgress::default()),
            WriteStallPolicy::LinuxQc880aControlStatusEpipe,
            |_| {
                if writes.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(HidError::IoError {
                        error: std::io::Error::from_raw_os_error(libc::EPIPE),
                    })
                } else {
                    Err(HidError::IoError {
                        error: std::io::Error::from_raw_os_error(libc::EIO),
                    })
                }
            },
        );
        assert_eq!(writes.load(Ordering::SeqCst), 2);
        let counts = telemetry.lock_recover();
        assert_eq!(counts.expected_write_stalls, 1);
        assert!(!counts.last_hid_write_completed);
        assert_eq!(counts.write_delivery.failed, 1);
        assert_eq!(counts.messages_sent, 0, "a partial frame is not sent");
        assert!(stopping.load(Ordering::Acquire));
        assert!(terminal
            .lock_recover()
            .as_deref()
            .unwrap()
            .contains(&libc::EIO.to_string()));
    }

    #[cfg(windows)]
    #[test]
    fn terminal_disconnect_is_propagated_while_error31_remains_delivered() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(HidWriteMessage {
                message_type: 4,
                reports: vec![vec![0_u8; 129], vec![1_u8; 129]],
                guard: None,
            })
            .unwrap();
        drop(sender);
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let terminal = Arc::new(Mutex::new(None));
        let writes = AtomicUsize::new(0);
        super::run_hid_writer(
            receiver,
            Arc::clone(&telemetry),
            Arc::clone(&stopping),
            Arc::clone(&terminal),
            Arc::new(super::WriteProgress::default()),
            WriteStallPolicy::WindowsQcError31,
            |_| {
                let index = writes.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    Err(HidError::IoError {
                        error: std::io::Error::from_raw_os_error(31),
                    })
                } else {
                    Err(HidError::IoError {
                        error: std::io::Error::from_raw_os_error(1167),
                    })
                }
            },
        );
        assert_eq!(writes.load(Ordering::SeqCst), 2);
        let counts = telemetry.lock().unwrap();
        assert_eq!(
            counts.expected_write_stalls, 1,
            "terminal failures are not expected stalls"
        );
        assert_eq!(
            counts.messages_sent, 0,
            "a partial frame is not a sent message"
        );
        assert_eq!(counts.write_delivery.failed, 1);
        assert!(stopping.load(Ordering::Acquire));
        assert!(terminal
            .lock()
            .unwrap()
            .as_deref()
            .unwrap()
            .contains("1167"));
    }

    #[cfg(windows)]
    fn writer_test_stall_policy() -> WriteStallPolicy {
        WriteStallPolicy::WindowsQcError31
    }

    #[cfg(target_os = "linux")]
    fn writer_test_stall_policy() -> WriteStallPolicy {
        WriteStallPolicy::Strict
    }

    fn writer_test_io(
        write: impl FnMut(&[u8]) -> Result<usize, HidError> + Send + 'static,
    ) -> HidIo {
        let (write_sender, write_receiver) = mpsc::sync_channel(1);
        let (_, receiver) = mpsc::channel();
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let terminal_write_error = Arc::new(Mutex::new(None));
        let write_progress = Arc::new(super::WriteProgress::default());
        let writer = {
            let telemetry = Arc::clone(&telemetry);
            let stopping = Arc::clone(&stopping);
            let terminal = Arc::clone(&terminal_write_error);
            let progress = Arc::clone(&write_progress);
            thread::spawn(move || {
                super::run_hid_writer(
                    write_receiver,
                    telemetry,
                    stopping,
                    terminal,
                    progress,
                    writer_test_stall_policy(),
                    write,
                )
            })
        };
        HidIo {
            receiver,
            write_sender: Some(write_sender),
            write_progress,
            telemetry,
            stopping,
            overflowed: Arc::new(AtomicBool::new(false)),
            terminal_write_error,
            reader: None,
            writer: Some(writer),
        }
    }

    fn test_write_guard(expired: bool) -> (Arc<Mutex<SessionRuntime>>, super::NativeWriteGuard) {
        let shared = Arc::new(Mutex::new(SessionRuntime::new(0)));
        let mut owner = shared.lock().unwrap();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "same-wire-id".into(),
            0,
        ));
        let budget = 60_000;
        let ticket = owner.plan_operation(0, budget).unwrap();
        let mut guard = super::NativeWriteGuard::capture(
            &shared,
            &owner,
            Some((&ticket, &Instant::now())),
            None,
        )
        .unwrap();
        if expired {
            guard.clock = Some(Instant::now() - Duration::from_secs(61));
        }
        drop(owner);
        (shared, guard)
    }

    #[cfg(windows)]
    #[test]
    fn native_writer_receipt_preserves_expected_stall_then_short_write_uncertainty() {
        use quadcortex::device::delivery::DeliveryStatus;
        let (owner, mut guard) = test_write_guard(false);
        let ticket = owner
            .lock_recover()
            .prepare_delivery(&guard.ticket, 4, 2, 0)
            .unwrap();
        guard.delivery = Some(ticket.clone());
        let mut io = writer_test_io(|report| {
            if report[0] == 0 {
                Err(HidError::IoError {
                    error: std::io::Error::from_raw_os_error(31),
                })
            } else {
                Ok(0)
            }
        });
        io.write_sender
            .as_ref()
            .unwrap()
            .send(HidWriteMessage {
                message_type: 4,
                reports: vec![vec![0; 129], vec![1; 129]],
                guard: Some(guard),
            })
            .unwrap();
        io.finish_writes();
        let record = owner.lock_recover().delivery_record(&ticket).unwrap();
        assert_eq!(record.status, DeliveryStatus::Indeterminate);
        assert_eq!(record.delivered_reports, 1);
        assert_eq!(record.expected_stalls, 1);
        assert!(io
            .terminal_write_failure()
            .unwrap()
            .contains("short write (0/129"));
        assert_eq!(io.telemetry().write_delivery.completed, 0);
    }

    #[test]
    fn final_native_callback_after_replacement_cannot_report_success() {
        use quadcortex::device::delivery::DeliveryStatus;
        let (owner, mut guard) = test_write_guard(false);
        let ticket = owner
            .lock_recover()
            .prepare_delivery(&guard.ticket, 4, 1, 0)
            .unwrap();
        guard.delivery = Some(ticket.clone());
        let replacement_owner = Arc::clone(&owner);
        let mut io = writer_test_io(move |report| {
            replacement_owner.lock_recover().opened(1).unwrap();
            Ok(report.len())
        });
        io.write_sender
            .as_ref()
            .unwrap()
            .send(HidWriteMessage {
                message_type: 4,
                reports: vec![vec![0; 129]],
                guard: Some(guard),
            })
            .unwrap();
        io.finish_writes();
        assert_eq!(
            owner
                .lock_recover()
                .delivery_record(&ticket)
                .unwrap()
                .status,
            DeliveryStatus::Indeterminate
        );
        assert!(io
            .terminal_write_failure()
            .unwrap()
            .contains("Indeterminate"));
        assert_eq!(io.telemetry().write_delivery.completed, 0);
    }

    #[test]
    fn expired_native_admission_discards_a_frame_before_any_report_is_written() {
        let (_, guard) = test_write_guard(true);
        let writes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&writes);
        let mut io = writer_test_io(move |report| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(report.len())
        });
        io.write_sender
            .as_ref()
            .unwrap()
            .send(HidWriteMessage {
                message_type: 4,
                reports: vec![vec![0; 129]],
                guard: Some(guard),
            })
            .unwrap();
        io.finish_writes();
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert_eq!(io.telemetry().write_delivery.cancelled, 1);
        assert_eq!(io.telemetry().write_delivery.started, 0);
        assert!(io.terminal_write_failure().is_none());
    }

    #[test]
    fn receive_queue_preserves_the_native_arrival_time() {
        let mut io = writer_test_io(|_| panic!("a receive test must not write"));
        let (sender, receiver) = mpsc::channel();
        io.receiver = receiver;
        let arrived = std::time::Instant::now() - Duration::from_secs(1);
        sender
            .send(super::HidReadEvent::Report(super::NativeReadReport {
                bytes: vec![1, 2, 3],
                epoch: 7,
                received_at: arrived,
                received_at_unix_ms: 123,
            }))
            .unwrap();
        let (report, native_read_succeeded) = io.read(0).unwrap();
        let report = report.unwrap();
        assert!(native_read_succeeded);
        assert_eq!(report.bytes, [1, 2, 3]);
        assert_eq!(report.epoch, 7);
        assert_eq!(report.received_at, arrived);
        assert_eq!(report.received_at_unix_ms, 123);
    }

    #[test]
    fn a_queued_old_frame_cannot_write_into_a_replacement_with_the_same_wire_id() {
        let (owner, guard) = test_write_guard(false);
        {
            let mut session = owner.lock().unwrap();
            session.opened(0).unwrap();
            session.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
                7,
                "same-wire-id".into(),
                0,
            ));
        }
        let writes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&writes);
        let mut io = writer_test_io(move |report| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(report.len())
        });
        io.write_sender
            .as_ref()
            .unwrap()
            .send(HidWriteMessage {
                message_type: 4,
                reports: vec![vec![0; 129]],
                guard: Some(guard),
            })
            .unwrap();
        io.finish_writes();
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert_eq!(io.telemetry().write_delivery.cancelled, 1);
        assert!(!io.stopping.load(Ordering::Acquire));
        assert!(owner
            .lock()
            .unwrap()
            .lifecycle
            .as_ref()
            .unwrap()
            .is_active());
    }

    #[test]
    fn revocation_after_one_fragment_is_terminal_partial_delivery_not_success() {
        let (owner, guard) = test_write_guard(false);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(HidWriteMessage {
                message_type: 4,
                reports: vec![vec![0; 129], vec![1; 129]],
                guard: Some(guard),
            })
            .unwrap();
        sender
            .send(HidWriteMessage {
                message_type: 71,
                reports: vec![vec![2; 129]],
                guard: None,
            })
            .unwrap();
        drop(sender);
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let terminal = Arc::new(Mutex::new(None));
        let writes = AtomicUsize::new(0);
        super::run_hid_writer(
            receiver,
            Arc::clone(&telemetry),
            Arc::clone(&stopping),
            Arc::clone(&terminal),
            Arc::new(super::WriteProgress::default()),
            WriteStallPolicy::Strict,
            |report| {
                writes.fetch_add(1, Ordering::SeqCst);
                owner.lock().unwrap().disconnect(1, false);
                Ok(report.len())
            },
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        let counts = telemetry.lock().unwrap();
        assert_eq!(counts.write_delivery.failed, 1);
        assert_eq!(counts.write_delivery.cancelled, 1);
        assert_eq!(counts.write_delivery.completed, 0);
        assert_eq!(counts.messages_sent, 0);
        assert!(stopping.load(Ordering::Acquire));
        assert!(terminal
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains("partially delivered"));
    }

    #[test]
    fn caller_admission_expiry_does_not_invent_a_fragment_transfer_deadline() {
        let (_, guard) = test_write_guard(true);
        assert!(guard.validate(false).is_err());
        assert!(guard.validate(true).is_ok());
    }

    #[test]
    fn a_full_native_queue_spends_the_original_operation_budget_without_enqueueing_it() {
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let writes = Arc::clone(&calls);
        let mut io = writer_test_io(move |report| {
            if writes.fetch_add(1, Ordering::SeqCst) == 0 {
                entered.send(()).unwrap();
                // An incorrectly unbounded producer still terminates the test.
                let _ = released.recv_timeout(Duration::from_secs(2));
            }
            Ok(report.len())
        });
        io.queue_message(26, vec![vec![0; 129]]).unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        io.queue_message(49, vec![vec![1; 129]]).unwrap();
        let (owner, _) = test_write_guard(false);
        let guard;
        let clock = Instant::now();
        {
            let mut session = owner.lock().unwrap();
            let ticket = session.plan_operation(0, 20).unwrap();
            guard =
                super::NativeWriteGuard::capture(&owner, &session, Some((&ticket, &clock)), None)
                    .unwrap();
        }
        let result = io.queue_message_guarded(71, vec![vec![2; 129]], Some(guard));
        release.send(()).unwrap();
        io.finish_writes();
        assert!(matches!(result, Err(UsbError::Admission(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(io.telemetry().write_delivery.admitted, 2);
        assert_eq!(io.telemetry().messages_sent_by_type.get(&71), None);
    }

    #[test]
    fn one_pending_slot_preserves_backpressure_for_a_long_control_burst() {
        let mut io = writer_test_io(|report| {
            thread::sleep(Duration::from_millis(1));
            Ok(report.len())
        });
        for _ in 0..25 {
            io.queue_message(26, vec![vec![0; 129]]).unwrap();
        }
        io.finish_writes();
        assert_eq!(io.telemetry().write_delivery.admitted, 25);
        assert_eq!(io.telemetry().write_delivery.completed, 25);
        assert_eq!(io.telemetry().write_delivery.cancelled, 0);
    }

    #[test]
    fn queue_admission_is_not_native_completion_and_graceful_close_drains() {
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let mut io = writer_test_io(move |report| {
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(report.len())
        });
        io.queue_message(26, vec![vec![0; 129]]).unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let counts = io.telemetry();
        assert_eq!(counts.write_delivery.admitted, 1);
        assert_eq!(counts.write_delivery.started, 1);
        assert_eq!(counts.messages_sent, 0);
        assert_eq!(counts.write_delivery.completed, 0);
        release.send(()).unwrap();
        io.queue_message(49, vec![vec![1; 129]]).unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        release.send(()).unwrap();
        io.finish_writes();
        let counts = io.telemetry();
        assert_eq!(counts.write_delivery.completed, 2);
        assert_eq!(counts.messages_sent_by_type.get(&26), Some(&1));
        assert_eq!(counts.messages_sent_by_type.get(&49), Some(&1));
        assert_eq!(counts.write_delivery.cancelled, 0);
    }

    #[test]
    fn abort_discards_the_partial_frame_and_queued_message_without_replay() {
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let writes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&writes);
        let mut io = writer_test_io(move |report| {
            observed.fetch_add(1, Ordering::SeqCst);
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(report.len())
        });
        io.queue_message(4, vec![vec![0; 129], vec![1; 129]])
            .unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        io.queue_message(71, vec![vec![2; 129]]).unwrap();
        io.abort();
        release.send(()).unwrap();
        io.finish_writes();
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        let counts = io.telemetry();
        assert_eq!(counts.write_delivery.admitted, 2);
        assert_eq!(counts.write_delivery.started, 1);
        assert_eq!(counts.write_delivery.failed, 1);
        assert_eq!(counts.write_delivery.cancelled, 1);
        assert_eq!(counts.messages_sent, 0);
        assert!(io.queue_message(49, vec![vec![3; 129]]).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_native_error_stops_the_frame_and_cancels_queued_delivery() {
        let (sender, receiver) = mpsc::channel();
        for message_type in [4, 71] {
            sender
                .send(HidWriteMessage {
                    message_type,
                    reports: vec![vec![0; 129], vec![1; 129]],
                    guard: None,
                })
                .unwrap();
        }
        drop(sender);
        let telemetry = Arc::new(Mutex::new(UsbTelemetry::default()));
        let terminal = Arc::new(Mutex::new(None));
        let writes = AtomicUsize::new(0);
        super::run_hid_writer(
            receiver,
            Arc::clone(&telemetry),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&terminal),
            Arc::new(super::WriteProgress::default()),
            WriteStallPolicy::Strict,
            |_| {
                writes.fetch_add(1, Ordering::SeqCst);
                // The Windows native status-stage exception must not turn a
                // Linux errno into successful delivery of subsequent reports.
                Err(HidError::IoError {
                    error: std::io::Error::from_raw_os_error(31),
                })
            },
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        let telemetry = telemetry.lock_recover();
        assert_eq!(telemetry.write_delivery.failed, 1);
        assert_eq!(telemetry.write_delivery.cancelled, 1);
        assert_eq!(telemetry.write_delivery.completed, 0);
        assert_eq!(telemetry.expected_write_stalls, 0);
        assert!(terminal.lock_recover().is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_failure_drop_is_bounded_while_native_write_retains_its_handle() {
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (exited, finished) = mpsc::channel();
        let writes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&writes);
        struct NativeOwner(mpsc::Sender<()>);
        impl Drop for NativeOwner {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let retained_owner = NativeOwner(exited);
        let io = writer_test_io(move |report| {
            let _owner = &retained_owner;
            observed.fetch_add(1, Ordering::SeqCst);
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(report.len())
        });
        io.queue_message(4, vec![vec![0; 129], vec![1; 129]])
            .unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        drop(io);
        assert!(start.elapsed() < super::FINISH_WRITES_JOIN_BUDGET + Duration::from_secs(2));
        assert!(
            finished.try_recv().is_err(),
            "in-flight I/O still owns its handle"
        );
        release.send(()).unwrap();
        finished
            .recv_timeout(Duration::from_secs(2))
            .expect("retained I/O cleans up once its syscall returns");
        assert_eq!(
            writes.load(Ordering::SeqCst),
            1,
            "abort never sends the remaining fragment"
        );
    }

    #[cfg(windows)]
    #[test]
    fn successful_status_stalls_complete_every_report_but_do_not_claim_os_success() {
        let mut io = writer_test_io(|_| {
            Err(HidError::IoError {
                error: std::io::Error::from_raw_os_error(31),
            })
        });
        io.queue_message(4, vec![vec![0; 129], vec![1; 129]])
            .unwrap();
        io.finish_writes();
        let counts = io.telemetry();
        assert_eq!(counts.expected_write_stalls, 2);
        assert_eq!(counts.messages_sent, 1);
        assert_eq!(counts.write_delivery.completed, 1);
        assert_eq!(counts.write_delivery.failed, 0);
        assert!(!counts.last_hid_write_completed);
    }

    // ------------------------------------------------------------------
    // Failed downward-gesture HID framing regression (tests only).
    //
    // Both gestures that did not navigate on installed Windows 0.1.180 are
    // pushed through the PRODUCTION frame builder (`commands::screen_drag`, the
    // same builder `DeviceOperation::ScreenDrag` calls), the PRODUCTION
    // `encode_session_reports` / `TransportRuntime`, and the PRODUCTION
    // `run_hid_writer` (through `writer_test_io`) with a fake write closure.
    // No HID handle is opened and nothing sleeps.
    //
    // Expected bytes are INDEPENDENT hand-derived literals, never produced by
    // the code under test. Derivation (framing.rs `encode_frame`, `profile`):
    //
    //   logical message = payload || trailer
    //   trailer (8 bytes) = message_type u32 LE (72 -> 48 00 00 00),
    //                       encrypted 00, compressed 00, device bytes 00 00
    //   report (129 bytes) = 02 (OUT report id), <len>, <flags>, chunk, zero pad
    //   <len>  = chunk length = payload length + 8 (one chunk, since <= 126)
    //   <flags>= FIRST 0x40 | LAST 0x80 = 0xc0 for a single-report message
    //   BodyOnly layout = the same report without the leading 02.
    //
    // Payload (protobuf RemoteControlMessage, UPDATE, mouse DRAG; floats are
    // IEEE-754 little-endian; y == 0 is omitted by proto3):
    //   08 01            action = UPDATE
    //   1a LL            field 3 (mouse), LL = body length
    //     0d <x f32>     400.0 = 0x43c80000 -> 00 00 c8 43
    //     15 <y f32>     100.0 = 0x42c80000 -> 00 00 c8 42   (omitted when 0)
    //     18 04          type = DRAG (4)
    //     25 <to_x f32>  400.0 -> 00 00 c8 43
    //     2d <to_y f32>  440.0 = 0x43dc0000 -> 00 00 dc 43
    //                    479.0 = 0x43ef8000 -> 00 80 ef 43
    //
    // Pair 1 (400,100)->(400,440): LL = 5+5+2+5+5 = 22 = 0x16, payload 26 bytes,
    //   chunk = 26 + 8 = 34 = 0x22.
    // Pair 2 (400,0)->(400,479): y omitted, LL = 5+2+5+5 = 17 = 0x11, payload
    //   21 bytes, chunk = 21 + 8 = 29 = 0x1d.
    //
    // Each literal below is the report up to and including the trailer; the
    // remaining bytes of the 129-byte report must be zero padding.
    const FAILED_DRAG_FRAMES: [(&str, [f32; 4], &str); 2] = [
        (
            "mid-grid (400,100)->(400,440)",
            [400.0, 100.0, 400.0, 440.0],
            concat!(
                "02",
                "22",
                "c0",
                "08011a160d0000c843150000c8421804250000c8432d0000dc43",
                "4800000000000000"
            ),
        ),
        (
            "top-edge (400,0)->(400,479)",
            [400.0, 0.0, 400.0, 479.0],
            concat!(
                "02",
                "1d",
                "c0",
                "08011a110d0000c8431804250000c8432d0080ef43",
                "4800000000000000"
            ),
        ),
    ];

    const PREFIXED: ReportLayout = ReportLayout::ReportIdPrefixed;

    fn hex_bytes(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn failed_drag_reports(coordinates: [f32; 4], layout: ReportLayout) -> Vec<Vec<u8>> {
        let [x, y, to_x, to_y] = coordinates;
        let [frame] = quadcortex::commands::screen_drag(x, y, to_x, to_y);
        assert_eq!(frame.message_type, profile::MESSAGE_TYPE_REMOTE_CONTROL);
        encode_session_reports(&TransportRuntime::new(0), &frame, layout).unwrap()
    }

    fn assert_exact_drag_report(
        label: &str,
        report: &[u8],
        prefix_hex: &str,
        layout: ReportLayout,
    ) {
        let prefix = hex_bytes(prefix_hex);
        let (expected_len, expected_prefix) = match layout {
            ReportLayout::ReportIdPrefixed => (129, prefix.as_slice()),
            ReportLayout::BodyOnly => (128, &prefix[1..]),
        };
        assert_eq!(report.len(), expected_len, "{label}: report length");
        assert_eq!(
            &report[..expected_prefix.len()],
            expected_prefix,
            "{label}: header, payload and trailer bytes"
        );
        assert!(
            report[expected_prefix.len()..]
                .iter()
                .all(|byte| *byte == 0),
            "{label}: padding must be zero"
        );
    }

    fn trailer_message_type(report: &[u8]) -> u32 {
        // Hand parser, independent of framing::decode: chunk length is the
        // second byte, the trailer is the last 8 bytes of the chunk.
        let length = usize::from(report[1]);
        let trailer = &report[3 + length - 8..3 + length];
        u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]])
    }

    fn io_capturing_writes(stall: bool) -> (HidIo, Arc<Mutex<Vec<Vec<u8>>>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&written);
        let io = writer_test_io(move |report| {
            sink.lock().unwrap().push(report.to_vec());
            if stall {
                Err(HidError::IoError {
                    error: std::io::Error::from_raw_os_error(31),
                })
            } else {
                Ok(report.len())
            }
        });
        (io, written)
    }

    #[test]
    fn hid_writer_frames_failed_drag_coordinates_exactly_in_both_report_layouts() {
        for (label, coordinates, prefix_hex) in FAILED_DRAG_FRAMES {
            for layout in [ReportLayout::ReportIdPrefixed, ReportLayout::BodyOnly] {
                let reports = failed_drag_reports(coordinates, layout);
                assert_eq!(reports.len(), 1, "{label}: one DRAG frame is one report");
                assert_exact_drag_report(label, &reports[0], prefix_hex, layout);
            }
        }
    }

    #[test]
    fn hid_writer_frames_failed_drag_coordinates_exactly_on_normal_delivery() {
        for (label, coordinates, prefix_hex) in FAILED_DRAG_FRAMES {
            let reports = failed_drag_reports(coordinates, ReportLayout::ReportIdPrefixed);
            let (mut io, written) = io_capturing_writes(false);
            io.queue_message(profile::MESSAGE_TYPE_REMOTE_CONTROL, reports)
                .unwrap();
            io.finish_writes();

            let written = written.lock().unwrap();
            assert_eq!(written.len(), 1, "{label}: exactly one report written");
            assert_exact_drag_report(label, &written[0], prefix_hex, PREFIXED);

            let counts = io.telemetry();
            assert_eq!(counts.write_delivery.admitted, 1, "{label}");
            assert_eq!(counts.write_delivery.started, 1, "{label}");
            assert_eq!(counts.write_delivery.completed, 1, "{label}");
            assert_eq!(counts.write_delivery.failed, 0, "{label}");
            assert_eq!(counts.write_delivery.cancelled, 0, "{label}");
            assert_eq!(counts.messages_sent, 1, "{label}");
            assert_eq!(
                counts
                    .messages_sent_by_type
                    .get(&profile::MESSAGE_TYPE_REMOTE_CONTROL),
                Some(&1),
                "{label}"
            );
            assert_eq!(counts.expected_write_stalls, 0, "{label}");
            assert!(counts.last_hid_write_completed, "{label}");
            assert!(io.terminal_write_failure().is_none(), "{label}");
            assert!(!io.stopping.load(Ordering::Acquire), "{label}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn hid_writer_frames_failed_drag_coordinates_exactly_through_error31_status_stall() {
        for (label, coordinates, prefix_hex) in FAILED_DRAG_FRAMES {
            let reports = failed_drag_reports(coordinates, ReportLayout::ReportIdPrefixed);
            let (mut io, written) = io_capturing_writes(true);
            io.queue_message(profile::MESSAGE_TYPE_REMOTE_CONTROL, reports)
                .unwrap();
            io.finish_writes();

            // ERROR_GEN_FAILURE (31) is the QC's accepted-then-STALLed status
            // stage: the bytes handed to the native write are identical to the
            // normal-delivery case and the frame counts as delivered.
            let written = written.lock().unwrap();
            assert_eq!(written.len(), 1, "{label}: exactly one report written");
            assert_exact_drag_report(label, &written[0], prefix_hex, PREFIXED);

            let counts = io.telemetry();
            assert_eq!(counts.expected_write_stalls, 1, "{label}");
            assert_eq!(counts.write_delivery.admitted, 1, "{label}");
            assert_eq!(counts.write_delivery.started, 1, "{label}");
            assert_eq!(counts.write_delivery.completed, 1, "{label}");
            assert_eq!(counts.write_delivery.failed, 0, "{label}");
            assert_eq!(counts.write_delivery.cancelled, 0, "{label}");
            assert_eq!(counts.messages_sent, 1, "{label}");
            assert_eq!(
                counts
                    .messages_sent_by_type
                    .get(&profile::MESSAGE_TYPE_REMOTE_CONTROL),
                Some(&1),
                "{label}"
            );
            // A stalled report is delivered but never "completed" by the OS.
            assert!(!counts.last_hid_write_completed, "{label}");
            assert!(io.terminal_write_failure().is_none(), "{label}");
            assert!(!io.stopping.load(Ordering::Acquire), "{label}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn hid_writer_records_failed_drag_error31_stall_as_a_completed_delivery_receipt() {
        use quadcortex::device::delivery::DeliveryStatus;
        for (label, coordinates, prefix_hex) in FAILED_DRAG_FRAMES {
            let reports = failed_drag_reports(coordinates, ReportLayout::ReportIdPrefixed);
            let (owner, mut guard) = test_write_guard(false);
            let ticket = owner
                .lock_recover()
                .prepare_delivery(&guard.ticket, profile::MESSAGE_TYPE_REMOTE_CONTROL, 1, 0)
                .unwrap();
            guard.delivery = Some(ticket.clone());
            let (mut io, written) = io_capturing_writes(true);
            io.write_sender
                .as_ref()
                .unwrap()
                .send(HidWriteMessage {
                    message_type: profile::MESSAGE_TYPE_REMOTE_CONTROL,
                    reports,
                    guard: Some(guard),
                })
                .unwrap();
            io.finish_writes();

            let written = written.lock().unwrap();
            assert_eq!(written.len(), 1, "{label}");
            assert_exact_drag_report(label, &written[0], prefix_hex, PREFIXED);
            let record = owner.lock_recover().delivery_record(&ticket).unwrap();
            assert_eq!(record.status, DeliveryStatus::Completed, "{label}");
            assert_eq!(record.report_count, 1, "{label}");
            assert_eq!(record.delivered_reports, 1, "{label}");
            assert_eq!(record.expected_stalls, 1, "{label}");
            let counts = io.telemetry();
            assert_eq!(counts.expected_write_stalls, 1, "{label}");
            assert_eq!(counts.write_delivery.completed, 1, "{label}");
            assert_eq!(counts.write_delivery.failed, 0, "{label}");
            assert_eq!(counts.messages_sent, 1, "{label}");
            assert!(io.terminal_write_failure().is_none(), "{label}");
        }
    }

    /// Narrow ordering test. It proves only that messages already placed in the
    /// writer queue, in this order, reach the write function in this order with
    /// each message's reports adjacent and the DRAG bytes unchanged. It runs an
    /// idle fake queue fed by one thread, so it does NOT show what the live
    /// scheduler (keepalive timer, read polling, concurrent producers) enqueues
    /// around a gesture, and it must not be read as proof that the live path
    /// cannot interleave other traffic with a DRAG.
    #[test]
    fn hid_writer_preserves_queued_order_around_a_failed_drag_frame() {
        let (label, coordinates, prefix_hex) = FAILED_DRAG_FRAMES[0];
        for stall in [false, true]
            .into_iter()
            .filter(|stall| !*stall || cfg!(windows))
        {
            let session = TransportRuntime::new(0);
            let layout = ReportLayout::ReportIdPrefixed;
            let neighbours = [
                quadcortex::commands::keepalive(),
                quadcortex::commands::read_graphics_tree(),
            ];
            let trailing = [
                quadcortex::commands::capture_screen(),
                quadcortex::commands::keepalive(),
            ];
            let mut queue: Vec<(u16, Vec<Vec<u8>>)> = Vec::new();
            for message in &neighbours {
                queue.push((
                    message.message_type,
                    encode_session_reports(&session, message, layout).unwrap(),
                ));
            }
            queue.push((
                profile::MESSAGE_TYPE_REMOTE_CONTROL,
                failed_drag_reports(coordinates, layout),
            ));
            for message in &trailing {
                queue.push((
                    message.message_type,
                    encode_session_reports(&session, message, layout).unwrap(),
                ));
            }

            let expected_reports: Vec<Vec<u8>> = queue
                .iter()
                .flat_map(|(_, reports)| reports.iter().cloned())
                .collect();
            let (mut io, written) = io_capturing_writes(stall);
            for (message_type, reports) in queue {
                io.queue_message(message_type, reports).unwrap();
            }
            io.finish_writes();

            let written = written.lock().unwrap();
            assert_eq!(
                *written, expected_reports,
                "{label}: exact queued report order"
            );
            let types: Vec<u32> = written
                .iter()
                .map(|report| trailer_message_type(report))
                .collect();
            assert_eq!(types, vec![32, 72, 72, 72, 32], "{label}: stall={stall}");
            assert_exact_drag_report(label, &written[2], prefix_hex, layout);
            for index in [0, 1, 3, 4] {
                assert_ne!(
                    written[index], written[2],
                    "{label}: neighbour {index} must not carry the DRAG bytes"
                );
            }
            let counts = io.telemetry();
            assert_eq!(counts.messages_sent, 5, "stall={stall}");
            assert_eq!(
                counts.messages_sent_by_type.get(&32),
                Some(&2),
                "stall={stall}"
            );
            assert_eq!(
                counts.messages_sent_by_type.get(&72),
                Some(&3),
                "stall={stall}"
            );
            assert_eq!(counts.write_delivery.completed, 5, "stall={stall}");
            assert_eq!(counts.write_delivery.failed, 0, "stall={stall}");
            assert_eq!(counts.expected_write_stalls, if stall { 5 } else { 0 });
        }
    }

    /// Same narrow claim for multi-report neighbours: queued whole messages are
    /// written one after another, so a DRAG queued between two two-report
    /// messages is never split by, nor split across, their fragments. The
    /// neighbours are synthetic 138-byte type-72 payloads, not real commands.
    /// Windows also exercises its accepted error-31 completion here. Linux
    /// exercises full writes; its separate failure test requires error 31 to
    /// stop the writer instead of treating it as a delivered report.
    #[test]
    fn hid_writer_keeps_each_queued_message_contiguous_around_a_failed_drag_frame() {
        let (label, coordinates, prefix_hex) = FAILED_DRAG_FRAMES[1];
        let session = TransportRuntime::new(0);
        let layout = ReportLayout::ReportIdPrefixed;
        let synthetic = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_REMOTE_CONTROL,
            payload: vec![0x5a; 138],
        };
        let fragments = encode_session_reports(&session, &synthetic, layout).unwrap();
        assert_eq!(fragments.len(), 2, "138 + 8 trailer bytes need two chunks");

        let stall = cfg!(windows);
        let (mut io, written) = io_capturing_writes(stall);
        io.queue_message(profile::MESSAGE_TYPE_REMOTE_CONTROL, fragments.clone())
            .unwrap();
        io.queue_message(
            profile::MESSAGE_TYPE_REMOTE_CONTROL,
            failed_drag_reports(coordinates, layout),
        )
        .unwrap();
        io.queue_message(profile::MESSAGE_TYPE_REMOTE_CONTROL, fragments.clone())
            .unwrap();
        io.finish_writes();

        let written = written.lock().unwrap();
        assert_eq!(written.len(), 5, "{label}");
        // Chunk flags: FIRST 0x40 / LAST 0x80 in the third byte of each report.
        let flags: Vec<u8> = written.iter().map(|report| report[2] & 0xc0).collect();
        assert_eq!(flags, vec![0x40, 0x80, 0xc0, 0x40, 0x80], "{label}");
        assert_eq!(written[0], fragments[0], "{label}");
        assert_eq!(written[1], fragments[1], "{label}");
        assert_exact_drag_report(label, &written[2], prefix_hex, layout);
        assert_eq!(written[3], fragments[0], "{label}");
        assert_eq!(written[4], fragments[1], "{label}");
        assert_eq!(
            io.telemetry().expected_write_stalls,
            if stall { 5 } else { 0 }
        );
        assert_eq!(io.telemetry().messages_sent, 3);
    }
}
