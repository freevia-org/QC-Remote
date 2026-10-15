//! Platform-neutral QC transport engine.
//!
//! Native hosts own USB handles and move raw reports. This type owns the
//! device-facing session policy and HID framing so Android and Windows cannot
//! make different decisions about report layout, handshake cadence, link
//! readiness, keepalives, or frame recovery.

use crate::commands::{self, OutboundMessage};
use crate::framing::{self, FrameAssembler, FrameDiscard, FrameError};
use crate::inbound_telemetry::InboundTelemetry;
use crate::profile;
use crate::proto::cortex_protobuf_v2 as pa;
use prost::Message;
#[cfg(feature = "cloud-forward")]
use zeroize::Zeroize;

/// Phase of the connection a [`TransportRuntime`] is managing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// No device is open and automatic reconnection is off.
    Disconnected,
    /// No device is open; the host retries the connection when it falls due.
    Searching,
    /// The transport is open and the reset-comms handshake is in progress.
    Handshaking,
    /// The handshake completed but the device state is not yet synchronized.
    Syncing,
    /// The handshake completed and the device state is synchronized.
    Ready,
}

#[derive(Debug)]
struct SessionMachine {
    phase: SessionPhase,
    auto_reconnect: bool,
    next_reconnect_at_ms: u64,
    handshake_started_at_ms: Option<u64>,
    handshake_attempts: u32,
    next_handshake_at_ms: u64,
    next_keepalive_at_ms: u64,
    consecutive_read_errors: u8,
    synchronized: bool,
    /// Connect attempts in a row that opened the device and then failed
    /// (handshake or startup timeout, rebuild loop, protocol error). Drives
    /// the retry backoff; cleared by a completed connect or an explicit reset.
    connect_failures: u32,
}

/// Longest wait between connect attempts once they keep failing. Host policy
/// (not a Cortex Control frame; Cortex Control loops at 200 ms): the 750 ms
/// base and this 10 s cap are BELIEVED, no measurement in the repository.
pub const RECONNECT_BACKOFF_CAP_MS: u64 = 10_000;

/// Delay before the next connect attempt after `failures` consecutive failed
/// ones: `RECONNECT_INTERVAL_MS` doubling per failure, capped at
/// [`RECONNECT_BACKOFF_CAP_MS`]. A unit that cannot complete startup gets
/// breathing room to settle instead of a fresh handshake every 750 ms.
pub fn reconnect_backoff_ms(failures: u32) -> u64 {
    let doubling = failures.saturating_sub(1).min(8);
    profile::RECONNECT_INTERVAL_MS
        .saturating_mul(1_u64 << doubling)
        .min(RECONNECT_BACKOFF_CAP_MS)
}

impl SessionMachine {
    fn new(now_ms: u64) -> Self {
        Self {
            phase: SessionPhase::Searching,
            auto_reconnect: true,
            next_reconnect_at_ms: now_ms,
            handshake_started_at_ms: None,
            handshake_attempts: 0,
            next_handshake_at_ms: now_ms,
            next_keepalive_at_ms: now_ms.saturating_add(profile::KEEPALIVE_INTERVAL_MS),
            consecutive_read_errors: 0,
            synchronized: false,
            connect_failures: 0,
        }
    }

    fn reset_connection_state(&mut self, now_ms: u64) {
        self.handshake_started_at_ms = None;
        self.handshake_attempts = 0;
        self.next_handshake_at_ms = now_ms;
        self.next_keepalive_at_ms = now_ms.saturating_add(profile::KEEPALIVE_INTERVAL_MS);
        self.consecutive_read_errors = 0;
        self.synchronized = false;
    }

    fn phase(&self) -> SessionPhase {
        self.phase
    }

    fn synchronized(&self) -> bool {
        self.synchronized
    }

    fn is_connected(&self) -> bool {
        matches!(self.phase, SessionPhase::Syncing | SessionPhase::Ready)
    }

    fn request_reconnect(&mut self, now_ms: u64) {
        // An explicit reset is the operator asking for a fresh try now.
        self.connect_failures = 0;
        self.auto_reconnect = true;
        self.phase = SessionPhase::Searching;
        self.next_reconnect_at_ms = now_ms;
        self.reset_connection_state(now_ms);
    }

    fn disconnect(&mut self, now_ms: u64, recover: bool) {
        self.auto_reconnect = recover;
        self.phase = if recover {
            SessionPhase::Searching
        } else {
            SessionPhase::Disconnected
        };
        self.next_reconnect_at_ms = now_ms.saturating_add(profile::RECONNECT_INTERVAL_MS);
        self.reset_connection_state(now_ms);
    }

    fn reconnect_due(&self, now_ms: u64) -> bool {
        self.auto_reconnect
            && self.phase == SessionPhase::Searching
            && now_ms >= self.next_reconnect_at_ms
    }

    /// A connect opened the device but did not complete. Schedules the next
    /// attempt with backoff; call after `disconnect(.., true)`, which sets the
    /// base interval this replaces.
    fn connect_failed(&mut self, now_ms: u64) {
        self.connect_failures = self.connect_failures.saturating_add(1);
        self.next_reconnect_at_ms =
            now_ms.saturating_add(reconnect_backoff_ms(self.connect_failures));
    }

    fn connect_succeeded(&mut self) {
        self.connect_failures = 0;
    }

    fn reconnect_attempted(&mut self, now_ms: u64) {
        self.next_reconnect_at_ms = now_ms.saturating_add(profile::RECONNECT_INTERVAL_MS);
    }

    fn transport_opened(&mut self, now_ms: u64) {
        self.phase = SessionPhase::Handshaking;
        self.handshake_started_at_ms = Some(now_ms);
        self.handshake_attempts = 0;
        self.next_handshake_at_ms = now_ms;
        self.synchronized = false;
        self.consecutive_read_errors = 0;
    }

    fn next_handshake_attempt(&mut self, now_ms: u64) -> Option<(u32, bool)> {
        if self.phase != SessionPhase::Handshaking
            || now_ms < self.next_handshake_at_ms
            || self.handshake_timed_out(now_ms)
        {
            return None;
        }
        self.handshake_attempts = self.handshake_attempts.saturating_add(1);
        self.next_handshake_at_ms = now_ms.saturating_add(profile::HANDSHAKE_ATTEMPT_TIMEOUT_MS);
        Some((self.handshake_attempts, self.handshake_attempts % 2 == 1))
    }

    fn handshake_timed_out(&self, now_ms: u64) -> bool {
        self.phase == SessionPhase::Handshaking
            && self.handshake_started_at_ms.is_some_and(|started| {
                now_ms.saturating_sub(started) >= profile::HANDSHAKE_TIMEOUT_MS
            })
    }

    fn awaiting_handshake_reply(&self, now_ms: u64) -> bool {
        self.phase == SessionPhase::Handshaking
            && now_ms < self.next_handshake_at_ms
            && !self.handshake_timed_out(now_ms)
    }

    fn handshake_completed(&mut self, _now_ms: u64, synchronized: bool) {
        self.phase = if synchronized {
            SessionPhase::Ready
        } else {
            SessionPhase::Syncing
        };
        self.synchronized = synchronized;
        self.consecutive_read_errors = 0;
    }

    fn state_observed(&mut self, _now_ms: u64, preset_synchronized: bool) {
        self.consecutive_read_errors = 0;
        if matches!(self.phase, SessionPhase::Syncing | SessionPhase::Ready) {
            self.synchronized = preset_synchronized;
            self.phase = if preset_synchronized {
                SessionPhase::Ready
            } else {
                SessionPhase::Syncing
            };
        }
    }

    fn keepalive_due(&self, now_ms: u64) -> bool {
        matches!(self.phase, SessionPhase::Syncing | SessionPhase::Ready)
            && now_ms >= self.next_keepalive_at_ms
    }

    fn keepalive_sent(&mut self, now_ms: u64) {
        self.next_keepalive_at_ms = now_ms.saturating_add(profile::KEEPALIVE_INTERVAL_MS);
    }

    fn defer_keepalive(&mut self, now_ms: u64) {
        self.next_keepalive_at_ms = now_ms.saturating_add(profile::KEEPALIVE_INTERVAL_MS);
    }

    fn read_succeeded(&mut self) {
        self.consecutive_read_errors = 0;
    }

    fn read_failed(&mut self) -> bool {
        self.consecutive_read_errors = self.consecutive_read_errors.saturating_add(1);
        self.consecutive_read_errors >= 2
    }

    fn terminal_read_failed(&mut self) -> bool {
        self.consecutive_read_errors = 2;
        true
    }
}

/// Layout expected by a host USB write API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportLayout {
    /// The native API accepts the complete 129-byte logical HID report.
    ReportIdPrefixed,
    /// The native API accepts only the 128-byte SET_REPORT data stage.
    BodyOnly,
}

/// A fully assembled QC message and transport diagnostics for its source frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundFrame {
    /// Protocol message type from the frame envelope.
    pub message_type: u16,
    /// Message payload, already inflated when the frame was compressed.
    pub payload: Vec<u8>,
    /// Number of HID reports the frame arrived in (at least 1).
    pub report_count: usize,
    /// Read timestamps of the frame's first and last report. `None` unless the
    /// reports were pushed with timestamps.
    pub ingress: Option<IngressTime>,
    /// The envelope's last two trailer bytes, carried through rather than
    /// discarded. The host writes zero in both and nothing on this path reads
    /// their value; they travel so that a host can tally what the device puts
    /// there. See [`crate::inbound_telemetry`].
    pub device_bytes: [u8; 2],
}

/// Monotonic native-read completions, sampled before host queueing or decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngressTime {
    /// Monotonic time, in milliseconds, at which the frame's first report was read.
    pub first_report_at_ms: u64,
    /// Monotonic time, in milliseconds, at which the frame's last report was read.
    pub last_report_at_ms: u64,
}

