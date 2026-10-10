//! Platform-neutral decoding for correlated device replies that are not part
//! of the continuous preset-state stream.

mod library;
pub use library::*;

use crate::{
    commands::is_tuner_reference_offset_hz,
    compression::{maybe_gunzip, InflateError},
    generated_payloads::{
        ExpressionPortSettings, GeneralSettings, GlobalBypassRows, GlobalEqParameter,
        GlobalEqSettings, HeadphonesFeed, HeadphonesSettings, InputPortSettings, IoSettings,
        LooperStatus, MasterVolumeAssignment, MidiPortSettings, ModeCycle, OutputPortSettings,
        TempoModeSettings, TempoSettings, TunerSettings, UsbPortSettings,
    },
    profile,
    proto::cortex_protobuf_v2 as pa,
    proto::{binary_preset, param, param_value, Param},
};
use prost::Message;
use serde::Serialize;
use thiserror::Error;

/// Reasons a correlated device reply could not be decoded or accepted.
#[derive(Debug, Error)]
pub enum ResponseDecodeError {
    /// The reply is larger than its message type can carry (that type's inflation
    /// limit, or the 1 MiB raw frame ceiling, whichever is larger), or a payload
    /// nested in it is larger than the 1 MiB frame-size limit.
    #[error("the QC protobuf reply exceeded its size limit")]
    OversizedReply,
    /// The payload is not a valid protobuf message of the expected type.
    #[error("QC protobuf reply could not be decoded: {0}")]
    Protobuf(#[from] prost::DecodeError),
    /// A gzip-compressed reply would inflate past its size ceiling.
    #[error("compressed QC reply exceeds the inflated-size limit")]
    InflatedLimit,
    /// A gzip-compressed reply could not be inflated; carries the decoder's message.
    #[error("compressed QC reply could not be decoded: {0}")]
    Compression(String),
    /// A field the decoder requires is absent; carries the name of the missing field.
    #[error("the QC reply is incomplete: {0}")]
    Incomplete(&'static str),
    /// The reply is well formed but is not the one expected (wrong action, request id, target or an out-of-range value); carries a short description.
    #[error("the QC reply does not match the request: {0}")]
    Mismatch(&'static str),
    /// The image payload is not a PNG with a valid `IHDR` header and non-zero dimensions.
    #[error("the QC response is not a PNG image")]
    InvalidPng,
    /// The assembled backup document exceeded the backup size limit.
    #[error("the QC backup exceeded the 32 MiB safety limit")]
    OversizedBackup,
    /// The backup stream ended with a document that is not complete JSON of the expected backup type.
    #[error("the QC backup stream ended with an incomplete or unsupported document")]
    InvalidBackupDocument,
}

impl From<InflateError> for ResponseDecodeError {
    fn from(error: InflateError) -> Self {
        match error {
            InflateError::Limit => Self::InflatedLimit,
            InflateError::Decode(message) => Self::Compression(message),
        }
    }
}

/// The envelope message type a reply message travels in, so that its payload
/// is bounded by that type's own limit rather than one shared ceiling.
trait ReplyMessage: Message + Default {
    const MESSAGE_TYPE: u16;
}

macro_rules! reply_message_types {
    ($($message:ty => $message_type:expr),* $(,)?) => {
        $(impl ReplyMessage for $message {
            const MESSAGE_TYPE: u16 = $message_type;
        })*
    };
}

reply_message_types! {
    pa::CompilerInhibitedModulesMessage => profile::MESSAGE_TYPE_COMPILER_INHIBITED_MODULES,
    pa::DiagnosticsMessage => profile::MESSAGE_TYPE_DIAGNOSTICS,
    pa::FileMessage => profile::MESSAGE_TYPE_FILE,
    pa::GeneralSettingsMessage => profile::MESSAGE_TYPE_GENERAL_SETTINGS,
    pa::GlobalEqMessage => profile::MESSAGE_TYPE_GLOBAL_EQ,
    pa::GlobalTempoMessage => profile::MESSAGE_TYPE_GLOBAL_TEMPO,
    pa::IoSettingsMessage => profile::MESSAGE_TYPE_IO_SETTINGS,
    pa::LocalBackupMessage => profile::MESSAGE_TYPE_LOCAL_BACKUP,
    pa::LooperMessage => profile::MESSAGE_TYPE_LOOPER,
    pa::ModeMessage => profile::MESSAGE_TYPE_MODE,
    pa::PinnedModelsMessage => profile::MESSAGE_TYPE_PINNED_MODELS,
    pa::RecallPresetMessage => profile::MESSAGE_TYPE_RECALL_PRESET,
    pa::RecentsFavoritesMessage => profile::MESSAGE_TYPE_RECENTS_FAVORITES,
    pa::RemoteControlMessage => profile::MESSAGE_TYPE_REMOTE_CONTROL,
    pa::SceneMessage => profile::MESSAGE_TYPE_SCENE,
    pa::ScreenshotMessage => profile::MESSAGE_TYPE_SCREENSHOT,
    pa::TunerMessage => profile::MESSAGE_TYPE_TUNER,
    pa::VersionMessage => profile::MESSAGE_TYPE_VERSION,
}

/// Decode a reply the transport delivered. The payload is already inflated
/// when its envelope was compressed, so it is bounded by the reply type's
/// inflation limit (or the raw frame ceiling, whichever is larger), never by
/// the raw ceiling alone.
fn decode_reply<M>(payload: &[u8]) -> Result<M, ResponseDecodeError>
where
    M: ReplyMessage,
{
    if payload.len() > crate::compression::max_delivered_payload_bytes_for(M::MESSAGE_TYPE) {
        return Err(ResponseDecodeError::OversizedReply);
    }
    let decoded = maybe_gunzip(payload)?;
    Ok(M::decode(decoded.as_slice())?)
}

fn require_update_action(action: i32, family: &'static str) -> Result<(), ResponseDecodeError> {
    if action == pa::message_action::Enum::Update as i32 {
        Ok(())
    } else {
        Err(ResponseDecodeError::Mismatch(family))
    }
}

/// Fold a sparse publication while replacing complete nested value groups.
/// Protobuf's recursive merge would retain old true values when the sender
/// omits proto3 false scalars inside an explicitly present assignment/row group.
pub fn merge_general_settings_publication(
    previous: &[u8],
    incoming: &[u8],
) -> Result<Vec<u8>, ResponseDecodeError> {
    let mut retained: pa::GeneralSettingsMessage = decode_reply(previous)?;
    let update: pa::GeneralSettingsMessage = decode_reply(incoming)?;
    require_update_action(update.action, "GeneralSettings")?;
    if update.master_volume_assignment.is_some() {
        retained.master_volume_assignment = None;
    }
    if update.global_bypass_cab.is_some() {
        retained.global_bypass_cab = None;
    }
    if update.global_bypass_ir.is_some() {
        retained.global_bypass_ir = None;
    }
    // `merge` appends a repeated field's incoming items onto whatever is
    // already retained (protobuf merge semantics), so a republished frame
    // that DOES carry values for one of these fields would otherwise
    // duplicate its own entries every time this function runs. Clear first,
    // the same technique used above for the Option fields, so merge acts as
    // a replace instead of an append -- but only when the update actually
    // carries a value for that field. Proto3 repeated fields have no
    // presence bit, so an omitted field and an explicitly-empty one are
    // wire-indistinguishable; clearing unconditionally would wipe previously
    // retained data on every sparse echo that mentions some unrelated
    // GeneralSettings field but says nothing about these two.
    //
    // Accepted limitation: because of that same wire ambiguity, this
    // conditional clear cannot represent the device genuinely clearing one
    // of these lists to empty either -- an explicit clear-to-empty and an
    // omitted field both decode as `is_empty()`, so the clear is silently
    // ignored and the previously retained entries survive. This is an
    // inherent proto3 limitation, not an oversight: it deliberately favors
    // never-clear-when-unsure (a stale retained list) over the alternative
    // of duplicating entries on every unrelated sparse update.
    if !update.looper_stomp_assignments.is_empty() {
        retained.looper_stomp_assignments.clear();
    }
    if !update.download_destination_folders.is_empty() {
        retained.download_destination_folders.clear();
    }
    retained.merge(update.encode_to_vec().as_slice())?;
    Ok(retained.encode_to_vec())
}

/// Whether a settings publication may be retained, or folded into the retained
/// one, as this session's settings state.
///
/// The pinned desktop receiver publishes nothing for a type-3 UPDATE whose
/// request_id differs from its own pending reset id (IOSettingsMessageReceiver
/// publisher, 0x141c00ec4..0x141c00edb). This host never sends one, so any
/// tagged type-3, a zero id included, is neither retained nor folded and the
/// last good publication stays. Folding it would also copy the tag into the
/// retained payload, which the I/O decoders then reject on every later read.
/// Every other payload is admitted unchanged; the merges and decoders keep
/// their own validation.
pub fn settings_publication_is_retainable(message_type: u16, payload: &[u8]) -> bool {
    message_type != profile::MESSAGE_TYPE_IO_SETTINGS
        || !decode_reply::<pa::IoSettingsMessage>(payload)
            .is_ok_and(|message| message.request_id.is_some())
}

/// I/O publications update addressed ports; repeated records are not a new
/// inventory and must not erase other ports or append duplicate identities.
pub fn merge_io_settings_publication(
    previous: &[u8],
    incoming: &[u8],
) -> Result<Vec<u8>, ResponseDecodeError> {
    if !settings_publication_is_retainable(profile::MESSAGE_TYPE_IO_SETTINGS, incoming) {
        return Err(ResponseDecodeError::Incomplete("I/O settings request_id"));
    }
    let mut retained: pa::IoSettingsMessage = decode_reply(previous)?;
    let mut update: pa::IoSettingsMessage = decode_reply(incoming)?;
    require_update_action(update.action, "IOSettings")?;
    if let Some(pa::io_settings_message::Settings::Settings(mut delta)) = update.settings.take() {
        let mut settings = retained
            .settings
            .take()
            .map(|pa::io_settings_message::Settings::Settings(value)| value)
            .unwrap_or_default();
        merge_port_records(
            &mut settings.in_port,
            std::mem::take(&mut delta.in_port),
            |port| port.input_port_id,
        )?;
        merge_port_records(
            &mut settings.out_port,
            std::mem::take(&mut delta.out_port),
            |port| port.output_port_id,
        )?;
        merge_port_records(
            &mut settings.exp_port,
            std::mem::take(&mut delta.exp_port),
            |port| port.exp_port_id,
        )?;
        if let (
            Some(pa::port_settings::HpPort::HpPort(old)),
            Some(pa::port_settings::HpPort::HpPort(new)),
        ) = (&mut settings.hp_port, &mut delta.hp_port)
        {
            for feed in std::mem::take(&mut new.hp_feed) {
                if let Some(current) = old
                    .hp_feed
                    .iter_mut()
                    .find(|item| item.output_port_id == feed.output_port_id)
                {
                    *current = feed;
                } else {
                    old.hp_feed.push(feed);
                }
            }
        }
        settings.merge(delta.encode_to_vec().as_slice())?;
        retained.settings = Some(pa::io_settings_message::Settings::Settings(settings));
    }
    retained.merge(update.encode_to_vec().as_slice())?;
    Ok(retained.encode_to_vec())
}

fn merge_port_records<T: Message + Default, K: PartialEq>(
    retained: &mut Vec<T>,
    updates: Vec<T>,
    key: impl Fn(&T) -> K,
) -> Result<(), prost::DecodeError> {
    for update in updates {
        if let Some(current) = retained.iter_mut().find(|item| key(item) == key(&update)) {
            current.merge(update.encode_to_vec().as_slice())?;
        } else {
            retained.push(update);
        }
    }
    Ok(())
}

/// Acknowledgement of a current-preset read, produced by [`decode_current_preset_ack`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentPresetAck {
    /// Request id echoed by the reply, if the reply carries one. The host's read is the bare
    /// type-15 READ Cortex Control sends at boot, which carries none.
    pub request_id: Option<u64>,
}

/// Validate the reply that refreshes the shared full-preset state. The read that asks for it is
/// the bare type-15 READ (`08 03`, no request id) Cortex Control sends at boot, so the reply is
/// matched by type and arrival after the read, not by an echoed id. The preset itself is reduced
/// by `StateDecoder`; this acknowledgement imposes no tempo-field presence.
pub fn decode_current_preset_ack(payload: &[u8]) -> Result<CurrentPresetAck, ResponseDecodeError> {
    let message: pa::RecallPresetMessage = decode_reply(payload)?;
    require_update_action(message.action, "current-preset action is not UPDATE")?;
    if message.preset.is_none() {
        return Err(ResponseDecodeError::Incomplete("current preset"));
    }
    let request_id = message
        .request_id
        .map(|pa::recall_preset_message::RequestId::RequestId(value)| value);
    Ok(CurrentPresetAck { request_id })
}

/// Identity details reported by the device's version reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    /// Device serial number.
    pub serial: String,
    /// Application firmware version string, if the reply carries one.
    ///
    /// This is the application firmware build identifier that the unit's
    /// Settings > Device Information screen labels as the app firmware (for
    /// example `d14e` has been observed on a unit running CorOS 4.1.0). It is
    /// observed not to be the CorOS version number.
    pub app_fw_version: Option<String>,
    /// User-assigned device name, if the reply carries one. A unit that has
    /// not been renamed has been observed to report `Neural DSP Quad Cortex`.
    pub custom_name: Option<String>,
    /// Raw device-type enumeration value from the reply, if present.
    ///
    /// A Quad Cortex reports `0`. The protocol schema also declares `1`, which
    /// is believed to identify a compact variant of the unit, with four
    /// footswitches (two pages of four scenes), a smaller set of I/O ports and
    /// a power-on mode setting. That has not been verified on the unit used
    /// for this project's tests. This crate treats `1` as an unsupported
    /// device during startup.
    pub device_type: Option<i32>,
}

/// Which global processing modules the device reports as inhibited. A field is `None` when the reply does not mention that module.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InhibitedModules {
    /// Inhibition state of the global gate module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_gate: Option<bool>,
    /// Inhibition state of the global EQ module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_eq: Option<bool>,
}

/// The models and captures pinned on the device.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedModels {
    /// Pinned model identifiers, as reported by the device.
    pub models: Vec<u32>,
    /// Keys of the pinned captures.
    pub captures: Vec<String>,
}

/// A validated PNG image returned by the device.
#[derive(Debug, Clone, PartialEq)]
pub struct PngImage {
    /// The complete PNG file.
    pub bytes: Vec<u8>,
    /// Width in pixels, read from the `IHDR` chunk.
    pub width: u32,
    /// Height in pixels, read from the `IHDR` chunk.
    pub height: u32,
}

/// Diagnostics counters for one DSP core, as reported by the device. The reply is passed through unscaled and its fields are not interpreted by this crate.
///
/// The field layout follows the protocol schema. Current firmware has not
/// been observed to answer the diagnostics request (it times out), and it was
/// not seen pushing a diagnostics message unprompted either: about nine minutes
/// of passive device traffic across four sessions on CorOS 4.1.0 contained
/// none. The units, ranges and bit meanings of these fields are therefore not
/// verified.
///
/// The unit's own DSP Diagnostics screen shows, for each of four cores,
/// dropped messages, floating-point exceptions, CPU average and peak
/// percentages, an external-memory DMA percentage and heap percentages for
/// three memory regions. The CPU, DMA and heap figures below are believed to be
/// percentages, and the three heap figures the data-memory, program-memory and
/// external-memory heaps, as on that screen; with no reply to compare against,
/// that has not been verified on the unit used for this project's tests. The
/// meaning of the status words and the error flags is not established.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DspDiagnostics {
    /// First status word.
    pub status1: u32,
    /// Second status word.
    pub status2: u32,
    /// Third status word.
    pub status3: u32,
    /// Fourth status word.
    pub status4: u32,
    /// Processing load figure, passed through as sent by the device; believed
    /// to be a percentage of the core's processing capacity. Always finite.
    pub process_cpu: f32,
    /// Peak value of the processing load figure, believed to be a percentage.
    /// Always finite.
    pub process_cpu_max: f32,
    /// Internal heap usage figure, believed to be a percentage of the
    /// data-memory heap. Always finite.
    pub internal_heap_usage: f32,
    /// External heap usage figure, believed to be a percentage of the
    /// external-memory heap. Always finite.
    pub external_heap_usage: f32,
    /// Internal program-memory heap usage figure, believed to be a percentage
    /// of the program-memory heap. Always finite.
    pub internal_pm_heap_usage: f32,
    /// External DMA usage figure, believed to be a percentage of the
    /// external-memory DMA capacity. Always finite.
    pub external_dma_usage: f32,
    /// Error flag bits.
    pub error_flags: u32,
}

/// USB subsystem counters from the device's diagnostics reply, as reported and passed through unscaled.
///
/// The field layout follows the protocol schema. Current firmware has not
/// been observed to answer the diagnostics request (it times out), and no
/// diagnostics message appeared in about nine minutes of passive device
/// traffic across four sessions on CorOS 4.1.0, so the units and meanings of
/// these counters are not verified. The unit's own USB Statistics screen lists
/// counters with similar names, but there is no reply to compare it against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsbDiagnostics {
    /// USB connected value as sent by the device. Believed to be a flag that is
    /// non-zero while a USB host is connected; the unit's own USB Statistics
    /// screen shows a field of that name as Yes or No.
    pub connected: u32,
    /// Read index (`inReadIdx`) of the USB input buffer.
    pub input_read_index: u32,
    /// Write index of the USB input buffer.
    pub input_write_index: u32,
    /// Read index of the USB output buffer.
    pub output_read_index: u32,
    /// Write index of the USB output buffer.
    pub output_write_index: u32,
    /// USB audio frames received.
    pub audio_rx_frames: u64,
    /// USB audio frames transmitted.
    pub audio_tx_frames: u64,
    /// USB audio transmit frames skipped.
    pub audio_tx_frames_skipped: u32,
    /// USB audio transmit frames aborted.
    pub audio_tx_frames_aborted: u32,
    /// Average read/write distance of the USB audio output buffer.
    pub audio_output_distance_average: u32,
    /// Minimum read/write sample count reported for USB audio.
    pub audio_min_samples: u32,
    /// Maximum read/write sample count reported for USB audio.
    pub audio_max_samples: u32,
    /// Plus-one-sample count reported for USB audio.
    pub audio_plus_one_sample_count: u32,
    /// Minus-one-sample count reported for USB audio.
    pub audio_minus_one_sample_count: u32,
    /// USB audio feedback state value.
    pub audio_feedback_state: u32,
    /// Streaming-endpoint-enabled value reported for USB audio.
    pub audio_streaming_endpoint_enabled: u32,
    /// Streaming-endpoint-disabled value reported for USB audio.
    pub audio_streaming_endpoint_disabled: u32,
    /// USB MIDI output count.
    pub midi_out_count: u64,
    /// USB MIDI input count.
    pub midi_in_count: u64,
    /// USB HID input count.
    pub hid_in_count: u64,
    /// USB HID output count.
    pub hid_out_count: u64,
    /// USB HID input reports dropped.
    pub hid_in_dropped_count: u32,
    /// USB HID output reports dropped.
    pub hid_out_dropped_count: u32,
    /// Average read/write distance of the USB audio input buffer.
    pub audio_input_distance_average: u32,
    /// USB audio output gaps detected.
    pub audio_output_gaps_detected: u32,
    /// USB audio output gap-reset count.
    pub audio_output_gap_resets: u32,
    /// USB audio output buffer resets.
    pub audio_output_buffer_resets: u32,
}

