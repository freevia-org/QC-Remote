// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
use serde::{Deserialize, Serialize};

/// Tuner state decoded from a tuner update: selected input, reference pitch, the mute-while-tuning preference and optional live meter fields. Returned by tuner reads and tuner setters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunerSettings {
    /// Input connector the tuner listens to. One of 1, 2, 3, 4, 5, 8 or 9, the ids offered by the tuner input choice; other values are rejected when decoding.
    pub input_port_id: i32,
    /// Reference pitch as a signed offset in Hz from A=440, from -40 through 40. For example 2.0 means a 442 Hz reference.
    pub reference_offset_hz: f32,
    /// Absolute reference pitch in Hz, computed as 440 plus `referenceOffsetHz`.
    pub reference_hz: f32,
    /// The persistent mute-while-tuning preference; when true, outputs are silenced while the tuner is engaged.
    pub muted: bool,
    /// Whether live tuner meter reports are switched on. Absent when the device omits it from the update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter: Option<f32>,
}

/// Four per-row flags for the global cab or IR bypass. Used for the cab and IR entries of the device settings and as the input of the global bypass write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalBypassRows {
    /// Global bypass flag for chain row 1; true means bypassed.
    pub row1: bool,
    /// Global bypass flag for chain row 2; true means bypassed.
    pub row2: bool,
    /// Global bypass flag for chain row 3; true means bypassed.
    pub row3: bool,
    /// Global bypass flag for chain row 4; true means bypassed.
    pub row4: bool,
}

/// Selects which outputs the master volume knob controls. Appears in the general settings and is written as a whole by the master volume assignment action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterVolumeAssignment {
    /// True when master volume applies to the Out 1/2 outputs.
    pub out12: bool,
    /// True when master volume applies to the Out 3/4 outputs.
    pub out34: bool,
    /// True when master volume applies to the Send 1/2 outputs.
    pub send12: bool,
    /// True when master volume applies to the headphone output.
    pub headphones: bool,
}

/// Device and system settings read from the unit. Every field is optional and absent when the device update did not carry it, so partial publications can be merged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    /// Screen brightness as a step from 1 through 32, derived from `screenBrightnessWire` as round(raw * 32 / 100), which matches what the unit showed (observed on a CorOS 4.1.0 unit, 2026-10-06). Written as a step; the library converts it to the wire value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_brightness: Option<i32>,
    /// LED brightness as a step from 0 through 32, derived from `ledBrightnessWire` as round(raw * 32 / 100), which matches what the unit showed (observed on a CorOS 4.1.0 unit, 2026-10-06).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub led_brightness: Option<i32>,
    /// Brightness of dimmed LEDs as a step from 0 through 31, derived from `dimmedLedBrightnessWire` and capped at 31 by this library. One raw write of 100 was echoed by the unit as 97; the scale itself was not tried for this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimmed_led_brightness: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_screen_and_volume_knob: Option<bool>,
    /// Global bypass flags, one per chain row, applied to cab blocks. Written with `set_global_bypass`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_bypass_cab: Option<GlobalBypassRows>,
    /// Global bypass flags, one per chain row, applied to IR loader blocks. Written with `set_global_bypass`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_bypass_ir: Option<GlobalBypassRows>,
    /// Global scene block-bypass persistence behavior: `alwaysOverwrite`, `nonstompOverwrite` or `neverOverwrite`. Written with `set_scene_bypass_behavior`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_bypass_behavior: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi_over_usb: Option<bool>,
    /// MIDI channel exactly as the wire carries it, 1 through 17. Values 1 through 16 are channels and 17 is shown by the unit as OMNI (observed on a CorOS 4.1.0 unit, 2026-10-06).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi_channel: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_duplicate_pc: Option<bool>,
    /// Storage space still available on the device, in the same unit as `totalDiskSpace`. Read-only; the apps show used space as total minus available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_disk_space: Option<f32>,
    /// Total storage space on the device, in the same unit as `availableDiskSpace`. Read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_disk_space: Option<f32>,
    /// Read-only flag reported by the device; writes to it are refused by the device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_midi_clock_enabled: Option<bool>,
    /// Which outputs the master volume knob controls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_volume_assignment: Option<MasterVolumeAssignment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stomp_mode_auto_assign: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_tempo_tuner_access: Option<bool>,
    /// Looper X footswitch assignments in physical A through H order: eight raw action indices forming a permutation of 0 through 7: 0 Record, 1 Play, 2 Undo, 3 Duplicate, 4 One Shot, 5 Half Speed, 6 Punch In, 7 Reverse (observed on a CorOS 4.1.0 unit, 2026-10-06). Present only when non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub looper_stomp_assignments: Option<Vec<i32>>,
    /// Where MIDI clock is sent: `off`, `midiDinOnly`, `usbMidiOnly` or `bothUsbAndDinMidi`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi_clock_out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_internet_connection_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_delay_compensation: Option<bool>,
    /// Whether LED dimming is on for Preset mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_dimmed: Option<bool>,
    /// Whether LED dimming is on for Scene mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_dimmed: Option<bool>,
    /// Whether LED dimming is on for Stomp mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stomp_dimmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi_clock_in: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gig_view_stomp_access: Option<bool>,
    /// Footswitch hold timing exactly as the wire carries it, an index from 0 through 5.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_timing_index: Option<i32>,
    /// Derived, not on the wire: 500 plus 100 times `holdTimingIndex`, so 500 through 1000. The unit showed 500, 800 and 1000 ms for indices 0, 3 and 5 (observed on a CorOS 4.1.0 unit, 2026-10-06).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_timing_ms: Option<i32>,
    /// Raw wire value of the screen brightness field, exactly as the unit reported it: a percentage-style value from 0 through 100 (observed on a CorOS 4.1.0 unit, 2026-10-06). `screenBrightness` is derived from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_brightness_wire: Option<i32>,
    /// Raw wire value of the LED brightness field, exactly as the unit reported it: a percentage-style value from 0 through 100 (observed on a CorOS 4.1.0 unit, 2026-10-06). `ledBrightness` is derived from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub led_brightness_wire: Option<i32>,
    /// Raw wire value of the dimmed LED brightness field, exactly as the unit reported it, assumed to share the 0 through 100 scale of the other two (not itself tried); `dimmedLedBrightness` is derived from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimmed_led_brightness_wire: Option<i32>,
}

