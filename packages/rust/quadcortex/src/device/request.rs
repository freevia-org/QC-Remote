//! Platform-neutral validation and planning for public gateway writes.
//!
//! Hosts execute the returned plan using their own HID/MIDI implementation and
//! feed readback into [`GatewaySnapshot`].

mod backup;
mod host_midi;
pub use backup::{finalize_device_backup, validate_backup_name, BACKUP_NAME_MAX_CHARACTERS};
pub use host_midi::{is_host_midi_method, plan_host_midi, HostMidiPlan};

use crate::commands::{
    input_level_db_to_wire, is_general_integer_value, is_input_ground_lift_value,
    is_input_impedance_value, is_input_setting_port_id, is_input_type_value, is_mode_cycle_value,
    is_tuner_input_port_id, is_usb_dry_wet_value, is_usb_headphones_source_value,
};
use crate::commands::{DeviceCommand, DeviceOperation, StompAssignmentType};
use crate::device::catalog::CatalogVerificationPolicy;
use crate::device::generated_gateway::{
    GLOBAL_EQ_BANDS, GLOBAL_EQ_BAND_ENABLED, GLOBAL_EQ_BAND_FREQUENCY, GLOBAL_EQ_BAND_GAIN,
    GLOBAL_EQ_BAND_Q, GLOBAL_EQ_BAND_STRIDE, GLOBAL_EQ_BAND_TYPE,
};
use crate::device::{GatewaySnapshot, PresetLibrary, PresetList};
use crate::responses::{
    decode_captured_screen, decode_current_preset_ack, decode_device_identity, decode_diagnostics,
    decode_favorites, decode_general_settings, decode_global_eq, decode_global_tempo_settings,
    decode_graphics_tree, decode_inhibited_modules, decode_io_settings, decode_looper_status,
    decode_mode_cycle, decode_pinned_models, decode_preset_screenshot,
    decode_preset_tempo_settings, decode_recents_favorites, decode_tuner_settings, PngImage,
};
use crate::state::{canonical_midi_slots, BlockParameter, MidiOutMessage};
use crate::{domain, profile};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A device write that a host executes, as produced by the planners in this module.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedWrite {
    /// A single command sent over the HID transport.
    HidCommand(DeviceCommand),
    /// An operation sent over the HID transport. It may encode to several messages; see
    /// [`PlannedWrite::inter_message_interval_ms`] for their pacing.
    HidOperation(DeviceOperation),
    /// A MIDI Control Change message sent through the host's MIDI output.
    MidiControlChange {
        /// Control Change controller number to send.
        controller: u8,
        /// Control Change data value to send.
        value: u8,
    },
}

impl PlannedWrite {
    /// Device-protocol pacing for the encoded messages in this write.
    ///
    /// Hosts still own the actual sleep/timer primitive, but they must not
    /// rediscover which QC operations are multi-report gestures.
    pub fn inter_message_interval_ms(&self) -> u64 {
        match self {
            Self::HidOperation(operation) => operation_inter_message_interval_ms(operation),
            Self::HidCommand(_) | Self::MidiControlChange { .. } => 0,
        }
    }
}

/// Overall host deadline, starting before planning. Existing phase allowances
/// remain independent ceilings within this budget; these are host policy, not
/// device acknowledgement timings. No preflight read is budgeted: every
/// stale-state guard consults decoded state and sends nothing.
pub fn gateway_write_budget_ms(method: &str, write: &PlannedWrite) -> Result<u64, String> {
    let policy = gateway_write_verification_policy(method);
    let dispatch_ms = planned_write_dispatch_budget_ms(write)?;
    let verification_ms = if gateway_write_is_realtime(method) {
        0
    } else {
        policy.timeout_ms
    };
    Ok(dispatch_ms.saturating_add(verification_ms))
}

/// Native phase ceiling shared by ordinary commands and preset workflows.
pub fn planned_write_dispatch_budget_ms(write: &PlannedWrite) -> Result<u64, String> {
    let message_count = match write {
        PlannedWrite::HidOperation(operation) => operation
            .clone()
            .try_encode()
            .map_err(|error| error.to_string())?
            .len() as u64,
        _ => 1,
    };
    Ok(profile::COMMAND_CONFIRMATION_TIMEOUT_MS.saturating_add(
        write
            .inter_message_interval_ms()
            .saturating_mul(message_count),
    ))
}

/// A hold must outlast the constant it is checked against. Asserted here rather
/// than in a test because both sides are constants: the compiler can settle it,
/// and a runtime assertion over two constants proves nothing at the moment it
/// runs. (`REMOTE_GESTURE_INTERVAL_MS` is the gap of the former two-frame tap,
/// kept only as that floor: a `PRESS` and `RELEASE` 20 ms or 80 ms apart did not
/// reliably tap on CorOS 4.1.0, observed 2026-10-06.)
const _: () = assert!(profile::REMOTE_HOLD_INTERVAL_MS > profile::REMOTE_GESTURE_INTERVAL_MS);
/// The stepped drag builder is paced no faster than that floor, and one paced
/// like a hold would exceed its cap. The builder is not reachable from the
/// gateway (`MOVE` is inert); these only keep its own constants coherent.
const _: () = assert!(profile::REMOTE_DRAG_INTERVAL_MS > profile::REMOTE_GESTURE_INTERVAL_MS);
const _: () = assert!(profile::REMOTE_HOLD_INTERVAL_MS > profile::REMOTE_DRAG_INTERVAL_MS);

/// Milliseconds a host must wait between consecutive encoded messages of `operation`.
///
/// Screen holds use the hold interval, the library's stepped screen drag builder
/// uses the drag interval, and every other operation returns 0. A tap
/// ([`DeviceOperation::ScreenTap`]), a swipe or drag ([`DeviceOperation::ScreenDrag`])
/// and a raw mouse frame ([`DeviceOperation::ScreenMouseFrame`]) are each a single frame, so there is
/// no gap to pace.
pub fn operation_inter_message_interval_ms(operation: &DeviceOperation) -> u64 {
    // A hold is two different frames, `PRESS` (touch-down) then `RELEASE`
    // (touch-up). The protocol has no long-press frame, so this interval is the
    // entire mechanism: the unit decides hold against tap from how long the touch
    // lasted, and the touch lasts exactly as long as the gap between the frame
    // that begins it and the frame that ends it. 1200 ms is a host choice, above the
    // Grid and parameter values measured by hand but below the 1.5 s tried on a
    // Modes hybrid (see `commands::screen_hold`), not a measured threshold itself.
    if matches!(operation, DeviceOperation::ScreenHold { .. }) {
        return profile::REMOTE_HOLD_INTERVAL_MS;
    }
    // The library's stepped drag is a stream of MOVE frames. Every frame,
    // including the touch-down and touch-up, is spaced by the same drag interval.
    // The gateway no longer plans it (MOVE is inert on CorOS 4.1.0).
    if matches!(operation, DeviceOperation::ScreenDragSteps { .. }) {
        return profile::REMOTE_DRAG_INTERVAL_MS;
    }
    // A tap is the single TAP frame and a swipe or drag the single DRAG frame, so
    // neither has an interval. The two-frame tap and three-frame swipe these
    // replaced sent a touch-down that was never released (observed on CorOS
    // 4.1.0, 2026-10-06), and at 20 ms a swipe did not scroll.
    0
}

/// The result of planning a gateway write: what to send, how to describe it, and how to confirm it.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayWritePlan {
    /// The message or messages the host sends.
    pub write: PlannedWrite,
    /// Short human-readable summary of the write, for the result reported to the caller.
    pub detail: String,
    /// Device-state predicate that proves the write landed, or [`GatewayVerification::None`] when
    /// no device state can confirm it.
    pub verification: GatewayVerification,
}

impl GatewayWritePlan {
    /// The guarded footswitch transition already matches, so its encoder emits no frame.
    /// Hosts must report a no-send preflight result, rather than await an echo or claim delivery.
    pub fn is_noop(&self) -> bool {
        matches!(&self.write,
            PlannedWrite::HidOperation(DeviceOperation::SetFootswitch {
                footswitch, assignment_type, previous_footswitch, previous_assignment_type, ..
            }) if footswitch == previous_footswitch
                && (footswitch.is_none() || previous_assignment_type.as_ref() == Some(assignment_type)))
    }
}

/// Whether a gateway write must return as soon as its bytes reach the native
/// transport and let the pushed state stream reconcile the authoritative QC
/// result. Both hosts use this policy so realtime controls never acquire a
/// platform-specific readback delay or snapshot polling loop.
pub fn gateway_write_is_realtime(method: &str) -> bool {
    matches!(
        method,
        "device.selectScene"
            | "device.toggleBypass"
            | "device.previewParameter"
            | "device.previewLaneControlParameter"
            | "device.setTempo"
            | "device.setMasterVolume"
            | "device.pressFootswitch"
            | "device.tapTempo"
            // A mode change completes at enqueue: no command ACK, request
            // correlation, readback, or timeout. A pushed ModeMessage UPDATE
            // reconciles it, as it does for every other performance control
            // here.
            | "device.selectModeSlot"
            | "device.showTuner"
            | "device.showGigView"
            | "device.controlLooper"
    )
}

/// Which observation a write preflight consumes as authoritative.
///
/// There is exactly one source: the decoded device state the session folds from
/// what the QC pushes. No preflight sends a READ (Cortex Control reads nothing
/// before a write); a guard whose state has not been published fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GatewayPreflightObservationSource {
    /// The decoded device-state snapshot maintained from the state stream.
    DecodedSnapshot,
}

/// Shared schedule for confirming a write, from [`gateway_write_verification_policy`] and
/// [`gateway_verification_policy`]. Hosts supply timers; the cadence and deadlines come from this
/// policy. Nothing in it sends a frame: confirmation is passive, from pushed state only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayWriteVerificationPolicy {
    /// Overall confirmation deadline, in milliseconds from the start of the write.
    pub timeout_ms: u64,
    /// Delays in milliseconds before each poll of a retained settings publication, indexed by
    /// attempt number; see [`gateway_correlated_readback_delay`]. A poll reads a cache the QC's
    /// own sparse echo has already updated and never touches the wire.
    pub correlated_readback_retry_intervals_ms: Vec<u64>,
    /// The decoded observation the write's stale-state guard consults, if it has one; see
    /// [`gateway_write_preflight_method`]. It names state, not a read to send.
    pub preflight_method: Option<&'static str>,
    /// Which observation the preflight consumes; `Some` exactly when `preflight_method` is `Some`.
    pub preflight_observation_source: Option<GatewayPreflightObservationSource>,
    /// The retained settings publication whose sparse echo confirms the write, if the QC
    /// publishes one; see [`gateway_write_readback_method`].
    pub readback_method: Option<&'static str>,
}

/// Shared schedule for confirming a write from what the QC pushes. Hosts provide timers only; the
/// overall confirmation deadline is shared host policy, not a firmware guarantee.
///
/// Host policy (not a Cortex Control frame). No frame is sent while waiting: Cortex Control
/// reads nothing after a write, and this host no longer issues the type-15 verification READs
/// (0/250/1500 ms and preset-transition ladders) it used to. When no pushed evidence arrives
/// before the deadline the write is reported `accepted_unverified`; that is not a failure and
/// quarantines nothing. The 3 s and 15 s deadlines are BELIEVED, with no measurement in the
/// repository; whether the unit pushes evidence for every write family is UNKNOWN (hardware
/// verification pending, experiments 9-11 and 16).
pub fn gateway_write_verification_policy(method: &str) -> GatewayWriteVerificationPolicy {
    let preset_transition = matches!(
        method,
        "device.recallPreset" | "device.navigateBank" | "device.reloadPreset"
    );
    let timeout_ms = if preset_transition {
        profile::PRESET_SYNC_TIMEOUT_MS
    } else {
        profile::COMMAND_CONFIRMATION_TIMEOUT_MS
    };
    let mut policy = gateway_verification_policy(timeout_ms);
    policy.preflight_method = gateway_write_preflight_method(method);
    policy.preflight_observation_source = gateway_write_preflight_observation_source(method);
    policy.readback_method = gateway_write_readback_method(method);
    policy
}

/// Verification-only policy used by the stages of a multi-write workflow.
/// Keeping this constructor beside ordinary write policy prevents native hosts
/// from rebuilding a partial policy and accidentally drifting in cadence.
pub fn gateway_verification_policy(timeout_ms: u64) -> GatewayWriteVerificationPolicy {
    GatewayWriteVerificationPolicy {
        timeout_ms,
        correlated_readback_retry_intervals_ms:
            profile::CORRELATED_WRITE_READBACK_RETRY_INTERVALS_MS.to_vec(),
        preflight_method: None,
        preflight_observation_source: None,
        readback_method: None,
    }
}

/// Return the delay before one poll of a retained settings publication. Native
/// hosts differ in how they wait (blocking on Windows, futures on Android), but
/// neither owns the poll count or cadence. No poll sends a frame.
pub fn gateway_correlated_readback_delay(method: &str, attempt: usize) -> Option<u64> {
    gateway_write_verification_policy(method)
        .correlated_readback_retry_intervals_ms
        .get(attempt)
        .copied()
}

/// Authoritative state predicate associated with a planned write. Hosts only
/// decide how to wait for a fresh device observation; the state that proves a
/// command landed is shared across Windows and Android.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GatewayVerification {
    /// No predicate. Nothing in device state can confirm the write.
    None,
    /// The active scene equals `scene`.
    Scene {
        /// Zero-based scene index (0 through 7).
        scene: u32,
    },
    /// The label of `scene` equals `label`.
    SceneLabel {
        /// Zero-based scene index (0 through 7).
        scene: u32,
        /// Expected label. `None` expects the scene to have no custom label: an empty label or the
        /// default name (`Scene A`, `Scene B`, and so on).
        label: Option<String>,
    },
    /// The colour of `scene` equals `color`.
    SceneColor {
        /// Zero-based scene index (0 through 7).
        scene: u32,
        /// Exact expected native ARGB value, including alpha.
        color: u32,
    },
    /// A scene copy (or swap) landed: the destination scene carries the source's label and colour,
    /// and for a swap the source carries the destination's.
    SceneCopy {
        /// Zero-based index of the source scene.
        from_scene: u32,
        /// Zero-based index of the destination scene.
        to_scene: u32,
        /// Whether the two scenes are exchanged rather than the source being copied over the
        /// destination.
        swap: bool,
        /// Label of the source scene before the copy.
        from_label: String,
        /// Label of the destination scene before the copy.
        to_label: String,
        /// Colour (`#rrggbb`) of the source scene before the copy. `None` skips the colour check.
        from_color: Option<String>,
        /// Colour (`#rrggbb`) of the destination scene before the copy. `None` skips the colour check.
        to_color: Option<String>,
    },
    /// The tempo equals `bpm`.
    Tempo {
        /// Expected tempo in beats per minute.
        bpm: u32,
    },
    /// The master volume equals `value`.
    MasterVolume {
        /// Expected master volume on the 0 through 100 scale used by the write.
        value: u32,
    },
    /// The footswitch mode equals `mode`.
    Mode {
        /// Device mode identifier: 0 is PRESET, 1 is SCENE, 2 is STOMP (observed on a CorOS 4.1.0 unit,
        /// 2026-10-07), and 3 through 8 are believed to be the hybrid pairings PRESET/SCENE,
        /// PRESET/STOMP, SCENE/PRESET, SCENE/STOMP, STOMP/PRESET and STOMP/SCENE, in that order. In a hybrid `A/B`, the first mode `A` is believed to
        /// apply to footswitches A-D and the second mode `B` to footswitches
        /// E-H. A value above 8 is not valid.
        mode: u32,
    },
    /// The block at (`row`, `column`) has the given bypass state.
    Bypass {
        /// Zero-based grid row of the block.
        row: u32,
        /// Zero-based grid column of the block.
        column: u32,
        /// Expected bypass state.
        bypassed: bool,
    },
    /// A parameter has the given normalized value, within a tolerance of 0.0005.
    Parameter {
        /// Zero-based grid row.
        row: u32,
        /// Zero-based grid column. Lane controls are addressed as columns 10 (input gate) and 11.
        column: u32,
        /// Index of the parameter within the block or lane control.
        parameter_index: u32,
        /// Expected normalized value of the parameter.
        value: f64,
        /// When set, the splitter model on this row must also equal this identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        routing_model_id: Option<u32>,
    },
    /// A parameter's scene-mode flag equals `enabled`.
    ParameterSceneMode {
        /// Zero-based grid row.
        row: u32,
        /// Zero-based grid column. Lane controls are addressed as columns 10 (input gate) and 11.
        column: u32,
        /// Index of the parameter within the block or lane control.
        parameter_index: u32,
        /// Expected scene-mode state.
        enabled: bool,
    },
    /// A parameter is assigned to expression pedal `pedal` with the given range, within a tolerance
    /// of 0.0005.
    ParameterExpression {
        /// Zero-based grid row.
        row: u32,
        /// Zero-based grid column. Lane controls are addressed as columns 10 (input gate) and 11.
        column: u32,
        /// Index of the parameter within the block or lane control.
        parameter_index: u32,
        /// Expression pedal number the parameter is expected to be assigned to.
        pedal: u32,
        /// Expected lower bound of the expression range, normalized to 0.0 through 1.0.
        minimum: f64,
        /// Expected upper bound of the expression range, normalized to 0.0 through 1.0.
        maximum: f64,
    },
    /// A block is present at (`row`, `column`), or absent when `present` is false.
    Block {
        /// Zero-based grid row.
        row: u32,
        /// Zero-based grid column.
        column: u32,
        /// Expected model identifier of the block. `None` accepts any model.
        #[serde(skip_serializing_if = "Option::is_none")]
        model_id: Option<u32>,
        /// Whether a block is expected at the position (`false` when verifying a removal).
        present: bool,
    },
    /// A block of `model_id` is at the destination and the source cell is empty.
    MoveBlock {
        /// Zero-based grid row the block moved from.
        from_row: u32,
        /// Zero-based grid column the block moved from.
        from_column: u32,
        /// Zero-based grid row the block moved to.
        to_row: u32,
        /// Zero-based grid column the block moved to.
        to_column: u32,
        /// Model identifier of the moved block.
        model_id: u32,
    },
    /// The block at (`row`, `column`) has the given footswitch assignment.
    Footswitch {
        /// Zero-based grid row of the block.
        row: u32,
        /// Zero-based grid column of the block.
        column: u32,
        /// Footswitch index the block is expected to be assigned to. `None` expects no assignment.
        footswitch: Option<u32>,
        /// Expected assignment type name, as reported in the snapshot. Set exactly when `footswitch`
        /// is `Some`.
        assignment_type: Option<String>,
    },
    /// A footswitch's momentary flag equals `momentary`.
    StompMomentary {
        /// Footswitch index.
        footswitch: u32,
        /// Expected momentary state.
        momentary: bool,
    },
    /// A footswitch's STOMP label equals `label`.
    StompLabel {
        /// Footswitch index.
        footswitch: u32,
        /// Expected label text.
        label: String,
    },
    /// The MIDI Out message list for a source (or the preset-load list) equals `messages`.
    MidiOut {
        /// MIDI Out source index. `None` selects the preset-load MIDI Out list.
        source: Option<u32>,
        /// Expected message list.
        messages: Vec<MidiOutMessage>,
    },
    /// The expression bypass assignment of a block matches the given settings.
    ExpressionBypass {
        /// Zero-based grid row of the block.
        row: u32,
        /// Zero-based grid column of the block.
        column: u32,
        /// Expression pedal number. 0 expects the assignment to have been cleared.
        pedal: u32,
        /// Expected bypass mode value, as sent in the write.
        mode: u32,
        /// Expected invert setting.
        invert: bool,
        /// Expected delay in milliseconds.
        delay_ms: u32,
        /// Expected latch-emulation setting.
        latch_emulation: bool,
    },
    /// A signal-chain row has the given input port.
    RouteInput {
        /// Zero-based grid row of the chain.
        row: u32,
        /// Expected input port identifier.
        input_id: u32,
    },
    /// A signal-chain row has the given output port.
    RouteOutput {
        /// Zero-based grid row of the chain.
        row: u32,
        /// Expected output port identifier.
        output_id: u32,
    },
    /// A signal-chain row has the given split and mix columns.
    RouteSplit {
        /// Zero-based grid row of the chain.
        row: u32,
        /// Expected split column, compared with the route's value; `None` expects no split column.
        split_column: Option<i32>,
        /// Expected mix column, compared with the route's value; `None` expects no mix column.
        mix_column: Option<i32>,
    },
    /// A signal-chain row's split mute equals `muted`.
    SplitMute {
        /// Zero-based grid row of the chain.
        row: u32,
        /// Expected mute state.
        muted: bool,
        /// Scene that must be active when the state is observed. `None` skips the check.
        scene: Option<u32>,
    },
    /// The active preset is at `position` in setlist `setlist_key`, optionally with a given name and a
    /// clean (saved) state.
    Preset {
        /// Setlist key of the expected preset; a trailing slash is ignored.
        setlist_key: String,
        /// Expected preset position within the setlist.
        position: u32,
        /// Expected preset name. `None` accepts any name.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Whether the preset must report no unsaved changes.
        require_clean: bool,
        /// When set, the snapshot's position revision must be greater than this value.
        #[serde(skip_serializing_if = "Option::is_none")]
        after_position_revision: Option<u64>,
        /// When set, the snapshot's full-preset revision must be greater than this value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_full_preset_revision: Option<u64>,
    },
}

/// Event-driven lifecycle for a native device write.  Platform hosts own only
/// the OS I/O handle and feed observed snapshots into this shared state
/// machine; freshness, matching, and timeout semantics must not drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayTransactionState {
    /// Neither verified nor timed out yet.
    Pending,
    /// A fresh observation after the write satisfies the predicate.
    Verified,
    /// The deadline passed before verification.
    TimedOut,
}

/// The one shared confirmation sequencer used around a native QC write. Hosts
/// wait for the requested delay and feed fresh observations back; they do not
/// interpret the cadence themselves. It never asks a host to send a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayVerificationAction {
    /// Wait `delay_ms` milliseconds, then advance again.
    Wait {
        /// Time to wait before advancing again, in milliseconds.
        delay_ms: u64,
    },
    /// A fresh observation satisfies the predicate; the write is confirmed.
    Verified,
    /// The deadline passed before the predicate was satisfied.
    TimedOut,
}

/// Merge optimistic UI state into gateway arguments using the canonical field
/// names understood by all native hosts.
pub fn merge_expected_state(params: &Value, expected: &Value) -> Value {
    let mut merged = params.as_object().cloned().unwrap_or_default();
    let Some(expected) = expected.as_object() else {
        return Value::Object(merged);
    };
    for (target, sources) in [
        ("expectedPresetName", &["presetName"][..]),
        ("expectedPosition", &["position", "presetPosition"][..]),
        ("expectedScene", &["activeScene", "scene"][..]),
        ("expectedTempo", &["tempo"][..]),
    ] {
        if merged.contains_key(target) {
            continue;
        }
        if let Some(value) = sources.iter().find_map(|key| expected.get(*key)) {
            merged.insert(target.into(), value.clone());
        }
    }
    Value::Object(merged)
}

/// One write's confirmation: a predicate, the observation it must postdate, and a deadline.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayTransaction {
    verification: GatewayVerification,
    after_observation_token: u128,
    deadline_ms: u64,
}

impl GatewayTransaction {
    /// Create a transaction whose deadline is `started_at_ms + timeout_ms`.
    ///
    /// Only an observation with a token greater than `after_observation_token` can verify it.
    pub fn new(
        verification: GatewayVerification,
        after_observation_token: u128,
        started_at_ms: u64,
        timeout_ms: u64,
    ) -> Self {
        Self {
            verification,
            after_observation_token,
            deadline_ms: started_at_ms.saturating_add(timeout_ms),
        }
    }

    /// Evaluate the transaction at `now_ms`.
    ///
    /// Returns [`GatewayTransactionState::TimedOut`] once the deadline is reached (checked first),
    /// [`GatewayTransactionState::Verified`] when `observation_token` is newer than the token the
    /// transaction was created with and the predicate matches `snapshot` and `parameter`, and
    /// [`GatewayTransactionState::Pending`] otherwise.
    pub fn state(
        &self,
        snapshot: &GatewaySnapshot,
        parameter: Option<&BlockParameter>,
        observation_token: u128,
        now_ms: u64,
    ) -> GatewayTransactionState {
        if now_ms >= self.deadline_ms {
            GatewayTransactionState::TimedOut
        } else if observation_token > self.after_observation_token
            && self.verification.matches(snapshot, parameter)
        {
            GatewayTransactionState::Verified
        } else {
            GatewayTransactionState::Pending
        }
    }

    /// Milliseconds left before the deadline at `now_ms`; 0 once it has passed.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.deadline_ms.saturating_sub(now_ms)
    }

    /// Shrink this transaction's own deadline, never extend it.
    pub fn restrict_deadline(&mut self, deadline_ms: u64) {
        self.deadline_ms = self.deadline_ms.min(deadline_ms);
    }
}

/// Drives the confirmation of one write: a [`GatewayTransaction`] and nothing else. It is passive:
/// it never schedules a read. A host calls [`GatewayVerificationRuntime::advance`] repeatedly and
/// performs the action it returns.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayVerificationRuntime {
    transaction: GatewayTransaction,
}

impl GatewayVerificationRuntime {
    pub(crate) fn changes_preset(&self) -> bool {
        matches!(
            self.transaction.verification,
            GatewayVerification::Preset { .. }
        )
    }

    /// Shrink the deadline to the given request deadline; never extends it.
    pub fn restrict_deadline(&mut self, deadline: crate::device::correlation::RequestDeadline) {
        self.transaction.restrict_deadline(deadline.expires_at_ms());
    }

    /// Create a runtime for `verification`, started at `started_at_ms`.
    ///
    /// The timeout comes from `policy`. Only observations with a token greater than
    /// `after_observation_token` can verify the write.
    pub fn new(
        verification: GatewayVerification,
        after_observation_token: u128,
        started_at_ms: u64,
        policy: &GatewayWriteVerificationPolicy,
    ) -> Self {
        Self {
            transaction: GatewayTransaction::new(
                verification,
                after_observation_token,
                started_at_ms,
                policy.timeout_ms,
            ),
        }
    }

    /// Decide the next step given the freshest observation.
    ///
    /// Returns [`GatewayVerificationAction::TimedOut`] when the deadline has passed,
    /// [`GatewayVerificationAction::Verified`] when `snapshot` (with `parameter`, for parameter
    /// predicates) satisfies the predicate and `observation_token` is newer than the write,
    /// and otherwise [`GatewayVerificationAction::Wait`] for the time until the deadline.
    pub fn advance(
        &mut self,
        snapshot: Option<&GatewaySnapshot>,
        parameter: Option<&BlockParameter>,
        observation_token: u128,
        now_ms: u64,
    ) -> GatewayVerificationAction {
        if self.transaction.remaining_ms(now_ms) == 0 {
            return GatewayVerificationAction::TimedOut;
        }
        if snapshot.is_some_and(|snapshot| {
            self.transaction
                .state(snapshot, parameter, observation_token, now_ms)
                == GatewayTransactionState::Verified
        }) {
            return GatewayVerificationAction::Verified;
        }
        GatewayVerificationAction::Wait {
            delay_ms: self.transaction.remaining_ms(now_ms),
        }
    }

    /// The `(row, column, parameter_index)` of the parameter being verified, or `None` when the
    /// predicate is not a parameter predicate.
    pub fn parameter_target(&self) -> Option<(u32, u32, u32)> {
        self.transaction.verification.parameter_target()
    }

    /// Milliseconds left before the deadline at `now_ms`; 0 once it has passed.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.transaction.remaining_ms(now_ms)
    }
}

impl GatewayVerification {
    /// Whether this is [`GatewayVerification::None`].
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Whether a fresh device observation can authoritatively prove this
    /// write landed. A missing predicate must never match an arbitrary state
    /// update merely because that update arrived after the write.
    pub fn requires_authoritative_readback(&self) -> bool {
        !self.is_none()
    }

    /// The `(row, column, parameter_index)` of the parameter this predicate concerns, or `None` when
    /// it is not a parameter predicate.
    pub fn parameter_target(&self) -> Option<(u32, u32, u32)> {
        match self {
            Self::Parameter {
                row,
                column,
                parameter_index,
                ..
            }
            | Self::ParameterSceneMode {
                row,
                column,
                parameter_index,
                ..
            }
            | Self::ParameterExpression {
                row,
                column,
                parameter_index,
                ..
            } => Some((*row, *column, *parameter_index)),
            _ => None,
        }
    }

