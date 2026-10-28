//! Shared post-handshake synchronization policy.
//!
//! Hosts drive this state machine from their own event loop. It decides which
//! QC commands are sent and when synchronization is complete; the host merely
//! performs writes and feeds observed message types back in.

use crate::commands::{self, OutboundMessage};
use crate::profile;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::state::{parse_model_repo, StateDecoder};
use prost::Message;

/// Owns the device-state lifecycle. `InitializationRuntime` below owns
/// only post-boot preset/state seeding; this runtime owns every protocol gate
/// which must complete before that seeding may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStartupPhase {
    /// Waiting for the `ResetCommsBuffers` reply that echoes this host's session id.
    SessionValidating,
    /// Waiting for the device's `Version` update, which is checked for device type and controller compatibility.
    VersionValidating,
    /// Version accepted (or an in-session `Connection(false)` rebuild began); waiting for the host to call `begin_building`.
    Disconnected,
    /// Waiting for the Model Repository after the building writes were sent.
    Building,
    /// Waiting for the module statistics update after the initializing writes were sent.
    Initializing,
    /// Waiting for the `Updater` update after the boot writes were sent.
    Booting,
    /// Staged startup finished; the connection stays active to answer `Version` reads and handle later rebuilds.
    Connected,
    /// The device was rejected (unsupported device, incompatible controller version or session mismatch); terminal.
    Invalid,
    /// Startup failed on a malformed or unusable reply, or on a rebuild loop; terminal.
    Failed,
}

impl DeviceStartupPhase {
    /// Returns the lower-camel-case name of the phase, such as `"sessionValidating"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionValidating => "sessionValidating",
            Self::VersionValidating => "versionValidating",
            Self::Disconnected => "disconnected",
            Self::Building => "building",
            Self::Initializing => "initializing",
            Self::Booting => "booting",
            Self::Connected => "connected",
            Self::Invalid => "invalid",
            Self::Failed => "failed",
        }
    }
}

/// Reason a [`DeviceStartupRuntime`] ended in the `Invalid` or `Failed` phase.
///
/// The discriminants are stable `u8` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceStartupError {
    /// Unspecified error. The startup runtime in this module never produces it.
    Undefined = 1,
    /// A reply could not be decoded, or exceeded the maximum frame size.
    ParseMessageFailure = 2,
    /// Invalid cloud endpoint. The startup runtime in this module never produces it.
    InvalidCloudEndpoint = 3,
    /// A reply arrived with an unexpected action, or `begin_building` was called outside the `Disconnected` phase.
    StateError = 4,
    /// The Model Repository reply could not be parsed or listed no models.
    ModelRepositoryDataError = 5,
    /// The module statistics reply was not an update, was malformed, or was empty.
    ModuleStatisticsDataError = 6,
    /// The `ResetCommsBuffers` reply carried a different session id than this host's.
    SessionMismatch = 7,
    /// The `Version` reply reported a device type other than the Quad Cortex.
    UnsupportedDevice = 8,
    /// The device reported the controller version as invalid, or answered the `Version` read with a generic error code of 0 or 1.
    IncompatibleControllerVersion = 9,
    /// The device kept announcing `Connection(false)` while this host rebuilt
    /// the session, so staged startup never reached `Connected`. See
    /// [`MAX_STARTUP_REBUILDS`].
    RebuildLoop = 10,
}

/// How many times staged startup may restart itself on a device-announced
/// `Connection(false)` before it has reached `Connected` once.
///
/// Every genuine rebuild re-arms `READY_WAIT_TIMEOUT_MS`, so without a cap a
/// device that answers each rebuild with another `Connection(false)` - seen
/// 2026-10-03, one cycle per ~1.1 s for 13 minutes, the unit never answering
/// the Model Repo read - keeps pushing the deadline away forever and the host
/// reports `handshaking` for as long as the process lives. A healthy boot
/// needs at most one rebuild. Exceeding the cap fails the session with
/// [`DeviceStartupError::RebuildLoop`], which tears the transport down so the
/// host redoes the whole handshake (fresh HID open, fresh ResetCommsBuffers).
///
/// Host policy (not a Cortex Control frame). The 2026-10-03 loop above is a
/// dated observation recorded in this comment only (no artifact retained in
/// the repository); the cap of 3 itself is BELIEVED.
pub const MAX_STARTUP_REBUILDS: u32 = 3;

impl DeviceStartupError {
    /// Returns the variant name, such as `"RebuildLoop"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undefined => "Undefined",
            Self::ParseMessageFailure => "ParseMessageFailure",
            Self::InvalidCloudEndpoint => "InvalidCloudEndpoint",
            Self::StateError => "StateError",
            Self::ModelRepositoryDataError => "ModelRepositoryDataError",
            Self::ModuleStatisticsDataError => "ModuleStatisticsDataError",
            Self::SessionMismatch => "SessionMismatch",
            Self::UnsupportedDevice => "UnsupportedDevice",
            Self::IncompatibleControllerVersion => "IncompatibleControllerVersion",
            Self::RebuildLoop => "RebuildLoop",
        }
    }
}

/// Content-free reason a type-35 startup reply was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleStatisticsFailureKind {
    /// The protobuf envelope could not be decoded.
    DecodeFailure,
    /// The decoded action was not UPDATE.
    NonUpdateAction,
    /// All four decoded statistics collections were empty.
    EmptyStatistics,
}

/// Bounded metadata about a rejected module-statistics reply. Payload bytes,
/// protobuf error strings and collection contents are deliberately not retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleStatisticsFailure {
    /// Which existing validation gate rejected the reply.
    pub kind: ModuleStatisticsFailureKind,
    /// Number of bytes submitted to the protobuf decoder.
    pub payload_bytes: usize,
    /// Decoded numeric action, absent when decoding failed.
    pub action: Option<i32>,
    /// Counts of stats, core_stats, size_relations and valid_id, in that order.
    /// Absent when decoding failed.
    pub collection_counts: Option<[usize; 4]>,
}

impl std::fmt::Display for ModuleStatisticsFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.kind {
            ModuleStatisticsFailureKind::DecodeFailure => "decode-failure",
            ModuleStatisticsFailureKind::NonUpdateAction => "non-update-action",
            ModuleStatisticsFailureKind::EmptyStatistics => "empty-statistics",
        };
        write!(
            formatter,
            "type 35 {reason}; payloadBytes={}",
            self.payload_bytes
        )?;
        if let Some(action) = self.action {
            write!(formatter, "; action={action}")?;
        }
        if let Some([stats, core_stats, size_relations, valid_id]) = self.collection_counts {
            write!(formatter, "; stats={stats}, coreStats={core_stats}, sizeRelations={size_relations}, validId={valid_id}")?;
        }
        Ok(())
    }
}

/// Instruction returned by [`DeviceStartupRuntime`] telling the host what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStartupAction {
    /// Nothing to send; keep waiting for further messages.
    Wait,
    /// Write these messages to the device.
    Send(Vec<OutboundMessage>),
    /// Send the Disconnected-state writes, then let the host confirm that its
    /// external dependencies are ready before calling `begin_building`.
    SendThenBuild(Vec<OutboundMessage>),
    /// Staged startup is complete.
    Connected,
    /// The device was rejected; the runtime is now in the `Invalid` phase.
    Invalid(DeviceStartupError),
    /// Startup failed; the runtime is now in the `Failed` phase.
    Failed(DeviceStartupError),
}

/// Options for [`DeviceStartupRuntime`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceStartupOptions {
    /// File.short_listing is obtained from the device/controller interface at
    /// runtime. Quad Cortex uses false; the option keeps that dynamic field
    /// explicit for other supported device types.
    pub short_file_listing: bool,
}

/// Deterministic, staged startup policy: startup proceeds through a fixed
/// sequence of stages in order.
#[derive(Debug)]
pub struct DeviceStartupRuntime {
    phase: DeviceStartupPhase,
    session_id: String,
    next_request_id: u64,
    deadline_ms: u64,
    options: DeviceStartupOptions,
    error: Option<DeviceStartupError>,
    failure_phase: Option<DeviceStartupPhase>,
    module_statistics_failure: Option<ModuleStatisticsFailure>,
    seed_evidence: SeedEvidence,
    /// Device-announced rebuilds since the session was last `Connected` (or
    /// since it started). Bounded by [`MAX_STARTUP_REBUILDS`].
    startup_rebuilds: u32,
}

impl DeviceStartupRuntime {
    /// Starts staged startup at time zero.
    ///
    /// Returns the runtime and the `ResetCommsBuffers` message the host must send first. `request_id` is used for that message; later correlation ids follow it in sequence.
    pub fn start(request_id: u64, session_id: impl Into<String>) -> (Self, Vec<OutboundMessage>) {
        Self::start_at(request_id, session_id, 0)
    }

    /// Like [`start`](Self::start), but takes the current time in milliseconds so the readiness deadline is measured from `now_ms`.
    pub fn start_at(
        request_id: u64,
        session_id: impl Into<String>,
        now_ms: u64,
    ) -> (Self, Vec<OutboundMessage>) {
        Self::start_with_options_at(
            request_id,
            session_id,
            now_ms,
            DeviceStartupOptions::default(),
        )
    }

    /// Like [`start`](Self::start), with explicit [`DeviceStartupOptions`].
    pub fn start_with_options(
        request_id: u64,
        session_id: impl Into<String>,
        options: DeviceStartupOptions,
    ) -> (Self, Vec<OutboundMessage>) {
        Self::start_with_options_at(request_id, session_id, 0, options)
    }

    /// Like [`start_at`](Self::start_at), with explicit [`DeviceStartupOptions`].
    pub fn start_with_options_at(
        request_id: u64,
        session_id: impl Into<String>,
        now_ms: u64,
        options: DeviceStartupOptions,
    ) -> (Self, Vec<OutboundMessage>) {
        let session_id = session_id.into();
        (
            Self {
                phase: DeviceStartupPhase::SessionValidating,
                session_id: session_id.clone(),
                next_request_id: request_id.wrapping_add(1),
                deadline_ms: now_ms.saturating_add(profile::READY_WAIT_TIMEOUT_MS),
                options,
                error: None,
                failure_phase: None,
                module_statistics_failure: None,
                seed_evidence: SeedEvidence::default(),
                startup_rebuilds: 0,
            },
            vec![commands::reset_comms(request_id, session_id)],
        )
    }

    /// Returns the current startup phase.
    pub fn phase(&self) -> DeviceStartupPhase {
        self.phase
    }

    /// Returns the error that ended startup, or `None` while no failure has occurred.
    pub fn error(&self) -> Option<DeviceStartupError> {
        self.error
    }

    /// Startup phase at the first rejection, before it became Failed or Invalid.
    pub fn failure_phase(&self) -> Option<DeviceStartupPhase> {
        self.failure_phase
    }