/// Decoded diagnostics reply. Each section is `None` when the device omits it.
///
/// Current firmware has not been observed to answer the diagnostics request
/// (it times out), so a reply of this shape has not been seen from a live unit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDiagnostics {
    /// Request id echoed by the reply, if any.
    pub request_id: Option<u64>,
    /// Diagnostics for core 1 of the first SoC.
    pub soc1_core1: Option<DspDiagnostics>,
    /// Diagnostics for core 2 of the first SoC.
    pub soc1_core2: Option<DspDiagnostics>,
    /// Diagnostics for core 1 of the second SoC.
    pub soc2_core1: Option<DspDiagnostics>,
    /// Diagnostics for core 2 of the second SoC.
    pub soc2_core2: Option<DspDiagnostics>,
    /// USB diagnostics reported by the ARM core of the second SoC.
    pub soc2_arm: Option<UsbDiagnostics>,
    /// Count of messages dropped between the first and second SoC, as reported by the device.
    pub soc1_to_soc2_dropped_count: Option<u64>,
}

/// Metronome clock status pushed by the device, as returned by [`decode_tempo_clock`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TempoClock {
    /// Whether the metronome is enabled.
    pub enabled: bool,
    /// Whether the pre-roll (count-in) is enabled. The unit's Looper X has a
    /// pre-roll count-in of 1, 2 or 4 bars before recording or playing; this
    /// field and the count-in fields below are believed to drive its pre-roll
    /// indicator (beats left in the bar, bars left in the count-in). Pre-roll
    /// was never enabled in the captures examined, so their values while a
    /// pre-roll runs have not been observed.
    pub preroll_enabled: bool,
    /// Current beat, as reported by the device. Zero-based: `0..=3` has been
    /// observed in a four-beat bar, and the bar counter advances on the update
    /// that reports beat `0` (observed on CorOS 4.1.0 at 120 BPM with the
    /// metronome off).
    pub current_beat: u32,
    /// Current bar, as reported by the device. A free-running counter that
    /// advances once per bar at the active tempo (a bar lasted 2000 ms at
    /// 120 BPM in 4/4) and is counted from an origin shared by all host
    /// sessions: it keeps counting across separate sessions, so a new session
    /// does not restart it. The origin is believed to be when the unit started,
    /// which fits the values seen, in the thousands and as low as the tens;
    /// that has not been verified on the unit used for this project's tests.
    /// Whether the first bar is `0` or `1` is not established.
    pub current_bar: u32,
    /// Current tick, as reported by the device. Observed on a CorOS 4.1.0 unit
    /// with the metronome not enabled, 2026-10-07: status messages arrive
    /// continuously, one beat message per beat with tick `0` and a second
    /// message with tick `1` about 23 ms later, so `1` is a pulse flag and not
    /// a subdivision count and no ticks-per-beat figure applies. Earlier
    /// captures saw values up to `21`, and others (for example `14` and `16`)
    /// only next to gaps in delivery to the host. The delay from tick `0` to
    /// tick `1` scaled with the beat length, about 23 ms at 120 BPM and about
    /// 14 ms at 212 BPM.
    pub current_tick: u32,
    /// Count-in beats remaining. While no count-in is running it counts down
    /// over the bar, from the bar's last beat index at beat `0` to `0` at the
    /// last beat (`3` down to `0` in the four-beat bars observed). Its values
    /// while a pre-roll runs have not been observed.
    pub count_in_beats_remaining: i32,
    /// Count-in bars remaining. While no count-in is running it is `-1`
    /// (observed with the metronome and pre-roll off, at two tempos). Its
    /// values while a pre-roll runs have not been observed; it may relate to
    /// the Looper X count-in of 1, 2 or 4 bars.
    pub count_in_bars_remaining: i32,
}

/// Reassembles a device backup from the stream of LocalBackup chunks into one JSON document.
///
/// Feed each LocalBackup payload to [`BackupAssembler::push`] or [`BackupAssembler::push_classified`]. Fragments
/// that precede the start of a JSON document are ignored, so a transfer begun by an earlier session does not
/// corrupt a new one.
#[derive(Debug, Default)]
pub struct BackupAssembler {
    document: String,
    started: bool,
    chunks: usize,
    ignored_prefix_chunks: usize,
    ignored_prefix_terminators: usize,
}

/// Payload-free classification for a LocalBackup protobuf frame. This is safe
/// to include in timeout diagnostics: it contains shape and lengths only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupFrameShape {
    /// Raw message-action value of the frame.
    ///
    /// The values are `0` create, `1` update, `2` delete, `3` read, `4` move,
    /// `5` copy, `6` upload, `7` download and `8` swap. Replies this crate
    /// accepts carry update; upload and download are declared by the schema
    /// but are never sent by this crate.
    pub action: i32,
    /// Length of the frame payload in bytes.
    pub payload_bytes: usize,
    /// Whether the frame carries a request id.
    pub request_id_present: bool,
    /// The frame's last-chunk flag, or `None` if the frame omits it.
    pub terminal: Option<bool>,
    /// Whether the frame carries a backup data chunk.
    pub has_data: bool,
    /// Length of the data chunk in bytes; zero when the frame has none.
    pub data_bytes: usize,
    /// The frame's can-apply-backup flag, if present.
    pub can_apply_backup: Option<bool>,
    /// The frame's applied-backup flag, if present.
    pub applied_backup: Option<bool>,
    /// How the assembler treated the frame: `ignored-action`, `missing-terminal`, `missing-data`, `ignored-prefix`,
    /// `started-chunk`, `continued-chunk` or `complete`.
    pub outcome: &'static str,
}

impl BackupAssembler {
    /// Discards any partially assembled document and all counters, ready for a new transfer.
    pub fn reset(&mut self) {
        self.document.clear();
        self.started = false;
        self.chunks = 0;
        self.ignored_prefix_chunks = 0;
        self.ignored_prefix_terminators = 0;
    }

    /// Returns whether the start of a backup document has been seen and chunks are being accumulated.
    pub fn started(&self) -> bool {
        self.started
    }

    /// Returns the number of chunks accumulated into the current document.
    pub fn chunks(&self) -> usize {
        self.chunks
    }

    /// Returns the number of chunks discarded before the start of a JSON document was seen.
    pub fn ignored_prefix_chunks(&self) -> usize {
        self.ignored_prefix_chunks
    }

    /// Returns the number of last-chunk markers discarded before the start of a JSON document was seen.
    pub fn ignored_prefix_terminators(&self) -> usize {
        self.ignored_prefix_terminators
    }

    /// Feeds one LocalBackup payload and returns the finished document once the last chunk arrives.
    ///
    /// Returns `Ok(None)` while the transfer is incomplete or when the frame is ignored. See
    /// [`BackupAssembler::push_classified`] for the errors and for a description of what happened to the frame.
    pub fn push(&mut self, payload: &[u8]) -> Result<Option<String>, ResponseDecodeError> {
        self.push_classified(payload)
            .map(|(document, _shape)| document)
    }

    /// Like [`BackupAssembler::push`], additionally returning a payload-free [`BackupFrameShape`] that says what
    /// happened to the frame.
    ///
    /// Chunks are appended only for UPDATE frames that carry both a last-chunk flag and data, and only from the
    /// first chunk that begins a JSON document. On error the assembler is reset. Fails if the payload cannot be
    /// decoded, if the document grows past the backup size limit ([`ResponseDecodeError::OversizedBackup`]), or if
    /// the final document is not a complete JSON backup ([`ResponseDecodeError::InvalidBackupDocument`]).
    pub fn push_classified(
        &mut self,
        payload: &[u8],
    ) -> Result<(Option<String>, BackupFrameShape), ResponseDecodeError> {
        let message: pa::LocalBackupMessage = decode_reply(payload)?;
        let action = message.action;
        let request_id_present = message.request_id.is_some();
        let terminal = message
            .is_last_chunk
            .map(|pa::local_backup_message::IsLastChunk::IsLastChunk(value)| value);
        let can_apply_backup = message
            .can_apply_backup
            .map(|pa::local_backup_message::CanApplyBackup::CanApplyBackup(value)| value);
        let applied_backup = message
            .applied_backup
            .map(|pa::local_backup_message::AppliedBackup::AppliedBackup(value)| value);
        let data = message
            .backup_json
            .map(|pa::local_backup_message::BackupJson::BackupJson(chunk)| chunk);
        let has_data = data.is_some();
        let data_bytes = data.as_ref().map_or(0, |chunk| chunk.len());
        let shape = |outcome| BackupFrameShape {
            action,
            payload_bytes: payload.len(),
            request_id_present,
            terminal,
            has_data,
            data_bytes,
            can_apply_backup,
            applied_backup,
            outcome,
        };
        // LocalBackup data streams only on the device's UPDATE path. CREATE is
        // the one-shot outbound export request, not a stream chunk, and must
        // not be spliced into a response stream.
        if message.action != pa::message_action::Enum::Update as i32 {
            return Ok((None, shape("ignored-action")));
        }
        // A chunk is appended only when the terminal field is explicitly
        // present. The device sends Some(false) on intermediate chunks and
        // Some(true) on the final chunk.
        let terminal = match terminal {
            Some(value) => value,
            None => return Ok((None, shape("missing-terminal"))),
        };
        let chunk = match data {
            Some(chunk) => chunk,
            None => return Ok((None, shape("missing-data"))),
        };

        if !self.started {
            // Current QC firmware does not echo request_id on LocalBackup
            // replies. A new client can therefore inherit the tail of a
            // transfer started by an earlier session. Synchronize only at a
            // JSON document boundary and ignore every preceding fragment or
            // terminator.
            if !chunk.trim_start().starts_with('{') {
                if !chunk.is_empty() || terminal {
                    self.ignored_prefix_chunks = self.ignored_prefix_chunks.saturating_add(1);
                }
                if terminal {
                    self.ignored_prefix_terminators =
                        self.ignored_prefix_terminators.saturating_add(1);
                }
                return Ok((None, shape("ignored-prefix")));
            }
            self.started = true;
        }

        self.chunks = self.chunks.saturating_add(1);
        self.document.push_str(&chunk);
        if self.document.len() > profile::BACKUP_MAXIMUM_DOCUMENT_BYTES {
            self.reset();
            return Err(ResponseDecodeError::OversizedBackup);
        }

        if terminal {
            let valid = serde_json::from_str::<serde_json::Value>(&self.document)
                .ok()
                .is_some_and(|document| {
                    document.get("type").and_then(serde_json::Value::as_str) == Some("backup")
                        && document.get("creator").and_then(serde_json::Value::as_str)
                            == Some("quad")
                });
            if !valid {
                self.reset();
                return Err(ResponseDecodeError::InvalidBackupDocument);
            }
            let complete = std::mem::take(&mut self.document);
            self.started = false;
            self.chunks = 0;
            return Ok((Some(complete), shape("complete")));
        }
        Ok((
            None,
            shape(if self.chunks == 1 {
                "started-chunk"
            } else {
                "continued-chunk"
            }),
        ))
    }
}

/// Decodes a metronome clock push.
///
/// Returns `Ok(None)` for a reply that carries a request id (a reply to a request rather than a push) or that has
/// no metronome status. Fails if the payload cannot be decoded or the message action is not UPDATE.
pub fn decode_tempo_clock(payload: &[u8]) -> Result<Option<TempoClock>, ResponseDecodeError> {
    let message: pa::GlobalTempoMessage = decode_reply(payload)?;
    require_update_action(message.action, "tempo-clock action is not UPDATE")?;
    if message.request_id.is_some() {
        return Ok(None);
    }
    let Some(pa::global_tempo_message::MetronomeStatus::MetronomeStatus(status)) =
        message.metronome_status
    else {
        return Ok(None);
    };
    Ok(Some(TempoClock {
        enabled: status.is_enabled != 0,
        preroll_enabled: status.preroll_enabled != 0,
        current_beat: status.current_beat,
        current_bar: status.current_bar,
        current_tick: status.current_tick,
        count_in_beats_remaining: status.count_in_beats_remaining,
        count_in_bars_remaining: status.count_in_bars_remaining,
    }))
}

/// Decode the active preset name without exposing prost's generated optional
/// field wrappers to either native host.
pub fn decode_recalled_preset_name(payload: &[u8]) -> Result<Option<String>, ResponseDecodeError> {
    let message: pa::RecallPresetMessage = decode_reply(payload)?;
    require_update_action(message.action, "recalled-preset action is not UPDATE")?;
    let Some(pa::recall_preset_message::Preset::Preset(preset)) = message.preset else {
        return Ok(None);
    };
    Ok(preset.name.map(|binary_preset::Name::Name(name)| name))
}

/// Decode the selected scene without leaking generated presence wrappers into
/// native adapters.
pub fn decode_selected_scene(payload: &[u8]) -> Result<Option<u32>, ResponseDecodeError> {
    let message: pa::SceneMessage = decode_reply(payload)?;
    require_update_action(message.action, "selected-scene action is not UPDATE")?;
    Ok(message
        .selected_scene
        .map(|pa::scene_message::SelectedScene::SelectedScene(scene)| scene))
}

/// Decodes the device's version reply into a [`DeviceIdentity`].
///
/// Fails if the payload cannot be decoded, the message action is not UPDATE, or the reply has no serial number.
pub fn decode_device_identity(payload: &[u8]) -> Result<DeviceIdentity, ResponseDecodeError> {
    let message: pa::VersionMessage = decode_reply(payload)?;
    require_update_action(message.action, "identity action is not UPDATE")?;
    let serial = match message.device_serial_number {
        Some(pa::version_message::DeviceSerialNumber::DeviceSerialNumber(value)) => value,
        None => return Err(ResponseDecodeError::Incomplete("device serial number")),
    };
    let app_fw_version = message
        .app_fw_version
        .map(|pa::version_message::AppFwVersion::AppFwVersion(value)| value);
    let custom_name = message
        .custom_name
        .map(|pa::version_message::CustomName::CustomName(value)| value);
    let device_type = message.device_type.map(|value| match value {
        pa::version_message::DeviceTypeOneOf::DeviceType(value) => value,
    });
    Ok(DeviceIdentity {
        serial,
        app_fw_version,
        custom_name,
        device_type,
    })
}

/// Decodes the tuner state.
///
/// The reply must carry the input port, the reference frequency offset and the mute state. The input port must be
/// a known tuner input and the offset must lie within -40..=40 Hz of 440 Hz; `reference_hz` in the result is
/// 440 Hz plus the offset. The meter and meter-enable fields are optional, and a meter value must be finite.
/// Fails if the payload cannot be decoded, the message action is not UPDATE, or any of those conditions is not met.
pub fn decode_tuner_settings(payload: &[u8]) -> Result<TunerSettings, ResponseDecodeError> {
    let message: pa::TunerMessage = decode_reply(payload)?;
    require_update_action(message.action, "tuner action is not UPDATE")?;
    let input_port_id = message
        .input_port_id
        .map(|pa::tuner_message::InputPortId::InputPortId(value)| value)
        .ok_or(ResponseDecodeError::Incomplete("tuner input_port_id"))?;
    if !crate::commands::is_tuner_input_port_id(input_port_id) {
        return Err(ResponseDecodeError::Mismatch("unknown tuner input_port_id"));
    }
    let reference_offset_hz = message
        .frequency
        .map(|pa::tuner_message::Frequency::Frequency(value)| value)
        .ok_or(ResponseDecodeError::Incomplete("tuner frequency"))?;
    if !is_tuner_reference_offset_hz(reference_offset_hz) {
        return Err(ResponseDecodeError::Mismatch(
            "tuner frequency offset is outside -40..40 Hz",
        ));
    }
    let muted = message
        .mute
        .map(|pa::tuner_message::Mute::Mute(value)| value)
        .ok_or(ResponseDecodeError::Incomplete("tuner mute"))?;
    let meter_enabled = message
        .enable_meter
        .map(|pa::tuner_message::EnableMeter::EnableMeter(value)| value);
    let meter = message
        .meter
        .map(|pa::tuner_message::Meter::Meter(value)| value);
    if meter.is_some_and(|value| !value.is_finite()) {
        return Err(ResponseDecodeError::Mismatch("tuner meter is not finite"));
    }
    Ok(TunerSettings {
        input_port_id,
        reference_offset_hz,
        reference_hz: 440.0 + reference_offset_hz,
        muted,
        meter_enabled,
        meter,
    })
}