    /// The decoded `StateUpdate` kinds that can carry evidence for this
    /// predicate. `"preset"` is in every list because one preset publication
    /// re-establishes the whole snapshot — blocks, routes, scenes, labels,
    /// colours, footswitch states, MIDI out and tempo — and the shared
    /// verification refresh is `device.currentPreset`, so a relevant frame is
    /// always reachable without resending the mutation.
    pub fn observed_state_kinds(&self) -> &'static [&'static str] {
        match self {
            // A missing predicate has nothing to observe; the caller must not
            // reach the state loop with it at all.
            Self::None => &[],
            Self::Scene { .. } => &["preset", "scene"],
            Self::SceneLabel { .. } => &["preset", "sceneLabel"],
            Self::SceneColor { .. } => &["preset", "sceneColor"],
            Self::SceneCopy { .. } => &["preset", "sceneLabel", "sceneColor"],
            Self::Tempo { .. } => &["preset", "tempo"],
            Self::MasterVolume { .. } => &["preset", "master"],
            Self::Mode { .. } => &["preset", "mode"],
            Self::Bypass { .. } => &["preset", "bypass", "bypassBatch"],
            // decode_grid publishes a "parameter" state from the inbound
            // type-1 GRID echo the QC sends after a host write, and it has
            // already applied the value to the source `matches` reads. Only
            // the token gate was rejecting that free evidence, which forced
            // a full-preset READ the device never needed to be asked for.
            // `laneControlParameter` is the kind decode_grid publishes for a
            // lane-control write. It appeared exactly once in the tree - at
            // the line creating it - so every lane-control write threw away
            // the echo the QC had already sent and bought a full-preset READ.
            Self::Parameter { .. } => &["preset", "parameter", "laneControlParameter"],
            Self::Block { present: false, .. } => &["preset", "blockRemoved"],
            // SplitMute additionally reads `active_scene`.
            Self::SplitMute { .. } => &["preset", "scene"],
            // `require_clean` reads the dirty flag, which CorOS can publish on
            // its own kind after the position and preset frames.
            Self::Preset { .. } => &["preset", "position", "dirty"],
            // Everything else is proved from the preset projection alone:
            // block details (every Parameter* variant), the grid, the routes,
            // footswitch assignments and MIDI out.
            _ => &["preset"],
        }
    }

    /// Whether one decoded frame can bear on this predicate at all.
    ///
    /// A newer frame *sequence* is not freshness. `worker.rs` publishes a
    /// tempo-clock-only frame — `states: []` — to the same subscriber list as a
    /// real state update, and the predicate is absolute (state == desired)
    /// against the cached snapshot, so accepting an unrelated tick as the
    /// observation certified any write whose requested value already equalled
    /// the current one.
    pub fn observes_state_kinds<K: AsRef<str>>(&self, kinds: impl IntoIterator<Item = K>) -> bool {
        let observed = self.observed_state_kinds();
        kinds
            .into_iter()
            .any(|kind| observed.contains(&kind.as_ref()))
    }

    /// Whether `snapshot` satisfies this predicate.
    ///
    /// Parameter predicates read the block parameter supplied in `parameter` (see
    /// [`GatewayVerification::parameter_target`]); other predicates ignore it. A predicate with no
    /// target ([`GatewayVerification::None`]) never matches.
    pub fn matches(&self, snapshot: &GatewaySnapshot, parameter: Option<&BlockParameter>) -> bool {
        match self {
            Self::None => false,
            Self::Scene { scene } => snapshot.active_scene == *scene,
            Self::SceneLabel { scene, label } => {
                snapshot
                    .scenes
                    .get(*scene as usize)
                    .is_some_and(|actual| match label {
                        Some(label) => actual == label,
                        None => {
                            actual.is_empty()
                                || actual == &format!("Scene {}", (b'A' + *scene as u8) as char)
                        }
                    })
            }
            Self::SceneColor { scene, color } => snapshot
                .scene_colors_argb
                .as_ref()
                .and_then(|colors| colors.get(*scene as usize))
                .is_some_and(|actual| actual == color),
            Self::SceneCopy {
                from_scene,
                to_scene,
                swap,
                from_label,
                to_label,
                from_color,
                to_color,
            } => {
                let copied_label = snapshot
                    .scenes
                    .get(*to_scene as usize)
                    .is_some_and(|actual| {
                        scene_copy_label_matches(actual, from_label, *from_scene, *to_scene)
                    });
                let copied_color = scene_copy_color_matches(snapshot, *to_scene, from_color);
                let swapped_label = !swap
                    || snapshot
                        .scenes
                        .get(*from_scene as usize)
                        .is_some_and(|actual| {
                            scene_copy_label_matches(actual, to_label, *to_scene, *from_scene)
                        });
                let swapped_color =
                    !swap || scene_copy_color_matches(snapshot, *from_scene, to_color);
                copied_label && copied_color && swapped_label && swapped_color
            }
            Self::Tempo { bpm } => snapshot.tempo == *bpm,
            Self::MasterVolume { value } => snapshot.master_volume == *value,
            Self::Mode { mode } => mode_verification_matches(snapshot, *mode),
            Self::Bypass {
                row,
                column,
                bypassed,
            } => {
                snapshot
                    .blocks
                    .iter()
                    .find(|block| block.row == *row && block.column == *column)
                    .and_then(|block| block.bypassed)
                    == Some(*bypassed)
            }
            Self::Parameter {
                row,
                value,
                routing_model_id,
                ..
            } => {
                routing_model_id.is_none_or(|expected| {
                    snapshot
                        .routes
                        .iter()
                        .find(|route| route.row == *row)
                        .and_then(|route| route.splitter_model_id)
                        == Some(expected)
                }) && parameter
                    .and_then(|actual| actual.normalized_value)
                    .is_some_and(|actual| (actual - value).abs() <= 0.0005)
            }
            Self::ParameterSceneMode { enabled, .. } => {
                parameter.is_some_and(|actual| actual.scene_mode == *enabled)
            }
            Self::ParameterExpression {
                pedal,
                minimum,
                maximum,
                ..
            } => parameter.is_some_and(|actual| {
                actual.expression == Some(*pedal as i32)
                    && actual
                        .expression_minimum
                        .is_some_and(|value| (f64::from(value) - minimum).abs() <= 0.0005)
                    && actual
                        .expression_maximum
                        .is_some_and(|value| (f64::from(value) - maximum).abs() <= 0.0005)
            }),
            Self::Block {
                row,
                column,
                model_id,
                present,
            } => {
                let actual = snapshot
                    .blocks
                    .iter()
                    .find(|block| block.row == *row && block.column == *column);
                if *present {
                    actual.is_some_and(|block| model_id.is_none() || block.model_id == *model_id)
                } else {
                    actual.is_none()
                }
            }
            Self::MoveBlock {
                from_row,
                from_column,
                to_row,
                to_column,
                model_id,
            } => {
                let destination_matches = snapshot.blocks.iter().any(|block| {
                    block.row == *to_row
                        && block.column == *to_column
                        && block.model_id == Some(*model_id)
                });
                let source_cleared = (*from_row, *from_column) == (*to_row, *to_column)
                    || snapshot
                        .blocks
                        .iter()
                        .all(|block| block.row != *from_row || block.column != *from_column);
                destination_matches && source_cleared
            }
            Self::Footswitch {
                row,
                column,
                footswitch,
                assignment_type,
            } => snapshot
                .blocks
                .iter()
                .find(|block| block.row == *row && block.column == *column)
                .is_some_and(|block| {
                    block.footswitch == *footswitch
                        && block.footswitch_assignment_type == *assignment_type
                }),
            Self::StompMomentary {
                footswitch,
                momentary,
            } => {
                snapshot
                    .footswitch_states
                    .as_ref()
                    .and_then(|states| states.iter().find(|state| state.index == *footswitch))
                    .and_then(|state| state.momentary)
                    == Some(*momentary)
            }
            Self::StompLabel { footswitch, label } => {
                snapshot
                    .footswitch_states
                    .as_ref()
                    .and_then(|states| states.iter().find(|state| state.index == *footswitch))
                    .and_then(|state| state.label.as_deref())
                    == Some(label.as_str())
            }
            // Slots are compared by position: a list is its twelve slots in
            // order, so an empty slot between two active ones is part of it
            // and trailing empty slots are not.
            Self::MidiOut { source, messages } => match source {
                Some(source) => {
                    canonical_midi_slots(
                        snapshot
                            .midi_out
                            .as_ref()
                            .and_then(|groups| groups.iter().find(|group| group.source == *source))
                            .map(|group| group.messages.as_slice())
                            .unwrap_or_default(),
                    ) == canonical_midi_slots(messages)
                }
                None => {
                    snapshot
                        .preset_load_midi_out
                        .as_deref()
                        .map(canonical_midi_slots)
                        == Some(canonical_midi_slots(messages))
                }
            },
            Self::ExpressionBypass {
                row,
                column,
                pedal,
                mode,
                invert,
                delay_ms,
                latch_emulation,
            } => {
                let block = snapshot
                    .blocks
                    .iter()
                    .find(|block| block.row == *row && block.column == *column);
                match (block, *pedal) {
                    // The block has to exist to have been written to, whichever
                    // way the assignment went.
                    (None, _) => false,
                    // A cleared assignment is the absence of one: the decoder
                    // drops a bypass expression whose pedal is zero, so there
                    // are no switch settings left to compare one by one.
                    //
                    // Absence is the state a clear asks for, so this is what
                    // `verified` means here. It is not a claim that the device
                    // moved: on a block that carried no assignment the cleared and
                    // the ignored cases are one state, and `changed: false` is the
                    // field that says so. Callers who need "did anything happen"
                    // read that, not this.
                    (Some(block), 0) => block.bypass_expression.is_none(),
                    (Some(block), _) => block.bypass_expression.as_ref().is_some_and(|actual| {
                        actual.pedal == *pedal
                            && actual.mode == *mode
                            && actual.invert == *invert
                            && actual.delay_ms == *delay_ms
                            && actual.latch_emulation == *latch_emulation
                            && (actual.minimum - 0.0).abs() <= 0.001
                            && (actual.maximum - 1.0).abs() <= 0.001
                    }),
                }
            }
            Self::RouteInput { row, input_id } => {
                snapshot
                    .routes
                    .iter()
                    .find(|route| route.row == *row)
                    .and_then(|route| route.input_id)
                    == Some(*input_id)
            }
            Self::RouteOutput { row, output_id } => {
                snapshot
                    .routes
                    .iter()
                    .find(|route| route.row == *row)
                    .and_then(|route| route.output_id)
                    == Some(*output_id)
            }
            Self::RouteSplit {
                row,
                split_column,
                mix_column,
            } => snapshot
                .routes
                .iter()
                .find(|route| route.row == *row)
                .is_some_and(|route| {
                    route.split_column == *split_column && route.mix_column == *mix_column
                }),
            Self::SplitMute { row, muted, scene } => {
                scene.is_none_or(|scene| snapshot.active_scene == scene)
                    && snapshot
                        .routes
                        .iter()
                        .find(|route| route.row == *row)
                        .is_some_and(|route| route.split_muted == Some(*muted))
            }
            Self::Preset {
                setlist_key,
                position,
                name,
                require_clean,
                after_position_revision,
                after_full_preset_revision,
            } => {
                snapshot.setlist_key.trim_end_matches('/') == setlist_key.trim_end_matches('/')
                    && snapshot.preset_position == *position
                    && name
                        .as_ref()
                        .is_none_or(|expected| snapshot.preset_name == *expected)
                    && (!require_clean || !snapshot.dirty)
                    && after_position_revision
                        .is_none_or(|revision| snapshot.position_revision > revision)
                    && after_full_preset_revision
                        .is_none_or(|revision| snapshot.full_preset_revision > revision)
            }
        }
    }
}

fn default_scene_label(scene: u32) -> String {
    format!("Scene {}", (b'A' + scene as u8) as char)
}

fn scene_copy_label_matches(
    actual: &str,
    before: &str,
    before_scene: u32,
    after_scene: u32,
) -> bool {
    actual == before
        || (before == default_scene_label(before_scene)
            && actual == default_scene_label(after_scene))
}

fn scene_copy_color_matches(
    snapshot: &GatewaySnapshot,
    scene: u32,
    expected: &Option<String>,
) -> bool {
    match expected {
        Some(expected) => snapshot
            .scene_colors
            .as_ref()
            .and_then(|colors| colors.get(scene as usize))
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected)),
        None => true,
    }
}

/// Plan for a guarded preset recall, bank navigation or reload, from [`plan_preset_recall`].
#[derive(Debug, Clone, PartialEq)]
pub struct PresetRecallPlan {
    /// Setlist position command to send.
    pub command: DeviceCommand,
    /// Setlist key of the target preset.
    pub setlist_key: String,
    /// Target preset position within the setlist (0 through 255).
    pub position: u32,
    /// Position revision of the snapshot the plan was made from; confirmation needs a newer one.
    pub after_position_revision: u64,
    /// Full-preset revision of the snapshot the plan was made from; confirmation needs a newer
    /// one.
    pub after_full_preset_revision: u64,
    /// Whether the destination must report no unsaved changes (set for reloads).
    pub require_clean: bool,
    /// Short human-readable summary of the workflow, for the result reported to the caller.
    pub detail: String,
}

/// One write within a persistent preset workflow, and how to confirm it.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetMutationStage {
    /// The write to send.
    pub write: PlannedWrite,
    /// Device-state predicate that confirms this stage.
    pub verification: GatewayVerification,
    /// Time budget for this stage, in milliseconds.
    pub timeout_ms: u64,
    /// Quiet period in milliseconds to leave after this stage before sending the next persistent
    /// command.
    pub settle_ms: u64,
}

/// A preset catalog entry that a workflow expects to find on the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetMutationRecord {
    /// Setlist key of the folder holding the preset.
    pub setlist_key: String,
    /// Position of the preset within the setlist.
    pub position: u32,
    /// Expected preset name.
    pub name: String,
    /// Instrument value stored with the preset's catalog entry: `1` has been
    /// observed for guitar and `2` for bass, `0` means untagged. Values `4`
    /// and `5` have been seen rarely on other catalog entries; see
    /// [`crate::responses::LibraryEntry::instrument`] for what is known.
    pub instrument: i32,
    /// Whether the device may have appended a numeric suffix to resolve a name collision; see
    /// [`stored_preset_name_matches`].
    pub allow_collision_suffix: bool,
}

/// What a host needs to verify a setlist create or duplicate against the device's directory
/// catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCopyVerification {
    /// `create` for AddSetlistTask and `copy` for DuplicateSetlistTask.
    pub operation: String,
    /// Key of the source setlist. Empty for a create.
    pub source_key: String,
    /// Name of the source setlist; for a create, the name that was requested (before any numeric
    /// suffix was added to avoid a collision).
    pub source_name: String,
    /// Keys of every setlist in the catalog before the operation, with trailing slashes removed.
    pub before_folder_keys: Vec<String>,
    /// Exact occupied source inventory observed before the one non-replayable
    /// COPY. Create-setlist leaves this empty and skips copy matching.
    #[serde(default)]
    pub expected_presets: Vec<PresetMutationRecord>,
    /// Operation-specific host verification policy. This never changes the
    /// single persistent write; it only bounds subsequent read-only catalogs.
    #[serde(default)]
    pub catalog_policy: CatalogVerificationPolicy,
}

/// A duplicate is complete only when the new folder's complete File frame
/// contains the exact occupied source inventory. CorOS may publish the new
/// folder first and populate its copied presets in a later catalog generation.
pub fn duplicate_setlist_contents_match(
    expected: &[PresetMutationRecord],
    observed: &PresetList,
) -> bool {
    duplicate_setlist_contents_mismatch(expected, observed).is_none()
}

/// Explain an incomplete folder copy without including user preset names.
/// Position lists are bounded so this text is safe to retain in conformance
/// evidence even when a malformed catalog contains hundreds of records.
pub fn duplicate_setlist_contents_mismatch(
    expected: &[PresetMutationRecord],
    observed: &PresetList,
) -> Option<String> {
    let occupied = observed
        .presets
        .iter()
        .filter(|preset| preset.product_key.is_some())
        .collect::<Vec<_>>();
    let missing = expected
        .iter()
        .filter(|source| {
            !occupied
                .iter()
                .any(|candidate| candidate.position == source.position)
        })
        .map(|source| source.position)
        .collect::<Vec<_>>();
    let unexpected = occupied
        .iter()
        .filter(|candidate| {
            !expected
                .iter()
                .any(|source| source.position == candidate.position)
        })
        .map(|candidate| candidate.position)
        .collect::<Vec<_>>();
    let name_mismatches = expected
        .iter()
        .filter(|source| {
            occupied.iter().any(|candidate| {
                candidate.position == source.position && candidate.name != source.name
            })
        })
        .map(|source| source.position)
        .collect::<Vec<_>>();
    let instrument_mismatches = expected
        .iter()
        .filter(|source| {
            occupied.iter().any(|candidate| {
                candidate.position == source.position && candidate.instrument != source.instrument
            })
        })
        .map(|source| source.position)
        .collect::<Vec<_>>();
    if expected.len() == occupied.len()
        && missing.is_empty()
        && unexpected.is_empty()
        && name_mismatches.is_empty()
        && instrument_mismatches.is_empty()
    {
        return None;
    }

    fn bounded_positions(positions: &[u32]) -> String {
        const LIMIT: usize = 8;
        let mut value = positions
            .iter()
            .take(LIMIT)
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        if positions.len() > LIMIT {
            value.push_str(&format!(",+{} more", positions.len() - LIMIT));
        }
        if value.is_empty() {
            value.push_str("none");
        }
        value
    }

    Some(format!(
        "duplicate inventory mismatch: expected {} occupied preset(s), observed {}; missing positions [{}]; unexpected positions [{}]; name mismatches [{}]; instrument mismatches [{}]",
        expected.len(),
        occupied.len(),
        bounded_positions(&missing),
        bounded_positions(&unexpected),
        bounded_positions(&name_mismatches),
        bounded_positions(&instrument_mismatches),
    ))
}

/// Evidence that a single preset deletion landed in the device catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetDeletionVerification {
    /// Setlist key of the folder that held the preset.
    pub setlist_key: String,
    /// Device file key of the preset that was deleted.
    pub product_key: String,
    /// Name the preset was expected to have.
    pub expected_name: String,
    /// Position the preset was expected to occupy.
    pub expected_position: u32,
}

/// Evidence that a setlist deletion landed in the device catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct SetlistDeletionVerification {
    /// Setlist key of the deleted folder.
    pub setlist_key: String,
}

/// A persistent preset or setlist workflow planned once for all hosts, from
/// [`plan_preset_mutation`].
#[derive(Debug, Clone, PartialEq)]
pub struct PresetMutationPlan {
    /// Writes to perform in order, each confirmed before the next.
    pub stages: Vec<PresetMutationStage>,
    /// Best-effort navigation restoration that hosts must execute even when a
    /// preceding stage has an ambiguous result. Cleanup writes are idempotent
    /// recalls, never persistent mutation replays.
    pub cleanup_stages: Vec<PresetMutationStage>,
    /// Short human-readable summary of the workflow, for the result reported to the caller.
    pub detail: String,
    /// Name of the preset or setlist the workflow produces or acts on. Empty for a setlist
    /// deletion.
    pub saved_name: String,
    /// Setlist key the workflow targets.
    pub setlist_key: String,
    /// Position the workflow targets; 0 for setlist-level workflows.
    pub position: u32,
    /// Instrument value associated with the preset (`1` has been observed for
    /// guitar and `2` for bass, `0` means untagged; values `4` and `5` have been
    /// seen rarely on other catalog entries, see
    /// [`crate::responses::LibraryEntry::instrument`]); 0 for setlist-level
    /// workflows.
    pub instrument: i32,
    /// Shared host readback policy for the already-dispatched persistent File
    /// write. It changes only read scheduling and never permits write replay.
    pub catalog_policy: CatalogVerificationPolicy,
    /// Catalog entries expected once the workflow completes.
    pub saved_presets: Vec<PresetMutationRecord>,
    /// Set for a setlist create or duplicate; the evidence used to verify it.
    pub folder_copy: Option<FolderCopyVerification>,
    /// Set for a single-preset deletion; the evidence used to verify it.
    pub deletion: Option<PresetDeletionVerification>,
    /// Set for a setlist deletion; the evidence used to verify it.
    pub setlist_deletion: Option<SetlistDeletionVerification>,
}

impl PresetMutationPlan {
    /// Folder deletion carries a typed native-catalog baseline, independently
    /// of the exact-product-key predicate used for deleting a single preset.
    pub fn deletes_setlist(&self) -> bool {
        self.setlist_deletion.is_some()
    }

    /// Whether verification needs the device's directory catalog, which is the case for a setlist
    /// create, duplicate or delete.
    pub fn needs_directory_catalog(&self) -> bool {
        self.folder_copy.is_some() || self.deletes_setlist()
    }
}

/// Match the name CorOS persisted after resolving a collision. Short names
/// retain the complete requested name. For longer names CorOS truncates the
/// base so the base plus underscore and decimal suffix occupy 20 characters.
/// When the requested name already ends in a canonical numeric suffix, the
/// stored collision name may use a greater suffix on that same stem (for
/// example, `Name_1` to `Name_2`). The observed increment is compatible with
/// CorOS finding an unused suffix; the exact allocator sequence is firmware
/// behavior. Keep reconciliation tied to the exact stem and a greater number.
pub fn stored_preset_name_matches(requested: &str, stored: &str) -> bool {
    if requested == stored {
        return true;
    }
    let Some((stored_base, suffix)) = stored.rsplit_once('_') else {
        return false;
    };
    if suffix.is_empty() || !suffix.chars().all(|character| character.is_ascii_digit()) {
        return false;
    }
    if stored_base == requested {
        return true;
    }

    let canonical_suffix = |value: &str| {
        !value.is_empty()
            && !(value.len() > 1 && value.starts_with('0'))
            && value.bytes().all(|byte| byte.is_ascii_digit())
    };
    let collision_suffix_matches = match requested.rsplit_once('_') {
        Some((requested_stem, requested_suffix)) if !requested_stem.is_empty() => {
            let requested_suffix_is_positive = requested_suffix.bytes().any(|byte| byte != b'0');
            let stored_suffix_is_greater = suffix.len() > requested_suffix.len()
                || (suffix.len() == requested_suffix.len() && suffix > requested_suffix);
            if canonical_suffix(requested_suffix)
                && requested_suffix_is_positive
                && canonical_suffix(suffix)
                && stored_suffix_is_greater
            {
                let available_stem_characters = domain::STORED_PRESET_NAME_CHARACTERS
                    .saturating_sub(suffix.chars().count() + 1);
                let expected_stored_stem: String = requested_stem
                    .chars()
                    .take(available_stem_characters)
                    .collect();
                stored_base == expected_stored_stem
            } else {
                false
            }
        }
        _ => false,
    };

    let requested_has_numeric_suffix = requested.rsplit_once('_').is_some_and(|(_, value)| {
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
    });

    collision_suffix_matches
        || (!requested_has_numeric_suffix
            && stored.chars().count() == domain::STORED_PRESET_NAME_CHARACTERS
            && stored_base.chars().count() < requested.chars().count()
            && requested.starts_with(stored_base))
}

/// Reconcile the active snapshot from an authoritative File-catalog entry.
/// CorOS does not always publish a new SetlistPosition frame when the current
/// slot is renamed, although the persisted File entry is already updated.
pub fn reconcile_saved_preset_snapshot(
    snapshot: &mut GatewaySnapshot,
    setlist_key: &str,
    position: u32,
    name: &str,
) -> bool {
    if snapshot.setlist_key.trim_end_matches('/') != setlist_key.trim_end_matches('/')
        || snapshot.preset_position != position
    {
        return false;
    }
    snapshot.preset_name = name.to_string();
    true
}

/// Which decoder to apply to the correlated reply of a planned gateway read, and the request
/// context it needs. See [`GatewayResponseProjection::decode`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GatewayResponseProjection {
    /// Device identity (version) reply.
    DeviceIdentity,
    /// Diagnostics reply.
    DeviceDiagnostics {
        /// Request identifier the reply must echo.
        request_id: u64,
    },
    /// Tuner settings reply.
    TunerSettings,
    /// General settings reply.
    GeneralSettings,
    /// Input/output settings reply.
    IoSettings,
    /// Global EQ reply.
    GlobalEq,
    /// Mode cycle reply.
    ModeCycle,
    /// Global tempo settings reply.
    GlobalTempoSettings,
    /// Preset tempo and metronome settings, read through the bare current-preset READ.
    PresetTempoSettings,
    /// Current preset acknowledgement reply to the bare current-preset READ.
    CurrentPreset,
    /// Looper status reply.
    LooperStatus,
    /// Recents reply.
    RecentsFavorites {
        /// Request identifier the reply must echo.
        request_id: u64,
    },
    /// Favorites reply.
    Favorites {
        /// Request identifier the reply must echo.
        request_id: u64,
    },
    /// Pinned models reply.
    PinnedModels,
    /// Inhibited modules reply.
    InhibitedModules,
    /// Preset screenshot reply, decoded to a PNG image.
    PresetScreenshot {
        /// Request identifier the reply must echo.
        request_id: u64,
        /// Name of the folder that holds the preset.
        folder_name: String,
        /// Position of the preset within the folder.
        position: u32,
        /// Whether the preset is in the factory library.
        is_factory: bool,
    },
    /// Captured screen reply, decoded to a PNG image.
    CapturedScreen,
    /// Graphics tree reply.
    GraphicsTree,
}

fn image_response(image: PngImage) -> Value {
    serde_json::json!({
        "pngBase64": BASE64.encode(image.bytes),
        "width": image.width,
        "height": image.height,
    })
}

impl GatewayResponseProjection {
    /// The request identifier the reply must echo, or `None` for projections that are not
    /// correlated by request identifier.
    pub fn expected_request_id(&self) -> Option<u64> {
        match self {
            Self::DeviceDiagnostics { request_id }
            | Self::RecentsFavorites { request_id }
            | Self::Favorites { request_id }
            | Self::PresetScreenshot { request_id, .. } => Some(*request_id),
            _ => None,
        }
    }