    /// Content-free metadata for the type-35 reply that failed startup.
    pub fn module_statistics_failure(&self) -> Option<ModuleStatisticsFailure> {
        self.module_statistics_failure
    }

    /// Rebuilds the device has forced since this session was last `Connected`.
    pub fn startup_rebuilds(&self) -> u32 {
        self.startup_rebuilds
    }

    /// The controller remains active after Connected so it can answer Version
    /// reads and handle an in-session Connection(false) rebuild.
    pub fn is_active(&self) -> bool {
        !matches!(
            self.phase,
            DeviceStartupPhase::Invalid | DeviceStartupPhase::Failed
        )
    }

    /// Returns `true` once the `Updater` update has been observed and the runtime is in the `Connected` phase.
    pub fn is_connected(&self) -> bool {
        self.phase == DeviceStartupPhase::Connected
    }

    /// Whether staged protocol startup exceeded the one generated readiness
    /// budget. Connected and terminal states no longer own a startup timer.
    pub fn timed_out(&self, now_ms: u64) -> bool {
        matches!(
            self.phase,
            DeviceStartupPhase::SessionValidating
                | DeviceStartupPhase::VersionValidating
                | DeviceStartupPhase::Disconnected
                | DeviceStartupPhase::Building
                | DeviceStartupPhase::Initializing
                | DeviceStartupPhase::Booting
        ) && now_ms >= self.deadline_ms
    }

    /// Reserve the next correlation id from the same sequence used by staged
    /// startup and post-boot reads. Native adapters must not maintain a second
    /// counter which can reuse an in-flight lifecycle id.
    pub fn reserve_request_id(&mut self) -> u64 {
        self.take_request_id()
    }

    /// Begin the bounded live-state seed with every message already observed
    /// during staged startup. This keeps an early preset/scene/mode push from
    /// being forgotten just because the final Updater gate arrived later.
    pub fn post_boot_initialization(
        &mut self,
        now_ms: u64,
    ) -> Result<InitializationRuntime, DeviceStartupError> {
        if !self.is_connected() {
            return Err(DeviceStartupError::StateError);
        }
        let request_id = self.take_request_id();
        let mut initialization = InitializationRuntime::start_post_boot(now_ms, request_id);
        initialization.seed_evidence = self.seed_evidence.clone();
        initialization.synchronized = initialization.seed_evidence.preset;
        Ok(initialization)
    }

    /// The external connection controller enters Building only after its
    /// dependencies are ready. Keeping this edge explicit prevents a valid
    /// Version response from collapsing every startup write into one burst.
    pub fn begin_building(&mut self) -> DeviceStartupAction {
        if self.phase != DeviceStartupPhase::Disconnected {
            return self.fail(DeviceStartupError::StateError);
        }
        self.phase = DeviceStartupPhase::Building;
        DeviceStartupAction::Send(commands::building_initialization())
    }

    /// Feeds one inbound device message into the state machine and returns the next action.
    ///
    /// `message_type` is the protocol message type, `payload` its encoded body and `now_ms` the current time, used to re-arm the readiness deadline on a rebuild. Messages that are not relevant to the current phase return [`DeviceStartupAction::Wait`]. A payload larger than the type's delivered-payload bound ([`crate::compression::max_delivered_payload_bytes_for`]) fails with [`DeviceStartupError::ParseMessageFailure`].
    pub fn observe(
        &mut self,
        message_type: u16,
        payload: &[u8],
        now_ms: u64,
    ) -> DeviceStartupAction {
        // The payload is already inflated when its envelope was compressed, so
        // it is bounded by that type's inflation limit, not the raw ceiling.
        if payload.len() > crate::compression::max_delivered_payload_bytes_for(message_type) {
            return self.fail(DeviceStartupError::ParseMessageFailure);
        }
        // Once connected, the post-boot InitializationRuntime is the sole
        // readiness reducer. Avoid decoding every live scene/tempo/state push
        // a second time here; Connection(false) clears the epoch before the
        // next startup seed is collected.
        if self.phase != DeviceStartupPhase::Connected {
            self.seed_evidence.observe_message(message_type, payload);
        }

        if message_type == profile::MESSAGE_TYPE_CONNECTION && self.handles_connection() {
            let message: pa::ConnectionMessage =
                match self.decode_or_fail(payload, DeviceStartupError::ParseMessageFailure) {
                    Ok(message) => message,
                    Err(action) => return action,
                };
            if matches!(
                message.connected,
                Some(pa::connection_message::Connected::Connected(false))
            ) {
                // A rebuild is already in flight: this phase was entered by a
                // SendThenBuild whose `begin_building` the host has not made
                // yet, because it is still writing the entry messages. Each
                // SendThenBuild obliges the host to call `begin_building`
                // exactly once, so issuing a second one here would produce a
                // second call which lands on Building - StateError, Failed, and
                // a torn-down transport with the unit stranded mid-boot. The
                // seed is already discarded and the rebuild under way re-seeds
                // this epoch, so there is nothing left to do.
                // The same applies before the reset gate has been answered. A
                // Connection(false) arriving in SessionValidating announces the
                // PREVIOUS host's session, which this handshake is replacing -
                // there is no session of ours to rebuild yet. A host that exits
                // without sending connection(false) (a kill, a crash) leaves the
                // QC holding that stale session, and it then reports it ~9 ms
                // before the ResetCommsBuffers echo. Rebuilding on it skips
                // VersionValidating, so the echo that arrives ~40 ms later is
                // discarded by the phase match below, and QcUsb::connect - whose
                // only exit is VersionValidating - strands the unit in Building
                // until HANDSHAKE_TIMEOUT_MS, six attempts later. This occurs
                // on the first connect after any host exits uncleanly.
                if matches!(
                    self.phase,
                    DeviceStartupPhase::Disconnected | DeviceStartupPhase::SessionValidating
                ) {
                    return DeviceStartupAction::Wait;
                }
                // A device that answers every rebuild with another
                // Connection(false) would otherwise re-arm the deadline below
                // for ever; see MAX_STARTUP_REBUILDS.
                self.startup_rebuilds += 1;
                if self.startup_rebuilds > MAX_STARTUP_REBUILDS {
                    return self.fail(DeviceStartupError::RebuildLoop);
                }
                // A fresh in-session build must prove a fresh authoritative
                // seed. Observations from the prior Connected epoch cannot
                // make the rebuilt session ready.
                self.seed_evidence = SeedEvidence::default();
                self.phase = DeviceStartupPhase::Disconnected;
                // A genuine disconnect starts a new bounded boot attempt.
                // Repeated notifications while Disconnected return
                // above and cannot postpone this deadline indefinitely.
                self.deadline_ms = now_ms.saturating_add(profile::READY_WAIT_TIMEOUT_MS);
                self.error = None;
                let request_id = self.take_request_id();
                return DeviceStartupAction::SendThenBuild(
                    commands::disconnect_initialization_exact(request_id),
                );
            }
            return DeviceStartupAction::Wait;
        }

        match (self.phase, message_type) {
            (DeviceStartupPhase::SessionValidating, profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS) => {
                self.observe_reset(payload)
            }
            (DeviceStartupPhase::VersionValidating, profile::MESSAGE_TYPE_VERSION) => {
                self.observe_version(payload)
            }
            (DeviceStartupPhase::VersionValidating, profile::MESSAGE_TYPE_GENERIC_ERROR) => {
                self.observe_version_error(payload)
            }
            (DeviceStartupPhase::Building, profile::MESSAGE_TYPE_MODEL_REPO) => {
                self.observe_model_repo(payload)
            }
            (DeviceStartupPhase::Initializing, profile::MESSAGE_TYPE_MODULE_STATS) => {
                self.observe_module_stats(payload)
            }
            (DeviceStartupPhase::Booting, profile::MESSAGE_TYPE_UPDATER) => {
                self.observe_updater(payload)
            }
            // In every state which dispatches Version, answer a device READ
            // with the controller version without treating it as validation.
            (
                DeviceStartupPhase::Building
                | DeviceStartupPhase::Initializing
                | DeviceStartupPhase::Booting
                | DeviceStartupPhase::Connected
                | DeviceStartupPhase::Disconnected,
                profile::MESSAGE_TYPE_VERSION,
            ) => self.answer_version_read(payload),
            _ => DeviceStartupAction::Wait,
        }
    }

    /// Active states dispatch Connection notices. Disconnected already owns
    /// its one pending transition; a repeated notice cannot enqueue another.
    fn handles_connection(&self) -> bool {
        matches!(
            self.phase,
            DeviceStartupPhase::SessionValidating
                | DeviceStartupPhase::VersionValidating
                | DeviceStartupPhase::Disconnected
                | DeviceStartupPhase::Building
                | DeviceStartupPhase::Initializing
                | DeviceStartupPhase::Booting
                | DeviceStartupPhase::Connected
        )
    }

    fn observe_reset(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let message: pa::ResetCommsBuffersMessage =
            match self.decode_or_fail(payload, DeviceStartupError::ParseMessageFailure) {
                Ok(message) => message,
                Err(action) => return action,
            };
        let session_id = message.session_id.map(|value| match value {
            pa::reset_comms_buffers_message::SessionId::SessionId(value) => value,
        });
        // The opaque session id is the correctness gate here. Some firmware
        // does not echo optional request ids consistently, so do not invent
        // a stronger requirement than what is actually needed here.
        if session_id.as_deref() != Some(&self.session_id) {
            return self.invalidate(DeviceStartupError::SessionMismatch);
        }
        self.phase = DeviceStartupPhase::VersionValidating;
        DeviceStartupAction::Send(vec![commands::read_version()])
    }

    fn observe_version(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let message: pa::VersionMessage =
            match self.decode_or_fail(payload, DeviceStartupError::ParseMessageFailure) {
                Ok(message) => message,
                Err(action) => return action,
            };
        if message.action == pa::message_action::Enum::Read as i32 {
            return DeviceStartupAction::Send(vec![commands::version_hello()]);
        }
        if message.action != pa::message_action::Enum::Update as i32 {
            return self.fail(DeviceStartupError::StateError);
        }
        let device_type = message.device_type.map(|value| match value {
            pa::version_message::DeviceTypeOneOf::DeviceType(value) => value,
        });
        if device_type != Some(pa::version_message::DeviceType::Qc as i32) {
            return self.invalidate(DeviceStartupError::UnsupportedDevice);
        }
        let version_valid = message
            .cortex_control_version_valid
            .map(|value| match value {
                pa::version_message::CortexControlVersionValid::CortexControlVersionValid(
                    value,
                ) => value,
            });
        // Retail CorOS may omit this optional field on a successful Version
        // reply. Only an explicit false is a compatibility rejection.
        if version_valid == Some(false) {
            return self.invalidate(DeviceStartupError::IncompatibleControllerVersion);
        }
        self.phase = DeviceStartupPhase::Disconnected;
        self.error = None;
        let request_id = self.take_request_id();
        DeviceStartupAction::SendThenBuild(commands::disconnect_initialization_exact(request_id))
    }