/// Settings of one physical analog input as reported in the I/O settings. Optional fields are absent when the device did not report them for that port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputPortSettings {
    /// Input settings port. Writable physical ports are 1 and 2 (In 1, In 2) and 4 and 5 (Return 1, Return 2).
    pub input_port_id: u32,
    /// Input gain as a normalized value from 0 to 1, spanning -12 to +60 dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    /// Input gain in dB, from -12 through 60, computed as -12 plus 72 times `level`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_db: Option<f32>,
    /// Input impedance as a normalized choice: nine steps from 0 to 1 in increments of 0.125, shown by the apps as 10k, 33k, 68k, 91k, 130k, 232k, 470k, 1M and 10M ohms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impedance: Option<f32>,
    /// Normalized input type: 0 for instrument, 0.5 for microphone, 1 for microphone with phantom power.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<f32>,
    /// Ground lift state: 0 for off, 1 for on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground_lift: Option<f32>,
    /// Whether a cable is detected in this input jack. The Grid view uses it to mark routed inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugged: Option<bool>,
}

/// Settings of one output port as reported in the I/O settings. Optional fields are absent when the device did not report them for that port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPortSettings {
    /// Output settings port id. Writes accept 1, 2 and 4 through 9, where 4 to 7 are Out 1 to Out 4 and 8 and 9 are Send 1 and Send 2.
    pub output_port_id: u32,
    /// Output level as a normalized value from 0 to 1, spanning -40 to +12 dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    /// Output level in dB, from -40 through 12, computed as -40 plus 52 times `level`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_db: Option<f32>,
    /// Ground lift state as 0 or 1. Writable only on ports 1, 4 and 5.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground_lift: Option<f32>,
    /// Whether the port is muted. Writable only on ports 1, 2 and 4 through 7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// Whether a cable is detected at this output. The Grid view uses it to mark routed outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugged: Option<bool>,
}

/// A headphone feed entry in the I/O settings, pairing an output port with a level. Entries are keyed by `outputPortId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadphonesFeed {
    /// Output port this headphone feed entry refers to; newer publications replace an existing entry with the same id.
    pub output_port_id: u32,
    pub level: f32,
}

/// Headphone output entry of the I/O settings, with the per-output feed list, an overall level and a connection flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadphonesSettings {
    /// Headphone feed entries, at most one per output port; later publications replace the entry for the same output port.
    pub feeds: Vec<HeadphonesFeed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    /// Whether headphones are detected in the headphone output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugged: Option<bool>,
}

/// USB entry of the I/O settings. Its level, headphone source and dry or wet routing are one global setting, not per channel pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsbPortSettings {
    /// USB level as a normalized value from 0 to 1, mapping to -40 through 0 dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    /// Which USB pair feeds the headphones, normalized in thirds: 0 none, 1/3 USB 1/2, 2/3 USB 3/4, 1 both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headphones_source: Option<f32>,
    /// Whether the USB connection to a host is detected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugged: Option<bool>,
    /// USB dry and wet channel routing: 0 puts dry on USB 1/2 and wet on USB 3/4, 1 puts dry on USB 3/4 and wet on USB 1/2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_wet: Option<f32>,
}

/// State of one expression pedal input in the I/O settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpressionPortSettings {
    /// Expression input id as reported by the device. Ids are one-based (ids 1 and 2 are the ones reported); which physical jack each id belongs to is not established.
    pub expression_port_id: i32,
    /// Whether an expression pedal is detected at this input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibrating: Option<bool>,
}

/// MIDI entry of the I/O settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiPortSettings {
    /// MIDI Thru state read from the I/O settings: 1.0 when enabled and 0.0 when disabled, as written by the MIDI Thru action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thru: Option<f32>,
}

/// Input, output, expression, headphone, USB, MIDI and output-pairing settings decoded from an I/O settings publication. Returned by the I/O settings read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IoSettings {
    /// Settings of the input ports the device listed; decoding requires at least one entry.
    pub inputs: Vec<InputPortSettings>,
    /// Settings of the output ports the device listed.
    pub outputs: Vec<OutputPortSettings>,
    /// State of the expression pedal inputs the device listed.
    pub expression_ports: Vec<ExpressionPortSettings>,
    /// Headphone output entry; absent when the device did not report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headphones: Option<HeadphonesSettings>,
    /// USB entry; absent when the device did not report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbPortSettings>,
    /// MIDI entry; absent when the device did not report it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiPortSettings>,
    /// Whether the XLR 1/2 outputs are paired. Absent when not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xlr12_linked: Option<bool>,
    /// Whether outputs 3/4 are paired. Absent when not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out34_linked: Option<bool>,
}

/// One sparse Global EQ control: a parameter slot and its normalized value, as returned by the Global EQ read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalEqParameter {
    /// Slot of the Global EQ control, 0 through 27. Slots 0 through 24 are five bands of five controls each, 25 is the output level, and 26 and 27 are the output 1/2 and 3/4 assignments.
    pub parameter_index: i32,
    /// Normalized control value from 0.0 to 1.0. Band gain, frequency and Q are normalized, filter type is the type number divided by 4, flags are 0.0 or 1.0, and output level maps -20 to +20 dB onto 0.0 to 1.0.
    pub value: f32,
}

/// Global EQ state from the Global EQ read: a sparse list of normalized controls plus the bypass flag when the device reported one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalEqSettings {
    /// Controls carried by this update, each with its slot (0 through 27) and normalized value. The list is sparse, so slots the device did not send are absent, and slots outside 0 through 27 are dropped.
    pub parameters: Vec<GlobalEqParameter>,
    /// Whether the Global EQ is bypassed. Absent when the update did not include the bypass state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypassed: Option<bool>,
}

