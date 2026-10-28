//! Ordered lifecycle effects for native USB hosts.
//!
//! Protocol observations and timers are reduced here. Hosts pull one identified
//! batch, submit it on their serialized native writer, and report admission or
//! failure. Admission is deliberately not device acknowledgement or readback.
//! No host callback chooses the next startup phase or constructs seed traffic.

use crate::device::initialization::{
    DeviceLifecycleRuntime, DeviceStartupAction, DeviceStartupError, DeviceStartupPhase,
    InitializationAction,
};
use crate::{commands::OutboundMessage, profile};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

// Host resource policy. A complete boot burst is one batch. Repeated
// unsolicited requests cannot grow the queue indefinitely while the native
// writer is unavailable.
const MAX_QUEUED_BATCHES: usize = 64;
const MAX_QUEUED_BYTES: usize = profile::MAX_FRAME_BYTES;

/// Identifies one dispatched [`LifecycleBatch`]. It is bound to the session id
/// and revision it was issued under, so a callback from an older session or
/// rebuild cannot be mistaken for the current batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleTicket {
    session_id: String,
    revision: u64,
    sequence: u64,
}

/// Identity of one session generation: the reset session id plus the revision
/// that advances on each in-session rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEpoch {
    session_id: String,
    revision: u64,
}

impl SessionEpoch {
    /// How many in-session rebuilds this session id has been through.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One ordered group of messages for the host's writer to send.
pub struct LifecycleBatch {
    /// Ticket to present back to [`SessionDriver::is_current`] and
    /// [`SessionDriver::dispatched`].
    pub ticket: LifecycleTicket,
    /// Messages to write, in order.
    pub messages: Vec<OutboundMessage>,
}

/// Result of submitting a batch to the host's ordered writer. The Windows
/// native queue can accept work before HID completion, so Accepted cannot be
/// used as evidence that a command landed. Terminal asynchronous I/O failure
/// must subsequently call `transport_failed` and invalidate pending work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The writer accepted the batch.
    Accepted,
    /// The writer could not submit the batch; the session fails with
    /// [`SessionFailure::Dispatch`].
    Failed,
}

/// Why a [`SessionDriver`] stopped. The first failure is kept and the driver
/// does not recover from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionFailure {
    /// Startup rejected the session or device, for example because of a session
    /// or controller-version mismatch.
    Invalid(DeviceStartupError),
    /// A startup protocol step failed, for example an undecodable or
    /// unusable reply.
    Protocol(DeviceStartupError),
    /// Startup did not reach `Connected` within its time budget; carries the
    /// phase it was stuck in.
    StartupTimeout(DeviceStartupPhase),
    /// The host's writer reported a failed submission or a terminal transport
    /// failure.
    Dispatch,
    /// More work was queued than the driver's batch-count or byte limit allows.
    QueueLimit,
    /// A ticket sequence number or session revision counter would overflow.
    IdentityExhausted,
}

#[derive(Debug, Clone, Copy)]
enum AfterDispatch {
    None,
    BeginBuilding,
    ReleaseSeed,
}

#[derive(Debug)]
enum Work {
    Send(Vec<OutboundMessage>, AfterDispatch),
    StartSeed,
}

#[derive(Debug)]
struct PendingBatch {
    ticket: LifecycleTicket,
    byte_count: usize,
    after: AfterDispatch,
}

/// One connection's ordered startup and seed driver. The opaque reset session
/// identity must be fresh for every physical open/handshake attempt. In-session
/// rebuilds additionally invalidate tickets through a revision counter.
#[derive(Debug)]
pub struct SessionDriver {
    lifecycle: DeviceLifecycleRuntime,
    session_id: String,
    revision: u64,
    next_sequence: u64,
    work: VecDeque<Work>,
    pending: Option<PendingBatch>,
    retained_bytes: usize,
    seed_dispatch_pending: bool,
    synchronization: Option<bool>,
    synchronization_change: Option<bool>,
    failure: Option<SessionFailure>,
}