    fn observe_version_error(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let message: pa::GenericErrorMessage =
            match self.decode_or_fail(payload, DeviceStartupError::ParseMessageFailure) {
                Ok(message) => message,
                Err(action) => return action,
            };
        let code = message.error_code.map(|value| match value {
            pa::generic_error_message::ErrorCodeOneOf::ErrorCode(value) => value,
        });
        if matches!(code, Some(0 | 1)) {
            self.invalidate(DeviceStartupError::IncompatibleControllerVersion)
        } else {
            DeviceStartupAction::Wait
        }
    }

    fn observe_model_repo(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let catalog = parse_model_repo(payload);
        self.observe_model_catalog(catalog.as_ref().map_err(|_| ()))
    }

    /// A native metadata worker can return the already parsed catalog. The
    /// same installability gate applies without parsing XML under a session lock.
    pub fn observe_model_catalog(
        &mut self,
        catalog: Result<&crate::state::ModelCatalog, ()>,
    ) -> DeviceStartupAction {
        if self.phase != DeviceStartupPhase::Building {
            return DeviceStartupAction::Wait;
        }
        let Ok(catalog) = catalog else {
            return self.fail(DeviceStartupError::ModelRepositoryDataError);
        };
        if catalog.model_list().audit.model_count == 0 {
            return self.fail(DeviceStartupError::ModelRepositoryDataError);
        }
        self.phase = DeviceStartupPhase::Initializing;
        self.error = None;
        DeviceStartupAction::Send(commands::module_stats_initialization())
    }

    fn observe_module_stats(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let message = match pa::ModuleStatsMessage::decode(payload) {
            Ok(message) => message,
            Err(_) => {
                self.module_statistics_failure = Some(ModuleStatisticsFailure {
                    kind: ModuleStatisticsFailureKind::DecodeFailure,
                    payload_bytes: payload.len(),
                    action: None,
                    collection_counts: None,
                });
                return self.fail(DeviceStartupError::ModuleStatisticsDataError);
            }
        };
        let reason = if message.action != pa::message_action::Enum::Update as i32 {
            Some(ModuleStatisticsFailureKind::NonUpdateAction)
        } else if message.stats.is_empty()
            && message.core_stats.is_empty()
            && message.size_relations.is_empty()
            && message.valid_id.is_empty()
        {
            Some(ModuleStatisticsFailureKind::EmptyStatistics)
        } else {
            None
        };
        if let Some(kind) = reason {
            self.module_statistics_failure = Some(ModuleStatisticsFailure {
                kind,
                payload_bytes: payload.len(),
                action: Some(message.action),
                collection_counts: Some([
                    message.stats.len(),
                    message.core_stats.len(),
                    message.size_relations.len(),
                    message.valid_id.len(),
                ]),
            });
            return self.fail(DeviceStartupError::ModuleStatisticsDataError);
        }
        self.phase = DeviceStartupPhase::Booting;
        self.error = None;
        let request_id = self.take_request_id();
        DeviceStartupAction::Send(commands::boot_initialization_exact(
            request_id,
            self.options.short_file_listing,
        ))
    }

    fn observe_updater(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let message: pa::UpdaterMessage =
            match self.decode_or_fail(payload, DeviceStartupError::ParseMessageFailure) {
                Ok(message) => message,
                Err(action) => return action,
            };
        if message.action != pa::message_action::Enum::Update as i32 {
            return self.fail(DeviceStartupError::StateError);
        }
        self.phase = DeviceStartupPhase::Connected;
        self.error = None;
        self.startup_rebuilds = 0;
        DeviceStartupAction::Connected
    }

    fn answer_version_read(&mut self, payload: &[u8]) -> DeviceStartupAction {
        let message: pa::VersionMessage =
            match self.decode_or_fail(payload, DeviceStartupError::ParseMessageFailure) {
                Ok(message) => message,
                Err(action) => return action,
            };
        if message.action == pa::message_action::Enum::Read as i32 {
            DeviceStartupAction::Send(vec![commands::version_hello()])
        } else {
            DeviceStartupAction::Wait
        }
    }

    /// Decode a correlated reply, or fail the startup runtime with `error`
    /// the same way every `observe_*`/`answer_*` handler here does on a
    /// malformed payload. Callers still need their own
    /// `match ... { Ok(message) => message, Err(action) => return action }`
    /// to return out of their own function, since that early return cannot
    /// happen inside this helper.
    fn decode_or_fail<M: Message + Default>(
        &mut self,
        payload: &[u8],
        error: DeviceStartupError,
    ) -> Result<M, DeviceStartupAction> {
        M::decode(payload).map_err(|_| self.fail(error))
    }

    fn fail(&mut self, error: DeviceStartupError) -> DeviceStartupAction {
        self.failure_phase.get_or_insert(self.phase);
        self.phase = DeviceStartupPhase::Failed;
        self.error = Some(error);
        DeviceStartupAction::Failed(error)
    }

    fn invalidate(&mut self, error: DeviceStartupError) -> DeviceStartupAction {
        self.failure_phase.get_or_insert(self.phase);
        self.phase = DeviceStartupPhase::Invalid;
        self.error = Some(error);
        DeviceStartupAction::Invalid(error)
    }

    fn take_request_id(&mut self) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        request_id
    }
}

/// The small authoritative state seed needed before the control surface can be
/// treated as coherent. Large File catalogs remain explicitly on demand.
pub const REQUIRED_SEED_TYPES: &[u16] = &[
    profile::MESSAGE_TYPE_SETLIST_POSITION,
    profile::MESSAGE_TYPE_SCENE,
    profile::MESSAGE_TYPE_MODE,
    profile::MESSAGE_TYPE_MASTER_VOLUME,
    profile::MESSAGE_TYPE_PRESET_DIRTY,
];

/// CorOS normally publishes this seed immediately after the preset. Three
/// seconds matches the proven Windows path and is now defined only here.
pub const INITIAL_SEED_TIMEOUT_MS: u64 = 3_000;

/// Semantic evidence required for a coherent live control surface. A protobuf
/// type number by itself is not evidence: malformed or empty messages must not
/// make one host ready while the other host's decoder rejects them.
#[derive(Debug, Clone, Default)]
struct SeedEvidence {
    preset: bool,
    setlist_key: bool,
    position: bool,
    scene: bool,
    mode: bool,
    master_volume: bool,
    dirty: bool,
}

impl SeedEvidence {
    fn observe_message(&mut self, message_type: u16, payload: &[u8]) {
        if message_type != profile::MESSAGE_TYPE_RECALL_PRESET
            && !REQUIRED_SEED_TYPES.contains(&message_type)
        {
            return;
        }
        let mut decoder = StateDecoder::new();
        let Ok(updates) = decoder.decode(message_type, payload) else {
            return;
        };
        for update in updates {
            match update.kind.as_str() {
                "preset" => self.preset = true,
                "position" => {
                    self.setlist_key |= update.setlist_key.is_some();
                    self.position |= update.position.is_some();
                }
                "scene" => self.scene |= update.active_scene.is_some(),
                "mode" => self.mode |= update.mode.is_some(),
                "master" => self.master_volume |= update.master_volume.is_some(),
                "dirty" => self.dirty |= update.dirty.is_some(),
                _ => {}
            }
        }
    }

    #[cfg(test)]
    fn observe_trusted_type(&mut self, message_type: u16) {
        match message_type {
            profile::MESSAGE_TYPE_RECALL_PRESET => self.preset = true,
            profile::MESSAGE_TYPE_SETLIST_POSITION => {
                self.setlist_key = true;
                self.position = true;
            }
            profile::MESSAGE_TYPE_SCENE => self.scene = true,
            profile::MESSAGE_TYPE_MODE => self.mode = true,
            profile::MESSAGE_TYPE_MASTER_VOLUME => self.master_volume = true,
            profile::MESSAGE_TYPE_PRESET_DIRTY => self.dirty = true,
            _ => {}
        }
    }

    fn satisfies_type(&self, message_type: u16) -> bool {
        match message_type {
            profile::MESSAGE_TYPE_SETLIST_POSITION => self.setlist_key && self.position,
            profile::MESSAGE_TYPE_SCENE => self.scene,
            profile::MESSAGE_TYPE_MODE => self.mode,
            profile::MESSAGE_TYPE_MASTER_VOLUME => self.master_volume,
            profile::MESSAGE_TYPE_PRESET_DIRTY => self.dirty,
            _ => false,
        }
    }

    fn complete(&self) -> bool {
        REQUIRED_SEED_TYPES
            .iter()
            .all(|message_type| self.satisfies_type(*message_type))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitializationPhase {
    InitialPreset,
    RequestedPreset,
    Seed,
    Complete,
}

/// Instruction returned by [`InitializationRuntime`] telling the host what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitializationAction {
    /// Nothing to send; keep waiting.
    Wait,
    /// Write these read requests to the device.
    Send(Vec<OutboundMessage>),
    /// The seed has finished.
    Complete {
        /// `true` when the current preset and every required seed message type
        /// were observed; `false` when the seed ended on a timeout.
        synchronized: bool,
    },
}

/// State machine for the bounded post-boot preset and live-state seed.
///
/// It waits for the device to publish the current preset, requests it if the wait times out, then reads any required seed message types that were not observed. Feed inbound messages through [`observe_message`](Self::observe_message) and call [`advance`](Self::advance) periodically.
#[derive(Debug)]
pub struct InitializationRuntime {
    phase: InitializationPhase,
    deadline_ms: u64,
    seed_evidence: SeedEvidence,
    synchronized: bool,
}

impl InitializationRuntime {
    /// Start only the bounded post-boot preset/state seed. Native hosts using
    /// [`DeviceStartupRuntime`] have already performed the staged Version,
    /// ModelRepo, ModuleStats, and boot-subscription sequence and must not send
    /// the legacy collapsed initialization burst a second time.
    ///
    /// `_reserved_request_id` is not carried into any frame: the preset request is the bare
    /// type-15 READ. The parameter stays so the request-id sequence the other startup frames
    /// follow is unchanged.
    pub fn start_post_boot(now_ms: u64, _reserved_request_id: u64) -> Self {
        Self {
            phase: InitializationPhase::InitialPreset,
            deadline_ms: now_ms.saturating_add(profile::INITIAL_SYNC_TIMEOUT_MS),
            seed_evidence: SeedEvidence::default(),
            synchronized: false,
        }
    }

    /// Records the evidence carried by one inbound message toward the preset and seed requirements.
    ///
    /// Malformed or empty payloads are ignored.
    pub fn observe_message(&mut self, message_type: u16, payload: &[u8]) {
        self.seed_evidence.observe_message(message_type, payload);
        self.synchronized = self.seed_evidence.preset;
    }