/// The ordered footswitch performance modes the device cycles through, as read or written by the mode cycle actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeCycle {
    /// Mode values in cycle order, one to three distinct entries of 0 through 8. 0 is Preset, 1 is Scene and 2 is Stomp (observed on a CorOS 4.1.0 unit, 2026-10-07). 3 through 8 are believed to be the hybrids Preset/Scene, Preset/Stomp, Scene/Preset, Scene/Stomp, Stomp/Preset and Stomp/Scene.
    pub slots: Vec<u32>,
}

/// Tempo and metronome state: the loaded preset's tempo block combined with the device-wide mode and global tempo. Returned by the global tempo settings read; fields the device did not report are absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TempoSettings {
    /// Whether the device follows each preset's own tempo (`PRESET`) or one shared global tempo (`GLOBAL`). This setting applies to the whole device, not to a single preset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Tempo of the loaded preset in beats per minute, within 40 through 240, derived from the normalized wire value as 40 plus 200 times it, rounded (observed on a CorOS 4.1.0 unit, 2026-10-07: 0.86 showed 212 and 0.4 showed 120).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<u32>,
    /// The device-wide tempo in beats per minute, within 40 through 240, derived from the normalized wire value as 40 plus 200 times it, rounded (the same mapping as the preset tempo, observed 2026-10-07). It is the tempo in effect while `mode` is `GLOBAL`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_bpm: Option<u32>,
    /// Whether the tempo LED is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub led_enabled: Option<bool>,
    /// Metronome volume in dB as shown on the device's Tempo screen, from -60 to +9.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_db: Option<f32>,
    /// Whether the metronome is audible; false means it is muted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    /// Metronome stereo position from -1.0 (left) through 0.0 (centre) to +1.0 (right).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pan: Option<f32>,
    /// Time signature of the metronome, one of the device's named options such as `4/4` or `7/8 (3+2+2)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<String>,
    /// Metronome note subdivision. Signatures counted in quarters (the `x/4` options) offer 1/4 through 1/16, while eighth-based signatures offer 1/8 through 1/32.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdivision: Option<String>,
    /// Metronome click sound: `BLIP`, `BLOCK`, `COWBELL`, `DIGITAL`, `DRUM KIT` or `SOFT KIT`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    /// Output routing of the metronome: `MULTI`, `HP`, `OUT 1/2`, `OUT 3/4` or `SEND 1/2`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<String>,
    /// Per-beat accent pattern, one entry for each beat cell the device reported (at most thirteen). Each entry is `NORMAL`, `MUTE`, `ACCENT 1` or `ACCENT 2`.
    pub beats: Vec<String>,
}

/// The device-wide tempo mode alone, decoded from a global tempo push that carries only the mode switch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TempoModeSettings {
    /// `PRESET` when each preset keeps its own tempo and metronome settings, or `GLOBAL` when one tempo is shared across every preset.
    pub mode: String,
}

/// Looper X transport and progress state from the looper status read, available when a Looper X is present. Fields the message did not carry are absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LooperStatus {
    /// Raw looper state value as the unit reports it. Observed on a CorOS 4.1.0 unit, 2026-10-07: 1 is IDLE and 5 is ARMED (a Record press toggles between them); no other value has been named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_progress: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_cycle: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_duplicate_cycles: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_shot_stopped: Option<bool>,
    /// Whether a redo is available, so the combined undo/redo control would redo instead of undo. True when the unit's unsigned value is above 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redo_available: Option<bool>,
    /// Raw loop length value as the unit reports it. The field name suggests audio samples, as for `freeSamples`; that unit has not been observed for this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_length: Option<u32>,
    /// Recording capacity still available, in audio samples at 96 kHz: 27131904 showed as AVAILABLE 4:43 on the unit (observed on a CorOS 4.1.0 unit, 2026-10-07).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reverse: Option<bool>,
    /// Whether the One Shot toggle is on: true when the unit's unsigned value is above 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_shot: Option<bool>,
    /// Whether the Half Speed toggle is on: true when the unit's unsigned value is above 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half_speed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_duplicate_cycles: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub armed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for_cycle: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_write_displacement: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_write_displacement: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_waiting_for_quantize: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_clock: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_shot_play: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_start_waiting: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantize_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_type: Option<i32>,
}

/// Expression-pedal assignment on a block's bypass switch, reported on `GridBlock` and `BlockDetails` only while an EXP pedal is assigned. Written back with the set expression bypass action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BypassExpression {
    /// Which expression input drives the bypass: 1 is EXP 1 and 2 is EXP 2. A block with no pedal assigned carries no `BypassExpression` at all, so this is never 0 here.
    pub pedal: u32,
    /// Lower end of the pedal range used for the bypass assignment, as a normalized value from 0 to 1.
    pub minimum: f32,
    /// Upper end of the pedal range used for the bypass assignment, as a normalized value from 0 to 1.
    pub maximum: f32,
    /// Bypass switch mode selector, 0 through 2, exchanged unchanged with the set expression bypass action: 0 Stop, 1 Switch, 2 Heel-Toe (observed on a CorOS 4.1.0 unit, 2026-10-07).
    pub mode: u32,
    /// Whether the pedal direction is inverted for the bypass switch.
    pub invert: bool,
    /// Switch delay in milliseconds, 0 through 5000.
    pub delay_ms: u32,
    /// Whether latch emulation is enabled for the bypass assignment.
    pub latch_emulation: bool,
}