/// A handshake write selected by the shared session policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeWrite {
    /// One-based handshake attempt number within the current connection.
    pub attempt: u32,
    /// HID report layout the host should use to write this attempt.
    pub layout: ReportLayout,
    /// The reset-comms message to send.
    pub message: OutboundMessage,
    request_id: u64,
    session_id: String,
}

impl HandshakeWrite {
    /// Request id carried by the handshake message; equal to the attempt number.
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Opaque session id, supplied by the host, that a reply must echo.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// A type-52 frame is not itself a successful handshake: the opaque
    /// session id must match the attempt. The request id is optional on
    /// replies and is not the session gate.
    pub fn matches_reply(&self, payload: &[u8]) -> bool {
        let Ok(reply) = pa::ResetCommsBuffersMessage::decode(payload) else {
            return false;
        };
        let session_id = reply.session_id.map(|value| match value {
            pa::reset_comms_buffers_message::SessionId::SessionId(value) => value,
        });
        session_id.as_deref() == Some(&self.session_id)
    }
}

/// Long-lived protocol state shared by every native USB host.
#[derive(Debug)]
pub struct TransportRuntime {
    session: SessionMachine,
    frames: FrameAssembler,
    frame_started_at_ms: Option<u64>,
    read_epoch: Option<u64>,
    inbound: InboundTelemetry,
    #[cfg(feature = "cloud-forward")]
    zencryption: crate::device_zencryption::ZencryptionState,
    #[cfg(feature = "cloud-forward")]
    zencryption_boundary_seen: bool,
    #[cfg(feature = "cloud-forward")]
    zencryption_fragment_seen: bool,
    #[cfg(feature = "cloud-forward")]
    last_zencryption_fragment_at_ms: Option<u64>,
    #[cfg(feature = "cloud-forward")]
    cloud_forward_enabled: bool,
}

impl TransportRuntime {
    /// Creates a runtime in [`SessionPhase::Searching`] with a connection
    /// attempt due immediately. `now_ms` is the host's monotonic clock in
    /// milliseconds; every later `now_ms` argument uses the same clock.
    pub fn new(now_ms: u64) -> Self {
        Self {
            session: SessionMachine::new(now_ms),
            frames: FrameAssembler::new(),
            frame_started_at_ms: None,
            read_epoch: None,
            inbound: InboundTelemetry::default(),
            #[cfg(feature = "cloud-forward")]
            zencryption: crate::device_zencryption::ZencryptionState::default(),
            #[cfg(feature = "cloud-forward")]
            zencryption_boundary_seen: true,
            #[cfg(feature = "cloud-forward")]
            zencryption_fragment_seen: false,
            #[cfg(feature = "cloud-forward")]
            last_zencryption_fragment_at_ms: None,
            #[cfg(feature = "cloud-forward")]
            cloud_forward_enabled: false,
        }
    }

    /// What this connection received and could not act on. Merged with the
    /// state decoder's half by [`crate::device::owner::SessionRuntime`].
    pub fn inbound_telemetry(&self) -> &InboundTelemetry {
        &self.inbound
    }

    /// Enables encrypted account forwarding only while the broker's
    /// existing internet-relay setting is on.
    #[cfg(feature = "cloud-forward")]
    pub fn set_cloud_forward_enabled(&mut self, enabled: bool) {
        self.cloud_forward_enabled = enabled;
    }

    #[cfg(feature = "cloud-forward")]
    fn clear_zencryption_session(&mut self) {
        self.zencryption.clear();
        self.zencryption_boundary_seen = true;
        self.zencryption_fragment_seen = false;
        self.last_zencryption_fragment_at_ms = None;
    }

    #[cfg(feature = "cloud-forward")]
    fn begin_zencryption_fragment(&mut self, received_at_ms: Option<u64>) {
        // Captures and live transports can carry more than one Version
        // handshake without replacing the TransportRuntime. Cortex Control
        // starts a fresh secret after Connection/ResetCommsBuffers, and the
        // transport also identifies a fresh handshake after a
        // >2 s gap between secret fragments. Fragments within one handshake
        // arrive within milliseconds and must continue to append.
        let gap_starts_session = self
            .last_zencryption_fragment_at_ms
            .zip(received_at_ms)
            .is_some_and(|(previous, current)| current.saturating_sub(previous) > 2_000);
        if self.zencryption_boundary_seen || !self.zencryption_fragment_seen || gap_starts_session {
            self.zencryption.clear();
        }
        self.zencryption_boundary_seen = false;
        self.zencryption_fragment_seen = true;
        self.last_zencryption_fragment_at_ms = received_at_ms;
    }

    /// Discards all session, framing and telemetry state and starts over as
    /// [`TransportRuntime::new`] would.
    pub fn reset(&mut self, now_ms: u64) {
        *self = Self::new(now_ms);
    }

    /// Current connection phase.
    pub fn phase(&self) -> SessionPhase {
        self.session.phase()
    }

    /// Whether the device state is synchronized (the phase is
    /// [`SessionPhase::Ready`]).
    pub fn synchronized(&self) -> bool {
        self.session.synchronized()
    }

    /// Whether the handshake has completed, that is, the phase is
    /// [`SessionPhase::Syncing`] or [`SessionPhase::Ready`].
    pub fn is_connected(&self) -> bool {
        self.session.is_connected()
    }

    /// Operator-requested fresh connection attempt. Drops any partially
    /// assembled frame and the inbound tally, clears the connect-failure
    /// backoff, and returns to [`SessionPhase::Searching`] with an attempt due
    /// immediately.
    pub fn request_reconnect(&mut self, now_ms: u64) {
        self.frames.reset();
        self.frame_started_at_ms = None;
        // A receive tally must never straddle two connections, so this one
        // ends where the others do.
        self.inbound = InboundTelemetry::default();
        #[cfg(feature = "cloud-forward")]
        self.clear_zencryption_session();
        self.session.request_reconnect(now_ms);
    }

    /// Ends the current connection and drops any partially assembled frame
    /// and the inbound tally.
    ///
    /// With `recover` set the phase becomes [`SessionPhase::Searching`] and the
    /// next attempt falls due one reconnect interval later; otherwise it becomes
    /// [`SessionPhase::Disconnected`] and no automatic reconnect is scheduled.
    pub fn disconnect(&mut self, now_ms: u64, recover: bool) {
        self.frames.reset();
        self.frame_started_at_ms = None;
        self.inbound = InboundTelemetry::default();
        #[cfg(feature = "cloud-forward")]
        self.clear_zencryption_session();
        self.session.disconnect(now_ms, recover);
    }

    /// Whether a connection attempt should be made now: automatic reconnection
    /// is enabled, the phase is [`SessionPhase::Searching`], and the scheduled
    /// time has been reached.
    pub fn reconnect_due(&self, now_ms: u64) -> bool {
        self.session.reconnect_due(now_ms)
    }

    /// Records that a connection attempt was made, deferring the next one by
    /// one reconnect interval.
    pub fn reconnect_attempted(&mut self, now_ms: u64) {
        self.session.reconnect_attempted(now_ms);
    }

    /// A connect opened the device but did not complete. Schedules the next
    /// attempt with backoff; call after `disconnect(.., true)`, which sets the
    /// base interval this replaces.
    pub fn connect_failed(&mut self, now_ms: u64) {
        self.session.connect_failed(now_ms);
    }

    /// Records a completed connect, clearing the consecutive-failure count that
    /// drives the retry backoff.
    pub fn connect_succeeded(&mut self) {
        self.session.connect_succeeded();
    }

    /// Consecutive connect attempts that opened the device and failed.
    pub fn connect_failures(&self) -> u32 {
        self.session.connect_failures
    }

    /// Enter recoverable link-loss state and return the delay until the first
    /// reconnect attempt. Hosts schedule I/O; the shared runtime owns cadence.
    pub fn schedule_reconnect(&mut self, now_ms: u64) -> u64 {
        self.disconnect(now_ms, true);
        profile::RECONNECT_INTERVAL_MS
    }

    /// Records that the native transport was opened. Enters
    /// [`SessionPhase::Handshaking`], starts the handshake clock and drops any
    /// partially assembled frame.
    pub fn transport_opened(&mut self, now_ms: u64) {
        self.frames.reset();
        self.frame_started_at_ms = None;
        #[cfg(feature = "cloud-forward")]
        self.clear_zencryption_session();
        self.session.transport_opened(now_ms);
    }

    /// Select and reserve the next handshake attempt, including the HID report
    /// layout probe. The host supplies entropy for the opaque session id but
    /// does not select or encode the protocol command.
    ///
    /// When no caller-supplied layout is available, this alternates by
    /// attempt parity between the only two candidate `ReportLayout`s. That
    /// alternation is an intentional, accepted probing heuristic -- the same
    /// explicit adapter policy documented on `next_handshake_write_with_layout`
    /// below -- not a bug or
    /// a stand-in for a real selection rule; there is no third layout to
    /// choose between and no better signal available before the device
    /// responds.
    pub fn next_handshake_write(
        &mut self,
        now_ms: u64,
        session_id: impl Into<String>,
    ) -> Option<HandshakeWrite> {
        let attempt = self.session.handshake_attempts.saturating_add(1);
        let layout = if attempt % 2 == 1 {
            ReportLayout::ReportIdPrefixed
        } else {
            ReportLayout::BodyOnly
        };
        self.next_handshake_write_with_layout(now_ms, session_id, layout)
    }

    /// Select the next handshake attempt with a host-defined native report
    /// layout. The Windows HID path always supplies the complete 129-byte
    /// report; Android's SET_REPORT compatibility probe remains an explicit
    /// adapter policy rather than an assumed protocol rule.
    pub fn next_handshake_write_with_layout(
        &mut self,
        now_ms: u64,
        session_id: impl Into<String>,
        layout: ReportLayout,
    ) -> Option<HandshakeWrite> {
        let session_id = session_id.into();
        self.session
            .next_handshake_attempt(now_ms)
            .map(|(attempt, _)| HandshakeWrite {
                attempt,
                layout,
                message: commands::reset_comms(u64::from(attempt), session_id.clone()),
                request_id: u64::from(attempt),
                session_id,
            })
    }