    #[cfg(test)]
    fn observe(&mut self, message_type: u16) {
        self.seed_evidence.observe_trusted_type(message_type);
        self.synchronized = self.seed_evidence.preset;
    }

    /// Returns `true` once a current preset has been observed.
    pub fn synchronized(&self) -> bool {
        self.synchronized
    }

    /// Returns `true` once the seed has finished, whether it completed or timed out.
    pub fn is_complete(&self) -> bool {
        self.phase == InitializationPhase::Complete
    }

    /// Advances the seed using the current time in milliseconds.
    ///
    /// Returns the reads to send when a stage begins, [`InitializationAction::Wait`] while a stage is pending, and [`InitializationAction::Complete`] once the seed is done or its deadline passed.
    pub fn advance(&mut self, now_ms: u64) -> InitializationAction {
        match self.phase {
            InitializationPhase::InitialPreset if self.synchronized => self.begin_seed(now_ms),
            InitializationPhase::InitialPreset if now_ms >= self.deadline_ms => {
                self.phase = InitializationPhase::RequestedPreset;
                self.deadline_ms = now_ms.saturating_add(profile::PRESET_SYNC_TIMEOUT_MS);
                InitializationAction::Send(vec![commands::read_current_preset()])
            }
            InitializationPhase::RequestedPreset
                if self.synchronized || now_ms >= self.deadline_ms =>
            {
                self.begin_seed(now_ms)
            }
            InitializationPhase::Seed if self.seed_complete() || now_ms >= self.deadline_ms => {
                self.phase = InitializationPhase::Complete;
                InitializationAction::Complete {
                    synchronized: self.ready(),
                }
            }
            InitializationPhase::Complete => InitializationAction::Complete {
                synchronized: self.ready(),
            },
            _ => InitializationAction::Wait,
        }
    }

    fn begin_seed(&mut self, now_ms: u64) -> InitializationAction {
        let messages = REQUIRED_SEED_TYPES
            .iter()
            .copied()
            .filter(|message_type| !self.seed_evidence.satisfies_type(*message_type))
            .map(|message_type| {
                if message_type == profile::MESSAGE_TYPE_SETLIST_POSITION {
                    commands::read_setlist_position()
                } else {
                    commands::read(message_type)
                }
            })
            .collect::<Vec<_>>();
        if messages.is_empty() {
            self.phase = InitializationPhase::Complete;
            InitializationAction::Complete {
                synchronized: self.synchronized,
            }
        } else {
            self.phase = InitializationPhase::Seed;
            self.deadline_ms = now_ms.saturating_add(INITIAL_SEED_TIMEOUT_MS);
            InitializationAction::Send(messages)
        }
    }

    fn seed_complete(&self) -> bool {
        self.seed_evidence.complete()
    }

    fn ready(&self) -> bool {
        self.synchronized && self.seed_complete()
    }
}

/// One protocol lifecycle epoch, from the reset handshake through staged boot
/// and the authoritative live-state seed. Keeping startup and initialization
/// together prevents native hosts from retaining seed evidence across an
/// in-session `Connection(false)` rebuild or releasing it at different times.
#[derive(Debug)]
pub struct DeviceLifecycleRuntime {
    startup: DeviceStartupRuntime,
    initialization: Option<InitializationRuntime>,
}

impl DeviceLifecycleRuntime {
    /// Starts a lifecycle at the given time; see [`DeviceStartupRuntime::start_at`].
    ///
    /// Returns the runtime and the first message to send.
    pub fn start_at(
        request_id: u64,
        session_id: impl Into<String>,
        now_ms: u64,
    ) -> (Self, Vec<OutboundMessage>) {
        let (startup, messages) = DeviceStartupRuntime::start_at(request_id, session_id, now_ms);
        (
            Self {
                startup,
                initialization: None,
            },
            messages,
        )
    }

    /// See [`DeviceStartupRuntime::startup_rebuilds`].
    pub fn startup_rebuilds(&self) -> u32 {
        self.startup.startup_rebuilds()
    }

    /// Returns the current startup phase.
    pub fn phase(&self) -> DeviceStartupPhase {
        self.startup.phase()
    }

    /// See [`DeviceStartupRuntime::failure_phase`].
    pub fn failure_phase(&self) -> Option<DeviceStartupPhase> {
        self.startup.failure_phase()
    }

    /// See [`DeviceStartupRuntime::module_statistics_failure`].
    pub fn module_statistics_failure(&self) -> Option<ModuleStatisticsFailure> {
        self.startup.module_statistics_failure()
    }

    /// Returns `false` once startup has become `Invalid` or `Failed`.
    pub fn is_active(&self) -> bool {
        self.startup.is_active()
    }

    /// Returns `true` once staged startup has reached the `Connected` phase.
    pub fn is_connected(&self) -> bool {
        self.startup.is_connected()
    }

    /// Returns `true` if staged startup, which is not yet connected, has run past its readiness deadline at `now_ms`.
    pub fn timed_out(&self, now_ms: u64) -> bool {
        self.startup.timed_out(now_ms)
    }

    /// Reserves the next correlation id from the shared lifecycle sequence.
    pub fn reserve_request_id(&mut self) -> u64 {
        self.startup.reserve_request_id()
    }

    /// Stamp File mutations according to the final-sender policy.
    ///
    /// Command builders intentionally remain pure protocol-shape builders. The
    /// request id belongs to the live device lifecycle so Windows and Android
    /// draw it from the same sequence as their correlated reads. Existing ids
    /// are preserved, making this safe at the final-send boundary.
    pub fn stamp_file_mutation_request_id(
        &mut self,
        message: &mut OutboundMessage,
    ) -> Result<Option<u64>, prost::DecodeError> {
        if message.message_type != profile::MESSAGE_TYPE_FILE {
            return Ok(None);
        }
        let mut file = pa::FileMessage::decode(message.payload.as_slice())?;
        // Only these six File actions assign a request id; readFolders is a
        // non-assigning read and never does. Do not extrapolate this policy
        // to the schema's unused File UPLOAD/DOWNLOAD values or unknown
        // future values.
        let assigns_request_id = matches!(
            pa::message_action::Enum::try_from(file.action),
            Ok(pa::message_action::Enum::Create
                | pa::message_action::Enum::Update
                | pa::message_action::Enum::Delete
                | pa::message_action::Enum::Move
                | pa::message_action::Enum::Copy
                | pa::message_action::Enum::Swap)
        );
        if !assigns_request_id {
            return Ok(None);
        }
        if let Some(pa::file_message::RequestId::RequestId(request_id)) = file.request_id {
            return Ok(Some(request_id));
        }
        let request_id = self.reserve_request_id();
        file.request_id = Some(pa::file_message::RequestId::RequestId(request_id));
        message.payload = file.encode_to_vec();
        Ok(Some(request_id))
    }

    /// Feed one decoded device message through both layers in the canonical
    /// order. A rebuild invalidates the previous seed atomically.
    pub fn observe(
        &mut self,
        message_type: u16,
        payload: &[u8],
        now_ms: u64,
    ) -> DeviceStartupAction {
        if let Some(initialization) = self.initialization.as_mut() {
            initialization.observe_message(message_type, payload);
        }
        let action = self.startup.observe(message_type, payload, now_ms);
        if matches!(action, DeviceStartupAction::SendThenBuild(_)) {
            self.initialization = None;
        }
        action
    }

    /// Moves from `Disconnected` to `Building`; see [`DeviceStartupRuntime::begin_building`].
    pub fn begin_building(&mut self) -> DeviceStartupAction {
        self.startup.begin_building()
    }

    /// Supplies the parsed model catalog for the `Building` phase; see [`DeviceStartupRuntime::observe_model_catalog`].
    pub fn observe_model_catalog(
        &mut self,
        catalog: Result<&crate::state::ModelCatalog, ()>,
    ) -> DeviceStartupAction {
        self.startup.observe_model_catalog(catalog)
    }

    /// Begins the post-boot state seed once startup is `Connected`.
    ///
    /// Reserves a request id, starts the seed with every message already observed, and returns the `SystemTimeSync` message (stamped with `unix_time_ms`) that the host must send before issuing the first seed read. Fails with [`DeviceStartupError::StateError`] if startup is not connected or a seed is already running.
    pub fn start_post_boot_initialization(
        &mut self,
        now_ms: u64,
        unix_time_ms: u64,
    ) -> Result<OutboundMessage, DeviceStartupError> {
        if !self.startup.is_connected() || self.initialization.is_some() {
            return Err(DeviceStartupError::StateError);
        }
        // The desktop sender reserves SystemTimeSync's id before the first
        // post-boot state read. Keep this allocation here so every native host
        // observes one ordering and cannot accidentally send an id-less sync.
        let request_id = self.startup.reserve_request_id();
        self.initialization = Some(self.startup.post_boot_initialization(now_ms)?);
        Ok(commands::sync_system_time(request_id, unix_time_ms))
    }

    /// Roll back the staged seed when the host could not write its preceding
    /// SystemTimeSync frame. Reserved ids remain consumed and monotonic.
    pub fn abort_post_boot_initialization(&mut self) {
        self.initialization = None;
    }

    /// Advances the post-boot seed, if one is running, and returns the resulting action.
    ///
    /// Returns [`InitializationAction::Wait`] when no seed is active. The seed is dropped once it completes as synchronized.
    pub fn advance_initialization(&mut self, now_ms: u64) -> InitializationAction {
        let action = self
            .initialization
            .as_mut()
            .map(|initialization| initialization.advance(now_ms))
            .unwrap_or(InitializationAction::Wait);
        if matches!(
            action,
            InitializationAction::Complete { synchronized: true }
        ) {
            self.initialization = None;
        }
        action
    }