    /// Decode the planned correlated reply into the public gateway shape.
    /// Keeping this beside the read plan prevents native hosts from owning
    /// subtly different protobuf validation and image projection rules.
    pub fn decode(&self, payload: &[u8]) -> Result<Value, String> {
        match self {
            Self::DeviceIdentity => serde_json::to_value(
                decode_device_identity(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::DeviceDiagnostics { request_id } => serde_json::to_value(
                decode_diagnostics(payload, Some(*request_id))
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::TunerSettings => serde_json::to_value(
                decode_tuner_settings(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::GeneralSettings => serde_json::to_value(
                decode_general_settings(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::IoSettings => serde_json::to_value(
                decode_io_settings(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::GlobalEq => {
                serde_json::to_value(decode_global_eq(payload).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())
            }
            Self::ModeCycle => {
                let cycle = decode_mode_cycle(payload).map_err(|error| error.to_string())?;
                if !is_mode_cycle_value(&cycle.slots) {
                    return Err(
                        "the QC mode cycle is outside the representable mode-editor domain".into(),
                    );
                }
                serde_json::to_value(cycle).map_err(|error| error.to_string())
            }
            Self::GlobalTempoSettings => serde_json::to_value(
                decode_global_tempo_settings(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::PresetTempoSettings => serde_json::to_value(
                decode_preset_tempo_settings(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::CurrentPreset => serde_json::to_value(
                decode_current_preset_ack(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::LooperStatus => serde_json::to_value(
                decode_looper_status(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::RecentsFavorites { request_id } => serde_json::to_value(
                decode_recents_favorites(payload, *request_id)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::Favorites { request_id } => serde_json::to_value(
                decode_favorites(payload, *request_id).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::PinnedModels => serde_json::to_value(
                decode_pinned_models(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::InhibitedModules => serde_json::to_value(
                decode_inhibited_modules(payload).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
            Self::PresetScreenshot {
                request_id,
                folder_name,
                position,
                is_factory,
            } => Ok(image_response(
                decode_preset_screenshot(payload, *request_id, folder_name, *position, *is_factory)
                    .map_err(|error| error.to_string())?,
            )),
            Self::CapturedScreen => Ok(image_response(
                decode_captured_screen(payload).map_err(|error| error.to_string())?,
            )),
            Self::GraphicsTree => Ok(serde_json::json!({
                "tree": decode_graphics_tree(payload).map_err(|error| error.to_string())?
            })),
        }
    }
}

/// Compose the device-wide tempo mode/value with the loaded preset's
/// metronome controls. CorOS exposes these through two different replies.
pub fn compose_global_tempo_settings(global: &Value, preset: &Value) -> Result<Value, String> {
    let mode = global
        .get("mode")
        .cloned()
        .ok_or_else(|| "The global tempo reply did not include PRESET/GLOBAL mode".to_string())?;
    let global_bpm = global
        .get("bpm")
        .cloned()
        .ok_or_else(|| "The global tempo reply did not include its BPM value".to_string())?;
    let mut result = preset.clone();
    let object = result
        .as_object_mut()
        .ok_or_else(|| "The preset tempo reply was not an object".to_string())?;
    object.insert("mode".into(), mode);
    object.insert("globalBpm".into(), global_bpm);
    Ok(result)
}

/// The retained settings publication whose sparse echo confirms a write, for the two settings
/// families CorOS publishes rather than answers (type 9 general settings and type 3 I/O).
///
/// Host policy (not a Cortex Control frame): Cortex Control waits for nothing after any of these
/// writes. The host only watches the QC's own sparse echo of the write in the retained publication
/// and sends no frame; no echo inside the window gives `accepted_unverified`. Every other write
/// (global EQ, mode cycle, tempo and metronome, favorites, pinned models, tuner, device name) has
/// no readback of any kind: it sends its frames and returns `accepted_unverified`. Earlier
/// versions polled a READ for those after the write; that is removed (Cortex Control never reads
/// a write back, and the poll's failure mode quarantined later writes).
pub fn gateway_write_readback_method(method: &str) -> Option<&'static str> {
    match method {
        // setGlobalBypass is excluded: this path has no acknowledgement
        // callback, correlated readback, retry, or post-enqueue error branch.
        "device.setGeneralInteger"
        | "device.setGeneralToggle"
        | "device.setSceneBypassBehavior"
        | "device.setMasterVolumeAssignment"
        | "device.setLooperStompAssignments" => Some("device.generalSettings"),
        "device.setInputPort"
        | "device.setOutputPort"
        | "device.setUsbPort"
        | "device.setMidiThru"
        | "device.setOutputPairing" => Some("device.ioSettings"),
        _ => None,
    }
}

/// Why this host does not offer an explicit directory refresh, in the words a caller sees.
///
/// Host policy (not a desktop-application frame): `refresh=true` is refused with this text, the
/// retained directory (the boot publication plus every File update the unit pushes since) is
/// served, and nothing is sent. It does NOT claim the unit cannot answer a directory read: that
/// belief is superseded. ESTABLISHED on CorOS 4.1.0 (direct USB, 2026-10-07): the unit answers a
/// bare directory READ (`readFolders(false)`, T4 `08 03 58 00`) on demand every time, within
/// seconds. An explicit refresh is therefore possible and is simply not implemented. Two earlier
/// recorded sessions saw no inbound File traffic after such a read (raw files
/// not kept); the later measurement outranks them.
pub use crate::generated_host_policy::PRESET_CATALOG_REFRESH_UNAVAILABLE;

/// Which readbacks are answered by a publication the QC pushes on its own. After the removal of
/// every correlated READ ladder, every readback this host has is of that kind.
///
/// CorOS answers a type-3 / type-9 write with a sparse settings echo that the worker merges into
/// its publication cache; that it does not answer an on-demand READ of either type is BELIEVED
/// (recorded in comments without a date or artifact). A host waits for that echo to stay in sync.
pub fn gateway_readback_is_published(read_method: &str) -> bool {
    matches!(read_method, "device.generalSettings" | "device.ioSettings")
}

/// The decoded observation a write's stale-state guard consults. No preflight sends a READ.
///
/// Host policy (not a Cortex Control frame; Cortex Control reads nothing before any write). The
/// guard compares the caller's `expected_*` arguments with state the session has already folded
/// from what the QC pushed, and fails closed with a clear error when that state has not been
/// published to this session. `selectScene` and `setTempoMode` additionally skip a write that
/// already matches, which Cortex Control's own UI does too; `setGlobalTempo`, `setFavorite` and
/// `setModelPinned` are stale-state guards only and never skip the frame (Cortex Control sends
/// those without checking). The names returned here identify the observation, not a read.
pub fn gateway_write_preflight_method(method: &str) -> Option<&'static str> {
    match method {
        "device.selectScene" => Some("device.currentPreset"),
        "device.setTempoMode" | "device.setGlobalTempo" => Some("device.globalTempoSettings"),
        "device.setFavorite" => Some("device.favorites"),
        "device.setModelPinned" => Some("device.pinnedModels"),
        // tapScreen/swipeScreen deliberately have no preflight. The capture
        // they used to force was never examined - the decision falls through
        // to Proceed - so it cost a full 800x480 framebuffer (~671 ms) to
        // prove only that the device had answered something. Verifying what
        // is on screen is the caller's job, with capture_screen, before
        // invoking a gesture.
        _ => None,
    }
}

/// Selects the authoritative observation consumed by a write preflight. Every preflight consumes
/// the decoded state the session folds; none reads the wire.
pub fn gateway_write_preflight_observation_source(
    method: &str,
) -> Option<GatewayPreflightObservationSource> {
    gateway_write_preflight_method(method)
        .map(|_| GatewayPreflightObservationSource::DecodedSnapshot)
}

/// What a caller is told when a write's stale-state guard has no published state to compare
/// against. Shared by both hosts so neither invents its own wording. Every message says nothing
/// was sent and none asks the host to read: a guard that cannot be checked fails closed.
pub fn gateway_preflight_unavailable_message(method: &str) -> String {
    match method {
        "device.selectScene" => "The Quad Cortex has not published a preset to this session yet, so the scene guard cannot be checked; nothing was sent.".into(),
        "device.setTempoMode" | "device.setGlobalTempo" => "The device-global tempo state (type 33) has not been published to this session yet, so the tempo guard cannot be checked; nothing was sent. The host sends no read to find out.".into(),
        "device.setFavorite" => "The favorites list is unconfirmed in this session (it has not been read here, or an accepted change or a partial update since made it unconfirmed), so expected_favorite cannot be checked; nothing was sent. Call list_favorites, then retry with the state it reports.".into(),
        "device.setModelPinned" => "The pinned-model list is unconfirmed in this session (it has not been published here, or an accepted change or a partial update since made it unconfirmed), so expected_pinned cannot be checked; nothing was sent. Call list_pinned_models, then retry with the state it reports.".into(),
        _ => format!("{method} has no published state to check its guard against; nothing was sent."),
    }
}

/// Outcome of an authoritative write preflight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayWritePreflightDecision {
    /// The decoded state is as the caller expected; send the write.
    Proceed,
    /// The decoded state already holds the requested value; the write can be skipped.
    /// Host policy (not a Cortex Control frame). Cortex Control makes the same
    /// skip for scene selection and tempo mode only; for every other write it sends the
    /// frame regardless, and so does this host.
    AlreadyMatched,
}

pub(crate) fn external_preflight_parameters(method: &str, params: &Value) -> Option<Value> {
    let keys: &[&str] = match method {
        "device.selectScene" => &["scene", "expectedPresetName"],
        "device.setTempoMode" => &["mode"],
        "device.setGlobalTempo" => &["expectedMode", "expectedGlobalBpm", "bpm"],
        "device.setModelPinned" => &["modelId", "expectedPinned", "pinned"],
        "device.setFavorite" => &[
            "name",
            "folderKey",
            "productKey",
            "isFactory",
            "isPlugin",
            "expectedFavorite",
            "favorite",
        ],
        _ => return None,
    };
    Some(Value::Object(
        keys.iter()
            .filter_map(|key| params.get(*key).map(|value| ((*key).into(), value.clone())))
            .collect(),
    ))
}

/// Retain only the setting or membership on which this preflight depends.
/// A change to a different model/favorite or metronome field is not a conflict.
pub(crate) fn external_preflight_target(
    method: &str,
    params: &Value,
    response: &Value,
) -> Result<Value, String> {
    match method {
        "device.selectScene" => {
            let name = response
                .get("presetName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or("No observed preset name for scene preflight")?;
            let scene = response
                .get("activeScene")
                .and_then(Value::as_u64)
                .filter(|scene| *scene < u64::from(domain::SCENE_COUNT))
                .ok_or("No observed active scene for native preflight")?;
            Ok(serde_json::json!([name, scene]))
        }
        "device.setTempoMode" | "device.setGlobalTempo" => {
            let mode = response
                .get("mode")
                .and_then(Value::as_str)
                .filter(|mode| matches!(*mode, "PRESET" | "GLOBAL"))
                .ok_or("No observed global tempo mode for native preflight")?;
            if method == "device.setTempoMode" {
                return Ok(serde_json::json!(mode));
            }
            let bpm = response
                .get("globalBpm")
                .and_then(Value::as_u64)
                .filter(|bpm| (40..=240).contains(bpm))
                .ok_or("No observed global BPM for native preflight")?;
            Ok(serde_json::json!([mode, bpm]))
        }
        "device.setModelPinned" => {
            let id = params
                .get("modelId")
                .and_then(Value::as_u64)
                .ok_or("Missing pinned model target")?;
            let models = response
                .get("models")
                .and_then(Value::as_array)
                .ok_or("No observed pinned models for native preflight")?;
            Ok(serde_json::json!(models
                .iter()
                .any(|value| value.as_u64() == Some(id))))
        }
        "device.setFavorite" => Ok(serde_json::json!(favorite_response_contains(
            params, response
        )?)),
        _ => Err("No external preflight target for this method".into()),
    }
}

/// Decide whether an authoritative preflight permits, suppresses, or rejects a write.
pub fn gateway_write_preflight_decision(
    method: &str,
    params: &Value,
    response: &Value,
) -> Result<GatewayWritePreflightDecision, String> {
    match method {
        "device.selectScene" => {
            let desired = params
                .get("scene")
                .and_then(Value::as_u64)
                .filter(|scene| *scene < u64::from(domain::SCENE_COUNT))
                .ok_or_else(|| "scene must be an integer from 0 through 7".to_string())?;
            let expected_preset = params
                .get("expectedPresetName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "expectedPresetName must be a non-empty string".to_string())?;
            let actual_preset = response
                .get("presetName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    "authoritative scene preflight did not contain a presetName".to_string()
                })?;
            if actual_preset != expected_preset {
                return Err(format!(
                    "write was based on a stale active preset (expected {expected_preset:?}, observed {actual_preset:?})"
                ));
            }
            let actual = response
                .get("activeScene")
                .and_then(Value::as_u64)
                .filter(|scene| *scene < u64::from(domain::SCENE_COUNT))
                .ok_or_else(|| {
                    "authoritative scene preflight did not contain an activeScene from 0 through 7"
                        .to_string()
                })?;
            Ok(if actual == desired {
                GatewayWritePreflightDecision::AlreadyMatched
            } else {
                GatewayWritePreflightDecision::Proceed
            })
        }
        "device.setTempoMode" => {
            let expected = params
                .get("mode")
                .and_then(Value::as_str)
                .filter(|mode| matches!(*mode, "PRESET" | "GLOBAL"))
                .ok_or_else(|| "mode must be PRESET or GLOBAL".to_string())?;
            let actual = response
                .get("mode")
                .and_then(Value::as_str)
                .filter(|mode| matches!(*mode, "PRESET" | "GLOBAL"))
                .ok_or_else(|| {
                    "authoritative tempo-mode preflight did not contain PRESET or GLOBAL"
                        .to_string()
                })?;
            Ok(if actual == expected {
                GatewayWritePreflightDecision::AlreadyMatched
            } else {
                GatewayWritePreflightDecision::Proceed
            })
        }
        "device.setGlobalTempo" => {
            // Host policy (not a desktop-application frame): the expected mode and
            // global BPM are stale-state guards that protect a value the caller
            // did not see. The frame is sent in either tempo mode; the desktop
            // application has no mode precondition, and in PRESET mode the unit
            // edits the loaded preset's tempo instead of ignoring the write
            // (ESTABLISHED, CorOS 4.1.0, 2026-10-07).
            if supplied_fields_match(
                params,
                response,
                &[("expectedMode", "mode"), ("expectedGlobalBpm", "globalBpm")],
            ) {
                Ok(GatewayWritePreflightDecision::Proceed)
            } else {
                Err("write was based on stale global tempo state".into())
            }
        }
        "device.setFavorite" => {
            let expected = params
                .get("expectedFavorite")
                .and_then(Value::as_bool)
                .ok_or_else(|| "expectedFavorite must be a boolean".to_string())?;
            let actual = favorite_response_contains(params, response)?;
            if actual != expected {
                return Err("write was based on stale favorite state".into());
            }
            // The favorite flag is validated but never used to skip the frame:
            // Cortex Control sends the CREATE or DELETE without comparing it
            // with the current state.
            boolean(params, "favorite")?;
            Ok(GatewayWritePreflightDecision::Proceed)
        }
        "device.setModelPinned" => {
            let model_id = params
                .get("modelId")
                .and_then(Value::as_u64)
                .filter(|value| (1..=i32::MAX as u64).contains(value))
                .ok_or_else(|| {
                    "modelId must be an integer from 1 through 2147483647".to_string()
                })?;
            let expected = params
                .get("expectedPinned")
                .and_then(Value::as_bool)
                .ok_or_else(|| "expectedPinned must be a boolean".to_string())?;
            params
                .get("pinned")
                .and_then(Value::as_bool)
                .ok_or_else(|| "pinned must be a boolean".to_string())?;
            let models = response
                .get("models")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    "authoritative pinned-model preflight did not contain models".to_string()
                })?;
            let actual = models.iter().any(|value| value.as_u64() == Some(model_id));
            if actual != expected {
                return Err("write was based on stale pinned-model state".into());
            }
            // `pinned` is validated but never used to skip the frame: Cortex
            // Control sends the pin or unpin without comparing it with the
            // current state.
            Ok(GatewayWritePreflightDecision::Proceed)
        }
        _ => Ok(GatewayWritePreflightDecision::Proceed),
    }
}

/// Compatibility predicate for callers that only need permit/reject semantics.
pub fn gateway_write_preflight_matches(method: &str, params: &Value, response: &Value) -> bool {
    gateway_write_preflight_decision(method, params, response).is_ok()
}

fn json_number_matches(actual: Option<&Value>, expected: &Value) -> bool {
    actual
        .and_then(Value::as_f64)
        .zip(expected.as_f64())
        .is_some_and(|(actual, expected)| (actual - expected).abs() <= 0.001)
}

fn supplied_fields_match(params: &Value, response: &Value, fields: &[(&str, &str)]) -> bool {
    fields
        .iter()
        .all(|(argument, output)| match params.get(*argument) {
            None | Some(Value::Null) => true,
            Some(expected) if expected.is_number() => {
                json_number_matches(response.get(*output), expected)
            }
            Some(expected) => response.get(*output) == Some(expected),
        })
}

fn favorite_response_contains(params: &Value, response: &Value) -> Result<bool, String> {
    let name = required_text(params, "name")?;
    let folder_key = required_text(params, "folderKey")?;
    let product_key = required_text(params, "productKey")?;
    let is_factory = boolean(params, "isFactory")?;
    let is_plugin = params
        .get("isPlugin")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entries = response
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "authoritative favorites reply did not contain entries".to_string())?;
    Ok(entries.iter().any(|entry| {
        entry.get("name").and_then(Value::as_str) == Some(name.as_str())
            && entry.get("folderKey").and_then(Value::as_str) == Some(folder_key.as_str())
            && entry.get("isFactory").and_then(Value::as_bool) == Some(is_factory)
            && entry.get("isPlugin").and_then(Value::as_bool) == Some(is_plugin)
            && (entry.get("productKey").and_then(Value::as_str) == Some(product_key.as_str())
                || (is_plugin && entry.get("productKey").is_none_or(Value::is_null)))
    }))
}

/// Compare a retained settings publication with the exact fields a write changed. Only the two
/// published families (type 9 general settings, type 3 I/O) have a readback; see
/// [`gateway_write_readback_method`].
pub fn gateway_write_readback_matches(method: &str, params: &Value, response: &Value) -> bool {
    match method {
        "device.setGeneralInteger" | "device.setGeneralToggle" => {
            let Some(setting) = params.get("setting").and_then(Value::as_str) else {
                return false;
            };
            let output = if setting == "holdTiming" {
                "holdTimingIndex"
            } else {
                setting
            };
            let expected = params.get("value").or_else(|| params.get("enabled"));
            expected.is_some_and(|expected| {
                if expected.is_number() {
                    json_number_matches(response.get(output), expected)
                } else {
                    response.get(output) == Some(expected)
                }
            })
        }
        "device.setSceneBypassBehavior" => {
            supplied_fields_match(params, response, &[("behavior", "sceneBypassBehavior")])
        }
        "device.setMasterVolumeAssignment" => {
            response
                .get("masterVolumeAssignment")
                .is_some_and(|nested| {
                    supplied_fields_match(
                        params,
                        nested,
                        &[
                            ("out12", "out12"),
                            ("out34", "out34"),
                            ("send12", "send12"),
                            ("headphones", "headphones"),
                        ],
                    )
                })
        }
        "device.setGlobalBypass" => ["cab", "ir"].iter().all(|name| {
            // A family left null or absent was not sent, so it is not compared.
            if params.get(*name).is_none_or(Value::is_null) {
                return true;
            }
            let Some(expected) = params.get(*name).and_then(Value::as_array) else {
                return false;
            };
            let output = if *name == "cab" {
                "globalBypassCab"
            } else {
                "globalBypassIr"
            };
            let Some(actual) = response.get(output).and_then(Value::as_object) else {
                return false;
            };
            expected
                .iter()
                .enumerate()
                .all(|(index, value)| actual.get(&format!("row{}", index + 1)) == Some(value))
        }),
        "device.setLooperStompAssignments" => supplied_fields_match(
            params,
            response,
            &[("assignments", "looperStompAssignments")],
        ),
        "device.setInputPort" => response
            .get("inputs")
            .and_then(Value::as_array)
            .and_then(|ports| {
                let id = params.get("inputPortId")?;
                let mut matching = ports
                    .iter()
                    .filter(|port| port.get("inputPortId") == Some(id));
                let found = matching.next()?;
                matching.next().is_none().then_some(found)
            })
            .is_some_and(|port| {
                supplied_fields_match(
                    params,
                    port,
                    &[
                        ("levelDb", "levelDb"),
                        ("impedance", "impedance"),
                        ("inputType", "inputType"),
                        ("groundLift", "groundLift"),
                    ],
                )
            }),
        "device.setOutputPort" => response
            .get("outputs")
            .and_then(Value::as_array)
            .and_then(|ports| {
                let id = params.get("outputPortId")?;
                let mut matching = ports
                    .iter()
                    .filter(|port| port.get("outputPortId") == Some(id));
                let found = matching.next()?;
                matching.next().is_none().then_some(found)
            })
            .is_some_and(|port| {
                supplied_fields_match(
                    params,
                    port,
                    &[
                        // `level` is supplied in dB (`optional_output_level`
                        // bounds it to -40..=12); the response's `level` is the
                        // normalized wire value. Comparing them agreed at
                        // exactly one point, 0, so the only output level that
                        // ever reported a successful readback was the one that
                        // pulls the mains down. Compare dB against the decoder's
                        // published dB, as `device.setInputPort` already does.
                        ("level", "levelDb"),
                        ("groundLift", "groundLift"),
                        ("mute", "muted"),
                    ],
                )
            }),
        "device.setUsbPort" => response.get("usb").is_some_and(|usb| {
            supplied_fields_match(
                params,
                usb,
                &[
                    ("level", "level"),
                    ("headphonesSource", "headphonesSource"),
                    ("dryWet", "dryWet"),
                ],
            )
        }),
        "device.setMidiThru" => params
            .get("enabled")
            .and_then(Value::as_bool)
            .zip(
                response
                    .get("midi")
                    .and_then(|midi| midi.get("thru"))
                    .and_then(Value::as_f64),
            )
            .is_some_and(|(enabled, thru)| (thru - if enabled { 1.0 } else { 0.0 }).abs() <= 0.001),
        "device.setOutputPairing" => supplied_fields_match(
            params,
            response,
            &[
                ("xlr12Linked", "xlr12Linked"),
                ("out34Linked", "out34Linked"),
            ],
        ),
        _ => false,
    }
}

/// Match only fields actually carried by one fresh settings echo, never a merged cache.
/// The host must independently fence this frame to the same session and after write admission.
/// Missing fields, malformed payloads, other message families and ambiguous empty repeated
/// fields cannot prove the requested setting. This does not invent a wire request identifier.
pub fn gateway_write_echo_matches(
    method: &str,
    params: &Value,
    message_type: u16,
    payload: &[u8],
) -> bool {
    let response = match (gateway_write_readback_method(method), message_type) {
        (Some("device.generalSettings"), profile::MESSAGE_TYPE_GENERAL_SETTINGS) => {
            decode_general_settings(payload)
                .ok()
                .and_then(|value| serde_json::to_value(value).ok())
        }
        (Some("device.ioSettings"), profile::MESSAGE_TYPE_IO_SETTINGS) => {
            crate::responses::decode_io_settings_echo(payload)
                .ok()
                .and_then(|value| serde_json::to_value(value).ok())
        }
        _ => None,
    };
    response.is_some_and(|response| gateway_write_readback_matches(method, params, &response))
}

/// Plan for a correlated gateway read, from [`plan_gateway_read`].
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayReadPlan {
    /// The operation to send.
    pub operation: DeviceOperation,
    /// Message type of the expected reply.
    pub response_type: u16,
    /// How long to wait for the reply, in milliseconds.
    pub timeout_ms: u64,
    /// How to decode the reply.
    pub projection: GatewayResponseProjection,
}

/// Additional authoritative read required to complete a public gateway read.
/// The hosts execute the dependency; the device-specific composition remains
/// in this runtime via [`compose_global_tempo_settings`].
pub fn gateway_read_followup_method(method: &str) -> Option<&'static str> {
    (method == "device.globalTempoSettings").then_some("device.presetTempoSettings")
}

impl PresetRecallPlan {
    /// Match the requested destination without comparing generation-local
    /// revision counters. A fully reinitialized session is itself fresh, but
    /// its counters restart from zero.
    pub fn matches_recovered(&self, snapshot: &GatewaySnapshot) -> bool {
        snapshot.setlist_key.trim_end_matches('/') == self.setlist_key.trim_end_matches('/')
            && snapshot.preset_position == self.position
            && (!self.require_clean || !snapshot.dirty)
    }

    /// Whether `snapshot` shows the requested destination, requiring a newer position and
    /// full-preset revision than the snapshot the plan was made from.
    pub fn matches(&self, snapshot: &GatewaySnapshot) -> bool {
        self.matches_recovered(snapshot)
            && snapshot.position_revision > self.after_position_revision
            && snapshot.full_preset_revision > self.after_full_preset_revision
    }

    /// The [`GatewayVerification::Preset`] predicate that confirms this recall.
    pub fn verification(&self) -> GatewayVerification {
        GatewayVerification::Preset {
            setlist_key: self.setlist_key.clone(),
            position: self.position,
            name: None,
            require_clean: self.require_clean,
            after_position_revision: Some(self.after_position_revision),
            after_full_preset_revision: Some(self.after_full_preset_revision),
        }
    }

    /// The result to report once [`verification`](Self::verification) matched `after`; see
    /// [`preset_address_observed_result`]. `None` never happens for a plan from
    /// [`plan_preset_recall`].
    pub fn address_observed_result(&self, after: &GatewaySnapshot) -> Option<Value> {
        preset_address_observed_result(&self.verification(), after)
    }

    /// The result detail when [`verification`](Self::verification) did not match within
    /// `timeout_ms`. That predicate needs ALL of: a newer position update naming the requested
    /// address, a newer full preset publication and, for a reload, a clean flag. A timeout
    /// means only that they were not all observed together in the bound, so the text names the
    /// required conditions and never asserts which one was missing: the host may have seen
    /// the position without a newer body, a body without the position, or both without the
    /// clean flag. The snapshot available at the timeout is a later read of the mirror, not
    /// evidence of what arrived inside the bound, and its counters restart on a session
    /// rebuild, so the missing condition is not derived from it.
    /// [`detail`](Self::detail) names the requested action, never an outcome.
    pub fn timeout_detail(&self, timeout_ms: u64) -> String {
        let address = format!(
            "{} position {}",
            self.setlist_key.trim_end_matches('/'),
            self.position
        );
        if self.require_clean {
            format!(
                "{}; the frame was sent, but within {timeout_ms} ms the host did not observe all of the required conditions together (a newer position update naming {address}, a newer full preset publication and a clean flag), so the discard is not verified",
                self.detail
            )
        } else {
            format!(
                "{}; the frame was sent, but within {timeout_ms} ms the host did not observe all of the required conditions together (a newer position update naming {address} and a newer full preset publication), so the effect is not verified",
                self.detail
            )
        }
    }
}

/// Whether `detail` claims verification: it says "verified" anywhere other than inside "not
/// verified" or "unverified". A result reported with `verified: false` must never claim it,
/// so the hosts' tests assert this over every unverified detail they build.
pub fn detail_claims_verification(detail: &str) -> bool {
    detail
        .to_ascii_lowercase()
        .replace("not verified", "")
        .replace("unverified", "")
        .contains("verified")
}

/// `observation.kind` of a recall, bank navigation or reload result: the host observed a
/// position update naming the requested preset address, and nothing bound the preset body
/// to that address.
pub const PRESET_ADDRESS_OBSERVED: &str = "preset_address_observed";

/// The result of a recall, bank navigation or reload whose predicate from
/// [`PresetRecallPlan::verification`] matched `after`. `None` for any other predicate, including
/// a save's [`GatewayVerification::Preset`], which carries no revisions.
///
/// That predicate OBSERVES the address: a SetlistPosition update naming the requested setlist
/// and slot is newer than the captured baseline, which may precede the queued physical send.
/// That is an observation, not causal proof: the
/// update carries no request id, so a queued or unrelated position update (a footswitch, an
/// earlier recall) satisfies it as well. It cannot bind the BODY either. A type-15 preset body
/// carries no address and no device identity, and a bare preset read carries no request id, so
/// a body that arrives after the position update can still be the previous preset's (a late
/// reply to an earlier read, or a large publication that completes late). A newer host revision
/// orders the host's reads, not what the unit serialized, and a destination name would not
/// settle it either, because names repeat. The result is therefore `accepted_unverified`, the
/// one unverified outcome the contract allows, never `authoritative_readback`, and
/// `observation` reports the observed address with the body unbound. The text and the
/// observation depend only on the address (and, for a reload, the clean flag), never on the
/// body, because the host cannot tell a stale body from the right one.
///
/// The host attaches its snapshot as the mirror it holds, so a caller that navigates by
/// `snapshot` keeps working; nobody may present those contents as the loaded preset's. For a
/// reload, `observation.dirty` reports the clean flag as observed and equally unbound: it can
/// predate the resend.
pub fn preset_address_observed_result(
    verification: &GatewayVerification,
    after: &GatewaySnapshot,
) -> Option<Value> {
    let GatewayVerification::Preset {
        require_clean,
        after_position_revision: Some(_),
        ..
    } = verification
    else {
        return None;
    };
    let address = format!(
        "{} position {}",
        after.setlist_key.trim_end_matches('/'),
        after.preset_position
    );
    let detail = if *require_clean {
        format!(
            "Active preset address resent; the host observed a position update for {address} and a clean flag, which do not prove this request caused them, and no preset body can be bound to this reload, so the discard is not verified"
        )
    } else {
        format!(
            "Preset change sent; the host observed a position update for {address}, which does not prove this request caused it, and no preset body can be bound to that address, so the loaded preset's contents are not verified"
        )
    };
    let mut observation = serde_json::json!({
        "kind": PRESET_ADDRESS_OBSERVED,
        "setlistKey": after.setlist_key,
        "position": after.preset_position,
        "address": "observed",
        "body": "unbound",
        "causallyCorrelated": false,
    });
    if *require_clean {
        observation["dirty"] = Value::Bool(after.dirty);
    }
    Some(serde_json::json!({
        "accepted": true,
        "verified": false,
        "verification": "accepted_unverified",
        "detail": detail,
        "observation": observation,
    }))
}

/// Guard a parameter write against the value the caller last observed.
///
/// Passes when `params` has no numeric `expectedValue`. Otherwise `actual` (the device's current
/// normalized value, if known) must be within 0.00001 of it. Equivalent to
/// [`assert_expected_parameter_with_options`] with no option counts.
///
/// # Errors
///
/// Returns a message when the parameter has no synchronized value or has changed.
pub fn assert_expected_parameter(actual: Option<f64>, params: &Value) -> Result<(), String> {
    assert_expected_parameter_with_options(actual, params, None)
}

/// Guard a parameter write against the value the caller last observed, with the
/// option-list rescaling rule.
///
/// An option list is stored as `index / (count - 1)` — `valueScale: "options"`,
/// `minimum: 0.0`, `maximum: 1.0`, `steps: N`, verified in
/// `docs/reference/qc-live-parameter-scales.json`. So adding or removing a block
/// changes the count on a block-enumerating list sitting on a row nobody wrote
/// to, and the *same selected option* reads back as a different float. A flat
/// `1e-5` comparison calls that "the parameter changed" where nothing changed.
/// This reconciliation belongs here rather than in a flat float comparison,
/// since only this layer knows both option counts involved.
///
/// `option_counts` is `(count when the caller recorded expectedValue, count in
/// force now)`. A host that only knows the current count passes it as both,
/// which still replaces the float comparison with an index comparison and so
/// absorbs the float32 storage error the flat tolerance rejects. `None` keeps
/// the flat comparison, which is correct for every continuous parameter.
///
/// Still open: for a plain float the tolerance here is `1e-5` while Python's
/// `params_equal` allows `1e-4` because the device stores these as float32.
/// That half of the gap is untouched, since widening it changes the guard for
/// every continuous parameter on hardware.
pub fn assert_expected_parameter_with_options(
    actual: Option<f64>,
    params: &Value,
    option_counts: Option<(usize, usize)>,
) -> Result<(), String> {
    let Some(expected) = params.get("expectedValue").and_then(Value::as_f64) else {
        return Ok(());
    };
    let actual =
        actual.ok_or_else(|| "That parameter has no synchronized numeric value".to_string())?;
    if let Some((before, after)) = option_counts {
        if before >= 2 && after >= 2 {
            let index = |value: f64, count: usize| (value * (count - 1) as f64).round() as i64;
            if index(expected, before) != index(actual, after) {
                return Err("The parameter changed on the Quad Cortex. Refresh and retry.".into());
            }
            return Ok(());
        }
    }
    if (actual - expected).abs() > 0.000_01 {
        return Err("The parameter changed on the Quad Cortex. Refresh and retry.".into());
    }
    Ok(())
}

/// The retained native library a read selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryReadTarget {
    /// Which library is read.
    pub kind: crate::device::native_library::LibraryKind,
    /// Opaque library folder key (a visible wire key of at most 512 characters).
    pub key: String,
}

/// Select retained native knowledge without constructing an outbound request.
pub fn plan_retained_library_read(
    method: &str,
    params: &Value,
) -> Result<LibraryReadTarget, String> {
    use crate::device::native_library::LibraryKind;
    let kind = match method {
        "device.captures" => LibraryKind::Capture,
        "device.irs" => LibraryKind::Ir,
        _ => return Err("Unsupported retained library read".into()),
    };
    let key = if kind == LibraryKind::Capture {
        kind.root_key()
    } else {
        match params.get("folder") {
            None | Some(Value::Null) => kind.root_key(),
            Some(Value::String(value)) if value.is_empty() => kind.root_key(),
            Some(Value::String(value)) => value,
            _ => return Err("folder must be a string or null".into()),
        }
    };
    // A read selects an opaque identity, not a filesystem path. This boundary
    // retains visible wire keys instead of rewriting slash-separated strings.
    let key = validate_visible_text(key.to_owned(), "folder", 512)?;
    Ok(LibraryReadTarget { kind, key })
}

/// Plan a correlated gateway read of `method`.
///
/// `request_id` is the correlation identifier embedded in reads whose replies echo one. The plan
/// carries the operation to send, the reply's message type, a timeout and the projection used to
/// decode the reply. `params` is consulted only by `device.presetScreenshot`.
///
/// # Errors
///
/// Returns a message for an unsupported method or invalid parameters.
pub fn plan_gateway_read(
    method: &str,
    params: &Value,
    request_id: u64,
) -> Result<GatewayReadPlan, String> {
    match method {
        "device.identity" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadVersion,
            response_type: profile::MESSAGE_TYPE_VERSION,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::DeviceIdentity,
        }),
        "device.diagnostics" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadDiagnostics { request_id },
            response_type: profile::MESSAGE_TYPE_DIAGNOSTICS,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::DeviceDiagnostics { request_id },
        }),
        "device.tunerSettings" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadTuner,
            response_type: profile::MESSAGE_TYPE_TUNER,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::TunerSettings,
        }),
        "device.generalSettings" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadGeneralSettings,
            response_type: profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::GeneralSettings,
        }),
        "device.ioSettings" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadIoSettings,
            response_type: profile::MESSAGE_TYPE_IO_SETTINGS,
            timeout_ms: 10_000,
            projection: GatewayResponseProjection::IoSettings,
        }),
        "device.globalEq" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadGlobalEq,
            response_type: profile::MESSAGE_TYPE_GLOBAL_EQ,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::GlobalEq,
        }),
        "device.modeCycle" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadModeCycle,
            response_type: profile::MESSAGE_TYPE_MODE,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::ModeCycle,
        }),
        "device.globalTempoSettings" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadGlobalTempo,
            response_type: profile::MESSAGE_TYPE_GLOBAL_TEMPO,
            timeout_ms: 30_000,
            projection: GatewayResponseProjection::GlobalTempoSettings,
        }),
        "device.presetTempoSettings" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadCurrentPreset,
            response_type: profile::MESSAGE_TYPE_RECALL_PRESET,
            timeout_ms: 15_000,
            projection: GatewayResponseProjection::PresetTempoSettings,
        }),
        "device.currentPreset" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadCurrentPreset,
            response_type: profile::MESSAGE_TYPE_RECALL_PRESET,
            timeout_ms: 15_000,
            projection: GatewayResponseProjection::CurrentPreset,
        }),
        "device.looperStatus" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadLooperStatus,
            response_type: profile::MESSAGE_TYPE_LOOPER,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::LooperStatus,
        }),
        "device.recents" | "device.favorites" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadRecentsFavorites {
                favorites: method == "device.favorites",
                request_id,
            },
            response_type: profile::MESSAGE_TYPE_RECENTS_FAVORITES,
            timeout_ms: if method == "device.favorites" {
                20_000
            } else {
                10_000
            },
            projection: if method == "device.favorites" {
                GatewayResponseProjection::Favorites { request_id }
            } else {
                GatewayResponseProjection::RecentsFavorites { request_id }
            },
        }),
        "device.pinnedModels" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadPinnedModels,
            response_type: profile::MESSAGE_TYPE_PINNED_MODELS,
            timeout_ms: 8_000,
            projection: GatewayResponseProjection::PinnedModels,
        }),
        "device.inhibitedModules" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadInhibitedModules,
            response_type: profile::MESSAGE_TYPE_COMPILER_INHIBITED_MODULES,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::InhibitedModules,
        }),
        "device.presetScreenshot" => {
            let folder_name = validate_path_component(
                required_text(params, "folderName")?,
                "folderName",
                64,
                &[],
            )?;
            let position = bounded_u32(params, "position", 255)?;
            let is_factory = params
                .get("isFactory")
                .and_then(Value::as_bool)
                .ok_or_else(|| "isFactory must be a boolean".to_string())?;
            Ok(GatewayReadPlan {
                operation: DeviceOperation::PresetScreenshot {
                    folder_name: folder_name.clone(),
                    position,
                    is_factory,
                    request_id,
                },
                response_type: profile::MESSAGE_TYPE_SCREENSHOT,
                timeout_ms: 10_000,
                projection: GatewayResponseProjection::PresetScreenshot {
                    request_id,
                    folder_name,
                    position,
                    is_factory,
                },
            })
        }
        "device.captureScreen" => Ok(GatewayReadPlan {
            operation: DeviceOperation::CaptureScreen,
            response_type: profile::MESSAGE_TYPE_REMOTE_CONTROL,
            timeout_ms: 10_000,
            projection: GatewayResponseProjection::CapturedScreen,
        }),
        "device.graphicsTree" => Ok(GatewayReadPlan {
            operation: DeviceOperation::ReadGraphicsTree,
            response_type: profile::MESSAGE_TYPE_REMOTE_CONTROL,
            timeout_ms: 5_000,
            projection: GatewayResponseProjection::GraphicsTree,
        }),
        _ => Err(format!(
            "Gateway correlated read is not supported: {method}"
        )),
    }
}