/// One block placed on the Grid, as published in a `preset` state update and in preset snapshots. Identified by row and column; bypass state is the value for the active scene.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridBlock {
    /// Stable identifier built from the grid position as `row:column`, for example `0:3`.
    pub id: String,
    /// Model hash identifying the block's model; the same number the model catalog lists. Absent when the model is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<u32>,
    /// Reserved. This library never sets it: the wire carries no category identifier for a block, and the catalog names a category by text only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<u32>,
    /// Display name of the model from the synchronized model catalog, or `Model` followed by the hash when the catalog has no entry.
    pub name: String,
    /// Broad block type guessed from the model's catalog category by substring matching; a lossy heuristic, not a device value. Defaults to `utility` when the model is not in the catalog, and also for categories the heuristic has no word for. Read `catalogCategory` for the unmodified category.
    pub kind: String,
    /// Display category name of the model, using the same vocabulary as the model list and block details. Absent when the model is not in the catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// True when the catalog marks the model as a plugin model. Absent when the model is not in the catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<bool>,
    /// Plugin identifier of a plugin model, as a decimal string. Absent for non-plugin models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    /// Zero-based grid row, 0 through 3.
    pub row: u32,
    /// Zero-based grid column, 0 through 7.
    pub column: u32,
    /// Whether the block is bypassed in the active scene. Absent when the preset carries no bypass record for this position. A record that names its own row and column is addressed by them, otherwise by its place in the list (see `bypassAddress`). What `bypassSceneMode` false means for the scene is not established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypassed: Option<bool>,
    /// Expression pedal assigned to this block's bypass switch. Absent when no pedal is assigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_expression: Option<BypassExpression>,
    /// Reserved. This library never sets it: the device reports no colour for a block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Optional tile-glyph hint for renderers; `capture-wave` selects the capture waveform icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glyph: Option<String>,
    /// Footswitch slot, 0 through 7 for A through H, that this block is assigned to in STOMP mode. Absent when unassigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch: Option<u32>,
    /// Position of this block's footswitch assignment in the preset's assignment list. Believed, not verified: with several blocks on one footswitch the lowest order is the one whose state the lamp follows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch_order: Option<u32>,
    /// Whether the footswitch assignment is the `primary` or `secondary` one, as reported by the device. Absent when unassigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch_assignment_type: Option<String>,
    /// Category of the model exactly as the synchronized model catalog names it, before the display vocabulary of `category` is applied. Absent when the model is not in the catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_category: Option<String>,
    /// The scene-mode flag of the bypass record this block's `bypassed` came from, exactly as the device wrote it. Absent when the record omits it or when there is no record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_scene_mode: Option<bool>,
    /// How the block's bypass record was found: `explicit` when the record names both its row and column, `positional` when it names neither and its place in the preset's bypass list was used, `mixed` when it names one and its place gave the other. Absent when there is no record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_address: Option<String>,
}

/// Lamp and assignment summary for one of the eight STOMP footswitches, derived by this library from the blocks assigned to it; the device reports none of `active`, `assigned` or `color`. Published in `preset` updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FootswitchState {
    /// Footswitch slot, 0 through 7 for footswitches A through H.
    pub index: u32,
    /// Derived by this library, not reported by the unit: true when the first block assigned to the footswitch, by assignment order, is not bypassed in the active scene. It is not a reading of the lamp, and that the lamp follows that block is believed, not verified.
    pub active: bool,
    /// Derived by this library: true when at least one block is assigned to this footswitch.
    pub assigned: bool,
    /// Derived by this library; the unit reports no lamp color. Lamp color as a lowercase hex string such as `#rrggbb`: the idle color when nothing is assigned, white when several blocks share the switch, otherwise the block category color.
    pub color: String,
    /// Whether the footswitch is set to momentary rather than latching behavior in the preset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub momentary: Option<bool>,
    /// Label shown for the footswitch: the single-block label if set, otherwise the shared label. Empty string when none is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One MIDI Out message slot of a footswitch, expression pedal or preset-load list. A list's index is the slot's position on the unit, so an empty slot (type 0) between two active ones is reported in place; empty slots after the last active one are not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiOutMessage {
    /// Raw wire message type: 0 None (an empty slot, reported between active slots and sent in its position), 1 Control Change, 2 Control Change Toggle, 3 Program Change (observed on a CorOS 4.1.0 unit, 2026-10-06 and 2026-10-07). Undefined types are hazardous and must not be written.
    pub r#type: u32,
    /// MIDI channel, 1 through 16. An empty slot is reported as channel 1 and its value is not sent.
    pub channel: u32,
    /// First data value, 0 through 127: the CC number for types 1 and 2, and the bank CC#0 value for type 3 (observed on a CorOS 4.1.0 unit, 2026-10-06).
    pub param1: u32,
    /// Second data value, 0 through 127: the value for type 1, the minimum value for type 2 and the bank CC#32 value for type 3 (observed on a CorOS 4.1.0 unit, 2026-10-06).
    pub param2: u32,
    /// Third data value, 0 through 127: the maximum value for type 2 and the program number for type 3; not shown by the unit for type 1 but kept in the live and stored bytes (observed on a CorOS 4.1.0 unit, 2026-10-06 and 2026-10-07).
    pub param3: u32,
}

/// The MIDI Out messages attached to one trigger source in the current preset. Sources with no messages are omitted from `midiOut` updates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiOutSource {
    /// Trigger source: 0 through 7 are footswitches A through H, and 8 and 9 are the EXP 1 and EXP 2 expression inputs.
    pub source: u32,
    /// Messages sent for this source, one entry per slot in the unit's order (up to 12 slots per source); empty slots between active ones are listed as type 0 and empty slots after the last active one are not.
    pub messages: Vec<MidiOutMessage>,
    /// Index in the preset's flat list of 120 message rows at which this source's group of twelve begins, exactly as read. Present for sources read from a preset. Group `s` of the 120 rows is source `s` (observed on a CorOS 4.1.0 unit, 2026-10-07).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_flat_offset: Option<u32>,
}