impl SessionDriver {
    /// Starts a session and queues its reset-comms handshake message as the
    /// first batch.
    ///
    /// `request_id` is the first correlation id of the lifecycle, `session_id`
    /// the fresh opaque id for this connection attempt, and `now_ms` the host's
    /// monotonic clock in milliseconds.
    pub fn start(request_id: u64, session_id: String, now_ms: u64) -> Self {
        let (lifecycle, reset) = DeviceLifecycleRuntime::start_at(request_id, &session_id, now_ms);
        let mut driver = Self {
            lifecycle,
            session_id,
            revision: 0,
            next_sequence: 1,
            work: VecDeque::new(),
            pending: None,
            retained_bytes: 0,
            seed_dispatch_pending: false,
            synchronization: None,
            synchronization_change: None,
            failure: None,
        };
        driver.enqueue(Work::Send(reset, AfterDispatch::None));
        driver
    }

    /// Current startup phase of the underlying lifecycle.
    pub fn phase(&self) -> DeviceStartupPhase {
        self.lifecycle.phase()
    }

    /// Device-announced rebuilds since startup last reached `Connected`.
    pub fn startup_rebuilds(&self) -> u32 {
        self.lifecycle.startup_rebuilds()
    }

    /// The current session id and revision.
    pub fn epoch(&self) -> SessionEpoch {
        SessionEpoch {
            session_id: self.session_id.clone(),
            revision: self.revision,
        }
    }

    /// Whether `epoch` is the current one and the session has not failed.
    pub fn matches_epoch(&self, epoch: &SessionEpoch) -> bool {
        self.failure.is_none() && &self.epoch() == epoch
    }

    /// Whether startup has reached `Connected` and the session has not failed.
    pub fn is_connected(&self) -> bool {
        self.failure.is_none() && self.lifecycle.is_connected()
    }

    /// Whether the session has not failed and startup has not been rejected
    /// (it stays active after `Connected` so it can answer later reads and
    /// rebuilds).
    pub fn is_active(&self) -> bool {
        self.failure.is_none() && self.lifecycle.is_active()
    }

    /// Whether the session is active and has moved past the session-id
    /// validation step.
    pub fn session_validated(&self) -> bool {
        self.is_active() && self.phase() != DeviceStartupPhase::SessionValidating
    }

    /// The failure that stopped the session, or `None` while it is healthy.
    pub fn failure(&self) -> Option<&SessionFailure> {
        self.failure.as_ref()
    }

    /// Failure text for native status and copied diagnostics. Only fixed labels,
    /// the startup phase and numeric type-35 metadata are added to the error code.
    /// Raw reply contents and decoder error strings are never included.
    pub fn failure_message(&self) -> Option<String> {
        let failure = self.failure.as_ref()?;
        let phase = self
            .lifecycle
            .failure_phase()
            .unwrap_or_else(|| match failure {
                SessionFailure::StartupTimeout(phase) => *phase,
                _ => self.phase(),
            });
        let mut message = format!("{failure:?}; startup phase={}", phase.as_str());
        if let Some(statistics) = self.lifecycle.module_statistics_failure() {
            use std::fmt::Write;
            let _ = write!(message, "; {statistics}");
        }
        Some(message)
    }

    /// The latest synchronization result: `None` before post-boot initialization
    /// finishes, then whether the device state is synchronized. A failure sets
    /// it to `Some(false)`; an in-session rebuild does so only if a result had
    /// already been reported.
    pub fn synchronization(&self) -> Option<bool> {
        self.synchronization
    }

    /// Takes the synchronization value that changed since the last call, or
    /// `None` if it has not changed. Lets a host forward each change once.
    pub fn take_synchronization_change(&mut self) -> Option<bool> {
        self.synchronization_change.take()
    }

    /// Reserves the next correlation id from the lifecycle's own sequence, so
    /// hosts never run a second counter that could reuse an in-flight id.
    pub fn reserve_request_id(&mut self) -> u64 {
        self.lifecycle.reserve_request_id()
    }

    /// Assigns a request id to an outgoing File-message mutation that lacks one.
    ///
    /// Returns the id that was stamped, or `Ok(None)` when the message is not a
    /// File message or its action does not carry a request id. An id already
    /// present is preserved. Errors if a File message payload cannot be decoded.
    pub fn stamp_file_mutation_request_id(
        &mut self,
        message: &mut OutboundMessage,
    ) -> Result<Option<u64>, prost::DecodeError> {
        self.lifecycle.stamp_file_mutation_request_id(message)
    }