/// Decodes a general-settings publication.
///
/// A field the message does not carry is `None` in the result, so the output suits both full and sparse
/// publications; see [`merge_general_settings_publication`] for folding sparse ones. Brightness values are
/// converted to a 32-step scale (`round(raw * 32 / 100)`, the reading the unit
/// itself showed for the screen and LED fields on a CorOS 4.1.0 unit,
/// 2026-10-06); the raw wire values are kept beside them in the
/// `*_brightness_wire` fields. `hold_timing_ms` is derived from
/// `hold_timing_index` as `500 + 100 * index`, the reading the unit showed on
/// the same date. The scene bypass behaviour is reported as
/// `alwaysOverwrite` (`0`, scene bypass changes are always saved),
/// `nonstompOverwrite` (`1`, changes made with a Stomp footswitch are not saved
/// but touchscreen changes are) or `neverOverwrite` (`2`, changes are never
/// saved). A `midi_channel` of `17` is shown by the unit as OMNI (observed on the
/// same unit and date).
/// Fails if the payload cannot be decoded, the message action is not UPDATE, a scene bypass
/// or MIDI clock output value is unknown, or the hold timing index is outside 0..=5.
pub fn decode_general_settings(payload: &[u8]) -> Result<GeneralSettings, ResponseDecodeError> {
    let message: pa::GeneralSettingsMessage = decode_reply(payload)?;
    require_update_action(message.action, "general-settings action is not UPDATE")?;
    let scene_bypass_behavior = match message.scene_block_bypass {
        Some(pa::general_settings_message::SceneBlockBypass::SceneBlockBypass(value)) => Some(
            match value {
                0 => "alwaysOverwrite",
                1 => "nonstompOverwrite",
                2 => "neverOverwrite",
                _ => {
                    return Err(ResponseDecodeError::Mismatch(
                        "unknown scene_block_bypass value",
                    ))
                }
            }
            .to_string(),
        ),
        None => None,
    };
    let midi_clock_out = match message.midi_clock_out {
        Some(pa::general_settings_message::MidiClockOut::MidiClockOut(value)) => Some(
            match value {
                0 => "off",
                1 => "midiDinOnly",
                2 => "usbMidiOnly",
                3 => "bothUsbAndDinMidi",
                _ => {
                    return Err(ResponseDecodeError::Mismatch(
                        "unknown midi_clock_out value",
                    ))
                }
            }
            .to_string(),
        ),
        None => None,
    };
    let rows = |value: pa::GlobalBypassRows| GlobalBypassRows {
        row1: value.row1,
        row2: value.row2,
        row3: value.row3,
        row4: value.row4,
    };
    let hold_timing_index = message
        .hold_timing
        .map(|pa::general_settings_message::HoldTiming::HoldTiming(value)| value);
    if hold_timing_index.is_some_and(|value| !(0..=5).contains(&value)) {
        return Err(ResponseDecodeError::Mismatch("hold_timing is out of range"));
    }
    // The brightness fields as the wire carries them, kept beside the derived
    // steps so a caller can see exactly what the unit reported.
    let screen_brightness_wire = message
        .screen_brightness
        .map(|pa::general_settings_message::ScreenBrightness::ScreenBrightness(value)| value);
    let led_brightness_wire = message
        .led_brightness
        .map(|pa::general_settings_message::LedBrightness::LedBrightness(value)| value);
    let dimmed_led_brightness_wire = message
        .dimmed_led_brightness
        .map(|pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(value)| value);
    Ok(GeneralSettings {
        screen_brightness: message.screen_brightness.map(
            |pa::general_settings_message::ScreenBrightness::ScreenBrightness(value)| {
                1 - ((((value as f32 - 1.0) / -100.0) * 32.0).trunc() as i32)
            },
        ),
        led_brightness: message.led_brightness.map(
            |pa::general_settings_message::LedBrightness::LedBrightness(value)| {
                (value as f32 * 32.0 / 100.0).round() as i32
            },
        ),
        dimmed_led_brightness: message.dimmed_led_brightness.map(
            |pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(value)| {
                ((value as f32 * 32.0 / 100.0).round() as i32).min(31)
            },
        ),
        lock_screen_and_volume_knob: message.lock_screen_and_volume_knob.map(
            |pa::general_settings_message::LockScreenAndVolumeKnob::LockScreenAndVolumeKnob(
                value,
            )| value,
        ),
        global_bypass_cab: message.global_bypass_cab.map(
            |pa::general_settings_message::GlobalBypassCab::GlobalBypassCab(value)| rows(value),
        ),
        global_bypass_ir: message.global_bypass_ir.map(
            |pa::general_settings_message::GlobalBypassIr::GlobalBypassIr(value)| rows(value),
        ),
        scene_bypass_behavior,
        midi_over_usb: message
            .midi_over_usb
            .map(|pa::general_settings_message::MidiOverUsb::MidiOverUsb(value)| value),
        midi_channel: message
            .midi_channel
            .map(|pa::general_settings_message::MidiChannel::MidiChannel(value)| value),
        ignore_duplicate_pc: message.ignore_duplicate_pc.map(
            |pa::general_settings_message::IgnoreDuplicatePc::IgnoreDuplicatePc(value)| value,
        ),
        available_disk_space: message.available_disk_space.map(
            |pa::general_settings_message::AvailableDiskSpace::AvailableDiskSpace(value)| value,
        ),
        total_disk_space: message.total_disk_space.map(
            |pa::general_settings_message::TotalDiskSpace::TotalDiskSpace(value)| value,
        ),
        internal_midi_clock_enabled: message.internal_midi_clock_enabled.map(
            |pa::general_settings_message::InternalMidiClockEnabled::InternalMidiClockEnabled(
                value,
            )| value,
        ),
        master_volume_assignment: message.master_volume_assignment.map(
            |pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(value)| {
                MasterVolumeAssignment {
                    out12: value.out12,
                    out34: value.out34,
                    send12: value.send12,
                    headphones: value.headphones,
                }
            },
        ),
        stomp_mode_auto_assign: message.stomp_mode_auto_assign.map(
            |pa::general_settings_message::StompModeAutoAssign::StompModeAutoAssign(value)| value,
        ),
        swap_tempo_tuner_access: message.swap_tempo_tuner_access.map(
            |pa::general_settings_message::SwapTempoTunerAccess::SwapTempoTunerAccess(value)| value,
        ),
        looper_stomp_assignments: (!message.looper_stomp_assignments.is_empty())
            .then_some(message.looper_stomp_assignments),
        midi_clock_out,
        disable_internet_connection_check: message.disable_internet_connection_check.map(
            |pa::general_settings_message::DisableInternetConnectionCheck::DisableInternetConnectionCheck(value)| value,
        ),
        dynamic_delay_compensation: message.enable_dynamic_delay_compensation.map(
            |pa::general_settings_message::EnableDynamicDelayCompensation::EnableDynamicDelayCompensation(value)| value,
        ),
        preset_dimmed: message.enable_preset_dimmed.map(
            |pa::general_settings_message::EnablePresetDimmed::EnablePresetDimmed(value)| value,
        ),
        scene_dimmed: message.enable_scene_dimmed.map(
            |pa::general_settings_message::EnableSceneDimmed::EnableSceneDimmed(value)| value,
        ),
        stomp_dimmed: message.enable_stomp_dimmed.map(
            |pa::general_settings_message::EnableStompDimmed::EnableStompDimmed(value)| value,
        ),
        midi_clock_in: message.midi_clock_in_enabled.map(
            |pa::general_settings_message::MidiClockInEnabled::MidiClockInEnabled(value)| value,
        ),
        gig_view_stomp_access: message.gig_view_stomp_access_enabled.map(
            |pa::general_settings_message::GigViewStompAccessEnabled::GigViewStompAccessEnabled(
                value,
            )| value,
        ),
        hold_timing_index,
        hold_timing_ms: hold_timing_index.map(|value| 500 + 100 * value),
        screen_brightness_wire,
        led_brightness_wire,
        dimmed_led_brightness_wire,
    })
}

/// Decodes an I/O settings publication into input, output, expression, headphone, USB and MIDI port settings.
///
/// Input and output levels are reported both as the normalized wire value and in dB. Fails if the payload cannot
/// be decoded, the message action is not UPDATE, the settings group is absent, or it lists no input ports.
pub fn decode_io_settings(payload: &[u8]) -> Result<IoSettings, ResponseDecodeError> {
    decode_io_settings_fields(payload, true)
}

/// Decode only the fields in a sparse I/O UPDATE echo for write verification.
/// Unlike the authoritative read decoder, an output-only echo needs no input ports.
/// Missing fields remain absent; callers must not merge retained fields before matching.
///
/// # Errors
/// Returns an error for malformed messages, non-UPDATE actions, an unowned request id, or an UPDATE with neither settings nor linked flags.
pub fn decode_io_settings_echo(payload: &[u8]) -> Result<IoSettings, ResponseDecodeError> {
    decode_io_settings_fields(payload, false)
}

fn decode_io_settings_fields(
    payload: &[u8],
    require_inputs: bool,
) -> Result<IoSettings, ResponseDecodeError> {
    let message: pa::IoSettingsMessage = decode_reply(payload)?;
    require_update_action(message.action, "I/O settings action is not UPDATE")?;
    // This host has no pending desktop resetSettingsRequestId to correlate a tag against.
    if message.request_id.is_some() {
        return Err(ResponseDecodeError::Incomplete("I/O settings request_id"));
    }
    let has_linked_flag = message.xlr1_2_linked.is_some() || message.out3_4_linked.is_some();
    let settings = match message.settings {
        Some(pa::io_settings_message::Settings::Settings(value)) => value,
        None if !require_inputs && has_linked_flag => pa::PortSettings::default(),
        None => return Err(ResponseDecodeError::Incomplete("I/O settings")),
    };
    if require_inputs && settings.in_port.is_empty() {
        return Err(ResponseDecodeError::Incomplete("input ports"));
    }

    let inputs = settings
        .in_port
        .into_iter()
        .map(|port| {
            let level = port
                .level
                .map(|pa::input_port_settings::Level::Level(value)| value);
            InputPortSettings {
                input_port_id: port.input_port_id,
                level,
                level_db: level.map(|value| -12.0 + 72.0 * value),
                impedance: port
                    .input_zmode
                    .map(|pa::input_port_settings::InputZmode::InputZmode(value)| value),
                input_type: port
                    .input_type
                    .map(|pa::input_port_settings::InputType::InputType(value)| value),
                ground_lift: port
                    .ground_lift
                    .map(|pa::input_port_settings::GroundLift::GroundLift(value)| value),
                plugged: port
                    .plugged
                    .map(|pa::input_port_settings::Plugged::Plugged(value)| value),
            }
        })
        .collect();
    let outputs = settings
        .out_port
        .into_iter()
        .map(|port| {
            let level = port
                .level
                .map(|pa::output_port_settings::Level::Level(value)| value);
            OutputPortSettings {
                output_port_id: port.output_port_id,
                level,
                // The wire field is normalized 0..1; the control the user sees
                // is -40..+12 dB. Publish both, as the input port already does,
                // so a level read back is a level that can be written again.
                level_db: level.map(crate::commands::output_level_wire_to_db),
                ground_lift: port
                    .ground_lift
                    .map(|pa::output_port_settings::GroundLift::GroundLift(value)| value),
                muted: port
                    .mute
                    .map(|pa::output_port_settings::Mute::Mute(value)| value),
                plugged: port
                    .plugged
                    .map(|pa::output_port_settings::Plugged::Plugged(value)| value),
            }
        })
        .collect();
    let expression_ports = settings
        .exp_port
        .into_iter()
        .map(|port| ExpressionPortSettings {
            expression_port_id: port.exp_port_id,
            plugged: port
                .plugged
                .map(|pa::exp_port_settings::Plugged::Plugged(value)| value),
            level: port
                .level
                .map(|pa::exp_port_settings::Level::Level(value)| value),
            calibrating: port
                .calibrating
                .map(|pa::exp_port_settings::Calibrating::Calibrating(value)| value),
        })
        .collect();
    let headphones =
        settings.hp_port.map(
            |pa::port_settings::HpPort::HpPort(port)| HeadphonesSettings {
                feeds: port
                    .hp_feed
                    .into_iter()
                    .map(|feed| HeadphonesFeed {
                        output_port_id: feed.output_port_id,
                        level: feed.level,
                    })
                    .collect(),
                level: port
                    .level
                    .map(|pa::headphones_settings::Level::Level(value)| value),
                plugged: port
                    .plugged
                    .map(|pa::headphones_settings::Plugged::Plugged(value)| value),
            },
        );
    let usb = settings.usb_port.map(
        |pa::port_settings::UsbPort::UsbPort(port)| UsbPortSettings {
            level: port
                .level
                .map(|pa::usb_port_settings::Level::Level(value)| value),
            headphones_source: port
                .hp_select
                .map(|pa::usb_port_settings::HpSelect::HpSelect(value)| value),
            plugged: port
                .plugged
                .map(|pa::usb_port_settings::Plugged::Plugged(value)| value),
            dry_wet: port
                .dry_wet
                .map(|pa::usb_port_settings::DryWet::DryWet(value)| value),
        },
    );
    let midi =
        settings.midi_port.map(
            |pa::port_settings::MidiPort::MidiPort(port)| MidiPortSettings {
                thru: port
                    .midi_thru
                    .map(|pa::midi_port_settings::MidiThru::MidiThru(value)| value),
            },
        );

    Ok(IoSettings {
        inputs,
        outputs,
        expression_ports,
        headphones,
        usb,
        midi,
        xlr12_linked: message
            .xlr1_2_linked
            .map(|pa::io_settings_message::Xlr12Linked::Xlr12Linked(value)| value),
        out34_linked: message
            .out3_4_linked
            .map(|pa::io_settings_message::Out34Linked::Out34Linked(value)| value),
    })
}

/// Decodes the Global EQ settings.
///
/// Parameters whose index is outside 0..=27 are dropped. Fails if the payload cannot be decoded or the message
/// action is not UPDATE.
pub fn decode_global_eq(payload: &[u8]) -> Result<GlobalEqSettings, ResponseDecodeError> {
    let message: pa::GlobalEqMessage = decode_reply(payload)?;
    require_update_action(message.action, "Global EQ action is not UPDATE")?;
    Ok(GlobalEqSettings {
        parameters: message
            .parameters
            .into_iter()
            // Every signed parameter index outside the 28-entry Global EQ
            // model is out of range and must not be exposed.
            .filter(|parameter| (0..=27).contains(&parameter.parameter_index))
            .map(|parameter| GlobalEqParameter {
                parameter_index: parameter.parameter_index,
                value: parameter.value,
            })
            .collect(),
        bypassed: message
            .bypassed
            .map(|pa::global_eq_message::Bypassed::Bypassed(value)| value),
    })
}

/// Decodes the list of modes the device cycles through.
///
/// Fails if the payload cannot be decoded, the message action is not UPDATE, or the mode list is absent or empty.
pub fn decode_mode_cycle(payload: &[u8]) -> Result<ModeCycle, ResponseDecodeError> {
    let message: pa::ModeMessage = decode_reply(payload)?;
    require_update_action(message.action, "mode action is not UPDATE")?;
    let slots = message
        .available_modes
        .map(|pa::mode_message::AvailableModes::AvailableModes(value)| value.modes)
        .filter(|values| !values.is_empty())
        .ok_or(ResponseDecodeError::Incomplete("mode cycle"))?;
    Ok(ModeCycle { slots })
}

/// Decodes a looper status push.
///
/// The fields drawn from the status group are always `Some`; the others are `Some` only when the message carries
/// them. Fails if the payload cannot be decoded, the message action is not UPDATE, or the status group is absent.
pub fn decode_looper_status(payload: &[u8]) -> Result<LooperStatus, ResponseDecodeError> {
    let message: pa::LooperMessage = decode_reply(payload)?;
    require_update_action(message.action, "looper action is not UPDATE")?;
    let status = message
        .status
        .map(|pa::looper_message::Status::Status(value)| value)
        .ok_or(ResponseDecodeError::Incomplete("looper status"))?;
    Ok(LooperStatus {
        state: Some(status.state),
        progress: Some(status.progress),
        undo_progress: Some(status.undo_progress),
        duplicate_cycle: Some(status.duplicate_cycle),
        num_duplicate_cycles: Some(status.num_duplicate_cycles),
        one_shot_stopped: Some(status.one_shot_stopped > 0),
        redo_available: Some(status.redo_available > 0),
        loop_length: Some(status.loop_length),
        free_samples: Some(status.free_samples),
        in_reverse: Some(status.in_reverse > 0),
        one_shot: Some(status.one_shot > 0),
        half_speed: Some(status.half_speed > 0),
        fixed_duplicate_cycles: Some(status.fixed_duplicate_cycles > 0),
        armed: Some(status.armed > 0),
        waiting_for_cycle: Some(status.waiting_for_cycle > 0),
        undo_count: Some(status.undo_count),
        max_write_displacement: Some(status.max_write_displacement),
        min_write_displacement: Some(status.min_write_displacement),
        events_waiting_for_quantize: Some(status.events_waiting_for_quantize),
        current_clock: Some(status.current_clock),
        transition: Some(status.transition),
        action: Some(status.action),
        one_shot_play: message
            .one_shot_play
            .map(|pa::looper_message::OneShotPlay::OneShotPlay(value)| value),
        sync_start_waiting: message
            .sync_start_waiting
            .map(|pa::looper_message::SyncStartWaiting::SyncStartWaiting(value)| value),
        quantize_enabled: message
            .quantize_enabled
            .map(|pa::looper_message::QuantizeEnabled::QuantizeEnabled(value)| value),
        update_type: message
            .update_type
            .map(|pa::looper_message::UpdateType::UpdateType(value)| value)
            .filter(|value| matches!(*value, 0 | 1)),
    })
}

fn tempo_value(parameter: &Param) -> Option<f32> {
    parameter
        .param_values
        .first()
        .and_then(|value| match value.value {
            Some(param_value::Value::FloatValue(value)) if (0.0..=1.0).contains(&value) => {
                Some(value)
            }
            _ => None,
        })
}

fn discrete(value: f32, labels: &[&str]) -> Option<String> {
    if !(0.0..=1.0).contains(&value) || labels.is_empty() {
        return None;
    }
    let index = (value * (labels.len() - 1) as f32).round() as usize;
    labels.get(index).map(|label| (*label).to_string())
}

/// Decode the preset/global TempoControl parameter vector. Unknown parameter 23
/// is intentionally ignored; the 23 attributed parameters are preserved here.
pub fn decode_tempo_settings(parameters: &[Param]) -> TempoSettings {
    decode_tempo_values(parameters.iter().map(|parameter| {
        // Sparse GlobalTempo publications use the protobuf scalar default.
        let index = match parameter.index {
            Some(param::Index::Index(index)) => index as usize,
            None => 0,
        };
        (index, parameter)
    }))
}

fn decode_tempo_values<'a>(parameters: impl Iterator<Item = (usize, &'a Param)>) -> TempoSettings {
    const SIGNATURES: &[&str] = &[
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
    const QUARTER_SUBDIVISIONS: &[&str] = &["1/4", "1/8", "1/8T", "1/16"];
    const EIGHTH_SUBDIVISIONS: &[&str] = &["1/8", "1/16", "1/16T", "1/32"];
    const SOUNDS: &[&str] = &[
        "BLIP", "BLOCK", "COWBELL", "DIGITAL", "DRUM KIT", "SOFT KIT",
    ];
    const ROUTING: &[&str] = &["MULTI", "HP", "OUT 1/2", "OUT 3/4", "SEND 1/2"];
    const BEATS: &[&str] = &["NORMAL", "MUTE", "ACCENT 1", "ACCENT 2"];
    let mut values = [None; 23];
    for (index, parameter) in parameters {
        if index < values.len() {
            values[index] = tempo_value(parameter);
        }
    }
    let signature_index = values[6]
        .filter(|value| (0.0..=1.0).contains(value))
        .map(|value| (value * (SIGNATURES.len() - 1) as f32).round() as usize)
        .filter(|index| *index < SIGNATURES.len());
    let time_signature = signature_index.map(|index| SIGNATURES[index].to_string());
    let subdivision = values[7].and_then(|value| {
        signature_index.and_then(|index| {
            discrete(
                value,
                if index < 12 {
                    QUARTER_SUBDIVISIONS
                } else {
                    EIGHTH_SUBDIVISIONS
                },
            )
        })
    });
    TempoSettings {
        mode: None,
        bpm: values[0].map(crate::commands::tempo_from_normalized),
        global_bpm: None,
        led_enabled: values[2].map(|value| value >= 0.5),
        // The unit's own taper, not linear: see commands::TEMPO_VOLUME_KNOTS.
        volume_db: values[3].map(crate::commands::tempo_volume_db_from_raw),
        running: values[4].map(|value| value >= 0.5),
        pan: values[5].map(|value| -1.0 + 2.0 * value),
        time_signature,
        subdivision,
        sound: values[8].and_then(|value| discrete(value, SOUNDS)),
        routing: values[9].and_then(|value| discrete(value, ROUTING)),
        beats: values[10..23]
            .iter()
            .filter_map(|value| value.and_then(|value| discrete(value, BEATS)))
            .collect(),
    }
}

/// Decode only the device-wide PRESET/GLOBAL switch from a GlobalTempo params
/// push. Clock-only messages are rejected so callers can wait for the right shape.
pub fn decode_tempo_mode(payload: &[u8]) -> Result<TempoModeSettings, ResponseDecodeError> {
    let message: pa::GlobalTempoMessage = decode_reply(payload)?;
    require_update_action(message.action, "GlobalTempo action is not UPDATE")?;
    if message.request_id.is_some() {
        return Err(ResponseDecodeError::Mismatch("GlobalTempo request id"));
    }
    let value = message
        .params
        .iter()
        .find_map(|parameter| {
            matches!(parameter.index, Some(param::Index::Index(1)))
                .then(|| tempo_value(parameter))?
        })
        .ok_or(ResponseDecodeError::Incomplete(
            "GlobalTempo mode parameter",
        ))?;
    let mode = match value {
        value if (value - 0.0).abs() < 0.000_01 => "PRESET",
        value if (value - 1.0).abs() < 0.000_01 => "GLOBAL",
        _ => return Err(ResponseDecodeError::Mismatch("GlobalTempo mode value")),
    };
    Ok(TempoModeSettings { mode: mode.into() })
}

/// Decodes the global tempo settings, including the device-wide PRESET/GLOBAL mode.
///
/// Replies that carry a request id are rejected, because only uncorrelated state pushes are accepted here. Fails
/// if the payload cannot be decoded, the message action is not UPDATE, there are no parameters, or the BPM or
/// mode parameter is missing or invalid.
pub fn decode_global_tempo_settings(payload: &[u8]) -> Result<TempoSettings, ResponseDecodeError> {
    let message: pa::GlobalTempoMessage = decode_reply(payload)?;
    require_update_action(message.action, "GlobalTempo action is not UPDATE")?;
    // Request-id-bearing envelopes are excluded from this state projection.
    // The type-33 READ itself is uncorrelated, so accepting one here would
    // let an unrelated correlated exchange satisfy this public read.
    if message.request_id.is_some() {
        return Err(ResponseDecodeError::Mismatch("GlobalTempo request id"));
    }
    if message.params.is_empty() {
        return Err(ResponseDecodeError::Incomplete("GlobalTempo parameters"));
    }
    let mut settings = decode_tempo_settings(&message.params);
    settings.mode = Some(decode_tempo_mode(payload)?.mode);
    if settings.bpm.is_none() {
        return Err(ResponseDecodeError::Incomplete("GlobalTempo BPM parameter"));
    }
    Ok(settings)
}

/// Decode the loaded preset's TempoControl block from a preset read. The read is the bare
/// type-15 READ Cortex Control sends at boot (no request id), so any full-preset UPDATE after it
/// qualifies. PRESET/GLOBAL mode is device-wide and therefore composed separately from a
/// GlobalTempo reply by the native host.
pub fn decode_preset_tempo_settings(payload: &[u8]) -> Result<TempoSettings, ResponseDecodeError> {
    let message: pa::RecallPresetMessage = decode_reply(payload)?;
    require_update_action(message.action, "current-preset action is not UPDATE")?;
    let preset = match message.preset {
        Some(pa::recall_preset_message::Preset::Preset(value)) => value,
        None => return Err(ResponseDecodeError::Incomplete("current preset")),
    };
    let tempo = preset
        .tempo_program_data
        .first()
        .ok_or(ResponseDecodeError::Incomplete("preset TempoControl"))?;
    // A full BinaryPreset contains an ordered model parameter vector. CorOS
    // replies omit every index in the 24-entry TempoControl block.
    // This differs from sparse GlobalTempo messages, where absent means zero.
    Ok(decode_tempo_values(tempo.params.iter().enumerate().map(
        |(position, parameter)| {
            let index = match parameter.index {
                Some(param::Index::Index(index)) => index as usize,
                None => position,
            };
            (index, parameter)
        },
    )))
}

/// Decodes which global modules are inhibited.
///
/// Fails if the payload cannot be decoded, the message action is not UPDATE, or the reply carries neither field.
pub fn decode_inhibited_modules(payload: &[u8]) -> Result<InhibitedModules, ResponseDecodeError> {
    let message: pa::CompilerInhibitedModulesMessage = decode_reply(payload)?;
    require_update_action(message.action, "inhibited-modules action is not UPDATE")?;
    let global_gate = message.global_gate.map(|value| match value {
        pa::compiler_inhibited_modules_message::GlobalGate::GlobalGate(value) => value,
    });
    let global_eq = message.global_eq.map(|value| match value {
        pa::compiler_inhibited_modules_message::GlobalEq::GlobalEq(value) => value,
    });
    if global_gate.is_none() && global_eq.is_none() {
        return Err(ResponseDecodeError::Incomplete("global_gate or global_eq"));
    }
    Ok(InhibitedModules {
        global_gate,
        global_eq,
    })
}

fn png_image(bytes: Vec<u8>) -> Result<PngImage, ResponseDecodeError> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24
        || !bytes.starts_with(SIGNATURE)
        || bytes.get(8..12) != Some(13_u32.to_be_bytes().as_slice())
        || bytes.get(12..16) != Some(b"IHDR")
    {
        return Err(ResponseDecodeError::InvalidPng);
    }
    let width = bytes
        .get(16..20)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_be_bytes)
        .filter(|value| *value != 0)
        .ok_or(ResponseDecodeError::InvalidPng)?;
    let height = bytes
        .get(20..24)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_be_bytes)
        .filter(|value| *value != 0)
        .ok_or(ResponseDecodeError::InvalidPng)?;
    Ok(PngImage {
        width,
        height,
        bytes,
    })
}