/// Routing of one Grid row: the input and output it is wired to plus its optional split and mix points. Published in `preset` updates, one entry per row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridRoute {
    /// Zero-based grid row, 0 through 3.
    pub row: u32,
    /// Numeric id of the row's input route exactly as the device wrote it. Absent when the device wrote none (the `input` label is then empty). `inputWire` names the wire enum value that carries this id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_id: Option<u32>,
    /// Numeric id of the row's output route exactly as the device wrote it. Absent when the device wrote none (the `output` label is then empty). `outputWire` names the wire enum value that carries this id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_id: Option<u32>,
    /// Display label of the input route, for example `In 1`, `Return 1/2`, `USB 5` or `Prev. Row`; id 0 reads `Not In Use`. An id the label table does not name reads as its wire enum name, or `Unknown input` followed by the id when the wire schema does not declare it either. Empty when the row carries no input id.
    pub input: String,
    /// Display label of the output route, for example `Out 1/2`, `Send 1`, `USB 3/4` or `Multi Out`; id 0 reads `Internal`. An id the label table does not name reads as its wire enum name, or `Unknown output` followed by the id when the wire schema does not declare it either. Empty when the row carries no output id.
    pub output: String,
    /// Grid column where the row's parallel split begins. Absent when the row has no split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split_column: Option<i32>,
    /// Grid column where the row's parallel paths are mixed back together. Absent when the row has no mixer point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mix_column: Option<i32>,
    /// Model hash of the row's splitter node. Absent when the row has no splitter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub splitter_model_id: Option<u32>,
    /// Whether the row's split/mix path is muted in the active scene.
    pub split_muted: bool,
    /// Name of the wire schema's enum value for `inputId`, for example `INPUT_1` or `SIDECHAIN_BUFFER`. That enum is declared on the gain-calibration port message; using its numbering for a Grid row's input is this library's reading. Absent when `inputId` is absent or undeclared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_wire: Option<String>,
    /// Name of the wire schema's enum value for `outputId`, for example `XLR_1_2` or `MULTIPLE_OUTS`. That enum is declared on the gain-calibration port message; using its numbering for a Grid row's output is this library's reading. Absent when `outputId` is absent or undeclared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_wire: Option<String>,
}

/// Connection state of one physical port, pushed by the device when a cable is plugged or unplugged. Delivered in `ioPorts` updates and merged into the snapshot by `kind` and `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IoPortState {
    /// Which port family the entry belongs to: an analog input, an analog output, the headphones jack, the USB port or an expression-pedal input.
    pub kind: String,
    /// Port identifier within its `kind`. Expression ids are reported by the device and are one-based (ids 1 and 2 are the ones reported); which physical jack each id belongs to is not established. Headphones and USB use 0.
    pub id: i32,
    /// Display name of the port assigned by this library, for example `In 1`, `Out 1/2`, `Headphones`, `USB` or `EXP 1`. An expression label is the reported id with a prefix; which jack an id belongs to is not established.
    pub label: String,
    /// True when the device reports a cable connected to this port.
    pub plugged: bool,
}

/// One entry of the device's configured footswitch mode list, reported with `mode` updates. Slots A, B and C are selected with the select mode slot action.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeSlot {
    /// Zero-based position in the mode list, 0 through 2 for slots A through C.
    pub slot: u32,
    /// Name of the mode in this slot: `PRESET`, `SCENE` or `STOMP`, or a hybrid pair such as `PRESET / SCENE`; `UNKNOWN` for a value outside the nine this library names.
    pub label: String,
    /// The mode family of the slot: `PRESET`, `SCENE`, `STOMP`, `HYBRID` for any pair of two base modes, or `UNKNOWN` for a wire value outside 0 through 8. The names follow the numbering the vendor's desktop application uses; the wire schema declares none.
    pub mode: String,
    /// The unmodified wire value of the slot's mode. Read this when `mode` is `UNKNOWN`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_mode: Option<u32>,
}

/// Bypass state of one block, one entry of a `bypassBatch` update produced by a sparse Grid push.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BypassUpdate {
    /// Zero-based grid row of the block, 0 through 3.
    pub row: u32,
    /// Zero-based grid column of the block, 0 through 7.
    pub column: u32,
    /// Whether the block is bypassed in the active scene.
    pub bypassed: bool,
}

/// Ordering stamp attached to state updates by the native host. Revisions are only comparable within one session, so a client adopts an update only when the session matches and the revision is not older.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeStateVersion {
    /// Opaque token identifying the device session the update belongs to. Compare for equality only; revisions from different sessions are unrelated.
    pub session: String,
    /// Counter that increases with each committed state reduction within a session, starting at 1 and never above 9007199254740991.
    pub revision: u64,
}

/// DSP load attributed to one Grid column, one entry of the `cpuColumns` list of a `cpuLoad` update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuColumnLoad {
    /// Load of this column as a fraction from 0 to 1.
    pub load: f32,
    /// Which processor core runs the column, 1 or 2.
    pub core: u32,
}

/// A model preset the device announced in this session, listed by the `entries` of a `modelPresetCatalog` update. The list is the session's account of what was announced, not a complete list. The Global EQ screen offers these for recall; `id`, `isFactory` and `modelHash` together identify the preset to load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPresetSummary {
    /// Preset identifier as the device announced it. With `isFactory` and `modelHash` it is the preset's identity.
    pub id: String,
    /// Preset name shown to the user.
    pub name: String,
    /// Whether the preset is a factory preset rather than a user preset.
    pub is_factory: bool,
    /// Whether the device flags this as the default preset for its model; shown as Default in the Global EQ preset menu.
    pub is_default: bool,
    /// Hash of the model the preset belongs to. Every entry the host publishes carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_hash: Option<u32>,
}

/// The model preset the device last reported as loaded, exactly as reported. Fields the device omitted stay absent. It is a notification, not authority to recall anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPresetCatalogMarker {
    /// Device-assigned preset identifier, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Whether the reported preset is a factory preset, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_factory: Option<bool>,
    /// Hash of the model the reported preset belongs to, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_hash: Option<u32>,
}