    /// Feeds one inbound message to the startup lifecycle and queues whatever
    /// work it calls for.
    ///
    /// Does nothing once the session has failed. A message arriving after the
    /// startup deadline fails the session with
    /// [`SessionFailure::StartupTimeout`]. A connection message that forces a
    /// rebuild discards queued work, advances the epoch revision and revokes a
    /// synchronization result already reported.
    pub fn observe(&mut self, message_type: u16, payload: &[u8], now_ms: u64) {
        if self.failure.is_some() {
            return;
        }
        // A reply arriving after expiry cannot win a race against the host's
        // next maintenance tick and promote an already expired startup.
        if self.lifecycle.timed_out(now_ms) {
            self.fail(SessionFailure::StartupTimeout(self.phase()));
            return;
        }
        let action = self.lifecycle.observe(message_type, payload, now_ms);
        if message_type == profile::MESSAGE_TYPE_CONNECTION
            && matches!(action, DeviceStartupAction::SendThenBuild(_))
        {
            self.invalidate_work();
            let Some(revision) = self.revision.checked_add(1) else {
                self.fail(SessionFailure::IdentityExhausted);
                return;
            };
            self.revision = revision;
            // Initial handshake is not transport-ready. Only revoke a
            // synchronization outcome that the host has already received.
            if self.synchronization.is_some() {
                self.set_synchronization(false);
            }
        }
        self.apply(action);
    }

    /// Feeds the outcome of parsing the model catalog to the startup lifecycle.
    ///
    /// `Err(())` reports that the catalog could not be parsed. It only matters
    /// while startup is waiting on the catalog; an unusable or empty catalog
    /// fails the session. Subject to the same failure and timeout rules as
    /// [`SessionDriver::observe`].
    pub fn observe_model_catalog(
        &mut self,
        catalog: Result<&crate::state::ModelCatalog, ()>,
        now_ms: u64,
    ) {
        if self.failure.is_some() {
            return;
        }
        if self.lifecycle.timed_out(now_ms) {
            self.fail(SessionFailure::StartupTimeout(self.phase()));
            return;
        }
        let action = self.lifecycle.observe_model_catalog(catalog);
        self.apply(action);
    }

    /// Advances timers: fails the session if startup has timed out, and queues
    /// any post-boot initialization reads that are due, recording the
    /// synchronization result when initialization completes. Held back while the
    /// initial system time sync is still waiting to be submitted.
    pub fn tick(&mut self, now_ms: u64) {
        if self.failure.is_some() {
            return;
        }
        if self.lifecycle.timed_out(now_ms) {
            self.fail(SessionFailure::StartupTimeout(self.phase()));
            return;
        }
        // SystemTimeSync must be submitted before any seed read. In
        // particular, a maintenance timer cannot race the queued sync batch.
        if self.seed_dispatch_pending {
            return;
        }
        match self.lifecycle.advance_initialization(now_ms) {
            InitializationAction::Wait => {}
            InitializationAction::Send(messages) => {
                self.enqueue(Work::Send(messages, AfterDispatch::None));
            }
            InitializationAction::Complete { synchronized } => {
                self.set_synchronization(synchronized);
            }
        }
    }

    /// Pull work on the host's single writer lane. Clock samples come from the
    /// same monotonic origin as observe/tick; only SystemTimeSync uses UTC.
    pub fn next_batch(&mut self, now_ms: u64, unix_time_ms: u64) -> Option<LifecycleBatch> {
        self.tick(now_ms);
        if self.failure.is_some() || self.pending.is_some() {
            return None;
        }
        loop {
            match self.work.pop_front()? {
                Work::StartSeed => {
                    let message = match self
                        .lifecycle
                        .start_post_boot_initialization(now_ms, unix_time_ms)
                    {
                        Ok(message) => message,
                        Err(error) => {
                            self.fail(SessionFailure::Protocol(error));
                            return None;
                        }
                    };
                    self.seed_dispatch_pending = true;
                    // StartSeed is an ordered barrier, so its sync must precede
                    // work already behind it (for example a Version reply).
                    let bytes = message.payload.len();
                    if self.retained_bytes.saturating_add(bytes) > MAX_QUEUED_BYTES {
                        self.fail(SessionFailure::QueueLimit);
                        return None;
                    }
                    self.retained_bytes += bytes;
                    self.work
                        .push_front(Work::Send(vec![message], AfterDispatch::ReleaseSeed));
                }
                Work::Send(messages, after) => {
                    let Some(next) = self.next_sequence.checked_add(1) else {
                        self.fail(SessionFailure::IdentityExhausted);
                        return None;
                    };
                    let ticket = LifecycleTicket {
                        session_id: self.session_id.clone(),
                        revision: self.revision,
                        sequence: self.next_sequence,
                    };
                    self.next_sequence = next;
                    self.pending = Some(PendingBatch {
                        ticket: ticket.clone(),
                        byte_count: messages.iter().map(|message| message.payload.len()).sum(),
                        after,
                    });
                    return Some(LifecycleBatch { ticket, messages });
                }
            }
        }
    }