/// Decode the legacy message-25 schema as a device-only candidate response.
/// This schema is not part of the current protocol surface; the codec exists
/// for forward compatibility only and is not confirmed device support.
pub fn decode_preset_screenshot(
    payload: &[u8],
    expected_request_id: u64,
    folder_name: &str,
    position: u32,
    is_factory: bool,
) -> Result<PngImage, ResponseDecodeError> {
    let message: pa::ScreenshotMessage = decode_reply(payload)?;
    let request_id = match message.request_id {
        Some(pa::screenshot_message::RequestId::RequestId(value)) => value,
        None => return Err(ResponseDecodeError::Incomplete("request id")),
    };
    let expected_position = i32::try_from(position)
        .map_err(|_| ResponseDecodeError::Mismatch("preset screenshot position overflow"))?;
    if request_id != expected_request_id
        || message.folder_name != folder_name
        || message.index != expected_position
        || message.is_factory != is_factory
    {
        return Err(ResponseDecodeError::Mismatch("preset screenshot target"));
    }
    let bytes = match message.png {
        Some(pa::screenshot_message::Png::Png(value)) => value,
        None => return Err(ResponseDecodeError::Incomplete("PNG payload")),
    };
    png_image(bytes)
}

/// Decode the message-72 screenshot schema as a device-only candidate response.
/// This RemoteControl path is not part of the confirmed protocol surface;
/// runtime capability evidence is required before treating this codec as
/// confirmed device support.
pub fn decode_captured_screen(payload: &[u8]) -> Result<PngImage, ResponseDecodeError> {
    let message: pa::RemoteControlMessage = decode_reply(payload)?;
    require_update_action(message.action, "screen action is not UPDATE")?;
    let bytes = message
        .screenshot
        .and_then(|value| value.payload)
        .ok_or(ResponseDecodeError::Incomplete("screen PNG payload"))?;
    // A gzip-wrapped protobuf may expand up to MAX_INFLATED_BYTES before prost
    // materializes this bytes field. Keep the retained framebuffer itself at
    // the ordinary logical-frame limit rather than returning a much larger
    // allocation to either host.
    if bytes.len() > profile::MAX_FRAME_BYTES {
        return Err(ResponseDecodeError::OversizedReply);
    }
    let image = png_image(bytes)?;
    // The device extension returns the complete CorOS framebuffer. Keep this
    // constraint local to capture_screen: preset screenshots intentionally use
    // a different height and the generic PNG parser serves both operations.
    if (image.width, image.height) != (800, 480) {
        return Err(ResponseDecodeError::Mismatch(
            "captured screen is not the 800x480 framebuffer",
        ));
    }
    Ok(image)
}

fn dsp_diagnostics(
    value: pa::DspCommsDiagnosticsMessage,
) -> Result<DspDiagnostics, ResponseDecodeError> {
    if [
        value.process_cpu,
        value.process_cpu_max,
        value.internal_heap_usage,
        value.external_heap_usage,
        value.internal_pm_heap_usage,
        value.ext_dma_usage,
    ]
    .into_iter()
    .any(|value| !value.is_finite())
    {
        return Err(ResponseDecodeError::Mismatch(
            "diagnostics floating-point value is not finite",
        ));
    }
    Ok(DspDiagnostics {
        status1: value.status1,
        status2: value.status2,
        status3: value.status3,
        status4: value.status4,
        process_cpu: value.process_cpu,
        process_cpu_max: value.process_cpu_max,
        internal_heap_usage: value.internal_heap_usage,
        external_heap_usage: value.external_heap_usage,
        internal_pm_heap_usage: value.internal_pm_heap_usage,
        external_dma_usage: value.ext_dma_usage,
        error_flags: value.error_flags,
    })
}

fn usb_diagnostics(value: pa::Soc2armCommsDiagnosticsMessage) -> UsbDiagnostics {
    UsbDiagnostics {
        connected: value.usb_connected,
        input_read_index: value.in_read_idx,
        input_write_index: value.in_write_idx,
        output_read_index: value.out_read_idx,
        output_write_index: value.out_write_idx,
        audio_rx_frames: value.usb_audio_rx_frame,
        audio_tx_frames: value.usb_audio_tx_frame,
        audio_tx_frames_skipped: value.usb_audio_tx_frame_skipped,
        audio_tx_frames_aborted: value.usb_audio_tx_frame_aborted,
        audio_output_distance_average: value.usb_audio_out_rd_wr_distance_average,
        audio_min_samples: value.usb_audio_rd_wr_min_num_samples,
        audio_max_samples: value.usb_audio_rd_wr_max_num_samples,
        audio_plus_one_sample_count: value.usb_audio_plus_one_sample_count,
        audio_minus_one_sample_count: value.usb_audio_minus_one_sample_count,
        audio_feedback_state: value.usb_audio_feedback_state,
        audio_streaming_endpoint_enabled: value.usb_audio_streaming_ep_enabled,
        audio_streaming_endpoint_disabled: value.usb_audio_streaming_ep_disabled,
        midi_out_count: value.usb_midi_out_count,
        midi_in_count: value.usb_midi_in_count,
        hid_in_count: value.usbhid_in_count,
        hid_out_count: value.usbhid_out_count,
        hid_in_dropped_count: value.usbhid_in_dropped_count,
        hid_out_dropped_count: value.usbhid_out_dropped_count,
        audio_input_distance_average: value.usb_audio_in_rd_wr_distance_average,
        audio_output_gaps_detected: value.usb_audio_out_gaps_detected,
        audio_output_gap_resets: value.usb_audio_out_gap_resets,
        audio_output_buffer_resets: value.usb_audio_out_buffer_resets,
    }
}

/// Decode message 7 without enabling any production-automation facilities.
pub fn decode_diagnostics(
    payload: &[u8],
    expected_request_id: Option<u64>,
) -> Result<DeviceDiagnostics, ResponseDecodeError> {
    use pa::diagnostics_message as dm;
    let message: pa::DiagnosticsMessage = decode_reply(payload)?;
    require_update_action(message.action, "diagnostics action is not UPDATE")?;
    let request_id = message
        .request_id
        .map(|dm::RequestId::RequestId(value)| value);
    if expected_request_id.is_some() && request_id != expected_request_id {
        return Err(ResponseDecodeError::Mismatch("diagnostics request id"));
    }
    Ok(DeviceDiagnostics {
        request_id,
        soc1_core1: message
            .soc1_core1_diagnostics
            .map(|dm::Soc1Core1Diagnostics::Soc1Core1Diagnostics(value)| dsp_diagnostics(value))
            .transpose()?,
        soc1_core2: message
            .soc1_core2_diagnostics
            .map(|dm::Soc1Core2Diagnostics::Soc1Core2Diagnostics(value)| dsp_diagnostics(value))
            .transpose()?,
        soc2_core1: message
            .soc2_core1_diagnostics
            .map(|dm::Soc2Core1Diagnostics::Soc2Core1Diagnostics(value)| dsp_diagnostics(value))
            .transpose()?,
        soc2_core2: message
            .soc2_core2_diagnostics
            .map(|dm::Soc2Core2Diagnostics::Soc2Core2Diagnostics(value)| dsp_diagnostics(value))
            .transpose()?,
        soc2_arm: message.soc2arm_core_diagnostics.map(
            |dm::Soc2armCoreDiagnostics::Soc2armCoreDiagnostics(value)| usb_diagnostics(value),
        ),
        soc1_to_soc2_dropped_count: message
            .soc1_to_soc2_dropped_count
            .map(|dm::Soc1ToSoc2DroppedCount::Soc1ToSoc2DroppedCount(value)| value),
    })
}