fn screen_coordinate(params: &Value, field: &str, upper: f64) -> Result<f32, String> {
    params
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && value.fract() == 0.0 && (0.0..upper).contains(value))
        .map(|value| value as f32)
        .ok_or_else(|| format!("{field} must be an integer satisfying 0 <= {field} < {upper}"))
}

/// Read an optional integer `field` that must lie in `minimum..=maximum`, or
/// return `default` when it is absent. A present value that is null, not a
/// whole number or out of range is refused, never clamped.
fn optional_bounded_u32(
    params: &Value,
    field: &str,
    default: u32,
    minimum: u32,
    maximum: u32,
) -> Result<u32, String> {
    let Some(value) = params.get(field) else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| format!("{field} must be an integer from {minimum} through {maximum}"))
}

fn preset_name(params: &Value) -> Result<String, String> {
    let name = required_text(params, "name")?.trim().to_string();
    validate_path_component(name, "Preset name", 80, &[])
}

fn save_stage(
    setlist_key: &str,
    position: u32,
    name: &str,
    instrument: i32,
) -> PresetMutationStage {
    let operation = DeviceOperation::SavePreset {
        setlist_key: setlist_key.into(),
        position,
        name: name.into(),
        instrument,
    };
    PresetMutationStage {
        verification: verification_for_operation(&operation, &Value::Null, None),
        write: PlannedWrite::HidOperation(operation),
        timeout_ms: 15_000,
        // The active-preset push precedes completion of the CorOS storage
        // transaction. Keep subsequent persistent commands out of that window.
        settle_ms: 3_000,
    }
}

/// Plan persistent preset workflows once for every native host. The host owns
/// USB scheduling and waits, while overwrite policy, stale-state guards,
/// factory-library rules and exact persistent command selection live here.
pub fn plan_preset_mutation(
    method: &str,
    params: &Value,
    snapshot: Option<&GatewaySnapshot>,
    library: &PresetLibrary,
) -> Result<PresetMutationPlan, String> {
    let before = snapshot;
    if !matches!(
        method,
        "device.deleteSetlist"
            | "device.createSetlist"
            | "device.duplicateSetlist"
            | "device.deletePreset"
            | "device.movePreset"
            // Copy sends Cortex Control's File COPY frame and never touches the
            // active preset, so it carries no expected-active-preset guard.
            | "device.copyPreset"
    ) {
        let before =
            before.ok_or_else(|| "No Quad Cortex preset has been synchronized yet".to_string())?;
        assert_expected_preset(Some(before), params)?;
        if before.preset_position != expected_position(params)? {
            return Err(
                "The active preset slot changed on the Quad Cortex. Refresh and retry.".into(),
            );
        }
    }

    match method {
        "device.deleteSetlist" => {
            let name = validate_setlist_name(required_text(params, "name")?)?;
            let setlist_key = format!("/media/p4/Presets/{name}");
            let before = before
                .filter(|before| before.has_preset && !before.setlist_key.is_empty())
                .ok_or("The active setlist is not synchronized; no deletion was sent")?;
            if before
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(&setlist_key)
            {
                return Err(
                    "The active setlist cannot be deleted. Recall another setlist first.".into(),
                );
            }
            let folders = library.folders();
            let named: Vec<_> = folders
                .iter()
                .filter(|folder| !folder.is_factory && folder.name == name)
                .collect();
            if named.len() != 1 || named[0].key != setlist_key {
                return Err("The fresh catalog did not resolve the requested setlist name to one exact device key; no deletion was sent".into());
            }
            if !folders.iter().any(|folder| {
                folder.key.trim_end_matches('/') == before.setlist_key.trim_end_matches('/')
            }) {
                return Err("The fresh deletion catalog did not include the known active setlist; no deletion was sent".into());
            }
            Ok(PresetMutationPlan {
                stages: vec![PresetMutationStage {
                    write: PlannedWrite::HidOperation(DeviceOperation::DeleteSetlist {
                        name: name.clone(),
                    }),
                    verification: GatewayVerification::None,
                    timeout_ms: 0,
                    settle_ms: 0,
                }],
                cleanup_stages: vec![],
                detail: "Deleted setlist and verified the fresh catalog".into(),
                saved_name: String::new(),
                setlist_key: setlist_key.clone(),
                position: 0,
                instrument: 0,
                catalog_policy: CatalogVerificationPolicy::persistent_file_mutation(),
                saved_presets: vec![],
                folder_copy: None,
                setlist_deletion: Some(SetlistDeletionVerification {
                    setlist_key: setlist_key.clone(),
                }),
                deletion: None,
            })
        }
        "device.createSetlist" => {
            let requested_name = validate_setlist_name(required_text(params, "name")?)?;
            let folders = library.folders();
            let custom_folders = folders
                .iter()
                .filter(|folder| !folder.is_factory)
                .collect::<Vec<_>>();
            if custom_folders.len() >= 12 {
                return Err("The Quad Cortex already contains the maximum of 12 setlists.".into());
            }

            // AddSetlistTask starts at suffix 2 and concatenates the decimal
            // ordinal directly to the requested UTF-8 name until its current
            // customPresetFolderList no longer contains the candidate.
            let mut name = requested_name.clone();
            let mut suffix = 2_u32;
            while custom_folders.iter().any(|folder| folder.name == name) {
                name = format!("{requested_name}{suffix}");
                suffix = suffix.checked_add(1).ok_or_else(|| {
                    "The setlist-name suffix exceeded the supported range".to_string()
                })?;
            }
            let setlist_key = format!("/media/p4/Presets/{name}");
            Ok(PresetMutationPlan {
                stages: vec![PresetMutationStage {
                    write: PlannedWrite::HidOperation(DeviceOperation::CreateSetlist {
                        name: name.clone(),
                    }),
                    verification: GatewayVerification::None,
                    timeout_ms: 15_000,
                    settle_ms: 0,
                }],
                cleanup_stages: Vec::new(),
                detail: format!("Created setlist {name}"),
                saved_name: name,
                setlist_key,
                position: 0,
                instrument: 0,
                catalog_policy: CatalogVerificationPolicy::default(),
                saved_presets: Vec::new(),
                folder_copy: Some(FolderCopyVerification {
                    operation: "create".into(),
                    source_key: String::new(),
                    source_name: requested_name,
                    before_folder_keys: folders
                        .iter()
                        .map(|folder| folder.key.trim_end_matches('/').to_string())
                        .collect(),
                    expected_presets: Vec::new(),
                    catalog_policy: CatalogVerificationPolicy::default(),
                }),
                setlist_deletion: None,
                deletion: None,
            })
        }
        "device.savePresetAs" => {
            let before = before.expect("preset snapshot checked above");
            let setlist_key =
                validate_setlist_key(required_text(params, "setlistKey")?, "setlistKey")?;
            let position = bounded_u32(params, "position", 255)?;
            let name = preset_name(params)?;
            let confirm_overwrite = params
                .get("confirmOverwrite")
                .and_then(Value::as_bool)
                .ok_or_else(|| "confirmOverwrite must be a boolean".to_string())?;
            if before.setlist_key.trim_end_matches('/') != setlist_key.trim_end_matches('/') {
                return Err("The active preset or setlist changed. Refresh and retry.".into());
            }
            if setlist_key.starts_with("/opt/") {
                return Err(
                    "Factory Library is read-only. Recall a user setlist before saving.".into(),
                );
            }
            let saving_current_unnamed = position == before.preset_position
                && (before.preset_name.is_empty() || before.preset_name == "Unsaved");
            if let Some(entry) = library
                .entry(&setlist_key, position)
                .filter(|_| !confirm_overwrite && !saving_current_unnamed)
            {
                return Err(format!(
                    "Slot {} contains {:?}; explicit overwrite confirmation is required.",
                    entry.location, entry.name
                ));
            }
            let instrument = library
                .entry(&setlist_key, before.preset_position)
                .map(|entry| entry.instrument)
                .unwrap_or(0);
            let saved_presets = vec![PresetMutationRecord {
                setlist_key: setlist_key.clone(),
                position,
                name: name.clone(),
                instrument,
                allow_collision_suffix: true,
            }];
            Ok(PresetMutationPlan {
                stages: vec![save_stage(&setlist_key, position, &name, instrument)],
                cleanup_stages: Vec::new(),
                detail: format!("Saved and verified {name}"),
                saved_name: name,
                setlist_key,
                position,
                instrument,
                catalog_policy: CatalogVerificationPolicy::persistent_file_mutation(),
                saved_presets,
                folder_copy: None,
                setlist_deletion: None,
                deletion: None,
            })
        }
        "device.renameCurrentPreset" => {
            let before = before.expect("preset snapshot checked above");
            require_confirmation(
                params,
                "confirmRename",
                "Renaming a stored preset requires explicit confirmation.",
            )?;
            if before.setlist_key.starts_with("/opt/") {
                return Err(
                    "Factory Library is read-only. Recall a user setlist before renaming.".into(),
                );
            }
            if before.dirty {
                return Err(
                    "The active preset has unsaved changes. Save or discard them before renaming."
                        .into(),
                );
            }
            let name = preset_name(params)?;
            if name == before.preset_name {
                return Err("The new preset name is identical to the current name.".into());
            }
            let source = library
                .entry(&before.setlist_key, before.preset_position)
                .ok_or_else(|| {
                    "The active stored preset no longer exists in the device catalog. Refresh and retry."
                        .to_string()
                })?;
            let instrument = source.instrument;
            // Cortex Control renames with one File UPDATE (EditPresetTask)
            // addressed by the authoritative opaque product key; it does not
            // re-save the Grid.
            let product_key = library
                .entry_product_key(&before.setlist_key, before.preset_position)
                .ok_or_else(|| {
                    "The active stored preset has no authoritative device file key. Refresh and retry."
                        .to_string()
                })?;
            // Host policy (not a Cortex Control frame), ESTABLISHED on CorOS 4.1.0
            // (direct USB, 2026-10-07): the unit does NOT de-duplicate a rename.
            // Renaming onto a name another slot of the folder already holds leaves
            // both slots listing the same name AND the same key, which makes any
            // later key-addressed write ambiguous. The host therefore refuses such
            // a target before sending the frame. (COPY, by contrast, is
            // de-duplicated by the unit with a `_1` suffix.)
            if library
                .preset_files(&before.setlist_key)
                .is_some_and(|files| {
                    files.iter().any(|file| {
                        file.product_key.is_some()
                            && !file.name.is_empty()
                            && file.name == name
                            && file.position != before.preset_position
                    })
                })
            {
                return Err(format!(
                    "A preset named {name:?} already exists in this setlist. The Quad Cortex does not de-duplicate a rename: both slots would list the same name and the same key, so no rename was sent. Choose a different name."
                ));
            }
            // The catalog must show exactly the name the UPDATE carried; a rename
            // is not suffixed by the unit, so no collision-suffix tolerance.
            let saved_presets = vec![PresetMutationRecord {
                setlist_key: before.setlist_key.clone(),
                position: before.preset_position,
                name: name.clone(),
                instrument,
                allow_collision_suffix: false,
            }];
            Ok(PresetMutationPlan {
                // One File UPDATE frame, exactly as Cortex Control sends it.
                // The catalog check after it is host policy (verification of
                // the result), not a further frame of the rename itself.
                stages: vec![PresetMutationStage {
                    write: PlannedWrite::HidOperation(DeviceOperation::RenamePreset {
                        setlist_key: before.setlist_key.clone(),
                        product_key,
                        name: name.clone(),
                        instrument,
                    }),
                    verification: GatewayVerification::None,
                    timeout_ms: 15_000,
                    settle_ms: 0,
                }],
                cleanup_stages: Vec::new(),
                detail: format!("Renamed and verified {name}"),
                saved_name: name,
                setlist_key: before.setlist_key.clone(),
                position: before.preset_position,
                instrument,
                catalog_policy: CatalogVerificationPolicy::persistent_file_mutation(),
                saved_presets,
                folder_copy: None,
                setlist_deletion: None,
                deletion: None,
            })
        }
        "device.copyPreset" => {
            require_confirmation(
                params,
                "confirmOverwrite",
                "Pasting a preset requires explicit overwrite confirmation.",
            )?;
            let source_key = validate_setlist_key(
                required_text(params, "sourceSetlistKey")?,
                "sourceSetlistKey",
            )?;
            let destination_key = writable_setlist_key(params, "destinationSetlistKey")?;
            let source_position = bounded_u32(params, "sourcePosition", 255)?;
            let destination_position = bounded_u32(params, "destinationPosition", 255)?;
            if source_key.trim_end_matches('/') == destination_key.trim_end_matches('/')
                && source_position == destination_position
            {
                return Err("The source and destination preset slots are identical.".into());
            }
            if destination_key.starts_with("/opt/") {
                return Err(
                    "Factory Library is read-only. Paste into a user setlist instead.".into(),
                );
            }
            let source = library
                .entry(&source_key, source_position)
                .ok_or_else(|| "The copied source preset no longer exists.".to_string())?;
            let expected_source_name = params
                .get("sourceName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !expected_source_name.is_empty() && source.name != expected_source_name {
                return Err(format!(
                    "The copied source changed from {expected_source_name:?} to {:?}. Copy it again before pasting.",
                    source.name
                ));
            }
            if library
                .entry(&destination_key, destination_position)
                .is_some_and(|destination| {
                    !source.name.is_empty() && destination.name == source.name
                })
            {
                return Err(
                    "The destination already contains a preset with the copied exact name. The uncorrelated File catalog cannot prove that overwrite changed, so no copy was sent."
                        .into(),
                );
            }
            // Host verification policy (not a Cortex Control frame): the catalog
            // must show the source's name at the destination, or the unit's
            // collision-suffix form of it. ESTABLISHED on CorOS 4.1.0 (direct USB,
            // 2026-10-07): the unit de-duplicates a COPY, so copying 'QC MIDI
            // SCRATCH' created 'QC MIDI SCRATCH_1'.
            let saved_presets = vec![PresetMutationRecord {
                setlist_key: destination_key.clone(),
                position: destination_position,
                name: source.name.clone(),
                instrument: source.instrument,
                allow_collision_suffix: true,
            }];
            // Cortex Control's COPY names the source by its opaque product key
            // (from the File catalog), never by a path rebuilt from its name.
            let source_product_key = library
                .entry_product_key(&source_key, source_position)
                .ok_or_else(|| {
                    "The copied source has no authoritative device file key. Refresh and retry."
                        .to_string()
                })?;
            let stages = vec![PresetMutationStage {
                // One File COPY frame, exactly as Cortex Control sends it:
                // no source recall, no Save, no restore of the active preset.
                // The catalog check after it is host policy (verification of
                // the result), not a further frame of the copy itself.
                write: PlannedWrite::HidOperation(DeviceOperation::CopyPreset {
                    source_setlist_key: source_key.clone(),
                    source_product_key,
                    source_is_factory: source_key.starts_with("/opt/"),
                    destination_setlist_key: destination_key.clone(),
                    destination_position,
                }),
                verification: GatewayVerification::None,
                timeout_ms: 15_000,
                settle_ms: 0,
            }];
            let cleanup_stages = Vec::new();
            Ok(PresetMutationPlan {
                stages,
                cleanup_stages,
                detail: format!("Copied {} · {} and verified", source.location, source.name),
                saved_name: source.name,
                setlist_key: destination_key,
                position: destination_position,
                instrument: source.instrument,
                catalog_policy: CatalogVerificationPolicy::persistent_file_mutation(),
                saved_presets,
                folder_copy: None,
                setlist_deletion: None,
                deletion: None,
            })
        }
        "device.duplicateSetlist" => {
            let source_key = validate_setlist_key(
                required_text(params, "sourceSetlistKey")?,
                "sourceSetlistKey",
            )?
            .trim_end_matches('/')
            .to_string();
            let folders = library.folders();
            if folders.iter().filter(|folder| !folder.is_factory).count() >= 12 {
                return Err("The Quad Cortex already contains the maximum of 12 setlists.".into());
            }
            let source = folders
                .iter()
                .find(|folder| folder.key.trim_end_matches('/') == source_key)
                .ok_or_else(|| {
                    "The source setlist has not been loaded from the Quad Cortex.".to_string()
                })?;
            let expected_source_name = required_text(params, "expectedSourceName")?;
            if source.name != expected_source_name {
                return Err(format!(
                    "The source setlist changed from {expected_source_name:?} to {:?}. Refresh it before duplicating.",
                    source.name
                ));
            }
            let before_folder_keys = folders
                .iter()
                .map(|folder| folder.key.trim_end_matches('/').to_string())
                .collect();
            let expected_presets: Vec<PresetMutationRecord> = library
                .preset_files(&source_key)
                .ok_or_else(|| {
                    "The source setlist has no authoritative preset inventory. Refresh it before duplicating."
                        .to_string()
                })?
                .iter()
                // CorOS emits a 256-entry folder on retail firmware and uses
                // keyless ProductData placeholders for empty slots. Only a
                // record with an authoritative key is an occupied preset.
                .filter(|preset| preset.product_key.is_some())
                .map(|preset| PresetMutationRecord {
                    setlist_key: source_key.clone(),
                    position: preset.position,
                    name: preset.name.clone(),
                    instrument: preset.instrument,
                    allow_collision_suffix: false,
                })
                .collect();
            let stages = vec![PresetMutationStage {
                write: PlannedWrite::HidOperation(DeviceOperation::DuplicateSetlist {
                    source_setlist_key: source_key.clone(),
                }),
                verification: GatewayVerification::None,
                timeout_ms: 15_000,
                settle_ms: 0,
            }];
            let catalog_policy =
                CatalogVerificationPolicy::duplicate_setlist(expected_presets.len());
            Ok(PresetMutationPlan {
                stages,
                cleanup_stages: Vec::new(),
                detail: format!("Duplicated setlist {}", source.name),
                saved_name: source.name.clone(),
                setlist_key: source_key.clone(),
                position: 0,
                instrument: 0,
                catalog_policy: CatalogVerificationPolicy::default(),
                saved_presets: Vec::new(),
                folder_copy: Some(FolderCopyVerification {
                    operation: "copy".into(),
                    source_key,
                    source_name: source.name.clone(),
                    before_folder_keys,
                    expected_presets,
                    catalog_policy,
                }),
                setlist_deletion: None,
                deletion: None,
            })
        }
        "device.deletePreset" => {
            let setlist_key = writable_setlist_key(params, "setlistKey")?;
            let position = bounded_u32(params, "position", 255)?;
            let expected_name = validate_path_component(
                required_text(params, "expectedName")?,
                "expectedName",
                80,
                &[],
            )?;
            let entry = library.entry(&setlist_key, position).ok_or_else(|| {
                "The selected preset no longer exists in the device catalog. Refresh and retry."
                    .to_string()
            })?;
            if entry.name != expected_name {
                return Err(format!(
                    "The selected preset changed from {expected_name:?} to {:?}. Refresh and retry.",
                    entry.name
                ));
            }
            let product_key = library
                .entry_product_key(&setlist_key, position)
                .ok_or_else(|| {
                    "The selected preset has no authoritative device file key. Refresh and retry."
                        .to_string()
                })?;
            Ok(PresetMutationPlan {
                stages: vec![PresetMutationStage {
                    write: PlannedWrite::HidOperation(DeviceOperation::DeletePreset {
                        setlist_key: setlist_key.clone(),
                        product_key: product_key.clone(),
                    }),
                    verification: GatewayVerification::None,
                    timeout_ms: 15_000,
                    settle_ms: 0,
                }],
                cleanup_stages: Vec::new(),
                detail: format!("Deleted and verified {expected_name}"),
                saved_name: expected_name.clone(),
                setlist_key: setlist_key.clone(),
                position,
                instrument: entry.instrument,
                catalog_policy: CatalogVerificationPolicy::persistent_file_mutation(),
                saved_presets: Vec::new(),
                folder_copy: None,
                setlist_deletion: None,
                deletion: Some(PresetDeletionVerification {
                    setlist_key,
                    product_key,
                    expected_name,
                    expected_position: position,
                }),
            })
        }
        "device.movePreset" => {
            let setlist_key = writable_setlist_key(params, "setlistKey")?
                .trim_end_matches('/')
                .to_string();
            let source_position = bounded_u32(params, "sourcePosition", 255)?;
            let destination_position = bounded_u32(params, "destinationPosition", 255)?;
            if source_position == destination_position {
                return Err("The source and destination preset slots are identical.".into());
            }
            let source = library
                .entry(&setlist_key, source_position)
                .ok_or_else(|| {
                    "The selected source preset no longer exists. Refresh and retry.".to_string()
                })?;
            let expected_source_name = required_text(params, "sourceName")?;
            if source.name != expected_source_name {
                return Err(format!(
                    "The selected source changed from {expected_source_name:?} to {:?}. Refresh and retry.",
                    source.name
                ));
            }
            let source_product_key = library
                .entry_product_key(&setlist_key, source_position)
                .ok_or_else(|| {
                    "The selected source has no authoritative device file key. Refresh and retry."
                        .to_string()
                })?;
            // CorOS includes keyless `Unsaved` ProductData placeholders for
            // empty slots. Presence of a projected row is therefore not
            // evidence of an occupied swap target; only an authoritative
            // device product key makes the reverse side of the swap real.
            let destination = library
                .entry(&setlist_key, destination_position)
                .filter(|entry| entry.product_key.is_some());
            let mut saved_presets = vec![PresetMutationRecord {
                setlist_key: setlist_key.clone(),
                position: destination_position,
                name: source.name.clone(),
                instrument: source.instrument,
                allow_collision_suffix: false,
            }];
            if let Some(ref destination) = destination {
                saved_presets.push(PresetMutationRecord {
                    setlist_key: setlist_key.clone(),
                    position: source_position,
                    name: destination.name.clone(),
                    instrument: destination.instrument,
                    allow_collision_suffix: false,
                });
            }
            let operation = if destination.is_some() {
                DeviceOperation::SwapPreset {
                    setlist_key: setlist_key.clone(),
                    product_key: source_product_key,
                    position: destination_position,
                }
            } else {
                DeviceOperation::MovePreset {
                    setlist_key: setlist_key.clone(),
                    product_key: source_product_key,
                    position: destination_position,
                }
            };
            Ok(PresetMutationPlan {
                stages: vec![PresetMutationStage {
                    write: PlannedWrite::HidOperation(operation),
                    verification: GatewayVerification::None,
                    timeout_ms: 15_000,
                    settle_ms: 0,
                }],
                cleanup_stages: Vec::new(),
                detail: format!(
                    "Moved {} from slot {} to slot {} and verified the swap",
                    source.name, source_position, destination_position
                ),
                saved_name: source.name,
                setlist_key,
                position: destination_position,
                instrument: source.instrument,
                catalog_policy: CatalogVerificationPolicy::persistent_file_mutation(),
                saved_presets,
                folder_copy: None,
                setlist_deletion: None,
                deletion: None,
            })
        }
        _ => Err(format!(
            "Persistent preset method is not supported: {method}"
        )),
    }
}

/// Read `field` from `params` as an unsigned integer no greater than `maximum`.
///
/// # Errors
///
/// Returns a message when the field is missing, not an unsigned integer, or above `maximum`.
pub fn bounded_u32(params: &Value, field: &str, maximum: u32) -> Result<u32, String> {
    params
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value <= maximum)
        .ok_or_else(|| format!("{field} must be an integer from 0 through {maximum}"))
}

/// Read `field` from `params` as a non-empty string.
///
/// # Errors
///
/// Returns a message when the field is missing, not a string, or empty.
pub fn required_text(params: &Value, field: &str) -> Result<String, String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(String::from)
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

fn parse_footswitch_assignment_type(
    value: &str,
    field: &str,
) -> Result<StompAssignmentType, String> {
    match value {
        "primary" => Ok(StompAssignmentType::Primary),
        "secondary" => Ok(StompAssignmentType::Secondary),
        _ => Err(format!("{field} must be primary or secondary")),
    }
}

fn footswitch_assignment_type_name(value: &StompAssignmentType) -> String {
    match value {
        StompAssignmentType::Primary => "primary".to_string(),
        StompAssignmentType::Secondary => "secondary".to_string(),
    }
}

fn validate_visible_text(value: String, field: &str, maximum: usize) -> Result<String, String> {
    if value.is_empty() || value.chars().count() > maximum || value.chars().any(char::is_control) {
        return Err(format!(
            "{field} must contain 1-{maximum} visible characters"
        ));
    }
    Ok(value)
}

fn validate_stomp_label(value: String) -> Result<String, String> {
    let characters = value.chars().count();
    if !(1..=16).contains(&characters) || value.chars().any(char::is_control) {
        return Err("label must contain 1-16 visible characters".into());
    }
    Ok(value)
}

fn validate_path_component(
    value: String,
    field: &str,
    maximum: usize,
    reserved: &[&str],
) -> Result<String, String> {
    let value = validate_visible_text(value, field, maximum)?;
    if value.trim() != value
        || value.contains(['/', '\\'])
        || matches!(value.as_str(), "." | "..")
        || reserved
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
    {
        return Err(format!(
            "{field} must not have surrounding whitespace, path separators, traversal components, or a reserved name"
        ));
    }
    Ok(value)
}

fn validate_library_key(value: String, field: &str) -> Result<String, String> {
    let value = validate_visible_text(value, field, 512)?;
    if value.trim() != value
        || value.trim_end_matches('/').is_empty()
        || value.contains('\\')
        || value.contains("//")
        || value
            .trim_end_matches('/')
            .split('/')
            .filter(|component| !component.is_empty())
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(format!(
            "{field} must be a library key without control characters, backslashes, or traversal components"
        ));
    }
    Ok(value)
}

pub(crate) fn validate_setlist_key(value: String, field: &str) -> Result<String, String> {
    let value = validate_library_key(value, field)?;
    if !value.starts_with('/') {
        return Err(format!("{field} must be an absolute setlist key"));
    }
    Ok(value)
}

/// Read `field` from `params` as an optional 32-bit integer; absent and `null` give `None`.
///
/// # Errors
///
/// Returns a message when the field is present but not an integer that fits in `i32`.
pub fn optional_i32(params: &Value, field: &str) -> Result<Option<i32>, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("{field} must be null or an integer")),
    }
}

fn optional_positive_u32(params: &Value, field: &str) -> Result<Option<u32>, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| format!("{field} must be null or a positive integer")),
    }
}