/// Tallies of what the model preset fold did with the messages it saw in this session. They describe the fold, not the device, and never establish that a list is complete.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPresetCatalogCounters {
    /// Model preset messages folded.
    pub messages: u32,
    /// Create messages applied.
    pub creates: u32,
    /// Update messages seen, including renames and loaded-preset notifications.
    pub updates: u32,
    /// Single-entry updates that matched a retained entry.
    pub renames: u32,
    /// Single-entry updates that matched no retained entry; they never insert.
    pub rename_misses: u32,
    /// Delete messages that removed at least one retained entry.
    pub deletes: u32,
    /// Delete entries that matched no retained entry.
    pub delete_misses: u32,
    /// Delete messages with no entries, which clear every retained list.
    pub wipes: u32,
    /// Read messages, which are ignored.
    pub reads_ignored: u32,
    /// Messages whose action the fold does not apply.
    pub other_actions_ignored: u32,
    /// Loaded-preset notifications recorded as the active marker.
    pub marker_updates: u32,
    /// Update messages whose loaded-preset fields were incomplete or lacked the preset identifier; they changed no list.
    pub ambiguous_markers: u32,
    /// Messages of another action that carried a loaded-preset field, which the fold ignored; they were applied by their own action.
    pub loaded_ignored: u32,
    /// Entries whose identity lacked a value, a hash or a factory flag.
    pub malformed_identities: u32,
    /// Model preset messages that could not be decoded.
    pub decode_failures: u32,
    /// Times inbound transport loss that could have cost a model preset message was observed in this session; the first one invalidates the projection.
    pub inbound_loss: u32,
    /// Appended entries that repeated a retained identity.
    pub duplicate_tuples: u32,
    /// Identities currently retained more than once; they are never offered or admitted.
    pub ambiguous_tuples: u32,
    /// Entries retained across all model hashes.
    pub retained_entries: u32,
    /// Retained entries outside the scope hash, which this update does not list.
    pub retained_other_hash_entries: u32,
}

/// The retained fold of the model preset list for one state session, carried by modelPresetCatalog updates and by snapshots. Each publication is the whole projection, so the latest one replaces any earlier one. It never claims the list is complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPresetCatalogState {
    /// unavailable before any list was seen, folding while the fold is usable, invalidated once something may have been lost; invalidation lasts until the session ends.
    pub state: String,
    /// The state session this projection belongs to; use it only when it equals the session of the snapshot or update that carries it.
    pub session: String,
    /// Folds applied in this session; it grows with every folded message.
    pub generation: u32,
    /// The model hash whose entries are listed.
    pub scope_hash: u32,
    /// Retained entries of the scope hash with a unique identity, in arrival order. Present only while folding, and never cut: 256 scope-hash entries, counting repeats, is an inclusive maximum; more invalidates the projection (publication-limit) instead of truncating it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<ModelPresetSummary>>,
    /// The most recent loaded-preset notification, as reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_marker: Option<ModelPresetCatalogMarker>,
    /// Why the fold was invalidated, when it was. Reconnecting is not known to cure malformed-identity, retention-limit or publication-limit. Limits are inclusive maxima: publication-limit is more than 256 scope-hash entries, counting repeats; retention-limit more than 20000 retained.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    /// What the fold did with the messages it saw.
    pub counters: ModelPresetCatalogCounters,
}

/// Progress report of a long device operation, carried by `bulkOperation` updates. Hosts block further control while a blocking operation has not finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkOperationState {
    /// Progress text supplied by the device; hosts display it as a notice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Whether the operation blocks other control while it runs. Hosts treat it as blocking only while `finished` is not true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    /// Whether the operation has completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<bool>,
}