/// Decode a candidate device `zenUI` widget tree; see [`crate::commands::read_graphics_tree`].
///
/// The payload is indented text, not a structured message, so it is returned
/// verbatim for a caller to parse or diff. This RemoteControl path is an
/// audited host extension outside the confirmed protocol surface: it is not
/// established for general device behavior, request-ID semantics, or
/// behavior across firmware versions.
pub fn decode_graphics_tree(payload: &[u8]) -> Result<String, ResponseDecodeError> {
    let message: pa::RemoteControlMessage = decode_reply(payload)?;
    require_update_action(message.action, "graphics tree action is not UPDATE")?;
    let tree = message
        .graphics_tree
        .and_then(|value| value.payload)
        .ok_or(ResponseDecodeError::Incomplete("graphics tree payload"))?;
    if tree.len() > profile::MAX_FRAME_BYTES {
        return Err(ResponseDecodeError::OversizedReply);
    }
    if tree.trim().is_empty() {
        return Err(ResponseDecodeError::Mismatch(
            "graphics tree payload is empty",
        ));
    }
    if tree.contains('\0') {
        return Err(ResponseDecodeError::Mismatch(
            "graphics tree payload contains NUL",
        ));
    }
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_update_action_accepts_update_and_rejects_everything_else() {
        assert!(require_update_action(pa::message_action::Enum::Update as i32, "family").is_ok());
        assert!(matches!(
            require_update_action(pa::message_action::Enum::Read as i32, "family"),
            Err(ResponseDecodeError::Mismatch("family"))
        ));
        assert!(matches!(
            require_update_action(pa::message_action::Enum::Create as i32, "other-family"),
            Err(ResponseDecodeError::Mismatch("other-family"))
        ));
    }

    #[test]
    fn sparse_io_publications_merge_addressed_ports_and_preserve_other_families() {
        let envelope = |settings| {
            pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                settings: Some(pa::io_settings_message::Settings::Settings(settings)),
                ..Default::default()
            }
            .encode_to_vec()
        };
        let original = envelope(pa::PortSettings {
            in_port: vec![pa::InputPortSettings {
                input_port_id: 0,
                level: Some(pa::input_port_settings::Level::Level(0.5)),
                ..Default::default()
            }],
            out_port: vec![pa::OutputPortSettings {
                output_port_id: 4,
                level: Some(pa::output_port_settings::Level::Level(0.4)),
                mute: Some(pa::output_port_settings::Mute::Mute(true)),
                ..Default::default()
            }],
            hp_port: Some(pa::port_settings::HpPort::HpPort(pa::HeadphonesSettings {
                hp_feed: vec![pa::HeadphonesFeedLevel {
                    output_port_id: 4,
                    level: 1.0,
                }],
                ..Default::default()
            })),
            ..Default::default()
        });
        let input = envelope(pa::PortSettings {
            in_port: vec![pa::InputPortSettings {
                input_port_id: 0,
                level: Some(pa::input_port_settings::Level::Level(0.0)),
                ..Default::default()
            }],
            ..Default::default()
        });
        let changed_input = merge_io_settings_publication(&original, &input).unwrap();
        let output = envelope(pa::PortSettings {
            out_port: vec![pa::OutputPortSettings {
                output_port_id: 4,
                mute: Some(pa::output_port_settings::Mute::Mute(false)),
                ..Default::default()
            }],
            hp_port: Some(pa::port_settings::HpPort::HpPort(pa::HeadphonesSettings {
                hp_feed: vec![pa::HeadphonesFeedLevel {
                    output_port_id: 4,
                    level: 0.0,
                }],
                ..Default::default()
            })),
            ..Default::default()
        });
        let changed_output = merge_io_settings_publication(&changed_input, &output).unwrap();
        let repeated = merge_io_settings_publication(&changed_output, &output).unwrap();
        let projected = decode_io_settings(&repeated).unwrap();
        assert_eq!(projected.inputs.len(), 1);
        assert_eq!(projected.inputs[0].level, Some(0.0));
        assert_eq!(projected.outputs.len(), 1);
        assert_eq!(projected.outputs[0].muted, Some(false));
        assert_eq!(projected.outputs[0].level, Some(0.4));
        let headphones = projected.headphones.unwrap();
        assert_eq!(headphones.feeds.len(), 1);
        assert_eq!(headphones.feeds[0].level, 0.0);
        assert_eq!(repeated, changed_output);
        let read = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(merge_io_settings_publication(&original, &read).is_err());
        assert!(merge_io_settings_publication(&original, &[0xff]).is_err());
    }

    #[test]
    fn sparse_settings_replace_complete_boolean_groups_including_all_false() {
        use pa::general_settings_message as gs;
        let original = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            led_brightness: Some(gs::LedBrightness::LedBrightness(32)),
            master_volume_assignment: Some(gs::MasterVolumeAssignment::MasterVolumeAssignment(
                pa::MasterVolumeAssignmentOptions {
                    out12: true,
                    out34: true,
                    send12: true,
                    headphones: true,
                },
            )),
            global_bypass_cab: Some(gs::GlobalBypassCab::GlobalBypassCab(pa::GlobalBypassRows {
                row1: true,
                row2: true,
                row3: true,
                row4: true,
            })),
            global_bypass_ir: Some(gs::GlobalBypassIr::GlobalBypassIr(pa::GlobalBypassRows {
                row1: true,
                row2: true,
                row3: true,
                row4: true,
            })),
            ..Default::default()
        };
        for bits in 0..16 {
            let expected = pa::MasterVolumeAssignmentOptions {
                out12: bits & 1 != 0,
                out34: bits & 2 != 0,
                send12: bits & 4 != 0,
                headphones: bits & 8 != 0,
            };
            let update = pa::GeneralSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                master_volume_assignment: Some(gs::MasterVolumeAssignment::MasterVolumeAssignment(
                    expected,
                )),
                ..Default::default()
            };
            let result = merge_general_settings_publication(
                &original.encode_to_vec(),
                &update.encode_to_vec(),
            )
            .unwrap();
            let result = pa::GeneralSettingsMessage::decode(result.as_slice()).unwrap();
            assert_eq!(
                result.master_volume_assignment,
                update.master_volume_assignment
            );
            assert_eq!(result.global_bypass_cab, original.global_bypass_cab);
            assert_eq!(result.global_bypass_ir, original.global_bypass_ir);
            assert_eq!(result.led_brightness, original.led_brightness);
        }
        let empty_groups = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            global_bypass_cab: Some(gs::GlobalBypassCab::GlobalBypassCab(
                pa::GlobalBypassRows::default(),
            )),
            global_bypass_ir: Some(gs::GlobalBypassIr::GlobalBypassIr(
                pa::GlobalBypassRows::default(),
            )),
            ..Default::default()
        };
        let result = merge_general_settings_publication(
            &original.encode_to_vec(),
            &empty_groups.encode_to_vec(),
        )
        .unwrap();
        let result = pa::GeneralSettingsMessage::decode(result.as_slice()).unwrap();
        assert_eq!(result.global_bypass_cab, empty_groups.global_bypass_cab);
        assert_eq!(result.global_bypass_ir, empty_groups.global_bypass_ir);
        assert_eq!(
            result.master_volume_assignment,
            original.master_volume_assignment
        );
        let read = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        };
        assert!(merge_general_settings_publication(
            &original.encode_to_vec(),
            &read.encode_to_vec()
        )
        .is_err());
        assert!(merge_general_settings_publication(&original.encode_to_vec(), &[0xff]).is_err());
    }

    // Regression: `merge` appends a repeated field's incoming items onto
    // whatever is already retained (protobuf merge semantics), unlike the
    // Option fields above, which are explicitly cleared first. Without the
    // same clear, looper_stomp_assignments/download_destination_folders grew
    // by one duplicate copy of their entries every time a GeneralSettings
    // frame republished, even when its repeated fields never changed.
    #[test]
    fn merging_general_settings_replaces_repeated_fields_instead_of_appending() {
        let original = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            looper_stomp_assignments: vec![1, 2, 3],
            download_destination_folders: vec![pa::DownloadDestinationFolder {
                key: "captures".into(),
                r#type: 0,
            }],
            ..Default::default()
        };

        // A republication carrying the exact same repeated values must not
        // duplicate them.
        let same_again = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            looper_stomp_assignments: vec![1, 2, 3],
            download_destination_folders: vec![pa::DownloadDestinationFolder {
                key: "captures".into(),
                r#type: 0,
            }],
            ..Default::default()
        };
        let result = merge_general_settings_publication(
            &original.encode_to_vec(),
            &same_again.encode_to_vec(),
        )
        .unwrap();
        let result = pa::GeneralSettingsMessage::decode(result.as_slice()).unwrap();
        assert_eq!(result.looper_stomp_assignments, vec![1, 2, 3]);
        assert_eq!(
            result.download_destination_folders,
            same_again.download_destination_folders
        );

        // A republication carrying different repeated values must fully
        // replace the retained ones, not merge onto them.
        let changed = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            looper_stomp_assignments: vec![4],
            download_destination_folders: vec![pa::DownloadDestinationFolder {
                key: "irs".into(),
                r#type: 1,
            }],
            ..Default::default()
        };
        let result =
            merge_general_settings_publication(&original.encode_to_vec(), &changed.encode_to_vec())
                .unwrap();
        let result = pa::GeneralSettingsMessage::decode(result.as_slice()).unwrap();
        assert_eq!(result.looper_stomp_assignments, vec![4]);
        assert_eq!(
            result.download_destination_folders,
            changed.download_destination_folders
        );
    }

    // Regression: the anti-duplication clear above was originally
    // unconditional, but proto3 repeated fields carry no presence bit, so an
    // update that OMITS these two fields entirely is wire-identical to one
    // that explicitly empties them. An unconditional clear before merge wiped
    // previously-retained looper_stomp_assignments/download_destination_folders
    // on any unrelated sparse GeneralSettings echo (e.g. one that only
    // republishes master_volume_assignment), even though that echo never
    // mentioned these two fields.
    #[test]
    fn merging_general_settings_preserves_repeated_fields_on_an_unrelated_sparse_echo() {
        let original = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            looper_stomp_assignments: vec![1, 2, 3],
            download_destination_folders: vec![pa::DownloadDestinationFolder {
                key: "captures".into(),
                r#type: 0,
            }],
            ..Default::default()
        };

        // A sparse echo that carries an unrelated field and says nothing
        // about the two repeated fields must leave them untouched.
        let unrelated_echo = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            master_volume_assignment: Some(
                pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                    pa::MasterVolumeAssignmentOptions {
                        out12: true,
                        out34: false,
                        send12: false,
                        headphones: true,
                    },
                ),
            ),
            ..Default::default()
        };
        let result = merge_general_settings_publication(
            &original.encode_to_vec(),
            &unrelated_echo.encode_to_vec(),
        )
        .unwrap();
        let result = pa::GeneralSettingsMessage::decode(result.as_slice()).unwrap();
        assert_eq!(
            result.looper_stomp_assignments, original.looper_stomp_assignments,
            "an unrelated sparse echo must not wipe retained looper_stomp_assignments"
        );
        assert_eq!(
            result.download_destination_folders, original.download_destination_folders,
            "an unrelated sparse echo must not wipe retained download_destination_folders"
        );
    }

    // Sparse presence mirrors the two independent Cortex property updates.
    #[test]
    fn inhibited_modules_preserve_independent_presence_and_boolean_polarity() {
        let gate_only = pa::CompilerInhibitedModulesMessage {
            action: pa::message_action::Enum::Update as i32,
            global_gate: Some(
                pa::compiler_inhibited_modules_message::GlobalGate::GlobalGate(false),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        let decoded = decode_inhibited_modules(&gate_only).unwrap();
        assert_eq!(decoded.global_gate, Some(false));
        assert_eq!(decoded.global_eq, None);
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::json!({"globalGate": false})
        );

        let eq_only = pa::CompilerInhibitedModulesMessage {
            action: pa::message_action::Enum::Update as i32,
            global_eq: Some(pa::compiler_inhibited_modules_message::GlobalEq::GlobalEq(
                true,
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let decoded = decode_inhibited_modules(&eq_only).unwrap();
        assert_eq!(decoded.global_gate, None);
        assert_eq!(decoded.global_eq, Some(true));
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::json!({"globalEq": true})
        );

        let empty = pa::CompilerInhibitedModulesMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_inhibited_modules(&empty),
            Err(ResponseDecodeError::Incomplete("global_gate or global_eq"))
        ));
        let read = pa::CompilerInhibitedModulesMessage {
            action: pa::message_action::Enum::Read as i32,
            global_gate: Some(pa::compiler_inhibited_modules_message::GlobalGate::GlobalGate(true)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_inhibited_modules(&read),
            Err(ResponseDecodeError::Mismatch(
                "inhibited-modules action is not UPDATE"
            ))
        ));
    }

    #[test]
    fn current_preset_ack_requires_update_and_preset_and_accepts_the_bare_reads_reply() {
        // The host's read is the bare type-15 READ, so its reply need not carry an id.
        let bare = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(decode_current_preset_ack(&bare).unwrap().request_id, None);
        let payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recall_preset_message::RequestId::RequestId(42)),
            preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_current_preset_ack(&payload).unwrap().request_id,
            Some(42)
        );
        let zero_id = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recall_preset_message::RequestId::RequestId(0)),
            preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_current_preset_ack(&zero_id).unwrap().request_id,
            Some(0)
        );
        let no_preset = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_current_preset_ack(&no_preset),
            Err(ResponseDecodeError::Incomplete("current preset"))
        ));

        for action in [
            pa::message_action::Enum::Create as i32,
            pa::message_action::Enum::Delete as i32,
            pa::message_action::Enum::Read as i32,
            99,
        ] {
            let wrong_action = pa::RecallPresetMessage {
                action,
                request_id: Some(pa::recall_preset_message::RequestId::RequestId(42)),
                preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(matches!(
                decode_current_preset_ack(&wrong_action),
                Err(ResponseDecodeError::Mismatch(
                    "current-preset action is not UPDATE"
                ))
            ));
        }

        let missing = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recall_preset_message::RequestId::RequestId(42)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_current_preset_ack(&missing),
            Err(ResponseDecodeError::Incomplete("current preset"))
        ));

        assert!(matches!(
            decode_current_preset_ack(&[0x80]),
            Err(ResponseDecodeError::Protobuf(_))
        ));
        assert!(matches!(
            decode_current_preset_ack(&vec![0; profile::MAX_FRAME_BYTES + 1]),
            Err(ResponseDecodeError::OversizedReply)
        ));

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &payload).unwrap();
        let compressed = encoder.finish().unwrap();
        assert_eq!(
            decode_current_preset_ack(&compressed).unwrap().request_id,
            Some(42)
        );

        let inflated = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recall_preset_message::RequestId::RequestId(42)),
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset {
                    name: Some(binary_preset::Name::Name(
                        "x".repeat(profile::MAX_INFLATED_BYTES + 1),
                    )),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &inflated).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            decode_current_preset_ack(&compressed),
            Err(ResponseDecodeError::InflatedLimit)
        ));
    }
    use proptest::prelude::*;

    /// One byte more than the transport can deliver for `message_type`.
    fn oversized_reply(message_type: u16) -> Vec<u8> {
        vec![0; crate::compression::max_delivered_payload_bytes_for(message_type) + 1]
    }

    #[test]
    fn an_inflated_file_listing_past_the_raw_ceiling_decodes_as_a_reply() {
        // decode_reply bounds a reply by its own type's limit, so a
        // File listing the transport inflated past 1 MiB is decoded, not
        // rejected as OversizedReply.
        let listing = crate::state::large_file_listing(profile::MAX_FRAME_BYTES + 1);
        assert!(listing.len() > profile::MAX_FRAME_BYTES);
        let message: pa::FileMessage = decode_reply(&listing).expect("within the File limit");
        assert!(matches!(
            message.folder,
            Some(pa::file_message::Folder::Folder(ref folder)) if folder.files.len() > 1_000
        ));
        // The bound still applies, per type.
        assert!(matches!(
            decode_reply::<pa::FileMessage>(&oversized_reply(profile::MESSAGE_TYPE_FILE)),
            Err(ResponseDecodeError::OversizedReply)
        ));
        // A small type stays at the raw ceiling, not its smaller inflation
        // limit: an uncompressed envelope of any type may carry up to 1 MiB.
        assert_eq!(
            crate::compression::max_delivered_payload_bytes_for(profile::MESSAGE_TYPE_SCENE),
            profile::MAX_FRAME_BYTES
        );
        assert!(matches!(
            decode_selected_scene(&oversized_reply(profile::MESSAGE_TYPE_SCENE)),
            Err(ResponseDecodeError::OversizedReply)
        ));
    }

    #[test]
    fn recalled_preset_name_hides_generated_oneof_wrappers() {
        let payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset {
                    name: Some(binary_preset::Name::Name("Shared preset".into())),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_recalled_preset_name(&payload).unwrap().as_deref(),
            Some("Shared preset")
        );
        assert_eq!(
            decode_recalled_preset_name(
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn selected_scene_hides_generated_presence_wrappers() {
        let payload = pa::SceneMessage {
            action: pa::message_action::Enum::Update as i32,
            selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(6)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(decode_selected_scene(&payload).unwrap(), Some(6));
        assert_eq!(
            decode_selected_scene(
                &pa::SceneMessage {
                    action: pa::message_action::Enum::Update as i32,
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn device_identity_requires_update_and_serial_presence() {
        let payload = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            device_serial_number: Some(
                pa::version_message::DeviceSerialNumber::DeviceSerialNumber("serial-1".into()),
            ),
            app_fw_version: Some(pa::version_message::AppFwVersion::AppFwVersion(
                "4.1.0".into(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let identity = decode_device_identity(&payload).unwrap();
        assert_eq!(identity.serial, "serial-1");
        assert_eq!(identity.app_fw_version.as_deref(), Some("4.1.0"));

        let missing_serial = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_device_identity(&missing_serial),
            Err(ResponseDecodeError::Incomplete("device serial number"))
        ));

        let wrong_action = pa::VersionMessage {
            action: pa::message_action::Enum::Read as i32,
            device_serial_number: Some(
                pa::version_message::DeviceSerialNumber::DeviceSerialNumber("serial-1".into()),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_device_identity(&wrong_action),
            Err(ResponseDecodeError::Mismatch(
                "identity action is not UPDATE"
            ))
        ));
    }

    /// Synthetic schema fixture; it is not traffic evidence.
    #[test]
    fn graphics_tree_returns_the_device_widget_text_verbatim() {
        let tree = "zenUI::RootGraphicsItem\n  zenUI::Grid\n    text : 'In\n1'\n";
        let payload = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            graphics_tree: Some(pa::RemoteControlGraphicsTree {
                payload: Some(tree.to_string()),
            }),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(decode_graphics_tree(&payload).unwrap(), tree);

        let wrong_action = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Read as i32,
            graphics_tree: Some(pa::RemoteControlGraphicsTree {
                payload: Some(tree.to_string()),
            }),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_graphics_tree(&wrong_action),
            Err(ResponseDecodeError::Mismatch(
                "graphics tree action is not UPDATE"
            ))
        ));
        assert!(matches!(
            decode_graphics_tree(&[0xff]),
            Err(ResponseDecodeError::Protobuf(_))
        ));
        assert!(matches!(
            decode_graphics_tree(&oversized_reply(profile::MESSAGE_TYPE_REMOTE_CONTROL)),
            Err(ResponseDecodeError::OversizedReply)
        ));

        // A screenshot reply carries no tree, and must not be reported as one.
        let screenshot = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            screenshot: Some(pa::RemoteControlScreenshot {
                payload: Some(vec![0x89, 0x50, 0x4E, 0x47]),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_graphics_tree(&screenshot),
            Err(ResponseDecodeError::Incomplete("graphics tree payload"))
        ));

        for invalid in ["", "  \r\n\t", "zenUI::Root\0hidden"] {
            let payload = pa::RemoteControlMessage {
                action: pa::message_action::Enum::Update as i32,
                graphics_tree: Some(pa::RemoteControlGraphicsTree {
                    payload: Some(invalid.into()),
                }),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(matches!(
                decode_graphics_tree(&payload),
                Err(ResponseDecodeError::Mismatch(_))
            ));
        }

        let oversized_message = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            graphics_tree: Some(pa::RemoteControlGraphicsTree {
                payload: Some("x".repeat(profile::MAX_FRAME_BYTES + 1)),
            }),
            ..Default::default()
        }
        .encode_to_vec();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &oversized_message).unwrap();
        let oversized = encoder.finish().unwrap();
        assert!(oversized.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            decode_graphics_tree(&oversized),
            Err(ResponseDecodeError::OversizedReply)
        ));

        let inflated_message = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            graphics_tree: Some(pa::RemoteControlGraphicsTree {
                payload: Some("x".repeat(profile::MAX_INFLATED_BYTES + 1)),
            }),
            ..Default::default()
        }
        .encode_to_vec();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &inflated_message).unwrap();
        let inflated = encoder.finish().unwrap();
        assert!(inflated.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            decode_graphics_tree(&inflated),
            Err(ResponseDecodeError::InflatedLimit)
        ));
    }

    #[test]
    fn diagnostics_preserve_dsp_and_usb_transport_counters() {
        use pa::diagnostics_message as dm;
        let dsp = pa::DspCommsDiagnosticsMessage {
            process_cpu: 0.42,
            process_cpu_max: 0.75,
            internal_heap_usage: 0.2,
            external_heap_usage: 0.3,
            internal_pm_heap_usage: 0.4,
            ext_dma_usage: 0.5,
            error_flags: 0x40,
            ..Default::default()
        };
        let usb = pa::Soc2armCommsDiagnosticsMessage {
            usb_connected: 1,
            usb_audio_rx_frame: 101,
            usb_audio_tx_frame: 102,
            usb_audio_tx_frame_skipped: 3,
            usb_audio_tx_frame_aborted: 4,
            usb_midi_out_count: 201,
            usb_midi_in_count: 202,
            usbhid_in_count: 301,
            usbhid_out_count: 302,
            usbhid_in_dropped_count: 5,
            usbhid_out_dropped_count: 6,
            usb_audio_in_rd_wr_distance_average: 7,
            usb_audio_out_gaps_detected: 8,
            usb_audio_out_gap_resets: 9,
            usb_audio_out_buffer_resets: 10,
            ..Default::default()
        };
        let payload = pa::DiagnosticsMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(dm::RequestId::RequestId(77)),
            soc1_core1_diagnostics: Some(dm::Soc1Core1Diagnostics::Soc1Core1Diagnostics(dsp)),
            soc2arm_core_diagnostics: Some(dm::Soc2armCoreDiagnostics::Soc2armCoreDiagnostics(usb)),
            soc1_to_soc2_dropped_count: Some(dm::Soc1ToSoc2DroppedCount::Soc1ToSoc2DroppedCount(9)),
            ..Default::default()
        }
        .encode_to_vec();
        let decoded = decode_diagnostics(&payload, Some(77)).unwrap();
        assert_eq!(decoded.soc1_core1.unwrap().error_flags, 0x40);
        let arm = decoded.soc2_arm.unwrap();
        assert_eq!(arm.hid_out_dropped_count, 6);
        assert_eq!(arm.audio_input_distance_average, 7);
        assert_eq!(arm.audio_output_gaps_detected, 8);
        assert_eq!(arm.audio_output_gap_resets, 9);
        assert_eq!(arm.audio_output_buffer_resets, 10);
        assert_eq!(decoded.soc1_to_soc2_dropped_count, Some(9));
        assert!(matches!(
            decode_diagnostics(&payload, Some(78)),
            Err(ResponseDecodeError::Mismatch("diagnostics request id"))
        ));
    }

    #[test]
    fn diagnostics_are_correlated_sparse_bounded_and_json_safe() {
        use pa::diagnostics_message as dm;

        let sparse = pa::DiagnosticsMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(dm::RequestId::RequestId(12)),
            ..Default::default()
        }
        .encode_to_vec();
        let decoded = decode_diagnostics(&sparse, Some(12)).unwrap();
        assert_eq!(decoded.request_id, Some(12));
        assert!(decoded.soc1_core1.is_none());
        assert!(decoded.soc2_arm.is_none());

        let wrong_action = pa::DiagnosticsMessage {
            action: pa::message_action::Enum::Read as i32,
            request_id: Some(dm::RequestId::RequestId(12)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_diagnostics(&wrong_action, Some(12)),
            Err(ResponseDecodeError::Mismatch(
                "diagnostics action is not UPDATE"
            ))
        ));

        let non_finite = pa::DiagnosticsMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(dm::RequestId::RequestId(12)),
            soc1_core1_diagnostics: Some(dm::Soc1Core1Diagnostics::Soc1Core1Diagnostics(
                pa::DspCommsDiagnosticsMessage {
                    process_cpu: f32::NAN,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_diagnostics(&non_finite, Some(12)),
            Err(ResponseDecodeError::Mismatch(
                "diagnostics floating-point value is not finite"
            ))
        ));
        assert!(matches!(
            decode_diagnostics(
                &oversized_reply(profile::MESSAGE_TYPE_DIAGNOSTICS),
                Some(12)
            ),
            Err(ResponseDecodeError::OversizedReply)
        ));
    }

    #[test]
    fn tuner_settings_are_complete_and_expose_absolute_reference() {
        let payload = pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(5)),
            frequency: Some(pa::tuner_message::Frequency::Frequency(2.0)),
            mute: Some(pa::tuner_message::Mute::Mute(true)),
            enable_meter: Some(pa::tuner_message::EnableMeter::EnableMeter(true)),
            meter: Some(pa::tuner_message::Meter::Meter(-0.25)),
            ..Default::default()
        }
        .encode_to_vec();
        let settings = decode_tuner_settings(&payload).unwrap();
        assert_eq!(settings.input_port_id, 5);
        assert_eq!(settings.reference_offset_hz, 2.0);
        assert_eq!(settings.reference_hz, 442.0);
        assert!(settings.muted);
        assert_eq!(settings.meter_enabled, Some(true));
        assert_eq!(settings.meter, Some(-0.25));

        for input_port_id in [0, 6, 7, 10] {
            let invalid = pa::TunerMessage {
                action: pa::message_action::Enum::Update as i32,
                input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(input_port_id)),
                frequency: Some(pa::tuner_message::Frequency::Frequency(0.0)),
                mute: Some(pa::tuner_message::Mute::Mute(false)),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(matches!(
                decode_tuner_settings(&invalid),
                Err(ResponseDecodeError::Mismatch("unknown tuner input_port_id"))
            ));
        }

        let non_finite_meter = pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(1)),
            frequency: Some(pa::tuner_message::Frequency::Frequency(0.0)),
            mute: Some(pa::tuner_message::Mute::Mute(false)),
            meter: Some(pa::tuner_message::Meter::Meter(f32::NAN)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_tuner_settings(&non_finite_meter),
            Err(ResponseDecodeError::Mismatch("tuner meter is not finite"))
        ));

        let live_meter_only = pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            meter: Some(pa::tuner_message::Meter::Meter(0.5)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_tuner_settings(&live_meter_only),
            Err(ResponseDecodeError::Incomplete("tuner input_port_id"))
        ));

        for reference_offset_hz in [-40.01, 40.01, f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
            let invalid = pa::TunerMessage {
                action: pa::message_action::Enum::Update as i32,
                input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(5)),
                frequency: Some(pa::tuner_message::Frequency::Frequency(reference_offset_hz)),
                mute: Some(pa::tuner_message::Mute::Mute(false)),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(matches!(
                decode_tuner_settings(&invalid),
                Err(ResponseDecodeError::Mismatch(
                    "tuner frequency offset is outside -40..40 Hz"
                ))
            ));
        }

        let incomplete = pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(5)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_tuner_settings(&incomplete),
            Err(ResponseDecodeError::Incomplete(_))
        ));

        let without_meter_state = pa::TunerMessage {
            action: pa::message_action::Enum::Update as i32,
            input_port_id: Some(pa::tuner_message::InputPortId::InputPortId(5)),
            frequency: Some(pa::tuner_message::Frequency::Frequency(2.0)),
            mute: Some(pa::tuner_message::Mute::Mute(true)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_tuner_settings(&without_meter_state)
                .unwrap()
                .meter_enabled,
            None
        );
    }

    #[test]
    fn general_settings_preserve_sparse_presence_and_derive_hold_milliseconds() {
        let payload = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            screen_brightness: Some(
                pa::general_settings_message::ScreenBrightness::ScreenBrightness(59),
            ),
            led_brightness: Some(pa::general_settings_message::LedBrightness::LedBrightness(
                50,
            )),
            dimmed_led_brightness: Some(
                pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(25),
            ),
            scene_block_bypass: Some(
                pa::general_settings_message::SceneBlockBypass::SceneBlockBypass(1),
            ),
            hold_timing: Some(pa::general_settings_message::HoldTiming::HoldTiming(3)),
            master_volume_assignment: Some(
                pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                    pa::MasterVolumeAssignmentOptions {
                        out12: true,
                        out34: false,
                        send12: true,
                        headphones: false,
                    },
                ),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        let settings = decode_general_settings(&payload).unwrap();
        assert_eq!(settings.screen_brightness, Some(19));
        assert_eq!(settings.led_brightness, Some(16));
        assert_eq!(settings.dimmed_led_brightness, Some(8));
        // The raw wire values are published unchanged beside the derived steps.
        assert_eq!(settings.screen_brightness_wire, Some(59));
        assert_eq!(settings.led_brightness_wire, Some(50));
        assert_eq!(settings.dimmed_led_brightness_wire, Some(25));
        assert_eq!(
            settings.scene_bypass_behavior.as_deref(),
            Some("nonstompOverwrite")
        );
        assert_eq!(settings.hold_timing_index, Some(3));
        assert_eq!(settings.hold_timing_ms, Some(800));
        assert!(settings.master_volume_assignment.unwrap().send12);

        let raw_dimmed_maximum = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            dimmed_led_brightness: Some(
                pa::general_settings_message::DimmedLedBrightness::DimmedLedBrightness(100),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_general_settings(&raw_dimmed_maximum)
                .unwrap()
                .dimmed_led_brightness,
            Some(31),
            "CorOS clamps a raw dimmed-brightness 100 write to 97; public reads must never invent step 32"
        );

        let sparse = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            midi_channel: Some(pa::general_settings_message::MidiChannel::MidiChannel(17)),
            ..Default::default()
        }
        .encode_to_vec();
        let settings = decode_general_settings(&sparse).unwrap();
        assert_eq!(settings.midi_channel, Some(17));
        assert_eq!(settings.scene_bypass_behavior, None);

        let empty = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            serde_json::to_value(decode_general_settings(&empty).unwrap()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn io_settings_preserve_every_sparse_port_field() {
        let payload = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    in_port: vec![pa::InputPortSettings {
                        input_port_id: 1,
                        level: Some(pa::input_port_settings::Level::Level(0.5)),
                        input_zmode: Some(pa::input_port_settings::InputZmode::InputZmode(0.75)),
                        plugged: Some(pa::input_port_settings::Plugged::Plugged(true)),
                        ..Default::default()
                    }],
                    out_port: vec![pa::OutputPortSettings {
                        output_port_id: 4,
                        mute: Some(pa::output_port_settings::Mute::Mute(true)),
                        ..Default::default()
                    }],
                    hp_port: Some(pa::port_settings::HpPort::HpPort(pa::HeadphonesSettings {
                        hp_feed: vec![pa::HeadphonesFeedLevel {
                            level: 0.25,
                            output_port_id: 4,
                        }],
                        level: Some(pa::headphones_settings::Level::Level(0.6)),
                        plugged: Some(pa::headphones_settings::Plugged::Plugged(true)),
                    })),
                    usb_port: Some(pa::port_settings::UsbPort::UsbPort(pa::UsbPortSettings {
                        hp_select: Some(pa::usb_port_settings::HpSelect::HpSelect(0.5)),
                        dry_wet: Some(pa::usb_port_settings::DryWet::DryWet(1.0)),
                        ..Default::default()
                    })),
                    midi_port: Some(pa::port_settings::MidiPort::MidiPort(
                        pa::MidiPortSettings {
                            midi_thru: Some(pa::midi_port_settings::MidiThru::MidiThru(1.0)),
                        },
                    )),
                    exp_port: vec![pa::ExpPortSettings {
                        exp_port_id: 1,
                        level: Some(pa::exp_port_settings::Level::Level(0.33)),
                        calibrating: Some(pa::exp_port_settings::Calibrating::Calibrating(false)),
                        ..Default::default()
                    }],
                },
            )),
            xlr1_2_linked: Some(pa::io_settings_message::Xlr12Linked::Xlr12Linked(true)),
            out3_4_linked: Some(pa::io_settings_message::Out34Linked::Out34Linked(false)),
            ..Default::default()
        }
        .encode_to_vec();

        let settings = decode_io_settings(&payload).unwrap();
        assert_eq!(settings.inputs[0].input_port_id, 1);
        assert_eq!(settings.inputs[0].level, Some(0.5));
        assert_eq!(settings.inputs[0].impedance, Some(0.75));
        assert_eq!(settings.inputs[0].plugged, Some(true));
        assert_eq!(settings.outputs[0].muted, Some(true));
        assert_eq!(
            settings.headphones.as_ref().unwrap().feeds[0].output_port_id,
            4
        );
        assert_eq!(settings.usb.as_ref().unwrap().dry_wet, Some(1.0));
        assert_eq!(settings.midi.as_ref().unwrap().thru, Some(1.0));
        assert_eq!(settings.expression_ports[0].expression_port_id, 1);
        assert_eq!(settings.expression_ports[0].level, Some(0.33));
        assert_eq!(settings.xlr12_linked, Some(true));
        assert_eq!(settings.out34_linked, Some(false));

        let incomplete = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_io_settings(&incomplete),
            Err(ResponseDecodeError::Incomplete("input ports"))
        ));

        let missing_settings = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_io_settings(&missing_settings),
            Err(ResponseDecodeError::Incomplete("I/O settings"))
        ));

        let read_reply = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Read as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    in_port: vec![pa::InputPortSettings {
                        input_port_id: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_io_settings(&read_reply),
            Err(ResponseDecodeError::Mismatch(
                "I/O settings action is not UPDATE"
            ))
        ));
    }

    #[test]
    fn tagged_io_settings_are_not_retained_or_folded_but_sparse_untagged_ones_are() {
        let full = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    in_port: vec![pa::InputPortSettings {
                        input_port_id: 1,
                        ..Default::default()
                    }],
                    out_port: vec![
                        pa::OutputPortSettings {
                            output_port_id: 1,
                            ..Default::default()
                        },
                        pa::OutputPortSettings {
                            output_port_id: 4,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let sparse = |request_id: Option<u64>| {
            pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: request_id.map(pa::io_settings_message::RequestId::RequestId),
                settings: Some(pa::io_settings_message::Settings::Settings(
                    pa::PortSettings {
                        out_port: vec![pa::OutputPortSettings {
                            output_port_id: 4,
                            mute: Some(pa::output_port_settings::Mute::Mute(true)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
            .encode_to_vec()
        };

        assert!(settings_publication_is_retainable(
            profile::MESSAGE_TYPE_IO_SETTINGS,
            &full
        ));
        assert!(settings_publication_is_retainable(
            profile::MESSAGE_TYPE_IO_SETTINGS,
            &sparse(None)
        ));
        // Only the I/O family carries this receiver guard.
        assert!(settings_publication_is_retainable(
            profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            &sparse(Some(7))
        ));
        for request_id in [0, 7] {
            assert!(!settings_publication_is_retainable(
                profile::MESSAGE_TYPE_IO_SETTINGS,
                &sparse(Some(request_id))
            ));
            assert!(matches!(
                merge_io_settings_publication(&full, &sparse(Some(request_id))),
                Err(ResponseDecodeError::Incomplete("I/O settings request_id"))
            ));
        }

        // An untagged sparse echo still folds: the other ports survive.
        let folded =
            decode_io_settings(&merge_io_settings_publication(&full, &sparse(None)).unwrap())
                .unwrap();
        assert_eq!(folded.inputs.len(), 1);
        assert_eq!(folded.outputs.len(), 2);
        assert_eq!(
            folded
                .outputs
                .iter()
                .find(|port| port.output_port_id == 4)
                .unwrap()
                .muted,
            Some(true)
        );
    }

    #[test]
    fn io_settings_echo_accepts_sparse_output_and_pairing_but_rejects_unowned_ids() {
        let update_without_settings = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_io_settings_echo(&update_without_settings),
            Err(ResponseDecodeError::Incomplete("I/O settings"))
        ));

        let pairing_only_update = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            xlr1_2_linked: Some(pa::io_settings_message::Xlr12Linked::Xlr12Linked(false)),
            out3_4_linked: Some(pa::io_settings_message::Out34Linked::Out34Linked(true)),
            ..Default::default()
        }
        .encode_to_vec();
        let pairing = decode_io_settings_echo(&pairing_only_update).unwrap();
        assert!(pairing.inputs.is_empty());
        assert!(pairing.outputs.is_empty());
        assert_eq!(pairing.xlr12_linked, Some(false));
        assert_eq!(pairing.out34_linked, Some(true));
        assert!(matches!(
            decode_io_settings(&pairing_only_update),
            Err(ResponseDecodeError::Incomplete("I/O settings"))
        ));

        let output_only_update = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    out_port: vec![pa::OutputPortSettings {
                        output_port_id: 4,
                        mute: Some(pa::output_port_settings::Mute::Mute(true)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();

        let echo = decode_io_settings_echo(&output_only_update).unwrap();
        assert!(echo.inputs.is_empty());
        assert_eq!(echo.outputs.len(), 1);
        assert_eq!(echo.outputs[0].output_port_id, 4);
        assert_eq!(echo.outputs[0].muted, Some(true));
        assert!(echo.expression_ports.is_empty());
        assert_eq!(echo.headphones, None);
        assert_eq!(echo.usb, None);
        assert_eq!(echo.midi, None);
        assert_eq!(echo.xlr12_linked, None);
        assert_eq!(echo.out34_linked, None);
        assert!(matches!(
            decode_io_settings(&output_only_update),
            Err(ResponseDecodeError::Incomplete("input ports"))
        ));

        for request_id in [0, 7] {
            let tagged_update = pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: Some(pa::io_settings_message::RequestId::RequestId(request_id)),
                settings: Some(pa::io_settings_message::Settings::Settings(
                    pa::PortSettings {
                        out_port: vec![pa::OutputPortSettings {
                            output_port_id: 4,
                            mute: Some(pa::output_port_settings::Mute::Mute(true)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(matches!(
                decode_io_settings_echo(&tagged_update),
                Err(ResponseDecodeError::Incomplete("I/O settings request_id"))
            ));
            assert!(matches!(
                decode_io_settings(&tagged_update),
                Err(ResponseDecodeError::Incomplete("I/O settings request_id"))
            ));
        }

        let read_reply = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Read as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    out_port: vec![pa::OutputPortSettings {
                        output_port_id: 4,
                        mute: Some(pa::output_port_settings::Mute::Mute(true)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_io_settings_echo(&read_reply),
            Err(ResponseDecodeError::Mismatch(
                "I/O settings action is not UPDATE"
            ))
        ));
    }

    #[test]
    fn rejects_non_png_screen_payloads() {
        let payload = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            screenshot: Some(pa::RemoteControlScreenshot {
                payload: Some(vec![1, 2, 3]),
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_captured_screen(&payload),
            Err(ResponseDecodeError::InvalidPng)
        ));
    }

    #[test]
    fn captured_screen_requires_update_payload_and_exact_framebuffer_bounds() {
        let png = |width: u32, height: u32| {
            let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&height.to_be_bytes());
            bytes
        };
        let reply = |action, screenshot| {
            pa::RemoteControlMessage {
                action,
                screenshot,
                ..Default::default()
            }
            .encode_to_vec()
        };

        let valid = reply(
            pa::message_action::Enum::Update as i32,
            Some(pa::RemoteControlScreenshot {
                payload: Some(png(800, 480)),
                ..Default::default()
            }),
        );
        assert_eq!(
            (
                decode_captured_screen(&valid).unwrap().width,
                decode_captured_screen(&valid).unwrap().height
            ),
            (800, 480)
        );

        // Capture has no host correlation: an optional device request_id and
        // unrelated/crop fields neither identify nor invalidate the reply.
        let tolerated_optional_fields = pa::RemoteControlMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(999),
            mouse: Some(pa::RemoteControlMouse::default()),
            screenshot: Some(pa::RemoteControlScreenshot {
                payload: Some(png(800, 480)),
                x: Some(7),
                y: Some(8),
                w: Some(9),
                h: Some(10),
            }),
            graphics_tree: Some(pa::RemoteControlGraphicsTree {
                payload: Some("ignored".into()),
            }),
        }
        .encode_to_vec();
        assert_eq!(
            decode_captured_screen(&tolerated_optional_fields)
                .unwrap()
                .bytes,
            png(800, 480)
        );

        let wrong_size = reply(
            pa::message_action::Enum::Update as i32,
            Some(pa::RemoteControlScreenshot {
                payload: Some(png(800, 384)),
                ..Default::default()
            }),
        );
        assert!(matches!(
            decode_captured_screen(&wrong_size),
            Err(ResponseDecodeError::Mismatch(
                "captured screen is not the 800x480 framebuffer"
            ))
        ));
        assert!(matches!(
            decode_captured_screen(&reply(pa::message_action::Enum::Read as i32, None)),
            Err(ResponseDecodeError::Mismatch("screen action is not UPDATE"))
        ));
        assert!(matches!(
            decode_captured_screen(&reply(pa::message_action::Enum::Update as i32, None)),
            Err(ResponseDecodeError::Incomplete("screen PNG payload"))
        ));
        let oversized = reply(
            pa::message_action::Enum::Update as i32,
            Some(pa::RemoteControlScreenshot {
                payload: Some(vec![0; profile::MAX_FRAME_BYTES + 1]),
                ..Default::default()
            }),
        );
        assert!(matches!(
            decode_captured_screen(&oversized),
            Err(ResponseDecodeError::OversizedReply)
        ));

        // The outer compressed frame can fit below 1 MiB while prost would
        // otherwise expose an image allocation above the logical-frame bound.
        let compressible = reply(
            pa::message_action::Enum::Update as i32,
            Some(pa::RemoteControlScreenshot {
                payload: Some({
                    let mut bytes = png(800, 480);
                    bytes.resize(profile::MAX_FRAME_BYTES + 1, 0);
                    bytes
                }),
                ..Default::default()
            }),
        );
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &compressible).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            decode_captured_screen(&compressed),
            Err(ResponseDecodeError::OversizedReply)
        ));
    }

    #[test]
    fn png_dimensions_come_from_the_shared_ihdr_projection() {
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&800_u32.to_be_bytes());
        png.extend_from_slice(&480_u32.to_be_bytes());
        let image = png_image(png).unwrap();
        assert_eq!((image.width, image.height), (800, 480));
        assert!(png_image(b"not an image".to_vec()).is_err());
    }

    #[test]
    fn tempo_clock_and_chunked_backup_are_projected_once_for_every_host() {
        let tempo = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            metronome_status: Some(pa::global_tempo_message::MetronomeStatus::MetronomeStatus(
                pa::MetronomeStatusUpdate {
                    is_enabled: 7,
                    preroll_enabled: 0,
                    current_beat: 2,
                    current_bar: 3,
                    current_tick: 4,
                    count_in_beats_remaining: -1,
                    count_in_bars_remaining: 5,
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_tempo_clock(&tempo).unwrap(),
            Some(TempoClock {
                enabled: true,
                preroll_enabled: false,
                current_beat: 2,
                current_bar: 3,
                current_tick: 4,
                count_in_beats_remaining: -1,
                count_in_bars_remaining: 5,
            })
        );

        let correlated = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::global_tempo_message::RequestId::RequestId(9)),
            metronome_status: Some(pa::global_tempo_message::MetronomeStatus::MetronomeStatus(
                pa::MetronomeStatusUpdate {
                    current_tick: 99,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            tempo,
            [
                0x08, 0x01, 0x22, 0x15, 0x08, 0x07, 0x18, 0x02, 0x20, 0x03, 0x28, 0x04, 0x30, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 0x38, 0x05,
            ]
        );
        assert_eq!(decode_tempo_clock(&correlated).unwrap(), None);
        let missing = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(decode_tempo_clock(&missing).unwrap(), None);

        let mut backup = BackupAssembler::default();
        let first = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "{\"type\":\"backup\",".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(false)),
            ..Default::default()
        }
        .encode_to_vec();
        let last = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "\"creator\":\"quad\"}".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(true)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(backup.push(&first).unwrap(), None);
        assert_eq!(
            backup.push(&last).unwrap().as_deref(),
            Some("{\"type\":\"backup\",\"creator\":\"quad\"}")
        );

        let stale_tail = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "old-tail".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(true)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(backup.push(&stale_tail).unwrap(), None);
        assert_eq!(backup.push(&first).unwrap(), None);
        assert_eq!(
            backup.push(&last).unwrap().as_deref(),
            Some("{\"type\":\"backup\",\"creator\":\"quad\"}")
        );
    }

    #[test]
    fn decoded_bpm_matches_the_domain_scaled_inverse_of_normalized_tempo() {
        let parameter = |value: f32| Param {
            index: Some(param::Index::Index(0)),
            param_values: vec![crate::proto::ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        };
        // The old implementation hardcoded `(40.0 + 200.0 * value).round() as u32`;
        // these are that literal formula's outputs for the boundary and midpoint
        // of the normalized 0..1 range, which the domain-constant-derived
        // formula must still reproduce exactly.
        assert_eq!(
            decode_tempo_settings(&[parameter(0.0)]).bpm,
            Some(40),
            "value=0.0 must decode to MINIMUM_TEMPO_BPM"
        );
        assert_eq!(
            decode_tempo_settings(&[parameter(1.0)]).bpm,
            Some(240),
            "value=1.0 must decode to MAXIMUM_TEMPO_BPM"
        );
        assert_eq!(
            decode_tempo_settings(&[parameter(0.5)]).bpm,
            Some(140),
            "value=0.5 must decode to the old literal formula's (40.0 + 200.0 * 0.5).round()"
        );
    }

    #[test]
    fn tempo_settings_project_every_named_option_and_reject_clock_only_mode_reads() {
        let parameter = |index: u32, value: f32| Param {
            index: Some(param::Index::Index(index)),
            param_values: vec![crate::proto::ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        };
        let params = vec![
            parameter(0, 0.4),
            parameter(1, 1.0),
            parameter(2, 1.0),
            parameter(3, 40.0 / 69.0),
            parameter(4, 0.0),
            parameter(5, 0.5),
            parameter(6, 0.1),
            parameter(7, 1.0 / 3.0),
            parameter(8, 0.4),
            parameter(9, 0.75),
            parameter(10, 2.0 / 3.0),
        ];
        let payload = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params,
            ..Default::default()
        }
        .encode_to_vec();
        let settings = decode_global_tempo_settings(&payload).unwrap();
        assert_eq!(settings.mode.as_deref(), Some("GLOBAL"));
        assert_eq!(settings.bpm, Some(120));
        assert_eq!(settings.pan, Some(0.0));
        assert_eq!(settings.time_signature.as_deref(), Some("4/4"));
        assert_eq!(settings.subdivision.as_deref(), Some("1/8"));
        assert_eq!(settings.sound.as_deref(), Some("COWBELL"));
        assert_eq!(settings.routing.as_deref(), Some("OUT 3/4"));
        assert_eq!(settings.beats, vec!["ACCENT 1"]);
        let eighth = decode_tempo_settings(&[parameter(6, 12.0 / 20.0), parameter(7, 0.0)]);
        assert_eq!(eighth.time_signature.as_deref(), Some("3/8"));
        assert_eq!(eighth.subdivision.as_deref(), Some("1/8"));
        let ambiguous_sparse = decode_tempo_settings(&[parameter(7, 0.0)]);
        assert_eq!(ambiguous_sparse.subdivision, None);
        let missing_index_defaults_to_zero = Param {
            index: None,
            param_values: vec![crate::proto::ParamValue {
                value: Some(param_value::Value::FloatValue(0.25)),
            }],
            ..Default::default()
        };
        let defaults = decode_tempo_settings(&[parameter(2, 1.0), missing_index_defaults_to_zero]);
        assert_eq!(defaults.bpm, Some(90));
        assert_eq!(defaults.led_enabled, Some(true));
        let duplicate_bpm = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params: vec![parameter(0, 0.25), parameter(1, 0.0), parameter(0, 0.5)],
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_global_tempo_settings(&duplicate_bpm).unwrap().bpm,
            Some(140)
        );
        let invalid_bpm = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params: vec![parameter(0, 1.01), parameter(1, 0.0)],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_global_tempo_settings(&invalid_bpm),
            Err(ResponseDecodeError::Incomplete("GlobalTempo BPM parameter"))
        ));
        let sparse_mode_echo = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params: vec![parameter(1, 1.0)],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_global_tempo_settings(&sparse_mode_echo),
            Err(ResponseDecodeError::Incomplete("GlobalTempo BPM parameter"))
        ));
        let correlated = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::global_tempo_message::RequestId::RequestId(42)),
            params: vec![parameter(0, 0.4), parameter(1, 1.0)],
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            correlated,
            [
                0x08, 0x01, 0x10, 0x2a, 0x1a, 0x09, 0x2a, 0x05, 0x15, 0xcd, 0xcc, 0xcc, 0x3e, 0x30,
                0x00, 0x1a, 0x09, 0x2a, 0x05, 0x15, 0x00, 0x00, 0x80, 0x3f, 0x30, 0x01,
            ]
        );
        assert!(matches!(
            decode_global_tempo_settings(&correlated),
            Err(ResponseDecodeError::Mismatch("GlobalTempo request id"))
        ));
        assert!(matches!(
            decode_tempo_mode(&correlated),
            Err(ResponseDecodeError::Mismatch("GlobalTempo request id"))
        ));
        let clock = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            metronome_status: Some(pa::global_tempo_message::MetronomeStatus::MetronomeStatus(
                Default::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_tempo_mode(&clock),
            Err(ResponseDecodeError::Incomplete(_))
        ));

        let preset_payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recall_preset_message::RequestId::RequestId(42)),
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset {
                    tempo_program_data: vec![crate::proto::Model {
                        params: vec![parameter(0, 0.355), parameter(1, 0.0), parameter(2, 0.0)],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let preset_settings = decode_preset_tempo_settings(&preset_payload).unwrap();
        assert_eq!(preset_settings.bpm, Some(111));
        assert_eq!(preset_settings.led_enabled, Some(false));
        assert_eq!(preset_settings.mode, None);
    }

    #[test]
    fn full_preset_tempo_uses_positions_while_sparse_global_tempo_defaults_to_zero() {
        let values = [
            0.4,
            0.0,
            1.0,
            0.5,
            0.0,
            0.5,
            0.1,
            0.0,
            0.0,
            0.0,
            2.0 / 3.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        let parameters: Vec<Param> = values
            .into_iter()
            .map(|value| Param {
                param_values: vec![
                    crate::proto::ParamValue {
                        value: Some(param_value::Value::FloatValue(value)),
                    };
                    8
                ],
                ..Default::default()
            })
            .collect();
        let sparse = decode_tempo_settings(&parameters);
        assert_eq!(sparse.bpm, Some(40));
        assert_eq!(sparse.led_enabled, None);
        let payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recall_preset_message::RequestId::RequestId(42)),
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset {
                    tempo_program_data: vec![crate::proto::Model {
                        params: parameters,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let full = decode_preset_tempo_settings(&payload).unwrap();
        assert_eq!(full.bpm, Some(120));
        assert_eq!(full.led_enabled, Some(true));
        // raw 0.5 is -3.4 dB on the unit's Tempo screen (it was -25.5 under
        // the old linear decode).
        assert!((full.volume_db.unwrap() - -3.4).abs() <= 0.15);
        assert_eq!(full.running, Some(false));
        assert_eq!(full.pan, Some(0.0));
        assert_eq!(full.time_signature.as_deref(), Some("4/4"));
        assert_eq!(full.subdivision.as_deref(), Some("1/4"));
        assert_eq!(full.sound.as_deref(), Some("BLIP"));
        assert_eq!(full.routing.as_deref(), Some("MULTI"));
        assert_eq!(full.beats.len(), 13);
        assert_eq!(full.beats[0], "ACCENT 1");
        assert!(full.beats[1..].iter().all(|beat| beat == "NORMAL"));
    }

    #[test]
    fn backup_ignores_an_uncorrelated_stale_tail_before_the_next_document() {
        let stale_tail = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "end-of-an-older-document".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(true)),
            ..Default::default()
        }
        .encode_to_vec();
        let valid = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "{\"type\":\"backup\",\"creator\":\"quad\"}".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(true)),
            ..Default::default()
        }
        .encode_to_vec();

        let mut backup = BackupAssembler::default();
        assert_eq!(backup.push(&stale_tail).unwrap(), None);
        assert!(!backup.started());
        assert_eq!(backup.ignored_prefix_chunks(), 1);
        assert_eq!(backup.ignored_prefix_terminators(), 1);
        assert_eq!(
            backup.push(&valid).unwrap().as_deref(),
            Some("{\"type\":\"backup\",\"creator\":\"quad\"}")
        );
    }

    #[test]
    fn backup_rejects_a_partial_document_instead_of_splicing_another_stream() {
        let first = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "{\"type\":\"backup\",".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(false)),
            ..Default::default()
        }
        .encode_to_vec();
        let broken_last = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "not-json".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(true)),
            ..Default::default()
        }
        .encode_to_vec();

        let mut backup = BackupAssembler::default();
        assert_eq!(backup.push(&first).unwrap(), None);
        assert_eq!(
            backup.push(&broken_last).unwrap_err().to_string(),
            "the QC backup stream ended with an incomplete or unsupported document"
        );
        assert!(!backup.started());
    }

    #[test]
    fn backup_accepts_only_the_expected_update_chunk_shape() {
        let create_echo = pa::LocalBackupMessage {
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "{\"type\":\"backup\",".into(),
            )),
            is_last_chunk: Some(pa::local_backup_message::IsLastChunk::IsLastChunk(false)),
            ..Default::default()
        }
        .encode_to_vec();
        let missing_terminal_presence = pa::LocalBackupMessage {
            action: pa::message_action::Enum::Update as i32,
            backup_json: Some(pa::local_backup_message::BackupJson::BackupJson(
                "{\"type\":\"backup\",".into(),
            )),
            ..Default::default()
        }
        .encode_to_vec();

        let mut backup = BackupAssembler::default();
        assert_eq!(backup.push(&create_echo).unwrap(), None);
        assert_eq!(backup.push(&missing_terminal_presence).unwrap(), None);
        assert!(!backup.started());
    }

    #[test]
    fn global_eq_mode_cycle_and_looper_reads_preserve_device_state() {
        let eq = pa::GlobalEqMessage {
            action: pa::message_action::Enum::Update as i32,
            parameters: vec![
                pa::GlobalEqParameter {
                    parameter_index: -1,
                    value: 0.1,
                },
                pa::GlobalEqParameter {
                    parameter_index: 6,
                    value: 0.75,
                },
                pa::GlobalEqParameter {
                    parameter_index: 25,
                    value: -0.25,
                },
                pa::GlobalEqParameter {
                    parameter_index: 27,
                    value: 2.0,
                },
                pa::GlobalEqParameter {
                    parameter_index: 28,
                    value: 0.9,
                },
            ],
            bypassed: Some(pa::global_eq_message::Bypassed::Bypassed(false)),
            ..Default::default()
        }
        .encode_to_vec();
        let eq = decode_global_eq(&eq).unwrap();
        assert_eq!(eq.parameters.len(), 3);
        assert_eq!(eq.parameters[0].parameter_index, 6);
        assert_eq!(eq.parameters[0].value, 0.75);
        assert_eq!(eq.parameters[1].parameter_index, 25);
        assert_eq!(eq.parameters[1].value, -0.25);
        assert_eq!(eq.parameters[2].parameter_index, 27);
        assert_eq!(eq.parameters[2].value, 2.0);
        assert_eq!(eq.bypassed, Some(false));

        let sparse = pa::GlobalEqMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec();
        let sparse = decode_global_eq(&sparse).unwrap();
        assert!(sparse.parameters.is_empty());
        assert_eq!(sparse.bypassed, None);

        let wrong_action = pa::GlobalEqMessage {
            action: pa::message_action::Enum::Read as i32,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_global_eq(&wrong_action),
            Err(ResponseDecodeError::Mismatch(
                "Global EQ action is not UPDATE"
            ))
        ));
        assert!(matches!(
            decode_global_eq(&[0x1a, 0x02, 0x08]),
            Err(ResponseDecodeError::Protobuf(_))
        ));

        let modes = pa::ModeMessage {
            action: pa::message_action::Enum::Update as i32,
            available_modes: Some(pa::mode_message::AvailableModes::AvailableModes(
                pa::AvailableModes {
                    modes: vec![7, 1, 2],
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(decode_mode_cycle(&modes).unwrap().slots, vec![7, 1, 2]);

        let looper = pa::LooperMessage {
            action: pa::message_action::Enum::Update as i32,
            status: Some(pa::looper_message::Status::Status(pa::LooperStatus {
                state: 3,
                progress: 0.5,
                in_reverse: 1,
                undo_count: 2,
                ..Default::default()
            })),
            one_shot_play: Some(pa::looper_message::OneShotPlay::OneShotPlay(true)),
            quantize_enabled: Some(pa::looper_message::QuantizeEnabled::QuantizeEnabled(false)),
            ..Default::default()
        }
        .encode_to_vec();
        let looper = decode_looper_status(&looper).unwrap();
        assert_eq!(looper.state, Some(3));
        assert_eq!(looper.progress, Some(0.5));
        assert_eq!(looper.in_reverse, Some(true));
        assert_eq!(looper.one_shot_play, Some(true));
    }

    #[test]
    fn mode_cycle_wire_decoder_rejects_wrong_action_and_presence_but_preserves_values() {
        let payload = |action, modes: Option<Vec<u32>>| {
            pa::ModeMessage {
                action,
                available_modes: modes.map(|modes| {
                    pa::mode_message::AvailableModes::AvailableModes(pa::AvailableModes { modes })
                }),
                ..Default::default()
            }
            .encode_to_vec()
        };

        let exact = payload(pa::message_action::Enum::Update as i32, Some(vec![8, 0, 6]));
        assert_eq!(
            exact,
            [0x08, 0x01, 0x22, 0x05, 0x0a, 0x03, 0x08, 0x00, 0x06]
        );
        assert_eq!(decode_mode_cycle(&exact).unwrap().slots, vec![8, 0, 6]);
        assert!(matches!(
            decode_mode_cycle(&payload(
                pa::message_action::Enum::Read as i32,
                Some(vec![0]),
            )),
            Err(ResponseDecodeError::Mismatch("mode action is not UPDATE"))
        ));
        for modes in [None, Some(vec![])] {
            assert!(matches!(
                decode_mode_cycle(&payload(pa::message_action::Enum::Update as i32, modes)),
                Err(ResponseDecodeError::Incomplete("mode cycle"))
            ));
        }
        for modes in [vec![9], vec![1, 1], vec![0, 1, 2, 3]] {
            assert_eq!(
                decode_mode_cycle(&payload(
                    pa::message_action::Enum::Update as i32,
                    Some(modes.clone())
                ))
                .unwrap()
                .slots,
                modes
            );
        }
    }

    #[test]
    fn looper_status_matches_the_expected_projection_and_sparse_outer_fields() {
        let payload = pa::LooperMessage {
            action: pa::message_action::Enum::Update as i32,
            status: Some(pa::looper_message::Status::Status(pa::LooperStatus {
                state: u32::MAX,
                progress: 0.25,
                undo_progress: 0.75,
                duplicate_cycle: 2,
                num_duplicate_cycles: 3,
                one_shot_stopped: 0,
                redo_available: 2,
                loop_length: u32::MAX,
                free_samples: 4,
                in_reverse: 1,
                one_shot: 0,
                half_speed: 7,
                fixed_duplicate_cycles: 0,
                armed: 1,
                waiting_for_cycle: 0,
                undo_count: 5,
                max_write_displacement: -0.5,
                min_write_displacement: 1.5,
                events_waiting_for_quantize: 6,
                current_clock: 7,
                transition: 8,
                action: 9,
            })),
            state: Some(pa::looper_message::State::State(10)),
            one_shot_play: Some(pa::looper_message::OneShotPlay::OneShotPlay(true)),
            sync_start_waiting: Some(pa::looper_message::SyncStartWaiting::SyncStartWaiting(
                false,
            )),
            quantize_enabled: Some(pa::looper_message::QuantizeEnabled::QuantizeEnabled(true)),
            update_type: Some(pa::looper_message::UpdateType::UpdateType(1)),
            ..Default::default()
        }
        .encode_to_vec();
        let status = decode_looper_status(&payload).unwrap();
        assert_eq!(status.state, Some(u32::MAX));
        assert_eq!(
            (status.progress, status.undo_progress),
            (Some(0.25), Some(0.75))
        );
        assert_eq!(
            (status.duplicate_cycle, status.num_duplicate_cycles),
            (Some(2), Some(3))
        );
        assert_eq!(
            (status.one_shot_stopped, status.redo_available),
            (Some(false), Some(true))
        );
        assert_eq!(
            (status.loop_length, status.free_samples),
            (Some(u32::MAX), Some(4))
        );
        assert_eq!(
            (status.in_reverse, status.one_shot, status.half_speed),
            (Some(true), Some(false), Some(true))
        );
        assert_eq!(
            (
                status.fixed_duplicate_cycles,
                status.armed,
                status.waiting_for_cycle
            ),
            (Some(false), Some(true), Some(false))
        );
        assert_eq!(status.undo_count, Some(5));
        assert_eq!(
            (status.max_write_displacement, status.min_write_displacement),
            (Some(-0.5), Some(1.5))
        );
        assert_eq!(
            (status.events_waiting_for_quantize, status.current_clock),
            (Some(6), Some(7))
        );
        assert_eq!((status.transition, status.action), (Some(8), Some(9)));
        assert_eq!(status.one_shot_play, Some(true));
        assert_eq!(status.sync_start_waiting, Some(false));
        assert_eq!(status.quantize_enabled, Some(true));
        assert_eq!(status.update_type, Some(1));

        let invalid_update_type = pa::LooperMessage {
            action: pa::message_action::Enum::Update as i32,
            status: Some(pa::looper_message::Status::Status(Default::default())),
            update_type: Some(pa::looper_message::UpdateType::UpdateType(2)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decode_looper_status(&invalid_update_type)
                .unwrap()
                .update_type,
            None
        );

        for invalid in [
            pa::LooperMessage::default().encode_to_vec(),
            pa::LooperMessage {
                action: pa::message_action::Enum::Update as i32,
                ..Default::default()
            }
            .encode_to_vec(),
        ] {
            assert!(decode_looper_status(&invalid).is_err());
        }
    }

    #[test]
    fn decoders_reject_non_update_response_envelopes() {
        let tempo = pa::GlobalTempoMessage::default().encode_to_vec();
        assert!(matches!(
            decode_tempo_clock(&tempo),
            Err(ResponseDecodeError::Mismatch(
                "tempo-clock action is not UPDATE"
            ))
        ));
        assert!(matches!(
            decode_tempo_mode(&tempo),
            Err(ResponseDecodeError::Mismatch(
                "GlobalTempo action is not UPDATE"
            ))
        ));

        let preset = pa::RecallPresetMessage::default().encode_to_vec();
        assert!(matches!(
            decode_recalled_preset_name(&preset),
            Err(ResponseDecodeError::Mismatch(
                "recalled-preset action is not UPDATE"
            ))
        ));
        assert!(matches!(
            decode_preset_tempo_settings(&preset),
            Err(ResponseDecodeError::Mismatch(
                "current-preset action is not UPDATE"
            ))
        ));

        let scene = pa::SceneMessage::default().encode_to_vec();
        assert!(matches!(
            decode_selected_scene(&scene),
            Err(ResponseDecodeError::Mismatch(
                "selected-scene action is not UPDATE"
            ))
        ));
    }

    #[test]
    fn preset_screenshot_requires_exact_echoed_target_and_png_presence() {
        let png = |width: u32, height: u32| {
            let mut bytes = vec![
                0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
                b'D', b'R',
            ];
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&height.to_be_bytes());
            bytes
        };
        let reply = |action: i32,
                     request_id: Option<u64>,
                     folder_name: &str,
                     index: i32,
                     is_factory: bool,
                     png: Option<Vec<u8>>|
         -> Vec<u8> {
            pa::ScreenshotMessage {
                action,
                request_id: request_id.map(pa::screenshot_message::RequestId::RequestId),
                folder_name: folder_name.into(),
                is_factory,
                index,
                png: png.map(pa::screenshot_message::Png::Png),
            }
            .encode_to_vec()
        };

        // The device reports an 800x384 screen. Action remains deliberately
        // unconstrained because this codec establishes no acceptance gate
        // on it.
        for action in [
            pa::message_action::Enum::Create as i32,
            pa::message_action::Enum::Update as i32,
            pa::message_action::Enum::Delete as i32,
            pa::message_action::Enum::Read as i32,
            99,
        ] {
            let decoded = decode_preset_screenshot(
                &reply(action, Some(91), "Live", 255, true, Some(png(800, 384))),
                91,
                "Live",
                255,
                true,
            )
            .unwrap();
            assert_eq!((decoded.width, decoded.height), (800, 384));
        }

        // The observation does not prove a universal dimension constraint.
        let other_dimensions = reply(0, Some(91), "Live", 255, true, Some(png(320, 180)));
        let decoded = decode_preset_screenshot(&other_dimensions, 91, "Live", 255, true).unwrap();
        assert_eq!((decoded.width, decoded.height), (320, 180));

        for request_id in [0, u64::MAX] {
            let payload = reply(0, Some(request_id), "Live", 255, true, Some(png(800, 384)));
            assert!(decode_preset_screenshot(&payload, request_id, "Live", 255, true).is_ok());
        }

        for payload in [
            reply(0, Some(92), "Live", 255, true, Some(png(800, 384))),
            reply(0, Some(91), "live", 255, true, Some(png(800, 384))),
            reply(0, Some(91), "Live", 254, true, Some(png(800, 384))),
            reply(0, Some(91), "Live", 255, false, Some(png(800, 384))),
        ] {
            assert!(matches!(
                decode_preset_screenshot(&payload, 91, "Live", 255, true),
                Err(ResponseDecodeError::Mismatch("preset screenshot target"))
            ));
        }
        assert!(matches!(
            decode_preset_screenshot(
                &reply(0, None, "Live", 255, true, Some(png(800, 384))),
                91,
                "Live",
                255,
                true,
            ),
            Err(ResponseDecodeError::Incomplete("request id"))
        ));
        assert!(matches!(
            decode_preset_screenshot(&other_dimensions, 91, "Live", u32::MAX, true),
            Err(ResponseDecodeError::Mismatch(
                "preset screenshot position overflow"
            ))
        ));
        assert!(matches!(
            decode_preset_screenshot(
                &reply(0, Some(91), "Live", 255, true, None),
                91,
                "Live",
                255,
                true,
            ),
            Err(ResponseDecodeError::Incomplete("PNG payload"))
        ));
        assert!(matches!(
            decode_preset_screenshot(
                &reply(0, Some(91), "Live", 255, true, Some(vec![0; 24])),
                91,
                "Live",
                255,
                true,
            ),
            Err(ResponseDecodeError::InvalidPng)
        ));
        assert!(matches!(
            decode_preset_screenshot(
                &oversized_reply(profile::MESSAGE_TYPE_SCREENSHOT),
                91,
                "Live",
                255,
                true
            ),
            Err(ResponseDecodeError::OversizedReply)
        ));

        let inflated = reply(
            0,
            Some(91),
            "Live",
            255,
            true,
            Some(vec![0; profile::MAX_INFLATED_BYTES + 1]),
        );
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &inflated).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            decode_preset_screenshot(&compressed, 91, "Live", 255, true),
            Err(ResponseDecodeError::InflatedLimit)
        ));
    }

    #[test]
    fn library_replies_are_correlated_and_preserve_device_metadata() {
        let recents = pa::RecentsFavoritesMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recents_favorites_message::RequestId::RequestId(71)),
            // A Favorites reply can omit the flag, leaving its protobuf
            // default false. Request correlation is authoritative.
            is_favorites: false,
            r#type: pa::recents_favorites_message::RecentFavoriteType::Preset as i32,
            items: vec![pa::RecentsFavoritesItem {
                name: "Stage".into(),
                folder_key: "/media/p4/Presets/Live".into(),
                folder_name: "Live".into(),
                is_factory: false,
                is_plugin: true,
                product_key: String::new(),
            }],
        }
        .encode_to_vec();
        let entries = decode_recents_favorites(&recents, 71).unwrap();
        assert_eq!(entries.entries.len(), 1);
        assert_eq!(entries.entries[0].name, "Stage");
        assert_eq!(entries.entries[0].product_key, None);
        assert!(entries.entries[0].is_plugin);
        assert!(matches!(
            decode_recents_favorites(&recents, 72),
            Err(ResponseDecodeError::Mismatch(_))
        ));

        let wrong_type = pa::RecentsFavoritesMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::recents_favorites_message::RequestId::RequestId(71)),
            r#type: pa::recents_favorites_message::RecentFavoriteType::ImpulseResponse as i32,
            items: vec![pa::RecentsFavoritesItem {
                name: "Room".into(),
                folder_key: "/media/p4/Impulse Responses".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_recents_favorites(&wrong_type, 71),
            Err(ResponseDecodeError::Mismatch(
                "recents/favorites reply is not the requested PRESET list"
            ))
        ));

        let pinned = pa::PinnedModelsMessage {
            models: vec![42, 84],
            captures: vec!["capture-a".into()],
            ..Default::default()
        }
        .encode_to_vec();
        let pinned = decode_pinned_models(&pinned).unwrap();
        assert_eq!(pinned.models, vec![42, 84]);
        assert_eq!(pinned.captures, vec!["capture-a"]);

        for action in [
            pa::message_action::Enum::Create,
            pa::message_action::Enum::Update,
            pa::message_action::Enum::Delete,
            pa::message_action::Enum::Read,
        ] {
            let empty = pa::PinnedModelsMessage {
                action: action as i32,
                request_id: Some(pa::pinned_models_message::RequestId::RequestId(99)),
                ..Default::default()
            }
            .encode_to_vec();
            assert_eq!(
                decode_pinned_models(&empty).unwrap(),
                PinnedModels {
                    models: Vec::new(),
                    captures: Vec::new(),
                },
                "accepts {action:?}, including an empty list"
            );
        }

        let files = pa::FileMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::file_message::RequestId::RequestId(73)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("local_ir_root".into())),
                name: Some(pa::folder_info::Name::Name("Impulse Responses".into())),
                is_factory: Some(pa::folder_info::IsFactory::IsFactory(false)),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key("ir/key".into())),
                    name: Some(pa::product_data::Name::Name("Room".into())),
                    author: Some(pa::product_data::Author::Author("Studio".into())),
                    description: Some(pa::product_data::Description::Description(
                        "Room capture".into(),
                    )),
                    date: Some(pa::product_data::Date::Date("2026-09-30".into())),
                    index: Some(pa::product_data::Index::Index(4)),
                    instrument: Some(pa::product_data::Instrument::Instrument(2)),
                    version: Some(pa::product_data::Version::Version("2".into())),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let entries = decode_library_files(&files, Some(73), "local_ir_root").unwrap();
        assert_eq!(entries.entries[0].key, "ir/key");
        assert_eq!(entries.entries[0].position, Some(4));
        assert_eq!(entries.entries[0].instrument, Some(2));
        assert_eq!(entries.entries[0].version.as_deref(), Some("2"));
        assert_eq!(entries.entries[0].author.as_deref(), Some("Studio"));
        assert_eq!(
            entries.entries[0].description.as_deref(),
            Some("Room capture")
        );
        assert_eq!(entries.entries[0].date.as_deref(), Some("2026-09-30"));
        assert!(matches!(
            decode_library_files(&files, Some(73), "another-folder"),
            Err(ResponseDecodeError::Mismatch(_))
        ));

        let files_without_request_id = pa::FileMessage {
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("local_ir_root".into())),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key("ir/key".into())),
                    name: Some(pa::product_data::Name::Name("Room".into())),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let entries =
            decode_library_files(&files_without_request_id, None, "local_ir_root").unwrap();
        assert_eq!(entries.entries[0].key, "ir/key");

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &files_without_request_id).unwrap();
        let compressed = encoder.finish().unwrap();
        let entries = decode_library_files(&compressed, None, "local_ir_root").unwrap();
        assert_eq!(entries.entries[0].name, "Room");

        let files_with_wrong_request_id = pa::FileMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::file_message::RequestId::RequestId(74)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("local_ir_root".into())),
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_library_files(&files_with_wrong_request_id, Some(73), "local_ir_root"),
            Err(ResponseDecodeError::Mismatch(
                "file listing request_id does not match"
            ))
        ));
    }

    #[test]
    fn recents_reply_rejects_wrong_action_missing_identity_and_malformed_payloads() {
        let reply = |action, request_id: Option<u64>, name: &str, folder_key: &str| {
            pa::RecentsFavoritesMessage {
                action,
                request_id: request_id.map(pa::recents_favorites_message::RequestId::RequestId),
                items: vec![pa::RecentsFavoritesItem {
                    name: name.into(),
                    folder_key: folder_key.into(),
                    ..Default::default()
                }],
                ..Default::default()
            }
            .encode_to_vec()
        };
        assert!(matches!(
            decode_recents_favorites(
                &reply(
                    pa::message_action::Enum::Create as i32,
                    Some(7),
                    "Stage",
                    "/Live"
                ),
                7,
            ),
            Err(ResponseDecodeError::Mismatch(
                "recents/favorites action is not UPDATE"
            ))
        ));
        assert!(matches!(
            decode_recents_favorites(
                &reply(
                    pa::message_action::Enum::Update as i32,
                    None,
                    "Stage",
                    "/Live"
                ),
                7,
            ),
            Err(ResponseDecodeError::Incomplete(
                "recents/favorites request_id"
            ))
        ));
        assert!(matches!(
            decode_recents_favorites(
                &reply(
                    pa::message_action::Enum::Update as i32,
                    Some(7),
                    "",
                    "/Live"
                ),
                7,
            ),
            Err(ResponseDecodeError::Incomplete(
                "recents/favorites item identity"
            ))
        ));
        assert!(matches!(
            decode_recents_favorites(&[0xff], 7),
            Err(ResponseDecodeError::Protobuf(_))
        ));
        assert!(matches!(
            decode_recents_favorites(&vec![0; profile::MAX_FRAME_BYTES + 1], 7),
            Err(ResponseDecodeError::OversizedReply)
        ));
    }

    #[test]
    fn ir_listing_requires_matching_correlation_folder_and_explicit_type() {
        let reply = |file_type: Option<i32>| {
            pa::FileMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: Some(pa::file_message::RequestId::RequestId(84)),
                r#type: file_type.map(pa::file_message::Type::Type),
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key("factory_ir_root".into())),
                    name: Some(pa::folder_info::Name::Name("Factory IRs".into())),
                    files: vec![pa::ProductData {
                        key: Some(pa::product_data::Key::Key("ir/factory/room".into())),
                        name: Some(pa::product_data::Name::Name("Room".into())),
                        index: Some(pa::product_data::Index::Index(7)),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec()
        };

        let entries = decode_ir_files(&reply(Some(1)), 84, "factory_ir_root").unwrap();
        assert_eq!(entries.entries[0].key, "ir/factory/room");
        assert_eq!(
            entries.entries[0].folder_name.as_deref(),
            Some("Factory IRs")
        );
        assert_eq!(entries.entries[0].position, Some(7));
        assert_eq!(
            decode_ir_files(&reply(None), 84, "factory_ir_root").unwrap(),
            entries
        );
        assert!(matches!(
            decode_ir_files(&reply(Some(0)), 84, "factory_ir_root"),
            Err(ResponseDecodeError::Mismatch(
                "file listing is not an Impulse Response list"
            ))
        ));
        assert!(matches!(
            decode_ir_files(&reply(Some(2)), 84, "factory_ir_root"),
            Err(ResponseDecodeError::Mismatch(
                "file listing is not an Impulse Response list"
            ))
        ));
    }

    #[test]
    fn file_listing_decoders_accept_all_receiver_routed_actions() {
        for action in [
            pa::message_action::Enum::Create,
            pa::message_action::Enum::Update,
            pa::message_action::Enum::Delete,
            pa::message_action::Enum::Read,
        ] {
            let payload = pa::FileMessage {
                action: action as i32,
                request_id: Some(pa::file_message::RequestId::RequestId(84)),
                r#type: Some(pa::file_message::Type::Type(1)),
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key("factory_ir_root".into())),
                    files: vec![pa::ProductData {
                        key: Some(pa::product_data::Key::Key("ir/key".into())),
                        name: Some(pa::product_data::Name::Name("Room".into())),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec();
            assert_eq!(
                decode_library_files(&payload, Some(84), "factory_ir_root")
                    .unwrap()
                    .entries
                    .len(),
                1,
                "library decoder rejected receiver-routed {action:?}",
            );
            assert_eq!(
                decode_ir_files(&payload, 84, "factory_ir_root")
                    .unwrap()
                    .entries
                    .len(),
                1,
                "IR decoder rejected receiver-routed {action:?}",
            );
            let (folder_key, entries) = decode_library_folder(&payload).unwrap().unwrap();
            assert_eq!(folder_key, "factory_ir_root");
            assert_eq!(
                entries.entries.len(),
                1,
                "broadcast folder decoder rejected receiver-routed {action:?}",
            );
        }
    }

    #[test]
    fn favorites_read_requires_correlated_preset_entries_but_allows_empty_lists() {
        let reply = |action, request_id, kind, items| {
            pa::RecentsFavoritesMessage {
                action,
                request_id,
                // This scalar has no presence bit; false can mean omitted.
                is_favorites: false,
                r#type: kind,
                items,
            }
            .encode_to_vec()
        };
        let item = pa::RecentsFavoritesItem {
            name: "Stage".into(),
            folder_key: "/media/p4/Presets/Live".into(),
            folder_name: "Live".into(),
            is_factory: false,
            is_plugin: true,
            product_key: "plugin:preset:stage".into(),
        };
        let preset = pa::recents_favorites_message::RecentFavoriteType::Preset as i32;
        let update = pa::message_action::Enum::Update as i32;
        let request = Some(pa::recents_favorites_message::RequestId::RequestId(82));
        let decoded =
            decode_favorites(&reply(update, request, preset, vec![item.clone()]), 82).unwrap();
        assert_eq!(decoded.entries[0].name, "Stage");
        assert!(decoded.entries[0].is_plugin);
        assert_eq!(
            decoded.entries[0].product_key.as_deref(),
            Some("plugin:preset:stage")
        );
        assert!(
            decode_favorites(&reply(update, request, preset, Vec::new()), 82)
                .unwrap()
                .entries
                .is_empty()
        );

        let capture = pa::recents_favorites_message::RecentFavoriteType::NeuralCapture as i32;
        assert!(matches!(
            decode_favorites(&reply(update, request, capture, vec![item.clone()]), 82),
            Err(ResponseDecodeError::Mismatch(_))
        ));
        assert!(matches!(
            decode_favorites(
                &reply(
                    pa::message_action::Enum::Create as i32,
                    request,
                    preset,
                    vec![item.clone()]
                ),
                82,
            ),
            Err(ResponseDecodeError::Mismatch(_))
        ));
        let mut incomplete = item.clone();
        incomplete.folder_key.clear();
        assert!(matches!(
            decode_favorites(&reply(update, request, preset, vec![incomplete]), 82),
            Err(ResponseDecodeError::Incomplete(_))
        ));
        assert!(matches!(
            decode_favorites(&reply(update, None, preset, vec![item]), 82),
            Err(ResponseDecodeError::Incomplete(_))
        ));
        let item = pa::RecentsFavoritesItem {
            name: "Stage".into(),
            folder_key: "/media/p4/Presets/Live".into(),
            ..Default::default()
        };
        let stale = Some(pa::recents_favorites_message::RequestId::RequestId(81));
        assert!(matches!(
            decode_favorites(&reply(update, stale, preset, vec![item.clone()]), 82),
            Err(ResponseDecodeError::Mismatch(_))
        ));
        let mut missing_name = item;
        missing_name.name.clear();
        assert!(matches!(
            decode_favorites(&reply(update, request, preset, vec![missing_name]), 82),
            Err(ResponseDecodeError::Incomplete(_))
        ));
        assert!(matches!(
            decode_favorites(&[0xff], 82),
            Err(ResponseDecodeError::Protobuf(_))
        ));
        assert!(matches!(
            decode_favorites(&vec![0; profile::MAX_FRAME_BYTES + 1], 82),
            Err(ResponseDecodeError::OversizedReply)
        ));
    }

    #[test]
    fn malformed_library_entries_are_rejected_instead_of_silently_defaulted() {
        let listing = |file: pa::ProductData| {
            pa::FileMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: Some(pa::file_message::RequestId::RequestId(9)),
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key("library".into())),
                    files: vec![file],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec()
        };

        let missing_key = listing(pa::ProductData {
            name: Some(pa::product_data::Name::Name("Preset".into())),
            ..Default::default()
        });
        assert!(matches!(
            decode_library_files(&missing_key, Some(9), "library"),
            Err(ResponseDecodeError::Incomplete("file key"))
        ));

        let negative_position = listing(pa::ProductData {
            key: Some(pa::product_data::Key::Key("preset-key".into())),
            name: Some(pa::product_data::Name::Name("Preset".into())),
            index: Some(pa::product_data::Index::Index(-1)),
            ..Default::default()
        });
        assert!(matches!(
            decode_library_files(&negative_position, Some(9), "library"),
            Err(ResponseDecodeError::Mismatch("negative file position"))
        ));
    }

    #[test]
    fn capture_listing_uses_root_and_item_identity_without_inventing_an_action_gate() {
        let listing = |action, folder_key: &str, key: &str, name: &str| {
            pa::FileMessage {
                action,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(folder_key.into())),
                    files: vec![pa::ProductData {
                        key: Some(pa::product_data::Key::Key(key.into())),
                        name: Some(pa::product_data::Name::Name(name.into())),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec()
        };

        for action in [
            pa::message_action::Enum::Create,
            pa::message_action::Enum::Update,
            pa::message_action::Enum::Delete,
            pa::message_action::Enum::Read,
        ] {
            let entries = decode_library_files(
                &listing(action as i32, "local_nc_root", "capture-key", "Stage Amp"),
                None,
                "local_nc_root",
            )
            .unwrap();
            assert_eq!(entries.entries[0].key, "capture-key");
            assert_eq!(entries.entries[0].name, "Stage Amp");
        }
        assert!(matches!(
            decode_library_files(
                &listing(
                    pa::message_action::Enum::Update as i32,
                    "local_ir_root",
                    "capture-key",
                    "Stage Amp",
                ),
                None,
                "local_nc_root",
            ),
            Err(ResponseDecodeError::Mismatch(
                "file listing folder does not match"
            ))
        ));
        for (key, name, missing) in [
            ("", "Stage Amp", "file key"),
            ("capture-key", "", "file name"),
        ] {
            assert!(matches!(
                decode_library_files(
                    &listing(
                        pa::message_action::Enum::Update as i32,
                        "local_nc_root",
                        key,
                        name,
                    ),
                    None,
                    "local_nc_root",
                ),
                Err(ResponseDecodeError::Incomplete(field)) if field == missing
            ));
        }
    }

    #[test]
    fn a_published_capture_folder_decodes_with_the_version_and_absent_position_the_wire_carried() {
        // `PresetFileListing` - what the preset catalog caches - carries no
        // version at all and forces a `u32` position onto every file, so a
        // capture listing served from it would report version null and position
        // 0. Cortex reads that version integer when choosing a Neural Capture
        // block family, so the publication cache decodes the frame in full.
        let frame = pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("local_nc_root/".into())),
                name: Some(pa::folder_info::Name::Name("Captures Library".into())),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key("capture-key".into())),
                    name: Some(pa::product_data::Name::Name("Brit 2555 Lead 16".into())),
                    version: Some(pa::product_data::Version::Version("2".into())),
                    instrument: Some(pa::product_data::Instrument::Instrument(1)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();

        let (key, entries) = decode_library_folder(&frame).unwrap().unwrap();
        // The key is learned from the frame and canonicalized: a broadcast burst
        // never tells the host which folder is arriving next.
        assert_eq!(key, "local_nc_root");
        let entry = &entries.entries[0];
        assert_eq!(entry.version.as_deref(), Some("2"));
        assert_eq!(entry.position, None);
        assert_eq!(entry.instrument, Some(1));
        assert_eq!(entry.folder_name.as_deref(), Some("Captures Library"));
        // The entry keeps the folder key exactly as the wire spelled it; only
        // the cache key is trimmed.
        assert_eq!(entry.folder_key.as_deref(), Some("local_nc_root/"));

        // The cached projection and the live correlated read are the same
        // function, so a caller cannot tell which one served it.
        assert_eq!(
            decode_library_files(&frame, None, "local_nc_root").unwrap(),
            entries
        );

        // A File push with no folder is a CREATE/UPDATE/DELETE broadcast, not a
        // decode error.
        assert!(decode_library_folder(
            &pa::FileMessage {
                action: pa::message_action::Enum::Update as i32,
                ..Default::default()
            }
            .encode_to_vec()
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn response_limits_and_numeric_edge_cases_are_explicit() {
        let oversized = vec![0_u8; profile::MAX_FRAME_BYTES + 1];
        assert!(matches!(
            decode_tempo_clock(&oversized),
            Err(ResponseDecodeError::OversizedReply)
        ));

        let parameter = |value| Param {
            index: Some(param::Index::Index(6)),
            param_values: vec![crate::proto::ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        };
        assert_eq!(
            decode_tempo_settings(&[parameter(f32::NAN)]).time_signature,
            None
        );
        assert_eq!(
            decode_tempo_settings(&[parameter(-0.1)]).time_signature,
            None
        );
        assert_eq!(
            decode_tempo_settings(&[parameter(1.1)]).time_signature,
            None
        );

        let invalid_general = pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            scene_block_bypass: Some(
                pa::general_settings_message::SceneBlockBypass::SceneBlockBypass(99),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            decode_general_settings(&invalid_general),
            Err(ResponseDecodeError::Mismatch(_))
        ));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_bounded_reply_bytes_never_panic(payload in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = decode_tempo_clock(&payload);
            let _ = decode_recalled_preset_name(&payload);
            let _ = decode_selected_scene(&payload);
            let _ = decode_device_identity(&payload);
            let _ = decode_recents_favorites(&payload, 1);
            let _ = decode_favorites(&payload, 1);
            let _ = decode_pinned_models(&payload);
            let _ = decode_library_files(&payload, Some(1), "library");
            let _ = decode_tuner_settings(&payload);
            let _ = decode_general_settings(&payload);
            let _ = decode_io_settings(&payload);
            let _ = decode_global_eq(&payload);
            let _ = decode_mode_cycle(&payload);
            let _ = decode_looper_status(&payload);
            let _ = decode_tempo_mode(&payload);
            let _ = decode_global_tempo_settings(&payload);
            let _ = decode_inhibited_modules(&payload);
            let _ = decode_preset_screenshot(&payload, 1, "folder", 1, false);
            let _ = decode_captured_screen(&payload);
            let mut backup = BackupAssembler::default();
            let _ = backup.push(&payload);
        }
    }
}