    #[cfg(test)]
    fn has_initialization(&self) -> bool {
        self.initialization.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_or_fail_returns_the_message_or_fails_the_runtime_with_the_given_error() {
        let (mut runtime, _) = DeviceStartupRuntime::start(1, "session");
        let payload = pa::ResetCommsBuffersMessage {
            request_id: Some(pa::reset_comms_buffers_message::RequestId::RequestId(1)),
            session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                "session".into(),
            )),
        }
        .encode_to_vec();
        let decoded: pa::ResetCommsBuffersMessage = runtime
            .decode_or_fail(&payload, DeviceStartupError::ParseMessageFailure)
            .expect("valid payload must decode");
        assert_eq!(
            decoded.session_id,
            Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                "session".into()
            ))
        );
        // decode_or_fail must not itself mutate startup phase/error on success;
        // only the enclosing observe_* handler's own subsequent logic does.
        assert_eq!(runtime.phase(), DeviceStartupPhase::SessionValidating);

        let (mut runtime, _) = DeviceStartupRuntime::start(1, "session");
        let failure = runtime
            .decode_or_fail::<pa::ResetCommsBuffersMessage>(
                &[0xff],
                DeviceStartupError::ModuleStatisticsDataError,
            )
            .expect_err("malformed payload must fail with the given error");
        assert_eq!(
            failure,
            DeviceStartupAction::Failed(DeviceStartupError::ModuleStatisticsDataError)
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Failed);
    }

    fn reset_reply(request_id: u64, session_id: &str) -> Vec<u8> {
        pa::ResetCommsBuffersMessage {
            request_id: Some(pa::reset_comms_buffers_message::RequestId::RequestId(
                request_id,
            )),
            session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                session_id.into(),
            )),
        }
        .encode_to_vec()
    }

    fn valid_version() -> Vec<u8> {
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

    fn valid_model_repo() -> Vec<u8> {
        pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(
                pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                    br#"<Models><Category name="Drive"><Model id="1" name="Test" /></Category></Models>"#
                        .to_vec(),
                ),
            ),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn valid_preset() -> Vec<u8> {
        pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn enter_building(runtime: &mut DeviceStartupRuntime) {
        assert!(matches!(
            runtime.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(7, "session")
            , 0),
            DeviceStartupAction::Send(messages)
                if messages == vec![commands::read_version()]
        ));
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_VERSION, &valid_version(), 0),
            DeviceStartupAction::SendThenBuild(messages)
                if messages == commands::disconnect_initialization_exact(8)
        ));
        assert!(matches!(
            runtime.begin_building(),
            DeviceStartupAction::Send(messages)
                if messages == commands::building_initialization()
        ));
    }

    #[test]
    fn parsed_catalog_failure_matches_the_wire_failure_only_during_building() {
        let (mut wire, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        let (mut parsed, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        assert_eq!(
            parsed.observe_model_catalog(Err(())),
            DeviceStartupAction::Wait
        );
        enter_building(&mut wire);
        enter_building(&mut parsed);
        assert_eq!(
            wire.observe(profile::MESSAGE_TYPE_MODEL_REPO, &[0xff], 1),
            DeviceStartupAction::Failed(DeviceStartupError::ModelRepositoryDataError)
        );
        assert_eq!(
            parsed.observe_model_catalog(Err(())),
            DeviceStartupAction::Failed(DeviceStartupError::ModelRepositoryDataError)
        );
        assert_eq!(parsed.phase(), wire.phase());
    }

    #[test]
    fn parsed_catalog_uses_the_same_startup_gate_and_exact_commands() {
        let (mut wire, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        let (mut parsed, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        enter_building(&mut wire);
        enter_building(&mut parsed);
        let payload = valid_model_repo();
        let catalog = parse_model_repo(&payload).unwrap();
        let expected = wire.observe(profile::MESSAGE_TYPE_MODEL_REPO, &payload, 1);
        assert_eq!(
            expected,
            DeviceStartupAction::Send(commands::module_stats_initialization())
        );
        assert_eq!(parsed.observe_model_catalog(Ok(&catalog)), expected);
        assert_eq!(parsed.phase(), DeviceStartupPhase::Initializing);
        assert_eq!(
            parsed.observe_model_catalog(Ok(&catalog)),
            DeviceStartupAction::Wait
        );
    }

    fn disconnected_frame() -> Vec<u8> {
        pa::ConnectionMessage {
            connected: Some(pa::connection_message::Connected::Connected(false)),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn valid_module_stats() -> Vec<u8> {
        pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn valid_updater() -> Vec<u8> {
        pa::UpdaterMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn module_stats_diagnostics_distinguish_failures_without_retaining_content() {
        let malformed = b"\xffprivate-payload-canary".to_vec();
        let non_update = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Read as i32,
            stats: vec![pa::ModuleStatsItem {
                id: 987_654,
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode_to_vec();
        let empty = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        for (payload, kind, action, counts) in [
            (
                malformed,
                ModuleStatisticsFailureKind::DecodeFailure,
                None,
                None,
            ),
            (
                non_update,
                ModuleStatisticsFailureKind::NonUpdateAction,
                Some(3),
                Some([1, 0, 0, 0]),
            ),
            (
                empty,
                ModuleStatisticsFailureKind::EmptyStatistics,
                Some(1),
                Some([0; 4]),
            ),
        ] {
            let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
            enter_building(&mut runtime);
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0);
            assert_eq!(runtime.phase(), DeviceStartupPhase::Initializing);
            assert_eq!(
                runtime.observe(profile::MESSAGE_TYPE_MODULE_STATS, &payload, 0),
                DeviceStartupAction::Failed(DeviceStartupError::ModuleStatisticsDataError)
            );
            let diagnostic = runtime.module_statistics_failure().unwrap();
            assert_eq!(diagnostic.kind, kind);
            assert_eq!(diagnostic.payload_bytes, payload.len());
            assert_eq!(diagnostic.action, action);
            assert_eq!(diagnostic.collection_counts, counts);
            assert_eq!(
                runtime.failure_phase(),
                Some(DeviceStartupPhase::Initializing)
            );
            assert_eq!(runtime.phase(), DeviceStartupPhase::Failed);
            let text = diagnostic.to_string();
            assert!(!text.contains("private-payload-canary"));
            assert!(!text.contains("987654"));
            // A late valid reply must neither recover the failed session nor erase its reason.
            assert_eq!(
                runtime.observe(profile::MESSAGE_TYPE_MODULE_STATS, &valid_module_stats(), 1),
                DeviceStartupAction::Wait
            );
            assert_eq!(runtime.module_statistics_failure(), Some(diagnostic));
        }
        let (fresh, _) = DeviceStartupRuntime::start(7, "another session");
        assert_eq!(fresh.module_statistics_failure(), None);
        assert_eq!(fresh.failure_phase(), None);
    }

    #[test]
    fn module_stats_diagnostics_do_not_change_nonempty_update_acceptance() {
        for message in [
            pa::ModuleStatsMessage {
                stats: vec![pa::ModuleStatsItem::default()],
                ..Default::default()
            },
            pa::ModuleStatsMessage {
                core_stats: vec![pa::ModuleStatsItem::default()],
                ..Default::default()
            },
            pa::ModuleStatsMessage {
                size_relations: vec![pa::ModuleStatsSizeRelation::default()],
                ..Default::default()
            },
            pa::ModuleStatsMessage {
                valid_id: vec![pa::ModuleStatsValidModelId::default()],
                ..Default::default()
            },
        ] {
            let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
            enter_building(&mut runtime);
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0);
            let payload = pa::ModuleStatsMessage {
                action: 1,
                ..message
            }
            .encode_to_vec();
            assert!(matches!(
                runtime.observe(35, &payload, 0),
                DeviceStartupAction::Send(_)
            ));
            assert_eq!(runtime.phase(), DeviceStartupPhase::Booting);
            assert_eq!(runtime.module_statistics_failure(), None);
            assert_eq!(runtime.failure_phase(), None);
        }
    }

    /// Drive one Building -> Initializing -> Booting -> Connected staged boot,
    /// which is what every in-session rebuild costs the player: the Booting
    /// burst re-reads the whole preset directory and resets the unit's screen.
    fn drive_boot_to_connected(runtime: &mut DeviceStartupRuntime) {
        assert_eq!(runtime.phase(), DeviceStartupPhase::Building);
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0),
            DeviceStartupAction::Send(_)
        ));
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODULE_STATS, &valid_module_stats(), 0),
            DeviceStartupAction::Send(_)
        ));
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_UPDATER, &valid_updater(), 0),
            DeviceStartupAction::Connected
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Connected);
    }

    #[test]
    fn startup_path_is_strictly_staged() {
        let (mut runtime, entry) = DeviceStartupRuntime::start(7, "session");
        assert_eq!(entry, vec![commands::reset_comms(7, "session")]);
        enter_building(&mut runtime);

        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0),
            DeviceStartupAction::Send(messages)
                if messages == commands::module_stats_initialization()
        ));
        let module_stats = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODULE_STATS, &module_stats, 0),
            DeviceStartupAction::Send(messages)
                if messages == commands::boot_initialization_exact(9, false)
        ));
        assert_eq!(runtime.phase(), DeviceStartupPhase::Booting);

        // Mere state traffic cannot bypass the final Updater gate.
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_RECALL_PRESET, &valid_preset(), 0),
            DeviceStartupAction::Wait
        );
        let updater = pa::UpdaterMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_UPDATER, &updater, 0),
            DeviceStartupAction::Connected
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Connected);
        assert!(runtime.is_active());
        assert!(runtime.is_connected());
        assert!(!runtime.timed_out(u64::MAX));

        let mut seed = runtime
            .post_boot_initialization(100)
            .expect("connected startup may begin its state seed");
        assert!(
            seed.synchronized(),
            "the preset observed before Updater is retained"
        );
        let InitializationAction::Send(messages) = seed.advance(100) else {
            panic!("missing state fields should produce a bounded seed read");
        };
        assert!(messages
            .iter()
            .all(|message| message.message_type != profile::MESSAGE_TYPE_RECALL_PRESET));
        assert_eq!(runtime.reserve_request_id(), 11);
    }

    /// End to end: a compressed File listing arrives within the raw
    /// frame ceiling, the transport inflates it past 1 MiB but within the File
    /// type's own limit, and every consumer of the inflated payload accepts it.
    /// Before the fix the startup reducer failed the whole lifecycle with
    /// ParseMessageFailure, because it held the inflated payload to the raw
    /// ceiling.
    #[test]
    fn a_compressed_file_listing_inflated_past_the_raw_ceiling_does_not_fail_startup() {
        use crate::device::state_runtime::DeviceStateRuntime;
        use crate::device::transport::TransportRuntime;
        use crate::framing;
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;

        let listing = crate::state::large_file_listing(profile::MAX_FRAME_BYTES + 1);
        assert!(listing.len() > profile::MAX_FRAME_BYTES);
        assert!(
            listing.len() <= crate::compression::max_inflated_bytes_for(profile::MESSAGE_TYPE_FILE)
        );
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&listing).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() <= profile::MAX_FRAME_BYTES);

        let mut transport = TransportRuntime::new(0);
        let mut delivered = None;
        for report in framing::encode_frame(&framing::Frame {
            message_type: u32::from(profile::MESSAGE_TYPE_FILE),
            payload: compressed,
            encrypted: false,
            compressed: true,
            device_bytes: [0, 0],
        }) {
            let mut report = report.to_vec();
            report[0] = framing::IN_REPORT_ID;
            if let Some(frame) = transport.push_report(&report).unwrap() {
                delivered = Some(frame);
            }
        }
        let frame = delivered.expect("the transport assembles and inflates the listing");
        assert_eq!(frame.message_type, profile::MESSAGE_TYPE_FILE);
        assert_eq!(frame.payload, listing);

        // The startup reducer, during Building and during the boot burst.
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        enter_building(&mut runtime);
        assert_eq!(
            runtime.observe(frame.message_type, &frame.payload, 0),
            DeviceStartupAction::Wait
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Building);
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0),
            DeviceStartupAction::Send(_)
        ));
        let module_stats = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODULE_STATS, &module_stats, 0),
            DeviceStartupAction::Send(_)
        ));
        assert_eq!(runtime.phase(), DeviceStartupPhase::Booting);
        assert_eq!(
            runtime.observe(frame.message_type, &frame.payload, 0),
            DeviceStartupAction::Wait
        );
        let updater = pa::UpdaterMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_UPDATER, &updater, 0),
            DeviceStartupAction::Connected
        );
        assert!(runtime.is_connected());

        // The post-boot seed and the retained state runtime.
        let mut seed = runtime.post_boot_initialization(100).unwrap();
        seed.observe_message(frame.message_type, &frame.payload);
        let mut state = DeviceStateRuntime::new();
        state
            .ingest(frame.message_type, &frame.payload)
            .expect("the state runtime ingests the listing");
    }

    #[test]
    fn staged_startup_timeout_uses_the_shared_ready_budget() {
        let started_at = 1_000;
        let (mut runtime, _) = DeviceStartupRuntime::start_at(7, "session", started_at);
        assert!(runtime.is_active());
        assert!(!runtime.is_connected());
        assert!(!runtime.timed_out(started_at + profile::READY_WAIT_TIMEOUT_MS - 1));
        assert!(runtime.timed_out(started_at + profile::READY_WAIT_TIMEOUT_MS));

        assert!(matches!(
            runtime.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(7, "other"),
                0
            ),
            DeviceStartupAction::Invalid(DeviceStartupError::SessionMismatch)
        ));
        assert!(!runtime.is_active());
        assert!(!runtime.is_connected());
        assert!(!runtime.timed_out(u64::MAX));
    }

    #[test]
    fn late_rebuild_gets_a_new_bounded_startup_budget() {
        let (mut runtime, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        enter_building(&mut runtime);
        drive_boot_to_connected(&mut runtime);
        let rebuild_at = profile::READY_WAIT_TIMEOUT_MS * 2;
        assert!(matches!(
            runtime.observe(
                profile::MESSAGE_TYPE_CONNECTION,
                &disconnected_frame(),
                rebuild_at
            ),
            DeviceStartupAction::SendThenBuild(_)
        ));
        assert!(!runtime.timed_out(rebuild_at));
        let deadline = rebuild_at + profile::READY_WAIT_TIMEOUT_MS;
        assert!(!runtime.timed_out(deadline - 1));
        // Repeated disconnects while the first effect is queued cannot
        // manufacture a later deadline.
        assert_eq!(
            runtime.observe(
                profile::MESSAGE_TYPE_CONNECTION,
                &disconnected_frame(),
                deadline - 1
            ),
            DeviceStartupAction::Wait
        );
        assert_eq!(
            runtime.observe(
                profile::MESSAGE_TYPE_CONNECTION,
                &disconnected_frame(),
                deadline - 1
            ),
            DeviceStartupAction::Wait
        );
        assert!(matches!(
            runtime.begin_building(),
            DeviceStartupAction::Send(_)
        ));
        assert!(runtime.timed_out(deadline));
    }

    #[test]
    fn a_device_that_answers_every_rebuild_with_another_disconnect_cannot_hold_startup_open() {
        // 2026-10-03: the unit answered each Building attempt with
        // Connection(false) roughly once a second and never replied to the
        // Model Repo read. Each notice was a "genuine disconnect" that re-armed
        // READY_WAIT_TIMEOUT_MS, so the session sat in `handshaking` for 13
        // minutes with the deadline always 35 s away.
        let (mut runtime, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        enter_building(&mut runtime);
        let mut now = 1_000;
        for rebuild in 1..=MAX_STARTUP_REBUILDS {
            assert!(
                matches!(
                    runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), now),
                    DeviceStartupAction::SendThenBuild(_)
                ),
                "rebuild {rebuild} is within the cap"
            );
            assert_eq!(runtime.startup_rebuilds(), rebuild);
            assert!(matches!(
                runtime.begin_building(),
                DeviceStartupAction::Send(_)
            ));
            now += 1_100;
        }
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), now),
            DeviceStartupAction::Failed(DeviceStartupError::RebuildLoop)
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Failed);
        assert_eq!(runtime.error(), Some(DeviceStartupError::RebuildLoop));
        assert!(!runtime.is_active());
    }

    #[test]
    fn reaching_connected_restores_the_whole_rebuild_budget() {
        let (mut runtime, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        enter_building(&mut runtime);
        for _ in 0..MAX_STARTUP_REBUILDS {
            assert!(matches!(
                runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), 10),
                DeviceStartupAction::SendThenBuild(_)
            ));
            assert!(matches!(
                runtime.begin_building(),
                DeviceStartupAction::Send(_)
            ));
        }
        drive_boot_to_connected(&mut runtime);
        assert_eq!(runtime.startup_rebuilds(), 0);
        // A later, unrelated device-initiated rebuild starts from a full budget.
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), 20),
            DeviceStartupAction::SendThenBuild(_)
        ));
        assert_eq!(runtime.startup_rebuilds(), 1);
    }

    #[test]
    fn repeated_notices_while_disconnected_do_not_spend_the_rebuild_budget() {
        let (mut runtime, _) = DeviceStartupRuntime::start_at(7, "session", 0);
        enter_building(&mut runtime);
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), 10),
            DeviceStartupAction::SendThenBuild(_)
        ));
        for _ in 0..20 {
            assert_eq!(
                runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), 11),
                DeviceStartupAction::Wait
            );
        }
        assert_eq!(runtime.startup_rebuilds(), 1);
        assert!(runtime.is_active());
    }

    #[test]
    fn startup_status_vocabulary_is_stable_and_shared() {
        assert_eq!(
            DeviceStartupPhase::SessionValidating.as_str(),
            "sessionValidating"
        );
        assert_eq!(DeviceStartupPhase::Connected.as_str(), "connected");
        assert_eq!(DeviceStartupPhase::Failed.as_str(), "failed");
        assert_eq!(
            DeviceStartupError::SessionMismatch.as_str(),
            "SessionMismatch"
        );
        assert_eq!(
            DeviceStartupError::IncompatibleControllerVersion.as_str(),
            "IncompatibleControllerVersion"
        );
    }

    #[test]
    fn reset_reply_requires_session_correlation_without_requiring_request_echo() {
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "expected");
        assert_eq!(
            runtime.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(7, "other"),
                0
            ),
            DeviceStartupAction::Invalid(DeviceStartupError::SessionMismatch)
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Invalid);

        let (mut runtime, _) = DeviceStartupRuntime::start(7, "expected");
        assert!(matches!(
            runtime.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(8, "expected")
            , 0),
            DeviceStartupAction::Send(messages) if messages == vec![commands::read_version()]
        ));
    }

    #[test]
    fn reset_device_session_reenters_a_fresh_correlated_startup_epoch() {
        let (mut lifecycle, messages) = DeviceLifecycleRuntime::start_at(41, "fresh-epoch", 100);
        assert_eq!(lifecycle.phase(), DeviceStartupPhase::SessionValidating);
        assert_eq!(messages, vec![commands::reset_comms(41, "fresh-epoch")]);
        assert!(!lifecycle.is_connected());
        assert_eq!(
            lifecycle.observe(profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS, &[0xff], 0),
            DeviceStartupAction::Failed(DeviceStartupError::ParseMessageFailure)
        );

        let (mut lifecycle, _) = DeviceLifecycleRuntime::start_at(42, "fresh-epoch", 200);
        assert_eq!(
            lifecycle.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(42, "stale-epoch"),
                0
            ),
            DeviceStartupAction::Invalid(DeviceStartupError::SessionMismatch)
        );

        let (mut lifecycle, _) = DeviceLifecycleRuntime::start_at(43, "fresh-epoch", 300);
        assert_eq!(
            lifecycle.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(999, "fresh-epoch"),
                0
            ),
            DeviceStartupAction::Send(vec![commands::read_version()])
        );
        assert_eq!(lifecycle.phase(), DeviceStartupPhase::VersionValidating);
        assert!(!lifecycle.is_connected());
    }

    #[test]
    fn decoded_startup_failures_retain_their_specific_error() {
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        assert!(matches!(
            runtime.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(7, "session"),
                0
            ),
            DeviceStartupAction::Send(_)
        ));
        let incompatible = pa::GenericErrorMessage {
            error_code: Some(pa::generic_error_message::ErrorCodeOneOf::ErrorCode(0)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_GENERIC_ERROR, &incompatible, 0),
            DeviceStartupAction::Invalid(DeviceStartupError::IncompatibleControllerVersion)
        );

        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        enter_building(&mut runtime);
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &[0xff], 0),
            DeviceStartupAction::Failed(DeviceStartupError::ModelRepositoryDataError)
        );
    }

    #[test]
    fn model_repository_gate_rejects_wrong_action_missing_payload_and_empty_catalog() {
        for payload in [
            pa::ModelRepoMessage {
                action: pa::message_action::Enum::Read as i32,
                ..Default::default()
            }
            .encode_to_vec(),
            pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                ..Default::default()
            }
            .encode_to_vec(),
            pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                        b"<Models />".to_vec(),
                    ),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        ] {
            let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
            enter_building(&mut runtime);
            assert_eq!(
                runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &payload, 0),
                DeviceStartupAction::Failed(DeviceStartupError::ModelRepositoryDataError)
            );
        }
    }

    #[test]
    fn omitted_legacy_compatibility_flag_is_not_treated_as_rejection() {
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        assert!(matches!(
            runtime.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(7, "session"),
                0
            ),
            DeviceStartupAction::Send(_)
        ));
        let legacy_version = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                pa::version_message::DeviceType::Qc as i32,
            )),
            cortex_control_version_valid: None,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_VERSION, &legacy_version, 0),
            DeviceStartupAction::SendThenBuild(_)
        ));
        assert_eq!(runtime.phase(), DeviceStartupPhase::Disconnected);
    }

    #[test]
    fn connection_false_returns_an_active_startup_to_disconnected() {
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        enter_building(&mut runtime);
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_RECALL_PRESET, &valid_preset(), 0),
            DeviceStartupAction::Wait
        );
        let disconnected = pa::ConnectionMessage {
            connected: Some(pa::connection_message::Connected::Connected(false)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected, 0),
            DeviceStartupAction::SendThenBuild(commands::disconnect_initialization_exact(9))
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::Disconnected);

        assert!(matches!(
            runtime.begin_building(),
            DeviceStartupAction::Send(_)
        ));
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0),
            DeviceStartupAction::Send(_)
        ));
        let module_stats = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            runtime.observe(profile::MESSAGE_TYPE_MODULE_STATS, &module_stats, 0),
            DeviceStartupAction::Send(_)
        ));
        let updater = pa::UpdaterMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_UPDATER, &updater, 0),
            DeviceStartupAction::Connected
        );
        let seed = runtime
            .post_boot_initialization(100)
            .expect("rebuilt startup may begin a fresh seed");
        assert!(
            !seed.synchronized(),
            "the prior connected epoch's preset must not satisfy a rebuild"
        );
    }

    #[test]
    fn file_mutation_identity_comes_from_the_live_lifecycle_once() {
        let (mut runtime, _) = DeviceLifecycleRuntime::start_at(18, "session", 0);
        let mut unrelated = commands::read_version();
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut unrelated),
            Ok(None)
        ));

        let mut folder_read = commands::read_folders(false);
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut folder_read),
            Ok(None)
        ));

        let mut move_message = commands::move_preset(
            "/media/p4/Presets/My Presets".into(),
            "/media/p4/Presets/My Presets/Numb.pb".into(),
            54,
        );
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut move_message),
            Ok(Some(19))
        ));
        let decoded = pa::FileMessage::decode(move_message.payload.as_slice()).unwrap();
        assert_eq!(
            decoded.request_id,
            Some(pa::file_message::RequestId::RequestId(19))
        );

        // Reaching another final-send boundary must preserve the transaction
        // identity and must not consume a second lifecycle id.
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut move_message),
            Ok(Some(19))
        ));
        assert_eq!(runtime.reserve_request_id(), 20);

        let mut copy_message = commands::copy_preset(
            "/media/p4/Presets/My Presets".into(),
            "/media/p4/Presets/My Presets/Numb.pb".into(),
            false,
            "/media/p4/Presets/My Presets".into(),
            55,
        );
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut copy_message),
            Ok(Some(21))
        ));
        let decoded = pa::FileMessage::decode(copy_message.payload.as_slice()).unwrap();
        assert_eq!(
            decoded.request_id,
            Some(pa::file_message::RequestId::RequestId(21))
        );
        assert_eq!(runtime.reserve_request_id(), 22);
    }

    #[test]
    fn copy_and_rename_final_wire_frames_are_the_builder_payload_plus_request_id() {
        // Builder payloads are hand-built literals (see the planner test in
        // request.rs); the final wire frame is the same bytes with the shared
        // lifecycle's `uint64 request_id` (field 2, tag 0x10) inserted between
        // the action and the type fields. Request ids 42 and 43 are 0x2a, 0x2b.
        let hex = |text: &str| -> Vec<u8> {
            (0..text.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
                .collect()
        };
        let rename_builder = hex(
            "0801180022420a162f6d656469612f70342f507265736574732f4c69766520003a260a196f70617175652d6465766963652d70726f647563742d6b65791a0752656e616d65647802",
        );
        let copy_builder = hex(
            "08051800223f0a1c2f6d656469612f70342f507265736574732f4d792050726573657473200028003a1b0a196f70617175652d6465766963652d70726f647563742d6b65792a1f0a192f6d656469612f70342f507265736574732f536372617463683a02101f",
        );

        let (mut runtime, _) = DeviceLifecycleRuntime::start_at(41, "session", 0);
        let mut rename = commands::rename_preset(
            "/media/p4/Presets/Live".into(),
            "opaque-device-product-key".into(),
            "Renamed".into(),
            2,
        );
        assert_eq!(rename.payload, rename_builder);
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut rename),
            Ok(Some(42))
        ));
        assert_eq!(
            rename.payload,
            [
                &rename_builder[..2],
                &[0x10, 0x2a][..],
                &rename_builder[2..]
            ]
            .concat()
        );

        let mut copy = commands::copy_preset(
            "/media/p4/Presets/My Presets".into(),
            "opaque-device-product-key".into(),
            false,
            "/media/p4/Presets/Scratch".into(),
            31,
        );
        assert_eq!(copy.payload, copy_builder);
        assert!(matches!(
            runtime.stamp_file_mutation_request_id(&mut copy),
            Ok(Some(43))
        ));
        assert_eq!(
            copy.payload,
            [&copy_builder[..2], &[0x10, 0x2b][..], &copy_builder[2..]].concat()
        );
    }

    #[test]
    fn request_identity_wraps_like_the_binary_allocator() {
        let (mut runtime, _) = DeviceLifecycleRuntime::start_at(u64::MAX, "session", 0);
        assert_eq!(runtime.reserve_request_id(), 0);
        assert_eq!(runtime.reserve_request_id(), 1);
    }

    #[test]
    fn every_binary_file_mutation_action_is_stamped_and_reads_are_not() {
        let (mut runtime, _) = DeviceLifecycleRuntime::start_at(100, "session", 0);
        let mutation_actions = [
            pa::message_action::Enum::Create,
            pa::message_action::Enum::Update,
            pa::message_action::Enum::Delete,
            pa::message_action::Enum::Move,
            pa::message_action::Enum::Copy,
            pa::message_action::Enum::Swap,
        ];

        for (offset, action) in mutation_actions.into_iter().enumerate() {
            let mut message = OutboundMessage {
                message_type: profile::MESSAGE_TYPE_FILE,
                payload: pa::FileMessage {
                    action: action as i32,
                    ..Default::default()
                }
                .encode_to_vec(),
            };
            let expected = 101 + offset as u64;
            assert_eq!(
                runtime
                    .stamp_file_mutation_request_id(&mut message)
                    .unwrap(),
                Some(expected)
            );
            assert_eq!(crate::wire::request_id(4, &message.payload), Some(expected));

            // A second host boundary, or a caller-owned correlated mutation,
            // must preserve the first identity without consuming another ID.
            assert_eq!(
                runtime
                    .stamp_file_mutation_request_id(&mut message)
                    .unwrap(),
                Some(expected)
            );
        }

        let unassigned_actions = [
            commands::read_startup_files(false),
            commands::read_folders(false),
            OutboundMessage {
                message_type: profile::MESSAGE_TYPE_FILE,
                payload: pa::FileMessage {
                    action: pa::message_action::Enum::Upload as i32,
                    ..Default::default()
                }
                .encode_to_vec(),
            },
            OutboundMessage {
                message_type: profile::MESSAGE_TYPE_FILE,
                payload: pa::FileMessage {
                    action: pa::message_action::Enum::Download as i32,
                    ..Default::default()
                }
                .encode_to_vec(),
            },
            OutboundMessage {
                message_type: profile::MESSAGE_TYPE_FILE,
                payload: pa::FileMessage {
                    action: 99,
                    ..Default::default()
                }
                .encode_to_vec(),
            },
        ];
        for mut message in unassigned_actions {
            assert_eq!(
                runtime
                    .stamp_file_mutation_request_id(&mut message)
                    .unwrap(),
                None
            );
            assert_eq!(crate::wire::request_id(4, &message.payload), None);
        }
        assert_eq!(runtime.reserve_request_id(), 107);

        let mut malformed = OutboundMessage {
            message_type: profile::MESSAGE_TYPE_FILE,
            payload: vec![0xff],
        };
        assert!(runtime
            .stamp_file_mutation_request_id(&mut malformed)
            .is_err());
        assert_eq!(runtime.reserve_request_id(), 108);
    }

    #[test]
    fn first_disconnect_rebuilds_every_active_phase_without_an_echo_allowance() {
        // The desktop Connection handler acts on the first false value. An
        // earlier host Connection(false) is not evidence that this is an echo.
        //
        // SessionValidating is excluded: before the reset gate is answered
        // there is no session of ours to rebuild, and the QC reports a
        // departed host's session there. See the dedicated case below.
        for phase in [
            DeviceStartupPhase::VersionValidating,
            DeviceStartupPhase::Building,
            DeviceStartupPhase::Initializing,
            DeviceStartupPhase::Booting,
            DeviceStartupPhase::Connected,
        ] {
            let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
            enter_building(&mut runtime);
            runtime.phase = phase;
            runtime.seed_evidence.preset = true;
            let now = 100_000;
            assert!(
                matches!(
                    runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), now),
                    DeviceStartupAction::SendThenBuild(_)
                ),
                "first disconnect ignored in {phase:?}"
            );
            assert_eq!(runtime.phase(), DeviceStartupPhase::Disconnected);
            assert!(!runtime.seed_evidence.preset);
            assert_eq!(runtime.deadline_ms, now + profile::READY_WAIT_TIMEOUT_MS);
            assert_eq!(
                runtime.observe(
                    profile::MESSAGE_TYPE_CONNECTION,
                    &disconnected_frame(),
                    now + 50
                ),
                DeviceStartupAction::Wait
            );
            assert_eq!(runtime.deadline_ms, now + profile::READY_WAIT_TIMEOUT_MS);
            assert!(matches!(
                runtime.begin_building(),
                DeviceStartupAction::Send(_)
            ));
            drive_boot_to_connected(&mut runtime);
            assert!(
                matches!(
                    runtime.observe(
                        profile::MESSAGE_TYPE_CONNECTION,
                        &disconnected_frame(),
                        now + 100
                    ),
                    DeviceStartupAction::SendThenBuild(_)
                ),
                "rebuild must not create a future disconnect allowance"
            );
        }
    }

    #[test]
    fn a_departed_hosts_disconnect_cannot_hijack_our_reset_gate() {
        // A host that exits without sending connection(false) - a kill or a
        // crash - leaves the QC holding a connected session. The device then
        // reports that stale session ~9 ms before echoing our
        // ResetCommsBuffers. Rebuilding on it moved us out of
        // SessionValidating, so the echo ~40 ms later was discarded by the
        // phase match, and QcUsb::connect - whose only exit is
        // VersionValidating - stranded the unit in Building for six attempts
        // until HANDSHAKE_TIMEOUT_MS. The app livelocked this way on its own
        // first launch after a prior host process exited uncleanly.
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        assert_eq!(runtime.phase(), DeviceStartupPhase::SessionValidating);
        assert_eq!(
            runtime.observe(
                profile::MESSAGE_TYPE_CONNECTION,
                &disconnected_frame(),
                1_000
            ),
            DeviceStartupAction::Wait
        );
        assert_eq!(runtime.phase(), DeviceStartupPhase::SessionValidating);
    }

    #[test]
    fn connection_requires_present_false_and_terminal_states_do_not_rebuild() {
        let (mut runtime, _) = DeviceStartupRuntime::start(7, "session");
        enter_building(&mut runtime);
        drive_boot_to_connected(&mut runtime);
        for payload in [vec![], vec![16, 1]] {
            assert_eq!(
                runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &payload, 10),
                DeviceStartupAction::Wait
            );
            assert!(runtime.is_connected());
        }
        assert_eq!(
            runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &[255], 11),
            DeviceStartupAction::Failed(DeviceStartupError::ParseMessageFailure)
        );
        for phase in [DeviceStartupPhase::Failed, DeviceStartupPhase::Invalid] {
            runtime.phase = phase;
            assert_eq!(
                runtime.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected_frame(), 12),
                DeviceStartupAction::Wait
            );
            assert_eq!(runtime.phase(), phase);
        }
    }

    #[test]
    fn lifecycle_discards_an_incomplete_seed_when_a_rebuild_starts() {
        let (mut lifecycle, _) = DeviceLifecycleRuntime::start_at(7, "session", 0);
        assert!(matches!(
            lifecycle.observe(
                profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
                &reset_reply(7, "session"),
                0
            ),
            DeviceStartupAction::Send(_)
        ));
        assert!(matches!(
            lifecycle.observe(profile::MESSAGE_TYPE_VERSION, &valid_version(), 0),
            DeviceStartupAction::SendThenBuild(_)
        ));
        assert!(matches!(
            lifecycle.begin_building(),
            DeviceStartupAction::Send(_)
        ));
        assert!(matches!(
            lifecycle.observe(profile::MESSAGE_TYPE_MODEL_REPO, &valid_model_repo(), 0),
            DeviceStartupAction::Send(_)
        ));
        let module_stats = pa::ModuleStatsMessage {
            action: pa::message_action::Enum::Update as i32,
            stats: vec![pa::ModuleStatsItem::default()],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            lifecycle.observe(profile::MESSAGE_TYPE_MODULE_STATS, &module_stats, 0),
            DeviceStartupAction::Send(_)
        ));
        let updater = pa::UpdaterMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            lifecycle.observe(profile::MESSAGE_TYPE_UPDATER, &updater, 0),
            DeviceStartupAction::Connected
        );
        let system_time = lifecycle
            .start_post_boot_initialization(100, 1_788_711_237_617)
            .expect("connected lifecycle starts a seed");
        assert_eq!(
            system_time.message_type,
            profile::MESSAGE_TYPE_SYSTEM_TIME_SYNC
        );
        let decoded = pa::SystemTimeSyncMessage::decode(system_time.payload.as_slice()).unwrap();
        assert_eq!(decoded.ms_since_epoch, 1_788_711_237_617);
        let sync_request_id = match decoded.request_id {
            Some(pa::system_time_sync_message::RequestId::RequestId(value)) => value,
            None => panic!("system-time sync must carry the shared lifecycle id"),
        };
        assert_eq!(
            lifecycle.startup.next_request_id,
            sync_request_id.wrapping_add(2),
            "SystemTimeSync N precedes the post-boot seed at N+1"
        );
        assert_eq!(
            lifecycle.start_post_boot_initialization(101, 1_788_711_237_618),
            Err(DeviceStartupError::StateError)
        );
        assert!(lifecycle.has_initialization());
        lifecycle.abort_post_boot_initialization();
        assert!(!lifecycle.has_initialization());
        let retry = lifecycle
            .start_post_boot_initialization(102, 1_788_711_237_619)
            .expect("a failed SystemTimeSync write can be retried");
        let retry = pa::SystemTimeSyncMessage::decode(retry.payload.as_slice()).unwrap();
        assert_eq!(
            retry.request_id,
            Some(pa::system_time_sync_message::RequestId::RequestId(
                sync_request_id.wrapping_add(2)
            ))
        );

        let disconnected = pa::ConnectionMessage {
            connected: Some(pa::connection_message::Connected::Connected(false)),
            ..Default::default()
        }
        .encode_to_vec();
        // The first disconnect discards the prior seed.
        assert!(matches!(
            lifecycle.observe(profile::MESSAGE_TYPE_CONNECTION, &disconnected, 0),
            DeviceStartupAction::SendThenBuild(_)
        ));
        assert!(!lifecycle.has_initialization());
    }

    #[test]
    fn an_unsolicited_preset_skips_the_explicit_preset_read() {
        let mut runtime = InitializationRuntime::start_post_boot(10, 41);
        runtime.observe(profile::MESSAGE_TYPE_RECALL_PRESET);
        let InitializationAction::Send(seed) = runtime.advance(11) else {
            panic!("missing state seed");
        };
        assert_eq!(seed.len(), REQUIRED_SEED_TYPES.len());
        assert!(seed
            .iter()
            .all(|message| message.message_type != profile::MESSAGE_TYPE_RECALL_PRESET));
    }

    #[test]
    fn malformed_selected_scene_is_not_coherent_initialization_seed_evidence() {
        let scene = |value| {
            pa::SceneMessage {
                action: pa::message_action::Enum::Update as i32,
                selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(value)),
                ..Default::default()
            }
            .encode_to_vec()
        };
        for value in [crate::domain::SCENE_COUNT, u32::MAX] {
            let mut seed = SeedEvidence::default();
            seed.observe_message(profile::MESSAGE_TYPE_SCENE, &scene(value));
            assert!(
                !seed.satisfies_type(profile::MESSAGE_TYPE_SCENE),
                "malformed native scene {value} cannot certify the scene seed"
            );
        }
        for value in [0, crate::domain::SCENE_COUNT - 1] {
            let mut seed = SeedEvidence::default();
            seed.observe_message(profile::MESSAGE_TYPE_SCENE, &scene(value));
            assert!(seed.satisfies_type(profile::MESSAGE_TYPE_SCENE));
        }
    }

    #[test]
    fn malformed_or_empty_seed_messages_never_satisfy_readiness() {
        let mut runtime = InitializationRuntime::start_post_boot(0, 41);
        runtime.observe_message(profile::MESSAGE_TYPE_RECALL_PRESET, &[]);
        runtime.observe_message(profile::MESSAGE_TYPE_SETLIST_POSITION, &[]);
        assert!(!runtime.synchronized());

        runtime.observe_message(profile::MESSAGE_TYPE_RECALL_PRESET, &valid_preset());
        assert!(runtime.synchronized());
        let InitializationAction::Send(messages) = runtime.advance(1) else {
            panic!("an empty position message must leave seed fields missing");
        };
        assert!(messages
            .iter()
            .any(|message| message.message_type == profile::MESSAGE_TYPE_SETLIST_POSITION));
    }

    #[test]
    fn a_late_seed_can_recover_after_the_bounded_incomplete_result() {
        let mut runtime = InitializationRuntime::start_post_boot(0, 41);
        runtime.observe_message(profile::MESSAGE_TYPE_RECALL_PRESET, &valid_preset());
        assert!(matches!(runtime.advance(1), InitializationAction::Send(_)));
        assert_eq!(
            runtime.advance(1 + INITIAL_SEED_TIMEOUT_MS),
            InitializationAction::Complete {
                synchronized: false
            }
        );

        for message_type in REQUIRED_SEED_TYPES {
            runtime.observe(*message_type);
        }
        assert_eq!(
            runtime.advance(2 + INITIAL_SEED_TIMEOUT_MS),
            InitializationAction::Complete { synchronized: true }
        );
    }

    #[test]
    fn a_missing_preset_is_requested_once_before_seeding() {
        let mut runtime = InitializationRuntime::start_post_boot(0, 77);
        let InitializationAction::Send(read) = runtime.advance(profile::INITIAL_SYNC_TIMEOUT_MS)
        else {
            panic!("missing explicit preset read");
        };
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].message_type, profile::MESSAGE_TYPE_RECALL_PRESET);
        assert_eq!(
            runtime.advance(profile::INITIAL_SYNC_TIMEOUT_MS + 1),
            InitializationAction::Wait
        );
    }

    #[test]
    fn post_boot_seed_does_not_replay_the_startup_burst() {
        let mut runtime = InitializationRuntime::start_post_boot(100, 77);
        assert_eq!(runtime.advance(101), InitializationAction::Wait);
        assert!(matches!(
            runtime.advance(100 + profile::INITIAL_SYNC_TIMEOUT_MS),
            InitializationAction::Send(messages)
                if messages == vec![commands::read_current_preset()]
        ));
    }

    #[test]
    fn seed_completion_has_one_shared_readiness_definition() {
        let mut runtime = InitializationRuntime::start_post_boot(0, 1);
        runtime.observe(profile::MESSAGE_TYPE_RECALL_PRESET);
        for message_type in REQUIRED_SEED_TYPES {
            runtime.observe(*message_type);
        }
        assert_eq!(
            runtime.advance(1),
            InitializationAction::Complete { synchronized: true }
        );
        assert!(runtime.is_complete());
    }

    #[test]
    fn a_device_without_a_preset_reaches_a_bounded_incomplete_state() {
        let mut runtime = InitializationRuntime::start_post_boot(0, 1);
        assert!(matches!(
            runtime.advance(profile::INITIAL_SYNC_TIMEOUT_MS),
            InitializationAction::Send(_)
        ));
        let preset_deadline = profile::INITIAL_SYNC_TIMEOUT_MS + profile::PRESET_SYNC_TIMEOUT_MS;
        assert!(matches!(
            runtime.advance(preset_deadline),
            InitializationAction::Send(_)
        ));
        assert_eq!(
            runtime.advance(preset_deadline + INITIAL_SEED_TIMEOUT_MS),
            InitializationAction::Complete {
                synchronized: false
            }
        );
    }

    #[test]
    fn a_preset_without_the_required_state_seed_is_not_ready() {
        let mut runtime = InitializationRuntime::start_post_boot(0, 1);
        runtime.observe(profile::MESSAGE_TYPE_RECALL_PRESET);
        assert!(matches!(runtime.advance(1), InitializationAction::Send(_)));
        assert_eq!(
            runtime.advance(1 + INITIAL_SEED_TIMEOUT_MS),
            InitializationAction::Complete {
                synchronized: false
            }
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// The decoders that see every unsolicited frame never panic,
        /// whatever bytes a declared message type carries.
        #[test]
        fn arbitrary_frames_never_panic_the_state_and_startup_decoders(
            type_index in 0..crate::message_registry::MESSAGE_TYPES.len(),
            payload in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..4096),
        ) {
            let (message_type, _) = crate::message_registry::MESSAGE_TYPES[type_index];
            let _ = crate::state::StateDecoder::new().decode(message_type, &payload);
            let _ = crate::wire::request_id(message_type, &payload);
            if message_type != profile::MESSAGE_TYPE_MODEL_REPO {
                let _ = crate::device::state_runtime::DeviceStateRuntime::new()
                    .ingest(message_type, &payload);
            }
            let (mut validating, _) = DeviceStartupRuntime::start(7, "session");
            let _ = validating.observe(message_type, &payload, 0);
            let (mut building, _) = DeviceStartupRuntime::start(7, "session");
            enter_building(&mut building);
            let _ = building.observe(message_type, &payload, 0);
            let _ = building.observe(message_type, &payload, 1);
        }
    }
}