fn writable_setlist_key(params: &Value, field: &str) -> Result<String, String> {
    let key = validate_setlist_key(required_text(params, field)?, field)?;
    let trimmed = key.trim_end_matches('/');
    let Some(name) = trimmed.strip_prefix("/media/p4/Presets/") else {
        return Err(format!("{field} must identify a writable user setlist"));
    };
    if name.is_empty() || name.contains('/') {
        return Err(format!("{field} must identify a writable user setlist"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn validate_setlist_name(name: String) -> Result<String, String> {
    validate_path_component(name, "setlist name", 64, &["My Presets"])
}

/// Read the `expectedPosition` guard: a preset position from 0 through 255.
///
/// # Errors
///
/// Returns a message when it is missing or out of range.
pub fn expected_position(params: &Value) -> Result<u32, String> {
    bounded_u32(params, "expectedPosition", 255)
}

/// Reject a write whose preset guards no longer match the device.
///
/// When `params` has `expectedPresetIdentity`, the snapshot's preset identity must equal it. When
/// it has a non-empty `expectedPresetName`, the snapshot's preset name must equal it, except that
/// the placeholder names `Current preset`, `Empty preset` and `Unsaved` are accepted while the
/// active preset is unnamed. Absent guards pass.
///
/// # Errors
///
/// Returns a message when a guard is malformed or the preset has changed.
pub fn assert_expected_preset(
    snapshot: Option<&GatewaySnapshot>,
    params: &Value,
) -> Result<(), String> {
    if let Some(expected) = params.get("expectedPresetIdentity") {
        let expected = expected
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("A preset identity from current device state is required")?;
        if snapshot.and_then(|value| value.preset_identity.as_deref()) != Some(expected) {
            return Err(
                "The preset or QC session changed. Refresh the device state before retrying".into(),
            );
        }
    }
    let expected = params
        .get("expectedPresetName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if expected.is_empty() {
        return Ok(());
    }
    let actual = snapshot
        .map(|value| value.preset_name.as_str())
        .unwrap_or_default();
    let placeholder = matches!(
        expected.to_ascii_lowercase().as_str(),
        "current preset" | "empty preset" | "unsaved"
    );
    if actual != expected && !(actual.is_empty() && placeholder) {
        return Err(format!(
            "Preset changed on the Quad Cortex: expected {expected:?}, but {:?} is active. Refresh and retry.",
            if actual.is_empty() {
                "an unnamed preset"
            } else {
                actual
            }
        ));
    }
    Ok(())
}

/// Machine-readable prefix of the refusal issued while the Quad Cortex has not yet reported its
/// dirty flag in this session. A host strips it and reports the code
/// `UNSAVED_STATE_NOT_YET_OBSERVED` as retryable: nothing was sent, and the flag arrives without
/// any action.
pub const UNSAVED_STATE_NOT_YET_OBSERVED_PREFIX: &str = "UNSAVED_STATE_NOT_YET_OBSERVED: ";

/// Refuses an action that replaces the working preset while the dirty flag is still the
/// placeholder: the unit has not said whether the preset has unsaved changes, so the `false` in
/// the snapshot is not evidence. Nothing is read or sent here.
fn assert_dirty_flag_observed(snapshot: &GatewaySnapshot) -> Result<(), String> {
    if snapshot.unobserved.contains(&"dirty") {
        return Err(format!(
            "{UNSAVED_STATE_NOT_YET_OBSERVED_PREFIX}The Quad Cortex has not yet reported whether the current preset has unsaved changes. Nothing was sent; retry once it has."
        ));
    }
    Ok(())
}

/** Plan guarded preset recall/navigation/reload while hosts own asynchronous readback. */
pub fn plan_preset_recall(
    method: &str,
    params: &Value,
    snapshot: Option<&GatewaySnapshot>,
) -> Result<PresetRecallPlan, String> {
    let before =
        snapshot.ok_or_else(|| "No Quad Cortex preset has been synchronized yet".to_string())?;
    assert_expected_preset(Some(before), params)?;
    if before.preset_position != expected_position(params)? {
        return Err("The active preset slot changed on the Quad Cortex. Refresh and retry.".into());
    }

    let (setlist_key, position, require_clean, detail) = match method {
        "device.recallPreset" => {
            let setlist_key =
                validate_setlist_key(required_text(params, "setlistKey")?, "setlistKey")?;
            if let Some(expected_setlist_key) = params
                .get("expectedSetlistKey")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                let expected_setlist_key =
                    validate_setlist_key(expected_setlist_key.to_string(), "expectedSetlistKey")?;
                if before.setlist_key.trim_end_matches('/')
                    != expected_setlist_key.trim_end_matches('/')
                {
                    return Err(
                        "The active setlist changed on the Quad Cortex. Refresh and retry.".into(),
                    );
                }
            }
            if before.dirty {
                return Err("The current preset has unsaved changes. Save or revert them before recalling another preset.".into());
            }
            assert_dirty_flag_observed(before)?;
            (
                setlist_key,
                bounded_u32(params, "position", 255)?,
                false,
                "Preset recall requested",
            )
        }
        "device.navigateBank" => {
            if before.dirty {
                return Err("The current preset has unsaved changes. Save or revert them before recalling another preset.".into());
            }
            assert_dirty_flag_observed(before)?;
            let direction = params
                .get("direction")
                .and_then(Value::as_i64)
                .filter(|value| matches!(value, -1 | 1))
                .ok_or_else(|| "direction must be -1 or 1".to_string())?;
            let target = before.preset_position as i64 + direction * 8;
            if !(0..256).contains(&target) {
                return Err("Already at the first or last preset bank.".into());
            }
            (
                before.setlist_key.clone(),
                target as u32,
                false,
                "Preset bank change requested",
            )
        }
        "device.reloadPreset" => (
            before.setlist_key.clone(),
            before.preset_position,
            true,
            "Active preset address resent",
        ),
        _ => return Err(format!("Preset recall method is not supported: {method}")),
    };

    Ok(PresetRecallPlan {
        command: DeviceCommand::SetlistPosition {
            is_factory: setlist_key.starts_with("/opt/"),
            setlist_key: setlist_key.clone(),
            position,
        },
        setlist_key,
        position,
        after_position_revision: before.position_revision,
        after_full_preset_revision: before.full_preset_revision,
        require_clean,
        detail: detail.into(),
    })
}

/// Rejects a write when the UI's optimistic concurrency token no longer
/// matches the latest device snapshot.  Both desktop and Android call this
/// planner, so these guards cannot drift between hosts.
pub fn assert_expected_state(
    method: &str,
    snapshot: Option<&GatewaySnapshot>,
    params: &Value,
) -> Result<(), String> {
    // Global-tempo mode is not the footswitch mode stored in GatewaySnapshot.
    // It is checked against a fresh device.globalTempoSettings preflight instead.
    let uses_global_tempo_mode = method == "device.setGlobalTempo";
    let has_expected_state = [
        "expectedScene",
        "expectedMode",
        "expectedTempo",
        "expectedValue",
        "expectedModelId",
        "expectedBypassed",
        "expectedFootswitch",
        "expectedAssignmentType",
        "expectedInputId",
        "expectedOutputId",
        "expectedSplitColumn",
        "expectedMixColumn",
        "expectedMuted",
        "expectedLabel",
        "expectedColor",
    ]
    .iter()
    .any(|field| {
        (*field != "expectedMode" || !uses_global_tempo_mode) && params.get(field).is_some()
    });
    if !has_expected_state {
        return Ok(());
    }
    let Some(snapshot) = snapshot else {
        return Err("No Quad Cortex preset has been synchronized yet".into());
    };
    if !uses_global_tempo_mode {
        if let Some(expected) = params.get("expectedMode").and_then(Value::as_str) {
            if snapshot.mode != expected {
                return Err(format!(
                    "Footswitch mode changed on the Quad Cortex: expected {expected}, but {} is active. Refresh and retry.",
                    snapshot.mode
                ));
            }
        }
    }
    if let Some(expected) = params.get("expectedScene").and_then(Value::as_u64) {
        if snapshot.active_scene != expected as u32 {
            return Err(format!(
                "Scene changed on the Quad Cortex: expected {}, but {} is active. Refresh and retry.",
                (b'A' + expected.min(7) as u8) as char,
                (b'A' + snapshot.active_scene.min(7) as u8) as char
            ));
        }
    }
    if let Some(expected) = params.get("expectedTempo").and_then(Value::as_u64) {
        if snapshot.tempo != expected as u32 {
            return Err(format!(
                "Tempo changed on the Quad Cortex: expected {expected}, but {} is active. Refresh and retry.",
                snapshot.tempo
            ));
        }
    }
    if method == "device.setSceneColor" {
        let scene = bounded_u32(params, "scene", domain::SCENE_COUNT - 1)?;
        let expected = params
            .get("expectedColor")
            .and_then(Value::as_str)
            .filter(|color| {
                color.len() == 7
                    && color.starts_with('#')
                    && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            })
            .ok_or_else(|| {
                "expectedColor must be a #rrggbb scene color from the current preset".to_string()
            })?;
        let actual = snapshot
            .scene_colors
            .as_ref()
            .and_then(|colors| colors.get(scene as usize))
            .ok_or_else(|| {
                "The selected scene color is unavailable; refresh and retry.".to_string()
            })?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err("The scene color changed on the Quad Cortex. Refresh and retry.".into());
        }
    }
    if method == "device.setSceneLabel" {
        let scene = bounded_u32(params, "scene", domain::SCENE_COUNT - 1)?;
        let expected = params
            .get("expectedLabel")
            .and_then(Value::as_str)
            .ok_or_else(|| "expectedLabel must be a string".to_string())?;
        let actual = snapshot.scenes.get(scene as usize).ok_or_else(|| {
            "The selected scene label is unavailable; refresh and retry.".to_string()
        })?;
        if actual != expected {
            return Err("The scene label changed on the Quad Cortex. Refresh and retry.".into());
        }
    }
    if method == "device.setStompLabel" {
        let footswitch = bounded_u32(params, "footswitch", domain::SCENE_COUNT - 1)?;
        let expected = params
            .get("expectedLabel")
            .and_then(Value::as_str)
            .ok_or_else(|| "expectedLabel must be a string".to_string())?;
        let actual = snapshot
            .footswitch_states
            .as_ref()
            .and_then(|states| states.iter().find(|state| state.index == footswitch))
            .and_then(|state| state.label.as_deref())
            .ok_or_else(|| {
                "The selected STOMP label is unavailable; refresh and retry.".to_string()
            })?;
        if actual != expected {
            return Err("The STOMP label changed on the Quad Cortex. Refresh and retry.".into());
        }
    }
    if method == "device.setMasterVolume" {
        if let Some(expected) = params.get("expectedValue").and_then(Value::as_u64) {
            if snapshot.master_volume != expected as u32 {
                return Err(format!(
                    "Master Volume changed on the Quad Cortex: expected {expected}, but {} is active. Refresh and retry.",
                    snapshot.master_volume
                ));
            }
        }
    }

    let Some(row) = params
        .get("row")
        .and_then(Value::as_u64)
        .map(|value| value as u32)
    else {
        return Ok(());
    };
    if let Some(expected_value) = params.get("expectedModelId") {
        let expected = if expected_value.is_null() {
            None
        } else {
            Some(
                expected_value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| {
                        "expectedModelId must be an unsigned 32-bit integer or null".to_string()
                    })?,
            )
        };
        let column = params
            .get("fromColumn")
            .or_else(|| params.get("column"))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let actual = match column {
            Some(column) if column == domain::GRID_COLUMNS => snapshot
                .routes
                .iter()
                .find(|route| {
                    route.row == row && route.split_column.is_some_and(|column| column >= 0)
                })
                .and_then(|route| route.splitter_model_id),
            Some(column) if column == domain::GRID_COLUMNS + 1 => snapshot
                .routes
                .iter()
                .find(|route| {
                    route.row == row && route.mix_column.is_some_and(|column| column >= 0)
                })
                .map(|_| 11_000),
            Some(column) => snapshot
                .blocks
                .iter()
                .find(|block| block.row == row && block.column == column)
                .and_then(|block| block.model_id),
            None => None,
        };
        if actual != expected {
            return Err(format!(
                "The block at row {row}, column {} changed: expected model {:?}, found {:?}. Refresh and retry.",
                params
                    .get("fromColumn")
                    .or_else(|| params.get("column"))
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX),
                expected,
                actual
            ));
        }
    }
    if let (Some(column), Some(expected)) = (
        params.get("column").and_then(Value::as_u64),
        params.get("expectedBypassed").and_then(Value::as_bool),
    ) {
        let actual = snapshot
            .blocks
            .iter()
            .find(|block| block.row == row && block.column == column as u32)
            .and_then(|block| block.bypassed);
        if actual != Some(expected) {
            return Err(
                "The block bypass state changed on the Quad Cortex. Refresh and retry.".into(),
            );
        }
    }
    if let Some(column) = params.get("column").and_then(Value::as_u64) {
        if let Some(expected) = params.get("expectedFootswitch") {
            let expected = if expected.is_null() {
                None
            } else {
                Some(
                    expected
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value < domain::SCENE_COUNT)
                        .ok_or_else(|| {
                            format!(
                                "expectedFootswitch must be null or an integer from 0 through {}",
                                domain::SCENE_COUNT - 1
                            )
                        })?,
                )
            };
            let actual = snapshot
                .blocks
                .iter()
                .find(|block| block.row == row && block.column == column as u32)
                .and_then(|block| block.footswitch);
            if expected != actual {
                return Err(
                    "The block footswitch assignment changed on the Quad Cortex. Refresh and retry."
                        .into(),
                );
            }
        }
        if let Some(expected) = params.get("expectedAssignmentType") {
            let expected = if expected.is_null() {
                None
            } else {
                let value = expected.as_str().ok_or_else(|| {
                    "expectedAssignmentType must be primary, secondary, or null".to_string()
                })?;
                parse_footswitch_assignment_type(value, "expectedAssignmentType")?;
                Some(value.to_string())
            };
            let actual = snapshot
                .blocks
                .iter()
                .find(|block| block.row == row && block.column == column as u32)
                .and_then(|block| block.footswitch_assignment_type.clone());
            if expected != actual {
                return Err(
                    "The block footswitch assignment type changed on the Quad Cortex. Refresh and retry."
                        .into(),
                );
            }
        }
    }
    let route_fields = [
        "expectedInputId",
        "expectedOutputId",
        "expectedSplitColumn",
        "expectedMixColumn",
        "expectedMuted",
    ];
    if route_fields.iter().any(|field| params.get(field).is_some()) {
        let route = snapshot
            .routes
            .iter()
            .find(|route| route.row == row)
            .ok_or_else(|| {
                "The signal-chain routing is not synchronized. Refresh and retry.".to_string()
            })?;
        for (field, actual) in [
            ("expectedInputId", route.input_id.map(i64::from)),
            ("expectedOutputId", route.output_id.map(i64::from)),
            ("expectedSplitColumn", route.split_column.map(i64::from)),
            ("expectedMixColumn", route.mix_column.map(i64::from)),
        ] {
            if let Some(expected) = params.get(field) {
                let expected = if expected.is_null() {
                    None
                } else {
                    Some(
                        expected
                            .as_i64()
                            .ok_or_else(|| format!("{field} must be an integer or null"))?,
                    )
                };
                if expected != actual {
                    return Err(
                        "The signal-chain routing changed on the Quad Cortex. Refresh and retry."
                            .into(),
                    );
                }
            }
        }
        if let Some(expected) = params.get("expectedMuted").and_then(Value::as_bool) {
            let actual = route.split_muted.ok_or(
                "STATE_UNAVAILABLE: The active scene's splitter/mixer mute state was not reported",
            )?;
            if actual != expected {
                return Err(
                    "The splitter/mixer mute state changed on the Quad Cortex. Refresh and retry."
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn boolean(params: &Value, field: &str) -> Result<bool, String> {
    params
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{field} must be a boolean"))
}

/// Guard an irreversible or surprising operation behind an explicit
/// `params[field] == true` acknowledgement, failing with `message` verbatim
/// otherwise. Unlike [`boolean`], a missing or non-boolean field is treated
/// the same as an explicit `false`: the caller simply has not confirmed.
fn require_confirmation(params: &Value, field: &str, message: &str) -> Result<(), String> {
    if params.get(field).and_then(Value::as_bool) != Some(true) {
        return Err(message.to_string());
    }
    Ok(())
}

fn normalized(params: &Value, field: &str) -> Result<f32, String> {
    params
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| (0.0..=1.0).contains(value))
        .map(|value| value as f32)
        .ok_or_else(|| format!("{field} must be a number from zero through one"))
}

fn optional_normalized(params: &Value, field: &str) -> Result<Option<f32>, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => normalized(params, field).map(Some),
    }
}

fn optional_boolean(params: &Value, field: &str) -> Result<Option<bool>, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => boolean(params, field).map(Some),
    }
}

/// Validate a public output level in decibels.
///
/// Unlike `optional_input_gain` this returns the decibel value unchanged:
/// `crate::commands::set_output_port` normalizes it onto the wire field, so
/// converting here as well would apply the transform twice.
fn optional_output_level(params: &Value) -> Result<Option<f32>, String> {
    match params.get("level") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite() && (-40.0..=12.0).contains(value))
            .map(|value| Some(value as f32))
            .ok_or_else(|| "level must be a finite dB value from -40 through 12".to_string()),
    }
}

fn optional_output_ground_lift(params: &Value) -> Result<Option<f32>, String> {
    match params.get("groundLift") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| *value == 0.0 || *value == 1.0)
            .map(|value| Some(value as f32))
            .ok_or_else(|| "groundLift must be the output enum value 0 or 1".to_string()),
    }
}

fn is_supported_output_settings_port(output_port_id: u32) -> bool {
    matches!(output_port_id, 1 | 2 | 4..=9)
}

/// Validate a public input gain in decibels and normalize it for the wire.
///
/// The arithmetic lives in `crate::commands` beside the frame builders, so
/// the units check there can hold it against the input-port contract's published
/// `physicalToWire` instead of it being hand-written here.
fn optional_input_gain(params: &Value, field: &str) -> Result<Option<f32>, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| (-12.0..=60.0).contains(value))
            .map(|value| Some(input_level_db_to_wire(value)))
            .ok_or_else(|| format!("{field} must be an input gain from -12 through +60 dB")),
    }
}

/// `savePreset`'s optional `instrument`: absent or null means `0`; a present
/// value must be one of [`crate::commands::PRESET_INSTRUMENTS`].
fn preset_instrument(params: &Value) -> Result<i32, String> {
    let range = crate::commands::PRESET_INSTRUMENTS;
    match params.get("instrument") {
        None | Some(Value::Null) => Ok(0),
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .filter(|value| range.contains(value))
            .ok_or_else(|| {
                format!(
                    "instrument must be an integer from {} through {}",
                    range.start(),
                    range.end()
                )
            }),
    }
}

fn midi_out_messages(params: &Value, field_name: &str) -> Result<Vec<MidiOutMessage>, String> {
    let values = params
        .get(field_name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{field_name} must be an array"))?;
    if values.len() > crate::commands::MIDI_OUT_SLOTS_PER_SOURCE {
        return Err("A MIDI Out source supports at most 12 messages".into());
    }
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let object = value
                .as_object()
                .ok_or_else(|| format!("messages[{index}] must be an object"))?;
            let field = |name: &str, minimum: u32, maximum: u32| {
                object
                    .get(name)
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| (minimum..=maximum).contains(value))
                    .ok_or_else(|| {
                        format!("messages[{index}].{name} must be {minimum} through {maximum}")
                    })
            };
            // Type 0 is an empty slot that keeps its position; it is sent as the
            // empty message, so its channel may be the 0 the unit publishes it with.
            let types = crate::commands::MIDI_OUT_TYPES;
            let channels = crate::commands::MIDI_OUT_CHANNELS;
            let param_max = crate::commands::MIDI_OUT_PARAMETER_MAX;
            let kind = field("type", crate::commands::MIDI_OUT_TYPE_NONE, *types.end())?;
            Ok(MidiOutMessage {
                r#type: kind,
                channel: field(
                    "channel",
                    if kind == crate::commands::MIDI_OUT_TYPE_NONE {
                        0
                    } else {
                        *channels.start()
                    },
                    *channels.end(),
                )?,
                param1: field("param1", 0, param_max)?,
                param2: field("param2", 0, param_max)?,
                param3: field("param3", 0, param_max)?,
            })
        })
        .collect()
}

/// The GeneralSettings boolean that reads but does not write, with the reason
/// it is read-only. Mirrors `contracts/public/qc-actions.v1.json`
/// `#/generalSettings/readOnlyBooleans`, whose own note says it best: "a caller
/// that cannot see the reason cannot tell an unsupported setting from a broken
/// device". A bare "unsupported GeneralSettings toggle" told them neither which
/// setting nor why.
use crate::generated_host_policy::READ_ONLY_GENERAL_TOGGLES;

fn general_toggle_refusal(setting: &str) -> String {
    match READ_ONLY_GENERAL_TOGGLES
        .iter()
        .find(|(name, _)| *name == setting)
    {
        Some((name, reason)) => format!(
            "GeneralSettings toggle {name:?} is readable but read-only: {reason}. It is projected on read and cannot be written."
        ),
        None => format!("{setting:?} is not a GeneralSettings toggle."),
    }
}