    /// Whether the overall handshake budget has elapsed since the transport
    /// was opened while still [`SessionPhase::Handshaking`].
    pub fn handshake_timed_out(&self, now_ms: u64) -> bool {
        self.session.handshake_timed_out(now_ms)
    }

    /// Whether a handshake attempt has been sent and its reply window is still
    /// open, so no further attempt is due and the handshake has not timed out.
    pub fn awaiting_handshake_reply(&self, now_ms: u64) -> bool {
        self.session.awaiting_handshake_reply(now_ms)
    }

    /// Marks the handshake complete: the phase becomes
    /// [`SessionPhase::Ready`] if `synchronized`, otherwise
    /// [`SessionPhase::Syncing`].
    pub fn handshake_completed(&mut self, now_ms: u64, synchronized: bool) {
        self.session.handshake_completed(now_ms, synchronized);
    }

    /// Apply an authoritative initialization result without asking adapters to
    /// interpret the current transport phase. The first result completes the
    /// handshake; late or rebuilt results update an established session.
    pub fn synchronization_completed(&mut self, now_ms: u64, synchronized: bool) {
        match self.phase() {
            SessionPhase::Handshaking => self.handshake_completed(now_ms, synchronized),
            SessionPhase::Syncing | SessionPhase::Ready => {
                self.state_observed(now_ms, synchronized)
            }
            SessionPhase::Disconnected | SessionPhase::Searching => {}
        }
    }

    fn state_observed(&mut self, now_ms: u64, preset_synchronized: bool) {
        self.session.state_observed(now_ms, preset_synchronized);
    }

    /// Return the one canonical maintenance write when it is due and reserve
    /// the next deadline immediately so repeated host polling cannot duplicate it.
    pub fn take_keepalive(&mut self, now_ms: u64) -> Option<OutboundMessage> {
        if !self.keepalive_due(now_ms) {
            return None;
        }
        self.keepalive_sent(now_ms);
        Some(commands::keepalive())
    }

    /// Split-phase form for adapters whose write must be queued on an OS-owned
    /// thread. The adapter checks the deadline, performs the write, then marks
    /// it sent only after the report has been handed to the native API.
    pub fn keepalive_due(&self, now_ms: u64) -> bool {
        self.session.keepalive_due(now_ms)
    }

    /// Records that a keepalive was handed to the native API and schedules the
    /// next one a keepalive interval later.
    pub fn keepalive_sent(&mut self, now_ms: u64) {
        self.session.keepalive_sent(now_ms);
    }

    /// Pushes the next keepalive deadline out by one keepalive interval without
    /// sending one.
    pub fn defer_keepalive(&mut self, now_ms: u64) {
        self.session.defer_keepalive(now_ms);
    }

    /// Records a successful native read, clearing the consecutive-error count.
    pub fn read_succeeded(&mut self) {
        self.session.read_succeeded();
    }

    /// Records a failed native read. Returns `true` once two reads in a row
    /// have failed, meaning the host should treat the link as lost.
    pub fn read_failed(&mut self) -> bool {
        self.session.read_failed()
    }

    /// Report an OS-level reader failure after which the native endpoint can
    /// no longer continue. Unlike a transient HID read error, a closed request
    /// queue does not benefit from retrying on the same handle.
    pub fn terminal_read_failed(&mut self) -> bool {
        self.session.terminal_read_failed()
    }

    /// Encode an outbound message exactly once, then adapt only its outer HID
    /// shape to the native USB API.
    pub fn encode_reports(
        message: &OutboundMessage,
        layout: ReportLayout,
    ) -> Result<Vec<Vec<u8>>, FrameError> {
        // The legacy stateless encoder is retained for existing public
        // unencrypted operations. Never leak a sensitive-family payload in
        // clear text; those callers must use `encode_reports_with_session`.
        if is_unsupported_encrypted_message_type(message.message_type) {
            return Err(FrameError::UnsupportedEncryptedMessageType);
        }
        reject_oversized_outbound(message)?;
        Ok(framing::encode(message.message_type, &message.payload)
            .into_iter()
            .map(|report| match layout {
                ReportLayout::ReportIdPrefixed => report.to_vec(),
                ReportLayout::BodyOnly => report[1..].to_vec(),
            })
            .collect())
    }

    /// Encode a message for the current connection. CloudLogin and
    /// ProductForward use the session's encrypted trailer only while the
    /// gated cloud-forward path is enabled. Other cloud/backup/license message
    /// families remain rejected rather than sent unencrypted or malformed.
    pub fn encode_reports_with_session(
        &self,
        message: &OutboundMessage,
        layout: ReportLayout,
    ) -> Result<Vec<Vec<u8>>, FrameError> {
        reject_oversized_outbound(message)?;
        #[cfg(feature = "cloud-forward")]
        if matches!(
            message.message_type,
            profile::MESSAGE_TYPE_CLOUD_LOGIN | profile::MESSAGE_TYPE_PRODUCT_FORWARD
        ) && self.cloud_forward_enabled
        {
            use zeroize::Zeroizing;
            let mut encrypted = Zeroizing::new(
                self.zencryption
                    .transform(&message.payload)
                    .map_err(|_| FrameError::UnsupportedEncryptedMessageType)?,
            );
            let reports = framing::encode_frame(&framing::Frame {
                message_type: u32::from(message.message_type),
                payload: std::mem::take(&mut *encrypted),
                encrypted: true,
                compressed: false,
                device_bytes: [0, 0],
            });

            return Ok(reports
                .into_iter()
                .map(|report| match layout {
                    ReportLayout::ReportIdPrefixed => report.to_vec(),
                    ReportLayout::BodyOnly => report[1..].to_vec(),
                })
                .collect());
        }
        if is_unsupported_encrypted_message_type(message.message_type) {
            return Err(FrameError::UnsupportedEncryptedMessageType);
        }
        let reports = framing::encode_frame(&framing::Frame {
            message_type: u32::from(message.message_type),
            payload: message.payload.clone(),
            encrypted: false,
            compressed: false,
            device_bytes: [0, 0],
        });
        Ok(reports
            .into_iter()
            .map(|report| match layout {
                ReportLayout::ReportIdPrefixed => report.to_vec(),
                ReportLayout::BodyOnly => report[1..].to_vec(),
            })
            .collect())
    }

    /// Required pacing: yield for six milliseconds after each group of ten
    /// HID reports, including a final full group. Returning the delay keeps
    /// both native adapters on one pacing rule without putting OS sleeps into
    /// the protocol crate.
    pub fn report_batch_delay_ms(report_index_one_based: usize) -> Option<u64> {
        (report_index_one_based > 0
            && report_index_one_based.is_multiple_of(profile::HID_REPORT_BATCH_SIZE))
        .then_some(profile::HID_REPORT_BATCH_DELAY_MS)
    }

    /// Normalize either native read layout and assemble one logical message.
    pub fn push_report(&mut self, raw_report: &[u8]) -> Result<Option<InboundFrame>, FrameError> {
        self.push_report_with_time(raw_report, None)
    }

    /// Like [`TransportRuntime::push_report`], but also records
    /// `received_at_ms` (the host's monotonic read time) so the returned
    /// frame carries [`IngressTime`] data.
    pub fn push_report_at(
        &mut self,
        raw_report: &[u8],
        received_at_ms: u64,
    ) -> Result<Option<InboundFrame>, FrameError> {
        self.push_report_with_time(raw_report, Some(received_at_ms))
    }

    /// Native ingress epochs delimit byte streams, including partial frames.
    /// A continuation received after an epoch change cannot start a new frame.
    pub fn push_report_in_epoch(
        &mut self,
        raw_report: &[u8],
        received_at_ms: u64,
        epoch: u64,
    ) -> Result<Option<InboundFrame>, FrameError> {
        if self.read_epoch != Some(epoch) {
            // After the reset the assembler ignores continuations until the
            // next FIRST report, and counts each one it ignores; a message the
            // epoch change cut off is counted here.
            let pending = self.frames.pending_reports();
            if pending > 0 {
                self.inbound
                    .record_abandoned_partial(pending, "the native ingress epoch changed");
            }
            self.frames.reset();
            self.frame_started_at_ms = None;
            self.read_epoch = Some(epoch);
            #[cfg(feature = "cloud-forward")]
            self.clear_zencryption_session();
        }
        self.push_report_at(raw_report, received_at_ms)
    }