    /// Check before each message in a dequeued batch, including after a native
    /// write yields. A disconnect may have invalidated the rest of that batch.
    pub fn is_current(&self, ticket: &LifecycleTicket) -> bool {
        self.failure.is_none()
            && self
                .pending
                .as_ref()
                .is_some_and(|pending| &pending.ticket == ticket)
    }

    /// Returns false for a duplicate or old-session callback. It cannot consume
    /// another batch or advance a replacement lifecycle.
    pub fn dispatched(&mut self, ticket: &LifecycleTicket, outcome: DispatchOutcome) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        let pending = self
            .pending
            .take()
            .expect("current ticket has pending batch");
        self.retained_bytes -= pending.byte_count;
        if outcome == DispatchOutcome::Failed {
            self.fail(SessionFailure::Dispatch);
            return true;
        }
        match pending.after {
            AfterDispatch::None => {}
            AfterDispatch::BeginBuilding => {
                let action = self.lifecycle.begin_building();
                self.apply(action);
            }
            AfterDispatch::ReleaseSeed => self.seed_dispatch_pending = false,
        }
        true
    }

    /// Reports a terminal asynchronous I/O failure from the host's writer. Fails
    /// the session with [`SessionFailure::Dispatch`] and discards pending work.
    pub fn transport_failed(&mut self) {
        self.fail(SessionFailure::Dispatch);
    }

    fn apply(&mut self, action: DeviceStartupAction) {
        match action {
            DeviceStartupAction::Wait => {}
            DeviceStartupAction::Send(messages) => {
                self.enqueue(Work::Send(messages, AfterDispatch::None));
            }
            DeviceStartupAction::SendThenBuild(messages) => {
                self.enqueue(Work::Send(messages, AfterDispatch::BeginBuilding));
            }
            DeviceStartupAction::Connected => self.enqueue(Work::StartSeed),
            DeviceStartupAction::Invalid(error) => self.fail(SessionFailure::Invalid(error)),
            DeviceStartupAction::Failed(error) => self.fail(SessionFailure::Protocol(error)),
        }
    }

    fn enqueue(&mut self, work: Work) {
        let bytes = match &work {
            Work::Send(messages, _) => messages.iter().map(|message| message.payload.len()).sum(),
            Work::StartSeed => 0,
        };
        if self.work.len() + usize::from(self.pending.is_some()) >= MAX_QUEUED_BATCHES
            || self.retained_bytes.saturating_add(bytes) > MAX_QUEUED_BYTES
        {
            self.fail(SessionFailure::QueueLimit);
            return;
        }
        self.retained_bytes += bytes;
        self.work.push_back(work);
    }

    fn set_synchronization(&mut self, synchronized: bool) {
        if self.synchronization != Some(synchronized) {
            self.synchronization = Some(synchronized);
            self.synchronization_change = Some(synchronized);
        }
    }

    fn invalidate_work(&mut self) {
        self.work.clear();
        self.pending = None;
        self.retained_bytes = 0;
        self.seed_dispatch_pending = false;
    }

    fn fail(&mut self, failure: SessionFailure) {
        self.invalidate_work();
        self.lifecycle.abort_post_boot_initialization();
        self.set_synchronization(false);
        self.failure.get_or_insert(failure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::cortex_protobuf_v2 as pa;
    use prost::Message;

    fn reset_reply(session: &str) -> Vec<u8> {
        pa::ResetCommsBuffersMessage {
            session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                session.into(),
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn version() -> Vec<u8> {
        pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                pa::version_message::DeviceType::Qc as i32,
            )),
            cortex_control_version_valid: Some(
                pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
            ),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn model_repo() -> Vec<u8> {
        pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                br#"<Models><Category name="Drive"><Model id="1" name="Test" /></Category></Models>"#.to_vec(),
            )),
            ..Default::default()
        }.encode_to_vec()
    }

    fn submitted(
        driver: &mut SessionDriver,
        expected_types: &[u16],
        now_ms: u64,
    ) -> LifecycleBatch {
        let batch = driver
            .next_batch(now_ms, 1_000_000)
            .expect("expected lifecycle batch");
        assert_eq!(
            batch
                .messages
                .iter()
                .map(|message| message.message_type)
                .collect::<Vec<_>>(),
            expected_types
        );
        assert!(driver.dispatched(&batch.ticket, DispatchOutcome::Accepted));
        batch
    }

    fn boot_to_updater(driver: &mut SessionDriver) {
        submitted(driver, &[52], 0);
        driver.observe(52, &reset_reply("session"), 10);
        let read = submitted(driver, &[10], 10);
        assert_eq!(read.messages[0].payload, [8, 3]);
        driver.observe(10, &version(), 20);
        let disconnected = driver.next_batch(20, 1_000_000).unwrap();
        assert_eq!(disconnected.messages[0].message_type, 26);
        assert_eq!(disconnected.messages[0].payload, [8, 2, 16, 8]);
        assert_eq!(disconnected.messages[1].payload, [16, 0]);
        assert_eq!(driver.phase(), DeviceStartupPhase::Disconnected);
        // A repeated notice while Disconnected cannot duplicate the pending
        // transition, even while the native write is still returning.
        driver.observe(49, &[16, 0], 21);
        assert!(driver.dispatched(&disconnected.ticket, DispatchOutcome::Accepted));
        submitted(driver, &[49, 51], 22);
        driver.observe(51, &model_repo(), 30);
        submitted(driver, &[35], 30);
        let stats = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec();
        driver.observe(35, &stats, 40);
        // This is the required boot message order, asserted directly rather
        // than computed from our own boot-command builder.
        //
        // License (58), which normally reads second, is excluded: this build
        // cannot send that family at all since its trailer encryption is not
        // implemented, and asking anyway fails the entire burst at that
        // position, leaving the device unreachable. Every other entry keeps
        // its required position.
        submitted(
            driver,
            &[
                26, 21, 3, 9, 24, 14, 38, 17, 4, 71, 46, 20, 20, 42, 15, 50, 54, 19, 33, 2, 34, 13,
                57, 60,
            ],
            40,
        );
        driver.observe(
            60,
            &pa::UpdaterMessage {
                action: pa::message_action::Enum::Update as i32,
                ..Default::default()
            }
            .encode_to_vec(),
            50,
        );
        assert_eq!(driver.phase(), DeviceStartupPhase::Connected);
        assert_eq!(driver.synchronization(), None);
    }

    #[test]
    fn module_stats_diagnostics_survive_driver_failure_and_keep_writes_blocked() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        submitted(&mut driver, &[52], 0);
        driver.observe(52, &reset_reply("session"), 10);
        submitted(&mut driver, &[10], 10);
        driver.observe(10, &version(), 20);
        submitted(&mut driver, &[26, 49], 20);
        submitted(&mut driver, &[49, 51], 20);
        driver.observe(51, &model_repo(), 30);
        submitted(&mut driver, &[35], 30);
        driver.observe(35, &[8, 1], 40); // UPDATE with no statistics collections.
        assert_eq!(
            driver.failure(),
            Some(&SessionFailure::Protocol(
                DeviceStartupError::ModuleStatisticsDataError
            ))
        );
        assert_eq!(driver.failure_message().as_deref(), Some(
            "Protocol(ModuleStatisticsDataError); startup phase=initializing; type 35 empty-statistics; payloadBytes=2; action=1; stats=0, coreStats=0, sizeRelations=0, validId=0"));
        assert!(!driver.is_active());
        assert!(driver.next_batch(41, 0).is_none());
    }

    #[test]
    fn a_rebuild_loop_ends_the_session_instead_of_rearming_the_startup_deadline_for_ever() {
        // The wire pattern of 2026-10-03: per cycle the host writes 26/49
        // (disconnect) then 49/51 (build) and the device answers with another
        // type 49 and never with the Model Repo. Before the cap this repeated
        // for 13 minutes because every cycle re-armed READY_WAIT_TIMEOUT_MS.
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        submitted(&mut driver, &[52], 0);
        driver.observe(52, &reset_reply("session"), 10);
        submitted(&mut driver, &[10], 10);
        driver.observe(10, &version(), 20);
        let mut now = 20;
        let mut cycles = 0;
        loop {
            let disconnect = driver.next_batch(now, 0).expect("disconnect batch");
            assert_eq!(
                disconnect
                    .messages
                    .iter()
                    .map(|message| message.message_type)
                    .collect::<Vec<_>>(),
                [26, 49]
            );
            assert!(driver.dispatched(&disconnect.ticket, DispatchOutcome::Accepted));
            submitted(&mut driver, &[49, 51], now);
            cycles += 1;
            now += 1_100;
            driver.observe(49, &[16, 0], now);
            if driver.failure().is_some() {
                break;
            }
            assert!(cycles < 20, "the loop must be bounded");
        }
        assert_eq!(
            driver.failure(),
            Some(&SessionFailure::Protocol(DeviceStartupError::RebuildLoop))
        );
        assert_eq!(
            cycles,
            crate::device::initialization::MAX_STARTUP_REBUILDS as usize + 1
        );
        assert!(!driver.is_active());
        assert!(driver.next_batch(now, 0).is_none());
        // Far inside what the re-armed deadline would still have allowed.
        assert!(now < profile::READY_WAIT_TIMEOUT_MS);
    }

    #[test]
    fn rejected_metadata_revokes_queued_startup_work_without_a_timer_tick() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        submitted(&mut driver, &[52], 0);
        driver.observe(52, &reset_reply("session"), 10);
        submitted(&mut driver, &[10], 10);
        driver.observe(10, &version(), 20);
        submitted(&mut driver, &[26, 49], 20);
        let building = driver.next_batch(21, 0).unwrap();
        assert_eq!(driver.phase(), DeviceStartupPhase::Building);
        driver.observe_model_catalog(Err(()), 22);
        assert_eq!(
            driver.failure(),
            Some(&SessionFailure::Protocol(
                DeviceStartupError::ModelRepositoryDataError
            ))
        );
        assert!(!driver.dispatched(&building.ticket, DispatchOutcome::Accepted));
        assert!(driver.next_batch(22, 0).is_none());
        assert!(!driver.is_active());
    }

    #[test]
    fn response_before_dispatch_receipt_preserves_write_order() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        let reset = driver.next_batch(0, 0).unwrap();
        driver.observe(52, &reset_reply("session"), 10);
        assert!(driver.next_batch(10, 0).is_none());
        assert!(driver.dispatched(&reset.ticket, DispatchOutcome::Accepted));
        assert!(!driver.dispatched(&reset.ticket, DispatchOutcome::Accepted));
        submitted(&mut driver, &[10], 11);
        assert!(driver.next_batch(11, 0).is_none());
    }

    #[test]
    fn binary_startup_order_precedes_seed_and_admission_is_not_ready() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        boot_to_updater(&mut driver);
        let sync = driver.next_batch(50, 1_000_000).unwrap();
        assert_eq!(sync.messages.len(), 1);
        assert_eq!(
            sync.messages[0].message_type,
            profile::MESSAGE_TYPE_SYSTEM_TIME_SYNC
        );
        driver.tick(5_000);
        assert!(driver.next_batch(5_000, 1_005_000).is_none());
        assert_eq!(driver.synchronization(), None);
        assert!(driver.dispatched(&sync.ticket, DispatchOutcome::Accepted));
        submitted(&mut driver, &[15], 5_000);
        assert_eq!(driver.synchronization(), None);
    }

    #[test]
    fn rebuild_invalidates_old_seed_batch_and_delayed_receipt() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        boot_to_updater(&mut driver);
        let old = driver.next_batch(50, 1_000_000).unwrap();
        let old_epoch = driver.epoch();
        let rebuild_at = profile::READY_WAIT_TIMEOUT_MS * 2;
        driver.observe(49, &[16, 0], rebuild_at);
        assert!(!driver.matches_epoch(&old_epoch));
        assert!(driver.matches_epoch(&driver.epoch()));
        assert!(!driver.is_current(&old.ticket));
        let new = driver.next_batch(rebuild_at, 2_000_000).unwrap();
        assert_eq!(
            new.messages
                .iter()
                .map(|message| message.message_type)
                .collect::<Vec<_>>(),
            [26, 49]
        );
        assert!(!driver.dispatched(&old.ticket, DispatchOutcome::Failed));
        assert!(driver.is_current(&new.ticket));
        assert!(driver.failure().is_none());
        assert!(driver.dispatched(&new.ticket, DispatchOutcome::Accepted));
        submitted(&mut driver, &[49, 51], rebuild_at);
        assert_eq!(driver.phase(), DeviceStartupPhase::Building);
    }

    #[test]
    fn old_connection_receipt_cannot_advance_replacement_connection() {
        let mut old = SessionDriver::start(7, "old".into(), 0);
        let old_batch = old.next_batch(0, 0).unwrap();
        let mut new = SessionDriver::start(7, "new".into(), 0);
        let new_batch = new.next_batch(0, 0).unwrap();
        let serialized = serde_json::to_string(&old.epoch()).unwrap();
        let old_epoch: SessionEpoch = serde_json::from_str(&serialized).unwrap();
        assert!(!new.matches_epoch(&old_epoch));
        assert!(old.matches_epoch(&old_epoch));
        assert!(!new.dispatched(&old_batch.ticket, DispatchOutcome::Accepted));
        assert!(new.is_current(&new_batch.ticket));
        assert_eq!(new.phase(), DeviceStartupPhase::SessionValidating);
    }

    #[test]
    fn failed_submission_is_terminal_and_never_replayed() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        let reset = driver.next_batch(0, 0).unwrap();
        driver.observe(52, &reset_reply("session"), 10);
        assert!(driver.dispatched(&reset.ticket, DispatchOutcome::Failed));
        assert_eq!(driver.failure(), Some(&SessionFailure::Dispatch));
        driver.observe(10, &version(), 11);
        assert!(driver.next_batch(11, 0).is_none());
        assert_eq!(driver.take_synchronization_change(), Some(false));
    }

    #[test]
    fn unsolicited_requests_cannot_grow_a_stalled_writer_queue_without_bound() {
        let mut driver = SessionDriver::start(7, "session".into(), 0);
        let pending = driver.next_batch(0, 0).unwrap();
        driver.observe(52, &reset_reply("session"), 1);
        for _ in 0..MAX_QUEUED_BATCHES + 1 {
            driver.observe(10, &[8, 3], 2);
        }
        assert_eq!(driver.failure(), Some(&SessionFailure::QueueLimit));
        assert!(!driver.is_current(&pending.ticket));
        assert!(driver.next_batch(2, 0).is_none());
        assert_eq!(driver.retained_bytes, 0);
    }

    #[test]
    fn startup_deadline_revokes_an_issued_batch() {
        let mut driver = SessionDriver::start(7, "session".into(), 100);
        let pending = driver.next_batch(100, 0).unwrap();
        driver.tick(100 + profile::READY_WAIT_TIMEOUT_MS);
        assert_eq!(
            driver.failure(),
            Some(&SessionFailure::StartupTimeout(
                DeviceStartupPhase::SessionValidating
            ))
        );
        assert!(!driver.dispatched(&pending.ticket, DispatchOutcome::Accepted));
    }

    #[test]
    fn late_reply_cannot_rescue_startup_before_the_next_host_tick() {
        let mut driver = SessionDriver::start(7, "session".into(), 100);
        let reset = driver.next_batch(100, 0).unwrap();
        driver.observe(
            52,
            &reset_reply("session"),
            100 + profile::READY_WAIT_TIMEOUT_MS,
        );
        assert!(!driver.session_validated());
        assert!(matches!(
            driver.failure(),
            Some(SessionFailure::StartupTimeout(_))
        ));
        assert!(!driver.dispatched(&reset.ticket, DispatchOutcome::Accepted));
        assert!(driver
            .next_batch(101 + profile::READY_WAIT_TIMEOUT_MS, 0)
            .is_none());
    }
}