fn operation(operation: &str, params: &Value) -> Result<DeviceOperation, String> {
    let row = || bounded_u32(params, "row", domain::GRID_ROWS - 1);
    let column = |field: &str| bounded_u32(params, field, domain::GRID_COLUMNS - 1);
    match operation {
        "setTunerInput" => {
            require_confirmation(
                params,
                "confirmTunerActivation",
                "Changing tuner input is believed to engage the tuner invisibly; confirmTunerActivation must be true.",
            )?;
            let input_port_id = params
                .get("inputPortId")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
                .filter(|value| is_tuner_input_port_id(*value))
                .ok_or_else(|| "inputPortId must be Input 1, Input 2, Input 1/2, Return 1, Return 2, USB 5, or USB 6 (1, 2, 3, 4, 5, 8, or 9)".to_string())?;
            Ok(DeviceOperation::SetTunerInput(input_port_id))
        }
        "setTunerMute" => {
            require_confirmation(
                params,
                "confirmTunerActivation",
                "Changing tuner mute is believed to engage the tuner invisibly and may silence every output; confirmTunerActivation must be true.",
            )?;
            Ok(DeviceOperation::SetTunerMute(boolean(params, "muted")?))
        }
        "setTunerReference" => {
            require_confirmation(
                params,
                "confirmTunerActivation",
                "Changing tuner reference is believed to engage the tuner invisibly; confirmTunerActivation must be true.",
            )?;
            let offset_hz = params
                .get("referenceOffsetHz")
                .and_then(Value::as_f64)
                .filter(|value| {
                    value.is_finite()
                        && (crate::commands::MINIMUM_TUNER_REFERENCE_OFFSET_HZ as f64
                            ..=crate::commands::MAXIMUM_TUNER_REFERENCE_OFFSET_HZ as f64)
                            .contains(value)
                })
                .ok_or_else(|| {
                    "referenceOffsetHz must be between -40 and 40 Hz relative to A=440".to_string()
                })?;
            Ok(DeviceOperation::SetTunerReference(offset_hz as f32))
        }
        "setTunerMeter" => {
            require_confirmation(
                params,
                "confirmTunerActivation",
                "Changing tuner meter reporting is believed to engage the tuner invisibly; confirmTunerActivation must be true.",
            )?;
            Ok(DeviceOperation::SetTunerMeter(boolean(params, "enabled")?))
        }
        "setGeneralInteger" => {
            let setting = required_text(params, "setting")?;
            let value = params
                .get("value")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
                .filter(|value| is_general_integer_value(&setting, *value))
                .ok_or_else(|| format!("invalid value for GeneralSettings integer {setting}"))?;
            Ok(DeviceOperation::SetGeneralInteger { setting, value })
        }
        "setGeneralToggle" => {
            let setting = required_text(params, "setting")?;
            if !matches!(
                setting.as_str(),
                "lockScreenAndVolumeKnob"
                    | "midiOverUsb"
                    | "ignoreDuplicatePc"
                    | "stompModeAutoAssign"
                    | "swapTempoTunerAccess"
                    | "disableInternetConnectionCheck"
                    | "dynamicDelayCompensation"
                    | "presetDimmed"
                    | "sceneDimmed"
                    | "stompDimmed"
                    | "midiClockIn"
                    | "gigViewStompAccess"
            ) {
                return Err(general_toggle_refusal(&setting));
            }
            Ok(DeviceOperation::SetGeneralToggle {
                setting,
                enabled: boolean(params, "enabled")?,
            })
        }
        "setSceneBypassBehavior" => {
            let behavior =
                match required_text(params, "behavior")?.as_str() {
                    "alwaysOverwrite" => 0,
                    "nonstompOverwrite" => 1,
                    "neverOverwrite" => 2,
                    _ => return Err(
                        "behavior must be alwaysOverwrite, nonstompOverwrite, or neverOverwrite"
                            .into(),
                    ),
                };
            Ok(DeviceOperation::SetSceneBypassBehavior(behavior))
        }
        "setMasterVolumeAssignment" => Ok(DeviceOperation::SetMasterVolumeAssignment {
            out12: boolean(params, "out12")?,
            out34: boolean(params, "out34")?,
            send12: boolean(params, "send12")?,
            headphones: boolean(params, "headphones")?,
        }),
        "setGlobalBypass" => {
            // Each family is optional (null or absent sends no frame for it);
            // at least one is required. The desktop application sends one
            // full-family frame per interaction, so only the supplied family is
            // sent.
            let rows = |field: &str| -> Result<Option<[bool; 4]>, String> {
                let Some(raw) = params.get(field).filter(|value| !value.is_null()) else {
                    return Ok(None);
                };
                let values = raw
                    .as_array()
                    .filter(|values| values.len() == 4)
                    .ok_or_else(|| format!("{field} must contain four booleans"))?;
                let mut rows = [false; 4];
                for (index, value) in values.iter().enumerate() {
                    rows[index] = value
                        .as_bool()
                        .ok_or_else(|| format!("{field}[{index}] must be a boolean"))?;
                }
                Ok(Some(rows))
            };
            let (cab, ir) = (rows("cab")?, rows("ir")?);
            if cab.is_none() && ir.is_none() {
                return Err("set_global_bypass needs cab or ir (or both)".into());
            }
            Ok(DeviceOperation::SetGlobalBypass { cab, ir })
        }
        "setInputPort" => {
            let input_port_id = bounded_u32(params, "inputPortId", 14)?;
            if !is_input_setting_port_id(input_port_id) {
                return Err(
                    "inputPortId must be one of the physical settings ports 1, 2, 4, or 5".into(),
                );
            }
            let level = optional_input_gain(params, "levelDb")?;
            let impedance = optional_normalized(params, "impedance")?;
            let input_type = optional_normalized(params, "inputType")?;
            let ground_lift = optional_normalized(params, "groundLift")?;
            if impedance.is_some_and(|value| !is_input_impedance_value(value)) {
                return Err(
                    "impedance must be one of 0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, or 1"
                        .into(),
                );
            }
            if input_type.is_some_and(|value| !is_input_type_value(value)) {
                return Err(
                    "inputType must be 0 (instrument), 0.5 (mic), or 1 (mic + phantom)".into(),
                );
            }
            if ground_lift.is_some_and(|value| !is_input_ground_lift_value(value)) {
                return Err("groundLift must be 0 (off) or 1 (on)".into());
            }
            if level.is_none()
                && impedance.is_none()
                && input_type.is_none()
                && ground_lift.is_none()
            {
                return Err("setInputPort needs at least one setting".into());
            }
            Ok(DeviceOperation::SetInputPort {
                input_port_id,
                level,
                impedance,
                input_type,
                ground_lift,
            })
        }
        "setOutputPort" => {
            let output_port_id = bounded_u32(params, "outputPortId", 22)?;
            if !is_supported_output_settings_port(output_port_id) {
                return Err(
                    "outputPortId must identify a supported output settings port (1, 2, or 4 through 9)"
                        .into(),
                );
            }
            let level = optional_output_level(params)?;
            let ground_lift = optional_output_ground_lift(params)?;
            let mute = optional_boolean(params, "mute")?;
            if level.is_none() && ground_lift.is_none() && mute.is_none() {
                return Err("setOutputPort needs at least one setting".into());
            }
            if ground_lift.is_some() && !matches!(output_port_id, 1 | 4 | 5) {
                return Err("groundLift is available only for output ports 1, 4, and 5".into());
            }
            if mute.is_some() && !matches!(output_port_id, 1 | 2 | 4..=7) {
                return Err("mute is available only for output ports 1, 2, and 4 through 7".into());
            }
            Ok(DeviceOperation::SetOutputPort {
                output_port_id,
                level,
                ground_lift,
                mute,
            })
        }
        "setUsbPort" => {
            let level = optional_normalized(params, "level")?;
            let headphones_source = optional_normalized(params, "headphonesSource")?;
            let dry_wet = optional_normalized(params, "dryWet")?;
            if headphones_source.is_some_and(|value| !is_usb_headphones_source_value(value)) {
                return Err(
                    "headphonesSource must be 0 (none), 1/3 (USB 1/2), 2/3 (USB 3/4), or 1 (both)"
                        .into(),
                );
            }
            if dry_wet.is_some_and(|value| !is_usb_dry_wet_value(value)) {
                return Err("dryWet must be 0 (1/2 dry, 3/4 wet) or 1 (3/4 dry, 1/2 wet)".into());
            }
            if level.is_none() && headphones_source.is_none() && dry_wet.is_none() {
                return Err("setUsbPort needs at least one setting".into());
            }
            Ok(DeviceOperation::SetUsbPort {
                level,
                headphones_source,
                dry_wet,
            })
        }
        "setMidiThru" => Ok(DeviceOperation::SetMidiThru(boolean(params, "enabled")?)),
        "setOutputPairing" => {
            let xlr12_linked = optional_boolean(params, "xlr12Linked")?;
            let out34_linked = optional_boolean(params, "out34Linked")?;
            if xlr12_linked.is_none() && out34_linked.is_none() {
                return Err("setOutputPairing needs xlr12Linked or out34Linked".into());
            }
            Ok(DeviceOperation::SetOutputPairing {
                xlr12_linked,
                out34_linked,
            })
        }
        "setGlobalEqBypassed" => Ok(DeviceOperation::SetGlobalEqBypassed(boolean(
            params, "bypassed",
        )?)),
        "setGlobalEqBand" => {
            let band = bounded_u32(params, "band", GLOBAL_EQ_BANDS)?;
            if band == 0 {
                return Err("band must be 1 through 5".into());
            }
            let mut controls = Vec::new();
            let base = i64::from(band - 1) * GLOBAL_EQ_BAND_STRIDE;
            for (field, offset) in [
                ("gain", GLOBAL_EQ_BAND_GAIN),
                ("frequency", GLOBAL_EQ_BAND_FREQUENCY),
                ("q", GLOBAL_EQ_BAND_Q),
            ] {
                if let Some(value) = optional_normalized(params, field)? {
                    controls.push(((base + offset) as i32, value));
                }
            }
            if let Some(value) = params.get("filterType").filter(|value| !value.is_null()) {
                let filter_type = value
                    .as_u64()
                    .filter(|value| *value <= 4)
                    .ok_or_else(|| "filterType must be null or 0 through 4".to_string())?;
                controls.push((
                    (base + GLOBAL_EQ_BAND_TYPE) as i32,
                    filter_type as f32 / 4.0,
                ));
            }
            if let Some(enabled) = optional_boolean(params, "enabled")? {
                controls.push((
                    (base + GLOBAL_EQ_BAND_ENABLED) as i32,
                    if enabled { 1.0 } else { 0.0 },
                ));
            }
            if controls.is_empty() {
                return Err("setGlobalEqBand needs at least one control".into());
            }
            Ok(DeviceOperation::SetGlobalEqParameters(controls))
        }
        "setGlobalEqOutput" => {
            let mut controls = Vec::new();
            if let Some(value) = params.get("level").filter(|value| !value.is_null()) {
                let level_db = value
                    .as_f64()
                    .filter(|value| value.is_finite() && (-20.0..=20.0).contains(value))
                    .ok_or_else(|| {
                        "level must be null or a finite dB value from -20 through 20".to_string()
                    })?;
                controls.push((25, ((level_db + 20.0) / 40.0) as f32));
            }
            if let Some(out12) = optional_boolean(params, "out12")? {
                controls.push((26, if out12 { 1.0 } else { 0.0 }));
            }
            if let Some(out34) = optional_boolean(params, "out34")? {
                controls.push((27, if out34 { 1.0 } else { 0.0 }));
            }
            if controls.is_empty() {
                return Err("setGlobalEqOutput needs level, out12, or out34".into());
            }
            Ok(DeviceOperation::SetGlobalEqParameters(controls))
        }
        "loadGlobalEqModelPreset" => {
            let value = validate_visible_text(required_text(params, "value")?, "value", 256)?;
            let is_factory = boolean(params, "isFactory")?;
            let hash = params
                .get("hash")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| "hash must be an unsigned 32-bit integer".to_string())?;
            Ok(DeviceOperation::LoadGlobalEqModelPreset {
                value,
                is_factory,
                hash,
            })
        }
        "setModeCycle" => {
            let values = params
                .get("slots")
                .and_then(Value::as_array)
                .filter(|values| !values.is_empty() && values.len() <= 3)
                .ok_or_else(|| "slots must contain one through three mode values".to_string())?;
            let mut slots = Vec::with_capacity(values.len());
            for value in values {
                let slot = value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value <= 8)
                    .ok_or_else(|| "mode-cycle values must be 0 through 8".to_string())?;
                if slots.contains(&slot) {
                    return Err("mode-cycle values must be unique".into());
                }
                slots.push(slot);
            }
            Ok(DeviceOperation::SetModeCycle(slots))
        }
        "setTempoMetronome" => {
            let mut values = Vec::new();
            let option = |key: &str, labels: &[&str]| -> Result<Option<f32>, String> {
                match params.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::String(value)) => labels
                        .iter()
                        .position(|label| *label == value)
                        .map(|index| index as f32 / (labels.len() - 1) as f32)
                        .ok_or_else(|| format!("unsupported {key} value"))
                        .map(Some),
                    _ => Err(format!("{key} must be a named option or null")),
                }
            };
            const TIME_SIGNATURES: &[&str] = &[
                "2/4",
                "3/4",
                "4/4",
                "5/4",
                "6/4",
                "7/4",
                "8/4",
                "9/4",
                "10/4",
                "11/4",
                "12/4",
                "13/4",
                "3/8",
                "6/8",
                "9/8",
                "12/8",
                "5/8 (3+2)",
                "5/8 (2+3)",
                "7/8 (3+2+2)",
                "7/8 (2+3+2)",
                "7/8 (2+2+3)",
            ];
            let time_signature = match params.get("timeSignature") {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) => TIME_SIGNATURES
                    .iter()
                    .position(|label| *label == value)
                    .ok_or_else(|| "unsupported timeSignature value".to_string())
                    .map(|index| Some((index, value.as_str()))),
                _ => Err("timeSignature must be a named option or null".into()),
            }?;
            // Signature first: it changes the subdivision menu and rewrites
            // the active beat/accent presentation pattern before subsequent updates.
            if let Some((index, _)) = time_signature {
                values.push((6, index as f32 / (TIME_SIGNATURES.len() - 1) as f32));
            }
            if let Some(enabled) = optional_boolean(params, "ledEnabled")? {
                values.push((2, if enabled { 1.0 } else { 0.0 }));
            }
            if let Some(db) = params
                .get("volumeDb")
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_f64()
                        .filter(|value| (-60.0..=9.0).contains(value))
                        .ok_or_else(|| "volumeDb must be -60 through +9 dB".to_string())
                })
                .transpose()?
            {
                // dB as the unit's Tempo screen shows it -> raw parameter,
                // through the same measured taper the decoder inverts.
                values.push((3, crate::commands::tempo_volume_raw_from_db(db as f32)));
            }
            if let Some(running) = optional_boolean(params, "running")? {
                values.push((4, if running { 1.0 } else { 0.0 }));
            }
            if let Some(pan) = params
                .get("pan")
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_f64()
                        .filter(|value| (-1.0..=1.0).contains(value))
                        .ok_or_else(|| "pan must be -1 through +1".to_string())
                })
                .transpose()?
            {
                values.push((5, ((pan + 1.0) / 2.0) as f32));
            }
            if params
                .get("subdivision")
                .is_some_and(|value| !value.is_null())
            {
                let label = params["subdivision"]
                    .as_str()
                    .ok_or_else(|| "subdivision must be a named option or null".to_string())?;
                let signature_group = time_signature.map(|(index, _)| index < 12);
                let value = match (signature_group, label) {
                    (Some(true), "1/4") | (Some(false), "1/8") | (None, "1/4") => 0.0,
                    (Some(true), "1/8") => 1.0 / 3.0,
                    (Some(false), "1/16") => 1.0 / 3.0,
                    (Some(true), "1/8T")
                    | (Some(false), "1/16T")
                    | (None, "1/8T")
                    | (None, "1/16T") => 2.0 / 3.0,
                    (Some(true), "1/16") | (Some(false), "1/32") | (None, "1/32") => 1.0,
                    (None, "1/8" | "1/16") => {
                        return Err(format!(
                            "subdivision {label} is ambiguous without the current timeSignature"
                        ));
                    }
                    _ => return Err("unsupported subdivision value for timeSignature".into()),
                };
                values.push((7, value));
            }
            if let Some(value) = option(
                "sound",
                &[
                    "BLIP", "BLOCK", "COWBELL", "DIGITAL", "DRUM KIT", "SOFT KIT",
                ],
            )? {
                values.push((8, value));
            }
            if let Some(value) = option(
                "routing",
                &["MULTI", "HP", "OUT 1/2", "OUT 3/4", "SEND 1/2"],
            )? {
                values.push((9, value));
            }
            if let Some(beats) = params.get("beats").filter(|value| !value.is_null()) {
                let beats = beats
                    .as_array()
                    .ok_or_else(|| "beats must be an array".to_string())?;
                let maximum_beats = time_signature
                    .and_then(|(_, signature)| signature.split_once('/'))
                    .and_then(|(numerator, _)| numerator.parse::<usize>().ok())
                    .unwrap_or(13);
                if beats.len() > maximum_beats {
                    return Err(format!(
                        "beats contains inactive steps; at most {maximum_beats} can be updated"
                    ));
                }
                for (index, beat) in beats.iter().enumerate() {
                    let label = beat.as_str().ok_or_else(|| {
                        "every beat must be NORMAL, MUTE, ACCENT 1, or ACCENT 2".to_string()
                    })?;
                    let value = ["NORMAL", "MUTE", "ACCENT 1", "ACCENT 2"]
                        .iter()
                        .position(|candidate| *candidate == label)
                        .ok_or_else(|| {
                            "every beat must be NORMAL, MUTE, ACCENT 1, or ACCENT 2".to_string()
                        })? as f32
                        / 3.0;
                    values.push((10 + index as u32, value));
                }
            }
            if values.is_empty() {
                return Err("setTempoMetronome needs at least one setting".into());
            }
            Ok(DeviceOperation::SetTempoParameters(values))
        }
        "setTempoMode" => Ok(DeviceOperation::SetTempoMode(
            match params.get("mode").and_then(Value::as_str) {
                Some("GLOBAL") => true,
                Some("PRESET") => false,
                _ => return Err("mode must be PRESET or GLOBAL".into()),
            },
        )),
        "setGlobalTempo" => {
            let bpm = bounded_u32(params, "bpm", domain::MAXIMUM_TEMPO_BPM)?;
            if bpm < domain::MINIMUM_TEMPO_BPM {
                return Err(format!(
                    "bpm must be an integer from {} through {}",
                    domain::MINIMUM_TEMPO_BPM,
                    domain::MAXIMUM_TEMPO_BPM
                ));
            }
            Ok(DeviceOperation::SetGlobalTempo(bpm))
        }
        "setFavorite" => {
            let name = validate_path_component(required_text(params, "name")?, "name", 80, &[])?;
            let folder_key =
                validate_library_key(required_text(params, "folderKey")?, "folderKey")?;
            let product_key =
                validate_visible_text(required_text(params, "productKey")?, "productKey", 512)?;
            if product_key.trim() != product_key {
                return Err("productKey must not have surrounding whitespace".into());
            }
            Ok(DeviceOperation::SetFavorite {
                name,
                folder_key,
                product_key,
                is_factory: boolean(params, "isFactory")?,
                is_plugin: match params.get("isPlugin") {
                    None => false,
                    Some(value) => value
                        .as_bool()
                        .ok_or_else(|| "isPlugin must be a boolean".to_string())?,
                },
                favorite: boolean(params, "favorite")?,
            })
        }
        "setModelPinned" => {
            let model_id = bounded_u32(params, "modelId", i32::MAX as u32)?;
            if model_id == 0 {
                return Err("modelId must be an integer from 1 through 2147483647".into());
            }
            let _expected_pinned = boolean(params, "expectedPinned")?;
            let pinned = boolean(params, "pinned")?;
            // Cortex Control pins a Neural Capture through its own route, by string capture key,
            // so the numeric command does not pin the Neural Capture model IDs. Only the new pin
            // is refused: undoing an existing pin must not acquire new proof, so an unpin of any
            // id is planned, whatever the id and whatever the catalog holds.
            if pinned && crate::commands::is_neural_capture_model(model_id) {
                return Err(
                    "modelId 14000 and 14001 are Neural Captures, which are pinned by their string capture key; setModelPinned accepts numeric Virtual Device models only"
                        .into(),
                );
            }
            Ok(DeviceOperation::SetModelPinned { model_id, pinned })
        }
        "createSetlist" | "deleteSetlist" => {
            let name = validate_setlist_name(required_text(params, "name")?)?;
            if operation == "createSetlist" {
                Ok(DeviceOperation::CreateSetlist { name })
            } else {
                Ok(DeviceOperation::DeleteSetlist { name })
            }
        }
        "movePreset" => Ok(DeviceOperation::MovePreset {
            setlist_key: writable_setlist_key(params, "setlistKey")?,
            product_key: validate_library_key(required_text(params, "productKey")?, "productKey")?,
            position: bounded_u32(params, "destinationPosition", 255)?,
        }),
        "loadCapture" => {
            let model_id = optional_positive_u32(params, "modelId")?;
            if model_id.is_some_and(|model_id| !crate::commands::is_neural_capture_model(model_id))
            {
                return Err("modelId must be 14000 or 14001 for a Neural Capture".into());
            }
            // Cortex uses the authoritative library key on the wire.  Keep the
            // required display name as caller confirmation metadata only.
            let _name = validate_visible_text(required_text(params, "name")?, "name", 128)?;
            Ok(DeviceOperation::LoadCapture {
                row: row()?,
                column: column("column")?,
                key: validate_library_key(required_text(params, "key")?, "key")?,
                model_id,
            })
        }
        "loadIr" => {
            if optional_positive_u32(params, "modelId")?.is_some() {
                return Err(
                    "Placing an IR Loader is a separate Cortex addModel workflow and is not supported by loadIr; modelId must be null"
                        .into(),
                );
            }
            // Cortex sends ProductData.key only. Keep the display name as
            // validated caller-confirmation metadata; it is not a wire field.
            let _name = validate_visible_text(required_text(params, "name")?, "name", 128)?;
            Ok(DeviceOperation::LoadIr {
                row: row()?,
                column: column("column")?,
                key: validate_library_key(required_text(params, "key")?, "key")?,
                slot: bounded_u32(params, "slot", 1)?,
            })
        }
        "addBlock" => Ok(DeviceOperation::AddBlock {
            row: row()?,
            column: column("column")?,
            model_id: params
                .get("modelId")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "modelId must be a positive integer".to_string())?,
        }),
        "replaceBlock" => Ok(DeviceOperation::ReplaceBlock {
            row: row()?,
            column: column("column")?,
            model_id: params
                .get("modelId")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "modelId must be a positive integer".to_string())?,
        }),
        "createModelPreset" => Ok(DeviceOperation::CreateModelPreset {
            row: row()?,
            column: column("column")?,
            model_hash: bounded_u32(params, "modelHash", i32::MAX as u32)?,
            name: validate_visible_text(required_text(params, "name")?, "name", 80)?,
        }),
        "setLooperStompAssignments" => {
            let assignments = params
                .get("assignments")
                .and_then(Value::as_array)
                .ok_or_else(|| "assignments must be an array".to_string())?
                .iter()
                .map(|value| {
                    value
                        .as_i64()
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or_else(|| "assignments must contain integers".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeviceOperation::SetLooperStompAssignments { assignments })
        }
        "removeBlock" => Ok(DeviceOperation::RemoveBlock {
            row: row()?,
            column: column("column")?,
        }),
        "resetBlockParameters" | "device.resetBlockParameters" => {
            Ok(DeviceOperation::ResetBlockParameters {
                row: row()?,
                column: column("column")?,
            })
        }
        "moveBlock" => Ok(DeviceOperation::MoveBlock {
            from_row: row()?,
            from_column: column("fromColumn")?,
            to_row: params
                .get("toRow")
                .or_else(|| params.get("row"))
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value < domain::GRID_ROWS)
                .ok_or_else(|| {
                    format!(
                        "toRow must be an integer from 0 through {}",
                        domain::GRID_ROWS - 1
                    )
                })?,
            to_column: column("toColumn")?,
            grid_model_ids: Vec::new(),
        }),
        "setFootswitch" => Ok(DeviceOperation::SetFootswitch {
            row: row()?,
            column: column("column")?,
            footswitch: match params.get("footswitch") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value < domain::SCENE_COUNT)
                        .ok_or_else(|| {
                            format!(
                                "footswitch must be null or an integer from 0 through {}",
                                domain::SCENE_COUNT - 1
                            )
                        })?,
                ),
            },
            assignment_type: parse_footswitch_assignment_type(
                &required_text(params, "assignmentType")?,
                "assignmentType",
            )?,
            previous_footswitch: match params.get("expectedFootswitch") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value < domain::SCENE_COUNT)
                        .ok_or_else(|| {
                            format!(
                                "expectedFootswitch must be null or an integer from 0 through {}",
                                domain::SCENE_COUNT - 1
                            )
                        })?,
                ),
            },
            previous_assignment_type: match params.get("expectedAssignmentType") {
                None | Some(Value::Null) => None,
                Some(value) => Some(parse_footswitch_assignment_type(
                    value.as_str().ok_or_else(|| {
                        "expectedAssignmentType must be primary, secondary, or null".to_string()
                    })?,
                    "expectedAssignmentType",
                )?),
            },
        }),
        "setChainInput" => Ok(DeviceOperation::SetChainInput {
            row: row()?,
            input_id: {
                let input_id = bounded_u32(params, "inputId", u32::MAX)?;
                if !domain::INPUT_ROUTES.iter().any(|(id, _)| *id == input_id) {
                    return Err(
                        "inputId must be one of the known device input route IDs 0 through 13"
                            .into(),
                    );
                }
                input_id
            },
        }),
        "setChainOutput" => Ok(DeviceOperation::SetChainOutput {
            row: row()?,
            output_id: params
                .get("outputId")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| crate::commands::is_chain_output_port_id(*value))
                .ok_or_else(|| {
                    "outputId must be one of the device's output route IDs, 0 through 22 \
                     (Out, Send, USB 3-8, Row 3/4, Multi Out and Internal)"
                        .to_string()
                })?,
        }),
        "setChainSplit" => {
            let row = row()?;
            if row % 2 != 0 {
                return Err("Parallel routing is available only on rows 0 and 2".into());
            }
            let split_column = optional_i32(params, "splitColumn")?;
            let mix_column = optional_i32(params, "mixColumn")?;
            if split_column.is_some_and(|value| !(0..domain::GRID_COLUMNS as i32).contains(&value))
                || mix_column
                    .is_some_and(|value| !(0..domain::GRID_COLUMNS as i32).contains(&value))
            {
                return Err("splitColumn and mixColumn must be null or 0 through 7".into());
            }
            if split_column
                .zip(mix_column)
                .is_some_and(|(split, mix)| mix <= split)
            {
                return Err("mixColumn must follow splitColumn when both are present".into());
            }
            Ok(DeviceOperation::SetChainSplit {
                row,
                split_column,
                mix_column,
            })
        }
        "setSplitMute" => {
            let row = row()?;
            if row % 2 != 0 {
                return Err("Splitter/mixer controls are available only on rows 0 and 2".into());
            }
            Ok(DeviceOperation::SetSplitMute {
                row,
                muted: boolean(params, "muted")?,
            })
        }
        "setRoutingParameter" => {
            let node = required_text(params, "node")?;
            if !matches!(node.as_str(), "splitter" | "mixer") {
                return Err("node must be splitter or mixer".into());
            }
            Ok(DeviceOperation::SetRoutingParameter {
                row: row()?,
                node,
                parameter_index: bounded_u32(params, "parameterIndex", u32::MAX)?,
                value: normalized(params, "value")?,
            })
        }
        "device.setLaneControlParameter" | "setLaneControlParameter" => {
            let control = required_text(params, "control")?;
            if !matches!(control.as_str(), "inputGate" | "laneOutput") {
                return Err("control must be inputGate or laneOutput".into());
            }
            Ok(DeviceOperation::SetLaneControlParameter {
                row: row()?,
                control,
                parameter_index: bounded_u32(params, "parameterIndex", u32::MAX)?,
                value: normalized(params, "value")?,
            })
        }
        "device.setLaneControlSceneMode" | "setLaneControlSceneMode" => {
            let control = required_text(params, "control")?;
            if !matches!(control.as_str(), "inputGate" | "laneOutput") {
                return Err("control must be inputGate or laneOutput".into());
            }
            Ok(DeviceOperation::SetLaneControlSceneMode {
                row: row()?,
                control,
                parameter_index: bounded_u32(params, "parameterIndex", u32::MAX)?,
                enabled: boolean(params, "enabled")?,
            })
        }
        "device.setLaneControlExpression" | "setLaneControlExpression" => {
            let control = required_text(params, "control")?;
            if !matches!(control.as_str(), "inputGate" | "laneOutput") {
                return Err("control must be inputGate or laneOutput".into());
            }
            Ok(DeviceOperation::SetLaneControlExpression {
                row: row()?,
                control,
                parameter_index: bounded_u32(params, "parameterIndex", u32::MAX)?,
                pedal: bounded_u32(params, "pedal", 2)?,
                minimum: normalized(params, "minimum")?,
                maximum: normalized(params, "maximum")?,
            })
        }
        "device.setParameterSceneMode" | "setParameterSceneMode" => {
            let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
            if column >= domain::GRID_COLUMNS && row()? % 2 != 0 {
                return Err("Splitter and mixer parameters exist only on rows 0 and 2".into());
            }
            Ok(DeviceOperation::SetParameterSceneMode {
                row: row()?,
                column,
                parameter_index: bounded_u32(params, "parameterIndex", u32::MAX)?,
                enabled: boolean(params, "enabled")?,
            })
        }
        "device.setParameterExpression" | "setParameterExpression" => {
            let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
            if column >= domain::GRID_COLUMNS && row()? % 2 != 0 {
                return Err("Splitter and mixer parameters exist only on rows 0 and 2".into());
            }
            Ok(DeviceOperation::SetParameterExpression {
                row: row()?,
                column,
                parameter_index: bounded_u32(params, "parameterIndex", u32::MAX)?,
                pedal: bounded_u32(params, "pedal", 2)?,
                minimum: normalized(params, "minimum")?,
                maximum: normalized(params, "maximum")?,
            })
        }
        "device.setStompMomentary" | "setStompMomentary" => {
            Ok(DeviceOperation::SetStompMomentary {
                footswitch: bounded_u32(params, "footswitch", domain::SCENE_COUNT - 1)?,
                momentary: boolean(params, "momentary")?,
            })
        }
        "device.setStompLabel" | "setStompLabel" => {
            let label = validate_stomp_label(required_text(params, "label")?)?;
            Ok(DeviceOperation::SetStompLabel {
                footswitch: bounded_u32(params, "footswitch", domain::SCENE_COUNT - 1)?,
                label,
                single: params
                    .get("single")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        }
        "device.setMidiOut" | "setMidiOut" => Ok(DeviceOperation::SetMidiOut {
            source: bounded_u32(params, "source", *crate::commands::MIDI_OUT_SOURCES.end())?,
            messages: midi_out_messages(params, "messages")?,
        }),
        "device.setPresetLoadMidiOut" | "setPresetLoadMidiOut" => {
            Ok(DeviceOperation::SetPresetLoadMidiOut {
                messages: midi_out_messages(params, "messages")?,
            })
        }
        "device.setExpressionBypass" | "setExpressionBypass" => {
            Ok(DeviceOperation::SetExpressionBypass {
                row: row()?,
                column: column("column")?,
                // Zero clears the assignment. The read side already agrees:
                // model_bypass_expression drops a bypass expression whose pedal
                // is zero, so a cleared block reads back as carrying none, and
                // the verification arm below expects exactly that.
                //
                // Absence afterwards is meaningful only where there was an
                // assignment to remove: on a block that already carried none,
                // the cleared and the ignored cases are the same state. The
                // planner does not try to tell them apart: `verified` here
                // means the readback shows the state that was asked for, and
                // `changed` is the field that says whether anything moved -
                // see verification_for_operation.
                pedal: params
                    .get("pedal")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| (0..=2).contains(value))
                    .ok_or_else(|| "pedal must be 0, 1 or 2".to_string())?,
                mode: bounded_u32(params, "mode", 2)?,
                invert: boolean(params, "invert")?,
                delay_ms: bounded_u32(params, "delayMs", 5_000)?,
                latch_emulation: boolean(params, "latchEmulation")?,
            })
        }
        "listPresetFolders" => Ok(DeviceOperation::ListPresetFolders),
        "savePreset" => Ok(DeviceOperation::SavePreset {
            setlist_key: validate_setlist_key(required_text(params, "setlistKey")?, "setlistKey")?,
            position: bounded_u32(params, "position", 255)?,
            name: validate_path_component(required_text(params, "name")?, "name", 80, &[])?,
            instrument: preset_instrument(params)?,
        }),
        "device.copyScene" | "copyScene" => Ok(DeviceOperation::CopyScene {
            from_index: bounded_u32(params, "fromScene", domain::SCENE_COUNT - 1)?,
            to_index: bounded_u32(params, "toScene", domain::SCENE_COUNT - 1)?,
            swap: params.get("swap").and_then(Value::as_bool).unwrap_or(false),
        }),
        "device.setSceneLabel" | "setSceneLabel" => Ok(DeviceOperation::SetSceneLabel {
            scene: bounded_u32(params, "scene", domain::SCENE_COUNT - 1)?,
            label: match params.get("label") {
                None | Some(Value::Null) => None,
                Some(Value::String(label)) => Some(label.clone()),
                _ => return Err("label must be a string or null".into()),
            },
        }),
        "device.setSceneColor" | "setSceneColor" => Ok(DeviceOperation::SetSceneColor {
            scene: bounded_u32(params, "scene", domain::SCENE_COUNT - 1)?,
            color: params
                .get("color")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| "color must be an unsigned 32-bit ARGB integer".to_string())?,
        }),
        _ => Err(format!("unknown native device operation: {operation}")),
    }
}

