//! Typed outbound Quad Cortex protocol messages shared by native hosts.
//!
//! Hosts own USB/MIDI handles and scheduling. This module owns message types,
//! protobuf shape, initialization order, and value normalization so Android and
//! Windows never hand-assemble the same wire command independently.

#[path = "screen.rs"]
mod screen;
#[cfg(test)]
use screen::screen_drag_release;
pub use screen::{
    screen_drag, screen_drag_duration_ms, screen_drag_end_frames, screen_drag_frame_count,
    screen_drag_hold_frames, screen_drag_steps, screen_hold, screen_mouse_frame, screen_tap,
    ScreenMouseType, SCREEN_DRAG_DEFAULT_HOLD_MS, SCREEN_DRAG_DEFAULT_STEPS,
    SCREEN_DRAG_END_HOLD_MS, SCREEN_DRAG_MAX_DURATION_MS, SCREEN_DRAG_MAX_HOLD_MS,
    SCREEN_DRAG_MAX_STEPS, SCREEN_DRAG_MIN_STEPS,
};

use crate::generated_payloads::MidiOutMessage;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::proto::{
    bypass, chain, col_bypass, model, param, param_value, BinaryPreset, Bypass, Chain, ColBypass,
    Model, Param, ParamValue, SceneBypass, SplitControlPoints, StompModeAssignment,
};
use crate::{domain, profile};
use prost::Message;
use thiserror::Error;

/// Error returned when a command or operation cannot be encoded for the device.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandEncodeError {
    /// A value is outside the range or set that the protocol or this API
    /// supports.
    #[error("unsupported {field}: {value}")]
    UnsupportedValue {
        /// Human-readable name of the offending field or selector.
        field: &'static str,
        /// The rejected value, formatted as text.
        value: String,
    },
    /// A value does not fit the signed 32-bit protobuf field it is encoded
    /// into.
    #[error("{field} must fit in a signed 32-bit protobuf field")]
    SignedFieldOverflow {
        /// Human-readable name of the field that overflowed.
        field: &'static str,
    },
}

/// Lowest accepted tuner reference-pitch offset, in Hz relative to A=440.
pub const MINIMUM_TUNER_REFERENCE_OFFSET_HZ: f32 = -40.0;
/// Highest accepted tuner reference-pitch offset, in Hz relative to A=440.
pub const MAXIMUM_TUNER_REFERENCE_OFFSET_HZ: f32 = 40.0;

/// Returns `true` when `value` is finite and lies within
/// [`MINIMUM_TUNER_REFERENCE_OFFSET_HZ`]..=[`MAXIMUM_TUNER_REFERENCE_OFFSET_HZ`].
pub fn is_tuner_reference_offset_hz(value: f32) -> bool {
    value.is_finite()
        && (MINIMUM_TUNER_REFERENCE_OFFSET_HZ..=MAXIMUM_TUNER_REFERENCE_OFFSET_HZ).contains(&value)
}

/// Number of message slots in each MIDI Out list (one list per source, plus the
/// preset-load list).
///
/// Encoders pad shorter lists with empty slots and the validating encoder
/// rejects longer ones.
pub const MIDI_OUT_SLOTS_PER_SOURCE: usize = 12;

/// MIDI Out message type `0`: the slot is empty.
pub const MIDI_OUT_TYPE_NONE: u32 = 0;
/// MIDI Out message type `1`: Control Change.
pub const MIDI_OUT_TYPE_CONTROL_CHANGE: u32 = 1;
/// MIDI Out message type `2`: Control Change Toggle.
pub const MIDI_OUT_TYPE_CONTROL_CHANGE_TOGGLE: u32 = 2;
/// MIDI Out message type `3`: Program Change.
pub const MIDI_OUT_TYPE_PROGRAM_CHANGE: u32 = 3;

/// The defined, non-empty MIDI Out message types. Together with
/// [`MIDI_OUT_TYPE_NONE`] this is the whole closed set `0..=3`; the wire field
/// is a bare integer, so this crate refuses to write anything else. See
/// [`set_midi_out_selector`] for what each type means.
pub const MIDI_OUT_TYPES: std::ops::RangeInclusive<u32> =
    MIDI_OUT_TYPE_CONTROL_CHANGE..=MIDI_OUT_TYPE_PROGRAM_CHANGE;

/// MIDI channels a MIDI Out message may address.
pub const MIDI_OUT_CHANNELS: std::ops::RangeInclusive<u32> = 1..=16;

/// Highest value of each of a MIDI Out message's three data parameters.
pub const MIDI_OUT_PARAMETER_MAX: u32 = 127;

/// MIDI Out sources: `0..=7` are footswitches A-H, `8` and `9` the two
/// expression pedal sources.
pub const MIDI_OUT_SOURCES: std::ops::RangeInclusive<u32> = 0..=9;

/// Device Settings `midiChannel` values: `1..=16` are the ordinary channels and
/// `17` selects OMNI.
pub const GENERAL_MIDI_CHANNELS: std::ops::RangeInclusive<i32> = 1..=17;

/// Device Settings `midiClockOut` values, the four members of its wire enum.
pub const GENERAL_MIDI_CLOCK_OUT: std::ops::RangeInclusive<i32> = 0..=3;

/// Preset instrument codes a caller may supply when saving a preset.
pub const PRESET_INSTRUMENTS: std::ops::RangeInclusive<i32> = 0..=5;

/// The two catalog model IDs that identify a Neural Capture block. Every
/// touch point that needs to know "is this model a Neural Capture" should
/// use `is_neural_capture_model` rather than re-deriving this pair inline;
/// it used to be independently spelled out with `matches!(id, 14_000 |
/// 14_001)` at 6+ sites across this file and `device/request.rs`.
pub(crate) const NEURAL_CAPTURE_MODEL_IDS: [u32; 2] = [14_000, 14_001];

pub(crate) fn is_neural_capture_model(model_id: u32) -> bool {
    NEURAL_CAPTURE_MODEL_IDS.contains(&model_id)
}

/// Every output route the device itself reports for a chain row
/// (`domain::OUTPUT_ROUTES`, the device's `OutputPortId` enum; see that table
/// for the label of each id, and `domain::INPUT_ROUTES` for input routes). Among
/// them, ids `4..=7` are the four individual main outputs, `8` and `9` the two
/// sends, and `20` a USB output.
///
/// Routes outside the commonly used ones, such as ids 0, 16 and 19, have been
/// observed on a unit, since a preset can load onto them; a route the device
/// reports must be writable back, or an edit made through this API cannot be
/// undone through it (live test, app 0.1.174: 19 -> 20 succeeded, 20 -> 19 was
/// refused).
pub fn is_chain_output_port_id(value: u32) -> bool {
    domain::OUTPUT_ROUTES.iter().any(|(id, _)| *id == value)
}

/// Whether a message may be written. Types `1..=3` are active messages and need
/// a channel of `1..=16`; type `0` is an empty slot that is sent as the empty
/// message whatever its other fields hold, so it takes a channel of `0..=16`.
/// Every parameter is bounded either way, and any other type is refused: an
/// undefined type was once followed by the unit restarting.
fn midi_out_message_is_valid(message: &MidiOutMessage) -> bool {
    if message.r#type == MIDI_OUT_TYPE_NONE {
        message.channel <= *MIDI_OUT_CHANNELS.end()
            && [message.param1, message.param2, message.param3]
                .iter()
                .all(|param| *param <= MIDI_OUT_PARAMETER_MAX)
    } else {
        midi_out_fields_are_valid(
            message.r#type,
            message.channel,
            [message.param1, message.param2, message.param3],
        )
    }
}

/// `true` when the fields form a defined, non-empty MIDI Out message: a type in
/// [`MIDI_OUT_TYPES`], a channel in [`MIDI_OUT_CHANNELS`] and every parameter
/// at most [`MIDI_OUT_PARAMETER_MAX`]. Shared by the encoder and the state
/// reader.
pub(crate) fn midi_out_fields_are_valid(r#type: u32, channel: u32, params: [u32; 3]) -> bool {
    MIDI_OUT_TYPES.contains(&r#type)
        && MIDI_OUT_CHANNELS.contains(&channel)
        && params.iter().all(|param| *param <= MIDI_OUT_PARAMETER_MAX)
}

/// One encoded outbound protocol message: a message type and its serialized
/// protobuf payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    /// Protocol message type number, one of the `profile::MESSAGE_TYPE_*` values.
    pub message_type: u16,
    /// Serialized protobuf payload for `message_type`.
    pub payload: Vec<u8>,
}

/// Exact protobuf target selected by the expression-assignment builders.
///
/// This stays internal: the public action accepts only block columns plus the
/// splitter/mixer pseudo-columns, even though the underlying converter has
/// additional model-hash branches for the lane controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpressionTarget {
    Model { column: u32 },
    OutputControl,
    InputControl,
    CombinedSplitter,
    Mixer,
}

/// Exact protobuf target selected by the bypass-assignment builders.
///
/// Bypass routing is intentionally distinct from parameter-expression routing:
/// both routing hashes converge on `Chain.mixer`, and the input-control
/// bypass assignment frame carries a fixed column value of `1` that its info
/// frame omits (the reason is unknown). The input control is the row's input
/// gate control (model `28000`), the noise-gate and input-gain control attached
/// to the input block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpressionBypassTarget {
    Model { column: u32 },
    OutputControl,
    InputControl,
    Routing,
}

/// Explicit type carried by Cortex stomp-assignment updates and removals.
///
/// The two names and their wire values (`PRIMARY` = 0, `SECONDARY` = 1) come
/// from the protocol schema. This crate does not interpret the difference
/// between the two values; it only carries the one the caller chooses.
///
/// A secondary assignment is believed to bind a footswitch to a model's second
/// stomp control (for example a momentary trigger), where a primary assignment
/// binds it to the block's bypass; only blocks whose model defines such a
/// control are expected to have one. That has not been verified on the unit
/// used for this project's tests.
///
/// Only primary assignments have been written to hardware in this project's
/// runs, none of its recorded hardware output shows a unit carrying a
/// secondary one, and the unit's own "Assign footswitch" dialog offers only a
/// latching/momentary choice and an unassign button, with no primary/secondary
/// choice (observed on CorOS 4.1.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StompAssignmentType {
    /// Primary assignment (wire value 0), believed to be the block's bypass.
    Primary,
    /// Secondary assignment (wire value 1), believed to bind a model's second
    /// stomp control. This crate does not interpret how it differs from
    /// [`StompAssignmentType::Primary`].
    Secondary,
}

impl StompAssignmentType {
    fn wire(self) -> i32 {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
        }
    }
}

impl OutboundMessage {
    fn encoded<M: Message>(message_type: u16, message: M) -> Self {
        Self {
            message_type,
            payload: message.encode_to_vec(),
        }
    }

    fn action(message_type: u16, action: pa::message_action::Enum) -> Self {
        Self {
            message_type,
            payload: vec![0x08, action as u8],
        }
    }
}

/// Intent-level live device operations supported by every native host.
/// Platform adapters parse host arguments and schedule USB writes; this enum
/// keeps operation-to-wire selection in the protocol crate.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceCommand {
    /// Select a scene by index (`0..=7`).
    SelectScene(u32),
    /// Select a performance mode by its enum value: 0 is PRESET, 1 is SCENE and
    /// 2 is STOMP. Only these three values are encoded; any other value is
    /// rejected before a frame is built. 3 through 8 are believed to be the
    /// hybrid pairings PRESET/SCENE, PRESET/STOMP, SCENE/PRESET, SCENE/STOMP,
    /// STOMP/PRESET and STOMP/SCENE, in that order, but the wire value for
    /// hybrid modes has not been measured, so they are refused too.
    ///
    /// Observed on a CorOS 4.1.0 unit, 2026-10-07, with its available modes
    /// listed as `[1, 0, 2]` (cycle order Scene, Preset, Stomp): writing `0`, `1`
    /// and `2` showed PRESET, SCENE and STOMP on the screen, so the field is the
    /// mode enum value and not an index into the cycle, and the available-modes
    /// list carries enum values in cycle order. The hybrid values `3` to `8` have
    /// not been written, so their order is not verified. An earlier report that
    /// writing mode `0` was ignored did not reproduce.
    ///
    /// For a hybrid written `A/B` in that list, the first mode `A` is believed
    /// to apply to footswitches A-D and the second mode `B` to footswitches
    /// E-H.
    SelectMode(u32),
    /// Set whether the block at a grid position is bypassed.
    SetBypass {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// `true` to bypass the block, `false` to engage it.
        bypassed: bool,
    },
    /// Set one block parameter to a normalized numeric value.
    ///
    /// Splitter and mixer columns are not addressable here; use
    /// [`DeviceOperation::SetRoutingParameter`] for those.
    SetParameterNumeric {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Index of the parameter within the block's parameter list.
        parameter_index: u32,
        /// Normalized value, `0.0..=1.0`. Non-finite or out-of-range values are
        /// rejected when encoding.
        value: f32,
    },
    /// Set one block parameter to a string value, such as a file key.
    SetParameterText {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Index of the parameter within the block's parameter list.
        parameter_index: u32,
        /// String value to write.
        value: String,
    },
    /// Show (`true`) or hide (`false`) the tuner. The show message itself does
    /// nothing on a CorOS 4.1.0 unit (observed 2026-10-07).
    ShowTuner(bool),
    /// Show (`true`) or hide (`false`) Gig View.
    ShowGigView(bool),
    /// Recall the preset stored at a setlist position.
    SetlistPosition {
        /// Key of the setlist folder, as reported by the device's folder listing.
        setlist_key: String,
        /// Slot position within the setlist (`0..=255`).
        position: u32,
        /// Whether the setlist is a factory setlist rather than a user setlist.
        is_factory: bool,
    },
    /// Set the loaded preset's tempo in beats per minute (`40..=240`).
    SetTempo(u32),
    /// Set the master volume as a normalized value (`0.0..=1.0`).
    SetMasterVolume(f32),
    /// Tell the device the host is disconnecting.
    Disconnect,
}

/// Higher-level mutations shared by every native host. Operations may expand
/// to several ordered messages.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceOperation {
    /// Perform a single [`DeviceCommand`].
    Command(DeviceCommand),
    /// Place a block with the given ordinary model at a grid position.
    ///
    /// Model ID `0` and the Neural Capture models are rejected when encoding;
    /// use [`DeviceOperation::LoadCapture`] for captures.
    AddBlock {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Catalog model ID of the block to place.
        model_id: u32,
    },
    /// Remove the block at a grid position.
    RemoveBlock {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
    },
    /// Reset every parameter of the block at a grid position to the defaults
    /// stored on the device.
    ResetBlockParameters {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
    },
    /// Replace the block at a grid position with a block of another ordinary
    /// model. The same model-ID restrictions as
    /// [`DeviceOperation::AddBlock`] apply. It sends the same single Grid
    /// UPDATE frame as `AddBlock`; what the unit does with that frame on an
    /// occupied cell is not established (hardware verification pending).
    ReplaceBlock {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Catalog model ID of the replacement block.
        model_id: u32,
    },
    /// Request creation of a user model preset from the block's settings with one
    /// type-71 CREATE frame. Nothing is read back, so creation and contents are
    /// not verified.
    CreateModelPreset {
        /// Zero-based grid row (`0..=3`) of the source block.
        row: u32,
        /// Zero-based grid column (`0..=7`) of the source block.
        column: u32,
        /// Model hash (catalog model ID) of the source block. Must be non-zero.
        model_hash: u32,
        /// Name of the new model preset: non-empty, at most 80 characters, with no
        /// leading or trailing whitespace and no control characters.
        name: String,
    },
    /// Replace all eight Looper X footswitch assignments, in physical A-H order.
    ///
    /// `assignments[i]` is the raw action index on footswitch `A + i`, sent
    /// unchanged: eight integers that together form a permutation of `0..=7`.
    /// The arrangement read back from the unit in this project's runs was
    /// `[3, 4, 5, 6, 0, 1, 7, 2]`.
    ///
    /// What each index means was observed on a CorOS 4.1.0 unit, 2026-10-06:
    /// writing `[0, 1, 2, 3, 4, 5, 6, 7]` made the unit's Looper X editor show
    /// footswitches A to H as Record, Play, Undo, Duplicate, One Shot, Half
    /// Speed, Punch In and Reverse. So `0` is Record, `1` Play, `2` Undo, `3`
    /// Duplicate, `4` One Shot, `5` Half Speed, `6` Punch In and `7` Reverse; the
    /// arrangement read back above shows A to H as Duplicate, One Shot, Half
    /// Speed, Punch In, Record, Play, Reverse and Undo. The unit's labels for
    /// the secondary states (Overdub, Redo, Re-Loop, Punch Out) are believed,
    /// not observed, to be what the same controls show while a loop is playing,
    /// after an undo, while Duplicate is active and while punched in.
    SetLooperStompAssignments {
        /// Eight values that together form a permutation of `0..=7`.
        assignments: Vec<i32>,
    },
    /// Move a block from one grid cell to another.
    MoveBlock {
        /// Zero-based source grid row (`0..=3`).
        from_row: u32,
        /// Zero-based source grid column (`0..=7`).
        from_column: u32,
        /// Zero-based destination grid row (`0..=3`).
        to_row: u32,
        /// Zero-based destination grid column (`0..=7`).
        to_column: u32,
        /// Model IDs of the grid as it currently stands, one inner vector per row.
        /// Must not be empty. Validated but not transmitted: the move frame
        /// carries no grid sidecar.
        grid_model_ids: Vec<Vec<u32>>,
    },
    /// Assign a block to a footswitch, or clear its assignment.
    ///
    /// When a previous assignment is supplied it is deleted first. No message is
    /// produced when the requested assignment equals the previous one.
    SetFootswitch {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Footswitch index (`0..=7`, A-H) to assign, or `None` to clear.
        footswitch: Option<u32>,
        /// Type of the new assignment.
        assignment_type: StompAssignmentType,
        /// Footswitch index (`0..=7`) of the block's existing assignment, which is
        /// deleted first. Must be `Some` exactly when `previous_assignment_type` is.
        previous_footswitch: Option<u32>,
        /// Type of the block's existing assignment. Must be `Some` exactly when
        /// `previous_footswitch` is.
        previous_assignment_type: Option<StompAssignmentType>,
    },
    /// Set the input route of a chain row.
    SetChainInput {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Input route ID from the device's input-route table, such as `1` for
        /// In 1 or `0` for not in use.
        input_id: u32,
    },
    /// Set the output route of a chain row.
    SetChainOutput {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Output route ID from the device's output-route table, such as `1` for
        /// Out 1/2 or `19` for Multi Out. Ids `4..=7` are the four individual
        /// main outputs, `8` and `9` the two sends, and `20` a USB output; see
        /// [`is_chain_output_port_id`].
        output_id: u32,
    },
    /// Set the parallel-routing split and mix columns of a chain row.
    ///
    /// Only rows `0` and `2` are accepted.
    SetChainSplit {
        /// Zero-based grid row: `0` or `2`.
        row: u32,
        /// Grid column (`0..=7`) of the split point, or `None` for no split.
        split_column: Option<i32>,
        /// Grid column (`0..=7`) of the mix point, or `None` for none. When both are
        /// set it must be greater than `split_column`.
        mix_column: Option<i32>,
    },
    /// Mute or unmute the split/mixer output of a chain for the selected scene.
    SetSplitMute {
        /// Zero-based grid row of the splitter/mixer; must be even (`0` or `2`).
        row: u32,
        /// `true` to mute, `false` to unmute.
        muted: bool,
    },
    /// Set a numeric parameter of a splitter or mixer node.
    SetRoutingParameter {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Routing node: `splitter`, `legacySplitter` or `mixer`.
        node: String,
        /// Index of the parameter within the node's parameter list.
        parameter_index: u32,
        /// Normalized value, `0.0..=1.0`.
        value: f32,
    },
    /// Set a numeric parameter of a lane's input gate or lane output control.
    SetLaneControlParameter {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Lane control: `inputGate` or `laneOutput`.
        control: String,
        /// Index of the parameter within the control's parameter list.
        parameter_index: u32,
        /// Normalized value, `0.0..=1.0`.
        value: f32,
    },
    /// Enable or disable per-scene storage for a lane-control parameter.
    SetLaneControlSceneMode {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Lane control: `inputGate` or `laneOutput`.
        control: String,
        /// Index of the parameter within the control's parameter list.
        parameter_index: u32,
        /// `true` to store the parameter per scene.
        enabled: bool,
    },
    /// Assign an expression pedal to a lane-control parameter.
    SetLaneControlExpression {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Lane control: `inputGate` or `laneOutput`.
        control: String,
        /// Index of the parameter within the control's parameter list.
        parameter_index: u32,
        /// Expression pedal selector (`0..=2`): `0` clears the assignment, `1`
        /// and `2` select the unit's EXP 1 and EXP 2 inputs.
        pedal: u32,
        /// Parameter value at the heel position, normalized `0.0..=1.0`.
        minimum: f32,
        /// Parameter value at the toe position, normalized `0.0..=1.0`.
        maximum: f32,
    },
    /// Request the device's folder (setlist) catalog.
    ListPresetFolders,
    /// Request the full current preset with the bare type-15 READ (`08 03`, no request id)
    /// Cortex Control sends at boot.
    ReadCurrentPreset,
    /// Save the current preset into a setlist slot.
    SavePreset {
        /// Key of the destination setlist folder.
        setlist_key: String,
        /// Slot position within the setlist; must fit in a signed 32-bit field.
        position: u32,
        /// Name to store the preset under.
        name: String,
        /// Instrument code stored with the preset, passed through unchanged.
        /// `0` is untagged, `1` guitar and `2` bass; `3`, `4` and `5` are
        /// believed to be Synth, Vocal and Other. The names for `3` to `5` have
        /// not been verified against the unit used for this project's tests.
        instrument: i32,
    },
    /// Rename a stored preset without re-saving or recalling it.
    RenamePreset {
        /// Key of the setlist folder containing the preset.
        setlist_key: String,
        /// Opaque key identifying the stored preset within its folder.
        product_key: String,
        /// New preset name.
        name: String,
        /// Instrument code of the preset, passed through unchanged. The values
        /// are the same as for [`DeviceOperation::SavePreset`]: `0` untagged,
        /// `1` guitar, `2` bass, and believed `3` Synth, `4` Vocal, `5` Other.
        instrument: i32,
    },
    /// Request the device's version information.
    ReadVersion,
    /// Request the current tuner state.
    ReadTuner,
    /// Show or hide the tuner, enabling the live meter while it is shown, and
    /// read tuner state afterwards (three ordered messages).
    ShowTuner(bool),
    /// Select the tuner input port (`1`, `2`, `3`, `4`, `5`, `8` or `9`).
    SetTunerInput(i32),
    /// Set the persistent mute-while-tuning preference.
    SetTunerMute(bool),
    /// Turn the tuner's live meter updates on or off.
    SetTunerMeter(bool),
    /// Set the tuner reference pitch as a signed offset in Hz from A=440
    /// (`-40.0..=40.0`).
    SetTunerReference(f32),
    /// Request the device's general settings.
    ReadGeneralSettings,
    /// Write one integer-valued general (device) setting.
    SetGeneralInteger {
        /// Setting name; see [`is_general_integer_value`] for the accepted names.
        setting: String,
        /// Value in the setting's public domain (see [`is_general_integer_value`]).
        value: i32,
    },
    /// Write one boolean general (device) setting.
    SetGeneralToggle {
        /// Setting name: `midiOverUsb`, `lockScreenAndVolumeKnob`,
        /// `ignoreDuplicatePc`, `stompModeAutoAssign`, `swapTempoTunerAccess`,
        /// `disableInternetConnectionCheck`, `dynamicDelayCompensation`,
        /// `presetDimmed`, `sceneDimmed`, `stompDimmed`, `midiClockIn` or
        /// `gigViewStompAccess`.
        setting: String,
        /// `true` to turn the setting on.
        enabled: bool,
    },
    /// Set the scene block-bypass behaviour setting (`0..=2`), which controls
    /// whether block bypass changes are saved into the scene: `0` always saves
    /// them (the default), `1` does not save changes made with a Stomp
    /// footswitch but does save touchscreen changes, and `2` never saves them.
    SetSceneBypassBehavior(i32),
    /// Choose which outputs the master volume assignment covers.
    SetMasterVolumeAssignment {
        /// Include Out 1/2.
        out12: bool,
        /// Include Out 3/4.
        out34: bool,
        /// Include Send 1/2.
        send12: bool,
        /// Include the headphone output.
        headphones: bool,
    },
    /// Set the global cab and/or IR bypass flags for each chain row. Each
    /// supplied family is one complete type-9 frame; a family left `None` is
    /// not sent. At least one family is required.
    SetGlobalBypass {
        /// Global cab bypass flags, one per chain row in row order, or `None`
        /// to send no Cab frame.
        cab: Option<[bool; 4]>,
        /// Global IR bypass flags, one per chain row in row order, or `None`
        /// to send no IR frame.
        ir: Option<[bool; 4]>,
    },
    /// Request the device's input/output settings.
    ReadIoSettings,
    /// Request the global EQ state.
    ReadGlobalEq,
    /// Bypass (`true`) or engage (`false`) the global EQ.
    SetGlobalEqBypassed(bool),
    /// Set global EQ parameters, given as `(parameter index, value)` pairs.
    /// One message is sent per pair.
    SetGlobalEqParameters(Vec<(i32, f32)>),
    /// Load a model preset into the global EQ.
    LoadGlobalEqModelPreset {
        /// Identifier (name) of the model preset.
        value: String,
        /// Whether the model preset is a factory preset.
        is_factory: bool,
        /// Hash of the model the preset belongs to.
        hash: u32,
    },
    /// Request the performance modes currently available for cycling.
    ReadModeCycle,
    /// Set the performance modes that are available for cycling: one to three
    /// distinct mode values, each `0..=8`, in cycle order and numbered as for
    /// [`DeviceCommand::SelectMode`] (including the hybrid pairings). A value above
    /// 8 is rejected: an undefined mode value is reported to be accepted by the
    /// unit and then to leave it unusable, so the closed set is enforced.
    SetModeCycle(Vec<u32>),
    /// Request the device-global tempo block.
    ReadGlobalTempo,
    /// Set the device-global tempo in beats per minute (`40..=240`).
    ///
    /// In GLOBAL mode this changes the global tempo; in PRESET mode the observed
    /// behavior changes the loaded preset tempo and marks it dirty. See [`set_global_tempo`].
    SetGlobalTempo(u32),
    /// Set tempo/metronome parameters, given as `(parameter index, normalized
    /// value)` pairs, one message per pair.
    ///
    /// The list must be non-empty, must not repeat an index, and each pair must
    /// satisfy [`is_tempo_metronome_parameter_value`].
    SetTempoParameters(Vec<(u32, f32)>),
    /// Select the device-global tempo (`true`) or the loaded preset's tempo
    /// (`false`).
    SetTempoMode(bool),
    /// Request the looper status.
    ReadLooperStatus,
    /// Request the recents or favorites list.
    ReadRecentsFavorites {
        /// `true` for favorites, `false` for recents.
        favorites: bool,
        /// Host-chosen correlation id carried in the request.
        request_id: u64,
    },
    /// Add a preset to, or remove it from, the favorites list.
    SetFavorite {
        /// Display name of the preset.
        name: String,
        /// Key of the folder containing the preset.
        folder_key: String,
        /// Key identifying the preset within its folder.
        product_key: String,
        /// Whether the preset is a factory preset.
        is_factory: bool,
        /// Value of the list item's plugin flag.
        is_plugin: bool,
        /// `true` to add the favorite, `false` to remove it.
        favorite: bool,
    },
    /// Request the list of pinned models.
    ReadPinnedModels,
    /// Pin or unpin a model.
    SetModelPinned {
        /// Catalog model ID.
        model_id: u32,
        /// `true` to pin the model, `false` to unpin it.
        pinned: bool,
    },
    /// Read the device file catalog.
    ///
    /// With neither `file_type` nor `request_id` this is a plain folder-catalog
    /// read; otherwise it is a typed, correlated read.
    ReadLibraryFiles {
        /// Folder of interest. The encoder does not currently place it on the wire.
        folder_key: String,
        /// Optional file type to query, as the protocol's file type value.
        file_type: Option<i32>,
        /// Optional host-chosen correlation id.
        request_id: Option<u64>,
    },
    /// Create a user setlist.
    CreateSetlist {
        /// Name of the new setlist.
        name: String,
    },
    /// Delete a user setlist by name.
    DeleteSetlist {
        /// Name of the setlist to delete: 1 to 64 characters, with no surrounding
        /// whitespace, control characters or path separators, and not `.`, `..` or
        /// `My Presets`.
        name: String,
    },
    /// Delete a stored preset from a user setlist.
    DeletePreset {
        /// Key of the setlist folder containing the preset.
        setlist_key: String,
        /// Key identifying the preset within its folder.
        product_key: String,
    },
    /// Move a stored user preset into an empty slot of the same setlist.
    MovePreset {
        /// Key of the setlist folder.
        setlist_key: String,
        /// Key identifying the preset to move.
        product_key: String,
        /// Destination slot position; must fit in a signed 32-bit field.
        position: u32,
    },
    /// Swap a stored user preset with the preset in an occupied slot of the same
    /// setlist.
    SwapPreset {
        /// Key of the setlist folder.
        setlist_key: String,
        /// Key identifying the preset to swap.
        product_key: String,
        /// Slot position of the other preset; must fit in a signed 32-bit field.
        position: u32,
    },
    /// Copy a stored preset to a slot of a setlist without recalling it.
    CopyPreset {
        /// Key of the setlist folder containing the source preset.
        source_setlist_key: String,
        /// Key identifying the source preset within its folder.
        source_product_key: String,
        /// Whether the source setlist is a factory setlist.
        source_is_factory: bool,
        /// Key of the destination setlist folder.
        destination_setlist_key: String,
        /// Destination slot position; must fit in a signed 32-bit field.
        destination_position: u32,
    },
    /// Duplicate a whole setlist. The device chooses the new name and key.
    DuplicateSetlist {
        /// Key of the setlist folder to duplicate.
        source_setlist_key: String,
    },
    /// Place a Neural Capture block whose capture is selected by `key`.
    LoadCapture {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Key of the capture file to load.
        key: String,
        /// Neural Capture model ID (`14000` or `14001`). `None` is rejected when
        /// encoding.
        model_id: Option<u32>,
    },
    /// Full-model Neural Capture retarget update. This is built only
    /// from an atomically cloned `StateDecoder` preset; public callers cannot
    /// manufacture the raw model/bypass state through JSON arguments.
    RetargetCapture {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`) of the capture block.
        column: u32,
        /// Index of the active scene; selects which per-scene value of the capture
        /// parameter is compared against `key` when that parameter is stored per
        /// scene.
        active_scene: u32,
        /// Capture key that the model's capture parameter must hold.
        key: String,
        /// State revision the update was built from. It is not sent to the device.
        source_revision: u64,
        /// Complete Neural Capture model to write. It must sit on `column` and carry
        /// exactly one parameter 5 holding `key`.
        model: Model,
        /// Optional bypass state for the same column, written together with the model.
        bypass: Option<ColBypass>,
    },
    /// Load an impulse response into a block's IR slot.
    LoadIr {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Key of the IR file to load.
        key: String,
        /// IR slot of the block: `0` or `1`.
        slot: u32,
    },
    /// Change settings of a physical input port. Only the fields that are `Some`
    /// are written, one message each.
    SetInputPort {
        /// Physical input port ID: `1`, `2`, `4` or `5`.
        input_port_id: u32,
        /// Already normalized to `0.0..=1.0` by the caller's argument
        /// conversion; see `commands::input_level_db_to_wire`.
        level: Option<f32>,
        /// Normalized impedance choice: one of nine enum ordinals divided by eight
        /// (`0.0`, `0.125`, ..., `1.0`).
        impedance: Option<f32>,
        /// Normalized input type: `0.0` instrument, `0.5` microphone, `1.0`
        /// microphone with phantom power.
        input_type: Option<f32>,
        /// Normalized ground-lift state: `0.0` or `1.0`.
        ground_lift: Option<f32>,
    },
    /// Change settings of a physical output port. Only the fields that are `Some`
    /// are written, one message each.
    SetOutputPort {
        /// Output port ID: `1`, `2` or `4..=9`.
        output_port_id: u32,
        /// Decibels, -40 through +12. `set_output_port` normalizes it onto the
        /// wire field; nothing upstream may do so twice.
        level: Option<f32>,
        /// Ground-lift value, written to the wire as given.
        ground_lift: Option<f32>,
        /// Mute state of the port.
        mute: Option<bool>,
    },
    /// Change settings of the USB port. Only the fields that are `Some` are
    /// written, one message each.
    SetUsbPort {
        /// USB level, written to the wire as given.
        level: Option<f32>,
        /// Normalized headphone source ordinal: `0.0` none, `1/3` USB 1/2, `2/3`
        /// USB 3/4, `1.0` both.
        headphones_source: Option<f32>,
        /// Normalized USB dry/wet routing choice: `0.0` or `1.0`.
        dry_wet: Option<f32>,
    },
    /// Enable or disable MIDI thru. On the unit, enabling MIDI Thru disables
    /// Preset MIDI Out.
    SetMidiThru(bool),
    /// Link or unlink output pairs. Only the fields that are `Some` are written.
    SetOutputPairing {
        /// Whether the XLR 1/2 outputs are linked.
        xlr12_linked: Option<bool>,
        /// Whether outputs 3/4 are linked.
        out34_linked: Option<bool>,
    },
    /// Set the device's custom name.
    SetDeviceName(String),
    /// Undo the last edit on the device.
    Undo,
    /// Redo the last undone edit on the device.
    Redo,
    /// Request the modules the device's compiler has inhibited.
    ReadInhibitedModules,
    /// Request a screenshot of a stored preset.
    PresetScreenshot {
        /// Name of the folder (setlist) containing the preset.
        folder_name: String,
        /// Slot position within the folder (`0..=255`).
        position: u32,
        /// Whether the folder is a factory folder.
        is_factory: bool,
        /// Host-chosen correlation id carried in the request.
        request_id: u64,
    },
    /// Request a screenshot of the device's display.
    CaptureScreen,
    /// Request a diagnostics snapshot. The request is read-only.
    ReadDiagnostics {
        /// Host-chosen correlation id carried in the request.
        request_id: u64,
    },
    /// Request the structure of what is currently on the device's screen, as
    /// indented text. See [`read_graphics_tree`].
    ReadGraphicsTree,
    /// Tap the device touchscreen at a point: the protocol's own one-frame TAP
    /// (mouse type 3). See [`screen_tap`].
    ScreenTap {
        /// Horizontal position in screen pixels: a whole number in `0..800`.
        x: f32,
        /// Vertical position in screen pixels: a whole number in `0..480`.
        y: f32,
    },
    /// A press and hold at one point: a touch-down frame (`PRESS`), a wait of
    /// `profile::REMOTE_HOLD_INTERVAL_MS`, then a touch-up frame (`RELEASE`). The
    /// protocol has no long-press frame and the device has no hold duration on
    /// the wire, so the wait between the two frames is the whole gesture. What a
    /// hold does is context dependent and is documented on [`screen_hold`], with
    /// which parts are measured and which are a host choice. See also
    /// `operation_inter_message_interval_ms`, and [`Self::abort_release_frame`]
    /// for the one touch-up a host sends if the gesture fails after the
    /// touch-down went out.
    ScreenHold {
        /// Horizontal position in screen pixels: a whole number in `0..800`.
        x: f32,
        /// Vertical position in screen pixels: a whole number in `0..480`.
        y: f32,
    },
    /// Swipe on the device touchscreen from one point to another: a scroll
    /// fling sent as one DRAG frame, with no touch-down or touch-up frame. See
    /// [`screen_drag`] for the frame and for what was observed on CorOS 4.1.0.
    ScreenDrag {
        /// Starting horizontal position in screen pixels: a whole number in `0..800`.
        x: f32,
        /// Starting vertical position in screen pixels: a whole number in `0..480`.
        y: f32,
        /// Ending horizontal position in screen pixels: a whole number in `0..800`.
        to_x: f32,
        /// Ending vertical position in screen pixels: a whole number in `0..480`.
        to_y: f32,
    },
    /// A paced stream of MOVE frames between a touch-down (`PRESS`) and a
    /// touch-up (`RELEASE`): a dwell at the start point, `steps` interpolated
    /// moves, a dwell at the end point, all at `profile::REMOTE_DRAG_INTERVAL_MS`.
    ///
    /// **Inert, and not reachable from the gateway.** `MOVE` had no observed
    /// effect in any encoding on CorOS 4.1.0 (2026-10-06, direct USB; see
    /// [`ScreenMouseType::Move`]), so this stream is a `PRESS` and a `RELEASE` with
    /// nothing in between that the unit reads. Six hardware attempts (scrollbar
    /// thumb at x=733 and x=740, the popup body, start dwells of 200 to 1000 ms,
    /// 18 to 40 steps) produced no scroll, only a highlight on the row under the
    /// end point; those attempts sent the frames in the earlier order
    /// (`RELEASE` first, `PRESS` last), so the literal order was never separately
    /// run, but `MOVE` itself was measured inert on its own, in a held press and
    /// in every coordinate encoding. The unit's own self-contained drag is ONE
    /// `DRAG` frame, which [`Self::ScreenDrag`] (the `swipe_screen` gesture) and a
    /// raw `drag` mouse frame send.
    ///
    /// The gateway's `drag_screen` action therefore no longer plans this
    /// operation: it plans the same single [`Self::ScreenDrag`] frame as
    /// `swipe_screen`, still bounds-checking and then ignoring `steps` and
    /// `holdMs` (`request.rs`, `device.dragScreen`). The operation and its builder stay in
    /// the library, documented as inert, so a caller who wants to experiment can
    /// still build the exact frames; nothing in this repository sends them to a
    /// unit. See [`screen_drag_steps`] for the frame sequence, and
    /// [`Self::abort_release_frame`] for what a host sends if it stops early.
    ScreenDragSteps {
        /// Starting horizontal position in screen pixels: a whole number in `0..800`.
        x: f32,
        /// Starting vertical position in screen pixels: a whole number in `0..480`.
        y: f32,
        /// Ending horizontal position in screen pixels: a whole number in `0..800`.
        to_x: f32,
        /// Ending vertical position in screen pixels: a whole number in `0..480`.
        to_y: f32,
        /// Interpolated MOVE frames between the two points
        /// (`SCREEN_DRAG_MIN_STEPS..=SCREEN_DRAG_MAX_STEPS`).
        steps: u32,
        /// Dwell at the start point in milliseconds, rounded up to whole
        /// frame intervals (`0..=SCREEN_DRAG_MAX_HOLD_MS`). The dwell at the end
        /// point is always `SCREEN_DRAG_END_HOLD_MS`.
        hold_ms: u32,
    },
    /// Send ONE raw RemoteControl mouse frame of the protocol, unmodified. See
    /// [`ScreenMouseType`] and [`screen_mouse_frame`]. A frame of type
    /// [`ScreenMouseType::Press`] puts a pointer down that stays down until a
    /// [`ScreenMouseType::Release`] frame lifts it; the host that sends it must
    /// track that (see `device::touch_guard`).
    ScreenMouseFrame {
        /// Which of the protocol's five mouse frame types to send.
        kind: ScreenMouseType,
        /// Horizontal position in screen pixels: a whole number in `0..800`.
        x: f32,
        /// Vertical position in screen pixels: a whole number in `0..480`.
        y: f32,
        /// Destination in screen pixels, present for [`ScreenMouseType::Drag`]
        /// and absent for every other type.
        to: Option<(f32, f32)>,
    },
    /// Copy one scene onto another, or swap two scenes. The complete scene state
    /// moves, including label and colour.
    CopyScene {
        /// Source scene index (`0..=7`).
        from_index: u32,
        /// Destination scene index (`0..=7`).
        to_index: u32,
        /// `true` to swap the two scenes, `false` to copy.
        swap: bool,
    },
    /// Set a scene's label.
    SetSceneLabel {
        /// Scene index (`0..=7`).
        scene: u32,
        /// New label, or `None` to clear it. The UTF-8 length is limited to what fits
        /// in one protocol frame.
        label: Option<String>,
    },
    /// Set a scene's colour.
    SetSceneColor {
        /// Scene index (`0..=7`).
        scene: u32,
        /// Native ARGB colour value.
        color: u32,
    },
    /// Enable or disable per-scene storage for one block parameter.
    SetParameterSceneMode {
        /// Zero-based grid row (`0..=3`). For splitter/mixer columns the row must
        /// be even.
        row: u32,
        /// Target column (`0..=9`): `0..=7` are block columns, `8` is the combined
        /// splitter and `9` the mixer.
        column: u32,
        /// Index of the parameter within the target's parameter list.
        parameter_index: u32,
        /// `true` to store the parameter per scene.
        enabled: bool,
    },
    /// Assign an expression pedal to one block parameter.
    SetParameterExpression {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Target column (`0..=9`): `0..=7` are block columns, `8` is the combined
        /// splitter and `9` the mixer.
        column: u32,
        /// Index of the parameter within the target's parameter list.
        parameter_index: u32,
        /// Expression pedal selector (`0..=2`): `0` clears the assignment, `1`
        /// and `2` select the unit's EXP 1 and EXP 2 inputs.
        pedal: u32,
        /// Parameter value at the heel position, normalized `0.0..=1.0`.
        minimum: f32,
        /// Parameter value at the toe position, normalized `0.0..=1.0`.
        maximum: f32,
    },
    /// Set whether a footswitch is momentary or latching for the current preset.
    SetStompMomentary {
        /// Physical footswitch index (A-H).
        footswitch: u32,
        /// `true` for momentary, `false` for latching.
        momentary: bool,
    },
    /// Set a footswitch label for the current preset.
    SetStompLabel {
        /// Footswitch index (`0..=7`, A-H).
        footswitch: u32,
        /// Label text: 1 to 16 characters, with no control characters.
        label: String,
        /// `true` writes the single-stomp label map, `false` the ordinary stomp label
        /// map.
        single: bool,
    },
    /// Replace the MIDI Out messages of one source.
    ///
    /// On the unit, a footswitch's messages are sent only in Scene, Stomp or
    /// Hybrid mode, while expression messages are always sent. Each message
    /// carries a raw wire `type` (`1` Control Change, `2` Control Change
    /// Toggle, `3` Program Change) and three parameters whose roles depend on
    /// it; see [`set_midi_out_selector`] for the observed roles and for a
    /// hazard of writing an undefined type.
    SetMidiOut {
        /// Source index (`0..=9`): `0..=7` are footswitches A-H, `8` is EXP 1
        /// and `9` is EXP 2 (observed on a CorOS 4.1.0 unit, 2026-10-06).
        source: u32,
        /// Up to [`MIDI_OUT_SLOTS_PER_SOURCE`] messages, one per slot in the
        /// order given; a type `0` message keeps its slot empty and unused
        /// trailing slots are cleared.
        messages: Vec<MidiOutMessage>,
    },
    /// Replace the list of MIDI messages sent when a preset loads.
    ///
    /// Observed on a CorOS 4.1.0 unit, 2026-10-06: this list accepts types `1`,
    /// `2` and `3`, shown as Control Change, Control Change Toggle and Program
    /// Change as in every other list, and an empty list clears it. The type
    /// numbering is in [`set_midi_out_selector`].
    SetPresetLoadMidiOut {
        /// Up to [`MIDI_OUT_SLOTS_PER_SOURCE`] messages, one per slot in the
        /// order given; a type `0` message keeps its slot empty and unused
        /// trailing slots are cleared.
        messages: Vec<MidiOutMessage>,
    },
    /// Assign an expression pedal to a placed block's bypass switch.
    SetExpressionBypass {
        /// Zero-based grid row (`0..=3`).
        row: u32,
        /// Zero-based grid column (`0..=7`).
        column: u32,
        /// Expression pedal selector (`0..=2`): `1` and `2` select the unit's
        /// EXP 1 and EXP 2 inputs, and `0` clears the assignment.
        pedal: u32,
        /// Expression bypass mode (`0..=2`), a raw wire number sent unchanged as
        /// the `type` field. Observed on a CorOS 4.1.0 unit, 2026-10-07 (the
        /// unit's assignment screen read, labelled SWITCH ON): `0` is Stop, `1`
        /// is Switch and `2` is Heel-Toe.
        ///
        /// The behaviours themselves are believed rather than shown. Heel-Toe
        /// bypasses the block at the heel end of the pedal's travel (at the toe
        /// end instead when `invert` is set), Switch bypasses the block while
        /// the pedal's toe switch is pressed, and Stop bypasses the block when
        /// the pedal is held still for `delay_ms`. A newly assigned bypass shows
        /// Heel-Toe selected (observed on CorOS 4.1.0).
        mode: u32,
        /// Whether the pedal direction is inverted: the unit's INVERT RANGE option
        /// shows On or Off to match, with no inversion of the value (observed on
        /// a CorOS 4.1.0 unit, 2026-10-07). It is believed to move Heel-Toe
        /// bypass from the heel end to the toe end.
        invert: bool,
        /// Delay in raw milliseconds (`0..=5000`): the unit displayed 250, 1000,
        /// 2500 and 5000 exactly, and showed `0` as OFF (observed on a CorOS
        /// 4.1.0 unit, 2026-10-07). It is believed to be the time the pedal must
        /// be held still for the Stop behaviour. A bypass newly assigned on the
        /// unit shows 600 ms on its screen (observed on CorOS 4.1.0).
        delay_ms: u32,
        /// Whether latch emulation is enabled: the unit's LATCH EMULATION option
        /// shows On or Off to match (observed 2026-10-07). It is believed to let
        /// a momentary toe switch behave as a latching one. The unit's screen
        /// greys this control out
        /// while its Heel-Toe behaviour is selected (observed on CorOS 4.1.0),
        /// and it is believed to be available only for the Switch behaviour
        /// (mode `1`).
        latch_emulation: bool,
    },
}

impl DeviceOperation {
    /// Encode a public operation after checking values that the protobuf wire
    /// shape cannot represent and string selectors used for sparse dispatch.
    ///
    /// The unchecked encoder is deliberately private so callers cannot bypass
    /// these validation checks.
    ///
    /// ```compile_fail
    /// use quadcortex::commands::DeviceOperation;
    ///
    /// let _ = DeviceOperation::Undo.encode_unchecked();
    /// ```
    pub fn try_encode(self) -> Result<Vec<OutboundMessage>, CommandEncodeError> {
        self.validate_for_encoding()?;
        self.encode_unchecked()
    }

    fn validate_for_encoding(&self) -> Result<(), CommandEncodeError> {
        let signed = |field, value| {
            i32::try_from(value)
                .map(|_| ())
                .map_err(|_| CommandEncodeError::SignedFieldOverflow { field })
        };
        let bounded = |field: &'static str,
                       value: u32,
                       upper_exclusive: u32|
         -> Result<(), CommandEncodeError> {
            if value < upper_exclusive {
                Ok(())
            } else {
                Err(CommandEncodeError::UnsupportedValue {
                    field,
                    value: value.to_string(),
                })
            }
        };
        let screen_coordinate = |field: &'static str,
                                 value: f32,
                                 upper_exclusive: f32|
         -> Result<(), CommandEncodeError> {
            if value.is_finite() && value.fract() == 0.0 && (0.0..upper_exclusive).contains(&value)
            {
                Ok(())
            } else {
                Err(CommandEncodeError::UnsupportedValue {
                    field,
                    value: value.to_string(),
                })
            }
        };
        let setlist_name = |value: &str| -> Result<(), CommandEncodeError> {
            let characters = value.chars().count();
            if (1..=64).contains(&characters)
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && !value.contains(['/', '\\'])
                && !matches!(value, "." | "..")
                && !value.eq_ignore_ascii_case("My Presets")
            {
                Ok(())
            } else {
                Err(CommandEncodeError::UnsupportedValue {
                    field: "setlist name",
                    value: value.to_string(),
                })
            }
        };

        match self {
            Self::AddBlock { row, column, .. }
            | Self::ReplaceBlock { row, column, .. }
            | Self::CreateModelPreset { row, column, .. }
            | Self::RemoveBlock { row, column }
            | Self::ResetBlockParameters { row, column }
            | Self::SetFootswitch { row, column, .. }
            | Self::LoadCapture { row, column, .. }
            | Self::RetargetCapture { row, column, .. }
            | Self::LoadIr { row, column, .. }
            | Self::SetExpressionBypass { row, column, .. } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
                bounded("grid column", *column, domain::GRID_COLUMNS)?;
            }
            Self::MoveBlock {
                from_row,
                from_column,
                to_row,
                to_column,
                grid_model_ids,
            } => {
                bounded("source grid row", *from_row, domain::GRID_ROWS)?;
                bounded("source grid column", *from_column, domain::GRID_COLUMNS)?;
                bounded("destination grid row", *to_row, domain::GRID_ROWS)?;
                bounded("destination grid column", *to_column, domain::GRID_COLUMNS)?;
                if grid_model_ids.is_empty() {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "move grid context",
                        value: "empty".into(),
                    });
                }
            }
            Self::SetChainInput { row, input_id } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
                if !domain::INPUT_ROUTES.iter().any(|(id, _)| id == input_id) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "chain input route ID",
                        value: input_id.to_string(),
                    });
                }
            }
            Self::SetChainOutput { row, .. }
            | Self::SetRoutingParameter { row, .. }
            | Self::SetLaneControlParameter { row, .. }
            | Self::SetLaneControlSceneMode { row, .. }
            | Self::SetLaneControlExpression { row, .. }
            | Self::SetParameterSceneMode { row, .. }
            | Self::SetParameterExpression { row, .. } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
            }
            Self::SetSplitMute { row, .. } => {
                bounded("split/mixer row", *row, domain::GRID_ROWS)?;
                if row % 2 != 0 {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "split/mixer row",
                        value: row.to_string(),
                    });
                }
            }
            Self::ScreenTap { x, y } | Self::ScreenHold { x, y } => {
                screen_coordinate("screen x", *x, 800.0)?;
                screen_coordinate("screen y", *y, 480.0)?;
            }
            Self::ScreenMouseFrame { kind, x, y, to } => {
                screen_coordinate("screen x", *x, 800.0)?;
                screen_coordinate("screen y", *y, 480.0)?;
                match (kind, to) {
                    (ScreenMouseType::Drag, Some((to_x, to_y))) => {
                        screen_coordinate("screen destination x", *to_x, 800.0)?;
                        screen_coordinate("screen destination y", *to_y, 480.0)?;
                        if x == to_x && y == to_y {
                            return Err(CommandEncodeError::UnsupportedValue {
                                field: "screen drag endpoints",
                                value: format!("({x}, {y}) to ({to_x}, {to_y})"),
                            });
                        }
                    }
                    (ScreenMouseType::Drag, None) => {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field: "screen mouse drag destination",
                            value: "missing".into(),
                        });
                    }
                    (_, Some((to_x, to_y))) => {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field: "screen mouse destination",
                            value: format!("({to_x}, {to_y}) is only valid for a drag frame"),
                        });
                    }
                    (_, None) => {}
                }
            }
            Self::ScreenDrag { x, y, to_x, to_y } => {
                screen_coordinate("screen x", *x, 800.0)?;
                screen_coordinate("screen y", *y, 480.0)?;
                screen_coordinate("screen destination x", *to_x, 800.0)?;
                screen_coordinate("screen destination y", *to_y, 480.0)?;
            }
            Self::ScreenDragSteps {
                x,
                y,
                to_x,
                to_y,
                steps,
                hold_ms,
            } => {
                screen_coordinate("screen x", *x, 800.0)?;
                screen_coordinate("screen y", *y, 480.0)?;
                screen_coordinate("screen destination x", *to_x, 800.0)?;
                screen_coordinate("screen destination y", *to_y, 480.0)?;
                if x == to_x && y == to_y {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "screen drag endpoints",
                        value: format!("({x}, {y}) to ({to_x}, {to_y})"),
                    });
                }
                if !(SCREEN_DRAG_MIN_STEPS..=SCREEN_DRAG_MAX_STEPS).contains(steps) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "screen drag steps",
                        value: steps.to_string(),
                    });
                }
                if *hold_ms > SCREEN_DRAG_MAX_HOLD_MS {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "screen drag hold",
                        value: hold_ms.to_string(),
                    });
                }
            }
            Self::CreateSetlist { name } | Self::DeleteSetlist { name } => {
                setlist_name(name)?;
            }
            _ => {}
        }
        match self {
            Self::Command(command) => command.validate_for_encoding()?,
            Self::SetTunerInput(input_port_id) => {
                if !is_tuner_input_port_id(*input_port_id) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "tuner input port ID",
                        value: input_port_id.to_string(),
                    });
                }
            }
            Self::SetTunerReference(offset_hz) => {
                if !is_tuner_reference_offset_hz(*offset_hz) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "tuner reference offset Hz",
                        value: offset_hz.to_string(),
                    });
                }
            }
            Self::SetGlobalBypass {
                cab: None,
                ir: None,
            } => {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "global bypass",
                    value: "neither cab nor ir was supplied".into(),
                });
            }
            Self::SetChainSplit {
                row,
                split_column,
                mix_column,
            } => {
                if !matches!(*row, 0 | 2) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "parallel-routing row",
                        value: row.to_string(),
                    });
                }
                let split = split_column.unwrap_or(-1);
                let mix = mix_column.unwrap_or(-1);
                if !(-1..domain::GRID_COLUMNS as i32).contains(&split) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "split column",
                        value: split.to_string(),
                    });
                }
                if !(-1..domain::GRID_COLUMNS as i32).contains(&mix) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "mix column",
                        value: mix.to_string(),
                    });
                }
                if split >= 0 && mix >= 0 && mix <= split {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "split/mix column pair",
                        value: format!("{split}/{mix}"),
                    });
                }
            }
            Self::SetInputPort {
                input_port_id,
                level,
                impedance,
                input_type,
                ground_lift,
                ..
            } => {
                if !is_input_setting_port_id(*input_port_id) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "input port ID",
                        value: input_port_id.to_string(),
                    });
                }
                for (field, value, valid) in [
                    (
                        "normalized input level",
                        *level,
                        level.is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value)),
                    ),
                    (
                        "input impedance",
                        *impedance,
                        impedance.is_none_or(is_input_impedance_value),
                    ),
                    (
                        "input type",
                        *input_type,
                        input_type.is_none_or(is_input_type_value),
                    ),
                    (
                        "input ground lift",
                        *ground_lift,
                        ground_lift.is_none_or(is_input_ground_lift_value),
                    ),
                ] {
                    if !valid {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field,
                            value: value
                                .expect("invalid optional value is present")
                                .to_string(),
                        });
                    }
                }
            }
            Self::SetOutputPort { output_port_id, .. }
                if !matches!(*output_port_id, 1 | 2 | 4 | 5 | 6 | 7 | 8 | 9) =>
            {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "output port ID",
                    value: output_port_id.to_string(),
                });
            }
            Self::SetOutputPort {
                level, ground_lift, ..
            } => {
                for (field, value, valid) in [
                    (
                        "output level dB",
                        *level,
                        level.is_none_or(|value| {
                            value.is_finite()
                                && (OUTPUT_LEVEL_MIN_DB..=OUTPUT_LEVEL_MAX_DB)
                                    .contains(&f64::from(value))
                        }),
                    ),
                    (
                        "output ground lift",
                        *ground_lift,
                        ground_lift.is_none_or(is_input_ground_lift_value),
                    ),
                ] {
                    if !valid {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field,
                            value: value
                                .expect("invalid optional value is present")
                                .to_string(),
                        });
                    }
                }
            }
            Self::SetUsbPort {
                level,
                headphones_source,
                dry_wet,
                ..
            } => {
                for (field, value, valid) in [
                    (
                        "normalized USB level",
                        *level,
                        level.is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value)),
                    ),
                    (
                        "USB headphones source",
                        *headphones_source,
                        headphones_source.is_none_or(is_usb_headphones_source_value),
                    ),
                    (
                        "USB dry/wet routing",
                        *dry_wet,
                        dry_wet.is_none_or(is_usb_dry_wet_value),
                    ),
                ] {
                    if !valid {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field,
                            value: value
                                .expect("invalid optional value is present")
                                .to_string(),
                        });
                    }
                }
            }
            Self::SetGlobalEqParameters(parameters) => {
                for (index, value) in parameters {
                    if !value.is_finite() {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field: "Global EQ parameter value",
                            value: format!("index {index}, value {value}"),
                        });
                    }
                }
            }
            Self::SetTempoParameters(parameters) => {
                if parameters.is_empty() {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "tempo/metronome parameters",
                        value: "empty".into(),
                    });
                }
                let mut seen = [false; 23];
                for (index, value) in parameters {
                    if !is_tempo_metronome_parameter_value(*index, *value) {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field: "tempo/metronome parameter",
                            value: format!("index {index}, value {value}"),
                        });
                    }
                    if seen[*index as usize] {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field: "tempo/metronome parameter index",
                            value: index.to_string(),
                        });
                    }
                    seen[*index as usize] = true;
                }
            }
            Self::SetGlobalTempo(bpm)
                if !(domain::MINIMUM_TEMPO_BPM..=domain::MAXIMUM_TEMPO_BPM).contains(bpm) =>
            {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "global tempo BPM",
                    value: bpm.to_string(),
                });
            }
            Self::SetFavorite {
                name,
                folder_key,
                product_key,
                ..
            } => validate_favorite_identity(name, folder_key, product_key)?,
            Self::LoadCapture { model_id, .. }
                if !model_id.is_some_and(is_neural_capture_model) =>
            {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "capture model ID",
                    value: model_id
                        .map_or_else(|| "retarget".to_string(), |value| value.to_string()),
                });
            }
            Self::RetargetCapture {
                column,
                active_scene,
                key,
                model,
                bypass,
                ..
            } => {
                bounded("active scene", *active_scene, domain::SCENE_COUNT)?;
                validate_capture_retarget(*column, *active_scene, key, model, bypass.as_ref())?;
            }
            Self::LoadIr { slot, .. } if *slot > 1 => {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "IR slot",
                    value: slot.to_string(),
                });
            }
            Self::SetGeneralInteger { setting, value } => {
                if !is_general_integer_value(setting, *value) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "GeneralSettings integer",
                        value: format!("{setting}={value}"),
                    });
                }
            }
            Self::SetGeneralToggle { setting, .. } => {
                // This is the complete two-state (boolean) Device Setting
                // whitelist.
                // It is not the complete list of writable General Settings:
                // `midiClockOut` (`[MANUAL 7.0]`) is a real setting backed by
                // a 4-value enum, not a bool, and is validated by
                // `is_general_integer_value`/`set_general_integer` instead.
                if !matches!(
                    setting.as_str(),
                    "midiOverUsb"
                        | "lockScreenAndVolumeKnob"
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
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "GeneralSettings toggle",
                        value: setting.clone(),
                    });
                }
            }
            Self::SetSceneBypassBehavior(value) if !(0..=2).contains(value) => {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "scene bypass behavior",
                    value: value.to_string(),
                });
            }
            Self::SetModeCycle(slots) if !is_mode_cycle_value(slots) => {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "mode cycle",
                    value: format!("{slots:?}"),
                });
            }
            Self::SetRoutingParameter { node, value, .. } => {
                if !matches!(node.as_str(), "splitter" | "legacySplitter" | "mixer") {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "routing node",
                        value: node.clone(),
                    });
                }
                if !value.is_finite() || !(0.0..=1.0).contains(value) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "normalized routing parameter value",
                        value: value.to_string(),
                    });
                }
            }
            Self::AddBlock { model_id, .. } | Self::ReplaceBlock { model_id, .. }
                if *model_id == 0 || is_neural_capture_model(*model_id) =>
            {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "ordinary model ID",
                    value: model_id.to_string(),
                });
            }
            Self::CreateModelPreset {
                model_hash, name, ..
            } => {
                signed("model preset source hash", *model_hash)?;
                if *model_hash == 0
                    || name.trim().is_empty()
                    || name.trim() != name
                    || name.chars().count() > 80
                    || name.chars().any(char::is_control)
                {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "model preset",
                        value: format!("{model_hash}:{name}"),
                    });
                }
            }
            Self::SetLooperStompAssignments { assignments } => {
                let mut sorted = assignments.clone();
                sorted.sort_unstable();
                if sorted != (0..8).collect::<Vec<_>>() {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "Looper X A-H assignment permutation",
                        value: format!("{assignments:?}"),
                    });
                }
            }
            Self::SetLaneControlParameter {
                row,
                control,
                value,
                ..
            } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
                if !matches!(control.as_str(), "inputGate" | "laneOutput") {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "lane control",
                        value: control.clone(),
                    });
                }
                if !value.is_finite() || !(0.0..=1.0).contains(value) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "normalized lane-control parameter value",
                        value: value.to_string(),
                    });
                }
            }
            Self::SetLaneControlSceneMode { control, .. } => {
                if !matches!(control.as_str(), "inputGate" | "laneOutput") {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "lane control",
                        value: control.clone(),
                    });
                }
            }
            Self::SetLaneControlExpression {
                control,
                pedal,
                minimum,
                maximum,
                ..
            } => {
                if !matches!(control.as_str(), "inputGate" | "laneOutput") {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "lane control",
                        value: control.clone(),
                    });
                }
                if *pedal > 2 {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "expression pedal",
                        value: pedal.to_string(),
                    });
                }
                for (field, value) in [
                    ("expression heel value", minimum),
                    ("expression toe value", maximum),
                ] {
                    if !value.is_finite() || !(0.0..=1.0).contains(value) {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field,
                            value: value.to_string(),
                        });
                    }
                }
            }
            Self::SetParameterSceneMode { row, column, .. } => {
                if *column > 9 {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "parameter target column",
                        value: column.to_string(),
                    });
                }
                if *column >= domain::GRID_COLUMNS && row % 2 != 0 {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "splitter/mixer parameter row",
                        value: row.to_string(),
                    });
                }
            }
            Self::SetParameterExpression { column, .. } if *column > 9 => {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "parameter target column",
                    value: column.to_string(),
                });
            }
            Self::MovePreset { position, .. }
            | Self::SwapPreset { position, .. }
            | Self::CopyPreset {
                destination_position: position,
                ..
            }
            | Self::SavePreset { position, .. } => signed("position", *position)?,
            Self::PresetScreenshot { position, .. } => bounded("position", *position, 256)?,
            Self::CopyScene {
                from_index,
                to_index,
                ..
            } => {
                bounded("from_index", *from_index, domain::SCENE_COUNT)?;
                bounded("to_index", *to_index, domain::SCENE_COUNT)?;
            }
            Self::SetSceneLabel { scene, label } => {
                bounded("scene", *scene, domain::SCENE_COUNT)?;
                if label
                    .as_ref()
                    .is_some_and(|label| label.len() > profile::MAX_FRAME_BYTES - 16)
                {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "scene label UTF-8 byte length",
                        value: label
                            .as_ref()
                            .expect("oversized label is present")
                            .len()
                            .to_string(),
                    });
                }
            }
            Self::SetSceneColor { scene, .. } => {
                if *scene >= domain::SCENE_COUNT {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "scene",
                        value: scene.to_string(),
                    });
                }
            }
            Self::SetStompMomentary { footswitch, .. } => {
                bounded("footswitch", *footswitch, domain::SCENE_COUNT)?;
            }
            Self::SetStompLabel {
                footswitch, label, ..
            } => {
                bounded("footswitch", *footswitch, domain::SCENE_COUNT)?;
                let characters = label.chars().count();
                if !(1..=16).contains(&characters) || label.chars().any(char::is_control) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "STOMP label character count",
                        value: characters.to_string(),
                    });
                }
            }
            Self::SetFootswitch {
                footswitch,
                previous_footswitch,
                previous_assignment_type,
                ..
            } => {
                if let Some(footswitch) = footswitch {
                    bounded("footswitch", *footswitch, domain::SCENE_COUNT)?;
                }
                if let Some(footswitch) = previous_footswitch {
                    bounded("previous footswitch", *footswitch, domain::SCENE_COUNT)?;
                }
                if previous_footswitch.is_some() != previous_assignment_type.is_some() {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "previous footswitch assignment",
                        value: "index and type presence must match".into(),
                    });
                }
            }
            Self::SetParameterExpression {
                pedal,
                minimum,
                maximum,
                ..
            } => {
                bounded("parameter expression pedal", *pedal, 3)?;
                for (field, value) in [
                    ("parameter expression minimum", minimum),
                    ("parameter expression maximum", maximum),
                ] {
                    if !value.is_finite() || !(0.0..=1.0).contains(value) {
                        return Err(CommandEncodeError::UnsupportedValue {
                            field,
                            value: value.to_string(),
                        });
                    }
                }
            }
            Self::SetExpressionBypass {
                pedal,
                mode,
                delay_ms,
                ..
            } => {
                if !(0..=2).contains(pedal) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "expression bypass pedal",
                        value: pedal.to_string(),
                    });
                }
                bounded("expression bypass mode", *mode, 3)?;
                bounded("expression bypass delay", *delay_ms, 5_001)?;
            }
            Self::SetMidiOut { source, messages } => {
                if !MIDI_OUT_SOURCES.contains(source) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "MIDI Out source",
                        value: source.to_string(),
                    });
                }
                validate_midi_out_messages(messages)?;
            }
            Self::SetPresetLoadMidiOut { messages } => validate_midi_out_messages(messages)?,
            Self::SetChainOutput { output_id, .. } if !is_chain_output_port_id(*output_id) => {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "chain output port ID",
                    value: output_id.to_string(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn encode_unchecked(self) -> Result<Vec<OutboundMessage>, CommandEncodeError> {
        Ok(match self {
            Self::Command(command) => vec![command.encode_unchecked()],
            Self::AddBlock {
                row,
                column,
                model_id,
            } => vec![set_block(row, column, model_id)],
            Self::ReplaceBlock {
                row,
                column,
                model_id,
            } => vec![set_block(row, column, model_id)],
            Self::CreateModelPreset {
                row,
                column,
                model_hash,
                name,
            } => {
                vec![create_model_preset(row, column, model_hash, name)]
            }
            Self::SetLooperStompAssignments { assignments } => {
                vec![set_looper_stomp_assignments(assignments)]
            }
            Self::RemoveBlock { row, column } => vec![remove_block(row, column)],
            Self::ResetBlockParameters { row, column } => {
                vec![reset_block_parameters(row, column)]
            }
            Self::MoveBlock {
                from_row,
                from_column,
                to_row,
                to_column,
                grid_model_ids,
            } => {
                vec![move_block(
                    from_row,
                    from_column,
                    to_row,
                    to_column,
                    grid_model_ids,
                )]
            }
            Self::SetFootswitch {
                row,
                column,
                footswitch,
                assignment_type,
                previous_footswitch,
                previous_assignment_type,
            } => set_footswitch_transition(
                row,
                column,
                footswitch,
                assignment_type,
                previous_footswitch,
                previous_assignment_type,
            ),
            Self::SetChainInput { row, input_id } => vec![set_chain_input(row, input_id)],
            Self::SetChainOutput { row, output_id } => vec![set_chain_output(row, output_id)],
            Self::SetChainSplit {
                row,
                split_column,
                mix_column,
            } => {
                vec![set_chain_split(row, split_column, mix_column)]
            }
            Self::SetSplitMute { row, muted } => vec![set_split_mute(row, muted)],
            Self::SetRoutingParameter {
                row,
                node,
                parameter_index,
                value,
            } => {
                vec![set_routing_parameter(row, &node, parameter_index, value)?]
            }
            Self::SetLaneControlParameter {
                row,
                control,
                parameter_index,
                value,
            } => vec![set_lane_control_parameter(
                row,
                &control,
                parameter_index,
                value,
            )?],
            Self::SetLaneControlSceneMode {
                row,
                control,
                parameter_index,
                enabled,
            } => vec![set_lane_control_scene_mode(
                row,
                &control,
                parameter_index,
                enabled,
            )?],
            Self::SetLaneControlExpression {
                row,
                control,
                parameter_index,
                pedal,
                minimum,
                maximum,
            } => vec![set_lane_control_expression(
                row,
                &control,
                parameter_index,
                pedal,
                minimum,
                maximum,
            )?],
            Self::ListPresetFolders => vec![read_folders(false)],
            Self::ReadCurrentPreset => vec![read_current_preset()],
            Self::ReadGeneralSettings => vec![read(profile::MESSAGE_TYPE_GENERAL_SETTINGS)],
            Self::SetGeneralInteger { setting, value } => {
                vec![set_general_integer(&setting, value)?]
            }
            Self::SetGeneralToggle { setting, enabled } => {
                vec![set_general_toggle(&setting, enabled)?]
            }
            Self::SetSceneBypassBehavior(behavior) => vec![set_scene_bypass_behavior(behavior)],
            Self::SetMasterVolumeAssignment {
                out12,
                out34,
                send12,
                headphones,
            } => vec![set_master_volume_assignment(
                out12, out34, send12, headphones,
            )],
            Self::SetGlobalBypass { cab, ir } => set_global_bypass(cab, ir),
            Self::SavePreset {
                setlist_key,
                position,
                name,
                instrument,
            } => {
                vec![save_preset(setlist_key, position, name, instrument)]
            }
            Self::RenamePreset {
                setlist_key,
                product_key,
                name,
                instrument,
            } => vec![rename_preset(setlist_key, product_key, name, instrument)],
            Self::ReadVersion => vec![read_version()],
            Self::ReadTuner => vec![read_tuner()],
            Self::ShowTuner(show) => show_tuner_workflow(show, false).to_vec(),
            Self::SetTunerInput(input_port_id) => vec![set_tuner_input(input_port_id)],
            Self::SetTunerMute(muted) => vec![set_tuner_mute(muted)],
            Self::SetTunerMeter(enabled) => vec![set_tuner_meter(enabled)],
            Self::SetTunerReference(offset_hz) => vec![set_tuner_reference(offset_hz)],
            Self::ReadIoSettings => vec![read(profile::MESSAGE_TYPE_IO_SETTINGS)],
            Self::ReadGlobalEq => vec![read_global_eq()],
            Self::SetGlobalEqBypassed(bypassed) => vec![set_global_eq_bypassed(bypassed)],
            Self::SetGlobalEqParameters(parameters) => set_global_eq_parameters(&parameters),
            Self::LoadGlobalEqModelPreset {
                value,
                is_factory,
                hash,
            } => {
                vec![load_global_eq_model_preset(&value, is_factory, hash)]
            }
            Self::ReadModeCycle => vec![read_mode_cycle()],
            Self::SetModeCycle(slots) => vec![set_mode_cycle(&slots)],
            Self::ReadGlobalTempo => vec![read(profile::MESSAGE_TYPE_GLOBAL_TEMPO)],
            Self::SetGlobalTempo(bpm) => vec![set_global_tempo(bpm)],
            Self::SetTempoParameters(parameters) => set_tempo_parameters(parameters),
            Self::SetTempoMode(global) => vec![set_tempo_mode(global)],
            Self::ReadLooperStatus => vec![read_looper_status()],
            Self::ReadRecentsFavorites {
                favorites,
                request_id,
            } => vec![read_recents_favorites(favorites, request_id)],
            Self::SetFavorite {
                name,
                folder_key,
                product_key,
                is_factory,
                is_plugin,
                favorite,
            } => vec![set_favorite(
                name,
                folder_key,
                product_key,
                is_factory,
                is_plugin,
                favorite,
            )],
            Self::ReadPinnedModels => vec![read_pinned_models()],
            Self::SetModelPinned { model_id, pinned } => {
                vec![set_model_pinned(model_id, pinned)]
            }
            Self::ReadLibraryFiles {
                folder_key,
                file_type,
                request_id,
            } => vec![read_library_files_query(folder_key, file_type, request_id)],
            Self::CreateSetlist { name } => vec![create_setlist(name)],
            Self::DeleteSetlist { name } => vec![delete_setlist(name)],
            Self::DeletePreset {
                setlist_key,
                product_key,
            } => vec![delete_preset(setlist_key, product_key)],
            Self::MovePreset {
                setlist_key,
                product_key,
                position,
            } => vec![move_preset(setlist_key, product_key, position)],
            Self::SwapPreset {
                setlist_key,
                product_key,
                position,
            } => vec![swap_preset(setlist_key, product_key, position)],
            Self::CopyPreset {
                source_setlist_key,
                source_product_key,
                source_is_factory,
                destination_setlist_key,
                destination_position,
            } => vec![copy_preset(
                source_setlist_key,
                source_product_key,
                source_is_factory,
                destination_setlist_key,
                destination_position,
            )],
            Self::DuplicateSetlist { source_setlist_key } => {
                vec![duplicate_setlist(source_setlist_key)]
            }
            Self::LoadCapture {
                row,
                column,
                key,
                model_id,
            } => model_id.map_or_else(Vec::new, |model_id| {
                vec![set_block_with_initial_value(
                    row,
                    column,
                    model_id,
                    Some(ParamValue {
                        value: Some(param_value::Value::StringValue(key)),
                    }),
                )]
            }),
            Self::RetargetCapture {
                row,
                column,
                model,
                bypass,
                ..
            } => vec![update_auditioning_model(row, column, model, bypass)],
            Self::LoadIr {
                row,
                column,
                key,
                slot,
            } => vec![set_parameter_text(
                row,
                column,
                if slot == 0 { 2 } else { 10 },
                key,
            )],
            Self::SetInputPort {
                input_port_id,
                level,
                impedance,
                input_type,
                ground_lift,
            } => set_input_port(input_port_id, level, impedance, input_type, ground_lift),
            Self::SetOutputPort {
                output_port_id,
                level,
                ground_lift,
                mute,
            } => set_output_port(output_port_id, level, ground_lift, mute),
            Self::SetUsbPort {
                level,
                headphones_source,
                dry_wet,
            } => set_usb_port(level, headphones_source, dry_wet),
            Self::SetMidiThru(enabled) => vec![set_midi_thru(enabled)],
            Self::SetOutputPairing {
                xlr12_linked,
                out34_linked,
            } => set_output_pairing(xlr12_linked, out34_linked),
            Self::SetDeviceName(name) => vec![set_device_name(name)],
            Self::Undo => vec![undo()],
            Self::Redo => vec![redo()],
            Self::ReadInhibitedModules => vec![read_inhibited_modules()],
            Self::PresetScreenshot {
                folder_name,
                position,
                is_factory,
                request_id,
            } => vec![preset_screenshot(
                folder_name,
                position,
                is_factory,
                request_id,
            )],
            Self::CaptureScreen => vec![capture_screen()],
            Self::ReadDiagnostics { request_id } => vec![read_diagnostics(Some(request_id))],
            Self::ReadGraphicsTree => vec![read_graphics_tree()],
            Self::ScreenTap { x, y } => screen_tap(x, y).to_vec(),
            Self::ScreenHold { x, y } => screen_hold(x, y).to_vec(),
            Self::ScreenMouseFrame { kind, x, y, to } => vec![screen_mouse_frame(kind, x, y, to)],
            Self::ScreenDrag { x, y, to_x, to_y } => screen_drag(x, y, to_x, to_y).to_vec(),
            Self::ScreenDragSteps {
                x,
                y,
                to_x,
                to_y,
                steps,
                hold_ms,
            } => screen_drag_steps(x, y, to_x, to_y, steps, hold_ms),
            Self::CopyScene {
                from_index,
                to_index,
                swap,
            } => vec![copy_scene(from_index, to_index, swap)],
            Self::SetSceneLabel { scene, label } => vec![set_scene_label(scene, label)],
            Self::SetSceneColor { scene, color } => vec![set_scene_color(scene, color)],
            Self::SetParameterSceneMode {
                row,
                column,
                parameter_index,
                enabled,
            } => {
                vec![set_parameter_scene_mode(
                    row,
                    column,
                    parameter_index,
                    enabled,
                )?]
            }
            Self::SetParameterExpression {
                row,
                column,
                parameter_index,
                pedal,
                minimum,
                maximum,
            } => {
                vec![set_parameter_expression(
                    row,
                    column,
                    parameter_index,
                    pedal,
                    minimum,
                    maximum,
                )?]
            }
            Self::SetStompMomentary {
                footswitch,
                momentary,
            } => vec![set_stomp_momentary(footswitch, momentary)],
            Self::SetStompLabel {
                footswitch,
                label,
                single,
            } => vec![set_stomp_label(footswitch, label, single)],
            Self::SetMidiOut { source, messages } => vec![set_midi_out(source, messages, false)?],
            Self::SetPresetLoadMidiOut { messages } => vec![set_midi_out(0, messages, true)?],
            Self::SetExpressionBypass {
                row,
                column,
                pedal,
                mode,
                invert,
                delay_ms,
                latch_emulation,
            } => set_expression_bypass(row, column, pedal, mode, invert, delay_ms, latch_emulation),
        })
    }
}

impl DeviceCommand {
    /// Encode a command after validating its public input constraints.
    ///
    /// The unchecked encoder is deliberately private so callers cannot bypass
    /// these validation checks.
    ///
    /// ```compile_fail
    /// use quadcortex::commands::DeviceCommand;
    ///
    /// let _ = DeviceCommand::SelectScene(0).encode_unchecked();
    /// ```
    pub fn try_encode(self) -> Result<OutboundMessage, CommandEncodeError> {
        self.validate_for_encoding()?;
        Ok(self.encode_unchecked())
    }

    fn validate_for_encoding(&self) -> Result<(), CommandEncodeError> {
        let bounded = |field: &'static str,
                       value: u32,
                       upper_exclusive: u32|
         -> Result<(), CommandEncodeError> {
            if value < upper_exclusive {
                Ok(())
            } else {
                Err(CommandEncodeError::UnsupportedValue {
                    field,
                    value: value.to_string(),
                })
            }
        };
        match self {
            Self::SelectScene(scene) => bounded("scene", *scene, domain::SCENE_COUNT)?,
            // Only the measured base modes; the hybrid values 3 to 8 are unmeasured.
            Self::SelectMode(mode) => bounded("mode", *mode, 3)?,
            Self::SetBypass { row, column, .. } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
                bounded("grid column", *column, domain::GRID_COLUMNS)?;
            }
            Self::SetParameterNumeric {
                row, column, value, ..
            } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
                // Columns 8/9 (splitter/mixer) are not addressable through
                // this Model-grid command: parameter_update() unconditionally
                // builds `chain.models[column]`, which would silently write
                // to a nonexistent grid column instead of the splitter/mixer.
                // Splitter/mixer numeric parameters are written through the
                // dedicated DeviceOperation::SetRoutingParameter instead --
                // see device.setParameter's `column >= GRID_COLUMNS` branch
                // in request.rs, which already routes there and never
                // constructs this command with column 8/9.
                bounded("parameter target column", *column, domain::GRID_COLUMNS)?;
                if !value.is_finite() || !(0.0..=1.0).contains(value) {
                    return Err(CommandEncodeError::UnsupportedValue {
                        field: "normalized parameter value",
                        value: value.to_string(),
                    });
                }
            }
            Self::SetParameterText { row, column, .. } => {
                bounded("grid row", *row, domain::GRID_ROWS)?;
                // Splitter/mixer parameters are numeric-only (see
                // SetRoutingParameter and request.rs's "Splitter and mixer
                // parameters use normalized numeric values"), so this
                // text-valued command has no valid splitter/mixer target at
                // all; restrict it to the real Model grid columns.
                bounded("parameter target column", *column, domain::GRID_COLUMNS)?;
            }
            Self::SetlistPosition { position, .. } => {
                bounded("position", *position, 256)?;
            }
            Self::SetTempo(bpm)
                if !(domain::MINIMUM_TEMPO_BPM..=domain::MAXIMUM_TEMPO_BPM).contains(bpm) =>
            {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "preset tempo BPM",
                    value: bpm.to_string(),
                });
            }
            Self::SetMasterVolume(volume)
                if !volume.is_finite() || !(0.0..=1.0).contains(volume) =>
            {
                return Err(CommandEncodeError::UnsupportedValue {
                    field: "normalized master volume",
                    value: volume.to_string(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn encode_unchecked(self) -> OutboundMessage {
        match self {
            Self::SelectScene(scene) => select_scene(scene),
            Self::SelectMode(mode) => select_mode(mode),
            Self::SetBypass {
                row,
                column,
                bypassed,
            } => set_bypass(row, column, bypassed),
            Self::SetParameterNumeric {
                row,
                column,
                parameter_index,
                value,
            } => set_parameter_numeric(row, column, parameter_index, value),
            Self::SetParameterText {
                row,
                column,
                parameter_index,
                value,
            } => set_parameter_text(row, column, parameter_index, value),
            Self::ShowTuner(show) => show_tuner(show),
            Self::ShowGigView(show) => show_gig_view(show),
            Self::SetlistPosition {
                setlist_key,
                position,
                is_factory,
            } => setlist_position(setlist_key, position, is_factory),
            Self::SetTempo(bpm) => set_tempo(bpm),
            Self::SetMasterVolume(volume) => set_master_volume(volume),
            Self::Disconnect => connection(false),
        }
    }
}

/// Build the message that asks the device to reset its communication buffers
/// for the session identified by `session_id`, correlated by `request_id`.
pub fn reset_comms(request_id: u64, session_id: impl Into<String>) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
        pa::ResetCommsBuffersMessage {
            request_id: Some(pa::reset_comms_buffers_message::RequestId::RequestId(
                request_id,
            )),
            session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                session_id.into(),
            )),
        },
    )
}

/// Build the Version UPDATE that announces the host's version string
/// (`profile::CORTEX_CONTROL_VERSION`) to the device.
pub fn version_hello() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_VERSION,
        pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            cortex_control_version: Some(
                pa::version_message::CortexControlVersion::CortexControlVersion(
                    profile::CORTEX_CONTROL_VERSION.into(),
                ),
            ),
            ..Default::default()
        },
    )
}

/// Native-host plumbing for statically enumerated startup/read plans.
///
/// This is intentionally hidden from the public command API: callers must not
/// present an arbitrary message type as a sanctioned device operation. Every
/// concrete use is audited independently.
#[doc(hidden)]
pub fn read(message_type: u16) -> OutboundMessage {
    OutboundMessage::action(message_type, pa::message_action::Enum::Read)
}

/// Read either the recent-products or favorites view. Boot fetches both views;
/// the identical message type does not make them duplicate subscriptions.
pub fn subscribe_recents_favorites(is_favorites: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_RECENTS_FAVORITES,
        pa::RecentsFavoritesMessage {
            action: pa::message_action::Enum::Read as i32,
            is_favorites,
            ..Default::default()
        },
    )
}

/// Start the CPU-load stream. This entry is a CREATE carrying a
/// correlation id, not an ordinary READ; protobuf omits the zero action.
pub fn subscribe_cpu_load(request_id: u64) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_CPU_LOAD,
        pa::CpuLoadMessage {
            request_id: Some(pa::cpu_load_message::RequestId::RequestId(request_id)),
            ..Default::default()
        },
    )
}

/// Stop the CPU-load stream started by [`subscribe_cpu_load`] with the same
/// `request_id`.
pub fn unsubscribe_cpu_load(request_id: u64) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_CPU_LOAD,
        pa::CpuLoadMessage {
            action: pa::message_action::Enum::Delete as i32,
            request_id: Some(pa::cpu_load_message::RequestId::RequestId(request_id)),
            ..Default::default()
        },
    )
}

/// Read the device file catalog during startup, optionally requesting the
/// short listing form.
pub fn read_startup_files(short_listing: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            short_listing: Some(pa::file_message::ShortListing::ShortListing(short_listing)),
            ..Default::default()
        },
    )
}

/// Read the device's cloud transfer state.
pub fn read_cloud_transfer_state() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_CLOUD_PRODUCT,
        pa::CloudProductMessage {
            action: pa::message_action::Enum::Read as i32,
            cloud_transfer_state: Some(
                pa::cloud_product_message::CloudTransferState::CloudTransferState(
                    pa::CloudTransferState::default(),
                ),
            ),
            ..Default::default()
        },
    )
}

/// Request the global EQ state.
pub fn read_global_eq() -> OutboundMessage {
    read(profile::MESSAGE_TYPE_GLOBAL_EQ)
}

/// Bypass (`true`) or engage (`false`) the global EQ.
pub fn set_global_eq_bypassed(bypassed: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GLOBAL_EQ,
        pa::GlobalEqMessage {
            action: pa::message_action::Enum::Update as i32,
            bypassed: Some(pa::global_eq_message::Bypassed::Bypassed(bypassed)),
            ..Default::default()
        },
    )
}

/// Set global EQ parameters, given as `(parameter index, value)` pairs.
/// Returns one message per pair, in order.
pub fn set_global_eq_parameters(parameters: &[(i32, f32)]) -> Vec<OutboundMessage> {
    parameters
        .iter()
        .map(|(parameter_index, value)| {
            OutboundMessage::encoded(
                profile::MESSAGE_TYPE_GLOBAL_EQ,
                pa::GlobalEqMessage {
                    action: pa::message_action::Enum::Update as i32,
                    parameters: vec![pa::GlobalEqParameter {
                        parameter_index: *parameter_index,
                        value: *value,
                    }],
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// Load a model preset into the global EQ, identified by name, factory flag
/// and model hash.
pub fn load_global_eq_model_preset(value: &str, is_factory: bool, hash: u32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GLOBAL_EQ,
        pa::GlobalEqMessage {
            action: pa::message_action::Enum::Update as i32,
            model_preset_to_load: Some(
                pa::global_eq_message::ModelPresetToLoad::ModelPresetToLoad(pa::ModelPresetId {
                    value: Some(pa::model_preset_id::Value::Value(value.to_owned())),
                    is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(is_factory)),
                    hash: Some(pa::model_preset_id::Hash::Hash(hash)),
                }),
            ),
            ..Default::default()
        },
    )
}

/// Request the performance modes currently available for cycling.
pub fn read_mode_cycle() -> OutboundMessage {
    read(profile::MESSAGE_TYPE_MODE)
}

/// Return whether `slots` is representable by the mode editor's constraints:
/// one to three distinct mode values, each `0..=8` (numbered as for
/// [`DeviceCommand::SelectMode`]).
pub fn is_mode_cycle_value(slots: &[u32]) -> bool {
    (1..=3).contains(&slots.len())
        && slots.iter().all(|slot| *slot <= 8)
        && slots
            .iter()
            .enumerate()
            .all(|(index, slot)| !slots[..index].contains(slot))
}

/// Set the performance modes available for cycling.
///
/// The slots are written as given; check them with [`is_mode_cycle_value`]
/// first.
pub fn set_mode_cycle(slots: &[u32]) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_MODE,
        pa::ModeMessage {
            action: pa::message_action::Enum::Update as i32,
            available_modes: Some(pa::mode_message::AvailableModes::AvailableModes(
                pa::AvailableModes {
                    modes: slots.to_vec(),
                },
            )),
            ..Default::default()
        },
    )
}

/// Request the looper status.
pub fn read_looper_status() -> OutboundMessage {
    read(profile::MESSAGE_TYPE_LOOPER)
}

/// Request the recents (`favorites == false`) or favorites
/// (`favorites == true`) list, correlated by `request_id`.
pub fn read_recents_favorites(favorites: bool, request_id: u64) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_RECENTS_FAVORITES,
        pa::RecentsFavoritesMessage {
            action: pa::message_action::Enum::Read as i32,
            request_id: Some(pa::recents_favorites_message::RequestId::RequestId(
                request_id,
            )),
            is_favorites: favorites,
            ..Default::default()
        },
    )
}

/// Add (`favorite == true`) or remove (`favorite == false`) one preset in the
/// favorites list.
pub fn set_favorite(
    name: String,
    folder_key: String,
    product_key: String,
    is_factory: bool,
    is_plugin: bool,
    favorite: bool,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_RECENTS_FAVORITES,
        pa::RecentsFavoritesMessage {
            action: if favorite {
                pa::message_action::Enum::Create as i32
            } else {
                pa::message_action::Enum::Delete as i32
            },
            is_favorites: true,
            items: vec![pa::RecentsFavoritesItem {
                name,
                folder_key,
                folder_name: String::new(),
                is_factory,
                is_plugin,
                product_key,
            }],
            ..Default::default()
        },
    )
}

fn validate_favorite_identity(
    name: &str,
    folder_key: &str,
    product_key: &str,
) -> Result<(), CommandEncodeError> {
    let visible = |field: &'static str, value: &str, maximum: usize| {
        if value.is_empty()
            || value.chars().count() > maximum
            || value.chars().any(char::is_control)
            || value.trim() != value
        {
            Err(CommandEncodeError::UnsupportedValue {
                field,
                value: value.to_string(),
            })
        } else {
            Ok(())
        }
    };
    visible("favorite name", name, 80)?;
    if name.contains(['/', '\\']) || matches!(name, "." | "..") {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "favorite name",
            value: name.to_string(),
        });
    }
    visible("favorite folder key", folder_key, 512)?;
    if folder_key.contains('\\')
        || folder_key.contains("//")
        || folder_key
            .trim_end_matches('/')
            .split('/')
            .filter(|component| !component.is_empty())
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "favorite folder key",
            value: folder_key.to_string(),
        });
    }
    visible("favorite product key", product_key, 512)
}

/// Request the list of pinned models.
pub fn read_pinned_models() -> OutboundMessage {
    read(profile::MESSAGE_TYPE_PINNED_MODELS)
}

/// Pin (`pinned == true`) or unpin (`pinned == false`) one model.
pub fn set_model_pinned(model_id: u32, pinned: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_PINNED_MODELS,
        pa::PinnedModelsMessage {
            action: if pinned {
                pa::message_action::Enum::Create as i32
            } else {
                pa::message_action::Enum::Delete as i32
            },
            models: vec![model_id],
            ..Default::default()
        },
    )
}

/// Read the device's folder catalog using the exact sparse shape the device expects.
pub fn read_folders(omit_factory_content: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            omit_factory_content: Some(pa::file_message::OmitFactoryContent::OmitFactoryContent(
                omit_factory_content,
            )),
            ..Default::default()
        },
    )
}

// Internal extension used for targeted IR/capture queries. The ordinary
// folder-read request does not carry these type/request-ID fields.
fn read_library_files_query(
    _folder_key: String,
    file_type: Option<i32>,
    request_id: Option<u64>,
) -> OutboundMessage {
    // Captures are discovered through an uncorrelated full folder-catalog
    // read. Only typed IR queries use the device extension.
    if file_type.is_none() && request_id.is_none() {
        return read_folders(false);
    }
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            request_id: request_id.map(pa::file_message::RequestId::RequestId),
            r#type: file_type.map(pa::file_message::Type::Type),
            ..Default::default()
        },
    )
}

/// Create a user setlist named `name`.
pub fn create_setlist(name: String) -> OutboundMessage {
    let key = format!("/media/p4/Presets/{name}");
    let parent_key = "/media/p4/Presets".to_owned();
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key)),
                parent_key: Some(pa::folder_info::ParentKey::ParentKey(parent_key)),
                name: Some(pa::folder_info::Name::Name(name)),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Delete the user setlist named `name`.
pub fn delete_setlist(name: String) -> OutboundMessage {
    let key = format!("/media/p4/Presets/{name}");
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Delete as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key)),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                ..Default::default()
            })),
            delete_from_library: Some(pa::file_message::DeleteFromLibrary::DeleteFromLibrary(
                false,
            )),
            ..Default::default()
        },
    )
}

/// Delete one stored preset from a user setlist.
pub fn delete_preset(setlist_key: String, product_key: String) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Delete as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(setlist_key)),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key(product_key)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Move one stored user preset into an empty slot.
///
/// An empty destination is routed through a sparse File MOVE that carries the
/// selected ProductData key, the explicitly present local-source flags, and
/// the target index.
pub fn move_preset(
    setlist_key: String,
    source_product_key: String,
    position: u32,
) -> OutboundMessage {
    relocate_preset(
        pa::message_action::Enum::Move,
        setlist_key,
        source_product_key,
        position,
        true,
    )
}

/// Swap one stored user preset with an occupied slot.
fn swap_preset(setlist_key: String, source_product_key: String, position: u32) -> OutboundMessage {
    relocate_preset(
        pa::message_action::Enum::Swap,
        setlist_key,
        source_product_key,
        position,
        false,
    )
}

fn relocate_preset(
    action: pa::message_action::Enum,
    setlist_key: String,
    source_product_key: String,
    position: u32,
    include_source_is_downloads: bool,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: action as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(setlist_key.clone())),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                is_downloads: include_source_is_downloads
                    .then_some(pa::folder_info::IsDownloads::IsDownloads(false)),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key(source_product_key)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            to_folder: Some(pa::file_message::ToFolder::ToFolder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(setlist_key)),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(position as i32)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Copy one stored preset with Cortex Control's single File COPY frame.
///
/// Local paste pairs one source product key with one destination index under
/// a possibly different destination folder. The host sends no recall or save
/// with it. This is the frame `device.copyPreset` plans; the unit's response to
/// it is not established until a hardware run (see the "Direct rename and copy
/// frames" row of the limits register).
pub fn copy_preset(
    source_setlist_key: String,
    source_product_key: String,
    source_is_factory: bool,
    destination_setlist_key: String,
    destination_position: u32,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Copy as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(source_setlist_key)),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(source_is_factory)),
                // The ordinary local paste route excludes Downloads, but Cortex
                // Control still sends this field as explicitly-present false.
                // Presence is significant for an optional protobuf field.
                is_downloads: Some(pa::folder_info::IsDownloads::IsDownloads(false)),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key(source_product_key)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            to_folder: Some(pa::file_message::ToFolder::ToFolder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(destination_setlist_key)),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(destination_position as i32)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Rename one stored preset with Cortex Control's single File UPDATE frame.
///
/// Renaming preserves the authoritative opaque file key and emits one sparse
/// file-update message containing the new name and the selected preset's
/// instrument metadata. The host sends no recall or save with it. This is the
/// frame `device.renameCurrentPreset` plans; the unit's response to it is not
/// established until a hardware run (see the "Direct rename and copy frames"
/// row of the limits register).
pub fn rename_preset(
    setlist_key: String,
    product_key: String,
    name: String,
    instrument: i32,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Update as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(setlist_key)),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key(product_key)),
                    name: Some(pa::product_data::Name::Name(name)),
                    instrument: Some(pa::product_data::Instrument::Instrument(instrument)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Duplicate one complete setlist using a sparse folder COPY.
///
/// The device derives the destination key and display name. Supplying a host
/// destination would produce a different, unintended command.
pub fn duplicate_setlist(source_setlist_key: String) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Copy as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(source_setlist_key)),
                // This field is sent as packed optional<bool>{false, present}.
                // Presence matters on the wire:
                // omitting it selects a slow/deferred firmware path and
                // publishes an empty destination for longer than the bounded
                // verification window allows.
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Build the Connection message announcing that the host is connected
/// (`true`) or disconnecting (`false`).
pub fn connection(connected: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_CONNECTION,
        pa::ConnectionMessage {
            connected: Some(pa::connection_message::Connected::Connected(connected)),
            ..Default::default()
        },
    )
}

/// The Building-state entry writes. These must not be merged
/// into the later subscription burst: ModelRepo is the gate to Initializing.
pub fn building_initialization() -> Vec<OutboundMessage> {
    vec![connection(true), read(profile::MESSAGE_TYPE_MODEL_REPO)]
}

/// The Initializing-state entry write. A decoded ModuleStats
/// UPDATE is the gate to Booting.
pub fn module_stats_initialization() -> Vec<OutboundMessage> {
    vec![read(profile::MESSAGE_TYPE_MODULE_STATS)]
}

/// The Booting-state entry sequence. Keep this explicit:
/// ordering is behavioral protocol, and the two type-20 reads have different
/// protobuf intent.
pub fn boot_initialization() -> Vec<OutboundMessage> {
    boot_initialization_exact(0, false)
}

/// Build the Booting-state entry sequence with an explicit CPU-load request id
/// and file-listing form.
///
/// [`boot_initialization`] calls this with request id `0` and the full file
/// listing.
pub fn boot_initialization_exact(
    cpu_load_request_id: u64,
    short_file_listing: bool,
) -> Vec<OutboundMessage> {
    vec![
        subscribe_cpu_load(cpu_load_request_id),
        // No License read. It belongs to the cloud-account/license family that
        // rides on a trailer encryption this build does not implement.
        // Issuing it here makes the session encoder refuse it, failing the
        // whole burst before the connection can complete within
        // READY_WAIT_TIMEOUT_MS, and an encrypted reply could not be decoded
        // here either. Restore it only together with that encryption.
        read(profile::MESSAGE_TYPE_UNDO_REDO),
        read(profile::MESSAGE_TYPE_IO_SETTINGS),
        read(profile::MESSAGE_TYPE_GENERAL_SETTINGS),
        read(profile::MESSAGE_TYPE_SHOW_GIG_VIEW),
        read(profile::MESSAGE_TYPE_MODE),
        read(profile::MESSAGE_TYPE_GLOBAL_EQ),
        read(profile::MESSAGE_TYPE_MASTER_VOLUME),
        read_startup_files(short_file_listing),
        read(profile::MESSAGE_TYPE_MODEL_PRESET),
        read_cloud_transfer_state(),
        subscribe_recents_favorites(false),
        subscribe_recents_favorites(true),
        read(profile::MESSAGE_TYPE_COMPILER_INHIBITED_MODULES),
        read(profile::MESSAGE_TYPE_RECALL_PRESET),
        read(profile::MESSAGE_TYPE_NEW_MODELS),
        read(profile::MESSAGE_TYPE_PINNED_MODELS),
        read(profile::MESSAGE_TYPE_DEFAULT_PARAMETERS),
        read(profile::MESSAGE_TYPE_GLOBAL_TEMPO),
        read_setlist_position(),
        read(profile::MESSAGE_TYPE_PRESET_DIRTY),
        read(profile::MESSAGE_TYPE_SCENE),
        read(profile::MESSAGE_TYPE_BULK_OPERATION),
        read(profile::MESSAGE_TYPE_UPDATER),
    ]
}

/// The Disconnected-state entry writes.
pub fn disconnect_initialization() -> Vec<OutboundMessage> {
    disconnect_initialization_exact(0)
}

/// Build the Disconnected-state entry writes with an explicit CPU-load request
/// id. [`disconnect_initialization`] uses request id `0`.
pub fn disconnect_initialization_exact(cpu_load_request_id: u64) -> Vec<OutboundMessage> {
    vec![unsubscribe_cpu_load(cpu_load_request_id), connection(false)]
}

/// Tell the QC what time it is, once per connection.
///
/// The startup sender reserves this message's request id from a shared
/// atomic sequence before serializing and dispatching it. We preserve that
/// lifecycle ordering because a host that knows the time should hand it over
/// without creating a second correlation sequence.
///
/// `ms_since_epoch` is believed to be the host's current time in UTC, in
/// milliseconds since the Unix epoch, with no time-zone offset. That has not
/// been verified on the unit used for this project's tests.
///
/// **What the device does with it is not established.** Type 43 answers no
/// READ, so the clock cannot be read back, and two attempts to observe an
/// effect both failed:
///
/// - IR dates come from the host. An IR imported with the `date` field omitted
///   came back with no date at all, whatever the clock said - so
///   `date_ms_since_epoch` on an IR is the host's string, not the device's clock.
/// - Preset dates ignored it. With the clock set to 2020-01-02, a preset saved
///   seconds later was still stamped with the true wall time, with and without
///   a `request_id` on the sync.
///
/// So do not rely on this to control any timestamp. It is sent for handshake
/// fidelity; the honest summary is that the unit has a time source of its own
/// that this does not appear to override.
pub fn sync_system_time(request_id: u64, ms_since_epoch: u64) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SYSTEM_TIME_SYNC,
        pa::SystemTimeSyncMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::system_time_sync_message::RequestId::RequestId(
                request_id,
            )),
            ms_since_epoch,
        },
    )
}

/// Build the keep-alive message the host sends to show it is still online.
pub fn keepalive() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_KEEP_ALIVE,
        pa::KeepAliveMessage {
            // The keepalive is an explicit correlation-id zero
            // sentinel with UPDATE action. It does not
            // reserve an id from the lifecycle counter.
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::keep_alive_message::RequestId::RequestId(0)),
            is_online: true,
            ..Default::default()
        },
    )
}

/// Request the full current preset with the bare type-15 READ Cortex Control sends at boot:
/// `08 03`, no request id. The unit answers it after boot (measured 2026-10-07, CorOS 4.1.0).
pub fn read_current_preset() -> OutboundMessage {
    read(profile::MESSAGE_TYPE_RECALL_PRESET)
}

/// Read the active setlist address without recalling or reloading a preset.
pub fn read_setlist_position() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SETLIST_POSITION,
        pa::SetlistPositionMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        },
    )
}

/// Select a scene by index.
pub fn select_scene(scene: u32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SCENE,
        pa::SceneMessage {
            action: pa::message_action::Enum::Update as i32,
            selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(scene)),
            ..Default::default()
        },
    )
}

/// Select one exact performance-mode enum value (see [`DeviceCommand::SelectMode`] for the numbering; its checked encoder accepts only `0..=2`).
pub fn select_mode(mode: u32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_MODE,
        pa::ModeMessage {
            action: pa::message_action::Enum::Update as i32,
            mode: Some(pa::mode_message::Mode::Mode(mode)),
            ..Default::default()
        },
    )
}

/// Copy or swap complete scene state, including its label and colour.
pub fn copy_scene(from_index: u32, to_index: u32, swap: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SCENE_COPY,
        pa::SceneCopyMessage {
            action: pa::message_action::Enum::Update as i32,
            from_index: from_index as i32,
            to_index: to_index as i32,
            is_swap: swap,
            ..Default::default()
        },
    )
}

/// Rename a scene. An unlabelled scene is represented as an empty string.
pub fn set_scene_label(scene: u32, label: Option<String>) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SCENE_LABEL,
        pa::SceneLabelMessage {
            action: pa::message_action::Enum::Update as i32,
            index: scene as i32,
            label: label.unwrap_or_default(),
            ..Default::default()
        },
    )
}

/// Set a scene's native ARGB colour value.
pub fn set_scene_color(scene: u32, color: u32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SCENE_COLOR,
        pa::SceneColorMessage {
            action: pa::message_action::Enum::Update as i32,
            index: scene as i32,
            color,
            ..Default::default()
        },
    )
}

/// Set whether the block at `row`/`column` is bypassed.
pub fn set_bypass(row: u32, column: u32, bypassed: bool) -> OutboundMessage {
    let preset = BinaryPreset {
        bypass: vec![Bypass {
            row: Some(bypass::Row::Row(row)),
            col_bypass: vec![ColBypass {
                column: Some(col_bypass::Column::Column(column)),
                scene_bypass: vec![SceneBypass { bypass: bypassed }],
                ..Default::default()
            }],
        }],
        ..Default::default()
    };
    grid_update(preset)
}

/// Set one block parameter to a normalized (`0.0..=1.0`) numeric value.
pub fn set_parameter_numeric(
    row: u32,
    column: u32,
    parameter_index: u32,
    normalized_value: f32,
) -> OutboundMessage {
    parameter_update(
        row,
        column,
        parameter_index,
        param_value::Value::FloatValue(normalized_value),
    )
}

/// Set one block parameter to a string value, such as a file key.
pub fn set_parameter_text(
    row: u32,
    column: u32,
    parameter_index: u32,
    value: impl Into<String>,
) -> OutboundMessage {
    parameter_update(
        row,
        column,
        parameter_index,
        param_value::Value::StringValue(value.into()),
    )
}

fn parameter_update(
    row: u32,
    column: u32,
    parameter_index: u32,
    value: param_value::Value,
) -> OutboundMessage {
    let parameter = Param {
        param_values: vec![ParamValue { value: Some(value) }],
        index: Some(param::Index::Index(parameter_index)),
        ..Default::default()
    };
    let preset = BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            models: vec![Model {
                column: Some(model::Column::Column(column)),
                params: vec![parameter],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    grid_update(preset)
}

fn parameter_metadata_update(
    row: u32,
    column: u32,
    parameter: Param,
    include_mixer_hash: bool,
) -> Result<OutboundMessage, CommandEncodeError> {
    let mut target = Model {
        params: vec![parameter],
        ..Default::default()
    };
    let mut chain = Chain {
        row: Some(chain::Row::Row(row)),
        ..Default::default()
    };
    match column {
        0..=7 => {
            target.column = Some(model::Column::Column(column));
            chain.models.push(target);
        }
        8 => chain.combined_splitter.push(target),
        9 => {
            if include_mixer_hash {
                target.hash = Some(model::Hash::Hash(11_000));
            }
            chain.mixer.push(target);
        }
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "parameter target column",
                value: column.to_string(),
            });
        }
    }
    Ok(grid_update(BinaryPreset {
        chains: vec![chain],
        ..Default::default()
    }))
}

fn expression_target_chain(row: u32, target: ExpressionTarget, model: Model) -> Chain {
    let mut chain = Chain {
        row: Some(chain::Row::Row(row)),
        ..Default::default()
    };
    match target {
        ExpressionTarget::Model { column } => {
            let mut model = model;
            model.column = Some(model::Column::Column(column));
            chain.models.push(model);
        }
        ExpressionTarget::OutputControl => chain.output_control.push(model),
        ExpressionTarget::InputControl => chain.input_control.push(model),
        ExpressionTarget::CombinedSplitter => chain.combined_splitter.push(model),
        ExpressionTarget::Mixer => chain.mixer.push(model),
    }
    chain
}

fn expression_bypass_target_chain(
    row: u32,
    target: ExpressionBypassTarget,
    mut model: Model,
    assignment: bool,
) -> Chain {
    let mut chain = Chain {
        row: Some(chain::Row::Row(row)),
        ..Default::default()
    };
    match target {
        ExpressionBypassTarget::Model { column } => {
            model.column = Some(model::Column::Column(column));
            chain.models.push(model);
        }
        ExpressionBypassTarget::OutputControl => chain.output_control.push(model),
        ExpressionBypassTarget::InputControl => {
            // The assignment frame addresses the input control with column 1
            // and the settings frame leaves the column out. No recorded
            // hardware run assigns an expression bypass to an input control, so
            // the value 1 is untested against a unit and its meaning is not
            // established here.
            if assignment {
                model.column = Some(model::Column::Column(1));
            }
            chain.input_control.push(model);
        }
        ExpressionBypassTarget::Routing => chain.mixer.push(model),
    }
    chain
}

fn lane_control_update(
    row: u32,
    control: &str,
    parameter: Param,
) -> Result<OutboundMessage, CommandEncodeError> {
    let target = Model {
        params: vec![parameter],
        ..Default::default()
    };
    let mut chain = Chain {
        row: Some(chain::Row::Row(row)),
        ..Default::default()
    };
    match control {
        "inputGate" => chain.input_control.push(target),
        "laneOutput" => chain.output_control.push(target),
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "lane control",
                value: control.to_owned(),
            });
        }
    }
    Ok(grid_update(BinaryPreset {
        chains: vec![chain],
        ..Default::default()
    }))
}

/// Set a numeric parameter of a lane's input gate (`inputGate`) or lane output
/// (`laneOutput`) control.
///
/// # Errors
///
/// Returns [`CommandEncodeError::UnsupportedValue`] for any other `control`.
pub fn set_lane_control_parameter(
    row: u32,
    control: &str,
    parameter_index: u32,
    value: f32,
) -> Result<OutboundMessage, CommandEncodeError> {
    lane_control_update(
        row,
        control,
        Param {
            index: Some(param::Index::Index(parameter_index)),
            param_values: vec![ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        },
    )
}

/// Enable or disable per-scene storage for a lane-control parameter.
///
/// # Errors
///
/// Returns [`CommandEncodeError::UnsupportedValue`] when `control` is neither
/// `inputGate` nor `laneOutput`.
pub fn set_lane_control_scene_mode(
    row: u32,
    control: &str,
    parameter_index: u32,
    enabled: bool,
) -> Result<OutboundMessage, CommandEncodeError> {
    lane_control_update(
        row,
        control,
        Param {
            index: Some(param::Index::Index(parameter_index)),
            scene_mode: Some(param::SceneMode::SceneMode(enabled)),
            ..Default::default()
        },
    )
}

/// Toggle per-scene storage for one block parameter. The flag travels alone.
pub fn set_parameter_scene_mode(
    row: u32,
    column: u32,
    parameter_index: u32,
    enabled: bool,
) -> Result<OutboundMessage, CommandEncodeError> {
    parameter_metadata_update(
        row,
        column,
        Param {
            index: Some(param::Index::Index(parameter_index)),
            scene_mode: Some(param::SceneMode::SceneMode(enabled)),
            ..Default::default()
        },
        false,
    )
}

/// Assign EXP 1/2 to a block parameter, or clear it with pedal zero.
pub fn set_parameter_expression(
    row: u32,
    column: u32,
    parameter_index: u32,
    pedal: u32,
    minimum: f32,
    maximum: f32,
) -> Result<OutboundMessage, CommandEncodeError> {
    let target = match column {
        0..=7 => ExpressionTarget::Model { column },
        8 => ExpressionTarget::CombinedSplitter,
        9 => ExpressionTarget::Mixer,
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "parameter target column",
                value: column.to_string(),
            });
        }
    };
    Ok(set_parameter_expression_target(
        row,
        target,
        parameter_index,
        pedal,
        minimum,
        maximum,
    ))
}

/// Assign EXP 1/2 to an input-gate or lane-output parameter.
///
/// These route through the same expression builder as block parameters,
/// selected by the lane-control model hashes rather than a grid column.
pub fn set_lane_control_expression(
    row: u32,
    control: &str,
    parameter_index: u32,
    pedal: u32,
    minimum: f32,
    maximum: f32,
) -> Result<OutboundMessage, CommandEncodeError> {
    let target = match control {
        "inputGate" => ExpressionTarget::InputControl,
        "laneOutput" => ExpressionTarget::OutputControl,
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "lane control",
                value: control.to_owned(),
            });
        }
    };
    Ok(set_parameter_expression_target(
        row,
        target,
        parameter_index,
        pedal,
        minimum,
        maximum,
    ))
}

fn set_parameter_expression_target(
    row: u32,
    target: ExpressionTarget,
    parameter_index: u32,
    pedal: u32,
    minimum: f32,
    maximum: f32,
) -> OutboundMessage {
    let model = Model {
        params: vec![Param {
            index: Some(param::Index::Index(parameter_index)),
            expression: Some(param::Expression::Expression(pedal as i32)),
            expression_min: Some(param::ExpressionMin::ExpressionMin(minimum)),
            expression_max: Some(param::ExpressionMax::ExpressionMax(maximum)),
            ..Default::default()
        }],
        ..Default::default()
    };
    grid_update(BinaryPreset {
        chains: vec![expression_target_chain(row, target, model)],
        ..Default::default()
    })
}

/// Assign an expression pedal to a placed block's bypass switch.
pub fn set_expression_bypass(
    row: u32,
    column: u32,
    pedal: u32,
    mode: u32,
    invert: bool,
    delay_ms: u32,
    latch_emulation: bool,
) -> Vec<OutboundMessage> {
    assert!(
        column <= 7,
        "unsupported expression-bypass target column: {column}"
    );
    set_expression_bypass_for_model(
        row,
        0,
        column,
        pedal,
        mode,
        invert,
        delay_ms,
        latch_emulation,
    )
}

/// Encode the model-hash-specific expression-bypass conversion exactly.
///
/// Higher-level actions should use [`set_expression_bypass`]. This low-level
/// builder exists so binary-only lane/routing targets retain their actual wire
/// semantics without overloading the public grid-column action.
#[doc(hidden)]
// Mirrors the parameter tuple exactly; grouping these values in a
// second options object would obscure the wire argument mapping.
#[allow(clippy::too_many_arguments)]
pub fn set_expression_bypass_for_model(
    row: u32,
    model_hash: u32,
    column: u32,
    pedal: u32,
    mode: u32,
    invert: bool,
    delay_ms: u32,
    latch_emulation: bool,
) -> Vec<OutboundMessage> {
    let target = match model_hash {
        23_000 => ExpressionBypassTarget::OutputControl,
        28_000 => ExpressionBypassTarget::InputControl,
        10_004 | 11_000 => ExpressionBypassTarget::Routing,
        _ => ExpressionBypassTarget::Model { column },
    };
    set_expression_bypass_target(row, target, pedal, mode, invert, delay_ms, latch_emulation)
}

fn set_expression_bypass_target(
    row: u32,
    target: ExpressionBypassTarget,
    pedal: u32,
    mode: u32,
    invert: bool,
    delay_ms: u32,
    latch_emulation: bool,
) -> Vec<OutboundMessage> {
    let assignment = Model {
        bypass_expression: vec![crate::proto::Expression {
            expression: pedal as i32,
            expression_min: 0.0,
            expression_max: 1.0,
        }],
        ..Default::default()
    };
    let info = Model {
        expression_bypass_info: vec![crate::proto::ExpressionBypassInfo {
            r#type: mode,
            invert,
            delay_ms,
            latch_emulation,
        }],
        ..Default::default()
    };
    let assignment = grid_update(BinaryPreset {
        chains: vec![expression_bypass_target_chain(
            row, target, assignment, true,
        )],
        ..Default::default()
    });
    let info = grid_update(BinaryPreset {
        chains: vec![expression_bypass_target_chain(row, target, info, false)],
        ..Default::default()
    });
    vec![assignment, info]
}

/// Place a block with the given ordinary model at a grid position.
///
/// # Panics
///
/// Panics for the Neural Capture models; use [`set_block_with_initial_value`]
/// for those.
pub fn set_block(row: u32, column: u32, model_id: u32) -> OutboundMessage {
    assert!(
        !is_neural_capture_model(model_id),
        "model {model_id} requires set_block_with_initial_value"
    );
    set_block_with_initial_value(row, column, model_id, None)
}

/// Place a block, optionally writing an initial value to its parameter 5.
///
/// # Panics
///
/// Panics unless `initial_value` is `Some` exactly when `model_id` is a Neural
/// Capture model (14000 or 14001).
pub fn set_block_with_initial_value(
    row: u32,
    column: u32,
    model_id: u32,
    initial_value: Option<ParamValue>,
) -> OutboundMessage {
    assert_eq!(
        is_neural_capture_model(model_id),
        initial_value.is_some(),
        "only models 14000/14001 carry addModel parameter 5"
    );
    let params = initial_value
        .into_iter()
        .map(|value| Param {
            index: Some(param::Index::Index(5)),
            param_values: vec![value],
            ..Default::default()
        })
        .collect();
    grid_update(BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            models: vec![Model {
                hash: Some(model::Hash::Hash(model_id)),
                column: Some(model::Column::Column(column)),
                params,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// Remove the block at a grid position.
pub fn remove_block(row: u32, column: u32) -> OutboundMessage {
    let preset = BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            models: vec![Model {
                column: Some(model::Column::Column(column)),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GRID,
        pa::GridMessage {
            action: pa::message_action::Enum::Delete as i32,
            preset: Some(pa::grid_message::Preset::Preset(preset)),
            ..Default::default()
        },
    )
}

/// Reset every parameter on one Grid block to the defaults stored by CorOS.
///
/// Cortex Control's `DefaultParametersMessageSender` emits UPDATE with only
/// the selected row and column for this menu action. CREATE is the distinct
/// "Set Parameters as Defaults" operation and must not be substituted here.
pub fn reset_block_parameters(row: u32, column: u32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_DEFAULT_PARAMETERS,
        pa::DefaultParametersMessage {
            action: pa::message_action::Enum::Update as i32,
            row: row as i32,
            column: column as i32,
            ..Default::default()
        },
    )
}

/// Request creation of a user device model preset from the settings of the
/// Grid block at row and column, by sending one type-71 ModelPreset CREATE
/// frame. The frame names the block (row, column and model_hash) and carries
/// a new user entry called name; the unit is believed, not shown, to fill the
/// preset body from that block.
///
/// This builder only encodes the frame. The host planner requires
/// expected_preset_identity and compares it with the current preset, and it
/// requires model_hash to equal the model of the block at row and column. The
/// published action also requires an explicit persistent-write confirmation.
/// Nothing is read back, so the host does not verify that the unit created the
/// preset or what it holds.
pub fn create_model_preset(
    row: u32,
    column: u32,
    model_hash: u32,
    name: String,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_MODEL_PRESET,
        pa::ModelPresetMessage {
            action: pa::message_action::Enum::Create as i32,
            presets: vec![pa::ModelPreset {
                id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                    value: Some(pa::model_preset_id::Value::Value(name.clone())),
                    is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                    hash: Some(pa::model_preset_id::Hash::Hash(model_hash)),
                })),
                name: Some(pa::model_preset::Name::Name(name)),
                is_default: Some(pa::model_preset::IsDefault::IsDefault(false)),
                ..Default::default()
            }],
            create_from_row: Some(pa::model_preset_message::CreateFromRow::CreateFromRow(
                row as i32,
            )),
            create_from_column: Some(
                pa::model_preset_message::CreateFromColumn::CreateFromColumn(column as i32),
            ),
            create_from_hash: Some(pa::model_preset_message::CreateFromHash::CreateFromHash(
                model_hash as i32,
            )),
            ..Default::default()
        },
    )
}

/// Replace all eight Looper X footswitch assignments in physical A-H order.
///
/// `assignments[i]` is the raw action index on footswitch `A + i`, sent
/// unchanged: eight integers that together form a permutation of `0..=7`. The
/// arrangement read back from the unit in this project's runs was
/// `[3, 4, 5, 6, 0, 1, 7, 2]`.
///
/// What each index means was observed on a CorOS 4.1.0 unit, 2026-10-06:
/// writing `[0, 1, 2, 3, 4, 5, 6, 7]` made the unit's Looper X editor show
/// footswitches A to H as Record, Play, Undo, Duplicate, One Shot, Half Speed,
/// Punch In and Reverse. So `0` is Record, `1` Play, `2` Undo, `3` Duplicate,
/// `4` One Shot, `5` Half Speed, `6` Punch In and `7` Reverse; the arrangement
/// read back above shows A to H as Duplicate, One Shot, Half Speed, Punch In,
/// Record, Play, Reverse and Undo. The unit's labels for the secondary states
/// (Overdub, Redo, Re-Loop, Punch Out) are believed, not observed, to be what
/// the same controls show while a loop is playing, after an undo, while
/// Duplicate is active and while punched in.
pub fn set_looper_stomp_assignments(assignments: Vec<i32>) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            looper_stomp_assignments: assignments,
            ..Default::default()
        },
    )
}

/// Move a block between grid cells.
///
/// Sends the bare frame: field 2 move entries only, with no field-3 grid
/// model-id sidecar. Hardware experiment 2026-10-06 showed the device applies
/// the bare frame in 4/4 arms but ignored the sidecar form in 2/4.
/// `grid_model_ids` is still accepted (and validated by the caller) but is not
/// sent.
pub fn move_block(
    from_row: u32,
    from_column: u32,
    to_row: u32,
    to_column: u32,
    _grid_model_ids: Vec<Vec<u32>>,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GRID_MOVE,
        pa::GridMoveMessage {
            r#move: vec![pa::GridMoveElement {
                from_row,
                from_col: from_column,
                to_row,
                to_col: to_column,
                is_drop: true,
            }],
            ..Default::default()
        },
    )
}

/// Assign a block to a footswitch (or clear the assignment with `None`),
/// without removing any earlier assignment.
pub fn set_footswitch_typed(
    row: u32,
    column: u32,
    footswitch: Option<u32>,
    assignment_type: StompAssignmentType,
) -> Vec<OutboundMessage> {
    set_footswitch_transition(row, column, footswitch, assignment_type, None, None)
}

/// Move a block's footswitch assignment from a previous footswitch and type to
/// a new one.
///
/// When a previous assignment is given it is deleted first; a `None`
/// `footswitch` then leaves the block unassigned. Returns no messages when the
/// requested assignment already matches the previous one.
pub fn set_footswitch_transition(
    row: u32,
    column: u32,
    footswitch: Option<u32>,
    assignment_type: StompAssignmentType,
    previous_footswitch: Option<u32>,
    previous_assignment_type: Option<StompAssignmentType>,
) -> Vec<OutboundMessage> {
    if footswitch == previous_footswitch
        && (footswitch.is_none() || Some(assignment_type) == previous_assignment_type)
    {
        return Vec::new();
    }
    let mut messages = Vec::with_capacity(2);
    if let (Some(previous_footswitch), Some(previous_assignment_type)) =
        (previous_footswitch, previous_assignment_type)
    {
        messages.push(OutboundMessage::encoded(
            profile::MESSAGE_TYPE_GRID,
            pa::GridMessage {
                action: pa::message_action::Enum::Delete as i32,
                preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                    stomp_mode_assignments: vec![StompModeAssignment {
                        row,
                        column,
                        stomp_index: previous_footswitch,
                        r#type: Some(crate::proto::stomp_mode_assignment::TypeOneOf::Type(
                            previous_assignment_type.wire(),
                        )),
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            },
        ));
    }
    let Some(footswitch) = footswitch else {
        return messages;
    };
    let update = grid_update(BinaryPreset {
        stomp_mode_assignments: vec![StompModeAssignment {
            row,
            column,
            stomp_index: footswitch,
            r#type: Some(crate::proto::stomp_mode_assignment::TypeOneOf::Type(
                assignment_type.wire(),
            )),
        }],
        ..Default::default()
    });
    messages.push(update);
    messages
}

/// Set the per-preset latching/momentary behavior for a physical A-H switch.
pub fn set_stomp_momentary(footswitch: u32, momentary: bool) -> OutboundMessage {
    let mut preset = BinaryPreset::default();
    preset.stomp_is_momentary.insert(footswitch, momentary);
    grid_update(preset)
}

/// Set one of the QC's two footswitch-label maps.
pub fn set_stomp_label(footswitch: u32, label: String, single: bool) -> OutboundMessage {
    let mut preset = BinaryPreset::default();
    if single {
        preset.single_stomp_labels.insert(footswitch, label);
    } else {
        preset.stomp_labels.insert(footswitch, label);
    }
    grid_update(preset)
}

/// Replace the MIDI messages for one source, or the preset-load list.
///
/// Sources `0..=7` are footswitches A-H, source `8` is EXP 1 and source `9` is
/// EXP 2 (observed on a CorOS 4.1.0 unit, 2026-10-06). The message type is a
/// raw wire number; its observed meaning, the parameter roles and a hazard for
/// undefined types are in [`set_midi_out_selector`].
///
/// The preset-load list accepts the same message types as every other list
/// (observed on a CorOS 4.1.0 unit, 2026-10-06).
///
/// # Errors
///
/// Returns [`CommandEncodeError::UnsupportedValue`], and encodes nothing, for
/// anything [`set_midi_out_selector`] refuses, including a `source` outside
/// [`MIDI_OUT_SOURCES`] for a non-preset-load list.
pub fn set_midi_out(
    source: u32,
    messages: Vec<MidiOutMessage>,
    preset_load: bool,
) -> Result<OutboundMessage, CommandEncodeError> {
    let selector = if preset_load {
        0
    } else {
        source.saturating_add(1)
    };
    set_midi_out_selector(selector, messages)
}

/// This selector is encoded literally: zero is the preset-load list and
/// positive values select general MIDI source `selector - 1`, so `1..=8` are
/// footswitches A-H, `9` is EXP 1 and `10` is EXP 2 (observed on a CorOS 4.1.0
/// unit, 2026-10-06: the unit's Preset MIDI Out screen lit those sources).
///
/// In each message, `channel` is `1..=16` and the three parameters are
/// `0..=127`. The message `type` is a raw wire number (an unsigned 32-bit
/// integer) sent unchanged. Observed on a CorOS 4.1.0 unit, 2026-10-06 (raw
/// frames written to a scratch preset, then the unit's own Preset MIDI Out
/// screen read):
///
/// - `0` is None: the slot is empty, and this function sends it as an empty
///   message in the position it was given, so the messages after it keep
///   their indices.
/// - `1` is Control Change: `param1` is the CC number and `param2` the value;
///   the screen shows no third value.
/// - `2` is Control Change Toggle: `param1` is the CC number, `param2` the
///   minimum value and `param3` the maximum value.
/// - `3` is Program Change: `param1` is the bank CC#0 value, `param2` the bank
///   CC#32 value and `param3` the program number.
///
/// A source holds up to twelve messages, which the unit shows as three pages of
/// four rows in the order sent; an empty list clears the source, and replacing a
/// longer list with a shorter one leaves only the shorter list. Those writes
/// took effect on screen at once and did not mark the preset modified.
///
/// Observed on the same unit on 2026-10-07 (live read, then a save and recall of
/// a scratch slot): a stored preset keeps the general messages in a 120-slot
/// list where source N occupies slots N * 12 to N * 12 + 11, and a shorter
/// legacy list mirrors only the first message of each source. The preset-load
/// messages are stored in a separate twelve-slot list that preserves positions,
/// so a type `0` row between two active rows stays where it was. A type `1`
/// message keeps `param3` in both the live and the stored bytes, which were
/// identical.
///
/// **Hazard (observed once, cause not proven):** writing an undefined type
/// (type `4`) was followed within seconds by the USB device vanishing for about
/// 25 seconds and re-enumerating with the unit restarted on its saved preset.
/// Send only `0..=3`. [`DeviceOperation::SetMidiOut`] checks the type, and
/// this function refuses an undefined type before encoding anything.
///
/// The preset-load list (selector `0`) accepted types `1`, `2` and `3` on the
/// same unit, with the same labels as the per-source lists.
///
/// # Errors
///
/// Returns [`CommandEncodeError::UnsupportedValue`], and encodes nothing, when
/// the selector names a source outside [`MIDI_OUT_SOURCES`], when there are
/// more than [`MIDI_OUT_SLOTS_PER_SOURCE`] messages, or when a message has a
/// type outside [`MIDI_OUT_TYPE_NONE`] and [`MIDI_OUT_TYPES`], a channel above
/// [`MIDI_OUT_CHANNELS`] (or below it, for a non-empty type) or a parameter
/// above [`MIDI_OUT_PARAMETER_MAX`]. Empty slots keep their positions; nothing
/// is compacted, reordered or padded beyond the twelve-slot frame.
pub fn set_midi_out_selector(
    selector: u32,
    messages: Vec<MidiOutMessage>,
) -> Result<OutboundMessage, CommandEncodeError> {
    validate_midi_out_selector(selector, &messages)?;
    let preset_load = selector == 0;
    let mut messages = messages
        .into_iter()
        .map(|message| {
            if message.r#type == 0 {
                crate::proto::MidiMessageInfo::default()
            } else {
                crate::proto::MidiMessageInfo {
                    r#type: message.r#type,
                    channel: message.channel,
                    param1: message.param1,
                    param2: message.param2,
                    param3: message.param3,
                }
            }
        })
        .take(MIDI_OUT_SLOTS_PER_SOURCE)
        .collect::<Vec<_>>();
    messages.resize_with(MIDI_OUT_SLOTS_PER_SOURCE, Default::default);
    let messages = pa::GeneralMidiMessages {
        messages: vec![pa::GeneralMidiMessage {
            source: if preset_load {
                None
            } else {
                Some(pa::general_midi_message::Source::Source(selector - 1))
            },
            msg: messages,
        }],
    };
    Ok(OutboundMessage::encoded(
        profile::MESSAGE_TYPE_MIDI_SETTINGS,
        pa::MidiSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            preset_load_messages: preset_load.then_some(
                pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(messages.clone()),
            ),
            general_midi_messages: (!preset_load).then_some(
                pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(messages),
            ),
            ..Default::default()
        },
    ))
}

/// The checks [`set_midi_out_selector`] applies before it encodes anything: the
/// selector must name the preset-load list or a source in [`MIDI_OUT_SOURCES`],
/// and the messages pass the same check as the typed operations, which accepts
/// an empty (type [`MIDI_OUT_TYPE_NONE`]) row at any position.
fn validate_midi_out_selector(
    selector: u32,
    messages: &[MidiOutMessage],
) -> Result<(), CommandEncodeError> {
    if selector != 0 && !MIDI_OUT_SOURCES.contains(&(selector - 1)) {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "MIDI Out selector",
            value: selector.to_string(),
        });
    }
    validate_midi_out_messages(messages)
}

fn validate_midi_out_messages(messages: &[MidiOutMessage]) -> Result<(), CommandEncodeError> {
    if messages.len() > MIDI_OUT_SLOTS_PER_SOURCE {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "MIDI Out message count",
            value: messages.len().to_string(),
        });
    }
    for (index, message) in messages.iter().enumerate() {
        if !midi_out_message_is_valid(message) {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "MIDI Out message",
                value: index.to_string(),
            });
        }
    }
    Ok(())
}

/// Set a chain row's input route.
pub fn set_chain_input(row: u32, input_id: u32) -> OutboundMessage {
    grid_update(BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            in_portid: Some(chain::InPortid::InPortid(input_id)),
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// Set a chain row's output route.
pub fn set_chain_output(row: u32, output_id: u32) -> OutboundMessage {
    grid_update(BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            out_portid: Some(chain::OutPortid::OutPortid(output_id)),
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// Set the parallel-routing split and mix columns of a chain row. `None`
/// clears the corresponding point.
pub fn set_chain_split(
    row: u32,
    split_column: Option<i32>,
    mix_column: Option<i32>,
) -> OutboundMessage {
    let split = split_column.unwrap_or(-1);
    let mix = mix_column.unwrap_or(-1);
    grid_update(BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            split_control_points: vec![SplitControlPoints { split, mix }],
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// The selected scene's mute state is written through `splitBypass` and its
/// authoritative value is reported through that scene's `mixBypass` entry.
pub fn set_split_mute(row: u32, muted: bool) -> OutboundMessage {
    grid_update(BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            split_bypass: vec![SceneBypass { bypass: muted }],
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// Set a numeric parameter of a splitter or mixer node.
///
/// `node` is `splitter`, `legacySplitter` or `mixer`.
///
/// # Errors
///
/// Returns [`CommandEncodeError::UnsupportedValue`] for any other `node`.
pub fn set_routing_parameter(
    row: u32,
    node: &str,
    parameter_index: u32,
    value: f32,
) -> Result<OutboundMessage, CommandEncodeError> {
    let parameter = Param {
        param_values: vec![ParamValue {
            value: Some(param_value::Value::FloatValue(value)),
        }],
        index: Some(param::Index::Index(parameter_index)),
        ..Default::default()
    };
    let mut chain_value = Chain {
        row: Some(chain::Row::Row(row)),
        ..Default::default()
    };
    match node {
        "splitter" => chain_value.combined_splitter.push(Model {
            params: vec![parameter],
            ..Default::default()
        }),
        "legacySplitter" => chain_value.splitter.push(Model {
            params: vec![parameter],
            ..Default::default()
        }),
        "mixer" => chain_value.mixer.push(Model {
            params: vec![parameter],
            ..Default::default()
        }),
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "routing node",
                value: node.to_owned(),
            });
        }
    }
    Ok(grid_update(BinaryPreset {
        chains: vec![chain_value],
        ..Default::default()
    }))
}

/// Save the current preset into slot `position` of the setlist `setlist_key`
/// under `name`, with the given instrument code.
pub fn save_preset(
    setlist_key: impl Into<String>,
    position: u32,
    name: impl Into<String>,
    instrument: i32,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(setlist_key.into())),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(position as i32)),
                    name: Some(pa::product_data::Name::Name(name.into())),
                    instrument: Some(pa::product_data::Instrument::Instrument(instrument)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
    )
}

/// Show (`true`) or hide (`false`) the tuner.
///
/// Observed on a CorOS 4.1.0 unit, 2026-10-07: this message does nothing. No
/// tuner screen appears, and the unit has no tuner view class to show. The frame
/// is still sent for fidelity with the vendor's own sequence.
pub fn show_tuner(show: bool) -> OutboundMessage {
    // Preserve the explicit false field. Proto3 would
    // otherwise omit it, which makes a hide command indistinguishable from a
    // message whose sender never supplied the `show` field.
    OutboundMessage {
        message_type: profile::MESSAGE_TYPE_SHOW_TUNER,
        payload: vec![0x08, 0x01, 0x18, u8::from(show)],
    }
}

/// The ordered tuner-selection workflow: show/hide the tuner, update its
/// meter enablement, then read tuner state.
///
/// Tuner visibility is tracked separately from a dedicated tuner view, and
/// the meter is enabled while either that view or the root tuner selection
/// is visible. Native public hosts have no independent dedicated tuner view,
/// so their public operation supplies `false` for `live_tuner_visible`.
pub fn show_tuner_workflow(show: bool, live_tuner_visible: bool) -> [OutboundMessage; 3] {
    [
        show_tuner(show),
        set_tuner_meter(show || live_tuner_visible),
        read_tuner(),
    ]
}

/// Show (`true`) or hide (`false`) Gig View.
///
/// Observed on a CorOS 4.1.0 unit, 2026-10-07: Gig View hides on both the bare
/// frame `08 01` and the explicit frame `08 01 18 00`, because the field is a
/// plain proto3 boolean, so the explicit false is not needed on the wire. This
/// function still sends the explicit form.
pub fn show_gig_view(show: bool) -> OutboundMessage {
    OutboundMessage {
        message_type: profile::MESSAGE_TYPE_SHOW_GIG_VIEW,
        payload: vec![0x08, 0x01, 0x18, u8::from(show)],
    }
}

/// Reply to the QC's request for a host-owned Neural Capture dialog.
///
/// This exactly replaces the show-capture-dialog wire operation,
/// but is intentionally not an intent-level [`DeviceCommand`]. Sending `true`
/// transfers the capture flow to the host; a client must not do that unless it
/// implements the recorder, trainer, refiner, calibration, A/B, and save UI.
pub fn acknowledge_capture_dialog(shown: bool) -> OutboundMessage {
    neural_capture_dialog(true, shown)
}

/// Encode both booleans captured by the `showDialog` sender. When
/// `include_show_dialog` is false, field 5 is absent regardless of `shown`.
pub fn neural_capture_dialog(include_show_dialog: bool, shown: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_NEURAL_CAPTURE,
        pa::NeuralCaptureMessage {
            action: pa::message_action::Enum::Update as i32,
            show_dialog: include_show_dialog
                .then_some(pa::neural_capture_message::ShowDialog::ShowDialog(shown)),
            ..Default::default()
        },
    )
}

/// Request the device's version information.
pub fn read_version() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_VERSION,
        pa::VersionMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        },
    )
}

/// Request the current tuner state.
pub fn read_tuner() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_TUNER,
        pa::TunerMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        },
    )
}

/// Select the tuner input: one TunerMessage UPDATE (`08 01 18 <id>`), the
/// frame Cortex Control sends, and nothing after it. Hosts never read a tuner
/// write back; Cortex Control does not either. That a tuner write engages the
/// tuner without showing it is BELIEVED (an undated measurement, not CC
/// static analysis), so hosts keep a confirmation guard before sending it.
pub fn set_tuner_input(input_port_id: i32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_TUNER,
        pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(input_port_id)),
            ..Default::default()
        },
    )
}

/// Input connector IDs exposed by the tuner input menu.
pub const TUNER_INPUT_PORT_IDS: &[i32] = &[1, 2, 3, 4, 5, 8, 9];

/// Returns `true` when `value` is one of [`TUNER_INPUT_PORT_IDS`].
pub fn is_tuner_input_port_id(value: i32) -> bool {
    TUNER_INPUT_PORT_IDS.contains(&value)
}

/// Set the persistent mute-while-tuning preference: one sparse TunerMessage
/// UPDATE (`08 01 28 00|01`), never read back. Firmware may engage tuning on
/// a direct write (BELIEVED); callers therefore retain the explicit
/// activation warning before `true` can silence every output. Unmuting is the
/// same frame with `false`; there is no separate restore operation.
pub fn set_tuner_mute(muted: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_TUNER,
        pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            mute: Some(pa::tuner_message::Mute::Mute(muted)),
            ..Default::default()
        },
    )
}

/// Turn the tuner's live meter push on or off.
///
/// The required tuner-selection workflow uses this as the middle step: after
/// the local selection update it sends a show/hide tuner update, this sparse
/// meter update, and a tuner read in order. The shared public operation
/// exposes only this sender-level update and therefore emits exactly one frame.
///
/// Whether this engages the tuner or changes firmware reporting is not
/// established by this protocol. Callers retain the conservative tuner safety
/// acknowledgement used by the other tuner writes.
pub fn set_tuner_meter(enabled: bool) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_TUNER,
        pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            enable_meter: Some(pa::tuner_message::EnableMeter::EnableMeter(enabled)),
            ..Default::default()
        },
    )
}

/// Set reference pitch as the signed Hz offset from A=440, matching the QC
/// wire representation (for example `2.0` means 442 Hz).
pub fn set_tuner_reference(offset_hz: f32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_TUNER,
        pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            frequency: Some(pa::tuner_message::Frequency::Frequency(offset_hz)),
            ..Default::default()
        },
    )
}

/// The number of brightness steps the device settings UI publishes.
pub const BRIGHTNESS_STEPS: i32 = 32;

/// The top of the brightness wire domain, which is a percentage-style value from
/// `0` to `100` (observed on a CorOS 4.1.0 unit, 2026-10-06, for the screen and
/// LED fields; the dimmed LED field was not tried).
pub const BRIGHTNESS_WIRE_MAX: i32 = 100;

/// Return whether a Device Setting is one of the three scaled brightnesses.
pub fn is_brightness_setting(setting: &str) -> bool {
    matches!(
        setting,
        "screenBrightness" | "ledBrightness" | "dimmedLedBrightness"
    )
}

/// Scale a public brightness step onto the wire field, which is a percentage-style
/// value from `0` to `100`.
///
/// The published domain every host speaks - and the value
/// `responses::decode_general_settings` projects - is a step, `0..=32`. The wire
/// fields (`screen_brightness`, `led_brightness`, `dimmed_led_brightness`) are
/// `0..=100` and the unit shows `round(raw * 32 / 100)` steps of 32, so a step is
/// `BRIGHTNESS_WIRE_MAX / BRIGHTNESS_STEPS` = 3.125 percent. Observed on a
/// CorOS 4.1.0 unit, 2026-10-06 (raw values written, the Settings brightness
/// page read): screen raw 25, 50 and 75 showed 8, 16 and 24; LED raw 50 and 100
/// showed 16 and 32. The dimmed LED field was not tried and is assumed to share
/// the scale. The raw wire values are published beside the steps
/// (`screen_brightness_wire` and its siblings).
///
/// Returning the centre of each step's pre-image rather than its lower edge
/// keeps the value away from the quantizer's boundaries: every step in every
/// published domain survives a write followed by the read projection, which
/// `every_brightness_value_read_from_the_device_can_be_written_back` walks
/// exhaustively.
pub fn brightness_step_to_wire(step: i32) -> i32 {
    (f64::from(step) * f64::from(BRIGHTNESS_WIRE_MAX) / f64::from(BRIGHTNESS_STEPS)).round() as i32
}

/// Return whether `value` is accepted for the integer Device Setting `setting`.
///
/// Accepted settings and ranges: `screenBrightness` `1..=32`,
/// `ledBrightness` `0..=32`, `dimmedLedBrightness` `0..=31`, `holdTiming`
/// `0..=5`, `midiChannel` `1..=17` and `midiClockOut` `0..=3`. Any other
/// setting name is rejected.
///
/// `midiChannel` and `holdTiming` are sent exactly as given. Observed on a
/// CorOS 4.1.0 unit, 2026-10-06: `midiChannel` `1`, `2` and `16` show as
/// channels 1, 2 and 16, and `17` shows as OMNI; `holdTiming` `0`, `3` and `5`
/// show as 500, 800 and 1000 ms, so index `n` is `500 + 100 * n` ms.
///
/// `midiClockOut` is validated here rather than as a toggle because it is
/// backed by a four-value enum (off, MIDI DIN only, USB MIDI only, both USB
/// and DIN MIDI), not a two-state boolean.
pub fn is_general_integer_value(setting: &str, value: i32) -> bool {
    match setting {
        "screenBrightness" => (1..=32).contains(&value),
        "ledBrightness" => (0..=32).contains(&value),
        "dimmedLedBrightness" => (0..=31).contains(&value),
        "holdTiming" => (0..=5).contains(&value),
        "midiChannel" => GENERAL_MIDI_CHANNELS.contains(&value),
        "midiClockOut" => GENERAL_MIDI_CLOCK_OUT.contains(&value),
        _ => false,
    }
}

/// Build the sparse `GeneralSettings` frame for one integer Device Setting.
///
/// `value` is always the public domain `is_general_integer_value` accepts. The
/// three brightnesses are the only ones whose wire field is a different unit, so
/// they are scaled by [`brightness_step_to_wire`] here; everything else is the
/// same number on both sides.
pub fn set_general_integer(
    setting: &str,
    value: i32,
) -> Result<OutboundMessage, CommandEncodeError> {
    let mut message = pa::GeneralSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        ..Default::default()
    };
    match setting {
        "screenBrightness" => {
            message.screen_brightness = Some(
                pa::general_settings_message::ScreenBrightness::ScreenBrightness(
                    brightness_step_to_wire(value),
                ),
            )
        }
        "ledBrightness" => {
            message.led_brightness =
                Some(pa::general_settings_message::LedBrightness::LedBrightness(
                    brightness_step_to_wire(value),
                ))
        }
        "dimmedLedBrightness" => {
            message.dimmed_led_brightness = Some(
                pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(
                    brightness_step_to_wire(value),
                ),
            )
        }
        "holdTiming" => {
            message.hold_timing = Some(pa::general_settings_message::HoldTiming::HoldTiming(value))
        }
        "midiChannel" => {
            message.midi_channel = Some(pa::general_settings_message::MidiChannel::MidiChannel(
                value,
            ))
        }
        "midiClockOut" => {
            message.midi_clock_out = Some(pa::general_settings_message::MidiClockOut::MidiClockOut(
                value,
            ))
        }
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "GeneralSettings integer",
                value: format!("{setting}={value}"),
            });
        }
    }
    Ok(OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        message,
    ))
}

/// Build the sparse `GeneralSettings` frame for one boolean Device Setting.
///
/// # Errors
///
/// Returns [`CommandEncodeError::UnsupportedValue`] when `setting` is not one
/// of the names listed on [`DeviceOperation::SetGeneralToggle`].
pub fn set_general_toggle(
    setting: &str,
    enabled: bool,
) -> Result<OutboundMessage, CommandEncodeError> {
    let mut message = pa::GeneralSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        ..Default::default()
    };
    match setting {
        "midiOverUsb" => {
            message.midi_over_usb =
                Some(pa::general_settings_message::MidiOverUsb::MidiOverUsb(enabled))
        }
        "lockScreenAndVolumeKnob" => message.lock_screen_and_volume_knob = Some(
            pa::general_settings_message::LockScreenAndVolumeKnob::LockScreenAndVolumeKnob(enabled),
        ),
        "ignoreDuplicatePc" => message.ignore_duplicate_pc = Some(
            pa::general_settings_message::IgnoreDuplicatePc::IgnoreDuplicatePc(enabled),
        ),
        "stompModeAutoAssign" => message.stomp_mode_auto_assign = Some(
            pa::general_settings_message::StompModeAutoAssign::StompModeAutoAssign(enabled),
        ),
        "swapTempoTunerAccess" => message.swap_tempo_tuner_access = Some(
            pa::general_settings_message::SwapTempoTunerAccess::SwapTempoTunerAccess(enabled),
        ),
        "disableInternetConnectionCheck" => message.disable_internet_connection_check = Some(
            pa::general_settings_message::DisableInternetConnectionCheck::DisableInternetConnectionCheck(enabled),
        ),
        "dynamicDelayCompensation" => message.enable_dynamic_delay_compensation = Some(
            pa::general_settings_message::EnableDynamicDelayCompensation::EnableDynamicDelayCompensation(enabled),
        ),
        "presetDimmed" => message.enable_preset_dimmed = Some(
            pa::general_settings_message::EnablePresetDimmed::EnablePresetDimmed(enabled),
        ),
        "sceneDimmed" => message.enable_scene_dimmed = Some(
            pa::general_settings_message::EnableSceneDimmed::EnableSceneDimmed(enabled),
        ),
        "stompDimmed" => message.enable_stomp_dimmed = Some(
            pa::general_settings_message::EnableStompDimmed::EnableStompDimmed(enabled),
        ),
        "midiClockIn" => message.midi_clock_in_enabled = Some(
            pa::general_settings_message::MidiClockInEnabled::MidiClockInEnabled(enabled),
        ),
        "gigViewStompAccess" => message.gig_view_stomp_access_enabled = Some(
            pa::general_settings_message::GigViewStompAccessEnabled::GigViewStompAccessEnabled(
                enabled,
            ),
        ),
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "GeneralSettings toggle",
                value: setting.to_owned(),
            });
        }
    }
    Ok(OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        message,
    ))
}

/// Set the scene block-bypass behaviour setting.
///
/// `0` always saves scene bypass changes (the default), `1` does not save
/// changes made with a Stomp footswitch but does save touchscreen changes, and
/// `2` never saves them. The device operations in `device::request` name these
/// values `alwaysOverwrite`, `nonstompOverwrite` and `neverOverwrite`.
pub fn set_scene_bypass_behavior(behavior: i32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            scene_block_bypass: Some(
                pa::general_settings_message::SceneBlockBypass::SceneBlockBypass(behavior),
            ),
            ..Default::default()
        },
    )
}

fn io_update(settings: pa::PortSettings) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_IO_SETTINGS,
        pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(settings)),
            ..Default::default()
        },
    )
}

/// Choose which outputs (Out 1/2, Out 3/4, Send 1/2, headphones) the master
/// volume assignment covers.
pub fn set_master_volume_assignment(
    out12: bool,
    out34: bool,
    send12: bool,
    headphones: bool,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            master_volume_assignment: Some(
                pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                    pa::MasterVolumeAssignmentOptions {
                        out12,
                        out34,
                        send12,
                        headphones,
                    },
                ),
            ),
            ..Default::default()
        },
    )
}

/// Set the global cab and/or IR bypass flags for the four chain rows.
///
/// Each supplied (`Some`) family is one complete type-9 GeneralSettings UPDATE
/// frame (Cab in outer tag 7, IR in outer tag 8), the shape the desktop
/// application sends per interaction; a family left `None` sends nothing. When both are
/// supplied the result is two frames in the order Cab then IR. The two are not
/// atomic: a failure between them can leave Cab applied and IR not. Returns an
/// empty list when neither family is supplied; `DeviceOperation::try_encode`
/// rejects that case.
pub fn set_global_bypass(cab: Option<[bool; 4]>, ir: Option<[bool; 4]>) -> Vec<OutboundMessage> {
    let rows = |values: [bool; 4]| pa::GlobalBypassRows {
        row1: values[0],
        row2: values[1],
        row3: values[2],
        row4: values[3],
    };
    let mut frames = Vec::with_capacity(2);
    if let Some(cab) = cab {
        frames.push(pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            global_bypass_cab: Some(
                pa::general_settings_message::GlobalBypassCab::GlobalBypassCab(rows(cab)),
            ),
            ..Default::default()
        });
    }
    if let Some(ir) = ir {
        frames.push(pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            global_bypass_ir: Some(
                pa::general_settings_message::GlobalBypassIr::GlobalBypassIr(rows(ir)),
            ),
            ..Default::default()
        });
    }
    frames
        .into_iter()
        .map(|message| OutboundMessage::encoded(profile::MESSAGE_TYPE_GENERAL_SETTINGS, message))
        .collect()
}

/// Build one `IOSettings` frame for each supplied (`Some`) setting of an input
/// port. The values are written to the wire as given.
pub fn set_input_port(
    input_port_id: u32,
    level: Option<f32>,
    impedance: Option<f32>,
    input_type: Option<f32>,
    ground_lift: Option<f32>,
) -> Vec<OutboundMessage> {
    let mut messages = Vec::new();
    for field in [
        level.map(|value| ("level", value)),
        impedance.map(|value| ("impedance", value)),
        input_type.map(|value| ("inputType", value)),
        ground_lift.map(|value| ("groundLift", value)),
    ]
    .into_iter()
    .flatten()
    {
        let mut port = pa::InputPortSettings {
            input_port_id,
            ..Default::default()
        };
        match field {
            ("level", value) => port.level = Some(pa::input_port_settings::Level::Level(value)),
            ("impedance", value) => {
                port.input_zmode = Some(pa::input_port_settings::InputZmode::InputZmode(value))
            }
            ("inputType", value) => {
                port.input_type = Some(pa::input_port_settings::InputType::InputType(value))
            }
            ("groundLift", value) => {
                port.ground_lift = Some(pa::input_port_settings::GroundLift::GroundLift(value))
            }
            _ => unreachable!(),
        }
        messages.push(io_update(pa::PortSettings {
            in_port: vec![port],
            ..Default::default()
        }));
    }
    messages
}

/// The QC IO-settings model exposes only these four physical
/// input-setting records. Route IDs (stereo, USB, previous-row, and internal)
/// are a different domain and must never be accepted by `set_input_port`.
pub const INPUT_SETTING_PORT_IDS: &[u32] = &[1, 2, 4, 5];

/// Return whether an ID names a physical QC input-settings record.
pub fn is_input_setting_port_id(value: u32) -> bool {
    INPUT_SETTING_PORT_IDS.contains(&value)
}

/// The nine impedance choices are enum ordinals normalized over eight steps.
pub fn is_input_impedance_value(value: f32) -> bool {
    value.is_finite() && (value * 8.0).fract() == 0.0 && (0.0..=1.0).contains(&value)
}

/// Instrument, microphone, and microphone-with-phantom are normalized enum
/// ordinals 0, 1/2, and 1 respectively.
pub fn is_input_type_value(value: f32) -> bool {
    matches!(value, 0.0 | 0.5 | 1.0)
}

/// Ground lift is represented as a normalized two-state control.
pub fn is_input_ground_lift_value(value: f32) -> bool {
    matches!(value, 0.0 | 1.0)
}

// ---------------------------------------------------------------------------
// Unit boundaries
//
// The QC carries every physical control as a bare protobuf number whose unit is
// nowhere in the wire format: an I/O level is a normalized `0.0..=1.0` float, a
// brightness is a percent integer. The public actions speak the unit a musician
// reads off the unit's own screen - decibels, and the 32-step brightness ladder.
// Each conversion between the two lives here, once, next to the frame builder
// that uses it, and `contract_transforms` in this module's tests re-derives
// every one of them from the arithmetic its workflow contract publishes. A
// setter that forgets to convert fails there rather than on an amplifier.
// ---------------------------------------------------------------------------

/// The decibel domain of an `OutputPortSettings.level` knob.
///
/// -40..+12 dB, the same span `packages/typescript/qc-ui/src/parameter-model.ts`
/// records.
pub const OUTPUT_LEVEL_MIN_DB: f64 = -40.0;
/// The top of the `OutputPortSettings.level` decibel domain.
pub const OUTPUT_LEVEL_MAX_DB: f64 = 12.0;

/// Convert an output level from decibels to the normalized wire field.
///
/// Derived from the device's own two domains rather than from the input gain's
/// constants, whose span is a different 72 dB: `IOSettings` parameter 33
/// `OUT 1 LEVEL` is `float 0.0..1.0 linear`
/// (`docs/reference/qc-live-parameter-scales.json`, model 31000) and the knob
/// spans `OUTPUT_LEVEL_MAX_DB - OUTPUT_LEVEL_MIN_DB` = 52 dB, so one wire unit
/// is 52 dB and 0 dB sits at 40/52 = 0.769230..., the unit's own 0 dB on all
/// eight output ports and on the headphones.
pub fn output_level_db_to_wire(level_db: f64) -> f32 {
    ((level_db - OUTPUT_LEVEL_MIN_DB) / (OUTPUT_LEVEL_MAX_DB - OUTPUT_LEVEL_MIN_DB)) as f32
}

/// Convert a normalized `OutputPortSettings.level` back to decibels.
///
/// The exact inverse of [`output_level_db_to_wire`], so a level read from the
/// device converts to the value a caller wrote.
pub fn output_level_wire_to_db(wire: f32) -> f32 {
    OUTPUT_LEVEL_MIN_DB as f32 + (OUTPUT_LEVEL_MAX_DB - OUTPUT_LEVEL_MIN_DB) as f32 * wire
}

/// Convert an input gain from decibels to the normalized wire field.
///
/// The sibling of [`output_level_db_to_wire`] over the input's own -12..+60 dB,
/// a different 72 dB span. The inverse is applied by
/// `responses::decode_io_settings` when it projects `levelDb`.
pub fn input_level_db_to_wire(level_db: f64) -> f32 {
    ((level_db + 12.0) / 72.0) as f32
}

/// Build the sparse `IOSettings` frames for one output port.
///
/// `level_db` is the public decibel value; the wire field it lands in is
/// normalized, so it is converted here by [`output_level_db_to_wire`]. Sending
/// a decibel number raw would write 0 dB as 0.0, which is -40 dB - silence.
pub fn set_output_port(
    output_port_id: u32,
    level_db: Option<f32>,
    ground_lift: Option<f32>,
    mute: Option<bool>,
) -> Vec<OutboundMessage> {
    let mut messages = Vec::new();
    if let Some(value) = level_db {
        messages.push(io_update(pa::PortSettings {
            out_port: vec![pa::OutputPortSettings {
                output_port_id,
                level: Some(pa::output_port_settings::Level::Level(
                    output_level_db_to_wire(f64::from(value)),
                )),
                ..Default::default()
            }],
            ..Default::default()
        }));
    }
    if let Some(value) = ground_lift {
        messages.push(io_update(pa::PortSettings {
            out_port: vec![pa::OutputPortSettings {
                output_port_id,
                ground_lift: Some(pa::output_port_settings::GroundLift::GroundLift(value)),
                ..Default::default()
            }],
            ..Default::default()
        }));
    }
    if let Some(value) = mute {
        messages.push(io_update(pa::PortSettings {
            out_port: vec![pa::OutputPortSettings {
                output_port_id,
                mute: Some(pa::output_port_settings::Mute::Mute(value)),
                ..Default::default()
            }],
            ..Default::default()
        }));
    }
    messages
}

/// Build one `IOSettings` frame for each supplied (`Some`) setting of the USB
/// port. The values are written to the wire as given.
pub fn set_usb_port(
    level: Option<f32>,
    headphones_source: Option<f32>,
    dry_wet: Option<f32>,
) -> Vec<OutboundMessage> {
    let mut messages = Vec::new();
    for field in [
        level.map(|value| ("level", value)),
        headphones_source.map(|value| ("headphonesSource", value)),
        dry_wet.map(|value| ("dryWet", value)),
    ]
    .into_iter()
    .flatten()
    {
        let mut port = pa::UsbPortSettings::default();
        match field {
            ("level", value) => port.level = Some(pa::usb_port_settings::Level::Level(value)),
            ("headphonesSource", value) => {
                port.hp_select = Some(pa::usb_port_settings::HpSelect::HpSelect(value))
            }
            ("dryWet", value) => port.dry_wet = Some(pa::usb_port_settings::DryWet::DryWet(value)),
            _ => unreachable!(),
        }
        messages.push(io_update(pa::PortSettings {
            usb_port: Some(pa::port_settings::UsbPort::UsbPort(port)),
            ..Default::default()
        }));
    }
    messages
}

/// HP source is the normalized ordinal of NONE, USB 1/2, USB 3/4, or BOTH.
pub fn is_usb_headphones_source_value(value: f32) -> bool {
    [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0].contains(&value)
}

/// USB dry/wet routing is the normalized ordinal of the two UI choices.
pub fn is_usb_dry_wet_value(value: f32) -> bool {
    matches!(value, 0.0 | 1.0)
}

/// Enable or disable MIDI thru. On the unit, enabling MIDI Thru disables
/// Preset MIDI Out.
pub fn set_midi_thru(enabled: bool) -> OutboundMessage {
    io_update(pa::PortSettings {
        midi_port: Some(pa::port_settings::MidiPort::MidiPort(
            pa::MidiPortSettings {
                midi_thru: Some(pa::midi_port_settings::MidiThru::MidiThru(if enabled {
                    1.0
                } else {
                    0.0
                })),
            },
        )),
        ..Default::default()
    })
}

/// Build one `IOSettings` frame for each supplied (`Some`) output-pairing
/// flag: XLR 1/2 linked and outputs 3/4 linked.
pub fn set_output_pairing(
    xlr12_linked: Option<bool>,
    out34_linked: Option<bool>,
) -> Vec<OutboundMessage> {
    let mut messages = Vec::new();
    if let Some(linked) = xlr12_linked {
        messages.push(OutboundMessage::encoded(
            profile::MESSAGE_TYPE_IO_SETTINGS,
            pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                xlr1_2_linked: Some(pa::io_settings_message::Xlr12Linked::Xlr12Linked(linked)),
                ..Default::default()
            },
        ));
    }
    if let Some(linked) = out34_linked {
        messages.push(OutboundMessage::encoded(
            profile::MESSAGE_TYPE_IO_SETTINGS,
            pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                out3_4_linked: Some(pa::io_settings_message::Out34Linked::Out34Linked(linked)),
                ..Default::default()
            },
        ));
    }
    messages
}

/// Device-only extension: no outbound sender for this custom-name field is
/// part of the confirmed protocol surface. Do not claim parity beyond what
/// this codec implements. Hosts send this one Version UPDATE frame and do not
/// read the name back; whether the unit applies it is hardware verification
/// pending.
pub fn set_device_name(name: impl Into<String>) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_VERSION,
        pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            custom_name: Some(pa::version_message::CustomName::CustomName(name.into())),
            ..Default::default()
        },
    )
}

/// Undo the last edit on the device: one UndoRedo UPDATE (`08 01 28 01`) and
/// no read afterwards. ESTABLISHED on CorOS 4.1.0 (direct USB, 2026-10-07): the
/// unit pushes a full preset, Scene, SetlistPosition, UndoRedo and Grid within
/// about 0.25 s of an undo, so no read is needed.
pub fn undo() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_UNDO_REDO,
        pa::UndoRedoMessage {
            action: pa::message_action::Enum::Update as i32,
            undo: Some(pa::undo_redo_message::Undo::Undo(true)),
            ..Default::default()
        },
    )
}

/// Redo the last undone edit on the device: one UndoRedo UPDATE
/// (`08 01 30 01`) and no read afterwards; see [`undo`].
pub fn redo() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_UNDO_REDO,
        pa::UndoRedoMessage {
            action: pa::message_action::Enum::Update as i32,
            redo: Some(pa::undo_redo_message::Redo::Redo(true)),
            ..Default::default()
        },
    )
}

/// Request the modules the device's compiler has inhibited.
pub fn read_inhibited_modules() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_COMPILER_INHIBITED_MODULES,
        pa::CompilerInhibitedModulesMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        },
    )
}

/// Request a screenshot of the stored preset at `position` in the folder named
/// `folder_name`, correlated by `request_id`.
pub fn preset_screenshot(
    folder_name: impl Into<String>,
    position: u32,
    is_factory: bool,
    request_id: u64,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SCREENSHOT,
        pa::ScreenshotMessage {
            action: pa::message_action::Enum::Read as i32,
            request_id: Some(pa::screenshot_message::RequestId::RequestId(request_id)),
            folder_name: folder_name.into(),
            is_factory,
            index: position as i32,
            ..Default::default()
        },
    )
}

/// Request a screenshot of the device's display.
pub fn capture_screen() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_REMOTE_CONTROL,
        pa::RemoteControlMessage {
            action: pa::message_action::Enum::Read as i32,
            screenshot: Some(pa::RemoteControlScreenshot::default()),
            ..Default::default()
        },
    )
}

/// Request a typed diagnostics snapshot (message 7). This is read-only and
/// does not enable any factory, calibration, serialization, or production
/// automation mode.
pub fn read_diagnostics(request_id: Option<u64>) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_DIAGNOSTICS,
        pa::DiagnosticsMessage {
            action: pa::message_action::Enum::Read as i32,
            request_id: request_id.map(pa::diagnostics_message::RequestId::RequestId),
            ..Default::default()
        },
    )
}

/// Ask the QC for the structure of what is currently on its screen.
///
/// `RemoteControlMessage` carries a third payload alongside `mouse` and
/// `screenshot` that nothing here used: the device's live scene graph, as
/// indented text. The unit answers with its `zenUI` widget tree, including
/// each node's class, its text, and the icon each image node draws:
///
/// ```text
/// zenUI::RootGraphicsItem
///   zenUI::GraphicsItem
///     zenUI::Grid
///       zenUI::Chain
///         zenUI::InputPortItem
///           zenUI::GraphicsItem
///             text : 'In\n1'
///           zenUI::GraphicsItem
///             image: 'icons/24/plus.png'
/// ```
///
/// This is a structural oracle for screen reconstructions: [`capture_screen`]
/// says what the device drew, and this says what it believes it drew and with
/// which widgets, which is what a pixel diff cannot tell you.
pub fn read_graphics_tree() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_REMOTE_CONTROL,
        pa::RemoteControlMessage {
            action: pa::message_action::Enum::Read as i32,
            graphics_tree: Some(pa::RemoteControlGraphicsTree::default()),
            ..Default::default()
        },
    )
}

/// Ask the device to create a local backup.
pub fn create_local_backup() -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_LOCAL_BACKUP,
        pa::LocalBackupMessage {
            action: pa::message_action::Enum::Create as i32,
            ..Default::default()
        },
    )
}

/// Recall the preset at `position` in the setlist `setlist_key`: one type-2
/// SetlistPosition UPDATE carrying folder key, zero-based position and
/// is_factory, with no request id (the desktop application's `updateSetlist` sends
/// none). Completion is observed as a newer position revision, never from an
/// echoed id, and that observation names the address only, not the preset body.
pub fn setlist_position(
    setlist_key: impl Into<String>,
    position: u32,
    is_factory: bool,
) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_SETLIST_POSITION,
        pa::SetlistPositionMessage {
            action: pa::message_action::Enum::Update as i32,
            folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                setlist_key.into(),
            )),
            position: Some(pa::setlist_position_message::Position::Position(position)),
            is_factory: Some(pa::setlist_position_message::IsFactory::IsFactory(
                is_factory,
            )),
            ..Default::default()
        },
    )
}

/// Map a BPM onto the QC's 0..1 tempo parameter.
///
/// The wire value is `(bpm - 40) / 200`, so 40 BPM is `0.0` and 240 BPM is
/// `1.0`. Observed on a CorOS 4.1.0 unit, 2026-10-07: the preset tempo value
/// `0.86` showed 212 BPM and `0.4` showed 120, and the device-wide tempo uses
/// the same mapping.
/// The same scale is what [`responses::decode_tempo_settings`] reads back via
/// `tempo_from_normalized`, its inverse.
pub(crate) fn normalized_tempo(bpm: u32) -> f32 {
    let minimum = domain::MINIMUM_TEMPO_BPM as f32;
    let span = (domain::MAXIMUM_TEMPO_BPM - domain::MINIMUM_TEMPO_BPM) as f32;
    (bpm.clamp(domain::MINIMUM_TEMPO_BPM, domain::MAXIMUM_TEMPO_BPM) as f32 - minimum) / span
}

/// Inverse of `normalized_tempo`: map the QC's 0..1 tempo parameter back onto
/// a BPM value in the fixed 40..240 domain (`40 + 200 * value`, rounded). Shared
/// by every site that decodes tempo param index 0 back to BPM
/// (`responses::decode_tempo_settings`, `state::tempo_from_params`), which
/// used to each independently recompute the same minimum/span pair.
pub(crate) fn tempo_from_normalized(value: f32) -> u32 {
    let minimum = domain::MINIMUM_TEMPO_BPM as f32;
    let span = (domain::MAXIMUM_TEMPO_BPM - domain::MINIMUM_TEMPO_BPM) as f32;
    (minimum + span * value).round() as u32
}

/// Measured taper of the tempo block's metronome VOLUME (param index 3),
/// as `(raw 0..1 parameter value, dB the unit's Tempo screen shows)`.
///
/// The raw value is NOT linear in dB. An earlier revision decoded it as
/// `-60 + 69 * raw`, which disagreed with the unit's own display by up to
/// ~29.8 dB (raw 0.05 displays -30.7 dB, not -56.6). These 16 points were read
/// off the Tempo screen on hardware (one decimal, 0.1 dB resolution) by
/// writing `raw` and reading the screen. No closed form fits them: a squared
/// taper `9 + 40 log10(raw)` is within ~0.4 dB for raw >= 0.3 but is 12 dB off
/// at raw 0.05, and the best power-law-with-offset or `A + B ln(raw + c)` fit
/// still misses by ~0.4 dB. The table is therefore the source of truth and is
/// interpolated linearly in dB between knots, which makes both directions
/// exact inverses and reproduces every measured point exactly.
///
/// Unmeasured: the segment raw 0..0.05 (the unit floors at -60 dB at raw 0; the
/// interior is a straight line between the floor and the first measurement) and
/// everything between knots. Add a knot here when a finer reading exists; keep
/// both columns strictly increasing.
pub(crate) const TEMPO_VOLUME_KNOTS: [(f64, f64); 16] = [
    (0.000, -60.0),
    (0.050, -30.7),
    (0.150, -19.9),
    (0.250, -13.6),
    (0.350, -8.9),
    (0.435, -5.6),
    (0.500, -3.4),
    (0.550, -1.8),
    (0.613, 0.0),
    (0.650, 1.0),
    (0.725, 2.9),
    (0.800, 4.7),
    (0.870, 6.3),
    (0.900, 7.0),
    (0.950, 8.0),
    (1.000, 9.0),
];

/// Lowest displayed tempo volume (raw 0): the unit's floor, -60 dB.
pub(crate) const TEMPO_VOLUME_MIN_DB: f64 = TEMPO_VOLUME_KNOTS[0].1;
/// Highest displayed tempo volume (raw 1): +9 dB, confirmed on hardware.
pub(crate) const TEMPO_VOLUME_MAX_DB: f64 = TEMPO_VOLUME_KNOTS[TEMPO_VOLUME_KNOTS.len() - 1].1;

/// Raw tempo-volume parameter (0..1) to the dB the unit's Tempo screen shows.
/// Out-of-range input clamps to the floor (-60 dB) at raw <= 0 and to +9 dB at
/// raw >= 1; callers that must reject out-of-range values check before this.
pub(crate) fn tempo_volume_db_from_raw(raw: f32) -> f32 {
    let raw = f64::from(raw).clamp(0.0, 1.0);
    let segment = TEMPO_VOLUME_KNOTS
        .windows(2)
        .find(|pair| raw <= pair[1].0)
        .unwrap_or(&TEMPO_VOLUME_KNOTS[TEMPO_VOLUME_KNOTS.len() - 2..]);
    let ((x0, y0), (x1, y1)) = (segment[0], segment[1]);
    (y0 + (y1 - y0) * (raw - x0) / (x1 - x0)) as f32
}

/// Inverse of [`tempo_volume_db_from_raw`]: the dB the unit shows to the raw
/// tempo-volume parameter. Clamps to -60..+9 dB (raw 0..1).
pub(crate) fn tempo_volume_raw_from_db(db: f32) -> f32 {
    let db = f64::from(db).clamp(TEMPO_VOLUME_MIN_DB, TEMPO_VOLUME_MAX_DB);
    let segment = TEMPO_VOLUME_KNOTS
        .windows(2)
        .find(|pair| db <= pair[1].1)
        .unwrap_or(&TEMPO_VOLUME_KNOTS[TEMPO_VOLUME_KNOTS.len() - 2..]);
    let ((x0, y0), (x1, y1)) = (segment[0], segment[1]);
    (x0 + (x1 - x0) * (db - y0) / (y1 - y0)) as f32
}

/// Set the loaded preset's tempo in beats per minute.
///
/// `bpm` is clamped to the device's `40..=240` range and written as a
/// normalized value to the preset's tempo block.
pub fn set_tempo(bpm: u32) -> OutboundMessage {
    let normalized = normalized_tempo(bpm);
    let preset = BinaryPreset {
        tempo_program_data: vec![Model {
            params: vec![Param {
                param_values: vec![ParamValue {
                    value: Some(param_value::Value::FloatValue(normalized)),
                }],
                index: Some(param::Index::Index(0)),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    grid_update(preset)
}

/// Return whether a normalized tempo/metronome parameter is a UI-representable value.
pub fn is_tempo_metronome_parameter_value(index: u32, value: f32) -> bool {
    if !value.is_finite() {
        return false;
    }

    match index {
        2 | 4 => matches!(value, 0.0 | 1.0),
        3 | 5 => (0.0..=1.0).contains(&value),
        6 => (0..=20).any(|ordinal| value == ordinal as f32 / 20.0),
        7 | 10..=22 => [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0].contains(&value),
        8 => (0..=5).any(|ordinal| value == ordinal as f32 / 5.0),
        9 => (0..=4).any(|ordinal| value == ordinal as f32 / 4.0),
        _ => false,
    }
}

/// Update one or more tempo/metronome fields. Each field is sent as one sparse
/// type-33 GlobalTempo UPDATE parameter frame, in the order given, so a failure
/// between frames leaves a partial state. Nothing reads them back. Whether the
/// unit stores these fields in the preset tempo block is not established
/// (hardware verification pending).
pub fn set_tempo_parameters(parameters: Vec<(u32, f32)>) -> Vec<OutboundMessage> {
    parameters
        .into_iter()
        .map(|(index, value)| global_tempo_parameter(index, value))
        .collect()
}

/// Write one parameter of the device-global tempo block.
fn global_tempo_parameter(index: u32, value: f32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GLOBAL_TEMPO,
        pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params: vec![Param {
                index: Some(param::Index::Index(index)),
                param_values: vec![ParamValue {
                    value: Some(param_value::Value::FloatValue(value)),
                }],
                ..Default::default()
            }],
            ..Default::default()
        },
    )
}

/// Select whether the QC plays the loaded preset tempo or the device-global
/// tempo block. This is GlobalTempo parameter 1, not preset TempoControl TYPE.
pub fn set_tempo_mode(global: bool) -> OutboundMessage {
    global_tempo_parameter(1, if global { 1.0 } else { 0.0 })
}

/// Set the device-global tempo, which is GlobalTempo parameter 0.
///
/// This is the target the physical TAP button writes; [`set_tempo`] writes
/// the *loaded preset's* TempoControl block instead, and [`set_tempo_mode`]
/// decides which of the two the QC plays.
///
/// The frame is sent in either tempo mode: the desktop application has no mode
/// precondition for it. ESTABLISHED on CorOS 4.1.0 (direct USB, 2026-10-07): while
/// parameter 1 is `0.0` (PRESET) the unit does not ignore the write. It edits the
/// loaded preset's tempo (`tempoProgramData` parameter 0), answers with UndoRedo,
/// Grid and PresetDirty pushes (not a type-33 echo), the metronome clock follows,
/// and the loaded preset becomes dirty.
pub fn set_global_tempo(bpm: u32) -> OutboundMessage {
    global_tempo_parameter(0, normalized_tempo(bpm))
}

/// Set the downstream master level. The wire value is a float from `0.0` to
/// `1.0`; the unit and QC Remote display it as 0-100 (`round(value * 100)`),
/// and no physical level, in decibels or otherwise, is established for it. Never include `calibrate` in a
/// level write: on the device that field opens the calibration workflow.
pub fn set_master_volume(volume: f32) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_MASTER_VOLUME,
        pa::MasterVolumeMessage {
            action: pa::message_action::Enum::Update as i32,
            volume: Some(pa::master_volume_message::Volume::Volume(volume)),
            ..Default::default()
        },
    )
}

fn validate_capture_retarget(
    column: u32,
    active_scene: u32,
    key: &str,
    model: &Model,
    bypass: Option<&ColBypass>,
) -> Result<(), CommandEncodeError> {
    let model_id = match model.hash {
        Some(model::Hash::Hash(value)) if is_neural_capture_model(value) => value,
        _ => {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "capture retarget model hash",
                value: "missing or not a Neural Capture".into(),
            });
        }
    };
    if !matches!(model.column, Some(model::Column::Column(value)) if value == column) {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "capture retarget model column",
            value: format!("does not match column {column}"),
        });
    }
    let mut matches = model
        .params
        .iter()
        .filter(|parameter| matches!(parameter.index, Some(param::Index::Index(5))));
    let Some(parameter) = matches.next() else {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "capture retarget parameter 5",
            value: "missing".into(),
        });
    };
    if matches.next().is_some() {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "capture retarget parameter 5",
            value: "ambiguous".into(),
        });
    }
    let scene_mode = matches!(
        parameter.scene_mode,
        Some(param::SceneMode::SceneMode(true))
    );
    // Keep shared singleton compatibility without certifying a different
    // stored scene when a multi-value record lacks the active index.
    let selected = if scene_mode && parameter.param_values.len() != 1 {
        active_scene as usize
    } else {
        0
    };
    if !matches!(
        parameter.param_values.get(selected).and_then(|value| value.value.as_ref()),
        Some(param_value::Value::StringValue(value)) if value == key
    ) {
        return Err(CommandEncodeError::UnsupportedValue {
            field: "capture retarget parameter 5",
            value: format!("does not contain the selected key for model {model_id}"),
        });
    }
    if let Some(bypass) = bypass {
        if !matches!(bypass.column, Some(col_bypass::Column::Column(value)) if value == column) {
            return Err(CommandEncodeError::UnsupportedValue {
                field: "capture retarget bypass column",
                value: format!("does not match column {column}"),
            });
        }
    }
    Ok(())
}

fn update_auditioning_model(
    row: u32,
    column: u32,
    mut current_model: Model,
    current_bypass: Option<ColBypass>,
) -> OutboundMessage {
    // updateAuditioningModel writes explicit selectors even when the current
    // full-preset representation relies on positional defaults.
    current_model.column = Some(model::Column::Column(column));
    grid_update(BinaryPreset {
        chains: vec![Chain {
            row: Some(chain::Row::Row(row)),
            models: vec![current_model],
            ..Default::default()
        }],
        bypass: current_bypass
            .map(|col_bypass| Bypass {
                row: Some(bypass::Row::Row(row)),
                col_bypass: vec![col_bypass],
            })
            .into_iter()
            .collect(),
        ..Default::default()
    })
}

fn grid_update(preset: BinaryPreset) -> OutboundMessage {
    OutboundMessage::encoded(
        profile::MESSAGE_TYPE_GRID,
        pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(preset)),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn checked_io_levels_reject_nonfinite_and_out_of_domain_values_without_frames() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            for operation in [
                DeviceOperation::SetInputPort {
                    input_port_id: 1,
                    level: Some(value),
                    impedance: None,
                    input_type: None,
                    ground_lift: None,
                },
                DeviceOperation::SetUsbPort {
                    level: Some(value),
                    headphones_source: None,
                    dry_wet: None,
                },
            ] {
                assert!(matches!(
                    operation.try_encode(),
                    Err(CommandEncodeError::UnsupportedValue { .. })
                ));
            }
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -40.01, 12.01] {
            assert!(DeviceOperation::SetOutputPort {
                output_port_id: 1,
                level: Some(value),
                ground_lift: None,
                mute: None
            }
            .try_encode()
            .is_err());
        }
        for value in [f32::NAN, f32::INFINITY, -0.01, 0.5, 1.01] {
            assert!(DeviceOperation::SetOutputPort {
                output_port_id: 1,
                level: None,
                ground_lift: Some(value),
                mute: None
            }
            .try_encode()
            .is_err());
        }
        for value in [0.0, 1.0] {
            for operation in [
                DeviceOperation::SetInputPort {
                    input_port_id: 1,
                    level: Some(value),
                    impedance: None,
                    input_type: None,
                    ground_lift: None,
                },
                DeviceOperation::SetUsbPort {
                    level: Some(value),
                    headphones_source: None,
                    dry_wet: None,
                },
                DeviceOperation::SetOutputPort {
                    output_port_id: 1,
                    level: Some(if value == 0.0 { -40.0 } else { 12.0 }),
                    ground_lift: Some(value),
                    mute: None,
                },
            ] {
                let unchecked = operation.clone().encode_unchecked().unwrap();
                assert_eq!(operation.try_encode().unwrap(), unchecked);
            }
        }
    }

    #[test]
    fn checked_setlist_creation_and_stomp_momentary_refuse_invalid_targets() {
        for name in [
            "",
            "..",
            "../Factory",
            "Child/Grandchild",
            "Child\\Grandchild",
            "My Presets",
            "my presets",
            " leading",
            "trailing ",
            "line\nbreak",
        ] {
            assert!(DeviceOperation::CreateSetlist { name: name.into() }
                .try_encode()
                .is_err());
        }
        assert!(DeviceOperation::CreateSetlist {
            name: "x".repeat(65)
        }
        .try_encode()
        .is_err());
        for name in ["Tour", "Тур3"] {
            let operation = DeviceOperation::CreateSetlist { name: name.into() };
            assert_eq!(
                operation.try_encode().unwrap(),
                vec![create_setlist(name.into())]
            );
        }
        for footswitch in [8, u32::MAX] {
            assert!(DeviceOperation::SetStompMomentary {
                footswitch,
                momentary: true
            }
            .try_encode()
            .is_err());
        }
        for footswitch in 0..8 {
            assert_eq!(
                DeviceOperation::SetStompMomentary {
                    footswitch,
                    momentary: true
                }
                .try_encode()
                .unwrap(),
                vec![set_stomp_momentary(footswitch, true)]
            );
        }
    }

    #[test]
    fn checked_capture_retarget_refuses_an_out_of_range_scene_before_fallback_selection() {
        let model = Model {
            hash: Some(model::Hash::Hash(14_000)),
            column: Some(model::Column::Column(0)),
            params: vec![Param {
                index: Some(param::Index::Index(5)),
                scene_mode: Some(param::SceneMode::SceneMode(true)),
                param_values: vec![ParamValue {
                    value: Some(param_value::Value::StringValue("key".into())),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        for active_scene in [8, u32::MAX] {
            assert!(matches!(
                DeviceOperation::RetargetCapture {
                    row: 0,
                    column: 0,
                    active_scene,
                    key: "key".into(),
                    source_revision: 1,
                    model: model.clone(),
                    bypass: None
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "active scene",
                    ..
                })
            ));
        }
        for active_scene in 0..8 {
            assert!(DeviceOperation::RetargetCapture {
                row: 0,
                column: 0,
                active_scene,
                key: "key".into(),
                source_revision: 1,
                model: model.clone(),
                bypass: None
            }
            .try_encode()
            .is_ok());
        }
    }

    /// (raw, dB the unit's Tempo screen showed) read on hardware, 0.1 dB resolution.
    const MEASURED_TEMPO_VOLUME: [(f32, f32); 16] = [
        (0.000, -60.0),
        (0.050, -30.7),
        (0.150, -19.9),
        (0.250, -13.6),
        (0.350, -8.9),
        (0.435, -5.6),
        (0.500, -3.4),
        (0.550, -1.8),
        (0.613, 0.0),
        (0.650, 1.0),
        (0.725, 2.9),
        (0.800, 4.7),
        (0.870, 6.3),
        (0.900, 7.0),
        (0.950, 8.0),
        (1.000, 9.0),
    ];

    #[test]
    fn tempo_volume_matches_every_measured_unit_reading() {
        for (raw, shown) in MEASURED_TEMPO_VOLUME {
            let decoded = tempo_volume_db_from_raw(raw);
            assert!(
                (decoded - shown).abs() <= 0.15,
                "raw {raw} decodes to {decoded} dB, the unit shows {shown}"
            );
            let encoded = tempo_volume_raw_from_db(shown);
            assert!(
                (encoded - raw).abs() <= 1e-4,
                "{shown} dB encodes to raw {encoded}, measured raw {raw}"
            );
        }
        // The old linear decode put raw 0.5 at -25.5 dB; the unit shows -3.4.
        assert!((tempo_volume_db_from_raw(0.5) - -3.4).abs() <= 0.15);
    }

    #[test]
    fn tempo_volume_floor_and_ceiling_are_explicit() {
        assert_eq!(tempo_volume_db_from_raw(0.0), -60.0);
        assert_eq!(tempo_volume_db_from_raw(1.0), 9.0);
        assert_eq!(tempo_volume_db_from_raw(-0.5), -60.0);
        assert_eq!(tempo_volume_db_from_raw(1.5), 9.0);
        assert_eq!(tempo_volume_raw_from_db(-60.0), 0.0);
        assert_eq!(tempo_volume_raw_from_db(9.0), 1.0);
        assert_eq!(tempo_volume_raw_from_db(-90.0), 0.0);
        assert_eq!(tempo_volume_raw_from_db(20.0), 1.0);
    }

    #[test]
    fn tempo_volume_knots_are_strictly_increasing() {
        for pair in TEMPO_VOLUME_KNOTS.windows(2) {
            assert!(pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1);
        }
        assert_eq!(TEMPO_VOLUME_KNOTS[0], (0.0, -60.0));
        assert_eq!(TEMPO_VOLUME_KNOTS[TEMPO_VOLUME_KNOTS.len() - 1].0, 1.0);
    }

    #[test]
    fn tempo_volume_round_trips_across_the_range() {
        for step in 0..=20_000u32 {
            let raw = step as f32 / 20_000.0;
            let back = tempo_volume_raw_from_db(tempo_volume_db_from_raw(raw));
            assert!((back - raw).abs() <= 1e-6, "raw {raw} came back as {back}");
        }
        for step in 0..=6_900u32 {
            let db = -60.0 + step as f32 / 100.0;
            let back = tempo_volume_db_from_raw(tempo_volume_raw_from_db(db));
            assert!((back - db).abs() <= 1e-3, "{db} dB came back as {back}");
        }
        // Monotone: a louder raw value never reads quieter.
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=2_000u32 {
            let db = tempo_volume_db_from_raw(step as f32 / 2_000.0);
            assert!(db >= previous);
            previous = db;
        }
    }

    #[test]
    fn current_preset_read_is_the_one_bare_boot_frame() {
        let message = read_current_preset();
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_RECALL_PRESET);
        assert_eq!(message.payload, [0x08, 0x03]);
        assert_eq!(
            DeviceOperation::ReadCurrentPreset.try_encode().unwrap(),
            vec![message]
        );
    }

    #[test]
    fn set_device_name_preserves_exact_utf8_in_the_sparse_device_extension() {
        let message = set_device_name("QC é🚀");
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_VERSION);
        assert_eq!(
            message.payload,
            [0x08, 0x01, 0x7a, 0x09, b'Q', b'C', b' ', 0xc3, 0xa9, 0xf0, 0x9f, 0x9a, 0x80,]
        );
        let decoded = pa::VersionMessage::decode(message.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(
            decoded.custom_name,
            Some(pa::version_message::CustomName::CustomName("QC é🚀".into()))
        );
        assert!(decoded.request_id.is_none());
    }

    #[test]
    fn staged_startup_commands_match_expected_order() {
        assert_eq!(
            building_initialization(),
            vec![connection(true), read(profile::MESSAGE_TYPE_MODEL_REPO)]
        );
        assert_eq!(
            module_stats_initialization(),
            vec![read(profile::MESSAGE_TYPE_MODULE_STATS)]
        );
        assert_eq!(
            boot_initialization()
                .iter()
                .map(|message| message.message_type)
                .collect::<Vec<_>>(),
            // This build cannot request License (58): that family rides on a
            // trailer encryption it does not implement, so its own session
            // encoder refuses the message, and asking anyway fails the whole
            // burst at that position. The rest of the order is unchanged.
            // Restore 58 only together with that encryption.
            [
                26, 21, 3, 9, 24, 14, 38, 17, 4, 71, 46, 20, 20, 42, 15, 50, 54, 19, 33, 2, 34, 13,
                57, 60,
            ]
        );
        // Every index below is one lower than it would otherwise be, because
        // the License read that would normally come second is absent here
        // for the reason above.
        let recents =
            pa::RecentsFavoritesMessage::decode(boot_initialization()[11].payload.as_slice())
                .unwrap();
        let favorites =
            pa::RecentsFavoritesMessage::decode(boot_initialization()[12].payload.as_slice())
                .unwrap();
        assert!(!recents.is_favorites);
        assert!(favorites.is_favorites);
        let exact = boot_initialization_exact(41, false);
        assert_eq!(exact[0].payload, [0x10, 0x29]);
        assert_eq!(exact[8].payload, [0x08, 0x03, 0x30, 0x00]);
        assert_eq!(exact[10].payload, [0x08, 0x03, 0x42, 0x00]);
        assert_eq!(exact[11].payload, [0x08, 0x03]);
        assert_eq!(exact[12].payload, [0x08, 0x03, 0x18, 0x01]);
    }

    #[test]
    fn disconnected_state_entry_has_exact_bytes_and_order() {
        assert_eq!(
            disconnect_initialization_exact(40),
            vec![
                OutboundMessage {
                    message_type: profile::MESSAGE_TYPE_CPU_LOAD,
                    payload: vec![0x08, 0x02, 0x10, 0x28],
                },
                OutboundMessage {
                    message_type: profile::MESSAGE_TYPE_CONNECTION,
                    payload: vec![0x10, 0x00],
                },
            ]
        );
    }

    #[test]
    fn local_backup_export_matches_expected_create_path() {
        let message = create_local_backup();
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_LOCAL_BACKUP);
        // CREATE is protobuf's zero value, so the canonical export request has
        // an empty body. Do not confuse this with the UPDATE sent after a
        // local backup has been streamed back to the device.
        assert!(message.payload.is_empty());
        let decoded = pa::LocalBackupMessage::decode(message.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Create as i32);
        assert!(decoded.request_id.is_none());
    }

    /// Pins the encoded Unix-millisecond field; this does not prove which device
    /// timestamps use the value. See [`sync_system_time`] for the observed limits.
    #[test]
    fn system_time_sync_carries_unix_milliseconds() {
        let now_ms = 1_788_711_237_617;
        let outbound = sync_system_time(7, now_ms);
        assert_eq!(outbound.message_type, 43);
        let decoded = pa::SystemTimeSyncMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(decoded.ms_since_epoch, now_ms);
        assert_eq!(
            decoded.request_id,
            Some(pa::system_time_sync_message::RequestId::RequestId(7))
        );
    }

    #[test]
    fn show_gig_view_preserves_explicit_boolean_polarity_and_sparse_bytes() {
        for (shown, expected) in [
            (true, [0x08, 0x01, 0x18, 0x01]),
            (false, [0x08, 0x01, 0x18, 0x00]),
        ] {
            let outbound = show_gig_view(shown);
            assert_eq!(outbound.message_type, profile::MESSAGE_TYPE_SHOW_GIG_VIEW);
            assert_eq!(outbound.payload, expected);
            let decoded = pa::ShowGigViewMessage::decode(outbound.payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(decoded.show, shown);
            assert!(decoded.request_id.is_none());
        }
    }

    #[test]
    fn tap_screen_has_exact_device_extension_bytes_order_and_bounds() {
        let origin = DeviceOperation::ScreenTap { x: 0.0, y: 0.0 }
            .try_encode()
            .unwrap();
        // One TAP frame (mouse type 3) and nothing else: no touch-down before
        // it and no touch-up after it. Observed on CorOS 4.1.0 (2026-10-06): the
        // lone frame toggled the menu button 10 times in 10 and left no row
        // highlighted.
        assert_eq!(origin.len(), 1);
        assert_eq!(origin[0].message_type, profile::MESSAGE_TYPE_REMOTE_CONTROL);
        assert_eq!(origin[0].payload, [0x08, 0x01, 0x1a, 0x02, 0x18, 0x03]);

        let corner = DeviceOperation::ScreenTap { x: 799.0, y: 479.0 }
            .try_encode()
            .unwrap();
        assert_eq!(corner.len(), 1);
        assert_eq!(
            corner[0].payload,
            [
                0x08, 0x01, 0x1a, 0x0c, 0x0d, 0x00, 0xc0, 0x47, 0x44, 0x15, 0x00, 0x80, 0xef, 0x43,
                0x18, 0x03,
            ]
        );

        for (x, y) in [
            (-1.0, 0.0),
            (0.5, 0.0),
            (800.0, 0.0),
            (0.0, -1.0),
            (0.0, 479.5),
            (0.0, 480.0),
            (f32::NAN, 0.0),
            (0.0, f32::INFINITY),
        ] {
            assert!(DeviceOperation::ScreenTap { x, y }.try_encode().is_err());
        }
    }

    #[test]
    fn screen_hold_is_a_touch_down_then_a_touch_up_because_the_protocol_has_no_long_press() {
        // The protocol has no long-press frame and the device carries no hold
        // duration. A hold is the touch-down frame (wire type 0, PRESS; observed
        // on CorOS 4.1.0, 2026-10-06), then the touch-up frame (wire type 1,
        // RELEASE), the host sending them REMOTE_HOLD_INTERVAL_MS apart. It is
        // NOT the one-frame TAP, and its long-press effect is not hardware-verified.
        for (x, y) in [(0.0, 0.0), (799.0, 479.0), (400.0, 240.0)] {
            let hold = DeviceOperation::ScreenHold { x, y }.try_encode().unwrap();
            assert_eq!(hold.len(), 2);
            assert_eq!(hold, screen_hold(x, y).to_vec());
            let down = decoded_mouse(&hold[0]);
            let up = decoded_mouse(&hold[1]);
            assert_eq!(down.r#type, pa::remote_control_mouse::Type::Press as i32);
            assert_eq!(up.r#type, pa::remote_control_mouse::Type::Release as i32);
            assert_eq!((down.x, down.y, up.x, up.y), (x, y, x, y));
            assert_ne!(
                hold,
                DeviceOperation::ScreenTap { x, y }.try_encode().unwrap(),
                "a hold at ({x}, {y}) must not be the one-frame tap"
            );
        }

        // And the same refusals: an off-screen or fractional hold is no more
        // sendable than an off-screen tap.
        for (x, y) in [
            (-1.0, 0.0),
            (0.5, 0.0),
            (800.0, 0.0),
            (0.0, -1.0),
            (0.0, 479.5),
            (0.0, 480.0),
            (f32::NAN, 0.0),
            (0.0, f32::INFINITY),
        ] {
            assert!(DeviceOperation::ScreenHold { x, y }.try_encode().is_err());
        }
    }

    #[test]
    fn shared_commands_match_the_verified_android_wire_shapes() {
        assert_eq!(select_scene(3).payload, [0x08, 0x01, 0x18, 0x03]);
        assert_eq!(show_tuner(true).payload, [0x08, 0x01, 0x18, 0x01]);
        assert_eq!(show_gig_view(false).payload, [0x08, 0x01, 0x18, 0x00]);
        assert_eq!(acknowledge_capture_dialog(true).message_type, 36);
        assert_eq!(
            acknowledge_capture_dialog(true).payload,
            [0x08, 0x01, 0x28, 0x01]
        );
        assert_eq!(
            acknowledge_capture_dialog(false).payload,
            [0x08, 0x01, 0x28, 0x00]
        );
        assert_eq!(
            neural_capture_dialog(false, true).payload,
            [0x08, 0x01],
            "the first captured bool gates field presence"
        );
        assert_eq!(keepalive().payload, [0x08, 0x01, 0x10, 0x00, 0x18, 0x01]);
        assert_eq!(read(profile::MESSAGE_TYPE_MODEL_REPO).payload, [0x08, 0x03]);
        assert_eq!(read_version().payload, [0x08, 0x03]);
        assert_eq!(read_tuner().message_type, 6);
        assert_eq!(read_tuner().payload, [0x08, 0x03]);
        assert_eq!(set_tuner_input(8).payload, [0x08, 0x01, 0x18, 0x08]);
        assert_eq!(set_tuner_mute(true).payload, [0x08, 0x01, 0x28, 0x01]);
        assert_eq!(set_tuner_mute(false).payload, [0x08, 0x01, 0x28, 0x00]);
        assert_eq!(
            set_tuner_reference(2.0).payload,
            [0x08, 0x01, 0x25, 0x00, 0x00, 0x00, 0x40]
        );
        assert_eq!(
            set_tuner_reference(-40.0).payload,
            [0x08, 0x01, 0x25, 0x00, 0x00, 0x20, 0xc2]
        );
        assert_eq!(
            set_tuner_reference(40.0).payload,
            [0x08, 0x01, 0x25, 0x00, 0x00, 0x20, 0x42]
        );
        assert_eq!(undo().payload, [0x08, 0x01, 0x28, 0x01]);
        assert_eq!(redo().payload, [0x08, 0x01, 0x30, 0x01]);
        assert_eq!(
            set_device_name("Stage QC").payload,
            [0x08, 0x01, 0x7a, 0x08, b'S', b't', b'a', b'g', b'e', b' ', b'Q', b'C']
        );
        assert_eq!(read_inhibited_modules().payload, [0x08, 0x03]);
        assert_eq!(
            preset_screenshot("My Presets", 12, false, 9).payload,
            [
                0x08, 0x03, 0x10, 0x09, 0x1a, 0x0a, b'M', b'y', b' ', b'P', b'r', b'e', b's', b'e',
                b't', b's', 0x28, 0x0c
            ]
        );
        assert_eq!(capture_screen().payload, [0x08, 0x03, 0x22, 0x00]);
        assert_eq!(read_diagnostics(Some(42)).message_type, 7);
        assert_eq!(read_diagnostics(Some(42)).payload, [0x08, 0x03, 0x10, 0x2a]);
        let tap = screen_tap(184.0, 147.0);
        assert_eq!(tap.len(), 1);
        assert_eq!(
            tap[0].payload,
            [
                0x08, 0x01, 0x1a, 0x0c, 0x0d, 0x00, 0x00, 0x38, 0x43, 0x15, 0x00, 0x00, 0x13, 0x43,
                0x18, 0x03
            ]
        );
        // A hold is a touch-down then a touch-up: wire types 0 (PRESS) then 1
        // (RELEASE). Type 0 is the proto3 default and is not written.
        let hold = screen_hold(184.0, 147.0);
        assert_eq!(
            hold[0].payload,
            [0x08, 0x01, 0x1a, 0x0a, 0x0d, 0x00, 0x00, 0x38, 0x43, 0x15, 0x00, 0x00, 0x13, 0x43]
        );
        assert_eq!(
            hold[1].payload,
            [
                0x08, 0x01, 0x1a, 0x0c, 0x0d, 0x00, 0x00, 0x38, 0x43, 0x15, 0x00, 0x00, 0x13, 0x43,
                0x18, 0x01
            ]
        );
        let drag = screen_drag(400.0, 8.0, 400.0, 220.0);
        assert_eq!(drag.len(), 1, "a swipe is one DRAG frame and nothing else");
        let remote = pa::RemoteControlMessage::decode(drag[0].payload.as_slice()).unwrap();
        let mouse = remote.mouse.unwrap();
        assert_eq!(mouse.r#type, pa::remote_control_mouse::Type::Drag as i32);
        assert_eq!(
            (mouse.x, mouse.y, mouse.to_x, mouse.to_y),
            (400.0, 8.0, 400.0, 220.0)
        );
    }

    #[test]
    fn swipe_screen_sends_exactly_one_drag_frame_with_no_begin_or_end_frame() {
        // Observed on CorOS 4.1.0 (2026-10-06): one DRAG frame scrolls a list
        // and leaves no row highlighted. The earlier begin/DRAG/end sequence did
        // not scroll at 20 ms spacing and its end frame activated the row under
        // the end point.
        let frames = screen_drag(1.0, 2.0, 3.0, 4.0);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, 72);
        assert_eq!(
            frames[0].payload,
            [
                0x08, 0x01, 0x1a, 0x16, 0x0d, 0x00, 0x00, 0x80, 0x3f, 0x15, 0x00, 0x00, 0x00, 0x40,
                0x18, 0x04, 0x25, 0x00, 0x00, 0x40, 0x40, 0x2d, 0x00, 0x00, 0x80, 0x40
            ]
        );
        let decoded = pa::RemoteControlMessage::decode(frames[0].payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(decoded.request_id, None);
        assert!(decoded.screenshot.is_none());
        assert!(decoded.graphics_tree.is_none());
        let mouse = decoded.mouse.unwrap();
        assert_eq!(mouse.r#type, pa::remote_control_mouse::Type::Drag as i32);
        // Neither touch-down (PRESS) nor touch-up (RELEASE) is ever sent.
        assert_ne!(mouse.r#type, pa::remote_control_mouse::Type::Release as i32);
        assert_ne!(mouse.r#type, pa::remote_control_mouse::Type::Press as i32);
        assert_eq!(
            (mouse.x, mouse.y, mouse.to_x, mouse.to_y),
            (1.0, 2.0, 3.0, 4.0)
        );
        // The operation encodes to the same single frame, and has no
        // touch-up to send if a host stops it.
        let operation = DeviceOperation::ScreenDrag {
            x: 1.0,
            y: 2.0,
            to_x: 3.0,
            to_y: 4.0,
        };
        assert_eq!(operation.abort_release_frame(), None);
        assert_eq!(operation.try_encode().unwrap(), frames.to_vec());
    }

    #[test]
    fn mouse_frame_sends_exactly_one_frame_of_each_protocol_type() {
        // The five RemoteControlMouse.Type values by wire number.
        let expected = [
            (ScreenMouseType::Press, 0, "press"),
            (ScreenMouseType::Release, 1, "release"),
            (ScreenMouseType::Move, 2, "move"),
            (ScreenMouseType::Tap, 3, "tap"),
            (ScreenMouseType::Drag, 4, "drag"),
        ];
        assert_eq!(expected.len(), ScreenMouseType::ALL.len());
        for (kind, wire, name) in expected {
            assert_eq!(kind.name(), name);
            assert_eq!(ScreenMouseType::from_name(name), Some(kind));
            let to = (kind == ScreenMouseType::Drag).then_some((30.0, 40.0));
            let frames = DeviceOperation::ScreenMouseFrame {
                kind,
                x: 10.0,
                y: 20.0,
                to,
            }
            .try_encode()
            .unwrap();
            assert_eq!(frames.len(), 1, "{name} is one frame and nothing else");
            let mouse = decoded_mouse(&frames[0]);
            assert_eq!(mouse.r#type, wire, "{name}");
            assert_eq!((mouse.x, mouse.y), (10.0, 20.0));
            assert_eq!((mouse.to_x, mouse.to_y), to.unwrap_or((0.0, 0.0)), "{name}");
        }
        // Names are exactly the five lowercase words.
        for wrong in ["", "Press", "PRESS", "touch", "down", "up", "swipe", "tap "] {
            assert_eq!(ScreenMouseType::from_name(wrong), None, "{wrong:?}");
        }
        // Convenience builders are the same bytes as the raw frame.
        assert_eq!(
            screen_tap(5.0, 6.0).to_vec(),
            vec![screen_mouse_frame(ScreenMouseType::Tap, 5.0, 6.0, None)]
        );
        assert_eq!(
            screen_drag(5.0, 6.0, 7.0, 8.0).to_vec(),
            vec![screen_mouse_frame(
                ScreenMouseType::Drag,
                5.0,
                6.0,
                Some((7.0, 8.0))
            )]
        );
    }

    #[test]
    fn mouse_frame_refuses_bad_pixels_and_misplaced_destinations() {
        let frame = |kind, x, y, to| DeviceOperation::ScreenMouseFrame { kind, x, y, to };
        assert!(frame(ScreenMouseType::Release, 799.0, 479.0, None)
            .try_encode()
            .is_ok());
        assert!(frame(ScreenMouseType::Drag, 0.0, 0.0, Some((799.0, 479.0)))
            .try_encode()
            .is_ok());
        assert!(!ScreenMouseType::ALL.is_empty());
        for (x, y) in [
            (-1.0, 0.0),
            (0.5, 0.0),
            (800.0, 0.0),
            (0.0, 480.0),
            (f32::NAN, 0.0),
            (0.0, f32::INFINITY),
        ] {
            for kind in ScreenMouseType::ALL {
                let to = (kind == ScreenMouseType::Drag).then_some((1.0, 1.0));
                assert!(
                    frame(kind, x, y, to).try_encode().is_err(),
                    "{kind:?} {x} {y}"
                );
            }
        }
        // A drag needs a destination, on screen and distinct from its start.
        assert!(frame(ScreenMouseType::Drag, 1.0, 1.0, None)
            .try_encode()
            .is_err());
        assert!(frame(ScreenMouseType::Drag, 1.0, 1.0, Some((1.0, 1.0)))
            .try_encode()
            .is_err());
        assert!(frame(ScreenMouseType::Drag, 1.0, 1.0, Some((800.0, 1.0)))
            .try_encode()
            .is_err());
        assert!(frame(ScreenMouseType::Drag, 1.0, 1.0, Some((1.0, 480.0)))
            .try_encode()
            .is_err());
        // Every other type refuses a destination rather than ignoring it.
        for kind in [
            ScreenMouseType::Press,
            ScreenMouseType::Release,
            ScreenMouseType::Move,
            ScreenMouseType::Tap,
        ] {
            assert!(
                frame(kind, 1.0, 1.0, Some((2.0, 2.0)))
                    .try_encode()
                    .is_err(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn no_gateway_gesture_leaves_a_press_without_a_later_release() {
        // PRESS is touch-down and RELEASE is touch-up (observed on CorOS 4.1.0,
        // 2026-10-06). The old tap (RELEASE then PRESS) and the old swipe
        // (RELEASE, DRAG, PRESS) ended on a PRESS that was never released and left
        // a stuck pointer on the unit. Every gesture the gateway builds must
        // balance its presses; only the explicit raw-frame command may not.
        use pa::remote_control_mouse::Type;
        let gestures = [
            DeviceOperation::ScreenTap { x: 10.0, y: 20.0 },
            DeviceOperation::ScreenHold { x: 10.0, y: 20.0 },
            DeviceOperation::ScreenDrag {
                x: 10.0,
                y: 20.0,
                to_x: 30.0,
                to_y: 40.0,
            },
            DeviceOperation::ScreenDragSteps {
                x: 10.0,
                y: 20.0,
                to_x: 30.0,
                to_y: 40.0,
                steps: 5,
                hold_ms: 100,
            },
        ];
        for operation in gestures {
            let types: Vec<i32> = operation
                .clone()
                .try_encode()
                .unwrap()
                .iter()
                .map(|frame| decoded_mouse(frame).r#type)
                .collect();
            let mut down = false;
            for wire in &types {
                if *wire == Type::Press as i32 {
                    assert!(!down, "{operation:?}: PRESS while already down");
                    down = true;
                } else if *wire == Type::Release as i32 {
                    assert!(down, "{operation:?}: RELEASE with nothing pressed");
                    down = false;
                }
            }
            assert!(
                !down,
                "{operation:?} ends with the pointer still down: {types:?}"
            );
        }
        // The raw frame is the one deliberate exception: a lone PRESS stays down.
        let lone = DeviceOperation::ScreenMouseFrame {
            kind: ScreenMouseType::Press,
            x: 1.0,
            y: 2.0,
            to: None,
        };
        assert_eq!(lone.try_encode().unwrap().len(), 1);
    }

    fn decoded_mouse(frame: &OutboundMessage) -> pa::RemoteControlMouse {
        assert_eq!(frame.message_type, profile::MESSAGE_TYPE_REMOTE_CONTROL);
        let message = pa::RemoteControlMessage::decode(frame.payload.as_slice()).unwrap();
        assert_eq!(message.action, pa::message_action::Enum::Update as i32);
        assert_eq!(message.request_id, None);
        assert!(message.screenshot.is_none());
        assert!(message.graphics_tree.is_none());
        message.mouse.unwrap()
    }

    #[test]
    fn stepped_drag_default_frame_sequence_is_exact() {
        use pa::remote_control_mouse::Type;
        let frames = screen_drag_steps(
            400.0,
            100.0,
            400.0,
            300.0,
            SCREEN_DRAG_DEFAULT_STEPS,
            SCREEN_DRAG_DEFAULT_HOLD_MS,
        );
        // down + ceil(200/50)=4 start MOVEs + 18 steps + ceil(300/50)=6 end MOVEs + up
        assert_eq!(frames.len(), 1 + 4 + 18 + 6 + 1);
        assert_eq!(frames.len() as u32, screen_drag_frame_count(18, 200));
        let mouse: Vec<_> = frames.iter().map(decoded_mouse).collect();
        // The labels are literal: PRESS begins the touch, RELEASE ends it.
        assert_eq!(mouse[0].r#type, Type::Press as i32);
        assert_eq!((mouse[0].x, mouse[0].y), (400.0, 100.0));
        let last = mouse.len() - 1;
        assert_eq!(mouse[last].r#type, Type::Release as i32);
        assert_eq!((mouse[last].x, mouse[last].y), (400.0, 300.0));
        // No frame is the atomic DRAG type and none carries a destination.
        assert!(mouse.iter().all(|frame| frame.r#type != Type::Drag as i32
            && frame.to_x == 0.0
            && frame.to_y == 0.0));
        // Everything between down and up is a MOVE.
        assert!(mouse[1..last]
            .iter()
            .all(|frame| frame.r#type == Type::Move as i32));
        // The first MOVE is at the start point and the last MOVE at the end point.
        assert_eq!((mouse[1].x, mouse[1].y), (400.0, 100.0));
        assert_eq!((mouse[last - 1].x, mouse[last - 1].y), (400.0, 300.0));
        // Start dwell: 4 frames at the start point.
        for frame in &mouse[1..=4] {
            assert_eq!((frame.x, frame.y), (400.0, 100.0));
        }
        // Interpolated moves: strictly increasing y, x fixed, the 18th exactly at the end.
        let moves = &mouse[5..5 + 18];
        for pair in moves.windows(2) {
            assert!(pair[1].y > pair[0].y);
        }
        assert!(moves.iter().all(|frame| frame.x == 400.0));
        assert!((moves[0].y - (100.0 + 200.0 / 18.0)).abs() < 1e-3);
        assert_eq!((moves[17].x, moves[17].y), (400.0, 300.0));
        // End dwell: 6 frames at the end point.
        for frame in &mouse[5 + 18..last] {
            assert_eq!((frame.x, frame.y), (400.0, 300.0));
        }
        assert_eq!(mouse[5 + 18..last].len(), 6);
        // Whole-gesture duration at the uniform 50 ms interval.
        assert_eq!(screen_drag_duration_ms(18, 200), 29 * 50);
    }

    #[test]
    fn stepped_drag_interpolation_is_exact_at_endpoints_and_diagonal() {
        let frames = screen_drag_steps(10.0, 20.0, 110.0, 70.0, 5, 0);
        let mouse: Vec<_> = frames.iter().map(decoded_mouse).collect();
        // hold 0: no start dwell; the fixed 300 ms end dwell is 6 MOVEs.
        assert_eq!(mouse.len(), 1 + 5 + 6 + 1);
        let points: Vec<_> = mouse[1..6].iter().map(|m| (m.x, m.y)).collect();
        assert_eq!(
            points,
            [
                (30.0, 30.0),
                (50.0, 40.0),
                (70.0, 50.0),
                (90.0, 60.0),
                (110.0, 70.0)
            ]
        );
        assert_eq!((mouse[6].x, mouse[6].y), (110.0, 70.0));
        assert_eq!((mouse[7].x, mouse[7].y), (110.0, 70.0));
        // A reverse drag interpolates downwards and still lands exactly.
        let back = screen_drag_steps(799.0, 479.0, 0.0, 0.0, 3, 0);
        let end: Vec<_> = back.iter().map(decoded_mouse).collect();
        assert_eq!((end[3].x, end[3].y), (0.0, 0.0));
        assert!(end[1].x < 799.0 && end[1].x > end[2].x);
    }

    #[test]
    fn stepped_drag_frame_budget_is_bounded_by_the_duration_cap() {
        for (steps, hold_ms, frames) in [(2, 0, 10), (18, 200, 30), (40, 1000, 68), (40, 1500, 78)]
        {
            assert_eq!(screen_drag_frame_count(steps, hold_ms), frames);
            let encoded = DeviceOperation::ScreenDragSteps {
                x: 1.0,
                y: 2.0,
                to_x: 3.0,
                to_y: 4.0,
                steps,
                hold_ms,
            }
            .try_encode()
            .unwrap();
            assert_eq!(encoded.len() as u32, frames);
        }
        assert_eq!(screen_drag_duration_ms(40, 1500), 77 * 50);
        assert!(
            screen_drag_duration_ms(40, SCREEN_DRAG_MAX_HOLD_MS) <= SCREEN_DRAG_MAX_DURATION_MS
        );
        // A hold that is not a whole interval rounds up.
        assert_eq!(screen_drag_hold_frames(1), 1);
        assert_eq!(screen_drag_hold_frames(50), 1);
        assert_eq!(screen_drag_hold_frames(51), 2);
        // The end dwell is fixed, whatever the start dwell is.
        assert_eq!(screen_drag_end_frames(), 6);
    }

    #[test]
    fn a_press_then_release_gesture_has_a_release_frame_equal_to_its_last_frame_and_nothing_else_does(
    ) {
        let operation = DeviceOperation::ScreenDragSteps {
            x: 10.0,
            y: 20.0,
            to_x: 110.0,
            to_y: 70.0,
            steps: 4,
            hold_ms: 0,
        };
        let frames = operation.clone().try_encode().unwrap();
        let release = operation.abort_release_frame().expect("a release frame");
        assert_eq!(release, *frames.last().unwrap());
        assert_eq!(release, screen_drag_release(110.0, 70.0));
        let mouse = decoded_mouse(&release);
        assert_eq!(mouse.r#type, pa::remote_control_mouse::Type::Release as i32);
        assert_eq!((mouse.x, mouse.y), (110.0, 70.0));

        // A hold presses and then releases at one point, so a host that loses the
        // touch-up has exactly one frame to send: a RELEASE byte-identical to the
        // hold's own last frame, at the held point. It is never a PRESS.
        let hold = DeviceOperation::ScreenHold { x: 123.0, y: 45.0 };
        let hold_frames = hold.clone().try_encode().unwrap();
        assert_eq!(hold_frames.len(), 2);
        let hold_release = hold.abort_release_frame().expect("a release frame");
        assert_eq!(hold_release, hold_frames[1]);
        assert_eq!(
            decoded_mouse(&hold_frames[0]).r#type,
            pa::remote_control_mouse::Type::Press as i32
        );
        let hold_mouse = decoded_mouse(&hold_release);
        assert_eq!(
            hold_mouse.r#type,
            pa::remote_control_mouse::Type::Release as i32
        );
        assert_eq!((hold_mouse.x, hold_mouse.y), (123.0, 45.0));

        // Every other operation keeps its exact sequence and has no release
        // frame: a raw press is tracked by the touch guard, a tap and a swipe are
        // single frames that begin no held touch.
        for other in [
            DeviceOperation::ScreenMouseFrame {
                kind: ScreenMouseType::Press,
                x: 1.0,
                y: 2.0,
                to: None,
            },
            DeviceOperation::ScreenTap { x: 1.0, y: 2.0 },
            DeviceOperation::ScreenDrag {
                x: 1.0,
                y: 2.0,
                to_x: 3.0,
                to_y: 4.0,
            },
            DeviceOperation::Undo,
        ] {
            assert!(other.abort_release_frame().is_none());
        }
    }

    #[test]
    fn stepped_drag_operation_refuses_out_of_range_input_with_a_typed_error() {
        let valid = |steps, hold_ms| DeviceOperation::ScreenDragSteps {
            x: 400.0,
            y: 100.0,
            to_x: 400.0,
            to_y: 300.0,
            steps,
            hold_ms,
        };
        assert!(valid(2, 0).try_encode().is_ok());
        assert!(valid(40, 1500).try_encode().is_ok());
        for (steps, hold_ms, field) in [
            (1, 200, "screen drag steps"),
            (0, 200, "screen drag steps"),
            (41, 200, "screen drag steps"),
            (18, 1501, "screen drag hold"),
        ] {
            assert!(matches!(
                valid(steps, hold_ms).try_encode(),
                Err(CommandEncodeError::UnsupportedValue { field: reported, .. })
                    if reported == field
            ));
        }
        for (x, y, to_x, to_y) in [
            (-1.0, 0.0, 5.0, 5.0),
            (800.0, 0.0, 5.0, 5.0),
            (0.5, 0.0, 5.0, 5.0),
            (0.0, 480.0, 5.0, 5.0),
            (0.0, 0.0, 800.0, 5.0),
            (0.0, 0.0, 5.0, 480.0),
            (f32::NAN, 0.0, 5.0, 5.0),
            (0.0, 0.0, f32::INFINITY, 5.0),
            (7.0, 9.0, 7.0, 9.0),
        ] {
            assert!(
                DeviceOperation::ScreenDragSteps {
                    x,
                    y,
                    to_x,
                    to_y,
                    steps: 18,
                    hold_ms: 200,
                }
                .try_encode()
                .is_err(),
                "({x}, {y}) -> ({to_x}, {to_y}) must be refused"
            );
        }
    }

    #[test]
    fn preset_screenshot_extension_is_byte_exact_and_position_bounded() {
        let frames = DeviceOperation::PresetScreenshot {
            folder_name: "X".into(),
            position: 255,
            is_factory: true,
            request_id: 300,
        }
        .try_encode()
        .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, 25);
        assert_eq!(
            frames[0].payload,
            [0x08, 0x03, 0x10, 0xac, 0x02, 0x1a, 0x01, b'X', 0x20, 0x01, 0x28, 0xff, 0x01,]
        );

        assert!(matches!(
            DeviceOperation::PresetScreenshot {
                folder_name: "X".into(),
                position: 256,
                is_factory: false,
                request_id: 1,
            }
            .try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "position",
                ..
            })
        ));
    }

    #[test]
    fn undo_device_matches_exact_sparse_frame() {
        let direct = undo();
        assert_eq!(direct.message_type, profile::MESSAGE_TYPE_UNDO_REDO);
        assert_eq!(direct.payload, [0x08, 0x01, 0x28, 0x01]);

        let operation = DeviceOperation::Undo.try_encode().unwrap();
        assert_eq!(operation.len(), 1);
        assert_eq!(operation[0], direct);

        let decoded = pa::UndoRedoMessage::decode(direct.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(decoded.undo, Some(pa::undo_redo_message::Undo::Undo(true)));
        assert_eq!(decoded.request_id, None);
        assert!(decoded.undo_frames.is_empty());
        assert!(decoded.redo_frames.is_empty());
        assert_eq!(decoded.redo, None);
    }

    #[test]
    fn redo_device_matches_exact_wire_contract() {
        let outbound = redo();
        assert_eq!(outbound.message_type, profile::MESSAGE_TYPE_UNDO_REDO);
        assert_eq!(outbound.payload, [0x08, 0x01, 0x30, 0x01]);

        let decoded = pa::UndoRedoMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert!(decoded.request_id.is_none());
        assert!(decoded.undo_frames.is_empty());
        assert!(decoded.redo_frames.is_empty());
        assert!(decoded.undo.is_none());
        assert_eq!(decoded.redo, Some(pa::undo_redo_message::Redo::Redo(true)));

        assert_eq!(DeviceOperation::Redo.try_encode().unwrap(), vec![outbound]);
    }

    #[test]
    fn select_scene_is_one_sparse_update_with_strict_domain() {
        let message = DeviceCommand::SelectScene(3).try_encode().unwrap();
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_SCENE);
        assert_eq!(message.payload, [0x08, 0x01, 0x18, 0x03]);
        assert_eq!(
            DeviceCommand::SelectScene(domain::SCENE_COUNT).try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "scene",
                value: domain::SCENE_COUNT.to_string(),
            })
        );
    }

    #[test]
    fn show_tuner_operation_matches_the_expected_three_frame_workflow() {
        for (shown, expected_meter) in [(false, false), (true, true)] {
            let frames = DeviceOperation::ShowTuner(shown).try_encode().unwrap();
            assert_eq!(frames.len(), 3);
            assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_SHOW_TUNER);
            assert_eq!(frames[0].payload, [0x08, 0x01, 0x18, u8::from(shown)]);
            assert_eq!(frames[1].message_type, profile::MESSAGE_TYPE_TUNER);
            assert_eq!(
                frames[1].payload,
                [0x08, 0x01, 0x30, u8::from(expected_meter)]
            );
            assert_eq!(frames[2].message_type, profile::MESSAGE_TYPE_TUNER);
            assert_eq!(frames[2].payload, [0x08, 0x03]);
        }

        let live_view_frames = show_tuner_workflow(false, true);
        assert_eq!(live_view_frames[1].payload, [0x08, 0x01, 0x30, 0x01]);
    }

    #[test]
    fn select_mode_is_one_sparse_update_with_strict_domain() {
        for mode in 0..=2 {
            let message = DeviceCommand::SelectMode(mode).try_encode().unwrap();
            assert_eq!(message.message_type, profile::MESSAGE_TYPE_MODE);
            assert_eq!(message.payload, [0x08, 0x01, 0x18, mode as u8]);
        }
        // The hybrid values 3 to 8 are unmeasured, and 9 is undefined: none is encoded.
        for mode in 3..=9 {
            assert_eq!(
                DeviceCommand::SelectMode(mode).try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "mode",
                    value: mode.to_string(),
                })
            );
        }
    }

    #[test]
    fn tuner_input_domain_and_sparse_wire_shape_match_expected_contract() {
        for input_port_id in TUNER_INPUT_PORT_IDS {
            let encoded = DeviceOperation::SetTunerInput(*input_port_id)
                .try_encode()
                .unwrap();
            assert_eq!(encoded.len(), 1);
            assert_eq!(encoded[0].message_type, 6);
            assert_eq!(encoded[0].payload, [0x08, 0x01, 0x18, *input_port_id as u8]);
        }
        for input_port_id in [0, 6, 7, 10] {
            assert_eq!(
                DeviceOperation::SetTunerInput(input_port_id).try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "tuner input port ID",
                    value: input_port_id.to_string(),
                })
            );
        }
    }

    #[test]
    fn tuner_mute_is_one_sparse_boolean_frame_for_each_polarity() {
        for (muted, expected) in [
            (false, [0x08, 0x01, 0x28, 0x00]),
            (true, [0x08, 0x01, 0x28, 0x01]),
        ] {
            let frames = DeviceOperation::SetTunerMute(muted).try_encode().unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_TUNER);
            assert_eq!(frames[0].payload, expected);
            let decoded = pa::TunerMessage::decode(frames[0].payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(decoded.mute, Some(pa::tuner_message::Mute::Mute(muted)));
            assert!(decoded.request_id.is_none());
            assert!(decoded.input_port_id.is_none());
            assert!(decoded.frequency.is_none());
            assert!(decoded.enable_meter.is_none());
            assert!(decoded.meter.is_none());
        }
    }

    #[test]
    fn parameter_and_bypass_commands_round_trip_through_the_schema() {
        let numeric = set_parameter_numeric(2, 5, 7, 0.25);
        assert_eq!(
            numeric.payload,
            [
                0x08, 0x01, 0x1a, 0x13, 0x5a, 0x11, 0x2a, 0x0d, 0x12, 0x09, 0x2a, 0x05, 0x15, 0x00,
                0x00, 0x80, 0x3e, 0x30, 0x07, 0x28, 0x05, 0x70, 0x02,
            ]
        );
        let grid = pa::GridMessage::decode(numeric.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let parameter = &preset.chains[0].models[0].params[0];
        assert_eq!(preset.chains[0].row, Some(chain::Row::Row(2)));
        assert_eq!(
            preset.chains[0].models[0].column,
            Some(model::Column::Column(5))
        );
        assert_eq!(parameter.index, Some(param::Index::Index(7)));
        assert!(matches!(
            parameter.param_values[0].value,
            Some(param_value::Value::FloatValue(value)) if value == 0.25
        ));

        let bypass = set_bypass(1, 4, true);
        assert_eq!(bypass.message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(
            bypass.payload,
            [8, 1, 26, 13, 146, 1, 10, 10, 6, 10, 2, 8, 1, 16, 4, 16, 1]
        );
        let grid = pa::GridMessage::decode(bypass.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert!(preset.bypass[0].col_bypass[0].scene_bypass[0].bypass);

        let enabled = set_bypass(0, 0, false);
        assert_eq!(
            enabled.payload,
            [8, 1, 26, 11, 146, 1, 8, 10, 4, 10, 0, 16, 0, 16, 0]
        );
        let grid = pa::GridMessage::decode(enabled.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(preset.bypass[0].row, Some(bypass::Row::Row(0)));
        assert_eq!(
            preset.bypass[0].col_bypass[0].column,
            Some(col_bypass::Column::Column(0))
        );
        assert!(!preset.bypass[0].col_bypass[0].scene_bypass[0].bypass);

        for invalid in [
            DeviceCommand::SetBypass {
                row: domain::GRID_ROWS,
                column: 0,
                bypassed: true,
            },
            DeviceCommand::SetBypass {
                row: 0,
                column: domain::GRID_COLUMNS,
                bypassed: true,
            },
        ] {
            assert!(invalid.try_encode().is_err());
        }
    }

    #[test]
    fn looper_assignment_update_preserves_physical_a_through_h_order() {
        let assignments = vec![3, 4, 5, 6, 0, 1, 7, 2];
        let frames = DeviceOperation::SetLooperStompAssignments {
            assignments: assignments.clone(),
        }
        .try_encode()
        .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].message_type,
            profile::MESSAGE_TYPE_GENERAL_SETTINGS
        );
        let decoded = pa::GeneralSettingsMessage::decode(frames[0].payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(decoded.looper_stomp_assignments, assignments);
        assert!(DeviceOperation::SetLooperStompAssignments {
            assignments: vec![0, 1, 2, 3, 4, 5, 6, 6]
        }
        .try_encode()
        .is_err());
    }

    #[test]
    fn model_preset_create_addresses_the_live_block_and_user_catalog_entry() {
        let frames = DeviceOperation::CreateModelPreset {
            row: 2,
            column: 4,
            model_hash: 31_000,
            name: "My Looper".into(),
        }
        .try_encode()
        .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_MODEL_PRESET);
        let decoded = pa::ModelPresetMessage::decode(frames[0].payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Create as i32);
        assert_eq!(
            decoded.create_from_row,
            Some(pa::model_preset_message::CreateFromRow::CreateFromRow(2))
        );
        assert_eq!(
            decoded.create_from_column,
            Some(pa::model_preset_message::CreateFromColumn::CreateFromColumn(4))
        );
        assert_eq!(
            decoded.create_from_hash,
            Some(pa::model_preset_message::CreateFromHash::CreateFromHash(
                31_000
            ))
        );
        assert_eq!(decoded.presets.len(), 1);
        let preset = &decoded.presets[0];
        assert_eq!(
            preset.id,
            Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                value: Some(pa::model_preset_id::Value::Value("My Looper".into())),
                is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                hash: Some(pa::model_preset_id::Hash::Hash(31_000)),
            }))
        );
        assert_eq!(
            preset.is_default,
            Some(pa::model_preset::IsDefault::IsDefault(false))
        );
        assert_eq!(
            preset.name,
            Some(pa::model_preset::Name::Name("My Looper".into()))
        );
    }

    #[test]
    fn replace_block_sends_the_same_single_frame_as_add_block() {
        let replace = DeviceOperation::ReplaceBlock {
            row: 1,
            column: 3,
            model_id: 31_000,
        }
        .try_encode()
        .unwrap();
        assert_eq!(replace, vec![set_block(1, 3, 31_000)]);
    }

    #[test]
    fn remove_block_uses_only_the_cortex_row_and_column_selector() {
        let message = remove_block(2, 5);
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(message.payload, [8, 2, 26, 8, 90, 6, 42, 2, 40, 5, 112, 2]);
        let grid = pa::GridMessage::decode(message.payload.as_slice()).unwrap();
        assert_eq!(grid.action, pa::message_action::Enum::Delete as i32);
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let model = &preset.chains[0].models[0];
        assert_eq!(preset.chains[0].row, Some(chain::Row::Row(2)));
        assert_eq!(model.column, Some(model::Column::Column(5)));
        assert!(
            model.hash.is_none(),
            "Cortex removeModel does not emit hash=0"
        );
    }

    #[test]
    fn reset_block_parameters_uses_atomic_default_parameter_update() {
        let message = reset_block_parameters(2, 5);
        assert_eq!(
            message.message_type,
            profile::MESSAGE_TYPE_DEFAULT_PARAMETERS
        );
        assert_eq!(message.payload, [8, 1, 24, 2, 32, 5]);
        let decoded = pa::DefaultParametersMessage::decode(message.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!((decoded.row, decoded.column), (2, 5));
        assert!(decoded.all_defaults.is_empty());
        assert!(decoded.request_id.is_none());
    }

    #[test]
    fn move_block_sends_bare_frame_without_grid_sidecar() {
        let message = move_block(1, 2, 3, 4, vec![vec![11, 12], vec![], vec![21], vec![]]);
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_GRID_MOVE);
        assert_eq!(
            message.payload,
            vec![18, 10, 8, 1, 16, 2, 24, 3, 32, 4, 40, 1]
        );
        let decoded = pa::GridMoveMessage::decode(message.payload.as_slice()).unwrap();
        assert!(decoded.request_id.is_none());
        assert!(decoded.grid.is_none());
    }

    #[test]
    fn move_block_operation_rejects_missing_grid_context() {
        let operation = DeviceOperation::MoveBlock {
            from_row: 0,
            from_column: 0,
            to_row: 1,
            to_column: 2,
            grid_model_ids: Vec::new(),
        };
        assert!(operation
            .try_encode()
            .unwrap_err()
            .to_string()
            .contains("move grid context"));
    }

    #[test]
    fn io_port_updates_send_exactly_one_field_per_message() {
        let messages = set_input_port(2, Some(0.4), Some(0.875), Some(0.5), Some(0.0));
        assert_eq!(messages.len(), 4);
        for message in messages {
            assert_eq!(message.message_type, 3);
            let decoded = pa::IoSettingsMessage::decode(message.payload.as_slice()).unwrap();
            let pa::io_settings_message::Settings::Settings(settings) = decoded.settings.unwrap();
            assert_eq!(settings.in_port.len(), 1);
            let port = &settings.in_port[0];
            assert_eq!(port.input_port_id, 2);
            let populated = [
                port.level.is_some(),
                port.input_zmode.is_some(),
                port.input_type.is_some(),
                port.ground_lift.is_some(),
            ]
            .into_iter()
            .filter(|value| *value)
            .count();
            assert_eq!(populated, 1);
        }

        let messages = set_output_port(1, Some(0.25), Some(1.0), Some(true));
        assert_eq!(messages.len(), 3);
        for message in messages {
            let decoded = pa::IoSettingsMessage::decode(message.payload.as_slice()).unwrap();
            let pa::io_settings_message::Settings::Settings(settings) = decoded.settings.unwrap();
            let port = &settings.out_port[0];
            let populated = [
                port.level.is_some(),
                port.ground_lift.is_some(),
                port.mute.is_some(),
            ]
            .into_iter()
            .filter(|value| *value)
            .count();
            assert_eq!(populated, 1, "QC drops some coalesced I/O fields");
        }

        let messages = set_usb_port(Some(0.2), Some(0.5), Some(1.0));
        assert_eq!(messages.len(), 3);
        for message in messages {
            let decoded = pa::IoSettingsMessage::decode(message.payload.as_slice()).unwrap();
            let pa::io_settings_message::Settings::Settings(settings) = decoded.settings.unwrap();
            let pa::port_settings::UsbPort::UsbPort(port) = settings.usb_port.unwrap();
            let populated = [
                port.level.is_some(),
                port.hp_select.is_some(),
                port.dry_wet.is_some(),
            ]
            .into_iter()
            .filter(|value| *value)
            .count();
            assert_eq!(populated, 1);
        }
    }

    #[test]
    fn output_port_batch_preserves_cortex_property_order_and_physical_values() {
        let messages = set_output_port(4, Some(-17.5), Some(1.0), Some(false));
        assert_eq!(messages.len(), 3);
        let ports = messages
            .iter()
            .map(|message| {
                assert_eq!(message.message_type, profile::MESSAGE_TYPE_IO_SETTINGS);
                let decoded = pa::IoSettingsMessage::decode(message.payload.as_slice()).unwrap();
                assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
                let pa::io_settings_message::Settings::Settings(settings) =
                    decoded.settings.unwrap();
                assert_eq!(settings.out_port.len(), 1);
                settings.out_port.into_iter().next().unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(ports[0].output_port_id, 4);
        assert!(matches!(
            ports[0].level,
            Some(pa::output_port_settings::Level::Level(value))
                if value == output_level_db_to_wire(-17.5)
        ));
        assert!(ports[0].ground_lift.is_none() && ports[0].mute.is_none());
        assert!(matches!(
            ports[1].ground_lift,
            Some(pa::output_port_settings::GroundLift::GroundLift(value)) if value == 1.0
        ));
        assert!(ports[1].level.is_none() && ports[1].mute.is_none());
        assert!(matches!(
            ports[2].mute,
            Some(pa::output_port_settings::Mute::Mute(false))
        ));
        assert!(ports[2].level.is_none() && ports[2].ground_lift.is_none());
    }

    fn general_integer_wire(setting: &str, value: i32) -> Option<i32> {
        let message = set_general_integer(setting, value).unwrap();
        let decoded = pa::GeneralSettingsMessage::decode(message.payload.as_slice()).unwrap();
        match setting {
            "screenBrightness" => decoded
                .screen_brightness
                .map(|pa::general_settings_message::ScreenBrightness::ScreenBrightness(wire)| wire),
            "ledBrightness" => decoded
                .led_brightness
                .map(|pa::general_settings_message::LedBrightness::LedBrightness(wire)| wire),
            "dimmedLedBrightness" => decoded.dimmed_led_brightness.map(
                |pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(wire)| wire,
            ),
            other => panic!("{other} is not a brightness setting"),
        }
    }

    fn general_integer_read_back(setting: &str, value: i32) -> Option<i32> {
        let message = set_general_integer(setting, value).unwrap();
        let settings = crate::responses::decode_general_settings(&message.payload).unwrap();
        match setting {
            "screenBrightness" => settings.screen_brightness,
            "ledBrightness" => settings.led_brightness,
            "dimmedLedBrightness" => settings.dimmed_led_brightness,
            other => panic!("{other} is not a brightness setting"),
        }
    }

    #[test]
    fn brightness_writes_reach_the_percent_wire_field_the_read_projects_from() {
        // The device carries brightness as a 0..100 percent, quantized onto its
        // own 32-step ladder of 100/32 = 3.125 percent per step. The published
        // step domain has a ceiling of 32, so a public step value written
        // directly to the wire without this scaling could never reach the
        // wire's full 100 percent range.
        for (setting, public, wire) in [
            ("ledBrightness", 0, 0),
            ("ledBrightness", 10, 31),
            ("ledBrightness", 32, 100),
            ("screenBrightness", 1, 3),
            ("screenBrightness", 16, 50),
            ("screenBrightness", 32, 100),
            ("dimmedLedBrightness", 0, 0),
            ("dimmedLedBrightness", 2, 6),
            ("dimmedLedBrightness", 31, 97),
        ] {
            assert_eq!(
                general_integer_wire(setting, public),
                Some(wire),
                "{setting} {public} must reach the wire as {wire} percent"
            );
        }
    }

    #[test]
    fn every_brightness_value_read_from_the_device_can_be_written_back() {
        for (setting, minimum, maximum) in [
            ("screenBrightness", 1, 32),
            ("ledBrightness", 0, 32),
            ("dimmedLedBrightness", 0, 31),
        ] {
            for value in minimum..=maximum {
                assert!(is_general_integer_value(setting, value));
                assert_eq!(
                    general_integer_read_back(setting, value),
                    Some(value),
                    "{setting} {value} did not survive a write followed by the read projection"
                );
            }
        }
    }

    #[test]
    fn non_brightness_general_integers_stay_on_the_wire_unchanged() {
        for (setting, value, expected) in [
            ("holdTiming", 5, 5),
            ("midiChannel", 17, 17),
            ("midiClockOut", 3, 3),
        ] {
            let message = set_general_integer(setting, value).unwrap();
            let decoded = pa::GeneralSettingsMessage::decode(message.payload.as_slice()).unwrap();
            let wire = match setting {
                "holdTiming" => decoded
                    .hold_timing
                    .map(|pa::general_settings_message::HoldTiming::HoldTiming(wire)| wire),
                "midiChannel" => decoded
                    .midi_channel
                    .map(|pa::general_settings_message::MidiChannel::MidiChannel(wire)| wire),
                _ => decoded
                    .midi_clock_out
                    .map(|pa::general_settings_message::MidiClockOut::MidiClockOut(wire)| wire),
            };
            assert_eq!(wire, Some(expected));
        }
    }

    fn sole_out_port_level(messages: &[OutboundMessage]) -> f32 {
        assert_eq!(messages.len(), 1);
        let decoded = pa::IoSettingsMessage::decode(messages[0].payload.as_slice()).unwrap();
        let pa::io_settings_message::Settings::Settings(settings) = decoded.settings.unwrap();
        let Some(pa::output_port_settings::Level::Level(value)) = settings.out_port[0].level else {
            panic!("the output-level frame carries no level");
        };
        value
    }

    #[test]
    fn output_level_writes_normalize_decibels_onto_the_zero_through_one_wire_field() {
        // `IOSettings` parameter 33 `OUT 1 LEVEL` is `float 0.0..1.0 linear`
        // (docs/reference/qc-live-parameter-scales.json, model 31000) carrying a
        // -40..+12 dB knob, so 0 dB is 40/52 of the span, i.e. wire value
        // 0.769230....
        for (level_db, wire) in [
            (-40.0, 0.0),
            (12.0, 1.0),
            (0.0, 40.0 / 52.0),
            (-17.5, 22.5 / 52.0),
            (-20.0, 20.0 / 52.0),
        ] {
            let actual = sole_out_port_level(&set_output_port(4, Some(level_db), None, None));
            assert!(
                (f64::from(actual) - wire).abs() <= 1e-6,
                "{level_db} dB must reach the wire as {wire}, not {actual}"
            );
        }
        // The value the action's own description calls 0 dB must never be the
        // bottom of the fader.
        assert!(sole_out_port_level(&set_output_port(1, Some(0.0), None, None)) > 0.5);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn output_level_round_trips_between_decibels_and_the_wire_field(
            level_db in -40.0f32..=12.0,
        ) {
            let wire = sole_out_port_level(&set_output_port(1, Some(level_db), None, None));
            prop_assert!(
                (0.0..=1.0).contains(&wire),
                "{level_db} dB left the normalized domain as {wire}"
            );
            let returned = output_level_wire_to_db(wire);
            prop_assert!(
                (returned - level_db).abs() <= 0.001,
                "{level_db} dB read back as {returned} dB"
            );
        }
    }

    #[test]
    fn io_midi_and_pairing_shapes_match_the_wire_format() {
        let midi = set_midi_thru(true);
        let decoded = pa::IoSettingsMessage::decode(midi.payload.as_slice()).unwrap();
        let pa::io_settings_message::Settings::Settings(settings) = decoded.settings.unwrap();
        let pa::port_settings::MidiPort::MidiPort(midi) = settings.midi_port.unwrap();
        assert!(matches!(
            midi.midi_thru,
            Some(pa::midi_port_settings::MidiThru::MidiThru(1.0))
        ));

        let pairing = set_output_pairing(None, Some(false));
        assert_eq!(pairing.len(), 1);
        let decoded = pa::IoSettingsMessage::decode(pairing[0].payload.as_slice()).unwrap();
        assert!(decoded.xlr1_2_linked.is_none());
        assert!(matches!(
            decoded.out3_4_linked,
            Some(pa::io_settings_message::Out34Linked::Out34Linked(false))
        ));

        let pairing = set_output_pairing(Some(true), Some(false));
        assert_eq!(pairing.len(), 2);
        let output12 = pa::IoSettingsMessage::decode(pairing[0].payload.as_slice()).unwrap();
        assert!(matches!(
            output12.xlr1_2_linked,
            Some(pa::io_settings_message::Xlr12Linked::Xlr12Linked(true))
        ));
        assert!(output12.out3_4_linked.is_none());
        let output34 = pa::IoSettingsMessage::decode(pairing[1].payload.as_slice()).unwrap();
        assert!(output34.xlr1_2_linked.is_none());
        assert!(matches!(
            output34.out3_4_linked,
            Some(pa::io_settings_message::Out34Linked::Out34Linked(false))
        ));
        assert!(set_output_pairing(None, None).is_empty());
    }

    #[test]
    fn io_port_ids_are_rejected_at_the_fallible_command_boundary() {
        let invalid_input = DeviceOperation::SetInputPort {
            input_port_id: 3,
            level: Some(0.5),
            impedance: None,
            input_type: None,
            ground_lift: None,
        };
        assert_eq!(
            invalid_input.try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "input port ID",
                value: "3".into(),
            })
        );
        let invalid_output = DeviceOperation::SetOutputPort {
            output_port_id: 22,
            level: Some(0.5),
            ground_lift: None,
            mute: None,
        };
        assert_eq!(
            invalid_output.try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "output port ID",
                value: "22".into(),
            })
        );
    }

    #[test]
    fn input_port_enum_values_are_rejected_at_the_fallible_command_boundary() {
        for impedance in [0.0, 0.125, 0.5, 0.875, 1.0] {
            assert!(DeviceOperation::SetInputPort {
                input_port_id: 1,
                level: None,
                impedance: Some(impedance),
                input_type: None,
                ground_lift: None,
            }
            .try_encode()
            .is_ok());
        }
        for (field, impedance, input_type, ground_lift) in [
            ("input impedance", Some(0.1), None, None),
            ("input type", None, Some(0.25), None),
            ("input ground lift", None, None, Some(0.5)),
        ] {
            assert!(matches!(
                DeviceOperation::SetInputPort {
                    input_port_id: 2,
                    level: None,
                    impedance,
                    input_type,
                    ground_lift,
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue { field: actual, .. }) if actual == field
            ));
        }
    }

    #[test]
    fn tuner_reference_rejects_values_outside_the_model_domain() {
        for value in [-40.0, 0.0, 40.0] {
            assert!(DeviceOperation::SetTunerReference(value)
                .try_encode()
                .is_ok());
        }
        for value in [-40.01, 40.01, f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
            assert!(matches!(
                DeviceOperation::SetTunerReference(value).try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "tuner reference offset Hz",
                    ..
                })
            ));
        }
    }

    #[test]
    fn usb_enum_values_are_rejected_at_the_fallible_command_boundary() {
        for headphones_source in [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0] {
            assert!(DeviceOperation::SetUsbPort {
                level: Some(0.25),
                headphones_source: Some(headphones_source),
                dry_wet: Some(1.0),
            }
            .try_encode()
            .is_ok());
        }
        for (field, headphones_source, dry_wet) in [
            ("USB headphones source", Some(0.5), None),
            ("USB dry/wet routing", None, Some(0.5)),
        ] {
            assert!(matches!(
                DeviceOperation::SetUsbPort {
                    level: None,
                    headphones_source,
                    dry_wet,
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue { field: actual, .. }) if actual == field
            ));
        }
    }

    #[test]
    fn grid_tail_values_that_need_more_wire_state_are_rejected() {
        assert_eq!(
            DeviceOperation::AddBlock {
                row: 0,
                column: 0,
                model_id: 14_000,
            }
            .try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "ordinary model ID",
                value: "14000".into(),
            })
        );
        assert_eq!(
            DeviceOperation::SetMidiOut {
                source: 10,
                messages: vec![],
            }
            .try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "MIDI Out source",
                value: "10".into(),
            })
        );
    }

    #[test]
    fn add_block_is_one_sparse_update_with_exact_bytes() {
        let messages = DeviceOperation::AddBlock {
            row: 2,
            column: 5,
            model_id: 12_345,
        }
        .try_encode()
        .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(
            messages[0].payload,
            vec![
                0x08, 0x01, 0x1a, 0x0b, 0x5a, 0x09, 0x2a, 0x05, 0x08, 0xb9, 0x60, 0x28, 0x05, 0x70,
                0x02,
            ]
        );
        for model_id in [0, 14_000, 14_001] {
            assert!(DeviceOperation::AddBlock {
                row: 0,
                column: 0,
                model_id,
            }
            .try_encode()
            .is_err());
        }
    }

    #[test]
    fn parameter_assignment_updates_are_sparse_and_preserve_reversed_ranges() {
        let scene_mode = set_parameter_scene_mode(2, 5, 7, true).unwrap();
        let grid = pa::GridMessage::decode(scene_mode.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let parameter = &preset.chains[0].models[0].params[0];
        assert_eq!(preset.chains[0].row, Some(chain::Row::Row(2)));
        assert_eq!(
            preset.chains[0].models[0].column,
            Some(model::Column::Column(5))
        );
        assert_eq!(parameter.index, Some(param::Index::Index(7)));
        assert_eq!(
            parameter.scene_mode,
            Some(param::SceneMode::SceneMode(true))
        );
        assert!(parameter.param_values.is_empty());
        assert!(parameter.expression.is_none());

        let expression = set_parameter_expression(1, 3, 9, 2, 0.85, 0.1).unwrap();
        let grid = pa::GridMessage::decode(expression.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let parameter = &preset.chains[0].models[0].params[0];
        assert_eq!(parameter.index, Some(param::Index::Index(9)));
        assert_eq!(parameter.expression, Some(param::Expression::Expression(2)));
        assert_eq!(
            parameter.expression_min,
            Some(param::ExpressionMin::ExpressionMin(0.85))
        );
        assert_eq!(
            parameter.expression_max,
            Some(param::ExpressionMax::ExpressionMax(0.1))
        );
        assert!(parameter.param_values.is_empty());
        assert!(parameter.scene_mode.is_none());

        let bypass = set_expression_bypass(2, 4, 1, 2, true, 250, true);
        assert_eq!(bypass.len(), 2);
        let grid = pa::GridMessage::decode(bypass[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let model = &preset.chains[0].models[0];
        assert_eq!(preset.chains[0].row, Some(chain::Row::Row(2)));
        assert_eq!(model.column, Some(model::Column::Column(4)));
        assert_eq!(model.bypass_expression[0].expression, 1);
        assert_eq!(model.bypass_expression[0].expression_min, 0.0);
        assert_eq!(model.bypass_expression[0].expression_max, 1.0);
        assert!(model.expression_bypass_info.is_empty());
        assert!(model.params.is_empty());
        let grid = pa::GridMessage::decode(bypass[1].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let model = &preset.chains[0].models[0];
        assert!(model.bypass_expression.is_empty());
        assert_eq!(model.expression_bypass_info[0].r#type, 2);
        assert!(model.expression_bypass_info[0].invert);
        assert_eq!(model.expression_bypass_info[0].delay_ms, 250);
        assert!(model.expression_bypass_info[0].latch_emulation);

        let splitter_scene = set_parameter_scene_mode(0, 8, 4, true).unwrap();
        let grid = pa::GridMessage::decode(splitter_scene.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert!(preset.chains[0].models.is_empty());
        assert!(preset.chains[0].combined_splitter[0].hash.is_none());
        assert_eq!(
            preset.chains[0].combined_splitter[0].params[0].index,
            Some(param::Index::Index(4))
        );
        assert_eq!(
            preset.chains[0].combined_splitter[0].params[0].scene_mode,
            Some(param::SceneMode::SceneMode(true))
        );

        let mixer_scene = set_parameter_scene_mode(2, 9, 3, false).unwrap();
        let grid = pa::GridMessage::decode(mixer_scene.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let mixer = &preset.chains[0].mixer[0];
        assert!(mixer.hash.is_none());
        assert_eq!(
            mixer.params[0].scene_mode,
            Some(param::SceneMode::SceneMode(false))
        );

        let mixer_expression = set_parameter_expression(2, 9, 3, 1, 0.2, 0.9).unwrap();
        let grid = pa::GridMessage::decode(mixer_expression.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let mixer = &preset.chains[0].mixer[0];
        assert!(mixer.hash.is_none());
        assert_eq!(
            mixer.params[0].expression,
            Some(param::Expression::Expression(1))
        );
    }

    #[test]
    fn parameter_scene_mode_routes_have_exact_sparse_bytes_and_strict_bounds() {
        let cases = [
            (
                DeviceOperation::SetParameterSceneMode {
                    row: 2,
                    column: 5,
                    parameter_index: 7,
                    enabled: true,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x0e, 0x5a, 0x0c, 0x2a, 0x08, 0x12, 0x04, 0x20, 0x01, 0x30,
                    0x07, 0x28, 0x05, 0x70, 0x02,
                ],
            ),
            (
                DeviceOperation::SetParameterSceneMode {
                    row: 0,
                    column: 8,
                    parameter_index: 4,
                    enabled: true,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x0c, 0x5a, 0x0a, 0x62, 0x06, 0x12, 0x04, 0x20, 0x01, 0x30,
                    0x04, 0x70, 0x00,
                ],
            ),
            (
                DeviceOperation::SetParameterSceneMode {
                    row: 2,
                    column: 9,
                    parameter_index: 3,
                    enabled: false,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x0c, 0x5a, 0x0a, 0x42, 0x06, 0x12, 0x04, 0x20, 0x00, 0x30,
                    0x03, 0x70, 0x02,
                ],
            ),
        ];
        for (operation, expected) in cases {
            let messages = operation.try_encode().unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].message_type, profile::MESSAGE_TYPE_GRID);
            assert_eq!(messages[0].payload, expected);
        }

        for (row, column) in [(4, 0), (0, 10), (1, 8), (3, 9)] {
            assert!(DeviceOperation::SetParameterSceneMode {
                row,
                column,
                parameter_index: 0,
                enabled: true,
            }
            .try_encode()
            .is_err());
        }
    }

    #[test]
    fn parameter_expression_routes_are_byte_exact_and_fallibly_bounded() {
        let cases = [
            (
                DeviceOperation::SetParameterExpression {
                    row: 1,
                    column: 3,
                    parameter_index: 9,
                    pedal: 2,
                    minimum: 0.85,
                    maximum: 0.1,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x18, 0x5a, 0x16, 0x2a, 0x12, 0x12, 0x0e, 0x08, 0x02, 0x15,
                    0x9a, 0x99, 0x59, 0x3f, 0x1d, 0xcd, 0xcc, 0xcc, 0x3d, 0x30, 0x09, 0x28, 0x03,
                    0x70, 0x01,
                ],
            ),
            (
                DeviceOperation::SetParameterExpression {
                    row: 0,
                    column: 8,
                    parameter_index: 4,
                    pedal: 1,
                    minimum: 0.0,
                    maximum: 1.0,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x16, 0x5a, 0x14, 0x62, 0x10, 0x12, 0x0e, 0x08, 0x01, 0x15,
                    0x00, 0x00, 0x00, 0x00, 0x1d, 0x00, 0x00, 0x80, 0x3f, 0x30, 0x04, 0x70, 0x00,
                ],
            ),
            (
                DeviceOperation::SetParameterExpression {
                    row: 2,
                    column: 9,
                    parameter_index: 3,
                    pedal: 0,
                    minimum: 1.0,
                    maximum: 0.0,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x16, 0x5a, 0x14, 0x42, 0x10, 0x12, 0x0e, 0x08, 0x00, 0x15,
                    0x00, 0x00, 0x80, 0x3f, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x30, 0x03, 0x70, 0x02,
                ],
            ),
        ];
        for (operation, expected) in cases {
            let messages = operation.try_encode().unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].message_type, profile::MESSAGE_TYPE_GRID);
            assert_eq!(messages[0].payload, expected);
        }

        for (pedal, minimum, maximum) in [
            (3, 0.0, 1.0),
            (1, -0.1, 1.0),
            (1, 0.0, 1.1),
            (1, f32::NAN, 1.0),
            (1, 0.0, f32::INFINITY),
        ] {
            assert!(DeviceOperation::SetParameterExpression {
                row: 0,
                column: 0,
                parameter_index: 0,
                pedal,
                minimum,
                maximum,
            }
            .try_encode()
            .is_err());
        }
    }

    #[test]
    fn splitter_and_mixer_parameters_use_their_native_chain_containers() {
        let splitter = set_routing_parameter(0, "splitter", 5, 0.25).unwrap();
        assert_eq!(
            splitter.payload,
            [
                0x08, 0x01, 0x1a, 0x11, 0x5a, 0x0f, 0x62, 0x0b, 0x12, 0x09, 0x2a, 0x05, 0x15, 0x00,
                0x00, 0x80, 0x3e, 0x30, 0x05, 0x70, 0x00,
            ]
        );
        let grid = pa::GridMessage::decode(splitter.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let chain = &preset.chains[0];
        assert_eq!(chain.row, Some(chain::Row::Row(0)));
        assert!(chain.models.is_empty());
        assert!(chain.combined_splitter[0].hash.is_none());
        assert_eq!(
            chain.combined_splitter[0].params[0].index,
            Some(param::Index::Index(5))
        );

        let legacy = set_routing_parameter(0, "legacySplitter", 5, 0.25).unwrap();
        assert_eq!(
            legacy.payload,
            [
                0x08, 0x01, 0x1a, 0x11, 0x5a, 0x0f, 0x3a, 0x0b, 0x12, 0x09, 0x2a, 0x05, 0x15, 0x00,
                0x00, 0x80, 0x3e, 0x30, 0x05, 0x70, 0x00,
            ]
        );
        let grid = pa::GridMessage::decode(legacy.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(preset.chains[0].splitter.len(), 1);
        assert!(preset.chains[0].combined_splitter.is_empty());

        let mixer = set_routing_parameter(2, "mixer", 3, 0.75).unwrap();
        assert_eq!(
            mixer.payload,
            [
                0x08, 0x01, 0x1a, 0x11, 0x5a, 0x0f, 0x42, 0x0b, 0x12, 0x09, 0x2a, 0x05, 0x15, 0x00,
                0x00, 0x40, 0x3f, 0x30, 0x03, 0x70, 0x02,
            ]
        );
        let grid = pa::GridMessage::decode(mixer.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let model = &preset.chains[0].mixer[0];
        assert!(model.hash.is_none());
        assert_eq!(model.params[0].index, Some(param::Index::Index(3)));
    }

    #[test]
    fn lane_control_updates_use_their_native_chain_containers() {
        let input = set_lane_control_parameter(1, "inputGate", 2, 0.4).unwrap();
        assert_eq!(
            input.payload,
            [
                0x08, 0x01, 0x1a, 0x11, 0x5a, 0x0f, 0x6a, 0x0b, 0x12, 0x09, 0x2a, 0x05, 0x15, 0xcd,
                0xcc, 0xcc, 0x3e, 0x30, 0x02, 0x70, 0x01,
            ]
        );
        let grid = pa::GridMessage::decode(input.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let chain = &preset.chains[0];
        assert!(chain.models.is_empty());
        assert!(chain.output_control.is_empty());
        assert!(chain.input_control[0].hash.is_none());
        assert_eq!(
            chain.input_control[0].params[0].index,
            Some(param::Index::Index(2))
        );

        let output = set_lane_control_scene_mode(3, "laneOutput", 0, true).unwrap();
        let grid = pa::GridMessage::decode(output.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let control = &preset.chains[0].output_control[0];
        assert!(control.hash.is_none());
        assert_eq!(
            control.params[0].scene_mode,
            Some(param::SceneMode::SceneMode(true))
        );
        assert!(
            control.params[0].param_values.is_empty(),
            "scene-mode metadata must travel alone"
        );

        let output_parameter = set_lane_control_parameter(3, "laneOutput", 4, 0.75).unwrap();
        assert_eq!(
            output_parameter.payload,
            [
                0x08, 0x01, 0x1a, 0x11, 0x5a, 0x0f, 0x4a, 0x0b, 0x12, 0x09, 0x2a, 0x05, 0x15, 0x00,
                0x00, 0x40, 0x3f, 0x30, 0x04, 0x70, 0x03,
            ]
        );
    }

    #[test]
    fn lane_control_scene_mode_has_exact_sparse_bytes_and_bounds() {
        let cases = [
            (
                DeviceOperation::SetLaneControlSceneMode {
                    row: 1,
                    control: "inputGate".into(),
                    parameter_index: 2,
                    enabled: true,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x0c, 0x5a, 0x0a, 0x6a, 0x06, 0x12, 0x04, 0x20, 0x01, 0x30,
                    0x02, 0x70, 0x01,
                ],
            ),
            (
                DeviceOperation::SetLaneControlSceneMode {
                    row: 3,
                    control: "laneOutput".into(),
                    parameter_index: 0,
                    enabled: false,
                },
                vec![
                    0x08, 0x01, 0x1a, 0x0c, 0x5a, 0x0a, 0x4a, 0x06, 0x12, 0x04, 0x20, 0x00, 0x30,
                    0x00, 0x70, 0x03,
                ],
            ),
        ];
        for (operation, expected) in cases {
            let messages = operation.try_encode().unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].message_type, profile::MESSAGE_TYPE_GRID);
            assert_eq!(messages[0].payload, expected);
        }

        for operation in [
            DeviceOperation::SetLaneControlSceneMode {
                row: 4,
                control: "inputGate".into(),
                parameter_index: 0,
                enabled: true,
            },
            DeviceOperation::SetLaneControlSceneMode {
                row: 0,
                control: "output".into(),
                parameter_index: 0,
                enabled: true,
            },
        ] {
            assert!(operation.try_encode().is_err());
        }
    }

    #[test]
    fn lane_control_expression_uses_native_hashless_containers() {
        let input = set_lane_control_expression(1, "inputGate", 2, 0, 0.25, 0.75).unwrap();
        let output = set_lane_control_expression(3, "laneOutput", 4, 2, 1.0, 0.0).unwrap();
        assert_eq!(
            input.payload,
            [
                0x08, 0x01, 0x1a, 0x16, 0x5a, 0x14, 0x6a, 0x10, 0x12, 0x0e, 0x08, 0x00, 0x15, 0x00,
                0x00, 0x80, 0x3e, 0x1d, 0x00, 0x00, 0x40, 0x3f, 0x30, 0x02, 0x70, 0x01,
            ]
        );
        assert_eq!(
            output.payload,
            [
                0x08, 0x01, 0x1a, 0x16, 0x5a, 0x14, 0x4a, 0x10, 0x12, 0x0e, 0x08, 0x02, 0x15, 0x00,
                0x00, 0x80, 0x3f, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x30, 0x04, 0x70, 0x03,
            ]
        );
    }

    #[test]
    fn chain_output_is_sparse_slot_exact_and_qc_bounded() {
        let output = DeviceOperation::SetChainOutput {
            row: 2,
            output_id: 20,
        }
        .try_encode()
        .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(
            output[0].payload,
            vec![0x08, 0x01, 0x1a, 0x06, 0x5a, 0x04, 0x10, 0x14, 0x70, 0x02]
        );
        // Every route the device reports round-trips, including Multi Out (19).
        for (output_id, _) in domain::OUTPUT_ROUTES {
            assert!(DeviceOperation::SetChainOutput {
                row: 0,
                output_id: *output_id
            }
            .try_encode()
            .is_ok());
        }
        for output_id in [23, 24, 100, u32::MAX] {
            assert!(DeviceOperation::SetChainOutput { row: 0, output_id }
                .try_encode()
                .is_err());
        }
    }

    #[test]
    fn stomp_metadata_updates_are_sparse_and_keyed_by_footswitch() {
        let momentary = set_stomp_momentary(4, true);
        assert_eq!(
            momentary.payload,
            vec![0x08, 0x01, 0x1a, 0x07, 0x9a, 0x02, 0x04, 0x08, 0x04, 0x10, 0x01]
        );
        let grid = pa::GridMessage::decode(momentary.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert!(matches!(preset.stomp_is_momentary.get(&4), Some(true)));
        assert!(preset.stomp_mode_assignments.is_empty());
        assert!(preset.stomp_labels.is_empty());
        assert!(preset.single_stomp_labels.is_empty());

        let label = set_stomp_label(7, "Solo".into(), true);
        assert_eq!(
            label.payload,
            vec![
                0x08, 0x01, 0x1a, 0x0b, 0x92, 0x02, 0x08, 0x08, 0x07, 0x12, 0x04, b'S', b'o', b'l',
                b'o'
            ]
        );
        let grid = pa::GridMessage::decode(label.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(
            preset.single_stomp_labels.get(&7).map(String::as_str),
            Some("Solo")
        );
        assert!(preset.stomp_labels.is_empty());
        assert!(preset.stomp_is_momentary.is_empty());

        let aggregate = DeviceOperation::SetStompLabel {
            footswitch: 4,
            label: "Lead".into(),
            single: false,
        }
        .try_encode()
        .unwrap()
        .remove(0);
        assert_eq!(
            aggregate.payload,
            vec![
                0x08, 0x01, 0x1a, 0x0b, 0x82, 0x02, 0x08, 0x08, 0x04, 0x12, 0x04, b'L', b'e', b'a',
                b'd'
            ]
        );
        for invalid in [
            DeviceOperation::SetStompLabel {
                footswitch: 8,
                label: "Lead".into(),
                single: false,
            },
            DeviceOperation::SetStompLabel {
                footswitch: 4,
                label: String::new(),
                single: false,
            },
            DeviceOperation::SetStompLabel {
                footswitch: 4,
                label: "12345678901234567".into(),
                single: false,
            },
        ] {
            assert!(invalid.try_encode().is_err());
        }
        assert!(DeviceOperation::SetStompLabel {
            footswitch: 4,
            label: "🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸🎸".into(),
            single: false,
        }
        .try_encode()
        .is_ok());
    }

    #[test]
    fn preset_midi_out_uses_midi_settings_and_preserves_source_slots() {
        let message = MidiOutMessage {
            r#type: 2,
            channel: 4,
            param1: 30,
            param2: 5,
            param3: 120,
        };
        let outbound =
            set_midi_out(7, vec![message.clone()], false).expect("defined message encodes");
        assert_eq!(outbound.message_type, 8);
        assert_eq!(
            outbound.payload,
            vec![
                0x08, 0x01, 0x22, 0x26, 0x0a, 0x24, 0x08, 0x07, 0x12, 0x0a, 0x08, 0x02, 0x10, 0x04,
                0x18, 0x1e, 0x20, 0x05, 0x28, 0x78, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00,
                0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00,
            ]
        );
        let decoded = pa::MidiSettingsMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert!(decoded.preset_load_messages.is_none());
        let Some(pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(groups)) =
            decoded.general_midi_messages
        else {
            panic!("general MIDI messages missing");
        };
        assert_eq!(groups.messages.len(), 1);
        assert_eq!(
            groups.messages[0].source,
            Some(pa::general_midi_message::Source::Source(7))
        );
        assert_eq!(groups.messages[0].msg[0].r#type, message.r#type);
        assert_eq!(groups.messages[0].msg[0].param3, message.param3);

        let filtered = set_midi_out_selector(
            8,
            vec![
                MidiOutMessage {
                    r#type: 0,
                    ..message.clone()
                },
                message.clone(),
            ],
        )
        .expect("an empty slot and a defined message encode");
        let decoded = pa::MidiSettingsMessage::decode(filtered.payload.as_slice()).unwrap();
        let Some(pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(groups)) =
            decoded.general_midi_messages
        else {
            panic!("general MIDI messages missing");
        };
        assert_eq!(
            groups.messages[0].source,
            Some(pa::general_midi_message::Source::Source(7))
        );
        assert_eq!(groups.messages[0].msg.len(), MIDI_OUT_SLOTS_PER_SOURCE);
        assert_eq!(groups.messages[0].msg[0], Default::default());
        assert_eq!(groups.messages[0].msg[1].r#type, message.r#type);

        let outbound =
            set_midi_out(0, vec![message.clone()], true).expect("defined message encodes");
        let decoded = pa::MidiSettingsMessage::decode(outbound.payload.as_slice()).unwrap();
        assert!(decoded.general_midi_messages.is_none());
        assert!(matches!(
            decoded.preset_load_messages,
            Some(pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(_))
        ));
        let Some(pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(groups)) =
            decoded.preset_load_messages
        else {
            unreachable!()
        };
        assert!(groups.messages[0].source.is_none());

        let exact = set_midi_out(
            0,
            vec![MidiOutMessage {
                r#type: 1,
                channel: 1,
                param1: 0,
                param2: 0,
                param3: 0,
            }],
            true,
        )
        .expect("defined message encodes");
        assert_eq!(
            exact.payload,
            vec![
                0x08, 0x01, 0x1a, 0x1e, 0x0a, 0x1c, 0x12, 0x04, 0x08, 0x01, 0x10, 0x01, 0x12, 0x00,
                0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00, 0x12, 0x00,
                0x12, 0x00, 0x12, 0x00, 0x12, 0x00,
            ]
        );

        assert!(DeviceOperation::SetMidiOut {
            source: 0,
            messages: vec![MidiOutMessage {
                r#type: 4,
                channel: 1,
                param1: 0,
                param2: 0,
                param3: 0,
            }],
        }
        .try_encode()
        .is_err());
        assert!(DeviceOperation::SetMidiOut {
            source: 0,
            messages: vec![message; MIDI_OUT_SLOTS_PER_SOURCE + 1],
        }
        .try_encode()
        .is_err());
    }

    fn midi_row(kind: u32, param1: u32) -> MidiOutMessage {
        MidiOutMessage {
            r#type: kind,
            channel: if kind == 0 { 1 } else { 5 },
            param1,
            param2: 7,
            param3: 9,
        }
    }

    /// The slots of a decoded MIDI Out frame, in the order the frame carries them.
    fn midi_frame_slots(
        outbound: &OutboundMessage,
        preset_load: bool,
    ) -> Vec<crate::proto::MidiMessageInfo> {
        let decoded = pa::MidiSettingsMessage::decode(outbound.payload.as_slice()).unwrap();
        let groups = if preset_load {
            let Some(pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(groups)) =
                decoded.preset_load_messages
            else {
                panic!("preset-load messages missing");
            };
            groups
        } else {
            let Some(pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(groups)) =
                decoded.general_midi_messages
            else {
                panic!("general MIDI messages missing");
            };
            groups
        };
        assert_eq!(groups.messages.len(), 1);
        groups.messages[0].msg.clone()
    }

    #[test]
    fn midi_out_keeps_an_empty_row_between_active_rows_in_place() {
        // Observed on a CorOS 4.1.0 unit, 2026-10-07: the unit preserves the
        // positions of the messages in the order sent, so a type 0 row between
        // two active rows keeps its index.
        let rows = vec![
            midi_row(1, 10),
            midi_row(0, 0),
            midi_row(3, 30),
            midi_row(0, 0),
            midi_row(2, 50),
        ];
        for (selector, preset_load) in [(0, true), (4, false)] {
            let outbound = set_midi_out_selector(selector, rows.clone())
                .expect("defined rows and empty rows encode");
            let slots = midi_frame_slots(&outbound, preset_load);
            assert_eq!(slots.len(), MIDI_OUT_SLOTS_PER_SOURCE);
            assert_eq!((slots[0].r#type, slots[0].param1), (1, 10));
            assert_eq!(slots[1], Default::default(), "the empty row keeps index 1");
            assert_eq!(
                (slots[2].r#type, slots[2].param1),
                (3, 30),
                "the row after it keeps index 2"
            );
            assert_eq!(
                slots[3],
                Default::default(),
                "a second empty row keeps index 3"
            );
            assert_eq!((slots[4].r#type, slots[4].param1), (2, 50));
            assert!(slots[5..].iter().all(|slot| *slot == Default::default()));
        }
        // The planner takes the same list: the operation neither compacts nor
        // reorders it before the frame is built.
        let encoded = DeviceOperation::SetMidiOut {
            source: 3,
            messages: rows.clone(),
        }
        .try_encode()
        .unwrap();
        assert_eq!(encoded.len(), 1);
        let slots = midi_frame_slots(&encoded[0], false);
        assert_eq!((slots[2].r#type, slots[2].param1), (3, 30));
        let encoded = DeviceOperation::SetPresetLoadMidiOut {
            messages: rows.clone(),
        }
        .try_encode()
        .unwrap();
        let slots = midi_frame_slots(&encoded[0], true);
        assert_eq!((slots[4].r#type, slots[4].param1), (2, 50));

        // A list padded with empty rows to the twelve-slot model is the same
        // frame as the unpadded one.
        let mut padded = rows.clone();
        padded.resize(MIDI_OUT_SLOTS_PER_SOURCE, midi_row(0, 0));
        assert_eq!(
            DeviceOperation::SetMidiOut {
                source: 3,
                messages: padded
            }
            .try_encode()
            .unwrap(),
            DeviceOperation::SetMidiOut {
                source: 3,
                messages: rows.clone()
            }
            .try_encode()
            .unwrap(),
        );
        // A leading empty row is also kept, and an all-empty list clears the source.
        let slots = midi_frame_slots(
            &set_midi_out_selector(1, vec![midi_row(0, 0), midi_row(1, 4)])
                .expect("a leading empty row encodes"),
            false,
        );
        assert_eq!(slots[0], Default::default());
        assert_eq!((slots[1].r#type, slots[1].param1), (1, 4));
        let cleared = DeviceOperation::SetMidiOut {
            source: 3,
            messages: vec![midi_row(0, 0); MIDI_OUT_SLOTS_PER_SOURCE],
        }
        .try_encode()
        .unwrap();
        assert!(midi_frame_slots(&cleared[0], false)
            .iter()
            .all(|slot| *slot == Default::default()));
    }

    #[test]
    fn midi_out_still_refuses_undefined_types_and_out_of_range_values() {
        let refused = |messages: Vec<MidiOutMessage>| {
            DeviceOperation::SetMidiOut {
                source: 0,
                messages: messages.clone(),
            }
            .try_encode()
            .is_err()
                && DeviceOperation::SetPresetLoadMidiOut { messages }
                    .try_encode()
                    .is_err()
        };
        assert!(
            refused(vec![midi_row(1, 1), midi_row(0, 0), midi_row(4, 1)]),
            "type 4 after an empty row"
        );
        assert!(refused(vec![
            midi_row(0, 0),
            MidiOutMessage {
                r#type: u32::MAX,
                ..midi_row(1, 1)
            }
        ]));
        assert!(
            refused(vec![MidiOutMessage {
                channel: 0,
                ..midi_row(1, 1)
            }]),
            "an active row needs a channel"
        );
        assert!(refused(vec![MidiOutMessage {
            channel: 17,
            ..midi_row(2, 1)
        }]));
        assert!(refused(vec![MidiOutMessage {
            param3: 128,
            ..midi_row(3, 1)
        }]));
        // An empty row is bounded too, though its values are not sent.
        assert!(refused(vec![MidiOutMessage {
            channel: 17,
            ..midi_row(0, 0)
        }]));
        assert!(refused(vec![MidiOutMessage {
            param2: 128,
            ..midi_row(0, 0)
        }]));
        assert!(refused(vec![midi_row(0, 0); MIDI_OUT_SLOTS_PER_SOURCE + 1]));
        // A published empty row is accepted back as published.
        assert!(!refused(vec![
            MidiOutMessage {
                channel: 0,
                ..midi_row(0, 0)
            },
            midi_row(1, 1)
        ]));
    }

    fn midi_slot(r#type: u32, channel: u32, params: [u32; 3]) -> MidiOutMessage {
        MidiOutMessage {
            r#type,
            channel,
            param1: params[0],
            param2: params[1],
            param3: params[2],
        }
    }

    #[test]
    fn midi_out_selector_encoder_refuses_undefined_values_before_encoding() {
        let refused = |selector: u32, messages: Vec<MidiOutMessage>| {
            assert!(
                matches!(
                    set_midi_out_selector(selector, messages.clone()),
                    Err(CommandEncodeError::UnsupportedValue { .. })
                ),
                "selector {selector} with {messages:?} must be refused"
            );
        };
        // The undefined type that preceded a USB drop on hardware, on a
        // general source and on the preset-load list.
        refused(1, vec![midi_slot(4, 1, [0, 0, 0])]);
        refused(0, vec![midi_slot(4, 1, [0, 0, 0])]);
        refused(1, vec![midi_slot(u32::MAX, 1, [0, 0, 0])]);
        refused(1, vec![midi_slot(1, 0, [0, 0, 0])]);
        refused(1, vec![midi_slot(1, 17, [0, 0, 0])]);
        refused(1, vec![midi_slot(1, 1, [0, 128, 0])]);
        refused(11, vec![midi_slot(1, 1, [0, 0, 0])]);
        // An empty row is bounded too, though its values are not sent.
        refused(1, vec![midi_slot(MIDI_OUT_TYPE_NONE, 17, [0, 0, 0])]);
        refused(0, vec![midi_slot(MIDI_OUT_TYPE_NONE, 0, [0, 128, 0])]);
        refused(
            1,
            vec![midi_slot(1, 1, [0, 0, 0]); MIDI_OUT_SLOTS_PER_SOURCE + 1],
        );
        // A defined slot before an undefined one does not hide it.
        refused(
            1,
            vec![midi_slot(1, 1, [0, 0, 0]), midi_slot(4, 1, [0, 0, 0])],
        );
        assert!(set_midi_out(10, vec![midi_slot(1, 1, [0, 0, 0])], false).is_err());
        assert!(set_midi_out(u32::MAX, Vec::new(), false).is_err());
        assert!(set_midi_out(0, vec![midi_slot(4, 1, [0, 0, 0])], true).is_err());

        // Every defined type, both edge channels and an empty slot still encode.
        for r#type in MIDI_OUT_TYPES {
            for channel in [*MIDI_OUT_CHANNELS.start(), *MIDI_OUT_CHANNELS.end()] {
                for selector in [0, 1, 10] {
                    assert!(set_midi_out_selector(
                        selector,
                        vec![midi_slot(r#type, channel, [MIDI_OUT_PARAMETER_MAX; 3])]
                    )
                    .is_ok());
                }
            }
        }
        assert!(set_midi_out_selector(1, vec![midi_slot(MIDI_OUT_TYPE_NONE, 0, [0; 3])]).is_ok());
        assert!(set_midi_out(9, Vec::new(), false).is_ok());
    }

    #[test]
    fn midi_out_selector_keeps_an_unpadded_list_with_a_middle_gap_in_place() {
        // The unit preserves row positions and accepts unpadded lists, so
        // validation must neither compact a type-0 gap nor require padding.
        let first = midi_slot(MIDI_OUT_TYPE_CONTROL_CHANGE, 1, [7, 100, 0]);
        let last = midi_slot(MIDI_OUT_TYPE_PROGRAM_CHANGE, 16, [0, 0, 42]);
        for selector in [0, 1, 10] {
            let outbound = set_midi_out_selector(
                selector,
                vec![
                    first.clone(),
                    midi_slot(MIDI_OUT_TYPE_NONE, 0, [0; 3]),
                    last.clone(),
                ],
            )
            .expect("a type-0 gap between two defined rows encodes");
            let decoded = pa::MidiSettingsMessage::decode(outbound.payload.as_slice()).unwrap();
            let list = match (decoded.preset_load_messages, decoded.general_midi_messages) {
                (
                    Some(pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(list)),
                    None,
                ) if selector == 0 => list,
                (
                    None,
                    Some(pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(list)),
                ) if selector != 0 => list,
                other => panic!("unexpected lists for selector {selector}: {other:?}"),
            };
            let slots = &list.messages[0].msg;
            assert_eq!(slots[0].r#type, first.r#type);
            assert_eq!(slots[0].param1, first.param1);
            assert_eq!(slots[1], Default::default());
            assert_eq!(slots[2].r#type, last.r#type);
            assert_eq!(slots[2].channel, last.channel);
            assert_eq!(slots[2].param3, last.param3);
        }
    }

    #[test]
    fn midi_out_bounds_constants_name_the_wire_values() {
        assert_eq!(MIDI_OUT_TYPE_NONE, 0);
        assert_eq!(MIDI_OUT_TYPE_CONTROL_CHANGE, 1);
        assert_eq!(MIDI_OUT_TYPE_CONTROL_CHANGE_TOGGLE, 2);
        assert_eq!(MIDI_OUT_TYPE_PROGRAM_CHANGE, 3);
        assert_eq!(MIDI_OUT_TYPES, 1..=3);
        assert!(!MIDI_OUT_TYPES.contains(&4));
        assert_eq!(MIDI_OUT_CHANNELS, 1..=16);
        assert_eq!(MIDI_OUT_PARAMETER_MAX, 127);
        assert_eq!(MIDI_OUT_SOURCES, 0..=9);
        assert_eq!(GENERAL_MIDI_CHANNELS, 1..=17);
        assert_eq!(GENERAL_MIDI_CLOCK_OUT, 0..=3);
        assert_eq!(PRESET_INSTRUMENTS, 0..=5);
        for value in [0, 18] {
            assert!(!is_general_integer_value("midiChannel", value));
        }
        for value in [-1, 4] {
            assert!(!is_general_integer_value("midiClockOut", value));
        }
        assert!(is_general_integer_value("midiChannel", 17));
        assert!(is_general_integer_value("midiClockOut", 3));
    }

    #[test]
    fn special_blocks_and_stomp_types_are_explicit() {
        let block = set_block_with_initial_value(
            1,
            3,
            14_000,
            Some(ParamValue {
                value: Some(param_value::Value::StringValue("capture-key".into())),
            }),
        );
        let grid = pa::GridMessage::decode(block.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        let model = &preset.chains[0].models[0];
        assert_eq!(model.params[0].index, Some(param::Index::Index(5)));
        assert!(matches!(
            model.params[0].param_values[0].value,
            Some(param_value::Value::StringValue(ref value)) if value == "capture-key"
        ));

        let messages = set_footswitch_typed(2, 4, Some(6), StompAssignmentType::Secondary);
        assert_eq!(messages.len(), 1);
        let grid = pa::GridMessage::decode(messages[0].payload.as_slice()).unwrap();
        assert_eq!(grid.action, pa::message_action::Enum::Update as i32);
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(
            preset.stomp_mode_assignments[0].r#type,
            Some(crate::proto::stomp_mode_assignment::TypeOneOf::Type(1))
        );
    }

    #[test]
    fn expression_targets_use_exact_containers_and_bypass_order() {
        let splitter = set_parameter_expression(0, 8, 2, 1, 0.0, 1.0).unwrap();
        let grid = pa::GridMessage::decode(splitter.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(preset.chains[0].combined_splitter.len(), 1);
        assert!(preset.chains[0].models.is_empty());

        let messages = set_expression_bypass_target(
            2,
            ExpressionBypassTarget::OutputControl,
            2,
            3,
            true,
            100,
            false,
        );
        assert_eq!(messages.len(), 2);
        let first = pa::GridMessage::decode(messages[0].payload.as_slice()).unwrap();
        let second = pa::GridMessage::decode(messages[1].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(first) = first.preset.unwrap();
        let pa::grid_message::Preset::Preset(second) = second.preset.unwrap();
        assert_eq!(first.chains[0].output_control[0].bypass_expression.len(), 1);
        assert!(first.chains[0].output_control[0]
            .expression_bypass_info
            .is_empty());
        assert_eq!(
            second.chains[0].output_control[0]
                .expression_bypass_info
                .len(),
            1
        );
        assert!(second.chains[0].output_control[0]
            .bypass_expression
            .is_empty());

        let messages = set_expression_bypass_target(
            1,
            ExpressionBypassTarget::InputControl,
            1,
            0,
            false,
            0,
            false,
        );
        let first = pa::GridMessage::decode(messages[0].payload.as_slice()).unwrap();
        let second = pa::GridMessage::decode(messages[1].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(first) = first.preset.unwrap();
        let pa::grid_message::Preset::Preset(second) = second.preset.unwrap();
        assert_eq!(
            first.chains[0].input_control[0].column,
            Some(model::Column::Column(1))
        );
        assert_eq!(second.chains[0].input_control[0].column, None);

        for model_hash in [10_004, 11_000] {
            let messages = set_expression_bypass_for_model(0, model_hash, 7, 1, 0, false, 0, false);
            for message in messages {
                let grid = pa::GridMessage::decode(message.payload.as_slice()).unwrap();
                let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
                assert_eq!(preset.chains[0].mixer.len(), 1);
                assert!(preset.chains[0].combined_splitter.is_empty());
            }
        }

        let ordinary = set_expression_bypass(2, 4, 1, 2, true, 250, true);
        assert_eq!(
            ordinary[0].payload,
            [
                0x08, 0x01, 0x1a, 0x11, 0x5a, 0x0f, 0x2a, 0x0b, 0x1a, 0x07, 0x08, 0x01, 0x1d, 0x00,
                0x00, 0x80, 0x3f, 0x28, 0x04, 0x70, 0x02,
            ]
        );
        assert_eq!(
            ordinary[1].payload,
            [
                0x08, 0x01, 0x1a, 0x13, 0x5a, 0x11, 0x2a, 0x0d, 0x22, 0x09, 0x08, 0x02, 0x10, 0x01,
                0x18, 0xfa, 0x01, 0x20, 0x01, 0x28, 0x04, 0x70, 0x02,
            ]
        );

        // Same-value requests are not deduplicated; every call encodes a full frame.
        assert_eq!(ordinary, set_expression_bypass(2, 4, 1, 2, true, 250, true));
    }

    #[test]
    fn checked_expression_bypass_rejects_values_outside_the_public_safe_subset() {
        let operation = |pedal, mode, delay_ms| DeviceOperation::SetExpressionBypass {
            row: 0,
            column: 0,
            pedal,
            mode,
            invert: false,
            delay_ms,
            latch_emulation: false,
        };
        assert!(operation(1, 0, 0).try_encode().is_ok());
        assert!(operation(2, 2, 5_000).try_encode().is_ok());
        // Pedal zero is the clear, not an out-of-range jack number.
        assert!(operation(0, 0, 0).try_encode().is_ok());
        for invalid in [
            operation(3, 0, 0),
            operation(1, 3, 0),
            operation(1, 0, 5_001),
        ] {
            assert!(matches!(
                invalid.try_encode(),
                Err(CommandEncodeError::UnsupportedValue { .. })
            ));
        }
    }

    #[test]
    fn scene_management_matches_the_wire_message_shapes() {
        let copy = copy_scene(1, 3, true);
        assert_eq!(copy.message_type, 22);
        assert_eq!(
            copy.payload,
            [0x08, 0x01, 0x18, 0x01, 0x20, 0x03, 0x28, 0x01]
        );
        let decoded = pa::SceneCopyMessage::decode(copy.payload.as_slice()).unwrap();
        assert_eq!(
            (decoded.from_index, decoded.to_index, decoded.is_swap),
            (1, 3, true)
        );
        assert_eq!(
            copy_scene(1, 3, false).payload,
            [0x08, 0x01, 0x18, 0x01, 0x20, 0x03]
        );
        assert_eq!(
            copy_scene(2, 2, false).payload,
            [0x08, 0x01, 0x18, 0x02, 0x20, 0x02]
        );
        for operation in [
            DeviceOperation::CopyScene {
                from_index: domain::SCENE_COUNT,
                to_index: 0,
                swap: false,
            },
            DeviceOperation::CopyScene {
                from_index: 0,
                to_index: domain::SCENE_COUNT,
                swap: false,
            },
        ] {
            assert!(matches!(
                operation.try_encode(),
                Err(CommandEncodeError::UnsupportedValue { .. })
            ));
        }

        let label = set_scene_label(2, Some("Lead".into()));
        assert_eq!(label.message_type, 23);
        assert_eq!(
            label.payload,
            [0x08, 0x01, 0x18, 0x02, 0x22, 0x04, b'L', b'e', b'a', b'd']
        );
        assert_eq!(
            pa::SceneLabelMessage::decode(label.payload.as_slice())
                .unwrap()
                .label,
            "Lead"
        );
        let clear = set_scene_label(2, None);
        assert_eq!(clear.payload, [0x08, 0x01, 0x18, 0x02]);
        assert_eq!(
            pa::SceneLabelMessage::decode(clear.payload.as_slice())
                .unwrap()
                .label,
            ""
        );
        assert!(DeviceOperation::SetSceneLabel {
            scene: 8,
            label: None
        }
        .try_encode()
        .is_err());
        assert!(DeviceOperation::SetSceneLabel {
            scene: 0,
            label: Some("x".repeat(profile::MAX_FRAME_BYTES)),
        }
        .try_encode()
        .is_err());

        let color = set_scene_color(4, 0xffff02c2);
        assert_eq!(color.message_type, 48);
        assert_eq!(
            color.payload,
            vec![0x08, 0x01, 0x18, 0x04, 0x20, 0xc2, 0x85, 0xfc, 0xff, 0x0f]
        );
        assert_eq!(
            pa::SceneColorMessage::decode(color.payload.as_slice())
                .unwrap()
                .color,
            0xffff02c2
        );

        assert!(DeviceOperation::SetSceneColor {
            scene: domain::SCENE_COUNT,
            color: 0xffff02c2,
        }
        .try_encode()
        .is_err());
    }

    #[test]
    fn preset_tempo_is_sparse_byte_exact_and_fallibly_bounded() {
        let message = DeviceCommand::SetTempo(120).try_encode().unwrap();
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(
            message.payload,
            vec![
                0x08, 0x01, 0x1a, 0x0e, 0x9a, 0x01, 0x0b, 0x12, 0x09, 0x2a, 0x05, 0x15, 0xcd, 0xcc,
                0xcc, 0x3e, 0x30, 0x00,
            ]
        );
        let grid = pa::GridMessage::decode(message.payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert!(matches!(
            preset.tempo_program_data[0].params[0].param_values[0].value,
            Some(param_value::Value::FloatValue(value)) if (value - 0.4).abs() < f32::EPSILON
        ));
        assert!(preset.tempo_program_data[0].hash.is_none());
        assert_eq!(
            preset.tempo_program_data[0].params[0].index,
            Some(param::Index::Index(0))
        );

        for (bpm, normalized) in [(40, 0.0), (240, 1.0)] {
            let message = DeviceCommand::SetTempo(bpm).try_encode().unwrap();
            let grid = pa::GridMessage::decode(message.payload.as_slice()).unwrap();
            let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
            assert!(matches!(
                preset.tempo_program_data[0].params[0].param_values[0].value,
                Some(param_value::Value::FloatValue(value)) if value == normalized
            ));
        }
        for bpm in [39, 241] {
            assert!(matches!(
                DeviceCommand::SetTempo(bpm).try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "preset tempo BPM",
                    ..
                })
            ));
        }
    }

    #[test]
    fn tempo_metronome_uses_sparse_global_tempo_parameters_in_input_order() {
        let frames = set_tempo_parameters(vec![(6, 0.5), (10, 2.0 / 3.0)]);
        assert_eq!(frames.len(), 2);
        for (frame, index) in frames.iter().zip([6_u32, 10]) {
            assert_eq!(frame.message_type, profile::MESSAGE_TYPE_GLOBAL_TEMPO);
            let decoded = pa::GlobalTempoMessage::decode(frame.payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(decoded.params.len(), 1);
            assert_eq!(decoded.params[0].index, Some(param::Index::Index(index)));
        }
        let mode = set_tempo_mode(true);
        assert_eq!(mode.message_type, 33);
        let decoded = pa::GlobalTempoMessage::decode(mode.payload.as_slice()).unwrap();
        assert_eq!(decoded.params[0].index, Some(param::Index::Index(1)));
        assert!(matches!(
            decoded.params[0].param_values[0].value,
            Some(param_value::Value::FloatValue(1.0))
        ));
    }

    #[test]
    fn tempo_metronome_rejects_non_ui_values_at_fallible_boundary() {
        for parameters in [
            vec![(4, 0.5)],
            vec![(6, 0.51)],
            vec![(7, 0.5)],
            vec![(8, 0.5)],
            vec![(9, 0.3)],
            vec![(10, 0.5)],
            vec![(23, 0.0)],
            vec![(10, 0.0), (10, 1.0)],
        ] {
            assert!(DeviceOperation::SetTempoParameters(parameters)
                .try_encode()
                .is_err());
        }
        assert!(
            DeviceOperation::SetTempoParameters(vec![(3, 0.5), (22, 2.0 / 3.0)])
                .try_encode()
                .is_ok()
        );
    }

    #[test]
    fn global_tempo_matches_the_expected_wire_contract() {
        let outbound = set_global_tempo(75);
        assert_eq!(outbound.message_type, 33);
        assert_eq!(
            outbound.payload,
            [0x08, 0x01, 0x1a, 0x09, 0x2a, 0x05, 0x15, 0x33, 0x33, 0x33, 0x3e, 0x30, 0x00]
        );
        let decoded = pa::GlobalTempoMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(decoded.params[0].index, Some(param::Index::Index(0)));
        assert!(matches!(
            decoded.params[0].param_values[0].value,
            Some(param_value::Value::FloatValue(value)) if (value - 0.175).abs() < 1e-6
        ));
        // Parameter 0 is the global tempo; parameter 1 stays the mode switch.
        let mode = set_tempo_mode(true);
        assert_ne!(mode.payload, outbound.payload);
    }

    #[test]
    fn global_tempo_accepts_endpoints_and_rejects_out_of_range_at_fallible_boundary() {
        for (bpm, expected) in [(40u32, 0.0f32), (240, 1.0)] {
            let decoded =
                pa::GlobalTempoMessage::decode(set_global_tempo(bpm).payload.as_slice()).unwrap();
            let Some(param_value::Value::FloatValue(value)) =
                decoded.params[0].param_values[0].value
            else {
                panic!("global tempo parameter is not a float");
            };
            assert!(
                (value - expected).abs() < 1e-6,
                "{bpm} bpm normalized to {value}, expected {expected}"
            );
        }
        for bpm in [0, 39, 241, 10_000] {
            assert!(DeviceOperation::SetGlobalTempo(bpm).try_encode().is_err());
        }
    }

    /// The second half of the tuner pair; see `set_tuner_meter`.
    #[test]
    fn tuner_meter_writes_only_the_enable_meter_field() {
        for enabled in [true, false] {
            let frames = DeviceOperation::SetTunerMeter(enabled)
                .try_encode()
                .unwrap();
            assert_eq!(frames.len(), 1);
            let outbound = &frames[0];
            assert_eq!(outbound.message_type, 6);
            let decoded = pa::TunerMessage::decode(outbound.payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(
                decoded.enable_meter,
                Some(pa::tuner_message::EnableMeter::EnableMeter(enabled))
            );
            // A tuner write that also carried input, mute or reference would
            // silently overwrite the user's settings.
            assert!(decoded.input_port_id.is_none());
            assert!(decoded.mute.is_none());
            assert!(decoded.frequency.is_none());
        }
        // Exact bytes for the enable half of the pair.
        assert_eq!(set_tuner_meter(true).payload, [0x08, 0x01, 0x30, 0x01]);
        assert_eq!(set_tuner_meter(false).payload, [0x08, 0x01, 0x30, 0x00]);
    }

    #[test]
    fn master_volume_uses_the_normalized_device_scale_without_calibration() {
        let outbound = set_master_volume(0.57);
        assert_eq!(outbound.message_type, 17);
        assert_eq!(outbound.payload, [0x08, 0x01, 0x1d, 0x85, 0xeb, 0x11, 0x3f]);
        let message = pa::MasterVolumeMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(message.action, pa::message_action::Enum::Update as i32);
        assert_eq!(
            message.volume,
            Some(pa::master_volume_message::Volume::Volume(0.57))
        );
        assert!(message.calibrate.is_none());

        assert_eq!(
            set_master_volume(0.0).payload,
            [0x08, 0x01, 0x1d, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            set_master_volume(1.0).payload,
            [0x08, 0x01, 0x1d, 0x00, 0x00, 0x80, 0x3f]
        );
        for volume in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            assert!(DeviceCommand::SetMasterVolume(volume).try_encode().is_err());
        }
    }

    #[test]
    fn device_command_owns_intent_to_wire_selection() {
        assert_eq!(
            DeviceCommand::SelectScene(3).try_encode().unwrap(),
            select_scene(3)
        );
        assert_eq!(
            DeviceCommand::SetBypass {
                row: 1,
                column: 4,
                bypassed: true
            }
            .try_encode()
            .unwrap(),
            set_bypass(1, 4, true)
        );
        assert_eq!(
            DeviceCommand::Disconnect.try_encode().unwrap(),
            connection(false)
        );
    }

    #[test]
    fn setlist_positions_carry_no_request_id() {
        let outbound = setlist_position("/media/p4/Presets/My Presets", 42, false);
        let decoded = pa::SetlistPositionMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(decoded.request_id, None);
        assert_eq!(
            decoded.position,
            Some(pa::setlist_position_message::Position::Position(42))
        );
        assert_eq!(
            outbound.payload,
            b"\x08\x01\x1a\x1c/media/p4/Presets/My Presets\x20\x2a\x28\x00"
        );
    }

    #[test]
    fn navigate_bank_absolute_setlist_position_bytes_match_expected_shape() {
        let plain = setlist_position("/user/live", 16, false);
        assert_eq!(plain.message_type, profile::MESSAGE_TYPE_SETLIST_POSITION);
        assert_eq!(plain.payload, b"\x08\x01\x1a\x0a/user/live\x20\x10\x28\x00");
    }

    #[test]
    fn active_setlist_position_read_matches_the_exact_expected_frame() {
        let outbound = read_setlist_position();
        assert_eq!(
            outbound.message_type,
            profile::MESSAGE_TYPE_SETLIST_POSITION
        );
        assert_eq!(outbound.payload, [0x08, 0x03]);
        let decoded = pa::SetlistPositionMessage::decode(outbound.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Read as i32);
        assert!(decoded.request_id.is_none());
        assert!(decoded.position.is_none());
        assert!(decoded.folder_key.is_none());
    }

    #[test]
    fn advanced_device_operations_own_verified_sparse_wire_shapes() {
        let add = DeviceOperation::AddBlock {
            row: 2,
            column: 5,
            model_id: 12_345,
        }
        .try_encode()
        .unwrap();
        assert_eq!(add.len(), 1);
        let grid = pa::GridMessage::decode(add[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(
            preset.chains[0].models[0].hash,
            Some(model::Hash::Hash(12_345))
        );

        let assignment = DeviceOperation::SetFootswitch {
            row: 1,
            column: 3,
            footswitch: Some(6),
            assignment_type: StompAssignmentType::Secondary,
            previous_footswitch: Some(5),
            previous_assignment_type: Some(StompAssignmentType::Primary),
        }
        .try_encode()
        .unwrap();
        assert_eq!(
            assignment.len(),
            2,
            "assignment must delete stale state before update"
        );
        let delete = pa::GridMessage::decode(assignment[0].payload.as_slice()).unwrap();
        assert_eq!(
            assignment[0].payload,
            [8, 2, 26, 11, 218, 1, 8, 8, 1, 16, 3, 24, 5, 32, 0]
        );
        let pa::grid_message::Preset::Preset(delete) = delete.preset.unwrap();
        assert_eq!(
            delete.stomp_mode_assignments[0].r#type,
            Some(crate::proto::stomp_mode_assignment::TypeOneOf::Type(0))
        );
        let update = pa::GridMessage::decode(assignment[1].payload.as_slice()).unwrap();
        assert_eq!(
            assignment[1].payload,
            [8, 1, 26, 11, 218, 1, 8, 8, 1, 16, 3, 24, 6, 32, 1]
        );
        let pa::grid_message::Preset::Preset(update) = update.preset.unwrap();
        assert_eq!(
            update.stomp_mode_assignments[0].r#type,
            Some(crate::proto::stomp_mode_assignment::TypeOneOf::Type(1))
        );

        let fresh =
            set_footswitch_transition(1, 3, Some(6), StompAssignmentType::Secondary, None, None);
        assert_eq!(fresh.len(), 1);
        assert_eq!(
            pa::GridMessage::decode(fresh[0].payload.as_slice())
                .unwrap()
                .action,
            pa::message_action::Enum::Update as i32
        );

        let clear = set_footswitch_transition(
            1,
            3,
            None,
            StompAssignmentType::Primary,
            Some(5),
            Some(StompAssignmentType::Primary),
        );
        assert_eq!(clear.len(), 1);
        assert_eq!(clear[0].payload, assignment[0].payload);

        assert!(set_footswitch_transition(
            1,
            3,
            Some(5),
            StompAssignmentType::Primary,
            Some(5),
            Some(StompAssignmentType::Primary),
        )
        .is_empty());

        assert!(DeviceOperation::SetFootswitch {
            row: 1,
            column: 3,
            footswitch: Some(8),
            assignment_type: StompAssignmentType::Primary,
            previous_footswitch: None,
            previous_assignment_type: None,
        }
        .try_encode()
        .is_err());
        assert!(DeviceOperation::SetFootswitch {
            row: 1,
            column: 3,
            footswitch: Some(5),
            assignment_type: StompAssignmentType::Primary,
            previous_footswitch: Some(4),
            previous_assignment_type: None,
        }
        .try_encode()
        .is_err());

        let split = DeviceOperation::SetChainSplit {
            row: 0,
            split_column: Some(2),
            mix_column: Some(7),
        }
        .try_encode()
        .unwrap();
        let grid = pa::GridMessage::decode(split[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(
            preset.chains[0].split_control_points[0],
            SplitControlPoints { split: 2, mix: 7 }
        );
        assert_eq!(
            split[0].payload,
            vec![8, 1, 26, 10, 90, 8, 50, 4, 8, 2, 16, 7, 112, 0]
        );

        let mixer_only = DeviceOperation::SetChainSplit {
            row: 2,
            split_column: None,
            mix_column: Some(7),
        }
        .try_encode()
        .unwrap();
        let grid = pa::GridMessage::decode(mixer_only[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(
            preset.chains[0].split_control_points[0],
            SplitControlPoints { split: -1, mix: 7 }
        );
        assert!(DeviceOperation::SetChainSplit {
            row: 1,
            split_column: Some(0),
            mix_column: Some(7),
        }
        .try_encode()
        .is_err());
        assert!(DeviceOperation::SetChainSplit {
            row: 0,
            split_column: Some(4),
            mix_column: Some(3),
        }
        .try_encode()
        .is_err());

        let mute = DeviceOperation::SetSplitMute {
            row: 2,
            muted: true,
        }
        .try_encode()
        .unwrap();
        let grid = pa::GridMessage::decode(mute[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
        assert_eq!(preset.chains[0].row, Some(chain::Row::Row(2)));
        assert_eq!(
            preset.chains[0].split_bypass,
            vec![SceneBypass { bypass: true }]
        );
        assert!(preset.chains[0].mix_bypass.is_empty());

        let save = DeviceOperation::SavePreset {
            setlist_key: "/media/p4/Presets/My Presets".into(),
            position: 220,
            name: "Shared Rust save".into(),
            instrument: 2,
        }
        .try_encode()
        .unwrap();
        assert_eq!(save[0].message_type, 4);
        let file = pa::FileMessage::decode(save[0].payload.as_slice()).unwrap();
        let pa::file_message::Folder::Folder(folder) = file.folder.unwrap();
        assert_eq!(folder.files.len(), 1);
        assert_eq!(
            folder.files[0].index,
            Some(pa::product_data::Index::Index(220))
        );
    }

    #[test]
    fn chain_input_uses_the_exact_sparse_grid_shape_and_route_domain() {
        let encoded = DeviceOperation::SetChainInput {
            row: 0,
            input_id: 1,
        }
        .try_encode()
        .unwrap();
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(
            encoded[0].payload,
            [0x08, 0x01, 0x1a, 0x06, 0x5a, 0x04, 0x08, 0x01, 0x70, 0x00]
        );

        let last = DeviceOperation::SetChainInput {
            row: 3,
            input_id: 13,
        }
        .try_encode()
        .unwrap();
        assert_eq!(
            last[0].payload,
            [0x08, 0x01, 0x1a, 0x06, 0x5a, 0x04, 0x08, 0x0d, 0x70, 0x03]
        );
        assert!(DeviceOperation::SetChainInput {
            row: 0,
            input_id: 14,
        }
        .try_encode()
        .is_err());
        assert!(DeviceOperation::SetChainInput {
            row: 4,
            input_id: 1,
        }
        .try_encode()
        .is_err());
    }

    #[test]
    fn split_mute_uses_selected_scene_sparse_shape_and_paired_rows() {
        let muted = DeviceOperation::SetSplitMute {
            row: 0,
            muted: true,
        }
        .try_encode()
        .unwrap();
        assert_eq!(muted.len(), 1);
        assert_eq!(muted[0].message_type, profile::MESSAGE_TYPE_GRID);
        assert_eq!(
            muted[0].payload,
            [0x08, 0x01, 0x1a, 0x08, 0x5a, 0x06, 0x52, 0x02, 0x08, 0x01, 0x70, 0x00]
        );
        let unmuted = DeviceOperation::SetSplitMute {
            row: 2,
            muted: false,
        }
        .try_encode()
        .unwrap();
        assert_eq!(
            unmuted[0].payload,
            [0x08, 0x01, 0x1a, 0x06, 0x5a, 0x04, 0x52, 0x00, 0x70, 0x02]
        );
        for row in [1, 3, 4] {
            assert!(DeviceOperation::SetSplitMute { row, muted: true }
                .try_encode()
                .is_err());
        }
    }

    #[test]
    fn general_settings_writes_are_sparse_and_nested_values_are_complete() {
        let brightness = DeviceOperation::SetGeneralInteger {
            setting: "screenBrightness".into(),
            value: 29,
        }
        .try_encode()
        .unwrap();
        let decoded = pa::GeneralSettingsMessage::decode(brightness[0].payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert!(decoded.screen_brightness.is_some());
        assert!(decoded.led_brightness.is_none());
        assert!(
            decoded.power_option.is_none(),
            "safe setting writes must never carry power commands"
        );
        assert!(decoded.reset_wifi_networks.is_none());

        let assignment = DeviceOperation::SetMasterVolumeAssignment {
            out12: true,
            out34: false,
            send12: true,
            headphones: false,
        }
        .try_encode()
        .unwrap();
        let decoded = pa::GeneralSettingsMessage::decode(assignment[0].payload.as_slice()).unwrap();
        let pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(value) =
            decoded.master_volume_assignment.unwrap();
        assert!(value.out12);
        assert!(!value.out34);
        assert!(value.send12);
        assert!(!value.headphones);

        let bypass = set_global_bypass(
            Some([true, false, true, false]),
            Some([false, true, false, true]),
        );
        assert_eq!(bypass.len(), 2);
        let cab = pa::GeneralSettingsMessage::decode(bypass[0].payload.as_slice()).unwrap();
        let ir = pa::GeneralSettingsMessage::decode(bypass[1].payload.as_slice()).unwrap();
        assert!(cab.global_bypass_cab.is_some());
        assert!(cab.global_bypass_ir.is_none());
        assert!(ir.global_bypass_cab.is_none());
        assert!(ir.global_bypass_ir.is_some());
    }

    #[test]
    fn global_bypass_frames_are_full_family_ordered_and_not_combined() {
        let frames = set_global_bypass(
            Some([true, false, true, false]),
            Some([false, true, false, true]),
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0].message_type,
            profile::MESSAGE_TYPE_GENERAL_SETTINGS
        );
        assert_eq!(
            frames[1].message_type,
            profile::MESSAGE_TYPE_GENERAL_SETTINGS
        );
        assert_eq!(
            frames[0].payload,
            [0x08, 0x01, 0x3a, 0x04, 0x08, 0x01, 0x18, 0x01]
        );
        assert_eq!(
            frames[1].payload,
            [0x08, 0x01, 0x42, 0x04, 0x10, 0x01, 0x20, 0x01]
        );

        let no_op_shape = set_global_bypass(Some([false; 4]), Some([false; 4]));
        assert_eq!(
            no_op_shape.len(),
            2,
            "planner does not suppress equal values"
        );
        assert_eq!(no_op_shape[0].payload, [0x08, 0x01, 0x3a, 0x00]);
        assert_eq!(no_op_shape[1].payload, [0x08, 0x01, 0x42, 0x00]);
    }

    #[test]
    fn global_bypass_sends_only_the_supplied_family_one_frame_each() {
        let cab_only = set_global_bypass(Some([true, false, true, false]), None);
        assert_eq!(cab_only.len(), 1, "Cab alone is one frame");
        assert_eq!(
            cab_only[0].payload,
            [0x08, 0x01, 0x3a, 0x04, 0x08, 0x01, 0x18, 0x01]
        );
        let ir_only = set_global_bypass(None, Some([false, true, false, true]));
        assert_eq!(ir_only.len(), 1, "IR alone is one frame");
        assert_eq!(
            ir_only[0].payload,
            [0x08, 0x01, 0x42, 0x04, 0x10, 0x01, 0x20, 0x01]
        );
        assert!(set_global_bypass(None, None).is_empty());
        assert!(DeviceOperation::SetGlobalBypass {
            cab: None,
            ir: None
        }
        .try_encode()
        .is_err());
    }

    #[test]
    fn every_general_toggle_is_one_exact_sparse_boolean_frame() {
        let cases: [(&str, &[u8], &[u8]); 12] = [
            (
                "lockScreenAndVolumeKnob",
                &[0x08, 0x01, 0x30, 0x00],
                &[0x08, 0x01, 0x30, 0x01],
            ),
            (
                "midiOverUsb",
                &[0x08, 0x01, 0x50, 0x00],
                &[0x08, 0x01, 0x50, 0x01],
            ),
            (
                "ignoreDuplicatePc",
                &[0x08, 0x01, 0x60, 0x00],
                &[0x08, 0x01, 0x60, 0x01],
            ),
            (
                "stompModeAutoAssign",
                &[0x08, 0x01, 0xb0, 0x01, 0x00],
                &[0x08, 0x01, 0xb0, 0x01, 0x01],
            ),
            (
                "swapTempoTunerAccess",
                &[0x08, 0x01, 0xb8, 0x01, 0x00],
                &[0x08, 0x01, 0xb8, 0x01, 0x01],
            ),
            (
                "disableInternetConnectionCheck",
                &[0x08, 0x01, 0xd0, 0x01, 0x00],
                &[0x08, 0x01, 0xd0, 0x01, 0x01],
            ),
            (
                "dynamicDelayCompensation",
                &[0x08, 0x01, 0xd8, 0x01, 0x00],
                &[0x08, 0x01, 0xd8, 0x01, 0x01],
            ),
            (
                "presetDimmed",
                &[0x08, 0x01, 0xe8, 0x01, 0x00],
                &[0x08, 0x01, 0xe8, 0x01, 0x01],
            ),
            (
                "sceneDimmed",
                &[0x08, 0x01, 0xf0, 0x01, 0x00],
                &[0x08, 0x01, 0xf0, 0x01, 0x01],
            ),
            (
                "stompDimmed",
                &[0x08, 0x01, 0xf8, 0x01, 0x00],
                &[0x08, 0x01, 0xf8, 0x01, 0x01],
            ),
            (
                "midiClockIn",
                &[0x08, 0x01, 0x90, 0x02, 0x00],
                &[0x08, 0x01, 0x90, 0x02, 0x01],
            ),
            (
                "gigViewStompAccess",
                &[0x08, 0x01, 0xb0, 0x02, 0x00],
                &[0x08, 0x01, 0xb0, 0x02, 0x01],
            ),
        ];
        for (setting, disabled, enabled) in cases {
            for (value, expected) in [(false, disabled), (true, enabled)] {
                let frame = set_general_toggle(setting, value).unwrap();
                assert_eq!(frame.message_type, profile::MESSAGE_TYPE_GENERAL_SETTINGS);
                assert_eq!(frame.payload, expected, "{setting}={value}");
                let decoded = pa::GeneralSettingsMessage::decode(frame.payload.as_slice()).unwrap();
                assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
                assert!(decoded.request_id.is_none());
            }
        }
    }

    #[test]
    fn master_volume_assignment_is_one_atomic_sparse_frame_for_every_tuple() {
        for mask in 0_u8..16 {
            let values = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0];
            let frames = DeviceOperation::SetMasterVolumeAssignment {
                out12: values[0],
                out34: values[1],
                send12: values[2],
                headphones: values[3],
            }
            .try_encode()
            .unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(
                frames[0].message_type,
                profile::MESSAGE_TYPE_GENERAL_SETTINGS
            );

            let decoded = pa::GeneralSettingsMessage::decode(frames[0].payload.as_slice()).unwrap();
            assert_eq!(
                decoded,
                pa::GeneralSettingsMessage {
                    action: pa::message_action::Enum::Update as i32,
                    master_volume_assignment: Some(
                        pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                            pa::MasterVolumeAssignmentOptions {
                                out12: values[0],
                                out34: values[1],
                                send12: values[2],
                                headphones: values[3],
                            },
                        ),
                    ),
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn global_eq_and_mode_cycle_writes_are_typed_and_sparse() {
        let bypass = set_global_eq_bypassed(true);
        assert_eq!(bypass.message_type, 38);
        let decoded = pa::GlobalEqMessage::decode(bypass.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert!(matches!(
            decoded.bypassed,
            Some(pa::global_eq_message::Bypassed::Bypassed(true))
        ));
        assert!(decoded.parameters.is_empty());

        let preset = load_global_eq_model_preset("preset-id", true, 123);
        assert_eq!(preset.message_type, 38);
        assert_eq!(
            preset.payload,
            [
                0x08, 0x01, 0x2a, 0x0f, 0x0a, 0x09, b'p', b'r', b'e', b's', b'e', b't', b'-', b'i',
                b'd', 0x10, 0x01, 0x18, 0x7b,
            ]
        );
        let decoded = pa::GlobalEqMessage::decode(preset.payload.as_slice()).unwrap();
        let pa::global_eq_message::ModelPresetToLoad::ModelPresetToLoad(id) =
            decoded.model_preset_to_load.unwrap();
        assert_eq!(
            id.value,
            Some(pa::model_preset_id::Value::Value("preset-id".into()))
        );
        assert_eq!(
            id.is_factory,
            Some(pa::model_preset_id::IsFactory::IsFactory(true))
        );
        assert_eq!(id.hash, Some(pa::model_preset_id::Hash::Hash(123)));

        let controls = set_global_eq_parameters(&[(0, 0.75), (4, 1.0)]);
        assert_eq!(controls.len(), 2);
        for (message, (expected_index, expected_value)) in
            controls.iter().zip([(0, 0.75), (4, 1.0)])
        {
            let decoded = pa::GlobalEqMessage::decode(message.payload.as_slice()).unwrap();
            assert!(decoded.bypassed.is_none());
            assert_eq!(decoded.parameters.len(), 1);
            assert_eq!(decoded.parameters[0].parameter_index, expected_index);
            assert_eq!(decoded.parameters[0].value, expected_value);
        }

        let cycle = set_mode_cycle(&[7, 1, 2]);
        assert_eq!(cycle.message_type, 14);
        let decoded = pa::ModeMessage::decode(cycle.payload.as_slice()).unwrap();
        let pa::mode_message::AvailableModes::AvailableModes(available) =
            decoded.available_modes.unwrap();
        assert_eq!(available.modes, vec![7, 1, 2]);

        assert_eq!(read_global_eq().message_type, 38);
        assert_eq!(read_global_eq().payload, [0x08, 0x03]);
        assert_eq!(read_mode_cycle().message_type, 14);
        assert_eq!(read_mode_cycle().payload, [0x08, 0x03]);
        let global_tempo = DeviceOperation::ReadGlobalTempo.try_encode().unwrap();
        assert_eq!(global_tempo.len(), 1);
        assert_eq!(global_tempo[0].message_type, 33);
        assert_eq!(global_tempo[0].payload, [0x08, 0x03]);
        assert_eq!(read_looper_status().message_type, 28);
        assert_eq!(read_looper_status().payload, [0x08, 0x03]);
    }

    #[test]
    fn typed_global_eq_rejects_non_finite_values_before_encoding() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let result =
                DeviceOperation::SetGlobalEqParameters(vec![(0, 0.5), (25, value)]).try_encode();
            assert!(matches!(
                result,
                Err(CommandEncodeError::UnsupportedValue {
                    field: "Global EQ parameter value",
                    ..
                })
            ));
        }
        // Finite-only validation does not invent firmware index or value bounds.
        let controls = [(0, 0.75), (25, -2.0), (26, 2.0)];
        let frames = DeviceOperation::SetGlobalEqParameters(controls.to_vec())
            .try_encode()
            .unwrap();
        assert_eq!(frames.len(), controls.len());
        for (frame, (index, value)) in frames.iter().zip(controls) {
            assert_eq!(frame.message_type, crate::profile::MESSAGE_TYPE_GLOBAL_EQ);
            let decoded = pa::GlobalEqMessage::decode(frame.payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(decoded.parameters.len(), 1);
            assert_eq!(decoded.parameters[0].parameter_index, index);
            assert_eq!(decoded.parameters[0].value, value);
        }
    }

    #[test]
    fn typed_model_preset_hash_must_fit_its_signed_source_field() {
        for model_hash in [i32::MAX as u32 + 1, u32::MAX] {
            assert!(matches!(
                DeviceOperation::CreateModelPreset {
                    row: 0,
                    column: 0,
                    model_hash,
                    name: "Tone".into(),
                }
                .try_encode(),
                Err(CommandEncodeError::SignedFieldOverflow {
                    field: "model preset source hash"
                })
            ));
        }
        for model_hash in [7, i32::MAX as u32] {
            let frames = DeviceOperation::CreateModelPreset {
                row: 0,
                column: 0,
                model_hash,
                name: "Tone".into(),
            }
            .try_encode()
            .unwrap();
            assert_eq!(frames.len(), 1);
            let decoded = pa::ModelPresetMessage::decode(frames[0].payload.as_slice()).unwrap();
            assert_eq!(
                decoded.create_from_hash,
                Some(pa::model_preset_message::CreateFromHash::CreateFromHash(
                    model_hash as i32
                ))
            );
            let pa::model_preset::Id::Id(id) = decoded.presets[0].id.as_ref().unwrap();
            assert_eq!(id.hash, Some(pa::model_preset_id::Hash::Hash(model_hash)));
        }
    }

    #[test]
    fn mode_cycle_rejects_values_the_editor_cannot_produce() {
        for slots in [vec![], vec![0, 1, 2, 3], vec![9], vec![1, 1]] {
            assert!(DeviceOperation::SetModeCycle(slots).try_encode().is_err());
        }
        assert!(DeviceOperation::SetModeCycle(vec![8, 0, 6])
            .try_encode()
            .is_ok());
    }

    #[test]
    fn scene_bypass_behavior_rejects_unknown_enum_values() {
        for value in [-1, 3] {
            assert!(DeviceOperation::SetSceneBypassBehavior(value)
                .try_encode()
                .is_err());
        }
        for value in 0..=2 {
            assert!(DeviceOperation::SetSceneBypassBehavior(value)
                .try_encode()
                .is_ok());
        }
    }

    #[test]
    fn global_eq_output_frames_are_one_parameter_each_and_ordered() {
        let frames = set_global_eq_parameters(&[(25, 0.25), (26, 1.0), (27, 0.0)]);
        assert_eq!(frames.len(), 3);
        for (frame, (index, value)) in frames.iter().zip([(25, 0.25), (26, 1.0), (27, 0.0)]) {
            assert_eq!(frame.message_type, 38);
            let decoded = pa::GlobalEqMessage::decode(frame.payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(decoded.parameters.len(), 1);
            assert_eq!(decoded.parameters[0].parameter_index, index);
            assert_eq!(decoded.parameters[0].value, value);
            assert!(decoded.request_id.is_none());
            assert!(decoded.bypassed.is_none());
            assert!(decoded.model_preset_to_load.is_none());
        }
    }

    #[test]
    fn library_reads_and_mutations_use_the_verified_file_protocol() {
        let recents = read_recents_favorites(false, 91);
        assert_eq!(recents.message_type, 20);
        assert_eq!(recents.payload, [0x08, 0x03, 0x10, 0x5b]);
        let decoded = pa::RecentsFavoritesMessage::decode(recents.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Read as i32);
        assert!(!decoded.is_favorites);
        assert!(matches!(
            decoded.request_id,
            Some(pa::recents_favorites_message::RequestId::RequestId(91))
        ));

        let favorite = set_favorite(
            "Stage".into(),
            "/media/p4/Presets/Live".into(),
            "preset-stage".into(),
            false,
            false,
            true,
        );
        let decoded = pa::RecentsFavoritesMessage::decode(favorite.payload.as_slice()).unwrap();
        assert_eq!(
            favorite.payload,
            [
                0x18, 0x01, 0x22, 0x2d, 0x0a, 0x05, 0x53, 0x74, 0x61, 0x67, 0x65, 0x12, 0x16, 0x2f,
                0x6d, 0x65, 0x64, 0x69, 0x61, 0x2f, 0x70, 0x34, 0x2f, 0x50, 0x72, 0x65, 0x73, 0x65,
                0x74, 0x73, 0x2f, 0x4c, 0x69, 0x76, 0x65, 0x32, 0x0c, 0x70, 0x72, 0x65, 0x73, 0x65,
                0x74, 0x2d, 0x73, 0x74, 0x61, 0x67, 0x65,
            ]
        );
        assert_eq!(decoded.action, pa::message_action::Enum::Create as i32);
        assert!(decoded.is_favorites);
        assert_eq!(decoded.items[0].name, "Stage");
        assert_eq!(
            decoded.r#type,
            pa::recents_favorites_message::RecentFavoriteType::Preset as i32
        );
        assert!(decoded.items[0].folder_name.is_empty());
        assert_eq!(decoded.items[0].product_key, "preset-stage");

        let unfavorite = set_favorite(
            "Stage".into(),
            "/media/p4/Presets/Live".into(),
            "preset-stage".into(),
            false,
            false,
            false,
        );
        let decoded = pa::RecentsFavoritesMessage::decode(unfavorite.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Delete as i32);

        let plugin = DeviceOperation::SetFavorite {
            name: "Plugin Stage".into(),
            folder_key: "/plugins/Presets".into(),
            product_key: "plugin:preset:stage".into(),
            is_factory: false,
            is_plugin: true,
            favorite: true,
        }
        .try_encode()
        .unwrap();
        let decoded = pa::RecentsFavoritesMessage::decode(plugin[0].payload.as_slice()).unwrap();
        assert!(decoded.items[0].is_plugin);

        for invalid in [
            DeviceOperation::SetFavorite {
                name: "../Stage".into(),
                folder_key: "/media/p4/Presets/Live".into(),
                product_key: "preset-stage".into(),
                is_factory: false,
                is_plugin: false,
                favorite: true,
            },
            DeviceOperation::SetFavorite {
                name: "Stage".into(),
                folder_key: "/media/p4/Presets/../Factory".into(),
                product_key: "preset-stage".into(),
                is_factory: false,
                is_plugin: false,
                favorite: true,
            },
            DeviceOperation::SetFavorite {
                name: "Stage".into(),
                folder_key: "/media/p4/Presets/Live".into(),
                product_key: " preset-stage".into(),
                is_factory: false,
                is_plugin: false,
                favorite: true,
            },
        ] {
            assert!(invalid.try_encode().is_err());
        }

        let pinned = set_model_pinned(12_345, true);
        assert_eq!(pinned.payload, [0x1a, 0x02, 0xb9, 0x60]);
        let decoded = pa::PinnedModelsMessage::decode(pinned.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Create as i32);
        assert_eq!(decoded.models, vec![12_345]);
        let pinned = set_model_pinned(12_345, false);
        assert_eq!(pinned.payload, [0x08, 0x02, 0x1a, 0x02, 0xb9, 0x60]);
        let decoded = pa::PinnedModelsMessage::decode(pinned.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Delete as i32);
        assert_eq!(decoded.models, vec![12_345]);

        let folders = read_folders(false);
        assert_eq!(folders.payload, [0x08, 0x03, 0x58, 0x00]);
        let decoded = pa::FileMessage::decode(folders.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Read as i32);
        assert_eq!(
            decoded.omit_factory_content,
            Some(pa::file_message::OmitFactoryContent::OmitFactoryContent(
                false
            ))
        );
        assert!(decoded.request_id.is_none());
        assert!(decoded.r#type.is_none());
        assert!(decoded.folder.is_none());
        assert_eq!(read_folders(true).payload, [0x08, 0x03, 0x58, 0x01]);
        assert_eq!(
            DeviceOperation::ListPresetFolders.try_encode().unwrap()[0].payload,
            folders.payload
        );

        let listing = read_library_files_query("local_ir_root".into(), Some(1), Some(92));
        let decoded = pa::FileMessage::decode(listing.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Read as i32);
        assert!(matches!(
            decoded.request_id,
            Some(pa::file_message::RequestId::RequestId(92))
        ));
        assert!(matches!(
            decoded.r#type,
            Some(pa::file_message::Type::Type(1))
        ));
        assert!(decoded.folder.is_none());

        let captures = read_library_files_query("local_nc_root".into(), None, None);
        assert_eq!(captures.payload, [0x08, 0x03, 0x58, 0x00]);
        let decoded = pa::FileMessage::decode(captures.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Read as i32);
        assert!(decoded.request_id.is_none());
        assert!(decoded.r#type.is_none());
        assert!(decoded.folder.is_none());
        assert_eq!(
            decoded.omit_factory_content,
            Some(pa::file_message::OmitFactoryContent::OmitFactoryContent(
                false
            ))
        );

        for (message, action) in [
            (
                create_setlist("Tour".into()),
                pa::message_action::Enum::Create,
            ),
            (
                delete_setlist("Tour".into()),
                pa::message_action::Enum::Delete,
            ),
        ] {
            let decoded = pa::FileMessage::decode(message.payload.as_slice()).unwrap();
            assert_eq!(decoded.action, action as i32);
            let delete_from_library = decoded.delete_from_library;
            let pa::file_message::Folder::Folder(folder) = decoded.folder.unwrap();
            assert_eq!(
                folder.key,
                Some(pa::folder_info::Key::Key("/media/p4/Presets/Tour".into()))
            );
            if action == pa::message_action::Enum::Create {
                assert_eq!(
                    folder.parent_key,
                    Some(pa::folder_info::ParentKey::ParentKey(
                        "/media/p4/Presets".into()
                    ))
                );
            } else {
                assert_eq!(
                    folder.is_factory,
                    Some(pa::folder_info::IsFactory::IsFactory(false))
                );
                assert!(folder.name.is_none());
                assert_eq!(
                    delete_from_library,
                    Some(pa::file_message::DeleteFromLibrary::DeleteFromLibrary(
                        false
                    ))
                );
            }
        }

        let moved = move_preset(
            "/media/p4/Presets/Live".into(),
            "device-product-key-42".into(),
            17,
        );
        let decoded = pa::FileMessage::decode(moved.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Move as i32);
        assert_eq!(decoded.r#type, Some(pa::file_message::Type::Type(0)));
        let pa::file_message::Folder::Folder(source) = decoded.folder.unwrap();
        assert_eq!(
            source.key,
            Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into()))
        );
        assert_eq!(
            source.is_downloads,
            Some(pa::folder_info::IsDownloads::IsDownloads(false))
        );
        assert_eq!(
            source.files[0].key,
            Some(pa::product_data::Key::Key("device-product-key-42".into()))
        );
        let pa::file_message::ToFolder::ToFolder(destination) = decoded.to_folder.unwrap();
        assert_eq!(
            destination.key,
            Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into()))
        );
        assert_eq!(
            destination.files[0].index,
            Some(pa::product_data::Index::Index(17))
        );
        assert_eq!(
            moved.payload,
            [
                0x08, 0x04, 0x18, 0x00, 0x22, 0x35, 0x0a, 0x16, b'/', b'm', b'e', b'd', b'i', b'a',
                b'/', b'p', b'4', b'/', b'P', b'r', b'e', b's', b'e', b't', b's', b'/', b'L', b'i',
                b'v', b'e', 0x20, 0x00, 0x28, 0x00, 0x3a, 0x17, 0x0a, 0x15, b'd', b'e', b'v', b'i',
                b'c', b'e', b'-', b'p', b'r', b'o', b'd', b'u', b'c', b't', b'-', b'k', b'e', b'y',
                b'-', b'4', b'2', 0x2a, 0x1c, 0x0a, 0x16, b'/', b'm', b'e', b'd', b'i', b'a', b'/',
                b'p', b'4', b'/', b'P', b'r', b'e', b's', b'e', b't', b's', b'/', b'L', b'i', b'v',
                b'e', 0x3a, 0x02, 0x10, 0x11,
            ]
        );

        let swapped = swap_preset(
            "/media/p4/Presets/Live".into(),
            "device-product-key-42".into(),
            17,
        );
        let decoded = pa::FileMessage::decode(swapped.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Swap as i32);
        assert_eq!(&swapped.payload[..2], &[0x08, 0x08]);
        let pa::file_message::Folder::Folder(source) = decoded.folder.unwrap();
        assert!(source.is_downloads.is_none());

        let copied = copy_preset(
            "/opt/neural/presets".into(),
            "/opt/neural/presets/Factory Lead.pb".into(),
            true,
            "/media/p4/Presets/Live".into(),
            23,
        );
        let decoded = pa::FileMessage::decode(copied.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Copy as i32);
        assert_eq!(decoded.r#type, Some(pa::file_message::Type::Type(0)));
        let pa::file_message::Folder::Folder(source) = decoded.folder.unwrap();
        assert_eq!(
            source.key,
            Some(pa::folder_info::Key::Key("/opt/neural/presets".into()))
        );
        assert_eq!(
            source.is_factory,
            Some(pa::folder_info::IsFactory::IsFactory(true))
        );
        assert_eq!(
            source.is_downloads,
            Some(pa::folder_info::IsDownloads::IsDownloads(false))
        );
        assert_eq!(
            source.files[0].key,
            Some(pa::product_data::Key::Key(
                "/opt/neural/presets/Factory Lead.pb".into()
            ))
        );
        assert_eq!(source.files.len(), 1);
        let pa::file_message::ToFolder::ToFolder(destination) = decoded.to_folder.unwrap();
        assert_eq!(
            destination.key,
            Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into()))
        );
        assert_eq!(
            destination.files[0].index,
            Some(pa::product_data::Index::Index(23))
        );
        assert_eq!(destination.files.len(), 1);

        let exact = copy_preset("/src".into(), "/src/a".into(), false, "/dst".into(), 7);
        assert_eq!(
            exact.payload,
            vec![
                0x08, 0x05, 0x18, 0x00, 0x22, 0x14, 0x0a, 0x04, 0x2f, 0x73, 0x72, 0x63, 0x20, 0x00,
                0x28, 0x00, 0x3a, 0x08, 0x0a, 0x06, 0x2f, 0x73, 0x72, 0x63, 0x2f, 0x61, 0x2a, 0x0a,
                0x0a, 0x04, 0x2f, 0x64, 0x73, 0x74, 0x3a, 0x02, 0x10, 0x07,
            ]
        );

        let renamed = rename_preset(
            "/media/p4/Presets/Live".into(),
            "opaque-device-product-key".into(),
            "Renamed".into(),
            2,
        );
        assert_eq!(
            renamed.payload,
            [
                0x08, 0x01, 0x18, 0x00, 0x22, 0x42, 0x0a, 0x16, 0x2f, 0x6d, 0x65, 0x64, 0x69, 0x61,
                0x2f, 0x70, 0x34, 0x2f, 0x50, 0x72, 0x65, 0x73, 0x65, 0x74, 0x73, 0x2f, 0x4c, 0x69,
                0x76, 0x65, 0x20, 0x00, 0x3a, 0x26, 0x0a, 0x19, 0x6f, 0x70, 0x61, 0x71, 0x75, 0x65,
                0x2d, 0x64, 0x65, 0x76, 0x69, 0x63, 0x65, 0x2d, 0x70, 0x72, 0x6f, 0x64, 0x75, 0x63,
                0x74, 0x2d, 0x6b, 0x65, 0x79, 0x1a, 0x07, 0x52, 0x65, 0x6e, 0x61, 0x6d, 0x65, 0x64,
                0x78, 0x02,
            ]
        );
        let decoded = pa::FileMessage::decode(renamed.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        assert_eq!(decoded.r#type, Some(pa::file_message::Type::Type(0)));
        assert!(decoded.request_id.is_none());
        assert!(decoded.to_folder.is_none());
        let pa::file_message::Folder::Folder(folder) = decoded.folder.unwrap();
        assert_eq!(
            folder.key,
            Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into()))
        );
        assert_eq!(
            folder.is_factory,
            Some(pa::folder_info::IsFactory::IsFactory(false))
        );
        assert!(folder.parent_key.is_none());
        assert!(folder.name.is_none());
        assert_eq!(folder.files.len(), 1);
        assert_eq!(
            folder.files[0].key,
            Some(pa::product_data::Key::Key(
                "opaque-device-product-key".into()
            ))
        );
        assert_eq!(
            folder.files[0].name,
            Some(pa::product_data::Name::Name("Renamed".into()))
        );
        assert!(folder.files[0].index.is_none());
        assert_eq!(
            folder.files[0].instrument,
            Some(pa::product_data::Instrument::Instrument(2))
        );

        // The preset model maps omitted raw ProductData.instrument to its
        // empty-string default, which the rename path projects to enum 0.
        // Optional presence is significant: zero must still serialize.
        let unspecified = rename_preset(
            "/media/p4/Presets/Live".into(),
            "opaque-device-product-key".into(),
            "Renamed".into(),
            0,
        );
        assert_eq!(
            &unspecified.payload[unspecified.payload.len() - 2..],
            &[0x78, 0x00]
        );
        let unspecified = pa::FileMessage::decode(unspecified.payload.as_slice()).unwrap();
        let pa::file_message::Folder::Folder(folder) = unspecified.folder.unwrap();
        assert_eq!(
            folder.files[0].instrument,
            Some(pa::product_data::Instrument::Instrument(0))
        );

        let duplicated = duplicate_setlist("/media/p4/Presets/Live".into());
        assert_eq!(
            duplicated.payload,
            [
                &[0x08, 0x05, 0x18, 0x00, 0x22, 0x1a, 0x0a, 0x16][..],
                b"/media/p4/Presets/Live",
                &[0x20, 0x00][..],
            ]
            .concat()
        );
        let decoded = pa::FileMessage::decode(duplicated.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Copy as i32);
        assert_eq!(decoded.r#type, Some(pa::file_message::Type::Type(0)));
        assert!(decoded.request_id.is_none());
        assert!(decoded.to_folder.is_none());
        assert!(decoded.short_listing.is_none());
        assert!(decoded.ir_payload.is_none());
        assert!(decoded.preset_payload.is_none());
        assert!(decoded.total_bulk_create_count.is_none());
        assert!(decoded.delete_from_library.is_none());
        assert!(decoded.omit_factory_content.is_none());
        assert!(decoded.user_content_estimate.is_none());
        let pa::file_message::Folder::Folder(source) = decoded.folder.unwrap();
        assert_eq!(
            source.key,
            Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into()))
        );
        assert!(source.parent_key.is_none());
        assert!(source.name.is_none());
        assert_eq!(
            source.is_factory,
            Some(pa::folder_info::IsFactory::IsFactory(false))
        );
        assert!(source.is_downloads.is_none());
        assert!(source.is_user_default.is_none());
        assert!(source.is_cloud.is_none());
        assert!(source.is_favorites.is_none());
        assert!(source.is_plugin.is_none());
        assert!(source.files.is_empty());

        let deleted = delete_preset(
            "/media/p4/Presets/Live".into(),
            "/media/p4/Presets/Live/opaque-device-key.pb".into(),
        );
        let decoded = pa::FileMessage::decode(deleted.payload.as_slice()).unwrap();
        assert_eq!(deleted.message_type, profile::MESSAGE_TYPE_FILE);
        assert_eq!(decoded.action, pa::message_action::Enum::Delete as i32);
        assert_eq!(decoded.r#type, Some(pa::file_message::Type::Type(0)));
        assert!(decoded.request_id.is_none());
        assert!(decoded.to_folder.is_none());
        let pa::file_message::Folder::Folder(folder) = decoded.folder.unwrap();
        assert_eq!(
            folder.key,
            Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into()))
        );
        assert_eq!(
            folder.is_factory,
            Some(pa::folder_info::IsFactory::IsFactory(false))
        );
        assert!(folder.parent_key.is_none());
        assert!(folder.name.is_none());
        assert_eq!(folder.files.len(), 1);
        assert_eq!(
            folder.files[0].key,
            Some(pa::product_data::Key::Key(
                "/media/p4/Presets/Live/opaque-device-key.pb".into()
            ))
        );
        assert!(folder.files[0].name.is_none());
        assert!(folder.files[0].index.is_none());
        assert!(folder.files[0].instrument.is_none());
    }

    #[test]
    fn favorites_read_has_exact_correlated_preset_bytes() {
        let message = read_recents_favorites(true, 82);
        assert_eq!(
            message.message_type,
            profile::MESSAGE_TYPE_RECENTS_FAVORITES
        );
        assert_eq!(message.payload, [0x08, 0x03, 0x10, 0x52, 0x18, 0x01]);
    }

    #[test]
    fn create_setlist_utf8_wire_bytes_are_exact_and_sparse() {
        let message = create_setlist("Тур3".into());
        assert_eq!(message.message_type, profile::MESSAGE_TYPE_FILE);
        assert_eq!(
            message.payload,
            [
                0x18, 0x00, 0x22, 0x39, 0x0a, 0x19, 0x2f, 0x6d, 0x65, 0x64, 0x69, 0x61, 0x2f, 0x70,
                0x34, 0x2f, 0x50, 0x72, 0x65, 0x73, 0x65, 0x74, 0x73, 0x2f, 0xd0, 0xa2, 0xd1, 0x83,
                0xd1, 0x80, 0x33, 0x12, 0x11, 0x2f, 0x6d, 0x65, 0x64, 0x69, 0x61, 0x2f, 0x70, 0x34,
                0x2f, 0x50, 0x72, 0x65, 0x73, 0x65, 0x74, 0x73, 0x1a, 0x07, 0xd0, 0xa2, 0xd1, 0x83,
                0xd1, 0x80, 0x33, 0x20, 0x00,
            ]
        );
        let decoded = pa::FileMessage::decode(message.payload.as_slice()).unwrap();
        assert_eq!(decoded.action, pa::message_action::Enum::Create as i32);
        assert!(decoded.request_id.is_none());
        assert!(decoded.to_folder.is_none());
    }

    #[test]
    fn delete_setlist_has_exact_expected_bytes_and_checked_name_boundary() {
        let encoded = DeviceOperation::DeleteSetlist {
            name: "Tour".into(),
        }
        .try_encode()
        .expect("valid user setlist deletion");
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].message_type, profile::MESSAGE_TYPE_FILE);
        assert_eq!(
            encoded[0].payload,
            b"\x08\x02\x18\x00\x22\x1a\x0a\x16/media/p4/Presets/Tour\x20\x00\x50\x00"
        );

        for name in [
            "",
            " My Setlist",
            "My Setlist ",
            ".",
            "..",
            "../Factory",
            "Child/Grandchild",
            "Child\\Grandchild",
            "My Presets",
            "my presets",
            "line\nbreak",
        ] {
            assert!(matches!(
                DeviceOperation::DeleteSetlist { name: name.into() }.try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "setlist name",
                    ..
                })
            ));
        }
        assert!(matches!(
            DeviceOperation::DeleteSetlist {
                name: "x".repeat(65),
            }
            .try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "setlist name",
                ..
            })
        ));
    }

    #[test]
    fn capture_add_is_atomic_and_ir_text_updates_remain_ordered() {
        fn text_update(message: &OutboundMessage) -> (u32, String) {
            assert_eq!(message.message_type, 1);
            let grid = pa::GridMessage::decode(message.payload.as_slice()).unwrap();
            let pa::grid_message::Preset::Preset(preset) = grid.preset.unwrap();
            let parameter = &preset.chains[0].models[0].params[0];
            let value = parameter.param_values[0].value.as_ref().unwrap();
            let param_value::Value::StringValue(value) = value else {
                panic!("expected text parameter update")
            };
            let Some(param::Index::Index(index)) = parameter.index else {
                panic!("expected parameter index")
            };
            (index, value.clone())
        }

        let capture = DeviceOperation::LoadCapture {
            row: 1,
            column: 2,
            key: "capture/".into(),
            model_id: Some(14_000),
        }
        .try_encode()
        .unwrap();
        assert_eq!(capture.len(), 1);
        let grid = pa::GridMessage::decode(capture[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(model) = grid.preset.unwrap();
        assert_eq!(
            model.chains[0].models[0].hash,
            Some(model::Hash::Hash(14_000))
        );
        assert_eq!(text_update(&capture[0]), (5, "capture/".into()));

        let retarget = DeviceOperation::LoadCapture {
            row: 1,
            column: 2,
            key: "capture/Crunch".into(),
            model_id: None,
        };
        assert!(matches!(
            retarget.try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "capture model ID",
                ..
            })
        ));

        let ir = DeviceOperation::LoadIr {
            row: 2,
            column: 3,
            key: "ir/key".into(),
            slot: 1,
        }
        .try_encode()
        .unwrap();
        assert_eq!(ir.len(), 1);
        assert_eq!(text_update(&ir[0]), (10, "ir/key".into()));

        let first_slot = DeviceOperation::LoadIr {
            row: 2,
            column: 3,
            key: "factory_ir_root/room".into(),
            slot: 0,
        }
        .try_encode()
        .unwrap();
        assert_eq!(first_slot.len(), 1);
        assert_eq!(
            text_update(&first_slot[0]),
            (2, "factory_ir_root/room".into())
        );
    }

    #[test]
    fn checked_operation_encoding_rejects_invalid_dispatch_and_signed_overflow() {
        let invalid_setting = DeviceOperation::SetGeneralInteger {
            setting: "notASetting".into(),
            value: 1,
        };
        assert!(matches!(
            invalid_setting.try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "GeneralSettings integer",
                ..
            })
        ));
        for (setting, value) in [
            ("screenBrightness", 0),
            ("screenBrightness", 33),
            ("ledBrightness", 33),
            ("dimmedLedBrightness", 32),
            ("holdTiming", 6),
            ("midiChannel", 0),
            ("midiChannel", 18),
            ("midiClockOut", -1),
            ("midiClockOut", 4),
        ] {
            assert!(DeviceOperation::SetGeneralInteger {
                setting: setting.into(),
                value,
            }
            .try_encode()
            .is_err());
        }
        for (setting, value) in [
            ("screenBrightness", 1),
            ("ledBrightness", 0),
            ("dimmedLedBrightness", 31),
            ("holdTiming", 5),
            ("midiChannel", 17),
            ("midiClockOut", 0),
            ("midiClockOut", 3),
        ] {
            assert!(DeviceOperation::SetGeneralInteger {
                setting: setting.into(),
                value,
            }
            .try_encode()
            .is_ok());
        }

        let invalid_control = DeviceOperation::SetLaneControlParameter {
            row: 0,
            control: "sidechain".into(),
            parameter_index: 0,
            value: 0.5,
        };
        assert!(matches!(
            invalid_control.try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "lane control",
                ..
            })
        ));

        let overflowing_position = DeviceOperation::MovePreset {
            setlist_key: "/media/p4/Presets/Live".into(),
            product_key: "device-product-key-42".into(),
            position: i32::MAX as u32 + 1,
        };
        assert_eq!(
            overflowing_position.try_encode(),
            Err(CommandEncodeError::SignedFieldOverflow { field: "position" })
        );
        let overflowing_swap_position = DeviceOperation::SwapPreset {
            setlist_key: "/media/p4/Presets/Live".into(),
            product_key: "device-product-key-42".into(),
            position: i32::MAX as u32 + 1,
        };
        assert_eq!(
            overflowing_swap_position.try_encode(),
            Err(CommandEncodeError::SignedFieldOverflow { field: "position" })
        );
        let overflowing_copy_position = DeviceOperation::CopyPreset {
            source_setlist_key: "/media/p4/Presets/Live".into(),
            source_product_key: "/media/p4/Presets/Live/Stage.pb".into(),
            source_is_factory: false,
            destination_setlist_key: "/media/p4/Presets/Other".into(),
            destination_position: i32::MAX as u32 + 1,
        };
        assert_eq!(
            overflowing_copy_position.try_encode(),
            Err(CommandEncodeError::SignedFieldOverflow { field: "position" })
        );

        for operation in [
            DeviceOperation::RemoveBlock { row: 4, column: 0 },
            DeviceOperation::RemoveBlock { row: 0, column: 8 },
            DeviceOperation::SetExpressionBypass {
                row: 0,
                column: 8,
                pedal: 1,
                mode: 0,
                invert: false,
                delay_ms: 0,
                latch_emulation: false,
            },
        ] {
            assert!(matches!(
                operation.try_encode(),
                Err(CommandEncodeError::UnsupportedValue { .. })
            ));
        }

        assert!(matches!(
            DeviceOperation::SetLaneControlExpression {
                row: 0,
                control: "sidechain".into(),
                parameter_index: 0,
                pedal: 1,
                minimum: 0.0,
                maximum: 1.0,
            }
            .try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "lane control",
                ..
            })
        ));
    }

    /// `[MANUAL 7.0]` describes MIDI Clock Out as an on/off toggle for
    /// sending MIDI clock over USB and/or TRS MIDI. The field decodes (`responses.rs`'s
    /// `decode_general_settings`) but, before this test, had no write path
    /// anywhere: it is a 4-value enum (`MidiClockOut.Enum`,
    /// `message MidiClockOut` in `proto/ProductionAutomation.proto`), not a boolean, so it
    /// fell through both the boolean toggle whitelist and the integer
    /// whitelist. It belongs on the integer path alongside `midiChannel`.
    #[test]
    fn set_general_integer_supports_midi_clock_out_enum_values() {
        for value in 0..=3 {
            let frames = DeviceOperation::SetGeneralInteger {
                setting: "midiClockOut".into(),
                value,
            }
            .try_encode()
            .unwrap();
            let decoded = pa::GeneralSettingsMessage::decode(frames[0].payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
            assert_eq!(
                decoded.midi_clock_out,
                Some(pa::general_settings_message::MidiClockOut::MidiClockOut(
                    value
                ))
            );
            assert!(decoded.midi_channel.is_none(), "writes must stay sparse");
        }
    }

    /// Regression: validate_for_encoding's SetGeneralToggle whitelist had 9
    /// names while set_general_toggle()'s encoder supports 12, so
    /// lockScreenAndVolumeKnob/sceneDimmed/stompDimmed -- all real, encoder-
    /// supported settings -- were permanently rejected by try_encode().
    #[test]
    fn set_general_toggle_whitelist_admits_lock_scene_and_stomp_dimming() {
        for (setting, enabled) in [
            ("lockScreenAndVolumeKnob", true),
            ("sceneDimmed", true),
            ("stompDimmed", false),
        ] {
            let frames = DeviceOperation::SetGeneralToggle {
                setting: setting.into(),
                enabled,
            }
            .try_encode()
            .unwrap_or_else(|error| panic!("{setting} must be a valid toggle: {error:?}"));
            let decoded = pa::GeneralSettingsMessage::decode(frames[0].payload.as_slice()).unwrap();
            assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
        }
        assert_eq!(
            DeviceOperation::SetGeneralToggle {
                setting: "lockScreenAndVolumeKnob".into(),
                enabled: true,
            }
            .try_encode()
            .unwrap(),
            DeviceOperation::SetGeneralToggle {
                setting: "lockScreenAndVolumeKnob".into(),
                enabled: true,
            }
            .encode_unchecked()
            .unwrap()
        );
    }

    #[test]
    fn checked_operation_encoding_preserves_valid_wire_bytes() {
        let operation = DeviceOperation::SetGeneralToggle {
            setting: "midiOverUsb".into(),
            enabled: true,
        };
        assert_eq!(
            operation.clone().try_encode().unwrap(),
            operation.encode_unchecked().unwrap()
        );
    }

    #[test]
    fn public_sparse_selector_builders_return_typed_errors() {
        let expected = |field: &'static str, value: &str| CommandEncodeError::UnsupportedValue {
            field,
            value: value.to_owned(),
        };

        assert_eq!(
            set_parameter_scene_mode(0, 10, 0, true),
            Err(expected("parameter target column", "10"))
        );
        assert_eq!(
            set_parameter_expression(0, 10, 0, 1, 0.0, 1.0),
            Err(expected("parameter target column", "10"))
        );
        assert_eq!(
            set_lane_control_parameter(0, "invalid", 0, 0.0),
            Err(expected("lane control", "invalid"))
        );
        assert_eq!(
            set_lane_control_scene_mode(0, "invalid", 0, true),
            Err(expected("lane control", "invalid"))
        );
        assert_eq!(
            set_lane_control_expression(0, "invalid", 0, 1, 0.0, 1.0),
            Err(expected("lane control", "invalid"))
        );
        assert_eq!(
            set_routing_parameter(0, "invalid", 0, 0.0),
            Err(expected("routing node", "invalid"))
        );
        assert_eq!(
            set_general_integer("invalid", 7),
            Err(expected("GeneralSettings integer", "invalid=7"))
        );
        assert_eq!(
            set_general_toggle("invalid", true),
            Err(expected("GeneralSettings toggle", "invalid"))
        );
    }

    #[test]
    fn checked_device_commands_reject_invalid_coordinates_before_encoding() {
        for command in [
            DeviceCommand::SelectScene(domain::SCENE_COUNT),
            DeviceCommand::SetBypass {
                row: domain::GRID_ROWS,
                column: 0,
                bypassed: true,
            },
            DeviceCommand::SetBypass {
                row: 0,
                column: domain::GRID_COLUMNS,
                bypassed: true,
            },
            DeviceCommand::SetParameterNumeric {
                row: 0,
                column: domain::GRID_COLUMNS + 2,
                parameter_index: 0,
                value: 0.5,
            },
        ] {
            assert!(matches!(
                command.try_encode(),
                Err(CommandEncodeError::UnsupportedValue { .. })
            ));
        }
        assert!(matches!(
            DeviceCommand::SetlistPosition {
                setlist_key: "test".into(),
                position: 256,
                is_factory: false,
            }
            .try_encode(),
            Err(CommandEncodeError::UnsupportedValue {
                field: "position",
                ..
            })
        ));
    }

    #[test]
    fn checked_parameter_encoders_reject_non_normalized_values() {
        for value in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            assert!(matches!(
                DeviceCommand::SetParameterNumeric {
                    row: 0,
                    column: 7,
                    parameter_index: u32::MAX,
                    value,
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "normalized parameter value",
                    ..
                })
            ));
            assert!(matches!(
                DeviceOperation::SetRoutingParameter {
                    row: 2,
                    node: "mixer".into(),
                    parameter_index: u32::MAX,
                    value,
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "normalized routing parameter value",
                    ..
                })
            ));
        }

        assert!(DeviceCommand::SetParameterNumeric {
            row: 3,
            column: 7,
            parameter_index: u32::MAX,
            value: 1.0,
        }
        .try_encode()
        .is_ok());
    }

    /// Regression: the bound check let column 8/9 (splitter/mixer) through
    /// SetParameterNumeric/SetParameterText, but parameter_update()
    /// unconditionally builds a normal Model at `chain.models[column]` for
    /// any validated column, silently writing to a nonexistent grid column
    /// instead of the splitter/mixer that device.setParameter's own
    /// `column >= GRID_COLUMNS` branch in request.rs routes those columns to
    /// via the dedicated DeviceOperation::SetRoutingParameter. Splitter/mixer
    /// parameters have no addressable Model-grid representation, so both
    /// commands must now reject columns 8/9 instead of miswriting them.
    #[test]
    fn set_parameter_numeric_and_text_reject_splitter_and_mixer_columns() {
        for column in [domain::GRID_COLUMNS, domain::GRID_COLUMNS + 1] {
            assert!(matches!(
                DeviceCommand::SetParameterNumeric {
                    row: 0,
                    column,
                    parameter_index: 0,
                    value: 0.5,
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "parameter target column",
                    ..
                })
            ));
            assert!(matches!(
                DeviceCommand::SetParameterText {
                    row: 0,
                    column,
                    parameter_index: 0,
                    value: "text".into(),
                }
                .try_encode(),
                Err(CommandEncodeError::UnsupportedValue {
                    field: "parameter target column",
                    ..
                })
            ));
        }
        // The real Model grid columns (0..GRID_COLUMNS) remain unaffected.
        assert!(DeviceCommand::SetParameterNumeric {
            row: 0,
            column: domain::GRID_COLUMNS - 1,
            parameter_index: 0,
            value: 0.5,
        }
        .try_encode()
        .is_ok());
    }

    #[test]
    fn checked_lane_control_parameter_encoder_rejects_invalid_targets_and_values() {
        for operation in [
            DeviceOperation::SetLaneControlParameter {
                row: 4,
                control: "inputGate".into(),
                parameter_index: 0,
                value: 0.5,
            },
            DeviceOperation::SetLaneControlParameter {
                row: 0,
                control: "laneOutput".into(),
                parameter_index: 0,
                value: f32::NAN,
            },
            DeviceOperation::SetLaneControlParameter {
                row: 0,
                control: "laneOutput".into(),
                parameter_index: 0,
                value: 1.01,
            },
        ] {
            assert!(matches!(
                operation.try_encode(),
                Err(CommandEncodeError::UnsupportedValue { .. })
            ));
        }

        assert!(DeviceOperation::SetLaneControlParameter {
            row: 3,
            control: "inputGate".into(),
            parameter_index: u32::MAX,
            value: 1.0,
        }
        .try_encode()
        .is_ok());
    }

    #[test]
    fn checked_lane_control_expression_encoder_rejects_invalid_assignments() {
        for operation in [
            DeviceOperation::SetLaneControlExpression {
                row: 0,
                control: "inputGate".into(),
                parameter_index: 0,
                pedal: 3,
                minimum: 0.0,
                maximum: 1.0,
            },
            DeviceOperation::SetLaneControlExpression {
                row: 0,
                control: "laneOutput".into(),
                parameter_index: 0,
                pedal: 1,
                minimum: f32::NAN,
                maximum: 1.0,
            },
            DeviceOperation::SetLaneControlExpression {
                row: 0,
                control: "laneOutput".into(),
                parameter_index: 0,
                pedal: 1,
                minimum: 0.0,
                maximum: 1.01,
            },
        ] {
            assert!(matches!(
                operation.try_encode(),
                Err(CommandEncodeError::UnsupportedValue { .. })
            ));
        }

        assert!(DeviceOperation::SetLaneControlExpression {
            row: 3,
            control: "inputGate".into(),
            parameter_index: u32::MAX,
            pedal: 0,
            minimum: 1.0,
            maximum: 0.0,
        }
        .try_encode()
        .is_ok());
    }

    /// Hold every published unit transform against the code that ships it.
    ///
    /// The output-level and brightness defects were one failure twice: a
    /// setter's conversion was hand-written, one was forgotten and one was
    /// written for the wrong domain, and nothing compared either against the
    /// workflow contract that already published the arithmetic. So the
    /// contracts are read here, every transform they state as arithmetic is
    /// evaluated, and each one must either reproduce the shipped conversion
    /// across its whole published domain or be listed with the reason no
    /// conversion exists to compare. A new declaration nobody wired up fails
    /// `every_published_transform_is_accounted_for`; a control that claims to
    /// need no conversion while its physical domain is not the wire's fails
    /// `a_physical_domain_unlike_the_wires_may_not_claim_identity`, which is
    /// the shape the output level had.
    mod published_units {
        use super::*;
        use serde_json::Value;

        // Stable public keys for the self-contained compatibility snapshot.
        // They intentionally describe behavior rather than repository-internal
        // source filenames.
        const INPUT_PORT: &str = "input-port";
        const OUTPUT_PORT: &str = "output-port";
        const USB_PORT: &str = "usb-port";
        const GLOBAL_EQ_OUTPUT: &str = "global-eq-output";
        const GENERAL_INTEGER: &str = "general-integer";
        const GENERAL_SETTINGS_READ: &str = "general-settings-read";

        /// The contracts that publish a unit for a value this crate encodes.
        const CONTRACTS: &[&str] = &[
            INPUT_PORT,
            OUTPUT_PORT,
            USB_PORT,
            GLOBAL_EQ_OUTPUT,
            GENERAL_INTEGER,
            GENERAL_SETTINGS_READ,
        ];

        /// Every key under which a supported contract states a conversion. A
        /// new spelling has to be added here to be checked, which is why the
        /// list is asserted against the contracts in
        /// `no_contract_states_a_conversion_under_an_unknown_key`.
        const CONVERSION_KEYS: &[&str] = &[
            "transform",
            "physicalToWire",
            "wireToPhysical",
            "physicalToNormalized",
            "normalizedToPhysical",
            "projection",
            "publicToWire",
            "wireToPublic",
            "toNormalized",
        ];

        /// Keys under which a contract states the physical domain of a control.
        const PHYSICAL_DOMAIN_KEYS: &[&str] =
            &["physicalDomain", "physicalRange", "physicalRangeDb"];

        /// Keys naming the control an array element describes.
        const LABEL_KEYS: &[&str] = &["modelKey", "publicField", "public", "property"];

        // -------------------------------------------------------------------
        // The arithmetic a contract is allowed to publish
        // -------------------------------------------------------------------

        /// Evaluate one published expression for one sample of its input.
        ///
        /// The grammar is deliberately small: decimals, `+ - * /`, parentheses,
        /// the `round`, `trunc` and two-argument `min` the projections use, and a single named
        /// value - `levelDb`, `wire`, `step`, `value` all mean the sample. Prose
        /// such as `"off=0, on=1"` or `"enum 0..2"` fails to parse, which is how
        /// a contract says its mapping is not arithmetic at all.
        struct Published<'a> {
            bytes: &'a [u8],
            at: usize,
            sample: f64,
        }

        impl<'a> Published<'a> {
            fn evaluate(text: &'a str, sample: f64) -> Result<f64, String> {
                let mut parser = Self {
                    bytes: text.as_bytes(),
                    at: 0,
                    sample,
                };
                let value = parser.sum()?;
                parser.spaces();
                if parser.at != parser.bytes.len() {
                    return Err(format!("{text:?} is prose, not arithmetic"));
                }
                Ok(value)
            }

            /// Whether the expression is a bare name - `"identity"`, `"direct"`,
            /// `"enumIndex"` - which states that nothing is converted.
            fn is_identity(text: &str) -> bool {
                !text.is_empty()
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    && !text
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_digit())
            }

            fn spaces(&mut self) {
                while self.bytes.get(self.at) == Some(&b' ') {
                    self.at += 1;
                }
            }

            fn eat(&mut self, byte: u8) -> bool {
                self.spaces();
                if self.bytes.get(self.at) == Some(&byte) {
                    self.at += 1;
                    true
                } else {
                    false
                }
            }

            fn sum(&mut self) -> Result<f64, String> {
                let mut total = self.product()?;
                loop {
                    if self.eat(b'+') {
                        total += self.product()?;
                    } else if self.eat(b'-') {
                        total -= self.product()?;
                    } else {
                        return Ok(total);
                    }
                }
            }

            fn product(&mut self) -> Result<f64, String> {
                let mut total = self.unary()?;
                loop {
                    if self.eat(b'*') {
                        total *= self.unary()?;
                    } else if self.eat(b'/') {
                        let divisor = self.unary()?;
                        if divisor == 0.0 {
                            return Err("a published transform divides by zero".into());
                        }
                        total /= divisor;
                    } else {
                        return Ok(total);
                    }
                }
            }

            fn unary(&mut self) -> Result<f64, String> {
                if self.eat(b'-') {
                    return Ok(-self.unary()?);
                }
                self.primary()
            }

            fn primary(&mut self) -> Result<f64, String> {
                self.spaces();
                if self.eat(b'(') {
                    let inner = self.sum()?;
                    if !self.eat(b')') {
                        return Err("unbalanced parentheses".into());
                    }
                    return Ok(inner);
                }
                let start = self.at;
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
                {
                    self.at += 1;
                }
                if self.at > start {
                    return self.slice(start).parse::<f64>().map_err(|error| {
                        format!("{} is not a number: {error}", self.slice(start))
                    });
                }
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    self.at += 1;
                }
                if self.at == start {
                    return Err("not arithmetic".into());
                }
                let name = self.slice(start).to_string();
                if self.eat(b'(') {
                    let first = self.sum()?;
                    let second = self.eat(b',').then(|| self.sum()).transpose()?;
                    if !self.eat(b')') {
                        return Err("unbalanced parentheses".into());
                    }
                    return match (name.as_str(), second) {
                        ("round", None) => Ok(first.round()),
                        ("trunc", None) => Ok(first.trunc()),
                        ("min", Some(second)) => Ok(first.min(second)),
                        other => Err(format!("{other:?} is not a published arithmetic helper")),
                    };
                }
                Ok(self.sample)
            }

            fn slice(&self, start: usize) -> &str {
                std::str::from_utf8(&self.bytes[start..self.at]).expect("contracts are UTF-8")
            }
        }

        // -------------------------------------------------------------------
        // Reading the contracts
        // -------------------------------------------------------------------

        fn contract(name: &str) -> Value {
            let fixture: Value = serde_json::from_str(include_str!(
                "../tests/fixtures/published-unit-contracts.json"
            ))
            .expect("the packaged published-unit fixture is valid JSON");
            fixture["contracts"][name].clone()
        }

        fn label(element: &Value, index: usize) -> String {
            LABEL_KEYS
                .iter()
                .find_map(|key| element.get(key).and_then(Value::as_str))
                .map_or_else(|| index.to_string(), str::to_string)
        }

        /// Collect every `(path, expression)` a contract states a conversion as.
        fn conversions(node: &Value, path: &str, found: &mut Vec<(String, String)>) {
            match node {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        let here = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}/{key}")
                        };
                        if CONVERSION_KEYS.contains(&key.as_str()) {
                            if let Some(text) = value.as_str() {
                                found.push((here.clone(), text.to_string()));
                            }
                        }
                        conversions(value, &here, found);
                    }
                }
                Value::Array(elements) => {
                    for (index, element) in elements.iter().enumerate() {
                        conversions(
                            element,
                            &format!("{path}[{}]", label(element, index)),
                            found,
                        );
                    }
                }
                _ => {}
            }
        }

        fn stated(name: &str) -> Vec<(String, String)> {
            let mut found = Vec::new();
            conversions(&contract(name), "", &mut found);
            found
        }

        // -------------------------------------------------------------------
        // What this crate actually does with each value
        // -------------------------------------------------------------------

        fn shipped_output_level_to_wire(level_db: f64) -> f64 {
            f64::from(sole_out_port_level(&set_output_port(
                1,
                Some(level_db as f32),
                None,
                None,
            )))
        }

        fn shipped_output_level_to_db(wire: f64) -> f64 {
            f64::from(output_level_wire_to_db(wire as f32))
        }

        fn shipped_input_level_to_wire(level_db: f64) -> f64 {
            f64::from(input_level_db_to_wire(level_db))
        }

        /// The read side of the input level, driven through the real decoder.
        fn shipped_input_level_to_db(wire: f64) -> f64 {
            let message = pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                settings: Some(pa::io_settings_message::Settings::Settings(
                    pa::PortSettings {
                        in_port: vec![pa::InputPortSettings {
                            input_port_id: 1,
                            level: Some(pa::input_port_settings::Level::Level(wire as f32)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                )),
                ..Default::default()
            };
            let settings = crate::responses::decode_io_settings(&message.encode_to_vec())
                .expect("the I/O decoder accepts a sparse input-level update");
            f64::from(
                settings.inputs[0]
                    .level_db
                    .expect("the decoder projects levelDb"),
            )
        }

        fn shipped_brightness_to_wire(setting: &str, step: f64) -> f64 {
            f64::from(
                general_integer_wire(setting, step as i32)
                    .expect("the brightness frame carries its field"),
            )
        }

        /// The read side of a brightness, driven through the real decoder.
        fn shipped_brightness_from_wire(setting: &str, wire: f64) -> f64 {
            let wire = wire as i32;
            let mut message = pa::GeneralSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                ..Default::default()
            };
            match setting {
                "screenBrightness" => {
                    message.screen_brightness =
                        Some(pa::general_settings_message::ScreenBrightness::ScreenBrightness(wire))
                }
                "ledBrightness" => {
                    message.led_brightness = Some(
                        pa::general_settings_message::LedBrightness::LedBrightness(wire),
                    )
                }
                _ => {
                    message.dimmed_led_brightness = Some(
                        pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(
                            wire,
                        ),
                    )
                }
            }
            let settings = crate::responses::decode_general_settings(&message.encode_to_vec())
                .expect("the General Settings decoder accepts a sparse brightness update");
            let step = match setting {
                "screenBrightness" => settings.screen_brightness,
                "ledBrightness" => settings.led_brightness,
                _ => settings.dimmed_led_brightness,
            };
            f64::from(step.expect("the decoder projects the brightness it was given"))
        }

        fn screen_to_wire(step: f64) -> f64 {
            shipped_brightness_to_wire("screenBrightness", step)
        }
        fn led_to_wire(step: f64) -> f64 {
            shipped_brightness_to_wire("ledBrightness", step)
        }
        fn dimmed_to_wire(step: f64) -> f64 {
            shipped_brightness_to_wire("dimmedLedBrightness", step)
        }
        fn screen_from_wire(wire: f64) -> f64 {
            shipped_brightness_from_wire("screenBrightness", wire)
        }
        fn led_from_wire(wire: f64) -> f64 {
            shipped_brightness_from_wire("ledBrightness", wire)
        }
        fn dimmed_from_wire(wire: f64) -> f64 {
            shipped_brightness_from_wire("dimmedLedBrightness", wire)
        }

        // -------------------------------------------------------------------
        // The register
        // -------------------------------------------------------------------

        enum Shipped {
            /// This crate converts, and the conversion is sampled across the
            /// published domain `from..=to` in `steps` even steps. `tolerance`
            /// is 0 where both sides are exact integers and a few f32 epsilons
            /// of the widest term where the shipped side computes in f32.
            Converts {
                apply: fn(f64) -> f64,
                from: f64,
                to: f64,
                steps: u32,
                tolerance: f64,
            },
            /// Nothing converts, and this is why.
            Nothing(&'static str),
        }

        struct Register {
            contract: &'static str,
            path: &'static str,
            shipped: Shipped,
        }

        const REGISTER: &[Register] = &[
            Register {
                contract: OUTPUT_PORT,
                path: "controls/level/physicalToWire",
                shipped: Shipped::Converts {
                    apply: shipped_output_level_to_wire,
                    from: -40.0,
                    to: 12.0,
                    steps: 104,
                    tolerance: 1e-6,
                },
            },
            Register {
                contract: OUTPUT_PORT,
                path: "controls/level/wireToPhysical",
                shipped: Shipped::Converts {
                    apply: shipped_output_level_to_db,
                    from: 0.0,
                    to: 1.0,
                    steps: 100,
                    tolerance: 1e-5,
                },
            },
            Register {
                contract: INPUT_PORT,
                path: "controls[level]/physicalToWire",
                shipped: Shipped::Converts {
                    apply: shipped_input_level_to_wire,
                    from: -12.0,
                    to: 60.0,
                    steps: 144,
                    tolerance: 1e-6,
                },
            },
            Register {
                contract: INPUT_PORT,
                path: "controls[level]/wireToPhysical",
                shipped: Shipped::Converts {
                    apply: shipped_input_level_to_db,
                    from: 0.0,
                    to: 1.0,
                    steps: 100,
                    tolerance: 1e-5,
                },
            },
            Register {
                contract: GENERAL_INTEGER,
                path: "settings/screenBrightness/transform",
                shipped: Shipped::Converts {
                    apply: screen_to_wire,
                    from: 1.0,
                    to: 32.0,
                    steps: 31,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_INTEGER,
                path: "settings/ledBrightness/transform",
                shipped: Shipped::Converts {
                    apply: led_to_wire,
                    from: 0.0,
                    to: 32.0,
                    steps: 32,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_INTEGER,
                path: "settings/dimmedLedBrightness/transform",
                shipped: Shipped::Converts {
                    apply: dimmed_to_wire,
                    from: 0.0,
                    to: 31.0,
                    steps: 31,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_INTEGER,
                path: "settings/screenBrightness/wireToPublic",
                shipped: Shipped::Converts {
                    apply: screen_from_wire,
                    from: 0.0,
                    to: 100.0,
                    steps: 100,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_INTEGER,
                path: "settings/ledBrightness/wireToPublic",
                shipped: Shipped::Converts {
                    apply: led_from_wire,
                    from: 0.0,
                    to: 100.0,
                    steps: 100,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_INTEGER,
                path: "settings/dimmedLedBrightness/wireToPublic",
                shipped: Shipped::Converts {
                    apply: dimmed_from_wire,
                    from: 0.0,
                    to: 100.0,
                    steps: 100,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_SETTINGS_READ,
                path: "publicProjection[screenBrightness]/projection",
                shipped: Shipped::Converts {
                    apply: screen_from_wire,
                    from: 0.0,
                    to: 100.0,
                    steps: 100,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_SETTINGS_READ,
                path: "publicProjection[ledBrightness]/projection",
                shipped: Shipped::Converts {
                    apply: led_from_wire,
                    from: 0.0,
                    to: 100.0,
                    steps: 100,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: GENERAL_SETTINGS_READ,
                path: "publicProjection[dimmedLedBrightness]/projection",
                shipped: Shipped::Converts {
                    apply: dimmed_from_wire,
                    from: 0.0,
                    to: 100.0,
                    steps: 100,
                    tolerance: 0.0,
                },
            },
            Register {
                contract: INPUT_PORT,
                path: "controls[inputZmode]/physicalToWire",
                shipped: Shipped::Nothing(
                    "the public impedance argument is the wire value itself, checked against the \
                     contract's own wireValues by is_input_impedance_value, so the enum ordinal \
                     never appears in this crate",
                ),
            },
            Register {
                contract: INPUT_PORT,
                path: "controls[inputType]/physicalToWire",
                shipped: Shipped::Nothing(
                    "the public inputType argument is the wire value itself, checked by \
                     is_input_type_value",
                ),
            },
            Register {
                contract: USB_PORT,
                path: "controls[select]/physicalToWire",
                shipped: Shipped::Nothing(
                    "the public headphonesSource argument is the wire value itself, checked by \
                     is_usb_headphones_source_value",
                ),
            },
            Register {
                contract: USB_PORT,
                path: "controls[level]/physicalToWire",
                shipped: Shipped::Nothing(
                    "set_usb_port takes the normalized level, not decibels: the public domain is \
                     0..1 and the dB range here describes the desktop USB LEVEL knob. Giving that \
                     action a physical argument would need this transform applied",
                ),
            },
            Register {
                contract: USB_PORT,
                path: "controls[level]/wireToPhysical",
                shipped: Shipped::Nothing(
                    "device.ioSettings reports usb.level as the normalized wire value with no dB \
                     companion, so there is no projection here to compare",
                ),
            },
            Register {
                contract: GLOBAL_EQ_OUTPUT,
                path: "uiWorkflow/controls[level]/physicalToNormalized",
                shipped: Shipped::Nothing(
                    "the global-EQ output level is converted in quadcortex::device's request layer, \
                     inline in the setGlobalEqOutput arm rather than beside this frame builder, so \
                     it cannot be driven from this crate. Moving it here would bring it under this \
                     check",
                ),
            },
        ];

        fn registered(contract: &str, path: &str) -> Option<&'static Register> {
            REGISTER
                .iter()
                .find(|entry| entry.contract == contract && entry.path == path)
        }

        // -------------------------------------------------------------------
        // The checks
        // -------------------------------------------------------------------

        #[test]
        fn published_transforms_reproduce_the_shipped_conversions() {
            for entry in REGISTER {
                let published = stated(entry.contract)
                    .into_iter()
                    .find(|(path, _)| path == entry.path)
                    .unwrap_or_else(|| {
                        panic!(
                            "{} no longer publishes {} - a registered conversion lost its \
                             declaration",
                            entry.contract, entry.path
                        )
                    })
                    .1;
                let Shipped::Converts {
                    apply,
                    from,
                    to,
                    steps,
                    tolerance,
                } = entry.shipped
                else {
                    let Shipped::Nothing(reason) = entry.shipped else {
                        unreachable!()
                    };
                    assert!(
                        reason.len() > 40,
                        "{} {} is registered as needing no conversion without saying why",
                        entry.contract,
                        entry.path
                    );
                    continue;
                };
                assert!(
                    !Published::is_identity(&published),
                    "{} {} states {published:?} yet the shipped code converts",
                    entry.contract,
                    entry.path,
                );
                for step in 0..=steps {
                    let sample = from + (to - from) * f64::from(step) / f64::from(steps);
                    let expected =
                        Published::evaluate(&published, sample).unwrap_or_else(|error| {
                            panic!("{} {} is unusable: {error}", entry.contract, entry.path)
                        });
                    let actual = apply(sample);
                    assert!(
                        (actual - expected).abs() <= tolerance,
                        "{} {} publishes {published:?}, which gives {expected} at {sample}, but \
                         the shipped conversion gives {actual}",
                        entry.contract,
                        entry.path
                    );
                }
            }
        }

        #[test]
        fn every_published_transform_is_accounted_for() {
            for name in CONTRACTS {
                for (path, published) in stated(name) {
                    if Published::is_identity(&published) {
                        continue;
                    }
                    if Published::evaluate(&published, 1.0).is_err() {
                        continue;
                    }
                    assert!(
                        registered(name, &path).is_some(),
                        "{name} publishes the transform {published:?} at {path} and nothing holds \
                         the implementation to it. Register it with the conversion this crate \
                         applies, or with the reason none exists"
                    );
                }
            }
        }

        #[test]
        fn a_physical_domain_unlike_the_wires_may_not_claim_identity() {
            for name in CONTRACTS {
                let contract = contract(name);
                let mut controls = Vec::new();
                collect_controls(&contract, "", &mut controls);
                for (path, control) in controls {
                    let Some((low, high)) = physical_bounds(&control) else {
                        continue;
                    };
                    if (low, high) == (0.0, 1.0) {
                        continue;
                    }
                    let converts = CONVERSION_KEYS.iter().any(|key| {
                        control
                            .get(key)
                            .and_then(Value::as_str)
                            .is_some_and(|text| {
                                !Published::is_identity(text)
                                    && Published::evaluate(text, 1.0).is_ok()
                            })
                    });
                    assert!(
                        converts,
                        "{name} {path} spans {low}..{high} while its wire field is normalized, so \
                         it cannot be carried unconverted. Publish the transform it needs"
                    );
                }
            }
        }

        #[test]
        fn no_contract_states_a_conversion_under_an_unknown_key() {
            // The spellings above were collected by hand; this fails if a
            // contract grows another one, so the walker cannot go quietly blind.
            let mut seen = Vec::new();
            for name in CONTRACTS {
                collect_keys(&contract(name), &mut seen);
            }
            let unknown: Vec<&String> = seen
                .iter()
                .filter(|key| {
                    let lowered = key.to_lowercase();
                    (lowered.contains("towire")
                        || lowered.contains("tophysical")
                        || lowered.contains("tonormalized")
                        || lowered.contains("topublic")
                        || lowered == "transform"
                        || lowered == "projection")
                        && !CONVERSION_KEYS.contains(&key.as_str())
                })
                .collect();
            assert!(unknown.is_empty(), "unchecked conversion keys: {unknown:?}");
        }

        fn collect_controls(node: &Value, path: &str, found: &mut Vec<(String, Value)>) {
            match node {
                Value::Object(fields) => {
                    if PHYSICAL_DOMAIN_KEYS
                        .iter()
                        .any(|key| fields.contains_key(*key))
                    {
                        found.push((path.to_string(), node.clone()));
                    }
                    for (key, value) in fields {
                        let here = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}/{key}")
                        };
                        collect_controls(value, &here, found);
                    }
                }
                Value::Array(elements) => {
                    for (index, element) in elements.iter().enumerate() {
                        collect_controls(
                            element,
                            &format!("{path}[{}]", label(element, index)),
                            found,
                        );
                    }
                }
                _ => {}
            }
        }

        fn physical_bounds(control: &Value) -> Option<(f64, f64)> {
            let domain = PHYSICAL_DOMAIN_KEYS
                .iter()
                .find_map(|key| control.get(key))?;
            let pair = match domain {
                Value::Object(fields) => (fields.get("min")?, fields.get("max")?),
                Value::Array(values) if values.len() == 2 => (&values[0], &values[1]),
                _ => return None,
            };
            Some((pair.0.as_f64()?, pair.1.as_f64()?))
        }

        fn collect_keys(node: &Value, found: &mut Vec<String>) {
            match node {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        found.push(key.clone());
                        collect_keys(value, found);
                    }
                }
                Value::Array(elements) => {
                    for element in elements {
                        collect_keys(element, found);
                    }
                }
                _ => {}
            }
        }

        #[test]
        fn the_published_grammar_reads_the_expressions_the_contracts_use_and_no_more() {
            assert_eq!(
                Published::evaluate("(levelDb + 40) / 52", 0.0).unwrap(),
                40.0 / 52.0
            );
            assert_eq!(Published::evaluate("-40 + 52 * value", 0.5).unwrap(), -14.0);
            assert_eq!(
                Published::evaluate("round(step * 100 / 32)", 10.0).unwrap(),
                31.0
            );
            assert_eq!(
                Published::evaluate("min(31, round(wire * 32 / 100))", 100.0).unwrap(),
                31.0
            );
            assert_eq!(
                Published::evaluate("1 - trunc(((wire - 1) / -100) * 32)", 59.0).unwrap(),
                19.0
            );
            for prose in [
                "off=0, on=1",
                "enum 0..2",
                "four optional row booleans",
                "index 0..5; 500 + 100 * index ms",
                "wire ordinal n displays 500 + 100*n ms",
                "assigned ? 1 : 0",
            ] {
                assert!(
                    Published::evaluate(prose, 1.0).is_err(),
                    "{prose:?} is prose and must not read as arithmetic"
                );
            }
            for identity in ["identity", "direct", "enumIndex"] {
                assert!(Published::is_identity(identity));
            }
            for stated in [
                "(levelDb + 40) / 52",
                "round(step * 100 / 32)",
                "enumIndex / 8",
            ] {
                assert!(!Published::is_identity(stated));
            }
            assert!(Published::evaluate("floor(value)", 1.0).is_err());
        }
    }
}