/// One normalized state change decoded from a device message. `kind` selects what changed, and only the fields belonging to that kind (plus the stamps `stateVersion` and `observedAt`) are present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QcStateUpdate {
    /// Which change this update describes, such as `scene`, `preset`, `bypassBatch` or `cpuLoad`. Each kind populates only its own fields, named per field below.
    pub kind: String,
    /// Kind `scene`: the newly selected scene, 0 through 7 for scenes A through H.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_scene: Option<u32>,
    /// Kind `dirty`: whether the current preset has unsaved edits. Full `preset` updates may also carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    /// Kind `master`: master volume as the normalized wire value from 0 to 1. The hosts' 0 to 100 figure is this value times 100, rounded. No physical level is established for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_volume: Option<f32>,
    /// Kinds `sceneLabel` and `sceneColor`: the scene the change applies to, 0 through 7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Kind `sceneLabel`: the scene's new label text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Kind `sceneColor`: the scene's new color as a lowercase hex string such as `#rrggbb`, with the alpha byte of the device's 32-bit ARGB value dropped. `rawColor` holds the value as sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Kinds `position` and `preset`: path-style key of the setlist holding the current preset; its last segment is the setlist name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setlist_key: Option<String>,
    /// Kinds `position` and `preset`: zero-based slot of the current preset within its setlist, as the unit reports it. The location label (bank from 1, letters A through H) is derived by this library: position divided by 8, plus 1, and the remainder as the letter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Kind `position`: whether the setlist reported by the device is flagged as factory content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_factory: Option<bool>,
    /// Kind `preset`: name of the current preset. Empty when the preset is unsaved and unnamed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_name: Option<String>,
    /// Kinds `tempo` and `preset`: tempo in beats per minute, between 40 and 240.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<u32>,
    /// Kinds `tempo` and `preset`: whether the tempo LED is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo_led_enabled: Option<bool>,
    /// Kind `preset`: the eight scene labels in order for display; an unset label is replaced by `Scene A` through `Scene H`, which is this library's text and not something the device sent. `sceneLabelsRaw` holds the labels as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenes: Option<Vec<String>>,
    /// Kind `preset`: the scene colors in scene order, each a lowercase hex string such as `#rrggbb`. The device stores 32-bit ARGB values; this drops the alpha byte. `sceneColorsArgb` holds the values as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_colors: Option<Vec<String>>,
    /// Kind `preset`: lamp and assignment summary for the eight STOMP footswitches, A through H.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch_states: Option<Vec<FootswitchState>>,
    /// Kinds `preset` and `midiOut`: MIDI Out messages per trigger source (footswitches A through H and the two expression inputs). Sources with no messages are omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi_out: Option<Vec<MidiOutSource>>,
    /// Kinds `preset` and `midiOut`: MIDI messages the preset sends when it loads, one entry per slot in the unit's order, with empty slots between active ones listed as type 0 and empty slots after the last active one not listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_load_midi_out: Option<Vec<MidiOutMessage>>,
    /// Kind `preset`: every placed block on the Grid with its state for the active scene.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<GridBlock>>,
    /// Kind `preset`: the routing of each Grid row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<GridRoute>>,
    /// Kind `ioPorts`: ports whose connection state was reported; entries are merged into the snapshot by port kind and id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_ports: Option<Vec<IoPortState>>,
    /// Kind `mode`: the active footswitch mode, `PRESET`, `SCENE`, `STOMP` or `HYBRID`, or `UNKNOWN` for a wire value outside 0 through 8. The wire schema declares no mode enum; the names follow the numbering the vendor's desktop application uses. `rawMode` carries the number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Kind `mode`: the device's configured mode list, up to three slots. Present only when the device reported it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_slots: Option<Vec<ModeSlot>>,
    /// Kind `mode`: two base mode names, `PRESET`, `SCENE` or `STOMP`, for the two parts of a hybrid mode in label order; both are the same outside hybrid modes. Absent when the mode value is unknown. That the first governs footswitches A-D and the second E-H is believed, not verified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footswitch_modes: Option<Vec<String>>,
    /// Kinds `bypass`, `blockRemoved`, `parameter` and `defaultParameters`: zero-based grid row, 0 through 3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u32>,
    /// Same kinds as `row`: grid column, 0 through 7. For `parameter` updates 8 addresses the row's splitter and 9 its mixer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Kind `bypass`: whether the block at `row` and `column` is bypassed in the active scene.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypassed: Option<bool>,
    /// Kind `bypassBatch`: bypass state of each block named by a sparse Grid push, ordered by row then column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_updates: Option<Vec<BypassUpdate>>,
    /// Kind `parameter`: index of the parameter within the block at `row` and `column`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter_index: Option<u32>,
    /// Kind `parameter`: the parameter's new value as a normalized number from 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_value: Option<f32>,
    /// True when the model catalog changed, either with a `catalog` update or on a `preset` update rebuilt after a catalog install; names and categories should be read again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_refresh: Option<bool>,
    /// Kind `gigView`: whether the device is showing Gig View.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gig_view_shown: Option<bool>,
    /// Kind `cpuLoad`: overall DSP load as a fraction from 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_total_load: Option<f32>,
    /// Kind `cpuLoad`: per-column DSP load, flattened across Grid rows in order, so a client can index it by row times 8 plus column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_columns: Option<Vec<CpuColumnLoad>>,
    /// Kind `history`: whether the device's undo frame list has any entries. Derived by this library, and not established as the device's own notion of availability: the undo list seen from a unit held twenty entries, mostly zero. Read `undoFrames`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_available: Option<bool>,
    /// Kind `history`: whether the device's redo frame list has any entries; derived like `undoAvailable`. Read `redoFrames`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redo_available: Option<bool>,
    /// Kind `refresh`: true when the change, such as a scene copy, cannot be applied incrementally and the current preset should be read again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_required: Option<bool>,
    /// Kind `suspend`: true while the device has suspended host control, for example during a backup, and false once it resumes. Writes are refused while suspended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended: Option<bool>,
    /// Kind `suspend`: reason name reported by the device in upper case, such as `BACKUP`; an unrecognized reason reads `UNKNOWN_` followed by its number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspend_reason: Option<String>,
    /// Kind `bulkOperation`: progress of a long running device operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bulk_operation: Option<BulkOperationState>,
    /// Kind `modelPresetCatalog`: the retained fold of the model preset list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preset_catalog: Option<ModelPresetCatalogState>,
    /// Kind `gridTopology`, and `preset` when present: whether the current block list has complete Grid topology. False after an unmergeable sparse structural update until a full preset publication restores it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid_layout_complete: Option<bool>,
    /// Kind `defaultParameters`: how many default parameter records the device sent for the block at `row` and `column`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_parameter_count: Option<u32>,
    /// Kind `catalog`: model hashes the device announced as newly available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_model_ids: Option<Vec<u32>>,
    /// Wall-clock time of the observation in milliseconds since the Unix epoch. Clients compare it with the issue time of their own commands to order them against device echoes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<u64>,
    /// Kinds `position` and `preset`: opaque token naming the current preset. Pass it back as the expected preset identity of guarded writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_identity: Option<String>,
    /// Ordering stamp assigned by the native host to the committed state; adopt an update only when its session matches and its revision is not older.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_version: Option<NativeStateVersion>,
    /// Kind `mode`: the unmodified wire value of the active mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_mode: Option<u32>,
    /// Kind `sceneColor`: the unmodified 32-bit ARGB value the device sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_color: Option<u32>,
    /// Kind `preset`: the scene colors in scene order as the device stores them, 32-bit ARGB values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_colors_argb: Option<Vec<u32>>,
    /// Kind `preset`: the eight scene labels exactly as stored, with no default text; unset labels are empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_labels_raw: Option<Vec<String>>,
    /// Kind `history`: the unmodified undo frame list. What its entries mean is not established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_frames: Option<Vec<i32>>,
    /// Kind `history`: the unmodified redo frame list. What its entries mean is not established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redo_frames: Option<Vec<i32>>,
}

/// One control point of a `lookup` parameter scale, pairing a normalized position with the real value shown there. Values between points are interpolated linearly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalePoint {
    /// Normalized parameter position for this point, from 0 through 1.
    pub normalized: f64,
    /// Real-world value (in the parameter's `units`) the control shows at this normalized position.
    pub real: f64,
}