fn verification_for_operation(
    operation: &DeviceOperation,
    params: &Value,
    snapshot: Option<&GatewaySnapshot>,
) -> GatewayVerification {
    match operation {
        DeviceOperation::SetLaneControlParameter {
            row,
            control,
            parameter_index,
            value,
        } => GatewayVerification::Parameter {
            row: *row,
            column: if control == "inputGate" { 10 } else { 11 },
            parameter_index: *parameter_index,
            value: *value as f64,
            routing_model_id: None,
        },
        DeviceOperation::AddBlock {
            row,
            column,
            model_id,
        }
        | DeviceOperation::ReplaceBlock {
            row,
            column,
            model_id,
        } => GatewayVerification::Block {
            row: *row,
            column: *column,
            model_id: Some(*model_id),
            present: true,
        },
        DeviceOperation::RemoveBlock { row, column } => GatewayVerification::Block {
            row: *row,
            column: *column,
            model_id: None,
            present: false,
        },
        DeviceOperation::ResetBlockParameters { .. } => GatewayVerification::None,
        DeviceOperation::LoadCapture {
            row,
            column,
            model_id: Some(model_id),
            ..
        } => GatewayVerification::Block {
            row: *row,
            column: *column,
            model_id: Some(*model_id),
            present: true,
        },
        // The target model identity is unchanged by retargeting and therefore
        // cannot prove that parameter 5 was applied. CorOS exposes no
        // workflow-specific acknowledgement, so report accepted/unverified.
        DeviceOperation::RetargetCapture { .. } => GatewayVerification::None,
        DeviceOperation::MoveBlock {
            from_row,
            from_column,
            to_row,
            to_column,
            ..
        } => GatewayVerification::MoveBlock {
            from_row: *from_row,
            from_column: *from_column,
            to_row: *to_row,
            to_column: *to_column,
            model_id: params
                .get("expectedModelId")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .or_else(|| {
                    snapshot.and_then(|snapshot| {
                        snapshot
                            .blocks
                            .iter()
                            .find(|block| block.row == *from_row && block.column == *from_column)
                            .and_then(|block| block.model_id)
                    })
                })
                .unwrap_or_default(),
        },
        DeviceOperation::SetFootswitch {
            row,
            column,
            footswitch,
            assignment_type,
            ..
        } => GatewayVerification::Footswitch {
            row: *row,
            column: *column,
            footswitch: *footswitch,
            assignment_type: footswitch.map(|_| footswitch_assignment_type_name(assignment_type)),
        },
        DeviceOperation::SetStompMomentary {
            footswitch,
            momentary,
        } => GatewayVerification::StompMomentary {
            footswitch: *footswitch,
            momentary: *momentary,
        },
        DeviceOperation::SetStompLabel {
            footswitch, label, ..
        } => GatewayVerification::StompLabel {
            footswitch: *footswitch,
            label: label.clone(),
        },
        DeviceOperation::SetMidiOut { source, messages } => GatewayVerification::MidiOut {
            source: Some(*source),
            messages: messages.clone(),
        },
        DeviceOperation::SetPresetLoadMidiOut { messages } => GatewayVerification::MidiOut {
            source: None,
            messages: messages.clone(),
        },
        DeviceOperation::SetExpressionBypass {
            row,
            column,
            pedal,
            mode,
            invert,
            delay_ms,
            latch_emulation,
        } => GatewayVerification::ExpressionBypass {
            // The predicate holds for a clear whether or not the block carried an
            // assignment, and that is correct: `verified` here means a fresh
            // authoritative readback shows the state that was asked for, not that
            // the device demonstrably moved. `changed` is what says whether
            // anything moved, and a same-value write reporting
            // `verified: true, changed: false` is this repository's established
            // shape - _require_dirty_after_change records exactly that for an
            // ordinary numeric parameter on retail firmware.
            //
            // Returning GatewayVerification::None here was tried, on the
            // grounds that absence after a clear proves nothing when the block was
            // already unassigned. True, but the label it produces says something
            // else: SHARED_QC_ASSISTANT_GUIDE defines accepted_unverified as "the
            // frame was accepted for transmission and nothing confirmed it landed
            // - say so, then verify with the matching get_* call", which is what
            // set_bypass and the other realtime writes mean by it. Reporting that
            // for a write whose desired state *was* read back told callers to go
            // and check something already checked, and dropped the snapshot the
            // guide has them read. The honest fix for the original finding is the
            // gateway's detail string, which no longer claims a clear happened
            // where nothing was assigned.
            row: *row,
            column: *column,
            pedal: *pedal,
            mode: *mode,
            invert: *invert,
            delay_ms: *delay_ms,
            latch_emulation: *latch_emulation,
        },
        DeviceOperation::SetChainInput { row, input_id } => GatewayVerification::RouteInput {
            row: *row,
            input_id: *input_id,
        },
        DeviceOperation::SetChainOutput { row, output_id } => GatewayVerification::RouteOutput {
            row: *row,
            output_id: *output_id,
        },
        DeviceOperation::SetChainSplit {
            row,
            split_column,
            mix_column,
        } => GatewayVerification::RouteSplit {
            row: *row,
            split_column: *split_column,
            mix_column: *mix_column,
        },
        DeviceOperation::SetSplitMute { row, muted } => GatewayVerification::SplitMute {
            row: *row,
            muted: *muted,
            scene: params
                .get("expectedScene")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .or_else(|| snapshot.map(|state| state.active_scene)),
        },
        DeviceOperation::SavePreset {
            setlist_key,
            position,
            name,
            ..
        } => GatewayVerification::Preset {
            setlist_key: setlist_key.clone(),
            position: *position,
            name: Some(name.clone()),
            require_clean: true,
            after_position_revision: None,
            after_full_preset_revision: None,
        },
        DeviceOperation::SetSceneLabel { scene, label } => GatewayVerification::SceneLabel {
            scene: *scene,
            label: label.clone(),
        },
        DeviceOperation::SetSceneColor { scene, color } => GatewayVerification::SceneColor {
            scene: *scene,
            color: *color,
        },
        DeviceOperation::SetLaneControlSceneMode {
            row,
            control,
            parameter_index,
            enabled,
        } => GatewayVerification::ParameterSceneMode {
            row: *row,
            column: if control == "inputGate" { 10 } else { 11 },
            parameter_index: *parameter_index,
            enabled: *enabled,
        },
        DeviceOperation::SetParameterSceneMode {
            row,
            column,
            parameter_index,
            enabled,
        } => GatewayVerification::ParameterSceneMode {
            row: *row,
            column: *column,
            parameter_index: *parameter_index,
            enabled: *enabled,
        },
        DeviceOperation::SetParameterExpression {
            row,
            column,
            parameter_index,
            pedal,
            minimum,
            maximum,
        } => GatewayVerification::ParameterExpression {
            row: *row,
            column: *column,
            parameter_index: *parameter_index,
            pedal: *pedal,
            minimum: f64::from(*minimum),
            maximum: f64::from(*maximum),
        },
        DeviceOperation::SetLaneControlExpression {
            row,
            control,
            parameter_index,
            pedal,
            minimum,
            maximum,
        } => GatewayVerification::ParameterExpression {
            row: *row,
            column: if control == "inputGate" { 10 } else { 11 },
            parameter_index: *parameter_index,
            pedal: *pedal,
            minimum: f64::from(*minimum),
            maximum: f64::from(*maximum),
        },
        DeviceOperation::CopyScene {
            from_index,
            to_index,
            swap,
        } => snapshot
            .and_then(|before| {
                Some(GatewayVerification::SceneCopy {
                    from_scene: *from_index,
                    to_scene: *to_index,
                    swap: *swap,
                    from_label: before.scenes.get(*from_index as usize)?.clone(),
                    to_label: before.scenes.get(*to_index as usize)?.clone(),
                    from_color: before
                        .scene_colors
                        .as_ref()
                        .and_then(|colors| colors.get(*from_index as usize))
                        .cloned(),
                    to_color: before
                        .scene_colors
                        .as_ref()
                        .and_then(|colors| colors.get(*to_index as usize))
                        .cloned(),
                })
            })
            .unwrap_or(GatewayVerification::None),
        DeviceOperation::Command(_)
        | DeviceOperation::SetRoutingParameter { .. }
        | DeviceOperation::CreateModelPreset { .. }
        | DeviceOperation::SetLooperStompAssignments { .. }
        | DeviceOperation::ListPresetFolders
        | DeviceOperation::ReadVersion
        | DeviceOperation::SetDeviceName(_)
        | DeviceOperation::Undo
        | DeviceOperation::Redo
        | DeviceOperation::ReadInhibitedModules
        | DeviceOperation::ReadTuner
        | DeviceOperation::ShowTuner(_)
        | DeviceOperation::SetTunerInput(_)
        | DeviceOperation::SetTunerMute(_)
        | DeviceOperation::SetTunerMeter(_)
        | DeviceOperation::SetTunerReference(_)
        | DeviceOperation::ReadGeneralSettings
        | DeviceOperation::SetGeneralInteger { .. }
        | DeviceOperation::SetGeneralToggle { .. }
        | DeviceOperation::SetSceneBypassBehavior(_)
        | DeviceOperation::SetMasterVolumeAssignment { .. }
        | DeviceOperation::SetGlobalBypass { .. }
        | DeviceOperation::ReadIoSettings
        | DeviceOperation::SetInputPort { .. }
        | DeviceOperation::SetOutputPort { .. }
        | DeviceOperation::SetUsbPort { .. }
        | DeviceOperation::SetMidiThru(_)
        | DeviceOperation::SetOutputPairing { .. }
        | DeviceOperation::ReadGlobalEq
        | DeviceOperation::SetGlobalEqBypassed(_)
        | DeviceOperation::SetGlobalEqParameters(_)
        | DeviceOperation::LoadGlobalEqModelPreset { .. }
        | DeviceOperation::ReadModeCycle
        | DeviceOperation::SetModeCycle(_)
        | DeviceOperation::ReadGlobalTempo
        | DeviceOperation::SetGlobalTempo(_)
        | DeviceOperation::SetTempoParameters(_)
        | DeviceOperation::SetTempoMode(_)
        | DeviceOperation::ReadLooperStatus
        | DeviceOperation::ReadRecentsFavorites { .. }
        | DeviceOperation::SetFavorite { .. }
        | DeviceOperation::ReadPinnedModels
        | DeviceOperation::SetModelPinned { .. }
        | DeviceOperation::ReadLibraryFiles { .. }
        | DeviceOperation::CreateSetlist { .. }
        | DeviceOperation::DeleteSetlist { .. }
        | DeviceOperation::DeletePreset { .. }
        | DeviceOperation::MovePreset { .. }
        | DeviceOperation::SwapPreset { .. }
        | DeviceOperation::CopyPreset { .. }
        | DeviceOperation::DuplicateSetlist { .. }
        | DeviceOperation::RenamePreset { .. }
        | DeviceOperation::LoadCapture { model_id: None, .. }
        | DeviceOperation::LoadIr { .. }
        | DeviceOperation::PresetScreenshot { .. }
        | DeviceOperation::CaptureScreen
        | DeviceOperation::ReadDiagnostics { .. }
        | DeviceOperation::ReadGraphicsTree
        | DeviceOperation::ScreenTap { .. }
        | DeviceOperation::ScreenHold { .. }
        | DeviceOperation::ScreenDrag { .. }
        | DeviceOperation::ScreenDragSteps { .. }
        | DeviceOperation::ScreenMouseFrame { .. }
        | DeviceOperation::ReadCurrentPreset => GatewayVerification::None,
    }
}

fn splitter_routing_identity(
    snapshot: Option<&GatewaySnapshot>,
    row: u32,
) -> Result<(&'static str, u32), String> {
    let model_id = snapshot
        .and_then(|snapshot| snapshot.routes.iter().find(|route| route.row == row))
        .and_then(|route| route.splitter_model_id)
        .ok_or_else(|| {
            format!(
                "Splitter identity for row {row} is unavailable or ambiguous; refresh the current preset and retry"
            )
        })?;
    Ok((
        if matches!(model_id, 10_000..=10_002) {
            "legacySplitter"
        } else {
            "splitter"
        },
        model_id,
    ))
}

fn exact_move_operation(
    params: &Value,
    state: Option<&crate::device::state_runtime::DeviceStateRuntime>,
) -> Result<DeviceOperation, String> {
    let state = state.ok_or_else(|| {
        "Moving a block requires the exact current grid; use the stateful native planner"
            .to_string()
    })?;
    let from_row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
    let from_column = bounded_u32(params, "fromColumn", domain::GRID_COLUMNS - 1)?;
    let to_row = match params.get("toRow") {
        Some(value) if !value.is_null() => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value < domain::GRID_ROWS)
            .ok_or_else(|| {
                format!(
                    "toRow must be an integer from 0 through {}",
                    domain::GRID_ROWS - 1
                )
            })?,
        _ => from_row,
    };
    let to_column = bounded_u32(params, "toColumn", domain::GRID_COLUMNS - 1)?;
    let expected_model_id = params
        .get("expectedModelId")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "expectedModelId must be a positive integer".to_string())
        })
        .transpose()?;
    state.move_block_operation(from_row, from_column, to_row, to_column, expected_model_id)
}

fn configured_mode_for_slot(
    snapshot: &GatewaySnapshot,
    requested_slot: u32,
) -> Result<u32, String> {
    let slots = snapshot.mode_slots.as_ref().ok_or_else(|| {
        "Selecting a configured Mode Slot requires synchronized device mode-cycle state".to_string()
    })?;
    if slots.is_empty() || slots.len() > 3 {
        return Err("The synchronized device mode-cycle state is malformed".into());
    }

    let mut requested = None;
    for (expected_slot, entry) in slots.iter().enumerate() {
        if entry.slot != expected_slot as u32 {
            return Err("The synchronized device mode-cycle state is malformed".into());
        }
        let (mode, kind) = match entry.label.as_str() {
            "PRESET" => (0, "PRESET"),
            "SCENE" => (1, "SCENE"),
            "STOMP" => (2, "STOMP"),
            "PRESET / SCENE" => (3, "HYBRID"),
            "PRESET / STOMP" => (4, "HYBRID"),
            "SCENE / PRESET" => (5, "HYBRID"),
            "SCENE / STOMP" => (6, "HYBRID"),
            "STOMP / PRESET" => (7, "HYBRID"),
            "STOMP / SCENE" => (8, "HYBRID"),
            _ => return Err("The synchronized device mode-cycle state is malformed".into()),
        };
        if entry.mode != kind {
            return Err("The synchronized device mode-cycle state is malformed".into());
        }
        if entry.slot == requested_slot {
            requested = Some((mode, kind, entry.label.as_str()));
        }
    }
    if let Some((_, "HYBRID", label)) = requested {
        // Only 0 PRESET, 1 SCENE and 2 STOMP have been written and observed on a
        // unit. The hybrid wire values are unmeasured, so none is ever sent.
        return Err(format!(
            "Mode Slot {} is configured as the hybrid mode {label:?}. Selecting a hybrid mode slot is unsupported: the wire value for hybrid modes has not been measured. Nothing was sent.",
            requested_slot + 1
        ));
    }
    requested.map(|(mode, _, _)| mode).ok_or_else(|| {
        format!(
            "Mode Slot {} is not present in the synchronized device mode cycle",
            requested_slot + 1
        )
    })
}

/// The kind and per-row base modes a mode value must read back as. The one table the state
/// decoder uses; a value outside `0..=8` verifies as nothing.
fn mode_verification_identity(mode: u32) -> Option<(&'static str, [&'static str; 2])> {
    crate::state::mode_identity(mode)
}

fn mode_verification_matches(snapshot: &GatewaySnapshot, mode: u32) -> bool {
    let Some((kind, pair)) = mode_verification_identity(mode) else {
        return false;
    };
    snapshot.mode == kind
        && snapshot.footswitch_modes.as_ref().is_some_and(|actual| {
            actual.len() == pair.len()
                && actual
                    .iter()
                    .zip(pair)
                    .all(|(actual, expected)| actual == expected)
        })
}

/// Plan a gateway write of `method` from `params` and the latest `snapshot`.
///
/// Checks the method's required expected-state guards against `snapshot`, validates the arguments,
/// and returns the message to send, a summary and the predicate that confirms it. Writes that need
/// the full device-state runtime (block moves and Neural Capture retargeting) fail here; use
/// [`plan_gateway_write_with_state`] for them.
///
/// # Errors
///
/// Returns a message for an unsupported method, invalid arguments, a missing guard, or a stale
/// guard.
pub fn plan_gateway_write(
    method: &str,
    params: &Value,
    snapshot: Option<&GatewaySnapshot>,
) -> Result<GatewayWritePlan, String> {
    plan_gateway_write_inner(method, params, snapshot, None)
}

/// Plan against one atomically held device-state runtime. This is required by
/// full-model writes such as Neural Capture retargeting; stateless callers
/// continue to fail closed through [`plan_gateway_write`].
pub fn plan_gateway_write_with_state(
    method: &str,
    params: &Value,
    state: &crate::device::state_runtime::DeviceStateRuntime,
) -> Result<GatewayWritePlan, String> {
    // Preview is the same real Grid mutation as setParameter, but returns
    // without readback. Apply its optimistic concurrency check here so the
    // shared Android planner fails closed like the Windows broker.
    if method == "device.previewParameter" {
        let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
        let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
        let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
        let actual = state.block_details(row, column).and_then(|details| {
            details
                .parameters
                .into_iter()
                .find(|parameter| parameter.index == parameter_index)
                .and_then(|parameter| parameter.normalized_value)
        });
        assert_expected_parameter(actual, params)?;
    }
    if matches!(method, "device.setParameter" | "device.previewParameter") {
        reject_sync_gated_numeric_write(params, state)?;
    }
    let snapshot = state.snapshot();
    plan_gateway_write_inner(
        method,
        params,
        snapshot.has_preset.then_some(snapshot),
        Some(state),
    )
}

/// While a Delay block's SYNC toggle is engaged, the device reports the
/// block's time/rate control (TIME) disabled (`enabled: false` in its own
/// block details) and takes the timing from SYNC NOTE instead. This function
/// refuses numeric writes to such a disabled control.
///
/// **Host policy (not a desktop-application frame).** The desktop
/// application's UI does not offer a device-disabled control for editing, and
/// this host mirrors that.
/// What the unit would do with the write is BELIEVED, not established: the
/// device-knowledge skill records a 2026-09-11 measurement that the unit
/// redirects such a write to SYNC NOTE, and no artifact for it is retained in
/// this repository. The refusal text therefore does not assert a redirect or
/// any other device behaviour.
///
/// Rather than let a caller believe a numeric write landed on a control the
/// device has disabled, fail closed whenever the target parameter is
/// itself device-disabled (`BlockParameter::enabled`, driven by the model
/// catalog's own `toggleOn`/`toggleOff` wiring -- exactly what disables
/// TIME while SYNC is on) *and* the switch the catalog names as the reason
/// it is disabled is a sync switch that is currently on.
///
/// The gating switch is asked for by index
/// (`DeviceStateRuntime::parameter_disabled_when_sibling_on`) rather than
/// found by scanning sibling names. Names alone cannot answer this:
/// Dual Delay carries `SYNC L` and `SYNC R`, each gating only its own side's
/// `DELAY TIME`, so a name scan either misses both or gates a left-side
/// write on the right-side switch. Asking the catalog also stops an
/// unrelated gate -- Misha Modulator's `PEDAL` behind `AUTO`, Multivoicer's
/// voices behind `GLISSANDO` -- from being reported as SYNC-gated
/// merely because the block happens to have SYNC engaged as well.
///
/// This surfaces a clear error naming the disabled control and the SYNC
/// state; it does not translate the write into a SYNC NOTE value, because the
/// ms-to-note-division mapping that would need is not known for every model.
///
/// Not every disabled control is reachable this way: three catalogue models
/// (Pattern Tremolo's `RATE` among them) declare a `SYNC` switch and a
/// `SYNC NOTE` control but no gate between them, so their time control never
/// reports `enabled: false` and nothing here can fire. Those still need
/// `get_block_details` read by the caller.
///
/// This is accepted, tracked technical debt: closing the three-model gap, or
/// translating the write into the correct SYNC NOTE value instead of failing
/// closed, both require the ms-to-note-division mapping for every affected
/// model, which is not yet available.
fn reject_sync_gated_numeric_write(
    params: &Value,
    state: &crate::device::state_runtime::DeviceStateRuntime,
) -> Result<(), String> {
    if params.get("text").is_some() {
        return Ok(());
    }
    let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
    let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
    if column >= domain::GRID_COLUMNS {
        // Splitter/mixer virtual parameters carry no per-model SYNC gating.
        return Ok(());
    }
    let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
    let Some(details) = state.block_details(row, column) else {
        return Ok(());
    };
    let Some(target) = details
        .parameters
        .iter()
        .find(|parameter| parameter.index == parameter_index)
    else {
        return Ok(());
    };
    if target.enabled {
        return Ok(());
    }
    let Some(gate_index) =
        state.parameter_disabled_when_sibling_on(details.model_id, parameter_index)
    else {
        return Ok(());
    };
    let Some(gate) = details
        .parameters
        .iter()
        .find(|parameter| parameter.index == gate_index)
    else {
        return Ok(());
    };
    if !is_sync_switch(gate) || gate.normalized_value.unwrap_or(0.0) < 0.5 {
        return Ok(());
    }
    Err(format!(
        "{} is reported disabled by the Quad Cortex because {} is on. The desktop application's UI does \
         not offer a device-disabled control, so this host refuses a numeric write to {} as \
         policy (host policy, not a desktop-application frame); what the unit would do with the \
         write is BELIEVED, not established (recorded as a 2026-09-11 measurement with no artifact \
         retained in the repository) -- turn {} off first, or write SYNC NOTE directly",
        target.name, gate.name, target.name, gate.name
    ))
}

/// Whether a gating switch is the note-division sync switch rather than some
/// other two-position control whose name merely contains "SYNC".
///
/// Enumerated over all 8851 parameters of the 633-model catalogue in
/// `docs/reference/qc-live-parameter-scales.json`: the note-division switches
/// are `SYNC` (68 models) and Dual Delay's `SYNC L`/`SYNC R`, and every one of
/// them is labelled `Off`/`On`. Two further parameters carry "SYNC" in their
/// name and are not this control at all -- Glitch's `SYNC SOURCE`
/// (`Global`/`Block`, which tempo the block follows; the model has no SYNC
/// NOTE rotary to redirect anything to) and Harmonic Tremolo's `SYNC ON`
/// (`Lows`/`Highs`, which band is affected, itself gated by that model's own
/// plain `SYNC`). The `Off`/`On` labelling is what separates them, so it is
/// required whenever the catalog declares labels at all.
fn is_sync_switch(parameter: &BlockParameter) -> bool {
    let name = parameter.name.trim();
    let named_sync = name.eq_ignore_ascii_case("sync")
        || name
            .split_once(char::is_whitespace)
            .is_some_and(|(head, _)| head.eq_ignore_ascii_case("sync"));
    let off_on = parameter.options.is_empty()
        || (parameter.options.len() == 2
            && parameter.options[0].eq_ignore_ascii_case("off")
            && parameter.options[1].eq_ignore_ascii_case("on"));
    named_sync && off_on
}