    fn push_report_with_time(
        &mut self,
        raw_report: &[u8],
        received_at_ms: Option<u64>,
    ) -> Result<Option<InboundFrame>, FrameError> {
        let report = match normalize_inbound_report(raw_report) {
            Ok(report) => report,
            Err(error) => {
                self.frames.reset();
                self.frame_started_at_ms = None;
                self.inbound.record_framing_error(&error.to_string());
                return Err(error);
            }
        };
        let first = report[2] & framing::FLAG_FIRST != 0;
        // The reports this message will have arrived in if this one completes
        // it; the assembler is the one record of how many are already open.
        let report_count = if first {
            1
        } else {
            self.frames.pending_reports().saturating_add(1)
        };
        if first {
            self.frame_started_at_ms = received_at_ms;
        }
        let assembled = self.frames.push_frame(report);
        match self.frames.take_discard() {
            Some(FrameDiscard::AbandonedPartial { reports }) => self
                .inbound
                .record_abandoned_partial(reports, "a new FIRST report arrived"),
            Some(FrameDiscard::OrphanedContinuation) => self.inbound.record_orphaned_continuation(),
            None => {}
        }
        let assembled = match assembled {
            Ok(value) => value,
            Err(error) => {
                self.frame_started_at_ms = None;
                self.inbound.record_framing_error(&error.to_string());
                return Err(error);
            }
        };
        let Some(frame) = assembled else {
            return Ok(None);
        };
        // A complete envelope consumes its assembly state even when dispatch,
        // decryption, or inflation subsequently rejects it.
        let ingress = self
            .frame_started_at_ms
            .take()
            .zip(received_at_ms)
            .map(|(first, last)| IngressTime {
                first_report_at_ms: first,
                last_report_at_ms: last,
            });
        let Ok(message_type) = u16::try_from(frame.message_type) else {
            // Past the host's dispatch range, so there is no 16-bit type to
            // key any other counter by; the raw trailer value is the record.
            // It is nevertheless one fully assembled envelope.
            self.inbound.record_unkeyed_envelope();
            self.inbound.record_undeclared_type(frame.message_type);
            return Err(FrameError::MessageTypeOutOfRange(frame.message_type));
        };
        // Counted before any rejection below, so a frame this build drops
        // still contributes its trailer bytes and its type to the tally.
        self.inbound
            .record_envelope(message_type, frame.device_bytes);
        let undeclared = crate::message_registry::message_name(message_type).is_none();
        if undeclared {
            self.inbound.record_undeclared_type(u32::from(message_type));
        }
        #[cfg(feature = "cloud-forward")]
        if matches!(
            message_type,
            profile::MESSAGE_TYPE_CONNECTION | profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS
        ) {
            self.clear_zencryption_session();
        }
        // Encrypted cloud requests are available only through the forwarding
        // switch. Decompression remains owned by the protocol decoder.
        if frame.encrypted {
            #[cfg(feature = "cloud-forward")]
            if self.cloud_forward_enabled && crate::device_zencryption::applies_to(message_type) {
                use zeroize::Zeroizing;
                let mut clear = Zeroizing::new(
                    self.zencryption
                        .transform(&frame.payload)
                        .map_err(|_| FrameError::UnsupportedEncryptedMessageType)?,
                );
                let payload = if frame.compressed {
                    self.inbound.record_compressed(message_type);
                    crate::compression::gunzip_for_message_type(&clear, message_type)
                        .map_err(|error| FrameError::Compression(error.to_string()))?
                } else {
                    std::mem::take(&mut *clear)
                };

                return Ok(Some(InboundFrame {
                    message_type,
                    payload,
                    report_count,
                    ingress,
                    device_bytes: frame.device_bytes,
                }));
            }
            self.inbound.record_encrypted_rejection(message_type);
            if undeclared {
                self.inbound.record_undeclared_rejection_overlap();
            }
            return Err(FrameError::UnsupportedEncryptedMessageType);
        }
        let payload = if frame.compressed {
            self.inbound.record_compressed(message_type);
            // Each type inflates only as far as its own ceiling; a small
            // control message can never expand to catalogue scale.
            match crate::compression::gunzip_for_message_type(&frame.payload, message_type) {
                Ok(payload) => payload,
                Err(error) => {
                    // Counted per type in the inbound telemetry and named in
                    // the error, so a dropped frame is never silent.
                    self.inbound.record_inflate_failure(message_type);
                    if undeclared {
                        self.inbound.record_undeclared_rejection_overlap();
                    }
                    return Err(FrameError::Compression(format!(
                        "type {message_type} (limit {} bytes): {error}",
                        crate::compression::max_inflated_bytes_for(message_type)
                    )));
                }
            }
        } else {
            frame.payload
        };
        #[cfg(feature = "cloud-forward")]
        let mut payload = payload;
        #[cfg(feature = "cloud-forward")]
        // Install the connection key during every version exchange. The relay
        // switch is live and can turn on after the handshake; retaining only
        // this per-session key lets encrypted type-29 forwarding start without
        // forcing a transport reconnect. No encrypted payload is decoded while
        // the forwarding switch is off.
        if message_type == profile::MESSAGE_TYPE_VERSION {
            if let Ok(version) = pa::VersionMessage::decode(payload.as_slice()) {
                let mut version = version;
                if version.action == pa::message_action::Enum::Update as i32 {
                    if let Some(pa::version_message::CommsVersion::CommsVersion(fragment)) =
                        &mut version.comms_version
                    {
                        self.begin_zencryption_fragment(ingress.map(|time| time.last_report_at_ms));
                        self.zencryption.install_wrapped_fragment(fragment);
                        fragment.zeroize();
                        // The wrapped session key is transport state, not
                        // application state. Strip it before the Version frame
                        // enters shared caches or state-decoder queues.
                        version.comms_version = None;
                        payload = version.encode_to_vec();
                    }
                }
            }
        }
        Ok(Some(InboundFrame {
            message_type,
            payload,
            report_count,
            ingress,
            device_bytes: frame.device_bytes,
        }))
    }
}

/// The decoder rejects an inbound message whose payload is past the raw frame
/// ceiling, so the encoder holds outbound messages to the same limit and
/// reports the oversize where the message is encoded, not later and
/// indirectly in a delivery journal (or not at all, for a host without one).
fn reject_oversized_outbound(message: &OutboundMessage) -> Result<(), FrameError> {
    if message.payload.len() > profile::MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge);
    }
    Ok(())
}

/// Encrypted families that must never fall through to plaintext framing.
/// The cloud-forward feature handles its supported families before this guard.
fn is_unsupported_encrypted_message_type(message_type: u16) -> bool {
    matches!(message_type, 18 | 29 | 30 | 31 | 41 | 58)
}