/// One editable control of a block, splitter, mixer or lane control as returned by `get_block_details`: its current normalized value, how to convert it to a displayed value, and its editing state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockParameter {
    /// Zero-based parameter index within the block's model; this is the index that parameter write actions take.
    pub index: u32,
    /// Position of the control in the model's editor layout, used to place it on the parameter pages. Falls back to `index` when the model declares no layout position.
    pub display_position: u32,
    /// Parameter label from the model catalog. A parameter the catalog does not name is reported as `Parameter N`, where N is `index` plus one.
    pub name: String,
    /// Current value as a normalized position from 0 through 1. Writes take the same scale. With `sceneMode` on, this is the value stored for the active scene.
    pub normalized_value: Option<f64>,
    /// Current value rendered as text the way the device shows it: the selected option, an end label, or the real value with `displayPrecision` decimals. Raw normalized value when `scaleKnown` is false.
    pub display_value: String,
    /// Unit text declared for the parameter in the model catalog, for example `dB`, `Hz`, `ms` or `%`. Empty when the parameter has no unit.
    pub units: String,
    /// Control type declared in the model catalog, for example `float`, `enum`, `rotarySwitch`, `toggleButton`, `switch`, `fader` or `grMeter`. Defaults to `float`, or `enum` when options exist.
    pub r#type: String,
    /// Real value, in `units`, at normalized 0 for linear and power scales. Zero when no scale is known.
    pub minimum: f64,
    /// Real value, in `units`, at normalized 1. One when no scale is known.
    pub maximum: f64,
    /// How normalized values map to real values: `linear`, `power`, `logarithmic`, `lookup` (see `scalePoints`), `options` (see `options`) or `unknown`.
    pub value_scale: String,
    /// Exponent of a `power` scale, where real = minimum + normalized to the power of this exponent times (maximum minus minimum). Absent for other scales.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_exponent: Option<f64>,
    /// Normalized-to-real control points of a `lookup` scale, in ascending normalized order. Empty for every other scale.
    pub scale_points: Vec<ScalePoint>,
    /// True when the numeric range and scale are established, or when the parameter is an option list. When false, do not convert between normalized and real values.
    pub scale_known: bool,
    /// Number of decimal places used to render `displayValue`. Absent when the parameter has no catalog entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_precision: Option<u32>,
    /// Text the device shows instead of a number at normalized 0, for example `Off`. Absent when none is defined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_label: Option<String>,
    /// Text shown at the exact middle of travel (normalized 0.5), for example `C` on a pan-style control. Absent when none is defined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midpoint_label: Option<String>,
    /// Text shown instead of a number at normalized 1. Absent when none is defined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_label: Option<String>,
    /// True when the value is displayed as a whole number with no decimals.
    pub show_as_integer: bool,
    /// True when normalized 0 is an Off position named by `minimumLabel` rather than the numeric `minimum`. Pan-style controls with minimum, midpoint and maximum labels report false.
    pub has_off_position: bool,
    /// Lowest real value above the Off position, in `units`: `minimum` plus 1 for integer controls or 0.01 otherwise. Null unless `hasOffPosition` is true and the scale is known.
    pub floor_display: Option<f64>,
    /// Normalized position of `floorDisplay`, the first writable value above Off. Null when `floorDisplay` is null or the scale is not linear or power.
    pub floor_normalized: Option<f64>,
    /// Number of discrete positions the control quantizes to, when the catalog declares one. Editors step by 1 divided by (steps minus 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    /// True when the parameter stores a separate value for each scene. `normalizedValue` then reflects the active scene.
    pub scene_mode: bool,
    /// Labels for a choice parameter, in order. The selected index is the normalized value spread evenly over the labels, so a lone label is index 0. Empty for continuous parameters.
    pub options: Vec<String>,
    /// False for read-only controls such as gain-reduction meters and for parameters that carry no stored value. A true value is not a guarantee that `enabled` is true.
    pub writable: bool,
    /// False when the device renders the control inactive because another parameter, such as a sync switch, currently gates it. Writes to a disabled parameter are refused.
    pub enabled: bool,
    /// False when the parameter cannot be assigned to an expression pedal, as declared by the model or because the parameter belongs to a legacy splitter.
    pub expression_assignable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_scene_mode: Option<u32>,
    /// Expression pedal assigned to this parameter (1 for EXP 1, 2 for EXP 2). Absent when no pedal is assigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<i32>,
    /// Heel end of the pedal range as a normalized value from 0 through 1. Absent when no range is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression_minimum: Option<f32>,
    /// Toe end of the pedal range as a normalized value from 0 through 1. Absent when no range is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression_maximum: Option<f32>,
    /// Level for controls that carry a signal LED. Editors treat a value above 0.02 as lit. Absent when no level is reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub led_value: Option<f32>,
    /// `text` when the device stores this parameter's value as a string, `numeric` when it stores a number.
    pub wire_value_kind: String,
}

/// Live parameters of one occupied Grid block, splitter or mixer, returned by `get_block_details` for the active scene, with the model identity and any expression-pedal bypass assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDetails {
    /// Zero-based Grid row of the block.
    pub row: u32,
    /// Zero-based Grid column of the block. Lane controls such as Input Gate and Lane Output are addressed in columns 10 and 11.
    pub column: u32,
    /// Numeric id of the block's model in the device's model catalog.
    pub model_id: u32,
    /// Model name from the catalog. Reported as `Model N` when the id is not in the catalog.
    pub name: String,
    /// Display category of the model, for example `Guitar Amp`, `Delay`, `EQ` or `Reverb`. Defaults to `Utility` when the model is unknown.
    pub category: String,
    /// Zero-based index of the active scene (0 through 7) that scene-mode parameter values were read for.
    pub scene: u32,
    /// Expression pedal assignment for the block's bypass state. Absent when no pedal is assigned to bypass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_expression: Option<BypassExpression>,
    /// Visible parameters of the block. Hidden catalog entries and parameters the model does not draw are left out.
    pub parameters: Vec<BlockParameter>,
}

/// Read-back of a plugin block placed by a verified `add_block`, attached to the result as `pluginBlockReadback`. Reports whether the device added it bypassed, since plugin ownership cannot be checked beforehand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginBlockReadback {
    /// Always true when present; the field appears only for a block whose model is flagged as a plugin.
    pub plugin: bool,
    /// Bypass state of the new block as read back from the device. Absent when the read-back snapshot reports no bypass state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypassed: Option<bool>,
    /// Whether the add asked for the block to be bypassed. Always false, because `add_block` takes no bypass argument.
    pub bypass_requested: bool,
    /// Explanation set only when the block reads back bypassed: the device added it bypassed and may show it locked if the plugin is not licensed on the unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