/// Host policy (not a desktop-application frame): the caller's
/// `expectedAssignments` must equal the looper assignment permutation in the
/// host's decoded general settings, so a permutation the caller did not see is
/// never overwritten. Fails closed when no decoded general settings (or no
/// looper assignments in them) are available.
fn assert_expected_looper_assignments(
    params: &Value,
    state: Option<&crate::device::state_runtime::DeviceStateRuntime>,
) -> Result<(), String> {
    let expected = params
        .get("expectedAssignments")
        .and_then(Value::as_array)
        .ok_or_else(|| "expectedAssignments must be an array".to_string())?
        .iter()
        .map(|value| {
            value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| "expectedAssignments must contain integers".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let unavailable = || {
        "The Looper X assignments in the decoded general settings are unavailable, so `expectedAssignments` cannot be checked; refusing the write. Reconnect the device session and retry.".to_string()
    };
    let payload = state
        .and_then(|state| state.published_settings(profile::MESSAGE_TYPE_GENERAL_SETTINGS))
        .ok_or_else(unavailable)?;
    let current = decode_general_settings(payload)
        .ok()
        .and_then(|settings| settings.looper_stomp_assignments)
        .ok_or_else(unavailable)?;
    if current != expected {
        return Err(
            "write was based on stale Looper X assignments: they changed on the Quad Cortex. Read the general settings again and retry."
                .into(),
        );
    }
    Ok(())
}

/// A Global EQ preset recall names the preset by the identity the device announced. It is
/// planned only against this session's retained fold (never against a published view): a recall
/// that fold cannot vouch for builds no frame. A stateless caller has no fold, so it is refused
/// as unavailable too.
///
/// Refusals keep apart the cases the fold can tell apart. Only "the fold cannot answer" (nothing
/// announced, or the fold invalidated) carries the `STATE_UNAVAILABLE: ` prefix, as the Grid
/// refusal does; a wrong family, an identity not announced (or deleted since) and an ambiguous
/// identity are plain sentences with their own wording. The fold is invalidated synchronously at
/// ingress when the transport counts a loss, so no further frame has to arrive before a write
/// notices a loss the owner can see.
fn assert_global_eq_preset_announced(
    params: &Value,
    state: Option<&crate::device::state_runtime::DeviceStateRuntime>,
) -> Result<(), String> {
    let DeviceOperation::LoadGlobalEqModelPreset {
        value,
        is_factory,
        hash,
    } = operation("loadGlobalEqModelPreset", params)?
    else {
        return Err("loadGlobalEqModelPreset did not parse as a Global EQ preset recall".into());
    };
    let Some(state) = state else {
        return Err(
            "STATE_UNAVAILABLE: Recalling a Global EQ preset needs the session's preset list"
                .into(),
        );
    };
    state
        .model_preset_catalog()
        .admits(&value, is_factory, hash)
        .map_err(|refusal| refusal.message())
}

fn validate_required_guard(method: &str, field: &str, params: &Value) -> Result<(), String> {
    let value = params
        .get(field)
        .ok_or_else(|| format!("{field} is required to guard {method}"))?;
    // Preserve the existing diagnostic for a missing usable preset-name guard.
    if !value.as_str().is_some_and(|name| !name.is_empty()) {
        if field == "expectedPresetName" {
            return Err(format!("{field} is required to guard {method}"));
        }
        if field == "expectedPresetIdentity" {
            return Err("A preset identity from current device state is required".into());
        }
    }
    let unsigned_between = |minimum: u64, maximum: u64| {
        value
            .as_u64()
            .is_some_and(|number| (minimum..=maximum).contains(&number))
    };
    let valid = match field {
        "expectedPresetName" | "expectedPresetIdentity" => {
            value.as_str().is_some_and(|text| !text.is_empty())
        }
        "expectedLabel" | "expectedColor" => value.is_string(),
        "expectedMode" if method == "device.setGlobalTempo" => value
            .as_str()
            .is_some_and(|mode| matches!(mode, "PRESET" | "GLOBAL")),
        "expectedMode" => value
            .as_str()
            .is_some_and(|mode| matches!(mode, "PRESET" | "SCENE" | "STOMP" | "HYBRID")),
        "expectedScene" => unsigned_between(0, u64::from(domain::SCENE_COUNT - 1)),
        "expectedTempo" | "expectedGlobalBpm" => unsigned_between(
            u64::from(domain::MINIMUM_TEMPO_BPM),
            u64::from(domain::MAXIMUM_TEMPO_BPM),
        ),
        "expectedValue" if method == "device.setMasterVolume" => unsigned_between(0, 100),
        // Text parameters have no normalized numeric value. Their explicit null remains
        // permitted only for a text write, never as a bypass of a numeric stale-value guard.
        "expectedValue" => {
            (value.is_null() && params.get("value").is_some_and(Value::is_string))
                || value
                    .as_f64()
                    .is_some_and(|number| number.is_finite() && (0.0..=1.0).contains(&number))
        }
        "expectedBypassed" | "expectedMuted" => value.is_boolean(),
        "expectedModelId" => value.is_null() || unsigned_between(0, u64::from(u32::MAX)),
        "expectedFootswitch" => {
            value.is_null() || unsigned_between(0, u64::from(domain::SCENE_COUNT - 1))
        }
        "expectedAssignmentType" => {
            value.is_null()
                || value
                    .as_str()
                    .is_some_and(|kind| matches!(kind, "primary" | "secondary"))
        }
        "expectedInputId" | "expectedOutputId" | "expectedSplitColumn" | "expectedMixColumn" => {
            value.is_null()
                || value
                    .as_i64()
                    .is_some_and(|number| i32::try_from(number).is_ok())
        }
        _ => return Err(format!("No native required-guard validator for {field}")),
    };
    if valid {
        Ok(())
    } else {
        Err(format!("{field} has an invalid guard value for {method}"))
    }
}

fn plan_gateway_write_inner(
    method: &str,
    params: &Value,
    snapshot: Option<&GatewaySnapshot>,
    state: Option<&crate::device::state_runtime::DeviceStateRuntime>,
) -> Result<GatewayWritePlan, String> {
    if method == "device.loadGlobalEqModelPreset" {
        assert_global_eq_preset_announced(params, state)?;
    }
    if gateway_method_requires_exact_grid(method) {
        let complete = state
            .map(|state| state.snapshot().grid_layout_complete)
            .or_else(|| snapshot.map(|snapshot| snapshot.grid_layout_complete));
        if complete != Some(true) {
            return Err(
                "STATE_UNAVAILABLE: The Grid layout is incomplete; this operation requires a complete current preset".into(),
            );
        }
    }
    let required_guards: &[&str] = match method {
        "device.selectScene"
        | "device.copyScene"
        | "device.selectModeSlot"
        | "device.setLaneControlSceneMode"
        | "device.setLaneControlExpression"
        | "device.setStompMomentary"
        | "device.setMidiOut"
        | "device.setPresetLoadMidiOut" => &["expectedPresetName"],
        "device.setStompLabel" => &["expectedLabel", "expectedPresetName"],
        "device.setSceneLabel" => &["expectedLabel", "expectedPresetName"],
        "device.setSceneColor" => &["expectedColor", "expectedPresetName"],
        "device.pressFootswitch" | "device.tapTempo" => &["expectedMode", "expectedPresetName"],
        "device.setMasterVolume" => &["expectedValue"],
        "device.setTempo" => &["expectedTempo", "expectedPresetName"],
        "device.setGlobalTempo" => &["expectedMode", "expectedGlobalBpm"],
        "device.toggleBypass" => &[
            "expectedModelId",
            "expectedBypassed",
            "expectedScene",
            "expectedPresetName",
        ],
        "device.setParameterSceneMode"
        | "device.setParameterExpression"
        | "device.setExpressionBypass"
        | "device.resetBlockParameters" => &["expectedModelId", "expectedPresetName"],
        "device.setParameter" | "device.previewParameter" => &[
            "expectedModelId",
            "expectedValue",
            "expectedScene",
            "expectedPresetName",
        ],
        "device.setLaneControlParameter" | "device.previewLaneControlParameter" => {
            &["expectedValue", "expectedPresetName"]
        }
        "device.moveBlock"
        | "device.addBlock"
        | "device.replaceBlock"
        | "device.removeBlock"
        | "device.loadCapture"
        | "device.loadIr" => &["expectedModelId", "expectedPresetName"],
        // The published arguments do not include expectedModelId, and expectedPresetName is
        // optional (assert_expected_preset checks a supplied name), so the preset identity is the
        // one guard this method requires. modelHash is bound to the block below.
        "device.createModelPreset" => &["expectedPresetIdentity"],
        "device.setBlockFootswitch" => &[
            "expectedFootswitch",
            "expectedAssignmentType",
            "expectedModelId",
            "expectedPresetName",
        ],
        "device.setChainInput" => &["expectedInputId", "expectedPresetName"],
        "device.setChainOutput" => &["expectedOutputId", "expectedPresetName"],
        "device.setChainSplit" => &[
            "expectedSplitColumn",
            "expectedMixColumn",
            "expectedPresetName",
        ],
        "device.setSplitMute" => &["expectedMuted", "expectedScene", "expectedPresetName"],
        _ => &[],
    };
    if matches!(
        method,
        "device.setParameter"
            | "device.previewParameter"
            | "device.setParameterSceneMode"
            | "device.setParameterExpression"
            | "device.toggleBypass"
            | "device.setExpressionBypass"
            | "device.resetBlockParameters"
    ) && !params
        .get("expectedModelId")
        .and_then(Value::as_u64)
        .is_some_and(|value| (1..=i32::MAX as u64).contains(&value))
    {
        return Err(
            "expectedModelId must be the positive model identifier from the observed block details"
                .into(),
        );
    }
    for field in required_guards {
        validate_required_guard(method, field, params)?;
    }
    if method == "device.setLooperStompAssignments" {
        assert_expected_looper_assignments(params, state)?;
    }
    if method == "device.addBlock" && !params.get("expectedModelId").is_some_and(Value::is_null) {
        return Err(
            "expectedModelId must be null because Cortex addModel targets an empty Grid cell"
                .into(),
        );
    }
    if method.starts_with("device.") {
        assert_expected_preset(snapshot, params)?;
        assert_expected_state(method, snapshot, params)?;
    }
    if method == "device.createModelPreset" {
        // modelHash names the model of the block the preset is saved from. Compare it with the
        // retained block exactly as expectedModelId is compared for the other block writes. A
        // missing, non-numeric or out-of-range modelHash is left to the operation parser, which
        // names modelHash. The catalog is not consulted: a block already in the grid may hold a
        // model the catalog hides. The checked cell is always the operation's own row and
        // column, so a stray fromColumn (assert_expected_state prefers it) is dropped.
        if let (Some(object), Some(model_hash)) = (
            params.as_object(),
            params
                .get("modelHash")
                .filter(|value| value.as_u64().is_some_and(|hash| hash <= i32::MAX as u64)),
        ) {
            let mut bound = object.clone();
            bound.remove("fromColumn");
            bound.insert("expectedModelId".into(), model_hash.clone());
            assert_expected_state(method, snapshot, &Value::Object(bound))?;
        }
    }
    if method == "device.pressFootswitch"
        && snapshot.is_some_and(|snapshot| snapshot.mode == "PRESET" && snapshot.dirty)
    {
        return Err(
            "The current preset has unsaved changes. Save or revert them before pressing a PRESET-mode footswitch."
                .into(),
        );
    }
    if method == "device.pressFootswitch" {
        if let Some(snapshot) = snapshot.filter(|snapshot| snapshot.mode == "PRESET") {
            assert_dirty_flag_observed(snapshot)?;
        }
    }
    let plan = match method {
        "device.setGeneralInteger"
        | "device.setGeneralToggle"
        | "device.setSceneBypassBehavior"
        | "device.setMasterVolumeAssignment"
        | "device.setGlobalBypass" => {
            let operation_name = match method {
                "device.setGeneralInteger" => "setGeneralInteger",
                "device.setGeneralToggle" => "setGeneralToggle",
                "device.setSceneBypassBehavior" => "setSceneBypassBehavior",
                "device.setMasterVolumeAssignment" => "setMasterVolumeAssignment",
                _ => "setGlobalBypass",
            };
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(operation(operation_name, params)?),
                detail: "Global device setting sent to the Quad Cortex".into(),
                verification: GatewayVerification::None,
            }
        }
        "device.setTunerInput"
        | "device.setTunerMute"
        | "device.setTunerMeter"
        | "device.setTunerReference" => {
            let operation_name = match method {
                "device.setTunerInput" => "setTunerInput",
                "device.setTunerMute" => "setTunerMute",
                "device.setTunerMeter" => "setTunerMeter",
                _ => "setTunerReference",
            };
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(operation(operation_name, params)?),
                detail: match method {
                    "device.setTunerInput" => {
                        "Tuner input write sent; the unit's reply is not read back"
                    }
                    "device.setTunerMute" => {
                        "Tuner mute write sent; the unit's reply is not read back"
                    }
                    "device.setTunerMeter" => {
                        "Tuner meter write sent; the unit's reply is not read back"
                    }
                    _ => "Tuner reference write sent; the unit's reply is not read back",
                }
                .into(),
                verification: GatewayVerification::None,
            }
        }
        "device.setInputPort"
        | "device.setOutputPort"
        | "device.setUsbPort"
        | "device.setMidiThru"
        | "device.setOutputPairing" => {
            let operation_name = match method {
                "device.setInputPort" => "setInputPort",
                "device.setOutputPort" => "setOutputPort",
                "device.setUsbPort" => "setUsbPort",
                "device.setMidiThru" => "setMidiThru",
                _ => "setOutputPairing",
            };
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(operation(operation_name, params)?),
                detail: "Global I/O setting sent to the Quad Cortex".into(),
                verification: GatewayVerification::None,
            }
        }
        "device.setGlobalEqBypassed"
        | "device.setGlobalEqBand"
        | "device.setGlobalEqOutput"
        | "device.loadGlobalEqModelPreset"
        | "device.setModeCycle" => {
            let operation_name = match method {
                "device.setGlobalEqBypassed" => "setGlobalEqBypassed",
                "device.setGlobalEqBand" => "setGlobalEqBand",
                "device.setGlobalEqOutput" => "setGlobalEqOutput",
                "device.loadGlobalEqModelPreset" => "loadGlobalEqModelPreset",
                _ => "setModeCycle",
            };
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(operation(operation_name, params)?),
                detail: if method == "device.setModeCycle" {
                    "Mode cycle sent to the Quad Cortex"
                } else if method == "device.loadGlobalEqModelPreset" {
                    "Global EQ preset recall sent to the Quad Cortex"
                } else {
                    "Global EQ setting sent to the Quad Cortex"
                }
                .into(),
                verification: GatewayVerification::None,
            }
        }
        "device.setTempoMetronome" => GatewayWritePlan {
            write: PlannedWrite::HidOperation(operation("setTempoMetronome", params)?),
            detail: "Tempo and metronome fields sent to the Quad Cortex, one global-tempo frame per field; a failure between frames leaves a partial state".into(),
            verification: GatewayVerification::None,
        },
        "device.setTempoMode" => GatewayWritePlan {
            write: PlannedWrite::HidOperation(operation("setTempoMode", params)?),
            detail: "Global tempo mode sent to the Quad Cortex".into(),
            verification: GatewayVerification::None,
        },
        "device.setGlobalTempo" => GatewayWritePlan {
            write: PlannedWrite::HidOperation(operation("setGlobalTempo", params)?),
            detail: format!(
                "Global tempo set to {} BPM",
                bounded_u32(params, "bpm", domain::MAXIMUM_TEMPO_BPM)?
            ),
            verification: GatewayVerification::None,
        },
        "device.setFavorite"
        | "device.setModelPinned"
        | "device.createSetlist"
        | "device.deleteSetlist"
        | "device.deletePreset" => {
            let operation_name = match method {
                "device.setFavorite" => "setFavorite",
                "device.setModelPinned" => "setModelPinned",
                "device.createSetlist" => "createSetlist",
                "device.deleteSetlist" => "deleteSetlist",
                "device.deletePreset" => "deletePreset",
                _ => "deletePreset",
            };
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(operation(operation_name, params)?),
                detail: format!("{operation_name} sent to the Quad Cortex"),
                verification: GatewayVerification::None,
            }
        }
        "device.loadCapture" | "device.loadIr" => {
            let operation_name = if method == "device.loadCapture" {
                let retarget = params
                    .get("expectedModelId")
                    .is_some_and(|value| !value.is_null());
                if retarget && state.is_none() {
                    return Err(
                        "Retargeting a Neural Capture requires the complete raw current model; use the stateful native planner"
                        .into(),
                    );
                }
                if !retarget && params.get("modelId").is_none_or(Value::is_null) {
                    return Err(
                        "modelId 14000 or 14001 is required when loading a Neural Capture into an empty target"
                            .into(),
                    );
                }
                "loadCapture"
            } else {
                if params.get("expectedModelId").is_none_or(Value::is_null) {
                    return Err(
                        "Retargeting an IR requires a non-null expectedModelId for the existing block; placement is a separate unsupported workflow"
                            .into(),
                    );
                }
                "loadIr"
            };
            let mut operation = operation(operation_name, params)?;
            if method == "device.loadCapture" {
                if let Some(expected_model_id) = params
                    .get("expectedModelId")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                {
                    let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
                    let column = bounded_u32(params, "column", domain::GRID_COLUMNS - 1)?;
                    let key = validate_library_key(required_text(params, "key")?, "key")?;
                    operation = state
                        .expect("retarget state checked above")
                        .capture_retarget_operation(row, column, expected_model_id, &key)?;
                }
            }
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: format!("{operation_name} sent to the Quad Cortex"),
            }
        }
        "device.selectModeSlot" => {
            let slot = bounded_u32(params, "slot", 2)?;
            let snapshot = snapshot.ok_or_else(|| {
                "Selecting a configured Mode Slot requires synchronized device state".to_string()
            })?;
            let mode = configured_mode_for_slot(snapshot, slot)?;
            GatewayWritePlan {
                write: PlannedWrite::HidCommand(DeviceCommand::SelectMode(mode)),
                detail: format!("Mode Slot {} selected", slot + 1),
                verification: GatewayVerification::Mode { mode },
            }
        }
        "device.selectScene" => {
            let scene = bounded_u32(params, "scene", domain::SCENE_COUNT - 1)?;
            GatewayWritePlan {
                write: PlannedWrite::HidCommand(DeviceCommand::SelectScene(scene)),
                detail: format!("Scene {} selected", (b'A' + scene as u8) as char),
                verification: GatewayVerification::Scene { scene },
            }
        }
        "device.toggleBypass" => {
            let bypassed = boolean(params, "desiredBypassed")?;
            GatewayWritePlan {
                write: PlannedWrite::HidCommand(DeviceCommand::SetBypass {
                    row: bounded_u32(params, "row", domain::GRID_ROWS - 1)?,
                    column: bounded_u32(params, "column", domain::GRID_COLUMNS - 1)?,
                    bypassed,
                }),
                detail: "Block bypass updated".into(),
                verification: GatewayVerification::Bypass {
                    row: bounded_u32(params, "row", domain::GRID_ROWS - 1)?,
                    column: bounded_u32(params, "column", domain::GRID_COLUMNS - 1)?,
                    bypassed,
                },
            }
        }
        "device.previewParameter" | "device.setParameter" => {
            let row = bounded_u32(params, "row", domain::GRID_ROWS - 1)?;
            let column = bounded_u32(params, "column", domain::GRID_COLUMNS + 1)?;
            let parameter_index = bounded_u32(params, "parameterIndex", u32::MAX)?;
            let numeric_value = params.get("value").and_then(Value::as_f64);
            let mut routing_model_id = None;
            let write = if column >= domain::GRID_COLUMNS {
                if row % 2 != 0 {
                    return Err("Splitter and mixer parameters exist only on rows 0 and 2".into());
                }
                if params.get("text").is_some() {
                    return Err(
                        "Splitter and mixer parameters use normalized numeric values".into(),
                    );
                }
                let node = if column == domain::GRID_COLUMNS {
                    let (node, model_id) = splitter_routing_identity(snapshot, row)?;
                    routing_model_id = Some(model_id);
                    node
                } else {
                    "mixer"
                };
                PlannedWrite::HidOperation(DeviceOperation::SetRoutingParameter {
                    row,
                    node: node.into(),
                    parameter_index,
                    value: normalized(params, "value")?,
                })
            } else if let Some(text) = params.get("text").and_then(Value::as_str) {
                PlannedWrite::HidCommand(DeviceCommand::SetParameterText {
                    row,
                    column,
                    parameter_index,
                    value: text.into(),
                })
            } else {
                PlannedWrite::HidCommand(DeviceCommand::SetParameterNumeric {
                    row,
                    column,
                    parameter_index,
                    value: normalized(params, "value")?,
                })
            };
            GatewayWritePlan {
                write,
                detail: if method == "device.previewParameter" {
                    "Parameter preview sent"
                } else {
                    "Parameter update sent to the Quad Cortex"
                }
                .into(),
                verification: if method == "device.previewParameter" {
                    GatewayVerification::None
                } else {
                    GatewayVerification::Parameter {
                        row,
                        column,
                        parameter_index,
                        value: numeric_value.unwrap_or_default(),
                        routing_model_id,
                    }
                },
            }
        }
        "device.previewLaneControlParameter" | "device.setLaneControlParameter" => {
            let operation = operation("setLaneControlParameter", params)?;
            let verification = if method == "device.previewLaneControlParameter" {
                GatewayVerification::None
            } else {
                verification_for_operation(&operation, params, snapshot)
            };
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(operation),
                detail: if method == "device.previewLaneControlParameter" {
                    "Lane control parameter preview sent"
                } else {
                    "Lane control parameter update sent to the Quad Cortex"
                }
                .into(),
                verification,
            }
        }
        "device.setLaneControlSceneMode" => {
            let operation = operation("setLaneControlSceneMode", params)?;
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: "Lane control scene behavior updated".into(),
            }
        }
        "device.setLaneControlExpression" => {
            let operation = operation("setLaneControlExpression", params)?;
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: "Lane control expression assignment updated".into(),
            }
        }
        "device.setTempo" => {
            let bpm = bounded_u32(params, "bpm", domain::MAXIMUM_TEMPO_BPM)?;
            if bpm < domain::MINIMUM_TEMPO_BPM {
                return Err(format!(
                    "bpm must be an integer from {} through {}",
                    domain::MINIMUM_TEMPO_BPM,
                    domain::MAXIMUM_TEMPO_BPM
                ));
            }
            GatewayWritePlan {
                write: PlannedWrite::HidCommand(DeviceCommand::SetTempo(bpm)),
                detail: format!("Tempo set to {bpm} BPM"),
                verification: GatewayVerification::Tempo { bpm },
            }
        }
        "device.setMasterVolume" => {
            let value = bounded_u32(params, "value", 100)?;
            GatewayWritePlan {
                write: PlannedWrite::HidCommand(DeviceCommand::SetMasterVolume(
                    value as f32 / 100.0,
                )),
                detail: format!("Master Volume set to {value}; awaiting device echo"),
                verification: GatewayVerification::MasterVolume { value },
            }
        }
        "device.setDeviceName" => {
            let name = validate_visible_text(required_text(params, "name")?, "Device name", 64)?;
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::SetDeviceName(name.clone())),
                detail: format!("Device name change to {name} sent"),
                verification: GatewayVerification::None,
            }
        }
        "device.undo" => GatewayWritePlan {
            write: PlannedWrite::HidOperation(DeviceOperation::Undo),
            detail: "Device undo sent".into(),
            verification: GatewayVerification::None,
        },
        "device.redo" => GatewayWritePlan {
            write: PlannedWrite::HidOperation(DeviceOperation::Redo),
            detail: "Device redo sent".into(),
            verification: GatewayVerification::None,
        },
        "device.tapScreen" => {
            let x = screen_coordinate(params, "x", 800.0)?;
            let y = screen_coordinate(params, "y", 480.0)?;
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::ScreenTap { x, y }),
                detail: format!("Tapped the Quad Cortex screen at ({x}, {y})"),
                verification: GatewayVerification::None,
            }
        }
        "device.longTapScreen" => {
            let x = screen_coordinate(params, "x", 800.0)?;
            let y = screen_coordinate(params, "y", 480.0)?;
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::ScreenHold { x, y }),
                detail: format!(
                    "Held the Quad Cortex screen at ({x}, {y}): a press, a {} ms wait (the host's own choice, not a measured threshold) and a release; the effect is not verified",
                    profile::REMOTE_HOLD_INTERVAL_MS
                ),
                verification: GatewayVerification::None,
            }
        }
        "device.mouseFrame" => {
            let kind = params
                .get("type")
                .and_then(Value::as_str)
                .and_then(crate::commands::ScreenMouseType::from_name)
                .ok_or_else(|| "type must be one of press, release, move, tap, drag".to_string())?;
            let x = screen_coordinate(params, "x", 800.0)?;
            let y = screen_coordinate(params, "y", 480.0)?;
            let destination_present = ["toX", "toY"]
                .iter()
                .any(|field| params.get(field).is_some_and(|value| !value.is_null()));
            let to = if kind == crate::commands::ScreenMouseType::Drag {
                let to_x = screen_coordinate(params, "toX", 800.0)?;
                let to_y = screen_coordinate(params, "toY", 480.0)?;
                if (x - to_x).abs() < f32::EPSILON && (y - to_y).abs() < f32::EPSILON {
                    return Err("A drag frame must end at a different pixel".into());
                }
                Some((to_x, to_y))
            } else if destination_present {
                return Err(format!(
                    "toX and toY are only valid for a drag frame, not {}",
                    kind.name()
                ));
            } else {
                None
            };
            let name = kind.name();
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::ScreenMouseFrame {
                    kind,
                    x,
                    y,
                    to,
                }),
                detail: match to {
                    Some((to_x, to_y)) => format!(
                        "Sent one raw {name} mouse frame from ({x}, {y}) to ({to_x}, {to_y}); the effect is not verified"
                    ),
                    None => format!(
                        "Sent one raw {name} mouse frame at ({x}, {y}); the effect is not verified"
                    ),
                },
                verification: GatewayVerification::None,
            }
        }
        "device.swipeScreen" => {
            let x = screen_coordinate(params, "x", 800.0)?;
            let y = screen_coordinate(params, "y", 480.0)?;
            let to_x = screen_coordinate(params, "toX", 800.0)?;
            let to_y = screen_coordinate(params, "toY", 480.0)?;
            if (x - to_x).abs() < f32::EPSILON && (y - to_y).abs() < f32::EPSILON {
                return Err("A screen swipe must end at a different pixel".into());
            }
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::ScreenDrag { x, y, to_x, to_y }),
                detail: format!(
                    "Swiped the Quad Cortex screen from ({x}, {y}) to ({to_x}, {to_y})"
                ),
                verification: GatewayVerification::None,
            }
        }
        "device.dragScreen" => {
            // `drag_screen` used to send a paced MOVE stream between a PRESS and a
            // RELEASE. MOVE had no observed effect in any encoding on CorOS 4.1.0
            // (2026-10-06, direct USB), so that stream did nothing the unit reads
            // and left a pressed pointer behind whenever it was cut short. The
            // unit's own drag is ONE `DRAG` frame, so this method now plans
            // exactly what `device.swipeScreen` plans. `steps` and `holdMs` are
            // still validated, so existing callers' bounds errors do not change,
            // but a DRAG frame has neither, so they are ignored.
            let x = screen_coordinate(params, "x", 800.0)?;
            let y = screen_coordinate(params, "y", 480.0)?;
            let to_x = screen_coordinate(params, "toX", 800.0)?;
            let to_y = screen_coordinate(params, "toY", 480.0)?;
            if (x - to_x).abs() < f32::EPSILON && (y - to_y).abs() < f32::EPSILON {
                return Err("A screen drag must end at a different pixel".into());
            }
            optional_bounded_u32(
                params,
                "steps",
                crate::commands::SCREEN_DRAG_DEFAULT_STEPS,
                crate::commands::SCREEN_DRAG_MIN_STEPS,
                crate::commands::SCREEN_DRAG_MAX_STEPS,
            )?;
            optional_bounded_u32(
                params,
                "holdMs",
                crate::commands::SCREEN_DRAG_DEFAULT_HOLD_MS,
                0,
                crate::commands::SCREEN_DRAG_MAX_HOLD_MS,
            )?;
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::ScreenDrag { x, y, to_x, to_y }),
                detail: format!(
                    "Dragged the Quad Cortex screen from ({x}, {y}) to ({to_x}, {to_y}) with one DRAG frame, the same frame swipe_screen sends; steps and holdMs are ignored because a DRAG frame carries neither; the effect is not verified"
                ),
                verification: GatewayVerification::None,
            }
        }
        "device.addBlock"
        | "device.replaceBlock"
        | "device.createModelPreset"
        | "device.setLooperStompAssignments"
        | "device.removeBlock"
        | "device.resetBlockParameters"
        | "device.moveBlock"
        | "device.setBlockFootswitch"
        | "device.setChainInput"
        | "device.setChainOutput"
        | "device.setChainSplit"
        | "device.setSplitMute"
        | "device.copyScene"
        | "device.setSceneLabel"
        | "device.setSceneColor" => {
            let operation_name = match method {
                "device.addBlock" => "addBlock",
                "device.replaceBlock" => "replaceBlock",
                "device.createModelPreset" => "createModelPreset",
                "device.setLooperStompAssignments" => "setLooperStompAssignments",
                "device.removeBlock" => "removeBlock",
                "device.resetBlockParameters" => "resetBlockParameters",
                "device.moveBlock" => "moveBlock",
                "device.setBlockFootswitch" => "setFootswitch",
                "device.setChainInput" => "setChainInput",
                "device.setChainOutput" => "setChainOutput",
                "device.setSplitMute" => "setSplitMute",
                "device.copyScene" => "copyScene",
                "device.setSceneLabel" => "setSceneLabel",
                "device.setSceneColor" => "setSceneColor",
                _ => "setChainSplit",
            };
            let operation = if method == "device.moveBlock" {
                exact_move_operation(params, state)?
            } else {
                operation(operation_name, params)?
            };
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: if method == "device.moveBlock" {
                    "Block moved".into()
                } else {
                    format!("{operation_name} sent to the Quad Cortex")
                },
            }
        }
        "device.setParameterSceneMode" | "device.setParameterExpression" => {
            let operation = operation(method, params)?;
            let (row, column) = match &operation {
                DeviceOperation::SetParameterSceneMode { row, column, .. }
                | DeviceOperation::SetParameterExpression { row, column, .. } => (*row, *column),
                _ => {
                    return Err(
                        "internal planning error: parameter assignment resolved to a different operation"
                            .into(),
                    )
                }
            };
            if column == domain::GRID_COLUMNS
                && splitter_routing_identity(snapshot, row)?.0 == "legacySplitter"
            {
                return Err(format!(
                    "{method} is unavailable for legacy splitter models 10000-10002: no legacy-container builder exists for this operation"
                ));
            }
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: if method == "device.setParameterSceneMode" {
                    "Parameter scene behavior sent to the Quad Cortex"
                } else {
                    "Parameter expression assignment sent to the Quad Cortex"
                }
                .into(),
            }
        }
        "device.setStompMomentary" => {
            let operation = operation(method, params)?;
            let DeviceOperation::SetStompMomentary { footswitch, .. } = operation else {
                unreachable!();
            };
            let snapshot = snapshot.ok_or_else(|| {
                "A synchronized preset is required to change stomp behavior".to_string()
            })?;
            let assigned = snapshot
                .blocks
                .iter()
                .filter(|block| block.footswitch == Some(footswitch))
                .count();
            if assigned != 1 {
                return Err(format!(
                    "Footswitch {} must drive exactly one block to change latching behavior; it currently drives {assigned}",
                    (b'A' + footswitch as u8) as char
                ));
            }
            let expected_momentary = boolean(params, "expectedMomentary")?;
            let actual_momentary = snapshot
                .footswitch_states
                .as_ref()
                .and_then(|states| states.iter().find(|state| state.index == footswitch))
                .and_then(|state| state.momentary)
                .ok_or_else(|| {
                    format!(
                        "Synchronized preset state did not contain latching behavior for footswitch {}",
                        (b'A' + footswitch as u8) as char
                    )
                })?;
            if actual_momentary != expected_momentary {
                return Err("write was based on stale stomp-momentary state".into());
            }
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, Some(snapshot)),
                write: PlannedWrite::HidOperation(operation),
                detail: "STOMP latching behavior sent to the Quad Cortex".into(),
            }
        }
        "device.setStompLabel" => {
            let footswitch = bounded_u32(params, "footswitch", domain::SCENE_COUNT - 1)?;
            let label = validate_stomp_label(required_text(params, "label")?)?;
            let assignment_count = snapshot
                .ok_or_else(|| {
                    "A synchronized preset is required to label a stomp switch".to_string()
                })?
                .blocks
                .iter()
                .filter(|block| block.footswitch == Some(footswitch))
                .count();
            if assignment_count == 0 {
                return Err("The selected footswitch has no STOMP assignment".into());
            }
            let single = assignment_count == 1;
            let operation = DeviceOperation::SetStompLabel {
                footswitch,
                label,
                single,
            };
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: "STOMP label sent to the Quad Cortex".into(),
            }
        }
        "device.setMidiOut" | "device.setPresetLoadMidiOut" | "device.setExpressionBypass" => {
            let operation = operation(method, params)?;
            if method == "device.setPresetLoadMidiOut" {
                let snapshot = snapshot.ok_or_else(|| {
                    "A synchronized preset is required to replace preset-load MIDI Out".to_string()
                })?;
                let expected = midi_out_messages(params, "expectedMessages")?;
                let actual = snapshot.preset_load_midi_out.as_ref().ok_or_else(|| {
                    "Synchronized preset state did not contain preset-load MIDI Out".to_string()
                })?;
                if canonical_midi_slots(actual) != canonical_midi_slots(&expected) {
                    return Err("write was based on stale preset-load MIDI Out state".into());
                }
            }
            GatewayWritePlan {
                verification: verification_for_operation(&operation, params, snapshot),
                write: PlannedWrite::HidOperation(operation),
                detail: match method {
                    "device.setMidiOut" => "Preset MIDI Out source sent to the Quad Cortex",
                    "device.setPresetLoadMidiOut" => "Preset-load MIDI Out sent to the Quad Cortex",
                    _ => "Expression bypass assignment sent to the Quad Cortex",
                }
                .into(),
            }
        }
        "device.showGigView" => {
            let shown = params
                .get("shown")
                .and_then(Value::as_bool)
                .ok_or_else(|| "shown must be a boolean".to_string())?;
            let command = DeviceCommand::ShowGigView(shown);
            GatewayWritePlan {
                write: PlannedWrite::HidCommand(command),
                detail: format!("{} {}", "Gig View", if shown { "opened" } else { "closed" }),
                verification: GatewayVerification::None,
            }
        }
        "device.showTuner" => {
            let shown = params
                .get("shown")
                .and_then(Value::as_bool)
                .ok_or_else(|| "shown must be a boolean".to_string())?;
            GatewayWritePlan {
                write: PlannedWrite::HidOperation(DeviceOperation::ShowTuner(shown)),
                detail: format!(
                    "Tuner visibility {} requested",
                    if shown { "open" } else { "close" }
                ),
                verification: GatewayVerification::None,
            }
        }
        "device.pressFootswitch" | "device.tapTempo" | "device.controlLooper" => {
            let midi = plan_host_midi(method, params)?;
            GatewayWritePlan {
                write: PlannedWrite::MidiControlChange {
                    controller: midi.controller,
                    value: midi.value,
                },
                detail: midi.detail,
                verification: GatewayVerification::None,
            }
        }
        _ => return Err(format!("Gateway write method is not supported: {method}")),
    };
    Ok(plan)
}

/// Grid-addressed writes must not use a cached block list after a sparse
/// structural echo invalidates topology. Retained block-detail reads have a
/// separate broker guard. Keep preset-level controls available while refusing
/// operations whose row/column or routing target may now refer to a different
/// block.
pub(crate) fn gateway_method_requires_exact_grid(method: &str) -> bool {
    matches!(
        method,
        "device.toggleBypass"
            | "device.setParameter"
            | "device.previewParameter"
            | "device.resetBlockParameters"
            | "device.setParameterSceneMode"
            | "device.setParameterExpression"
            | "device.setExpressionBypass"
            | "device.moveBlock"
            | "device.addBlock"
            | "device.replaceBlock"
            | "device.createModelPreset"
            | "device.removeBlock"
            | "device.loadCapture"
            | "device.loadIr"
            | "device.setBlockFootswitch"
            | "device.setChainInput"
            | "device.setChainOutput"
            | "device.setChainSplit"
            | "device.setSplitMute"
            | "device.setStompMomentary"
            | "device.setStompLabel"
            | "device.setLaneControlParameter"
            | "device.previewLaneControlParameter"
            | "device.setLaneControlSceneMode"
            | "device.setLaneControlExpression"
    )
}

/// The neutral note a verified `add_block` result carries when the plugin block it placed reads
/// back bypassed although no bypass was requested. It states only what was read back: QC Remote
/// has no evidence of plugin ownership, so it does not say whether the plugin is owned.
pub const PLUGIN_BLOCK_BYPASSED_NOTE: &str = "The Quad Cortex added this plugin block bypassed. QC Remote cannot read plugin ownership; if the plugin is not licensed on this unit, the device shows it locked.";

/// Reports, as fact, how a plugin block placed by a verified `device.addBlock` read back.
///
/// QC Remote does not check plugin ownership before placing a model, so it reports the device
/// response instead (in the tested unowned model the block appeared locked and bypassed). After
/// a verified add whose authoritative `snapshot`
/// shows the requested model in the requested cell flagged as a plugin, this inserts
/// `pluginBlockReadback` with the block's read-back `bypassed` state and `bypassRequested`
/// (always false: `add_block` takes no bypass argument). When the block reads back bypassed, the
/// result also carries [`PLUGIN_BLOCK_BYPASSED_NOTE`] in `pluginBlockReadback.note` and at the end
/// of `detail`. The outcome itself is never changed: nothing is refused or removed.
///
/// Every other write, an unverified result, a result without a snapshot, and a non-plugin block
/// are left untouched.
pub fn annotate_plugin_block_readback(write: &PlannedWrite, result: &mut Value) {
    let PlannedWrite::HidOperation(DeviceOperation::AddBlock {
        row,
        column,
        model_id,
    }) = write
    else {
        return;
    };
    if result.get("verified").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(readback) = result
        .get("snapshot")
        .and_then(|snapshot| plugin_block_readback_at(*row, *column, *model_id, snapshot))
    else {
        return;
    };
    let note = readback.note.clone();
    let Ok(readback) = serde_json::to_value(readback) else {
        return;
    };
    let Some(object) = result.as_object_mut() else {
        return;
    };
    if let Some(note) = note {
        let detail = object
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default();
        object.insert(
            "detail".into(),
            Value::String(detail_with_plugin_note(detail, &note)),
        );
    }
    object.insert("pluginBlockReadback".into(), readback);
}

/// `detail` with the plugin note appended as its own sentence.
pub fn detail_with_plugin_note(detail: &str, note: &str) -> String {
    if detail.is_empty() {
        note.to_string()
    } else if detail.ends_with('.') {
        format!("{detail} {note}")
    } else {
        format!("{detail}. {note}")
    }
}

/// How the block at `row`/`column` reads back in an authoritative `snapshot`, when it is the
/// requested `model_id` and a plugin model; `None` otherwise. `bypassRequested` is always false
/// because `add_block` takes no bypass argument, and the note is set only when the block reads
/// back bypassed.
fn plugin_block_readback_at(
    row: u32,
    column: u32,
    model_id: u32,
    snapshot: &Value,
) -> Option<crate::generated_payloads::PluginBlockReadback> {
    let block = snapshot
        .get("blocks")
        .and_then(Value::as_array)?
        .iter()
        .find(|block| {
            block.get("row").and_then(Value::as_u64) == Some(u64::from(row))
                && block.get("column").and_then(Value::as_u64) == Some(u64::from(column))
        })?;
    if block.get("modelId").and_then(Value::as_u64) != Some(u64::from(model_id))
        || block.get("plugin").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    let bypassed = block.get("bypassed").and_then(Value::as_bool);
    let bypass_requested = false;
    Some(crate::generated_payloads::PluginBlockReadback {
        plugin: true,
        bypassed,
        bypass_requested,
        note: (bypassed == Some(true) && !bypass_requested)
            .then(|| PLUGIN_BLOCK_BYPASSED_NOTE.to_string()),
    })
}

/// The `pluginBlockReadback` for a host that completes a write once its shared verification
/// reports it verified, and so has no result object to annotate (the Android adapter).
///
/// `snapshot` must be the authoritative state that verification just matched, read under the
/// same owner lock. Returns `None` for anything other than a `device.addBlock` whose verification
/// is the present-block predicate, and for a block that is not a plugin model.
pub fn verified_plugin_block_readback(
    method: &str,
    verification: &GatewayVerification,
    snapshot: &GatewaySnapshot,
) -> Option<crate::generated_payloads::PluginBlockReadback> {
    if method != "device.addBlock" {
        return None;
    }
    let GatewayVerification::Block {
        row,
        column,
        model_id: Some(model_id),
        present: true,
    } = verification
    else {
        return None;
    };
    let snapshot = serde_json::to_value(snapshot).ok()?;
    plugin_block_readback_at(*row, *column, *model_id, &snapshot)
}

#[cfg(test)]
#[path = "request_create_model_preset_probe_tests.rs"]
mod create_model_preset_probe_tests;

#[cfg(test)]
#[path = "request_create_model_preset_name_probe_tests.rs"]
mod create_model_preset_name_probe_tests;

#[cfg(test)]
#[path = "request_create_model_preset_controls_tests.rs"]
mod create_model_preset_controls_tests;

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "request_catalog_guard_tests.rs"]
mod catalog_guard_tests;