/// Android interrupt reads expose the 128-byte body while hidapi exposes the
/// 129-byte report-id-prefixed form. Normalize both before parsing flags or
/// chunk lengths. The first body byte can legitimately equal the report id, so
/// length—not byte value alone—selects the layout.
fn normalize_inbound_report(raw: &[u8]) -> Result<Vec<u8>, FrameError> {
    if raw.len() == framing::REPORT_SIZE {
        if raw[0] != framing::IN_REPORT_ID {
            return Err(FrameError::InvalidLength);
        }
        return Ok(raw.to_vec());
    }
    // A body-only HID report must contain at least its length and flags bytes.
    // Android can surface a one-byte residual read while an interface is being
    // closed or reopened; reject it before push_report indexes the flags byte.
    if raw.len() < 2 || raw.len() > framing::REPORT_BODY_SIZE {
        return Err(FrameError::InvalidLength);
    }
    let mut report = Vec::with_capacity(raw.len() + 1);
    report.push(framing::IN_REPORT_ID);
    report.extend_from_slice(raw);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    /// One inbound report carrying an arbitrary envelope, including the
    /// trailer fields `framing::encode` fixes for the host's own writes.
    fn inbound_envelope(
        message_type: u32,
        payload: &[u8],
        encrypted: bool,
        compressed: bool,
        device_bytes: [u8; 2],
    ) -> Vec<u8> {
        let mut report = framing::encode_frame(&framing::Frame {
            message_type,
            payload: payload.to_vec(),
            encrypted,
            compressed,
            device_bytes,
        })[0]
            .to_vec();
        report[0] = framing::IN_REPORT_ID;
        report
    }

    #[test]
    fn overlapping_unknown_rejections_count_each_unusable_envelope_once() {
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(
            runtime.push_report(&inbound_envelope(73, &[8, 1], true, false, [0, 0])),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
        assert!(matches!(
            runtime.push_report(&inbound_envelope(73, &[0x1f, 0x8b, 0], false, true, [0, 0])),
            Err(FrameError::Compression(_))
        ));
        runtime
            .push_report(&inbound_envelope(73, &[8, 1], false, false, [0, 0]))
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime.push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_LICENSE),
                &[8, 1],
                true,
                false,
                [0, 0]
            )),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.frames, 4);
        assert_eq!(snapshot.unacted_frames, 4);
        assert_eq!(snapshot.undeclared_types[&73], 3);
        assert_eq!(snapshot.encrypted_rejections[&73], 1);
        assert_eq!(snapshot.inflate_failures[&73], 1);
        let mut merged = runtime.inbound_telemetry().clone();
        merged.merge(runtime.inbound_telemetry());
        assert_eq!(merged.snapshot().frames, 8);
        assert_eq!(merged.snapshot().unacted_frames, 8);
        assert_eq!(merged.snapshot().undeclared_types[&73], 6);
    }

    #[test]
    fn an_undeclared_inbound_type_is_counted_rather_than_vanishing() {
        let mut runtime = TransportRuntime::new(0);
        let frame = runtime
            .push_report(&inbound_envelope(73, &[8, 1], false, false, [0, 0]))
            .unwrap()
            .unwrap();
        assert_eq!(frame.message_type, 73);
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.frames, 1);
        assert_eq!(snapshot.undeclared_types[&73], 1);
        assert_eq!(snapshot.unacted_frames, 1);

        // A declared type on the same link is not confused with it.
        runtime
            .push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_SCENE),
                &[8, 1],
                false,
                false,
                [0, 0],
            ))
            .unwrap()
            .unwrap();
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.frames, 2);
        assert_eq!(snapshot.undeclared_types.len(), 1);
        assert_eq!(snapshot.unacted_frames, 1);
    }

    #[test]
    fn a_type_past_the_host_dispatch_range_is_recorded_by_its_raw_trailer_value() {
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(runtime.push_report(&[0]), Err(FrameError::InvalidLength));
        assert_eq!(runtime.inbound_telemetry().snapshot().frames, 0);
        assert_eq!(
            runtime.push_report(&inbound_envelope(0x0001_0000, &[], false, false, [0, 0])),
            Err(FrameError::MessageTypeOutOfRange(0x0001_0000))
        );
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.undeclared_types[&0x0001_0000], 1);
        // Complete envelopes count even when no 16-bit trailer key exists.
        assert_eq!(snapshot.frames, 1);
        assert_eq!(snapshot.unacted_frames, 1);
        assert!(snapshot.device_trailer_bytes.is_empty());
        assert!(snapshot.encrypted_rejections.is_empty());
        assert!(snapshot.inflate_failures.is_empty());
        assert_eq!(
            runtime.push_report(&inbound_envelope(u32::MAX, &[], false, false, [7, 8])),
            Err(FrameError::MessageTypeOutOfRange(u32::MAX))
        );
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.frames, 2);
        assert_eq!(snapshot.unacted_frames, 2);
        assert_eq!(snapshot.undeclared_types[&u32::MAX], 1);
        assert_eq!(snapshot.undeclared_types.len(), 2);
        assert!(snapshot.device_trailer_bytes.is_empty());
    }

    #[test]
    fn an_encrypted_or_uninflatable_frame_names_its_type_in_the_tally() {
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(
            runtime.push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_LICENSE),
                &[8, 1],
                true,
                false,
                [0, 0],
            )),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
        assert!(matches!(
            runtime.push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_MODEL_REPO),
                &[0x1f, 0x8b, 0x00],
                false,
                true,
                [0, 0],
            )),
            Err(FrameError::Compression(_))
        ));
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(
            snapshot.encrypted_rejections[&profile::MESSAGE_TYPE_LICENSE],
            1
        );
        assert_eq!(snapshot.compressed[&profile::MESSAGE_TYPE_MODEL_REPO], 1);
        assert_eq!(
            snapshot.inflate_failures[&profile::MESSAGE_TYPE_MODEL_REPO],
            1
        );
        assert_eq!(snapshot.frames, 2);
        assert_eq!(snapshot.unacted_frames, 2);
        // A compressed frame that inflates is tallied but is not an outcome.
        let payload = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).unwrap();
        runtime
            .push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_VERSION),
                &encoder.finish().unwrap(),
                false,
                true,
                [0, 0],
            ))
            .unwrap()
            .unwrap();
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.compressed[&profile::MESSAGE_TYPE_VERSION], 1);
        assert!(!snapshot
            .inflate_failures
            .contains_key(&profile::MESSAGE_TYPE_VERSION));
        assert_eq!(snapshot.unacted_frames, 2);
    }

    #[test]
    fn the_device_trailer_bytes_reach_the_frame_and_the_tally() {
        use crate::inbound_telemetry::DeviceTrailerTally;

        let mut runtime = TransportRuntime::new(0);
        let frame = runtime
            .push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_SCENE),
                &[8, 1],
                false,
                false,
                [0x9a, 0xbc],
            ))
            .unwrap()
            .unwrap();
        assert_eq!(frame.device_bytes, [0x9a, 0xbc]);
        // A rejected frame still contributes the bytes nothing interprets.
        assert_eq!(
            runtime.push_report(&inbound_envelope(
                u32::from(profile::MESSAGE_TYPE_SCENE),
                &[8, 1],
                true,
                false,
                [0x9a, 0xbc],
            )),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
        assert_eq!(
            runtime.inbound_telemetry().snapshot().device_trailer_bytes,
            vec![DeviceTrailerTally {
                message_type: profile::MESSAGE_TYPE_SCENE,
                device_bytes: [0x9a, 0xbc],
                count: 2,
            }]
        );
    }

    #[test]
    fn a_reconnect_ends_the_inbound_tally_with_the_connection() {
        let mut runtime = TransportRuntime::new(0);
        runtime
            .push_report(&inbound_envelope(73, &[], false, false, [1, 2]))
            .unwrap();
        assert!(runtime.inbound_telemetry().revision() > 0);
        runtime.disconnect(0, true);
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.frames, 0);
        assert!(snapshot.undeclared_types.is_empty());
        assert!(snapshot.device_trailer_bytes.is_empty());
    }

    fn inbound_reports(message_type: u16, payload: &[u8]) -> Vec<Vec<u8>> {
        framing::encode(message_type, payload)
            .into_iter()
            .map(|mut report| {
                report[0] = framing::IN_REPORT_ID;
                report.to_vec()
            })
            .collect()
    }

    #[cfg(feature = "cloud-forward")]
    fn push_wrapped_version_fragment(
        runtime: &mut TransportRuntime,
        wrapped_fragment: &[u8],
        received_at_ms: u64,
    ) {
        let payload = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            comms_version: Some(pa::version_message::CommsVersion::CommsVersion(
                wrapped_fragment.to_vec(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        for report in inbound_reports(profile::MESSAGE_TYPE_VERSION, &payload) {
            runtime.push_report_at(&report, received_at_ms).unwrap();
        }
    }

    #[cfg(feature = "cloud-forward")]
    fn assert_type29_decrypts_with_fragments(
        receiver: &mut TransportRuntime,
        wrapped_fragments: &[&[u8]],
        received_at_ms: u64,
    ) {
        let mut sender = TransportRuntime::new(0);
        sender.set_cloud_forward_enabled(true);
        assert!(
            !wrapped_fragments.is_empty(),
            "test requires handshake fragments"
        );
        for fragment in wrapped_fragments {
            assert!(sender.zencryption.install_wrapped_fragment(fragment));
        }
        let clear_payload = vec![0x08, 0x01, 0x18, 0x22, 0x2a, 0x02, 0x0a, 0x00];
        let encrypted_reports = sender
            .encode_reports_with_session(
                &OutboundMessage {
                    message_type: profile::MESSAGE_TYPE_PRODUCT_FORWARD,
                    payload: clear_payload.clone(),
                },
                ReportLayout::ReportIdPrefixed,
            )
            .unwrap();
        let mut decoded = None;
        for mut report in encrypted_reports {
            report[0] = framing::IN_REPORT_ID;
            decoded = receiver
                .push_report_at(&report, received_at_ms)
                .unwrap()
                .or(decoded);
        }
        assert_eq!(
            decoded
                .expect("encrypted type-29 frame is reassembled")
                .payload,
            clear_payload
        );
    }

    #[test]
    fn prefixed_and_body_only_hosts_decode_identically() {
        let payload = vec![0x5a; framing::CHUNK_SIZE * 2 + 7];
        let reports = inbound_reports(profile::MESSAGE_TYPE_MODEL_REPO, &payload);
        let mut prefixed = TransportRuntime::new(0);
        let mut body_only = TransportRuntime::new(0);
        let mut left = None;
        let mut right = None;
        for report in reports {
            left = prefixed.push_report(&report).unwrap().or(left);
            right = body_only.push_report(&report[1..]).unwrap().or(right);
        }
        assert_eq!(left, right);
        assert_eq!(left.unwrap().report_count, 3);
    }

    #[test]
    fn assembled_ingress_keeps_the_first_native_report_time() {
        let reports = inbound_reports(13, &vec![0x5a; framing::CHUNK_SIZE * 2]);
        assert_eq!(reports.len(), 3);
        let mut runtime = TransportRuntime::new(0);
        assert!(runtime.push_report_at(&reports[0], 10).unwrap().is_none());
        assert!(runtime.push_report_at(&reports[1], 20).unwrap().is_none());
        let frame = runtime.push_report_at(&reports[2], 30).unwrap().unwrap();
        assert_eq!(
            frame.ingress,
            Some(IngressTime {
                first_report_at_ms: 10,
                last_report_at_ms: 30
            })
        );
        // A malformed report discards both buffered bytes and their provenance.
        assert!(runtime.push_report_at(&reports[0], 40).unwrap().is_none());
        assert!(runtime.push_report_at(&[1], 50).is_err());
        let single = inbound_reports(13, &[8, 1]);
        let frame = runtime.push_report_at(&single[0], 60).unwrap().unwrap();
        assert_eq!(
            frame.ingress,
            Some(IngressTime {
                first_report_at_ms: 60,
                last_report_at_ms: 60
            })
        );
    }

    #[test]
    fn truncated_body_only_reports_are_rejected_without_panicking() {
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(runtime.push_report(&[]), Err(FrameError::InvalidLength));
        assert_eq!(runtime.push_report(&[0]), Err(FrameError::InvalidLength));
    }

    #[test]
    fn encrypted_reports_are_rejected() {
        let mut report = framing::encode(13, &[0x08, 0x01])[0].to_vec();
        report[0] = framing::IN_REPORT_ID;
        let trailer_offset = 3 + 2;
        report[trailer_offset + 4] = 1;
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(
            runtime.push_report(&report),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
    }

    #[test]
    fn compressed_version_message_is_inflated_before_dispatch() {
        let payload = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut report = framing::encode_frame(&framing::Frame {
            message_type: u32::from(profile::MESSAGE_TYPE_VERSION),
            payload: compressed,
            encrypted: false,
            compressed: true,
            device_bytes: [0, 0],
        })[0]
            .to_vec();
        report[0] = framing::IN_REPORT_ID;
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(
            runtime.push_report(&report).unwrap().unwrap().payload,
            payload
        );
    }

    /// Pushes every report of one compressed inbound envelope and returns the
    /// outcome of the last one.
    fn push_compressed(
        runtime: &mut TransportRuntime,
        message_type: u16,
        plain: &[u8],
    ) -> Result<Option<InboundFrame>, FrameError> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(plain).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() <= profile::MAX_FRAME_BYTES);
        let reports = framing::encode_frame(&framing::Frame {
            message_type: u32::from(message_type),
            payload: compressed,
            encrypted: false,
            compressed: true,
            device_bytes: [0, 0],
        });
        let mut outcome = Ok(None);
        for report in reports {
            let mut report = report.to_vec();
            report[0] = framing::IN_REPORT_ID;
            outcome = runtime.push_report(&report);
        }
        outcome
    }

    #[test]
    fn a_gzip_bomb_on_a_small_type_is_rejected_and_tallied() {
        let mut runtime = TransportRuntime::new(0);
        // 8 MiB of zeros: within the global ceiling, far past a Version
        // message's own.
        let outcome = push_compressed(
            &mut runtime,
            profile::MESSAGE_TYPE_VERSION,
            &vec![0; 8 * 1024 * 1024],
        );
        assert!(
            matches!(outcome, Err(FrameError::Compression(ref message)) if message.contains("limit")),
            "{outcome:?}"
        );
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.compressed[&profile::MESSAGE_TYPE_VERSION], 1);
        assert_eq!(snapshot.inflate_failures[&profile::MESSAGE_TYPE_VERSION], 1);
        // The rejected envelope does not poison the next one. 32 KiB is under
        // Version's captured 64 KiB limit and 100x its largest real frame.
        let legit = vec![5; 32 * 1024];
        assert_eq!(
            push_compressed(&mut runtime, profile::MESSAGE_TYPE_VERSION, &legit)
                .unwrap()
                .unwrap()
                .payload,
            legit
        );
    }

    #[test]
    fn a_model_repo_frame_may_inflate_to_mib_scale_but_not_past_its_ceiling() {
        let mut runtime = TransportRuntime::new(0);
        let catalogue = vec![9; 4 * 1024 * 1024];
        let frame = push_compressed(&mut runtime, profile::MESSAGE_TYPE_MODEL_REPO, &catalogue)
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload.len(), catalogue.len());
        let outcome = push_compressed(
            &mut runtime,
            profile::MESSAGE_TYPE_MODEL_REPO,
            &vec![0; profile::MAX_INFLATED_BYTES + 1],
        );
        assert!(
            matches!(outcome, Err(FrameError::Compression(ref message)) if message.contains("limit")),
            "{outcome:?}"
        );
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.compressed[&profile::MESSAGE_TYPE_MODEL_REPO], 2);
        assert_eq!(
            snapshot.inflate_failures[&profile::MESSAGE_TYPE_MODEL_REPO],
            1
        );
    }

    #[test]
    fn a_partial_frame_cut_by_a_new_first_report_is_counted() {
        // The assembler used to restart silently, throwing away a
        // partial message that left no trace anywhere.
        let mut runtime = TransportRuntime::new(0);
        let cut = inbound_reports(51, &[0x33; framing::CHUNK_SIZE * 3]);
        assert!(runtime.push_report(&cut[0]).unwrap().is_none());
        assert!(runtime.push_report(&cut[1]).unwrap().is_none());
        let fresh = inbound_reports(13, &[8, 1, 24, 2]);
        let frame = runtime.push_report(&fresh[0]).unwrap().unwrap();
        assert_eq!(frame.payload, [8, 1, 24, 2]);
        assert_eq!(frame.report_count, 1);
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.abandoned_partial_frames, 1);
        assert_eq!(snapshot.orphaned_continuation_reports, 0);
        assert_eq!(snapshot.framing_errors, 0);
        assert!(snapshot
            .last_unacted
            .as_deref()
            .is_some_and(|reason| reason.contains("2 report(s)")));
    }

    #[test]
    fn an_orphaned_continuation_is_counted() {
        let mut runtime = TransportRuntime::new(0);
        let reports = inbound_reports(51, &[0x33; framing::CHUNK_SIZE * 2]);
        // The host attached after the FIRST report; both continuations are
        // ignored, and each is counted.
        assert!(runtime.push_report(&reports[1]).unwrap().is_none());
        assert!(runtime.push_report(&reports[2]).unwrap().is_none());
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.orphaned_continuation_reports, 2);
        assert_eq!(snapshot.abandoned_partial_frames, 0);
        assert_eq!(snapshot.frames, 0);
    }

    #[test]
    fn an_epoch_change_counts_the_frame_it_cuts_and_the_continuations_it_drops() {
        let mut runtime = TransportRuntime::new(0);
        let reports = inbound_reports(51, &[0x33; framing::CHUNK_SIZE * 2]);
        assert!(runtime
            .push_report_in_epoch(&reports[0], 1, 1)
            .unwrap()
            .is_none());
        // A new epoch: the open message is cut, and its remaining reports
        // can no longer complete anything.
        assert!(runtime
            .push_report_in_epoch(&reports[1], 2, 2)
            .unwrap()
            .is_none());
        assert!(runtime
            .push_report_in_epoch(&reports[2], 3, 2)
            .unwrap()
            .is_none());
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.abandoned_partial_frames, 1);
        assert_eq!(snapshot.orphaned_continuation_reports, 2);
        // The next FIRST report in the new epoch assembles normally.
        let fresh = inbound_reports(13, &[8, 1, 24, 2]);
        let frame = runtime
            .push_report_in_epoch(&fresh[0], 4, 2)
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload, [8, 1, 24, 2]);
        assert_eq!(frame.report_count, 1);
    }

    #[test]
    fn framing_errors_are_counted() {
        let mut runtime = TransportRuntime::new(0);
        assert_eq!(runtime.push_report(&[0]), Err(FrameError::InvalidLength));
        // A single report that is FIRST and LAST but shorter than the trailer.
        let mut short = vec![0_u8; framing::REPORT_SIZE];
        short[0] = framing::IN_REPORT_ID;
        short[1] = 2;
        short[2] = framing::FLAG_FIRST | framing::FLAG_LAST;
        assert_eq!(runtime.push_report(&short), Err(FrameError::MissingTrailer));
        let snapshot = runtime.inbound_telemetry().snapshot();
        assert_eq!(snapshot.framing_errors, 2);
        assert!(snapshot
            .last_unacted
            .as_deref()
            .is_some_and(|reason| reason.starts_with("framing error")));
    }

    #[test]
    fn a_multi_report_frame_still_reports_its_report_count() {
        let mut runtime = TransportRuntime::new(0);
        let reports = inbound_reports(51, &[0x33; framing::CHUNK_SIZE * 2]);
        assert_eq!(reports.len(), 3);
        let mut frame = None;
        for (index, report) in reports.iter().enumerate() {
            frame = runtime.push_report_at(report, 10 + index as u64).unwrap();
        }
        let frame = frame.unwrap();
        assert_eq!(frame.report_count, 3);
        assert_eq!(
            frame.ingress,
            Some(IngressTime {
                first_report_at_ms: 10,
                last_report_at_ms: 12
            })
        );
    }

    #[test]
    fn the_encoder_rejects_a_payload_past_the_frame_ceiling() {
        // The decoder rejects anything over MAX_FRAME_BYTES, so the
        // encoder does too, where the message is created.
        let runtime = TransportRuntime::new(0);
        let at_limit = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_FILE,
            payload: vec![0; profile::MAX_FRAME_BYTES],
        };
        let past_limit = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_FILE,
            payload: vec![0; profile::MAX_FRAME_BYTES + 1],
        };
        for layout in [ReportLayout::ReportIdPrefixed, ReportLayout::BodyOnly] {
            assert!(TransportRuntime::encode_reports(&at_limit, layout).is_ok());
            assert!(runtime
                .encode_reports_with_session(&at_limit, layout)
                .is_ok());
            assert_eq!(
                TransportRuntime::encode_reports(&past_limit, layout),
                Err(FrameError::TooLarge)
            );
            assert_eq!(
                runtime.encode_reports_with_session(&past_limit, layout),
                Err(FrameError::TooLarge)
            );
        }
    }

    #[test]
    fn a_compressed_file_listing_may_inflate_past_the_raw_ceiling() {
        // The transport delivers a File listing inflated past 1 MiB
        // when it stays within the File type's own limit.
        let mut runtime = TransportRuntime::new(0);
        let listing = crate::state::large_file_listing(profile::MAX_FRAME_BYTES + 1);
        let frame = push_compressed(&mut runtime, profile::MESSAGE_TYPE_FILE, &listing)
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload, listing);
        assert!(frame.payload.len() > profile::MAX_FRAME_BYTES);
        assert!(
            frame.payload.len()
                <= crate::compression::max_delivered_payload_bytes_for(profile::MESSAGE_TYPE_FILE)
        );
    }

    #[test]
    fn every_native_read_length_is_total() {
        for length in 0..=framing::REPORT_SIZE * 2 {
            let mut report = vec![0_u8; length];
            if length == framing::REPORT_SIZE {
                report[0] = framing::IN_REPORT_ID;
            }
            let mut runtime = TransportRuntime::new(0);
            let _ = runtime.push_report(&report);
        }
    }

    #[test]
    fn invalid_native_report_discards_partial_message_on_both_layouts() {
        for body_only in [false, true] {
            let reports = inbound_reports(51, &[0x33; framing::CHUNK_SIZE]);
            let native = |report: &[u8]| report[usize::from(body_only)..].to_vec();
            let mut runtime = TransportRuntime::new(0);
            assert!(runtime.push_report(&native(&reports[0])).unwrap().is_none());
            assert_eq!(runtime.push_report(&[0]), Err(FrameError::InvalidLength));
            assert!(runtime.push_report(&native(&reports[1])).unwrap().is_none());
            let fresh = inbound_reports(13, &[8, 1, 24, 2]);
            let frame = runtime.push_report(&native(&fresh[0])).unwrap().unwrap();
            assert_eq!(frame.payload, [8, 1, 24, 2]);
            assert_eq!(frame.report_count, 1);
        }
    }

    #[test]
    fn compressed_envelope_rejects_plain_protobuf() {
        let mut report = framing::encode_frame(&framing::Frame {
            message_type: 13,
            payload: vec![8, 1, 24, 2],
            encrypted: false,
            compressed: true,
            device_bytes: [0, 0],
        })[0];
        report[0] = framing::IN_REPORT_ID;
        assert!(matches!(
            TransportRuntime::new(0).push_report(&report),
            Err(FrameError::Compression(_))
        ));
    }

    #[test]
    fn body_chunk_length_equal_to_report_id_is_not_misclassified() {
        // Payload + eight-byte trailer = 127, so the final body-only report's
        // chunk-length byte is 1, the same value as IN_REPORT_ID.
        let payload = vec![0x33; framing::CHUNK_SIZE - 7];
        let reports = inbound_reports(profile::MESSAGE_TYPE_MODEL_REPO, &payload);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[1][1], framing::IN_REPORT_ID);
        let mut runtime = TransportRuntime::new(0);
        assert!(runtime.push_report(&reports[0][1..]).unwrap().is_none());
        let frame = runtime.push_report(&reports[1][1..]).unwrap().unwrap();
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.report_count, 2);
    }

    #[test]
    fn outbound_layout_changes_no_protocol_bytes() {
        let message = commands::read(profile::MESSAGE_TYPE_VERSION);
        let prefixed =
            TransportRuntime::encode_reports(&message, ReportLayout::ReportIdPrefixed).unwrap();
        let body = TransportRuntime::encode_reports(&message, ReportLayout::BodyOnly).unwrap();
        assert_eq!(prefixed.len(), body.len());
        for (full, data) in prefixed.iter().zip(body) {
            assert_eq!(&full[1..], data);
        }
    }

    /// Every message an initialization burst composes has to be encodable by
    /// the encoder that will send it. Nothing checked that, and the two halves
    /// drifted apart the moment the cloud-account/license encryption was
    /// removed and the outbound guard took its place: the Booting burst went on
    /// asking for a License read, the encoder refused it, and the app could no
    /// longer connect to the device at all. A burst that cannot be sent is not
    /// a runtime condition to handle - it cannot ever work.
    #[test]
    fn every_initialization_burst_message_is_encodable() {
        let runtime = TransportRuntime::new(0);
        let seed_reads = crate::device::initialization::REQUIRED_SEED_TYPES
            .iter()
            .copied()
            .map(commands::read)
            .collect::<Vec<_>>();
        let bursts: Vec<(&str, Vec<OutboundMessage>)> = vec![
            ("building", commands::building_initialization()),
            ("module_stats", commands::module_stats_initialization()),
            ("boot", commands::boot_initialization()),
            ("boot_exact", commands::boot_initialization_exact(7, true)),
            ("disconnect", commands::disconnect_initialization()),
            (
                "disconnect_exact",
                commands::disconnect_initialization_exact(7),
            ),
            ("seed", seed_reads),
        ];
        assert!(!bursts.is_empty(), "no initialization bursts to check");
        for (burst, messages) in bursts {
            // Without this the loop below asserts nothing when a composer
            // returns an empty vec, and the gate passes having checked no
            // message at all - which is exactly the shape of failure it exists
            // to catch, since an empty burst sends nothing to the device.
            assert!(!messages.is_empty(), "empty burst: {burst}");
            for (position, message) in messages.iter().enumerate() {
                assert!(
                    runtime
                        .encode_reports_with_session(message, ReportLayout::ReportIdPrefixed)
                        .is_ok(),
                    "the {burst} burst composes message type {} at position {position}, \
                     which this build's own session encoder refuses to send",
                    message.message_type,
                );
            }
        }
    }

    #[test]
    fn stateless_encoder_never_leaks_an_encrypted_family_in_plaintext() {
        let message = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_LICENSE,
            payload: b"must-not-be-cleartext".to_vec(),
        };
        assert_eq!(
            TransportRuntime::encode_reports(&message, ReportLayout::ReportIdPrefixed),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
        assert_eq!(
            TransportRuntime::new(0)
                .encode_reports_with_session(&message, ReportLayout::ReportIdPrefixed),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );
    }

    #[cfg(feature = "cloud-forward")]
    #[test]
    fn opted_in_product_forward_response_is_encrypted_and_decodes_in_same_session() {
        let mut runtime = TransportRuntime::new(0);
        runtime.cloud_forward_enabled = true;
        assert!(runtime.zencryption.install_wrapped_fragment(&[
            0x63, 0x7a, 0xa2, 0x2b, 0x4b, 0xc2, 0x1e, 0x69, 0x43, 0x5e, 0xf1, 0x1d, 0x01, 0xd1,
            0x29, 0xb8, 0x37, 0xf5, 0x61, 0x05,
        ]));
        let clear = vec![0x08, 0x01, 0x18, 0x22, 0x2a, 0x02, 0x0a, 0x00];
        let message = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_PRODUCT_FORWARD,
            payload: clear.clone(),
        };

        let reports = runtime
            .encode_reports_with_session(&message, ReportLayout::ReportIdPrefixed)
            .unwrap();
        assert!(!reports.is_empty());
        let mut decoded = None;
        for report in reports {
            let mut inbound = report;
            inbound[0] = framing::IN_REPORT_ID;
            decoded = runtime.push_report(&inbound).unwrap().or(decoded);
        }
        let decoded = decoded.expect("encrypted frame reassembled");
        assert_eq!(decoded.message_type, profile::MESSAGE_TYPE_PRODUCT_FORWARD);
        assert_eq!(decoded.payload, clear);
    }

    #[cfg(feature = "cloud-forward")]
    #[test]
    fn opted_in_cloud_login_update_is_encrypted_and_decodes_in_same_session() {
        let mut runtime = TransportRuntime::new(0);
        runtime.cloud_forward_enabled = true;
        assert!(runtime.zencryption.install_wrapped_fragment(&[
            0x63, 0x7a, 0xa2, 0x2b, 0x4b, 0xc2, 0x1e, 0x69, 0x43, 0x5e, 0xf1, 0x1d, 0x01, 0xd1,
            0x29, 0xb8, 0x37, 0xf5, 0x61, 0x05,
        ]));
        let clear = vec![0x08, 0x01, 0x22, 0x02, 0x0a, 0x00];
        let message = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_CLOUD_LOGIN,
            payload: clear.clone(),
        };

        let reports = runtime
            .encode_reports_with_session(&message, ReportLayout::ReportIdPrefixed)
            .unwrap();
        assert!(!reports.is_empty());
        let mut decoded = None;
        for report in reports {
            let mut inbound = report;
            inbound[0] = framing::IN_REPORT_ID;
            decoded = runtime.push_report(&inbound).unwrap().or(decoded);
        }
        let decoded = decoded.expect("encrypted frame reassembled");
        assert_eq!(decoded.message_type, profile::MESSAGE_TYPE_CLOUD_LOGIN);
        assert_eq!(decoded.payload, clear);
    }

    #[cfg(feature = "cloud-forward")]
    #[test]
    fn enabling_cloud_forward_after_handshake_uses_the_captured_session_key() {
        let wrapped_secret = [
            0x63, 0x7a, 0xa2, 0x2b, 0x4b, 0xc2, 0x1e, 0x69, 0x43, 0x5e, 0xf1, 0x1d, 0x01, 0xd1,
            0x29, 0xb8, 0x37, 0xf5, 0x61, 0x05,
        ];
        let version = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            comms_version: Some(pa::version_message::CommsVersion::CommsVersion(
                wrapped_secret.to_vec(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut receiver = TransportRuntime::new(0);
        assert!(!receiver.cloud_forward_enabled);
        let mut decoded_version = None;
        for report in inbound_reports(profile::MESSAGE_TYPE_VERSION, &version) {
            decoded_version = receiver.push_report(&report).unwrap().or(decoded_version);
        }
        let decoded_version = decoded_version.expect("version frame is reassembled");
        let decoded_version =
            pa::VersionMessage::decode(decoded_version.payload.as_slice()).unwrap();
        assert!(
            decoded_version.comms_version.is_none(),
            "wrapped key is stripped before publication"
        );
        receiver.set_cloud_forward_enabled(true);

        let mut sender = TransportRuntime::new(0);
        assert!(sender.zencryption.install_wrapped_fragment(&wrapped_secret));
        sender.set_cloud_forward_enabled(true);
        let clear_payload = vec![0x08, 0x01, 0x18, 0x22, 0x2a, 0x02, 0x0a, 0x00];
        let encrypted_reports = sender
            .encode_reports_with_session(
                &OutboundMessage {
                    message_type: profile::MESSAGE_TYPE_PRODUCT_FORWARD,
                    payload: clear_payload.clone(),
                },
                ReportLayout::ReportIdPrefixed,
            )
            .unwrap();
        let mut decoded = None;
        for mut report in encrypted_reports {
            report[0] = framing::IN_REPORT_ID;
            decoded = receiver.push_report(&report).unwrap().or(decoded);
        }
        let decoded = decoded.expect("encrypted type-29 frame is reassembled");
        assert_eq!(decoded.message_type, profile::MESSAGE_TYPE_PRODUCT_FORWARD);
        assert_eq!(decoded.payload, clear_payload);
    }

    #[cfg(feature = "cloud-forward")]
    #[test]
    fn cloud_secret_resets_at_connection_and_comms_reset_boundaries() {
        let wrapped_fragment = [
            0x63, 0x7a, 0xa2, 0x2b, 0x4b, 0xc2, 0x1e, 0x69, 0x43, 0x5e, 0xf1, 0x1d, 0x01, 0xd1,
            0x29, 0xb8, 0x37, 0xf5, 0x61, 0x05,
        ];

        for boundary_type in [
            profile::MESSAGE_TYPE_CONNECTION,
            profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
        ] {
            let mut receiver = TransportRuntime::new(0);
            receiver.set_cloud_forward_enabled(true);
            push_wrapped_version_fragment(&mut receiver, &wrapped_fragment, 10);
            for report in inbound_reports(boundary_type, &[0x08, 0x01]) {
                receiver.push_report_at(&report, 20).unwrap();
            }
            push_wrapped_version_fragment(&mut receiver, &wrapped_fragment, 30);
            assert_type29_decrypts_with_fragments(&mut receiver, &[&wrapped_fragment], 40);
        }
    }

    #[cfg(feature = "cloud-forward")]
    #[test]
    fn cloud_secret_resets_after_a_gap_between_handshake_fragments() {
        let wrapped_fragment = [
            0x63, 0x7a, 0xa2, 0x2b, 0x4b, 0xc2, 0x1e, 0x69, 0x43, 0x5e, 0xf1, 0x1d, 0x01, 0xd1,
            0x29, 0xb8, 0x37, 0xf5, 0x61, 0x05,
        ];
        let mut receiver = TransportRuntime::new(0);
        receiver.set_cloud_forward_enabled(true);
        push_wrapped_version_fragment(&mut receiver, &wrapped_fragment, 10);
        push_wrapped_version_fragment(&mut receiver, &wrapped_fragment, 2_011);

        assert_type29_decrypts_with_fragments(&mut receiver, &[&wrapped_fragment], 2_012);
    }

    #[cfg(feature = "cloud-forward")]
    #[test]
    fn cloud_fragments_within_one_handshake_are_still_appended() {
        let wrapped_fragment = [
            0x63, 0x7a, 0xa2, 0x2b, 0x4b, 0xc2, 0x1e, 0x69, 0x43, 0x5e, 0xf1, 0x1d, 0x01, 0xd1,
            0x29, 0xb8, 0x37, 0xf5, 0x61, 0x05,
        ];
        let mut receiver = TransportRuntime::new(0);
        receiver.set_cloud_forward_enabled(true);
        push_wrapped_version_fragment(&mut receiver, &wrapped_fragment, 10);
        push_wrapped_version_fragment(&mut receiver, &wrapped_fragment, 11);

        assert_type29_decrypts_with_fragments(
            &mut receiver,
            &[&wrapped_fragment, &wrapped_fragment],
            12,
        );
    }

    #[test]
    fn handshake_layout_and_keepalive_are_reserved_once() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(10);
        let first = runtime.next_handshake_write(10, "session-a").unwrap();
        assert_eq!(first.attempt, 1);
        assert_eq!(first.layout, ReportLayout::ReportIdPrefixed);
        assert!(runtime.next_handshake_write(11, "unused").is_none());
        let second = runtime
            .next_handshake_write(10 + profile::HANDSHAKE_ATTEMPT_TIMEOUT_MS, "session-b")
            .unwrap();
        assert_eq!(second.attempt, 2);
        assert_eq!(second.layout, ReportLayout::BodyOnly);

        runtime.handshake_completed(20, true);
        let due = 20 + profile::KEEPALIVE_INTERVAL_MS;
        assert!(runtime.take_keepalive(due).is_some());
        assert!(runtime.take_keepalive(due).is_none());
    }

    #[test]
    fn host_layout_is_explicit_and_reference_report_pacing_is_shared() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        let first = runtime
            .next_handshake_write_with_layout(0, "windows", ReportLayout::ReportIdPrefixed)
            .unwrap();
        assert_eq!(first.layout, ReportLayout::ReportIdPrefixed);
        assert_eq!(TransportRuntime::report_batch_delay_ms(9), None);
        assert_eq!(TransportRuntime::report_batch_delay_ms(10), Some(6));
        assert_eq!(TransportRuntime::report_batch_delay_ms(20), Some(6));
    }

    #[test]
    fn handshake_write_rejects_type_only_and_mismatched_sessions() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        let write = runtime.next_handshake_write(0, "opaque-session").unwrap();
        let reply = |request_id, session_id: &str| {
            pa::ResetCommsBuffersMessage {
                request_id: Some(pa::reset_comms_buffers_message::RequestId::RequestId(
                    request_id,
                )),
                session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                    session_id.into(),
                )),
            }
            .encode_to_vec()
        };
        assert!(write.matches_reply(&reply(1, "opaque-session")));
        assert!(write.matches_reply(&reply(2, "opaque-session")));
        assert!(write.matches_reply(
            &pa::ResetCommsBuffersMessage {
                request_id: None,
                session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                    "opaque-session".into(),
                )),
            }
            .encode_to_vec()
        ));
        assert!(!write.matches_reply(&reply(1, "other-session")));
        assert!(!write.matches_reply(&[]));
    }

    #[test]
    fn busy_traffic_does_not_defer_the_dedicated_keepalive() {
        // Ordinary session activity -- here, repeated read-side
        // synchronization updates -- must never substitute for or defer the
        // device's dedicated KeepAlive. (A prior revision also drove a
        // dedicated, deliberately no-op `outbound()` hook through this same
        // loop to pin that ordinary *writes* could not defer it either; that
        // hook and its only callers were removed as dead code, since a call
        // that does nothing should not exist. This test still stands for the
        // read side of the same guarantee.)
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        runtime.handshake_completed(10, false);
        runtime.keepalive_sent(10);
        for tick in (0..profile::KEEPALIVE_INTERVAL_MS).step_by(250) {
            runtime
                .synchronization_completed(10 + tick, tick >= profile::KEEPALIVE_INTERVAL_MS / 2);
        }
        assert_eq!(runtime.phase(), SessionPhase::Ready);
        assert!(runtime
            .take_keepalive(10 + profile::KEEPALIVE_INTERVAL_MS)
            .is_some());
    }

    #[test]
    fn link_loss_requires_two_consecutive_read_errors() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        runtime.handshake_completed(1, true);
        assert!(!runtime.read_failed());
        runtime.read_succeeded();
        assert!(!runtime.read_failed());
        assert!(runtime.read_failed());
        runtime.disconnect(100, true);
        assert!(!runtime.reconnect_due(100));
        assert!(runtime.reconnect_due(100 + profile::RECONNECT_INTERVAL_MS));
    }

    #[test]
    fn a_terminal_native_reader_failure_requests_immediate_recovery() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        runtime.handshake_completed(1, true);
        assert!(runtime.terminal_read_failed());
    }

    #[test]
    fn reconnect_discards_an_incomplete_frame() {
        let reports = inbound_reports(51, &vec![1; framing::CHUNK_SIZE * 2]);
        let mut runtime = TransportRuntime::new(0);
        assert!(runtime.push_report(&reports[0]).unwrap().is_none());
        runtime.request_reconnect(5);
        assert!(runtime.push_report(&reports[1]).unwrap().is_none());
        let complete = inbound_reports(13, &[8, 1, 24, 2]);
        assert_eq!(
            runtime
                .push_report(&complete[0])
                .unwrap()
                .unwrap()
                .message_type,
            13
        );
    }

    #[test]
    fn reset_device_session_discards_transport_epoch_before_retry() {
        let reports = inbound_reports(51, &vec![1; framing::CHUNK_SIZE * 2]);
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        runtime.handshake_completed(1, true);
        assert!(runtime.synchronized());
        assert!(runtime.push_report(&reports[0]).unwrap().is_none());

        runtime.request_reconnect(50);
        assert_eq!(runtime.phase(), SessionPhase::Searching);
        assert!(!runtime.synchronized());
        assert!(runtime.reconnect_due(50));
        assert!(runtime.push_report(&reports[1]).unwrap().is_none());
        assert_eq!(
            runtime.encode_reports_with_session(
                &OutboundMessage {
                    message_type: profile::MESSAGE_TYPE_BACKUPS_FORWARD,
                    payload: vec![],
                },
                ReportLayout::ReportIdPrefixed,
            ),
            Err(FrameError::UnsupportedEncryptedMessageType)
        );

        runtime.transport_opened(51);
        let retry = runtime.next_handshake_write(51, "new-epoch").unwrap();
        assert_eq!(retry.attempt, 1);
        assert_eq!(retry.layout, ReportLayout::ReportIdPrefixed);
        assert_eq!(retry.session_id(), "new-epoch");
    }

    #[test]
    fn shared_runtime_owns_reconnect_schedule_and_reservation() {
        let mut runtime = TransportRuntime::new(0);
        let delay = runtime.schedule_reconnect(100);
        assert_eq!(delay, profile::RECONNECT_INTERVAL_MS);
        assert!(!runtime.reconnect_due(100 + delay - 1));
        assert!(runtime.reconnect_due(100 + delay));
        runtime.reconnect_attempted(100 + delay);
        assert!(!runtime.reconnect_due(100 + delay));
        assert!(runtime.reconnect_due(100 + delay * 2));
    }

    #[test]
    fn failed_connects_back_off_and_an_explicit_reset_clears_the_backoff() {
        assert_eq!(reconnect_backoff_ms(0), profile::RECONNECT_INTERVAL_MS);
        assert_eq!(reconnect_backoff_ms(1), profile::RECONNECT_INTERVAL_MS);
        assert_eq!(reconnect_backoff_ms(2), profile::RECONNECT_INTERVAL_MS * 2);
        assert_eq!(reconnect_backoff_ms(3), profile::RECONNECT_INTERVAL_MS * 4);
        assert_eq!(reconnect_backoff_ms(u32::MAX), RECONNECT_BACKOFF_CAP_MS);

        let mut runtime = TransportRuntime::new(0);
        let mut now = 0;
        let mut previous_delay = 0;
        for failure in 1..=6 {
            runtime.reconnect_attempted(now);
            runtime.disconnect(now, true);
            runtime.connect_failed(now);
            assert_eq!(runtime.connect_failures(), failure);
            let delay = reconnect_backoff_ms(failure);
            assert!(delay >= previous_delay, "backoff never shrinks");
            assert!(!runtime.reconnect_due(now + delay - 1));
            assert!(runtime.reconnect_due(now + delay));
            previous_delay = delay;
            now += delay;
        }
        assert_eq!(previous_delay, RECONNECT_BACKOFF_CAP_MS);

        // The operator asking for a reset wants an attempt now, not after the
        // backoff the wedge earned.
        runtime.request_reconnect(now);
        assert_eq!(runtime.connect_failures(), 0);
        assert!(runtime.reconnect_due(now));

        runtime.connect_failed(now);
        runtime.connect_succeeded();
        assert_eq!(runtime.connect_failures(), 0);
    }

    #[test]
    fn authoritative_state_can_return_a_ready_session_to_syncing() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        runtime.handshake_completed(1, true);
        assert_eq!(runtime.phase(), SessionPhase::Ready);
        assert!(runtime.synchronized());

        runtime.synchronization_completed(2, false);
        assert_eq!(runtime.phase(), SessionPhase::Syncing);
        assert!(!runtime.synchronized());

        runtime.synchronization_completed(3, true);
        assert_eq!(runtime.phase(), SessionPhase::Ready);
        assert!(runtime.synchronized());
    }

    #[test]
    fn synchronization_completion_interprets_transport_phase_once() {
        let mut runtime = TransportRuntime::new(0);
        runtime.transport_opened(0);
        runtime.synchronization_completed(1, false);
        assert_eq!(runtime.phase(), SessionPhase::Syncing);
        runtime.synchronization_completed(2, true);
        assert_eq!(runtime.phase(), SessionPhase::Ready);
        runtime.synchronization_completed(3, false);
        assert_eq!(runtime.phase(), SessionPhase::Syncing);
    }
}
