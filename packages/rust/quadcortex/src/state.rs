//! Stateful QC protobuf normalization shared by every native host.
//!
//! USB enumeration, permissions, and endpoint I/O remain platform concerns.
//! This module owns the device-independent mapping from QC protobuf messages to
//! the small camelCase state-event contract consumed by `@qc-remote/core`.

mod model_preset_catalog;
#[cfg(test)]
mod numeric_projection_tests;
mod projection;
pub use model_preset_catalog::{
    ModelPresetCatalog, Refusal, GLOBAL_EQ_MODEL_PRESET_HASH, PUBLISHED_ENTRY_LIMIT,
    RETAINED_ENTRY_LIMIT,
};
pub(super) use projection::*;

use crate::commands::DeviceOperation;
use crate::compression::{maybe_gunzip, InflateError};
pub use crate::generated_payloads::{
    BlockDetails, BlockParameter, BulkOperationState, BypassExpression, BypassUpdate,
    CpuColumnLoad, FootswitchState, GridBlock, GridRoute, IoPortState, MidiOutMessage,
    MidiOutSource, ModeSlot, ModelPresetSummary, QcStateUpdate as StateUpdate, ScalePoint,
};
use crate::inbound_telemetry::InboundTelemetry;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::proto::{
    binary_preset, bypass, chain, col_bypass, model, param, param_value, BinaryPreset, Chain,
    Model, Param,
};
use crate::{domain, profile};
use prost::Message;
use quick_xml::events::Event;
use quick_xml::Reader;
use quick_xml::XmlVersion;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

// Software cache ceilings, not firmware index/count maxima. A session can
// otherwise accumulate arbitrary sparse indices across individually small frames.
const RETAINED_SPARSE_PARAMETER_LIMIT: usize = model_preset_catalog::RETAINED_ENTRY_LIMIT;
const MODEL_CATALOG_MEMORY_LIMIT: usize = profile::MAX_INFLATED_BYTES * 4;
const MODEL_CATALOG_CLONE_DEPTH_LIMIT: usize = 128;

/// Errors produced while decoding QC protobuf payloads or the model catalog.
#[derive(Debug, Error)]
pub enum StateDecodeError {
    /// Compatibility classification for sparse budget failures from consumers.
    #[error("QC sparse parameter state exceeds its retained-entry limit; refresh the preset")]
    ParameterRetentionLimit,
    /// A selected scene did not name any of the eight declared scene slots.
    #[error("QC selected scene {0} is outside the declared scene domain")]
    InvalidSelectedScene(u32),
    /// A projected numeric field was nonfinite; no firmware range is inferred.
    #[error("QC protobuf type {message_type} field {field} is not finite")]
    NonFiniteProjection {
        /// Wire message family carrying the rejected value.
        message_type: u16,
        /// Public numeric evidence that cannot be represented.
        field: &'static str,
    },
    /// A cumulative software cache budget was exceeded; no truncated state is valid.
    #[error("QC state retention budget exceeded: {0}")]
    RetentionLimit(&'static str),
    /// The payload is larger than its message type can legitimately carry: the
    /// type's inflation limit for a compressed envelope, or the raw frame
    /// ceiling, whichever is larger.
    #[error("QC state payload exceeds its message type's size limit")]
    PayloadLimit,
    /// The payload could not be parsed as the protobuf message for `message_type`.
    #[error("QC protobuf type {message_type} could not be decoded: {source}")]
    Protobuf {
        /// Wire message type number being decoded.
        message_type: u16,
        /// The underlying protobuf decode error.
        #[source]
        source: prost::DecodeError,
    },
    /// A message with an action other than update arrived where only an update is accepted.
    #[error("QC protobuf type {message_type} used unexpected action {action}")]
    UnexpectedAction {
        /// Wire message type number of the offending message.
        message_type: u16,
        /// Raw action value received: `0` create, `1` update, `2` delete, `3`
        /// read, `4` move, `5` copy, `6` upload, `7` download or `8` swap.
        action: i32,
    },
    /// A field that must be a normalized value was not finite or fell outside `0.0..=1.0`.
    #[error(
        "QC protobuf type {message_type} field {field} used out-of-domain normalized value {value}"
    )]
    InvalidNormalizedValue {
        /// Wire message type number of the offending message.
        message_type: u16,
        /// Name of the offending field.
        field: &'static str,
        /// The rejected value.
        value: f32,
    },
    /// A grid row or column was negative or outside the grid bounds.
    #[error("QC protobuf type {message_type} used out-of-domain Grid coordinates")]
    InvalidGridCoordinate {
        /// Wire message type number of the offending message.
        message_type: u16,
    },
    /// A compressed payload would inflate beyond the inflated-size limit.
    #[error("compressed QC state exceeds the inflated-size limit")]
    InflatedLimit,
    /// Decompressing a payload failed; carries the decoder's message.
    #[error("compressed QC state could not be decoded: {0}")]
    Compression(String),
    /// The model catalog archive is malformed, for example a truncated tar member or a missing XML
    /// member; carries a description.
    #[error("model catalog archive is invalid: {0}")]
    CatalogArchive(String),
    /// The model catalog XML is malformed; carries the parser's message.
    #[error("model catalog XML is invalid: {0}")]
    CatalogXml(String),
}

impl From<InflateError> for StateDecodeError {
    fn from(error: InflateError) -> Self {
        match error {
            InflateError::Limit => Self::InflatedLimit,
            InflateError::Decode(message) => Self::Compression(message),
        }
    }
}

fn is_update_action(action: i32) -> bool {
    action == pa::message_action::Enum::Update as i32
}

impl StateUpdate {
    /// Construct an empty typed update for native reducers and parity fixtures.
    pub fn empty(kind: &str) -> Self {
        Self::new(kind)
    }

    fn new(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            active_scene: None,
            dirty: None,
            master_volume: None,
            index: None,
            label: None,
            color: None,
            setlist_key: None,
            position: None,
            is_factory: None,
            preset_name: None,
            preset_identity: None,
            tempo: None,
            tempo_led_enabled: None,
            scenes: None,
            scene_colors: None,
            footswitch_states: None,
            midi_out: None,
            preset_load_midi_out: None,
            blocks: None,
            routes: None,
            io_ports: None,
            io_settings_error: None,
            mode: None,
            mode_slots: None,
            footswitch_modes: None,
            row: None,
            column: None,
            bypassed: None,
            bypass_updates: None,
            parameter_index: None,
            normalized_value: None,
            catalog_refresh: None,
            observed_at: None,
            state_version: None,
            gig_view_shown: None,
            cpu_total_load: None,
            cpu_columns: None,
            undo_available: None,
            redo_available: None,
            refresh_required: None,
            suspended: None,
            suspend_reason: None,
            bulk_operation: None,
            model_preset_catalog: None,
            grid_layout_complete: None,
            default_parameter_count: None,
            new_model_ids: None,
            raw_mode: None,
            raw_color: None,
            scene_colors_argb: None,
            scene_labels_raw: None,
            undo_frames: None,
            redo_frames: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ModelInfo {
    name: String,
    category: String,
    kind: String,
    clone_id: Option<u32>,
    direct_parameters: Vec<ParameterSpec>,
    parameters: BTreeMap<u32, ParameterSpec>,
    hidden: bool,
    based_on: String,
    plugin_id: Option<u64>,
    sku: Option<String>,
}

/// Parsed, resolved ModelRepo metadata. Hosts may build this on a low-priority
/// worker and install it atomically without delaying live state reduction.
pub struct ModelCatalog(HashMap<u32, ModelInfo>);

/// One selectable model in the synchronized model catalog.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    /// Model identifier, the model hash that preset grid blocks carry.
    pub id: u32,
    /// Display name of the model.
    pub name: String,
    /// Display category of the model, for example `Guitar Amp`.
    pub category: String,
    /// Description of the product the model is based on, as declared by the catalog; empty when the
    /// catalog declares none.
    pub based_on: String,
    /// Identifier of the plugin license the model requires; `None` for built-in models.
    pub plugin_id: Option<u64>,
    /// Product SKU declared by the catalog, if any.
    pub sku: Option<String>,
    /// Whether QC Remote will place the model. Every listed model is available, plugin models
    /// included: QC Remote does not establish plugin ownership; the device response does.
    pub available: bool,
}

/// The license state decoded from the device's License messages. Informational only; plugin
/// ownership is enforced by the device, not by QC Remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseObservation {
    /// The last decoded `is_enabled` flag, if any message carried one.
    pub enabled: Option<bool>,
    /// The last decoded `is_deactivated` flag, if any message carried one.
    pub deactivated: Option<bool>,
    /// Plugin identifiers in the last decoded license list, sorted.
    pub plugin_ids: Vec<u64>,
}

/// A model parameter that failed the catalog consistency audit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogAuditException {
    /// Identifier of the model that owns the parameter.
    pub model_id: u32,
    /// Display name of the model.
    pub model_name: String,
    /// Index of the offending parameter within the model.
    pub parameter_index: u32,
    /// Human-readable description of the problem: a duplicate parameter index, an empty parameter
    /// name, an unsupported parameter type, or a zero step count.
    pub issue: String,
}

/// Summary of a consistency audit over the visible models and parameters of a catalog.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogAudit {
    /// Number of listed (non-hidden) models.
    pub model_count: usize,
    /// Number of visible parameters across the listed models.
    pub parameter_count: usize,
    /// Number of distinct categories among the listed models.
    pub category_count: usize,
    /// Parameters that failed the audit, ordered by model id, parameter index, then issue text.
    pub exceptions: Vec<ModelCatalogAuditException>,
}

/// The selectable models of a catalog together with its audit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelList {
    /// Listed models, sorted by category, then name (case-insensitively), then id.
    pub models: Vec<ModelEntry>,
    /// Consistency audit of the listed models.
    pub audit: ModelCatalogAudit,
}

/// One slot record of a preset folder, as published by the device.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresetFileListing {
    /// Zero-based slot index within the folder, in `0..256`.
    pub position: u32,
    /// Display name of the preset; empty when the record carries none.
    pub name: String,
    /// Raw instrument value of the record; zero when the device omitted it (see
    /// `instrument_present`).
    ///
    /// Hardware listings have been observed to report `1` for items tagged as
    /// guitar and `2` for items tagged as bass; `0` means untagged or absent.
    /// Values `3`, `4` and `5` are believed to be Synth, Vocal and Other, the
    /// order in which the unit's capture-save screen offers Guitar, Bass, Synth,
    /// Vocal and Other as the preferred instrument (CorOS 4.1.0). Values `4` and
    /// `5` have been seen rarely in full library listings (`4` on four factory
    /// presets, three of them named for vocal use, and one capture; `5` on one
    /// capture), which fits that reading; `3` has not been seen. The names for
    /// `3` to `5` have not been verified against the unit used for this
    /// project's tests.
    pub instrument: i32,
    /// Whether ProductData.instrument was present on the wire. Its public
    /// compatibility value remains zero when absent.
    pub instrument_present: bool,
    /// Catalog key identifying the stored preset; `None` for a record without one.
    pub product_key: Option<String>,
    /// Optional author supplied by CorOS for directory sorting/display.
    pub author: Option<String>,
    /// Original device date text, retained when CorOS does not publish an epoch value.
    pub date: Option<String>,
    /// Device publication timestamp in milliseconds since Unix epoch.
    pub date_ms_since_epoch: Option<i64>,
}

impl PresetFileListing {
    /// Single source of truth for slot occupancy: a product_key (the
    /// authoritative catalog identity) OR a non-empty display name from a
    /// keyless compatibility record (a Grid preset saved before its File
    /// product key was correlated) marks the slot occupied. Either signal
    /// alone must be enough -- gating purely on name lets an
    /// empty-named-but-occupied slot bypass savePresetAs' overwrite
    /// confirmation, while gating purely on product_key makes a
    /// named-but-keyless source preset look nonexistent to copy/rename.
    ///
    /// `PresetLibrary::entry()` (savePresetAs' overwrite guard) and
    /// `PresetLibrary::project_slots()` (`device.listPresetSlots`) both rely
    /// on this exact predicate agreeing; keep it here as the one definition
    /// rather than inlining it at each call site, since these two call sites
    /// have drifted apart before.
    pub fn is_occupied(&self) -> bool {
        self.product_key.is_some() || !self.name.is_empty()
    }
}

/// A preset folder (setlist) as published by the device, with its slot records.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetFolderListing {
    /// Device folder key, a path-like string.
    pub key: String,
    /// Display name of the folder; derived from the last segment of the key when the device sends
    /// none.
    pub name: String,
    /// FileMessage.type. Preset folders are type 0; plugin/IR/NC providers
    /// are routed through a separate file type.
    pub file_type: Option<i32>,
    /// Whether the device marks the folder as factory content.
    pub is_factory: bool,
    /// Whether the device marks the folder as a plugin folder.
    pub is_plugin: bool,
    /// Slot records of the folder, sorted by position.
    pub files: Vec<PresetFileListing>,
    /// Position of the folder in the order the device first published it, counted from zero.
    ///
    /// The wire folder record carries no index, but the device publishes its user folders in the
    /// order its own directory lists them, so first-seen arrival is that order. Only the retained
    /// catalog observations assign it; every other projection leaves it `None`.
    pub order: Option<u32>,
}

/// Decode a File envelope without dropping action, destination, or field presence.
/// Consumers must apply action semantics before treating its records as state.
pub fn decode_file_message(payload: &[u8]) -> Result<pa::FileMessage, StateDecodeError> {
    validate_wire_payload(profile::MESSAGE_TYPE_FILE, payload)?;
    let decoded = maybe_gunzip(payload)?;
    decode::<pa::FileMessage>(profile::MESSAGE_TYPE_FILE, &decoded)
}

/// Compatibility projection of one source folder, not a complete inventory or
/// evidence of acknowledgement. Native state reduction uses the full envelope.
pub fn decode_preset_folder(
    payload: &[u8],
) -> Result<Option<PresetFolderListing>, StateDecodeError> {
    let message = decode_file_message(payload)?;
    let file_type = match message.r#type {
        Some(pa::file_message::Type::Type(value)) => Some(value),
        _ => None,
    };
    let Some(pa::file_message::Folder::Folder(folder)) = message.folder else {
        return Ok(None);
    };
    Ok(project_preset_folder(&folder, file_type))
}

/// Project fields for display. This deliberately does not assign snapshot or
/// deletion semantics; the action-aware catalog reducer owns those decisions.
pub fn project_preset_folder(
    folder: &pa::FolderInfo,
    file_type: Option<i32>,
) -> Option<PresetFolderListing> {
    let Some(pa::folder_info::Key::Key(key)) = folder.key.clone() else {
        return None;
    };
    if key.is_empty() {
        return None;
    }
    let name = match folder.name.clone() {
        Some(pa::folder_info::Name::Name(value)) => value,
        _ => key
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .into(),
    };
    let is_factory = matches!(
        folder.is_factory,
        Some(pa::folder_info::IsFactory::IsFactory(true))
    );
    let is_plugin = matches!(
        folder.is_plugin,
        Some(pa::folder_info::IsPlugin::IsPlugin(true))
    );
    let mut files = folder
        .files
        .iter()
        .cloned()
        .filter_map(|file| {
            let pa::product_data::Index::Index(index) = file.index?;
            if !(0..256).contains(&index) {
                return None;
            }
            let product_key = match file.key {
                Some(pa::product_data::Key::Key(value)) if !value.is_empty() => Some(value),
                _ => None,
            };
            let name = match file.name {
                Some(pa::product_data::Name::Name(value)) => value,
                _ => String::new(),
            };
            let (instrument, instrument_present) = match file.instrument {
                Some(pa::product_data::Instrument::Instrument(value)) => (value, true),
                _ => (0, false),
            };
            let author = match file.author {
                Some(pa::product_data::Author::Author(value)) if !value.is_empty() => Some(value),
                _ => None,
            };
            let date = match file.date {
                Some(pa::product_data::Date::Date(value)) if !value.is_empty() => Some(value),
                _ => None,
            };
            let date_ms_since_epoch = match file.date_ms_since_epoch {
                Some(pa::product_data::DateMsSinceEpoch::DateMsSinceEpoch(value)) => Some(value),
                _ => None,
            };
            Some(PresetFileListing {
                position: index as u32,
                name,
                instrument,
                instrument_present,
                product_key,
                author,
                date,
                date_ms_since_epoch,
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|file| file.position);
    Some(PresetFolderListing {
        order: None,
        key,
        name,
        file_type,
        is_factory,
        is_plugin,
        files,
    })
}

impl ModelCatalog {
    /// Lists the models that are not hidden, sorted by category, name and id, together with an
    /// audit of their visible parameters.
    ///
    /// Every listed model is available: plugin ownership is enforced by the device.
    pub fn model_list(&self) -> ModelList {
        const SUPPORTED: &[&str] = &[
            "comboBox",
            "empty",
            "fader",
            "float",
            "floatWithLed",
            "grMeter",
            "int",
            "rotarySwitch",
            "string",
            "switch",
            "toggleButton",
        ];
        let mut models = Vec::new();
        let mut categories = HashSet::new();
        let mut parameter_count = 0;
        let mut exceptions = Vec::new();
        for (id, info) in &self.0 {
            if info.hidden {
                continue;
            }
            categories.insert(info.category.clone());
            models.push(ModelEntry {
                id: *id,
                name: info.name.clone(),
                category: display_category(&info.category),
                based_on: info.based_on.clone(),
                plugin_id: info.plugin_id,
                sku: info.sku.clone(),
                available: true,
            });
            let mut seen = HashSet::new();
            for parameter in info
                .parameters
                .values()
                .filter(|parameter| !parameter.hidden)
            {
                parameter_count += 1;
                let issue = if !seen.insert(parameter.index) {
                    Some(format!("duplicate parameter index {}", parameter.index))
                } else if parameter.name.trim().is_empty() {
                    Some("parameter name is empty".into())
                } else if !SUPPORTED.contains(&parameter.r#type.as_str()) {
                    Some(format!("unsupported parameter type {:?}", parameter.r#type))
                } else if parameter.steps == Some(0) {
                    Some("invalid step count 0".into())
                } else {
                    None
                };
                if let Some(issue) = issue {
                    exceptions.push(ModelCatalogAuditException {
                        model_id: *id,
                        model_name: info.name.clone(),
                        parameter_index: parameter.index,
                        issue,
                    });
                }
            }
        }
        models.sort_by(|left, right| {
            left.category
                .to_ascii_lowercase()
                .cmp(&right.category.to_ascii_lowercase())
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        exceptions.sort_by(|left, right| {
            left.model_id
                .cmp(&right.model_id)
                .then_with(|| left.parameter_index.cmp(&right.parameter_index))
                .then_with(|| left.issue.cmp(&right.issue))
        });
        ModelList {
            audit: ModelCatalogAudit {
                model_count: models.len(),
                parameter_count,
                category_count: categories.len(),
                exceptions,
            },
            models,
        }
    }
}

#[derive(Debug, Clone)]
struct ParameterSpec {
    index: u32,
    name: String,
    r#type: String,
    minimum: f64,
    maximum: f64,
    scale_known: bool,
    value_scale: String,
    scale_exponent: Option<f64>,
    scale_points: Vec<ScalePoint>,
    units: String,
    steps: Option<u32>,
    display_position: u32,
    display_position_raw: Option<u32>,
    screen_visible: bool,
    options: Vec<String>,
    hidden: bool,
    minimum_label: Option<String>,
    midpoint_label: Option<String>,
    maximum_label: Option<String>,
    show_as_integer: bool,
    expression_assignable: bool,
    linked_scene_mode: Option<u32>,
    enable_when_on: Option<u32>,
    enable_when_off: Option<u32>,
    enable_when_steps: Vec<u32>,
    replaces: Option<u32>,
}

/// The span CorOS draws for every linear control carrying left, centre, and
/// right endpoint labels. The ModelRepo describes those 36 pan-style controls
/// with four mutually inconsistent numeric ranges; all of them display as
/// 50 L through C to 50 R.
pub const LABELLED_END_SPAN: (f64, f64) = (-50.0, 50.0);

impl ParameterSpec {
    fn display_precision(&self) -> Option<u32> {
        if self.show_as_integer {
            return Some(0);
        }
        let units = self.units.trim().to_ascii_lowercase();
        match units.as_str() {
            "bpm" | "semitones" | "cents" | "st" | "bits" => Some(0),
            "%" => Some(if self.steps.unwrap_or(0) > 101 { 1 } else { 0 }),
            "db" | "db/oct" => Some(1),
            "hz" => Some(if self.maximum <= 20.0 { 2 } else { 0 }),
            "s" | "seconds" => Some(2),
            "ms" => Some(if self.maximum <= 10.0 {
                2
            } else if self.maximum <= 100.0 {
                1
            } else {
                0
            }),
            // A bare 0..1 amount (WAH, RANGE, Q, WIDTH) is read to two places:
            // the wah pedal sits at 0.41, not 0.4. Position and distance are
            // their own 0..10-style scales and keep one place.
            "" if self.minimum == 0.0
                && self.maximum == 1.0
                && self.minimum_label.is_none()
                && self.r#type.eq_ignore_ascii_case("float")
                && !matches!(
                    self.name.trim().to_ascii_lowercase().as_str(),
                    "position" | "distance"
                ) =>
            {
                Some(2)
            }
            _ => Some(1),
        }
    }

    fn has_off_position(&self) -> bool {
        self.minimum_label
            .as_deref()
            .is_some_and(|label| !label.is_empty())
            && !self
                .midpoint_label
                .as_deref()
                .is_some_and(|label| !label.is_empty())
    }

    fn floor_display(&self) -> Option<f64> {
        if !self.has_off_position() || !self.scale_known || self.maximum <= self.minimum {
            return None;
        }
        let floor = self.minimum + if self.show_as_integer { 1.0 } else { 0.01 };
        (floor <= self.maximum).then_some(floor)
    }

    fn floor_normalized(&self) -> Option<f64> {
        let floor = self.floor_display()?;
        let span = self.maximum - self.minimum;
        let ratio = (floor - self.minimum) / span;
        match self.value_scale.as_str() {
            "linear" => Some(ratio),
            "power" => self
                .scale_exponent
                .filter(|exponent| exponent.is_finite() && *exponent > 0.0)
                .map(|exponent| ratio.powf(1.0 / exponent)),
            _ => None,
        }
    }
}

fn catalog_number(value: &str) -> Option<f64> {
    let symbolic = match value.trim() {
        "MIN_EQ_DB" => Some(-12.0),
        "MAX_EQ_DB" => Some(12.0),
        "MIN_EQ_FREQ" => Some(20.0),
        "MAX_EQ_FREQ" => Some(20_000.0),
        "MIN_TEMPO" => Some(domain::MINIMUM_TEMPO_BPM as f64),
        "MAX_TEMPO" => Some(domain::MAXIMUM_TEMPO_BPM as f64),
        "DEFAULT_TEMPO" => Some(120.0),
        "MIN_CABSIM_DB" | "MIN_MIXER_DB" | "MIN_FXLOOP_OUT_GAIN_DB" | "MIN_FXLOOP_IN_GAIN_DB" => {
            Some(-40.0)
        }
        "MAX_CABSIM_DB" | "MAX_MIXER_DB" | "MAX_FXLOOP_OUT_GAIN_DB" | "MAX_FXLOOP_IN_GAIN_DB" => {
            Some(12.0)
        }
        // NC_Recorder OUT LEVEL cannot be placed safely on the grid; its named
        // bounds remain deliberately unknown rather than becoming a guessed scale.
        "MIN_INPUT_TRIM" | "MAX_INPUT_TRIM" => None,
        "LIN_SKEW" => Some(1.0),
        _ => None,
    };
    symbolic
        .or_else(|| value.trim().parse().ok())
        .filter(|value| value.is_finite())
}

fn validate_catalog_numeric_attributes(
    attrs: &HashMap<String, String>,
) -> Result<(), StateDecodeError> {
    for key in ["min", "max", "skew", "xValues", "yValues"] {
        if attrs.get(key).is_some_and(|values| {
            values.split(',').any(|text| {
                text.trim()
                    .parse::<f64>()
                    .is_ok_and(|value| !value.is_finite())
            })
        }) {
            return Err(StateDecodeError::CatalogXml(format!(
                "non-finite parameter {key}"
            )));
        }
    }
    if attrs
        .get("skew")
        .and_then(|text| catalog_number(text))
        .is_some_and(|skew| skew <= 0.0 || !(1.0 / skew).is_finite())
    {
        return Err(StateDecodeError::CatalogXml(
            "invalid parameter skew".into(),
        ));
    }
    if attrs
        .get("min")
        .and_then(|text| catalog_number(text))
        .zip(attrs.get("max").and_then(|text| catalog_number(text)))
        .is_some_and(|(minimum, maximum)| !(maximum - minimum).is_finite())
    {
        return Err(StateDecodeError::CatalogXml(
            "parameter span is not finite".into(),
        ));
    }
    let x = number_list(attrs.get("xValues"));
    let y = number_list(attrs.get("yValues"));
    if x.windows(2)
        .chain(y.windows(2))
        .any(|pair| !(pair[1] - pair[0]).is_finite())
    {
        return Err(StateDecodeError::CatalogXml(
            "parameter lookup span is not finite".into(),
        ));
    }
    Ok(())
}

fn split_text_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn split_u32_list(value: Option<&String>) -> Vec<u32> {
    value
        .map(|value| {
            value
                .split(',')
                .filter_map(|item| item.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn number_list(value: Option<&String>) -> Vec<f64> {
    value
        .map(|value| value.split(',').filter_map(catalog_number).collect())
        .unwrap_or_default()
}

fn normalized_to_real(normalized: f64, spec: Option<&ParameterSpec>) -> f64 {
    let amount = normalized.clamp(0.0, 1.0);
    let Some(spec) = spec else { return amount };
    match spec.value_scale.as_str() {
        "lookup" if spec.scale_points.len() >= 2 => {
            for pair in spec.scale_points.windows(2) {
                let left = &pair[0];
                let right = &pair[1];
                if amount <= right.normalized {
                    let span = right.normalized - left.normalized;
                    let ratio = if span == 0.0 {
                        0.0
                    } else {
                        (amount - left.normalized) / span
                    };
                    return left.real + ratio * (right.real - left.real);
                }
            }
            spec.scale_points
                .last()
                .map(|point| point.real)
                .unwrap_or(amount)
        }
        "power" => {
            spec.minimum
                + amount.powf(spec.scale_exponent.unwrap_or(1.0)) * (spec.maximum - spec.minimum)
        }
        "logarithmic" if spec.minimum > 0.0 => {
            spec.minimum * (spec.maximum / spec.minimum).powf(amount)
        }
        _ => spec.minimum + amount * (spec.maximum - spec.minimum),
    }
}

fn special_value_label(normalized: f64, spec: Option<&ParameterSpec>) -> Option<String> {
    let spec = spec?;
    if normalized <= 0.000_001 {
        spec.minimum_label.clone()
    } else if (normalized - 0.5).abs() <= 0.000_001 {
        spec.midpoint_label.clone()
    } else if normalized >= 0.999_999 {
        spec.maximum_label.clone()
    } else {
        None
    }
}

/// The unit writes a left/centre/right control as a magnitude and a side, "15 L"
/// or "50 R", with the centre label for zero, never as a signed number.
fn labelled_end_value_label(real: f64, spec: Option<&ParameterSpec>) -> Option<String> {
    let spec = spec?;
    let (low, centre, high) = (
        spec.minimum_label
            .as_deref()
            .filter(|label| !label.is_empty())?,
        spec.midpoint_label
            .as_deref()
            .filter(|label| !label.is_empty())?,
        spec.maximum_label
            .as_deref()
            .filter(|label| !label.is_empty())?,
    );
    let rounded = real.round();
    Some(if rounded == 0.0 {
        centre.to_string()
    } else if rounded < 0.0 {
        format!("{} {low}", -rounded)
    } else {
        format!("{rounded} {high}")
    })
}

fn trim_number(value: f64, precision: usize) -> String {
    let rendered = format!("{value:.precision$}");
    let rendered = rendered.trim_end_matches('0').trim_end_matches('.');
    if rendered == "-0" {
        "0".into()
    } else {
        rendered.into()
    }
}

fn format_parameter_number(value: f64, precision: Option<u32>) -> String {
    match precision {
        Some(precision) => {
            let rendered = format!("{value:.precision$}", precision = precision as usize);
            if rendered.starts_with('-') && rendered.parse::<f64>().unwrap_or(value) == 0.0 {
                rendered[1..].into()
            } else {
                rendered
            }
        }
        None => trim_number(value, 3),
    }
}

fn parameter_enabled(
    spec: &ParameterSpec,
    values: &HashMap<u32, Option<f64>>,
    info: Option<&ModelInfo>,
) -> bool {
    let selected_step = |controller: u32, value: f64| {
        let count = info
            .and_then(|model| model.parameters.get(&controller))
            .and_then(|parameter| parameter.steps)
            .unwrap_or(2);
        (value.clamp(0.0, 1.0) * count.saturating_sub(1).max(1) as f64).round() as u32
    };
    if let Some(controller) = spec.enable_when_on {
        let Some(value) = values.get(&controller).copied().flatten() else {
            return false;
        };
        if if spec.enable_when_steps.is_empty() {
            value < 0.5
        } else {
            !spec
                .enable_when_steps
                .contains(&selected_step(controller, value))
        } {
            return false;
        }
    }
    if let Some(controller) = spec.enable_when_off {
        let Some(value) = values.get(&controller).copied().flatten() else {
            return false;
        };
        if if spec.enable_when_steps.is_empty() {
            value >= 0.5
        } else {
            spec.enable_when_steps
                .contains(&selected_step(controller, value))
        } {
            return false;
        }
    }
    true
}

fn conditional_parameter_hidden(
    model_id: u32,
    parameter_index: u32,
    values: &HashMap<u32, Option<f64>>,
) -> bool {
    if model_id != 18007 {
        return false;
    }
    let quantized = values.get(&4).copied().flatten().unwrap_or(0.0) >= 0.5;
    if quantized {
        matches!(parameter_index, 20 | 21)
    } else {
        matches!(parameter_index, 7 | 11)
    }
}

fn normalized_category(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn display_category(value: &str) -> String {
    match normalized_category(value).as_str() {
        "bassamplifier" | "bassamp" => "Bass Amp",
        "bassoverdrive" => "Bass Overdrive",
        "cabsimbassm" | "cabsimbassst" | "basscabinet" | "basscab" => "Bass Cab",
        "cabsimguitarm" | "cabsimguitarst" | "guitarcabinet" | "guitarcab" => "Guitar Cab",
        "compressor" => "Compressor",
        "delay" => "Delay",
        "equalizer" | "eq" => "EQ",
        "filter" => "Filter",
        "fxloop" => "FX Loop",
        "guitaramplifier" | "guitaramp" | "amp" => "Guitar Amp",
        "guitaroverdrive" => "Guitar Overdrive",
        "irloaders" | "irloader" => "IR Loader",
        "loopers" | "looper" => "Looper",
        "modulation" => "Modulation",
        "morph" => "Morph",
        "neuralcapture" => "Neural Capture",
        "pitch" => "Pitch",
        "reverb" => "Reverb",
        "synth" => "Synth",
        "utility" => "Utility",
        "wah" => "Wah",
        _ => return value.trim().into(),
    }
    .into()
}

#[derive(Debug, Clone, Default)]
struct TempoState {
    bpm: Option<u32>,
    led_enabled: Option<bool>,
}

/// Opaque retained source contents. Equality compares decoded maps by content,
/// not protobuf serialization order or a publication revision. Deliberately
/// neither serializable nor Debug: preset contents stay inside the owner.
#[derive(Clone, PartialEq)]
pub struct PresetSourceSnapshot {
    preset: std::sync::Arc<BinaryPreset>,
}

impl PresetSourceSnapshot {
    /// Restoration-only comparison after a catalog-verified Save. CorOS changes
    /// name, hash and date when saving the same complete source at a new address.
    /// Never use this weaker relation to admit a persistent write.
    pub fn matches_saved_contents(&self, source: &Self, saved_name: &str) -> bool {
        if !matches!(&self.preset.name, Some(binary_preset::Name::Name(name)) if name == saved_name)
        {
            return false;
        }
        self.matches_save_progress(source, saved_name, None)
    }

    /// Position can arrive between the old-name/new-hash publication and the
    /// final saved-name publication. After Rename, the retained full preset can
    /// still carry its older name; the prepared source catalog proves the name
    /// used by the intermediate publication. All non-save fields remain exact.
    pub fn matches_save_progress(
        &self,
        source: &Self,
        saved_name: &str,
        source_catalog_name: Option<&str>,
    ) -> bool {
        if self.preset.name != source.preset.name
            && !matches!(&self.preset.name, Some(binary_preset::Name::Name(name)) if name == saved_name)
            && !matches!(&self.preset.name, Some(binary_preset::Name::Name(name)) if Some(name.as_str()) == source_catalog_name)
        {
            return false;
        }
        let mut saved = self.preset.as_ref().clone();
        saved.name.clone_from(&source.preset.name);
        saved.hash = source.preset.hash;
        saved.date.clone_from(&source.preset.date);
        saved == *source.preset
    }
}

#[derive(Default)]
struct PresetMetadataObservations {
    labels: [Option<String>; 8],
    colors: [Option<u32>; 8],
    midi_sources: HashMap<u32, Vec<MidiOutMessage>>,
    preset_load: Option<Vec<MidiOutMessage>>,
    tempo_led: Option<bool>,
}

/// Stateful because scene selection, ModelRepo metadata, the active preset,
/// and live parameter overrides arrive in independent QC messages.
#[derive(Default)]
pub struct StateDecoder {
    active_scene: u32,
    setlist_key: Option<String>,
    position: Option<u32>,
    is_factory: Option<bool>,
    preset: Option<BinaryPreset>,
    preset_exact: bool,
    /// The retained chains' model identities and coordinates still equal the
    /// device's, even though `preset_exact` has been cleared by a sparse Grid
    /// push. The move planner's retained grid context and its source/destination
    /// checks read only this layout, so they must not be refused because a
    /// parameter or bypass push followed a recall.
    grid_layout_exact: bool,
    state_revision: u64,
    catalog: HashMap<u32, ModelInfo>,
    licensed_plugins: HashSet<u64>,
    licenses_enabled: Option<bool>,
    licenses_deactivated: Option<bool>,
    parameter_overrides: HashMap<(u32, u32, u32), f64>,
    metadata_observations: PresetMetadataObservations,
    // Projection evidence only: at most the 4x8 Grid addresses and eight
    // relevant scene values per address, witnessed by the retained model id.
    // The raw preset/capture-retarget source and address provenance are not rewritten.
    scene_bypasses: HashMap<(u32, u32), (u32, BypassRecord)>,
    sparse_state_invalidated: bool,
    inbound: InboundTelemetry,
    model_preset_catalog: ModelPresetCatalog,
}

impl StateDecoder {
    fn withdraw_preset_authority(&mut self) -> StateUpdate {
        self.preset = None;
        self.preset_exact = false;
        self.grid_layout_exact = false;
        self.parameter_overrides.clear();
        self.scene_bypasses.clear();
        self.metadata_observations = Default::default();
        self.state_revision = self.state_revision.saturating_add(1);
        let mut update = StateUpdate::new("gridTopology");
        update.grid_layout_complete = Some(false);
        update.refresh_required = Some(true);
        update
    }

    /// Name carried by the retained working buffer, never a catalog fallback.
    pub(crate) fn preset_buffer_name(&self) -> Option<&str> {
        match self.preset.as_ref()?.name.as_ref()? {
            binary_preset::Name::Name(name) => Some(name.as_str()),
        }
    }

    /// Returns an opaque snapshot of the retained current preset.
    ///
    /// Returns `None` when no preset is retained, when the retained copy is no longer an exact full
    /// preset (for example after a sparse Grid update), or when live parameter overrides are
    /// pending.
    pub fn preset_source_snapshot(&self) -> Option<PresetSourceSnapshot> {
        if !self.preset_exact || !self.parameter_overrides.is_empty() {
            return None;
        }
        self.preset.as_ref().map(|preset| PresetSourceSnapshot {
            preset: std::sync::Arc::new(preset.clone()),
        })
    }

    /// Returns whether the retained preset is still exact, no live parameter overrides are pending,
    /// and its contents equal `source`.
    pub fn preset_source_matches(&self, source: &PresetSourceSnapshot) -> bool {
        self.preset_exact
            && self.parameter_overrides.is_empty()
            && self
                .preset
                .as_ref()
                .is_some_and(|preset| preset == source.preset.as_ref())
    }

    /// Creates a decoder with no retained preset and no model catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards all retained state, including the preset, the model catalog and license state, and
    /// returns the decoder to its initial state.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Sparse parameter retention was lost. Only a full preset or reset can
    /// re-establish state; owners must publish this invalidation on decode Err.
    pub fn sparse_state_invalidated(&self) -> bool {
        self.sparse_state_invalidated
    }

    /// Which declared types reached [`Self::decode`] and produced nothing:
    /// the half of the inbound telemetry only a dispatch attempt can
    /// establish. Merged with the transport's half for a reader.
    pub fn inbound_telemetry(&self) -> &InboundTelemetry {
        &self.inbound
    }

    /// The retained fold of the model preset list. It starts unavailable and
    /// returns there on [`Self::reset`].
    pub fn model_preset_catalog(&self) -> &ModelPresetCatalog {
        &self.model_preset_catalog
    }

    /// The fold as a `modelPresetCatalog` update, for publication after a
    /// message or a failure changed it. The session stamp is left to the
    /// session runtime.
    pub fn model_preset_catalog_update(&self) -> StateUpdate {
        let mut update = StateUpdate::new("modelPresetCatalog");
        update.model_preset_catalog = Some(self.model_preset_catalog.view());
        update
    }

    /// Records the transport's loss total at the moment this decoder is created or reset.
    pub fn seed_inbound_loss(&mut self, total: u64) {
        self.model_preset_catalog.seed_inbound_loss(total);
    }

    /// Feeds the transport's running inbound loss total to the fold. Returns
    /// the update to publish when this invalidated it.
    pub fn observe_inbound_loss(&mut self, total: u64) -> Option<StateUpdate> {
        self.model_preset_catalog
            .observe_inbound_loss(total)
            .then(|| self.model_preset_catalog_update())
    }

    /// Returns the number of models in the installed catalog, including hidden ones.
    pub fn model_count(&self) -> usize {
        self.catalog.len()
    }

    /// Lists the selectable models of the installed catalog, with `available` as decided by
    /// [`Self::assert_model_available`].
    pub fn model_list(&self) -> ModelList {
        let mut list = ModelCatalog(self.catalog.clone()).model_list();
        for model in &mut list.models {
            model.available = self.assert_model_available(model.id).is_ok();
        }
        list
    }

    /// Admits a model for placement when the synchronized catalog lists it and does not hide it.
    ///
    /// Plugin models are admitted whatever the decoded license state says. QC Remote does not
    /// establish plugin ownership; inspect the device response (in the tested unowned model the
    /// block appeared locked and bypassed). There is no local plugin-availability precheck.
    pub fn assert_model_available(&self, model_id: u32) -> Result<(), String> {
        let model = self.catalog.get(&model_id).ok_or_else(|| {
            format!("Model {model_id} is absent from the synchronized QC catalog")
        })?;
        if model.hidden {
            return Err(format!("Model {model_id} is not a selectable QC model"));
        }
        Ok(())
    }

    /// The license state decoded from License UPDATEs, or `None` when none carried any.
    ///
    /// Informational only. Nothing gates a write on it, because the device enforces plugin
    /// ownership itself.
    pub fn license_observation(&self) -> Option<LicenseObservation> {
        if self.licenses_enabled.is_none()
            && self.licenses_deactivated.is_none()
            && self.licensed_plugins.is_empty()
        {
            return None;
        }
        let mut plugin_ids: Vec<u64> = self.licensed_plugins.iter().copied().collect();
        plugin_ids.sort_unstable();
        Some(LicenseObservation {
            enabled: self.licenses_enabled,
            deactivated: self.licenses_deactivated,
            plugin_ids,
        })
    }

    /// The sibling parameter whose ON position the model catalog declares as
    /// the reason `parameter_index` goes disabled -- ModelRepo's own
    /// `toggleOff` wiring, the same declaration the decoder reduces
    /// into `BlockParameter::enabled`. Callers that need to explain *why* a
    /// control is disabled must ask for the controller rather than guess it
    /// from sibling names: a block can carry several switches, and Dual
    /// Delay carries one per side.
    ///
    /// Only the plain on/off form is reported. A gate that also declares
    /// `toggleStep` selects on a multi-position controller, where "on" is not
    /// a single position, so it is not answerable in these terms and returns
    /// `None` (three parameters in the shipped catalogue).
    pub fn parameter_disabled_when_sibling_on(
        &self,
        model_id: u32,
        parameter_index: u32,
    ) -> Option<u32> {
        let spec = self
            .catalog
            .get(&model_id)?
            .parameters
            .get(&parameter_index)?;
        if !spec.enable_when_steps.is_empty() {
            return None;
        }
        spec.enable_when_off
    }

    /// Compact, content-free description of the retained chains as the move
    /// planner's grid context sees them, for the flight recorder.
    ///
    /// `move_block_operation` builds one grid-context row per retained chain,
    /// in chain order, with empty cells dropped; it never consults a chain's
    /// own `row`. The wire frame no longer carries that context (hardware
    /// experiment 2026-10-06: the bare frame applied 4/4, the sidecar form was
    /// ignored 2/4), so this is diagnostic only. Model ids and grid
    /// coordinates only.
    ///
    /// Format: `chain#<index>:row=<explicit row or ->[<model id>@<column or ->,..]`
    /// joined by `;`, with `exact=<bool>`.
    pub fn move_grid_layout_diagnostic(&self) -> String {
        let Some(preset) = self.preset.as_ref() else {
            return "no-retained-preset".into();
        };
        let chains = preset
            .chains
            .iter()
            .enumerate()
            .map(|(index, chain)| {
                let row = match chain.row {
                    Some(chain::Row::Row(value)) => value.to_string(),
                    _ => "-".into(),
                };
                let models = chain
                    .models
                    .iter()
                    .filter_map(|model| {
                        let id = model_hash(model).filter(|id| *id > 0)?;
                        let column = match model.column {
                            Some(model::Column::Column(value)) => value.to_string(),
                            _ => "-".into(),
                        };
                        Some(format!("{id}@{column}"))
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("chain#{index}:row={row}[{models}]")
            })
            .collect::<Vec<_>>()
            .join(";");
        format!(
            "exact={} {chains}",
            self.preset_exact || self.grid_layout_exact
        )
    }

    /// Build the grid-context payload for a model move from an exact current
    /// preset: every chain's model IDs in model order alongside the
    /// coordinate change.
    pub fn move_block_operation(
        &self,
        from_row: u32,
        from_column: u32,
        to_row: u32,
        to_column: u32,
        expected_model_id: Option<u32>,
    ) -> Result<DeviceOperation, String> {
        if from_row >= domain::GRID_ROWS
            || to_row >= domain::GRID_ROWS
            || from_column >= domain::GRID_COLUMNS
            || to_column >= domain::GRID_COLUMNS
        {
            return Err("Move coordinates must address rows 0-3 and columns 0-7".into());
        }
        if !(self.preset_exact || self.grid_layout_exact) {
            return Err(
                "The complete current grid is stale after a structural Grid update; refresh the current preset and retry"
                    .into(),
            );
        }
        let preset = self
            .preset
            .as_ref()
            .ok_or_else(|| "No complete current preset is retained".to_string())?;
        let (_, _, source_model) =
            locate_model(preset, from_row, from_column).map_err(|error| match error {
                LocateError::Absent => "The source block is absent; refresh and retry".to_string(),
                LocateError::Ambiguous => {
                    "The source block is ambiguous; refresh and retry".to_string()
                }
            })?;
        if expected_model_id.is_some_and(|expected| model_hash(source_model) != Some(expected)) {
            return Err("The source block changed; refresh and retry".into());
        }
        let destination_occupied = preset
            .chains
            .iter()
            .enumerate()
            .any(|(chain_index, chain)| {
                let row = chain_row(chain, chain_index as u32);
                chain.models.iter().enumerate().any(|(model_index, model)| {
                    model_hash(model).is_some_and(|model_id| model_id > 0)
                        && row == to_row
                        && model_column(model, model_index as u32) == to_column
                })
            });
        if (from_row, from_column) != (to_row, to_column) && destination_occupied {
            return Err("The destination block is occupied; refresh and retry".into());
        }
        let grid_model_ids = preset
            .chains
            .iter()
            .map(|chain| {
                chain
                    .models
                    .iter()
                    .filter_map(|model| model_hash(model).filter(|model_id| *model_id > 0))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        Ok(DeviceOperation::MoveBlock {
            from_row,
            from_column,
            to_row,
            to_column,
            grid_model_ids,
        })
    }

    /// Clone the exact current Neural Capture model for Cortex's
    /// `updateAuditioningModel` path and mutate parameter 5 in that clone.
    ///
    /// Sparse Grid pushes are sufficient for UI reduction but cannot prove a
    /// complete raw Model. Once one is observed, this therefore fails closed
    /// until the next full current-preset message refreshes the retained
    /// `BinaryPreset`.
    pub fn capture_retarget_operation(
        &self,
        row: u32,
        column: u32,
        expected_model_id: u32,
        key: &str,
    ) -> Result<DeviceOperation, String> {
        if row >= domain::GRID_ROWS || column >= domain::GRID_COLUMNS {
            return Err(
                "Capture retarget coordinates must address rows 0-3 and columns 0-7".into(),
            );
        }
        if !crate::commands::is_neural_capture_model(expected_model_id) {
            return Err("The expected target is not a Neural Capture model".into());
        }
        if !self.preset_exact {
            return Err(
                "The complete current model is stale after a sparse Grid update; refresh the current preset and retry"
                    .into(),
            );
        }
        let preset = self
            .preset
            .as_ref()
            .ok_or_else(|| "No complete current preset is retained".to_string())?;
        let (_, chain) = locate_chain(preset, row).map_err(|error| match error {
            LocateError::Absent => format!("Current preset has no row {row}"),
            LocateError::Ambiguous => format!("Current preset has ambiguous row {row}"),
        })?;
        let mut models = chain
            .models
            .iter()
            .enumerate()
            .filter(|(index, model)| model_column(model, *index as u32) == column);
        let (_, current_model) = models
            .next()
            .ok_or_else(|| format!("Current preset has no model at row {row}, column {column}"))?;
        if models.next().is_some() {
            return Err(format!(
                "Current preset has ambiguous models at row {row}, column {column}"
            ));
        }
        if model_hash(current_model) != Some(expected_model_id) {
            return Err("The Neural Capture target changed; refresh and retry".into());
        }

        let mut model = current_model.clone();
        model.column = Some(model::Column::Column(column));
        let mut parameter_matches = model
            .params
            .iter_mut()
            .enumerate()
            .filter(|(index, parameter)| param_index(parameter, *index as u32) == 5);
        let (_, parameter) = parameter_matches
            .next()
            .ok_or_else(|| "The current Neural Capture model has no parameter 5".to_string())?;
        if parameter_matches.next().is_some() {
            return Err(
                "The current Neural Capture model has ambiguous parameter 5 records".into(),
            );
        }
        parameter.index = Some(param::Index::Index(5));
        // The local Cortex mutation changes the active scene value only when
        // this parameter is scene-enabled; all other values and metadata stay
        // byte-for-byte represented by the cloned protobuf fields.
        // A singleton is the existing shared-value compatibility shape. A
        // multi-value record must actually contain the active scene index;
        // selecting its last value would edit another stored scene instead.
        let selected = if param_scene_mode(parameter) && parameter.param_values.len() != 1 {
            self.active_scene as usize
        } else {
            0
        };
        let value = parameter.param_values.get_mut(selected).ok_or_else(|| {
            "The current Neural Capture parameter 5 has no retained value".to_string()
        })?;
        value.value = Some(param_value::Value::StringValue(key.to_string()));

        // Resolved by array position, and deliberately left that way. The snapshot
        // projection (`bypass_records`) addresses a record by its own row and column
        // when it carries them and by position otherwise. A full preset as a unit
        // writes it carries neither, so the two rules pick the same record there;
        // they could differ only for a record whose explicit address disagrees with
        // its place in the list. This lookup feeds a write, and what a write sends
        // is not changed by a reading of the wire, so it keeps the positional rule
        // and the test below pins it.
        let bypass = preset
            .bypass
            .get(row as usize)
            .and_then(|bypass_row| bypass_row.col_bypass.get(column as usize))
            .cloned()
            .map(|mut value| {
                value.column = Some(col_bypass::Column::Column(column));
                value
            });

        Ok(DeviceOperation::RetargetCapture {
            row,
            column,
            active_scene: self.active_scene,
            key: key.to_string(),
            source_revision: self.state_revision,
            model,
            bypass,
        })
    }

    /// Returns whether the retained preset is still exact and its state revision equals `revision`,
    /// the revision recorded in an operation built by [`Self::capture_retarget_operation`].
    pub fn capture_retarget_revision_is_current(&self, revision: u64) -> bool {
        self.preset_exact && self.state_revision == revision
    }

    /// Installs a parsed model catalog, replacing any previous one.
    ///
    /// Returns the preset update re-projected against the new catalog, or an empty list when no
    /// preset is retained.
    pub fn install_catalog(&mut self, catalog: ModelCatalog) -> Vec<StateUpdate> {
        self.catalog = catalog.0;
        self.preset_update(true).into_iter().collect()
    }

    /// Decodes one assembled QC message of type `message_type` and folds it into the retained
    /// state.
    ///
    /// Gzip-compressed payloads are inflated first. Returns the normalized updates the message
    /// produced; message types without a handler, and messages that report nothing, yield an empty
    /// list. Types without a handler are recorded in the inbound telemetry.
    ///
    /// # Errors
    ///
    /// Returns a [`StateDecodeError`] when the payload exceeds its type's size limit, cannot be
    /// inflated or parsed, or carries values outside their domain.
    pub fn decode(
        &mut self,
        message_type: u16,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let decoded = self.decode_message(message_type, payload);
        // Every way a model preset message can fail (size limit, inflate,
        // protobuf) leaves the retained list unable to vouch for itself.
        if decoded.is_err() && message_type == profile::MESSAGE_TYPE_MODEL_PRESET {
            self.model_preset_catalog.note_decode_failure();
        }
        decoded
    }

    fn decode_message(
        &mut self,
        message_type: u16,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        validate_wire_payload(message_type, payload)?;
        let decoded = maybe_gunzip(payload)?;
        let dispatched = match message_type {
            profile::MESSAGE_TYPE_GRID => self.decode_grid(&decoded),
            profile::MESSAGE_TYPE_GRID_MOVE => self.decode_grid_move(&decoded),
            profile::MESSAGE_TYPE_SETLIST_POSITION => self.decode_position(&decoded),
            profile::MESSAGE_TYPE_IO_SETTINGS => self.decode_io_settings(&decoded),
            profile::MESSAGE_TYPE_MIDI_SETTINGS => self.decode_midi_settings(&decoded),
            profile::MESSAGE_TYPE_SCENE => self.decode_scene(&decoded),
            profile::MESSAGE_TYPE_MODE => self.decode_mode(&decoded),
            profile::MESSAGE_TYPE_RECALL_PRESET => self.decode_preset_message(&decoded),
            profile::MESSAGE_TYPE_MASTER_VOLUME => self.decode_master_volume(&decoded),
            profile::MESSAGE_TYPE_DEFAULT_PARAMETERS => self.decode_default_parameters(&decoded),
            profile::MESSAGE_TYPE_UNDO_REDO => self.decode_undo_redo(&decoded),
            profile::MESSAGE_TYPE_SCENE_COPY => self.decode_scene_copy(&decoded),
            profile::MESSAGE_TYPE_SCENE_LABEL => self.decode_scene_label(&decoded),
            profile::MESSAGE_TYPE_SHOW_GIG_VIEW => self.decode_show_gig_view(&decoded),
            profile::MESSAGE_TYPE_CPU_LOAD => self.decode_cpu_load(&decoded),
            profile::MESSAGE_TYPE_GLOBAL_TEMPO => self.decode_global_tempo(&decoded),
            profile::MESSAGE_TYPE_PRESET_DIRTY => self.decode_dirty(&decoded),
            profile::MESSAGE_TYPE_SCENE_COLOR => self.decode_scene_color(&decoded),
            profile::MESSAGE_TYPE_NEW_MODELS => self.decode_new_models(&decoded),
            profile::MESSAGE_TYPE_SUSPEND_CONNECTION => self.decode_suspend_connection(&decoded),
            profile::MESSAGE_TYPE_BULK_OPERATION => self.decode_bulk_operation(&decoded),
            profile::MESSAGE_TYPE_LICENSE => self.decode_license(&decoded),
            profile::MESSAGE_TYPE_MODEL_REPO => self.decode_model_repo(&decoded),
            profile::MESSAGE_TYPE_MODEL_PRESET => self.decode_model_presets(&decoded),
            // No handler. An empty update list here is indistinguishable
            // from a handled message that had nothing to report, which is
            // how a type the device pushes disappears without trace. Ask the
            // protocol's own decoders what the body was instead, and record
            // whichever answer comes back. Counted at the default itself, so
            // the tally cannot drift from the table above it.
            _ => {
                self.inbound
                    .record_dispatch_fallthrough(message_type, &decoded);
                return Ok(Vec::new());
            }
        };
        // A handled type whose body did not decode belongs in the same
        // tally, from the one place that sees the typed error.
        if let Err(StateDecodeError::Protobuf { message_type, .. }) = &dispatched {
            self.inbound.record_decode_failure(*message_type);
        }
        dispatched
    }

    fn decode_midi_settings(
        &mut self,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::MidiSettingsMessage = decode(8, payload)?;
        if !is_update_action(message.action) || message.request_id.is_some() {
            return Ok(Vec::new());
        }
        let mut update = StateUpdate::new("midiOut");
        // A group that carries a source belongs to that source. A group that carries none is the
        // preset-load list: the desktop application's receiver files a group without a source
        // under the preset-load rows, and a unit has been seen to report a preset-load edit that
        // way, in the general field, with every one of the twelve rows present. The sender
        // writes preset-load rows in the dedicated field instead, and both are accepted.
        let mut sourced = Vec::new();
        let mut preset_load = Vec::new();

        let mut preset_load_seen = false;
        if let Some(pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(groups)) =
            message.general_midi_messages
        {
            for group in groups.messages {
                if group.msg.len() > 12 {
                    return Err(StateDecodeError::RetentionLimit("MIDI source slots"));
                }
                match group.source {
                    Some(pa::general_midi_message::Source::Source(source)) => {
                        if source >= 10
                            || sourced
                                .iter()
                                .any(|entry: &MidiOutSource| entry.source == source)
                        {
                            return Err(StateDecodeError::RetentionLimit("MIDI source identities"));
                        }
                        sourced.push(MidiOutSource {
                            source,
                            messages: midi_slot_list(group.msg.iter()),
                            ..Default::default()
                        })
                    }
                    None => {
                        preset_load_seen = true;
                        preset_load.extend(group.msg);
                    }
                }
            }
            update.midi_out = Some(sourced);
        }
        if let Some(pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(groups)) =
            message.preset_load_messages
        {
            // Every group of this field is preset-load, whatever source it carries; the source,
            // which the sender leaves absent, is not read.
            preset_load_seen = true;
            for group in &groups.messages {
                preset_load.extend(group.msg.iter().cloned());
            }
        }
        if preset_load_seen {
            if preset_load.len() > 12 {
                return Err(StateDecodeError::RetentionLimit("preset-load MIDI slots"));
            }
            update.preset_load_midi_out = Some(midi_slot_list(preset_load.iter()));
        }
        // Commit only after the entire bounded batch has been validated.
        if let Some(sources) = &update.midi_out {
            for source in sources {
                self.metadata_observations
                    .midi_sources
                    .insert(source.source, source.messages.clone());
            }
        }
        if let Some(messages) = &update.preset_load_midi_out {
            self.metadata_observations.preset_load = Some(messages.clone());
        }
        if update
            .midi_out
            .as_ref()
            .is_some_and(|sources| !sources.is_empty())
            || update.preset_load_midi_out.is_some()
        {
            self.preset_exact = false;
            self.state_revision = self.state_revision.saturating_add(1);
        }
        Ok(
            (update.midi_out.is_some() || update.preset_load_midi_out.is_some())
                .then_some(update)
                .into_iter()
                .collect(),
        )
    }

    fn decode_default_parameters(
        &self,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::DefaultParametersMessage = decode(19, payload)?;
        if message.row < 0 || message.column < 0 {
            return Err(StateDecodeError::InvalidGridCoordinate {
                message_type: profile::MESSAGE_TYPE_DEFAULT_PARAMETERS,
            });
        }
        let mut update = StateUpdate::new("defaultParameters");
        update.row = Some(message.row as u32);
        update.column = Some(message.column as u32);
        update.default_parameter_count = Some(message.all_defaults.len() as u32);
        Ok(vec![update])
    }

    fn decode_undo_redo(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::UndoRedoMessage = decode(21, payload)?;
        let mut update = StateUpdate::new("history");
        // The device sends fixed-length lists even with no known undoable edit.
        // Their entries have no established availability interpretation. Preserve raw evidence,
        // but leave availability unknown rather than presenting list length as capability.
        update.undo_frames = Some(message.undo_frames);
        update.redo_frames = Some(message.redo_frames);
        Ok(vec![update])
    }

    fn decode_scene_copy(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::SceneCopyMessage = decode(22, payload)?;
        if !is_update_action(message.action)
            || message.request_id.is_some()
            || !(0..8).contains(&message.from_index)
            || !(0..8).contains(&message.to_index)
        {
            return Ok(Vec::new());
        }
        // Copy/swap changes scene parameters and bypasses, not just labels.
        // Until a full publication arrives, no raw source is authoritative.
        let update = self.withdraw_preset_authority();
        Ok(vec![update])
    }

    fn decode_show_gig_view(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::ShowGigViewMessage = decode(24, payload)?;
        let mut update = StateUpdate::new("gigView");
        update.gig_view_shown = Some(message.show);
        Ok(vec![update])
    }

    fn decode_cpu_load(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::CpuLoadMessage = decode(26, payload)?;
        if let Some(pa::cpu_load_message::CpuTotalLoad::CpuTotalLoad(value)) =
            message.cpu_total_load
        {
            require_finite_projection(profile::MESSAGE_TYPE_CPU_LOAD, "total CPU load", value)?;
        }
        for column in message.chains.iter().flat_map(|chain| &chain.columns) {
            require_finite_projection(
                profile::MESSAGE_TYPE_CPU_LOAD,
                "column CPU load",
                column.cpu_load,
            )?;
        }
        let mut update = StateUpdate::new("cpuLoad");
        update.cpu_total_load = message.cpu_total_load.map(|value| match value {
            pa::cpu_load_message::CpuTotalLoad::CpuTotalLoad(load) => load,
        });
        update.cpu_columns = Some(
            message
                .chains
                .into_iter()
                .flat_map(|chain| chain.columns)
                .map(|column| CpuColumnLoad {
                    load: column.cpu_load,
                    core: u32::from(column.is_on_core2) + 1,
                })
                .collect(),
        );
        Ok(vec![update])
    }

    fn decode_new_models(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::NewModelsMessage = decode(50, payload)?;
        let mut update = StateUpdate::new("catalog");
        update.catalog_refresh = Some(true);
        update.new_model_ids = Some(message.models);
        Ok(vec![update])
    }

    fn decode_suspend_connection(
        &self,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::SuspendConnectionMessage = decode(53, payload)?;
        let Some(pa::suspend_connection_message::Suspend::Suspend(suspended)) = message.suspend
        else {
            return Ok(Vec::new());
        };
        let mut update = StateUpdate::new("suspend");
        update.suspended = Some(suspended);
        update.suspend_reason = message.reason.map(|reason| match reason {
            pa::suspend_connection_message::ReasonOneOf::Reason(value) => {
                pa::suspend_connection_message::Reason::try_from(value)
                    .map(|value| value.as_str_name().to_owned())
                    .unwrap_or_else(|_| format!("UNKNOWN_{value}"))
            }
        });
        Ok(vec![update])
    }

    fn decode_bulk_operation(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::BulkOperationMessage = decode(57, payload)?;
        if let Some(pa::bulk_operation_message::Progress::Progress(value)) = message.progress {
            require_finite_projection(
                profile::MESSAGE_TYPE_BULK_OPERATION,
                "bulk progress",
                value,
            )?;
        }
        let mut update = StateUpdate::new("bulkOperation");
        update.bulk_operation = Some(BulkOperationState {
            message: message.progress_message.map(|value| match value {
                pa::bulk_operation_message::ProgressMessage::ProgressMessage(value) => value,
            }),
            blocking: message.blocking.map(|value| match value {
                pa::bulk_operation_message::Blocking::Blocking(value) => value,
            }),
            cancellable: message.cancellable.map(|value| match value {
                pa::bulk_operation_message::Cancellable::Cancellable(value) => value,
            }),
            progress: message.progress.map(|value| match value {
                pa::bulk_operation_message::Progress::Progress(value) => value,
            }),
            finished: message.finished.map(|value| match value {
                pa::bulk_operation_message::Finished::Finished(value) => value,
            }),
        });
        Ok(vec![update])
    }

    fn decode_model_presets(
        &mut self,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::ModelPresetMessage = decode(profile::MESSAGE_TYPE_MODEL_PRESET, payload)?;
        // The fold reads the action, the presets and every loaded-preset
        // field; the model body of a preset is never decoded into the update.
        self.model_preset_catalog.fold(&message);
        Ok(vec![self.model_preset_catalog_update()])
    }

    fn decode_license(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::LicenseMessage = decode(profile::MESSAGE_TYPE_LICENSE, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        if let Some(pa::license_message::IsEnabled::IsEnabled(value)) = message.is_enabled {
            self.licenses_enabled = Some(value);
        }
        if let Some(pa::license_message::IsDeactivated::IsDeactivated(value)) =
            message.is_deactivated
        {
            self.licenses_deactivated = Some(value);
        }
        // UPDATE replaces the license repository wholesale, including with
        // an empty list. supported_plugin_ids is firmware capability.
        self.licensed_plugins = message
            .licenses
            .into_iter()
            .map(|item| item.plugin_id)
            .collect();
        Ok(Vec::new())
    }

    fn decode_grid_move(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::GridMoveMessage = decode(profile::MESSAGE_TYPE_GRID_MOVE, payload)?;
        if message.r#move.is_empty() {
            return Ok(Vec::new());
        }
        if message.r#move.iter().any(|item| {
            item.from_row >= domain::GRID_ROWS
                || item.to_row >= domain::GRID_ROWS
                || item.from_col >= domain::GRID_COLUMNS
                || item.to_col >= domain::GRID_COLUMNS
        }) {
            return Err(StateDecodeError::InvalidGridCoordinate {
                message_type: profile::MESSAGE_TYPE_GRID_MOVE,
            });
        }
        let layout_was_exact = self.preset_exact || self.grid_layout_exact;
        self.preset_exact = false;
        // Every early return below leaves the layout unproven; only the
        // fully applied move re-establishes it.
        self.grid_layout_exact = false;
        // Raw bypass addresses are not proven to move with these model records.
        // Never attach an old address's scene evidence to the destination model.
        self.scene_bypasses.clear();
        self.state_revision = self.state_revision.saturating_add(1);
        let unproven = || {
            let mut update = StateUpdate::new("gridTopology");
            update.grid_layout_complete = Some(false);
            vec![update]
        };
        let Some(preset) = self.preset.as_mut() else {
            return Ok(unproven());
        };

        // A batch is reconstructed from the single retained pre-state below.
        // Reusing a source would fabricate copies; reusing a destination or
        // moving into an unmoved occupied cell cannot prove the final layout.
        // Preserve the bytes but leave topology unproven, as for an unresolved
        // source/destination. This is a software-authority rule, not a claim
        // about every batch shape firmware may publish.
        let mut sources = HashSet::new();
        let mut destinations = HashSet::new();
        for item in &message.r#move {
            if !sources.insert((item.from_row, item.from_col))
                || !destinations.insert((item.to_row, item.to_col))
            {
                return Ok(unproven());
            }
        }
        let mut empty_destinations = Vec::new();
        for item in &message.r#move {
            if !sources.contains(&(item.to_row, item.to_col)) {
                match locate_model(preset, item.to_row, item.to_col) {
                    Ok((_, _, model)) if model_hash(model).is_some_and(|id| id > 0) => {
                        return Ok(unproven());
                    }
                    Ok((chain_index, model_index, _)) => {
                        empty_destinations.push((chain_index, model_index));
                    }
                    Err(LocateError::Ambiguous) => return Ok(unproven()),
                    _ => {}
                }
            }
        }
        let mut moved = Vec::with_capacity(message.r#move.len());
        for item in &message.r#move {
            let Ok((chain_index, model_index, model)) =
                locate_model(preset, item.from_row, item.from_col)
            else {
                return Ok(unproven());
            };
            let model = model.clone();
            let Ok((destination_index, _)) = locate_chain(preset, item.to_row) else {
                return Ok(unproven());
            };
            moved.push((
                chain_index,
                model_index,
                destination_index,
                item.to_col,
                model,
            ));
        }
        let mut removals: HashMap<usize, Vec<usize>> = HashMap::new();
        for (chain_index, model_index) in empty_destinations {
            removals.entry(chain_index).or_default().push(model_index);
        }
        for (chain_index, model_index, ..) in &moved {
            removals.entry(*chain_index).or_default().push(*model_index);
        }
        // Full recalls may address cells by vector position. Freeze those
        // positions only after every lookup succeeds, before removing any
        // source/empty destination can shift surviving cells.
        for chain in &mut preset.chains {
            for (index, model) in chain.models.iter_mut().enumerate() {
                model.column = Some(model::Column::Column(model_column(model, index as u32)));
            }
        }
        for (chain_index, indexes) in removals.iter_mut() {
            indexes.sort_unstable_by(|left, right| right.cmp(left));
            indexes.dedup();
            for index in indexes.iter().copied() {
                preset.chains[*chain_index].models.remove(index);
            }
        }
        for (_, _, destination_index, to_column, mut model) in moved {
            model.column = Some(model::Column::Column(to_column));
            preset.chains[destination_index].models.push(model);
        }
        self.grid_layout_exact = layout_was_exact;
        Ok(self.preset_update(false).into_iter().collect())
    }

    fn decode_io_settings(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message: pa::IoSettingsMessage = decode(profile::MESSAGE_TYPE_IO_SETTINGS, payload)?;
        if !is_update_action(message.action) || message.request_id.is_some() {
            return Ok(Vec::new());
        }
        let Some(pa::io_settings_message::Settings::Settings(settings)) = message.settings else {
            return Ok(Vec::new());
        };
        let mut ports = Vec::new();
        for port in settings.in_port {
            if let Some(pa::input_port_settings::Plugged::Plugged(plugged)) = port.plugged {
                ports.push(IoPortState {
                    kind: "input".into(),
                    id: port.input_port_id as i32,
                    label: input_label(port.input_port_id),
                    plugged,
                });
            }
        }
        for port in settings.out_port {
            if let Some(pa::output_port_settings::Plugged::Plugged(plugged)) = port.plugged {
                ports.push(IoPortState {
                    kind: "output".into(),
                    id: port.output_port_id as i32,
                    label: output_label(port.output_port_id),
                    plugged,
                });
            }
        }
        if let Some(pa::port_settings::HpPort::HpPort(port)) = settings.hp_port {
            if let Some(pa::headphones_settings::Plugged::Plugged(plugged)) = port.plugged {
                ports.push(IoPortState {
                    kind: "headphones".into(),
                    id: 0,
                    label: "Headphones".into(),
                    plugged,
                });
            }
        }
        if let Some(pa::port_settings::UsbPort::UsbPort(port)) = settings.usb_port {
            if let Some(pa::usb_port_settings::Plugged::Plugged(plugged)) = port.plugged {
                ports.push(IoPortState {
                    kind: "usb".into(),
                    id: 0,
                    label: "USB".into(),
                    plugged,
                });
            }
        }
        // The label uses the reported id as is. Every hardware run reported
        // exactly two expression ports, carrying the ids 1 and 2 explicitly on
        // the wire, and never an id 0; the unit's own screens (CorOS 4.1.0)
        // name exactly two expression jacks, EXP 1 and EXP 2. Adding one, as
        // this code once did, printed them as "EXP 2" and "EXP 3". No recorded
        // run had a pedal plugged in, so that id 1 is the jack printed EXP 1
        // (rather than EXP 2) is still an inference; the per-jack hardware
        // check (one pedal, one jack at a time) is pending.
        for port in settings.exp_port {
            if let Some(pa::exp_port_settings::Plugged::Plugged(plugged)) = port.plugged {
                ports.push(IoPortState {
                    kind: "expression".into(),
                    id: port.exp_port_id,
                    label: expression_port_label(port.exp_port_id),
                    plugged,
                });
            }
        }
        if ports.is_empty() {
            return Ok(Vec::new());
        }
        let mut update = StateUpdate::new("ioPorts");
        update.io_ports = Some(ports);
        Ok(vec![update])
    }

    /// Returns the parameters and current values of the block at grid `row` and `column` of the
    /// retained preset.
    ///
    /// Returns `None` when no preset is retained or no such block exists. Column 8 addresses the
    /// row's splitter and column 9 its mixer.
    pub fn block_details(&self, row: u32, column: u32) -> Option<BlockDetails> {
        let preset = self.preset.as_ref()?;
        let chain = preset
            .chains
            .iter()
            .enumerate()
            .find(|(index, item)| chain_row(item, *index as u32) == row)?
            .1;
        let routing_node = match column {
            8 => Some("splitter"),
            9 => Some("mixer"),
            _ => None,
        };
        let (model, model_id) = match routing_node {
            Some("splitter") => {
                let points = chain.split_control_points.first()?;
                if points.split < 0 {
                    return None;
                }
                let (model, model_id, node) = splitter_target(chain)?;
                return self.parameter_target_details(row, column, model, model_id, Some(node));
            }
            Some("mixer") => {
                let points = chain.split_control_points.first()?;
                if points.mix < 0 {
                    return None;
                }
                (chain.mixer.first()?, 11_000)
            }
            _ => {
                let model = chain
                    .models
                    .iter()
                    .enumerate()
                    .find(|(index, item)| model_column(item, *index as u32) == column)?
                    .1;
                (model, model_hash(model)?)
            }
        };
        self.parameter_target_details(row, column, model, model_id, routing_node)
    }

    /// Parameters attached to the row itself rather than to a Grid cell.
    /// These are explicit control targets in the public API; the private column
    /// discriminator only keeps optimistic live-value overrides collision-free.
    pub fn lane_control_details(&self, row: u32, control: &str) -> Option<BlockDetails> {
        let preset = self.preset.as_ref()?;
        let chain = preset
            .chains
            .iter()
            .enumerate()
            .find(|(index, item)| chain_row(item, *index as u32) == row)?
            .1;
        let (model, model_id, discriminator) = match control {
            "inputGate" => (chain.input_control.first()?, 28_000, 10),
            "laneOutput" => (chain.output_control.first()?, 23_000, 11),
            _ => return None,
        };
        self.parameter_target_details(row, discriminator, model, model_id, None)
    }

    fn parameter_target_details(
        &self,
        row: u32,
        column: u32,
        model: &Model,
        model_id: u32,
        routing_node: Option<&str>,
    ) -> Option<BlockDetails> {
        let info = self.catalog.get(&model_id);
        let mut current_values = HashMap::new();
        for (positional, parameter) in model.params.iter().enumerate() {
            let index = param_index(parameter, positional as u32);
            let options = parameter_options(
                info,
                parameter,
                index,
                model_id,
                self.active_scene,
                routing_node,
            );
            current_values.insert(
                index,
                self.parameter_overrides
                    .get(&(row, column, index))
                    .copied()
                    .or_else(|| normalized_parameter(parameter, self.active_scene, &options)),
            );
        }
        let category_key = info
            .map(|item| normalized_category(&item.category))
            .unwrap_or_default();
        let show_offscreen = category_key.contains("cabsim") || category_key.contains("irloader");
        let lane_control = matches!(column, 10 | 11);
        let splitter_visible = if routing_node == Some("splitter") {
            current_values
                .get(&0)
                .and_then(|value| *value)
                .map(|value| {
                    match (value * 2.0).round().clamp(0.0, 2.0) as u32 {
                        0 => [0, 1, 3, 4].as_slice(),
                        1 => [0, 1, 2].as_slice(),
                        _ => [0, 1, 5, 6].as_slice(),
                    }
                    .to_vec()
                })
        } else {
            None
        };
        let mut parameters = Vec::new();
        for (positional, parameter) in model.params.iter().enumerate() {
            let index = param_index(parameter, positional as u32);
            if let Some(block_parameter) = self.visible_block_parameter(
                row,
                column,
                model_id,
                routing_node,
                info,
                &current_values,
                lane_control,
                show_offscreen,
                &splitter_visible,
                parameter,
                index,
            ) {
                parameters.push(block_parameter);
            }
        }
        Some(BlockDetails {
            row,
            column,
            model_id,
            name: info
                .map(|item| item.name.clone())
                .unwrap_or_else(|| format!("Model {model_id}")),
            category: info
                .map(|item| display_category(&item.category))
                .unwrap_or_else(|| "Utility".into()),
            scene: self.active_scene,
            bypass_expression: model_bypass_expression(model),
            parameters,
        })
    }

    /// Build the public `BlockParameter` for one already-located parameter, or
    /// `None` when it is filtered out (hidden, conditionally hidden, not part
    /// of the active splitter mode, etc.) or currently has no normalized
    /// value. Shared by `parameter_target_details` (called once per
    /// parameter) and `single_parameter_target_detail_for_model` (called
    /// exactly once, for the single parameter a write-verification tick
    /// actually needs).
    #[allow(clippy::too_many_arguments)]
    fn visible_block_parameter(
        &self,
        row: u32,
        column: u32,
        model_id: u32,
        routing_node: Option<&str>,
        info: Option<&ModelInfo>,
        current_values: &HashMap<u32, Option<f64>>,
        lane_control: bool,
        show_offscreen: bool,
        splitter_visible: &Option<Vec<u32>>,
        parameter: &Param,
        index: u32,
    ) -> Option<BlockParameter> {
        let spec = info.and_then(|item| item.parameters.get(&index));
        // Input Gate and Lane Output are hidden catalog models because they
        // are row controls rather than addable Grid devices. Their own
        // parameter screen is nevertheless public and editable. Conversely,
        // CorOS currently carries one undocumented fifth value in both
        // control records; never present that unknown value as a writable
        // "Parameter 5" merely because it exists in the preset protobuf.
        // The same holds for every other block: a protobuf slot the model's
        // catalog entry does not declare is not a control the unit draws, so it
        // is not invented as "Parameter N" with a value of 0.0 either.
        if info.is_some() && spec.is_none() {
            return None;
        }
        if spec.is_some_and(|item| {
            (!lane_control && (item.hidden || (!item.screen_visible && !show_offscreen)))
                || conditional_parameter_hidden(model_id, index, current_values)
        }) {
            return None;
        }
        if splitter_visible
            .as_ref()
            .is_some_and(|visible| !visible.contains(&index))
        {
            return None;
        }
        if routing_node == Some("mixer") && spec.is_some_and(|item| item.name == "SPLIT MODE") {
            return None;
        }
        let options = parameter_options(
            info,
            parameter,
            index,
            model_id,
            self.active_scene,
            routing_node,
        );
        let scene_mode = param_scene_mode(parameter);
        let normalized = self
            .parameter_overrides
            .get(&(row, column, index))
            .copied()
            .or_else(|| normalized_parameter(parameter, self.active_scene, &options))?;
        let minimum = spec.map(|item| item.minimum).unwrap_or(0.0);
        let maximum = spec.map(|item| item.maximum).unwrap_or(1.0);
        let units = spec.map(|item| item.units.clone()).unwrap_or_default();
        let scale_known = !options.is_empty()
            || (spec.is_some_and(|item| item.scale_known)
                && normalized_to_real(normalized, spec).is_finite());
        let value_scale = if !scale_known {
            "unknown"
        } else if !options.is_empty() {
            "options"
        } else {
            spec.map(|item| item.value_scale.as_str())
                .unwrap_or("unknown")
        };
        let display_precision = spec.and_then(|item| item.display_precision());
        let display = if !options.is_empty() {
            let selected = ((normalized * (options.len().saturating_sub(1)) as f64).round()
                as usize)
                .min(options.len() - 1);
            options[selected].clone()
        } else if scale_known {
            special_value_label(normalized, spec)
                .or_else(|| labelled_end_value_label(normalized_to_real(normalized, spec), spec))
                .unwrap_or_else(|| {
                    format_parameter_number(normalized_to_real(normalized, spec), display_precision)
                })
        } else {
            trim_number(normalized, 3)
        };
        Some(BlockParameter {
            index,
            display_position: spec.map(|item| item.display_position).unwrap_or(index),
            name: spec
                .map(|item| item.name.clone())
                .unwrap_or_else(|| format!("Parameter {}", u64::from(index) + 1)),
            normalized_value: Some(normalized),
            display_value: display,
            units,
            r#type: spec.map(|item| item.r#type.clone()).unwrap_or_else(|| {
                if options.is_empty() {
                    "float".into()
                } else {
                    "enum".into()
                }
            }),
            minimum,
            maximum,
            value_scale: value_scale.into(),
            scale_exponent: scale_known
                .then(|| spec.and_then(|item| item.scale_exponent))
                .flatten(),
            scale_points: if scale_known {
                spec.map(|item| item.scale_points.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            scale_known,
            display_precision,
            minimum_label: spec.and_then(|item| item.minimum_label.clone()),
            midpoint_label: spec.and_then(|item| item.midpoint_label.clone()),
            maximum_label: spec.and_then(|item| item.maximum_label.clone()),
            show_as_integer: spec.is_some_and(|item| item.show_as_integer),
            has_off_position: spec.is_some_and(ParameterSpec::has_off_position),
            floor_display: scale_known
                .then(|| spec.and_then(ParameterSpec::floor_display))
                .flatten(),
            floor_normalized: scale_known
                .then(|| spec.and_then(ParameterSpec::floor_normalized))
                .flatten(),
            steps: spec.and_then(|item| item.steps),
            scene_mode,
            options,
            writable: !parameter.param_values.is_empty()
                && !spec.is_some_and(|item| item.r#type.eq_ignore_ascii_case("grMeter")),
            enabled: spec
                .map(|item| parameter_enabled(item, current_values, info))
                .unwrap_or(true),
            expression_assignable: routing_node != Some("legacySplitter")
                && spec.map(|item| item.expression_assignable).unwrap_or(true),
            linked_scene_mode: spec.and_then(|item| item.linked_scene_mode),
            expression: param_expression(parameter),
            expression_minimum: param_expression_minimum(parameter),
            expression_maximum: param_expression_maximum(parameter),
            led_value: None,
            wire_value_kind: if parameter
                .param_values
                .iter()
                .any(|value| matches!(value.value, Some(param_value::Value::StringValue(_))))
            {
                "text".into()
            } else {
                "numeric".into()
            },
        })
    }

    /// Narrower sibling of `parameter_target_details`: locates and
    /// materializes only the parameter at `target_index`, skipping the
    /// label-clone and display-formatting work for every other parameter on
    /// the block. Used by write-verification polling
    /// (`advance_gateway_verification`), which only ever needs one
    /// parameter's value each tick and previously rebuilt (and discarded) the
    /// entire block's parameter list just to get it.
    fn single_parameter_target_detail_for_model(
        &self,
        row: u32,
        column: u32,
        model: &Model,
        model_id: u32,
        routing_node: Option<&str>,
        target_index: u32,
    ) -> Option<BlockParameter> {
        let info = self.catalog.get(&model_id);
        let mut current_values = HashMap::new();
        for (positional, parameter) in model.params.iter().enumerate() {
            let index = param_index(parameter, positional as u32);
            let options = parameter_options(
                info,
                parameter,
                index,
                model_id,
                self.active_scene,
                routing_node,
            );
            current_values.insert(
                index,
                self.parameter_overrides
                    .get(&(row, column, index))
                    .copied()
                    .or_else(|| normalized_parameter(parameter, self.active_scene, &options)),
            );
        }
        let category_key = info
            .map(|item| normalized_category(&item.category))
            .unwrap_or_default();
        let show_offscreen = category_key.contains("cabsim") || category_key.contains("irloader");
        let lane_control = matches!(column, 10 | 11);
        let splitter_visible = if routing_node == Some("splitter") {
            current_values
                .get(&0)
                .and_then(|value| *value)
                .map(|value| {
                    match (value * 2.0).round().clamp(0.0, 2.0) as u32 {
                        0 => [0, 1, 3, 4].as_slice(),
                        1 => [0, 1, 2].as_slice(),
                        _ => [0, 1, 5, 6].as_slice(),
                    }
                    .to_vec()
                })
        } else {
            None
        };
        let (positional, parameter) =
            model
                .params
                .iter()
                .enumerate()
                .find(|(positional, parameter)| {
                    param_index(parameter, *positional as u32) == target_index
                })?;
        let index = param_index(parameter, positional as u32);
        self.visible_block_parameter(
            row,
            column,
            model_id,
            routing_node,
            info,
            &current_values,
            lane_control,
            show_offscreen,
            &splitter_visible,
            parameter,
            index,
        )
    }

    /// Narrower sibling of `block_details` that returns only the parameter at
    /// `index`, without cloning/formatting the rest of the block's
    /// parameters.
    fn single_block_parameter_detail(
        &self,
        row: u32,
        column: u32,
        index: u32,
    ) -> Option<BlockParameter> {
        let preset = self.preset.as_ref()?;
        let (_, chain) = locate_chain(preset, row).ok()?;
        let routing_node = match column {
            8 => Some("splitter"),
            9 => Some("mixer"),
            _ => None,
        };
        let (model, model_id) = match routing_node {
            Some("splitter") => {
                let points = chain.split_control_points.first()?;
                if points.split < 0 {
                    return None;
                }
                let (model, model_id, node) = splitter_target(chain)?;
                return self.single_parameter_target_detail_for_model(
                    row,
                    column,
                    model,
                    model_id,
                    Some(node),
                    index,
                );
            }
            Some("mixer") => {
                let points = chain.split_control_points.first()?;
                if points.mix < 0 {
                    return None;
                }
                (chain.mixer.first()?, 11_000)
            }
            _ => {
                let model = chain
                    .models
                    .iter()
                    .enumerate()
                    .find(|(model_index, item)| model_column(item, *model_index as u32) == column)?
                    .1;
                (model, model_hash(model)?)
            }
        };
        self.single_parameter_target_detail_for_model(
            row,
            column,
            model,
            model_id,
            routing_node,
            index,
        )
    }

    /// Narrower sibling of `lane_control_details` that returns only the
    /// parameter at `index`.
    fn single_lane_control_parameter_detail(
        &self,
        row: u32,
        control: &str,
        index: u32,
    ) -> Option<BlockParameter> {
        let preset = self.preset.as_ref()?;
        let (_, chain) = locate_chain(preset, row).ok()?;
        let (model, model_id, discriminator) = match control {
            "inputGate" => (chain.input_control.first()?, 28_000, 10),
            "laneOutput" => (chain.output_control.first()?, 23_000, 11),
            _ => return None,
        };
        self.single_parameter_target_detail_for_model(
            row,
            discriminator,
            model,
            model_id,
            None,
            index,
        )
    }

    /// Narrow accessor for write-verification polling: returns just the one
    /// `BlockParameter` a verification tick needs, choosing between the
    /// lane-control (columns 10/11) and generic block lookup exactly as
    /// `block_details`/`lane_control_details` do, but without materializing
    /// the full parameter list only to discard everything but one entry.
    /// `block_details`/`lane_control_details` themselves are unchanged and
    /// remain the right call for callers that need the whole block.
    pub fn single_parameter_target_detail(
        &self,
        row: u32,
        column: u32,
        index: u32,
    ) -> Option<BlockParameter> {
        match column {
            10 => self.single_lane_control_parameter_detail(row, "inputGate", index),
            11 => self.single_lane_control_parameter_detail(row, "laneOutput", index),
            _ => self.single_block_parameter_detail(row, column, index),
        }
    }

    fn decode_scene(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message = decode::<pa::SceneMessage>(profile::MESSAGE_TYPE_SCENE, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        let Some(pa::scene_message::SelectedScene::SelectedScene(scene)) = message.selected_scene
        else {
            return Ok(Vec::new());
        };
        if scene >= domain::SCENE_COUNT {
            return Err(StateDecodeError::InvalidSelectedScene(scene));
        }
        // The sparse numeric cache is not scene-qualified. Rather than carry
        // an active-scene edit into another scene (or revive its old raw value
        // on return), require one event-driven full publication.
        let invalidated = (self.active_scene != scene && !self.parameter_overrides.is_empty())
            .then(|| self.withdraw_preset_authority());
        self.active_scene = scene;
        self.state_revision = self.state_revision.saturating_add(1);
        let mut update = StateUpdate::new("scene");
        update.active_scene = Some(self.active_scene);
        // Existing blocks are not republished/replaced: consumers use only
        // these model-witnessed bypass fields, and clear missing evidence.
        update.blocks = Some(if self.grid_layout_exact {
            self.preset
                .as_ref()
                .map(|preset| {
                    project_scene_bypasses(preset, &self.scene_bypasses, self.active_scene)
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        });
        update.footswitch_states = self.projected_footswitch_states();
        update.routes = Some(if self.grid_layout_exact {
            self.preset
                .as_ref()
                .map(|preset| self.preset_routes(preset))
                .unwrap_or_default()
        } else {
            Vec::new()
        });
        let mut updates = vec![update];
        updates.extend(invalidated);
        Ok(updates)
    }

    fn decode_position(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message =
            decode::<pa::SetlistPositionMessage>(profile::MESSAGE_TYPE_SETLIST_POSITION, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        if let Some(pa::setlist_position_message::FolderKey::FolderKey(value)) = message.folder_key
        {
            self.setlist_key = Some(value);
        }
        if let Some(pa::setlist_position_message::Position::Position(value)) = message.position {
            self.position = Some(value);
        }
        if let Some(pa::setlist_position_message::IsFactory::IsFactory(value)) = message.is_factory
        {
            self.is_factory = Some(value);
        }
        let mut update = StateUpdate::new("position");
        update.setlist_key = self.setlist_key.clone();
        update.position = self.position;
        update.is_factory = self.is_factory;
        Ok(vec![update])
    }

    fn decode_preset_message(
        &mut self,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message =
            decode::<pa::RecallPresetMessage>(profile::MESSAGE_TYPE_RECALL_PRESET, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        let Some(pa::recall_preset_message::Preset::Preset(preset)) = message.preset else {
            return Ok(Vec::new());
        };
        validate_preset_expression_projection(&preset, profile::MESSAGE_TYPE_RECALL_PRESET)?;
        self.parameter_overrides.clear();
        self.metadata_observations = Default::default();
        self.sparse_state_invalidated = false;
        self.scene_bypasses = witnessed_scene_bypasses(&preset, &preset.bypass);
        self.preset = Some(preset);
        self.preset_exact = true;
        self.grid_layout_exact = true;
        self.state_revision = self.state_revision.saturating_add(1);
        Ok(self.preset_update(false).into_iter().collect())
    }

    /// Record every in-range normalized parameter override carried by
    /// `params`, both into `self.parameter_overrides` (keyed by `row`,
    /// `column`, and the parameter's own index) and as a `kind`-tagged
    /// `StateUpdate` pushed onto `states`. Shared by decode_grid's three
    /// identically-shaped sweeps over a chain's models, lane input/output
    /// controls, and splitter/mixer targets -- which differ only in the
    /// source iterator, the column/discriminator value, and whether the
    /// update kind is "parameter" or "laneControlParameter".
    fn record_parameter_overrides(
        &mut self,
        model: &Model,
        row: u32,
        column: u32,
        kind: &str,
        states: &mut Vec<StateUpdate>,
    ) {
        for (parameter_pos, parameter) in model.params.iter().enumerate() {
            let index = param_index(parameter, parameter_pos as u32);
            let scene_mode = self.sparse_parameter_scene_mode(row, column, index, model, parameter);
            if scene_mode.is_none() && parameter.param_values.len() > 1 {
                continue;
            }
            let selected = if scene_mode == Some(true) && parameter.param_values.len() != 1 {
                self.active_scene as usize
            } else {
                0
            };
            let Some(value) = parameter.param_values.get(selected).and_then(float_value) else {
                continue;
            };
            if !(0.0..=1.0).contains(&value) {
                continue;
            }
            self.parameter_overrides
                .insert((row, column, index), value as f64);
            let mut update = StateUpdate::new(kind);
            update.row = Some(row);
            update.column = Some(column);
            update.parameter_index = Some(index);
            update.normalized_value = Some(value);
            states.push(update);
        }
    }

    fn decode_grid(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message = decode::<pa::GridMessage>(profile::MESSAGE_TYPE_GRID, payload)?;
        if !matches!(
            pa::message_action::Enum::try_from(message.action),
            Ok(pa::message_action::Enum::Update | pa::message_action::Enum::Delete)
        ) {
            return Ok(Vec::new());
        }
        let Some(pa::grid_message::Preset::Preset(preset)) = message.preset else {
            return Ok(Vec::new());
        };
        if message.action == pa::message_action::Enum::Update as i32 {
            validate_preset_expression_projection(&preset, profile::MESSAGE_TYPE_GRID)?;
        }
        if self.sparse_state_invalidated {
            return Err(StateDecodeError::RetentionLimit("sparse parameter records"));
        }
        if message.action == pa::message_action::Enum::Update as i32
            && self.grid_parameter_mode_changes(&preset)
        {
            return Ok(vec![self.withdraw_preset_authority()]);
        }
        if message.action == pa::message_action::Enum::Update as i32
            && !self.sparse_parameter_budget_fits(&preset)
        {
            self.parameter_overrides = HashMap::new();
            self.metadata_observations = Default::default();
            self.scene_bypasses.clear();
            self.preset = None;
            self.preset_exact = false;
            self.grid_layout_exact = false;
            self.sparse_state_invalidated = true;
            self.state_revision = self.state_revision.saturating_add(1);
            return Err(StateDecodeError::RetentionLimit("sparse parameter records"));
        }
        let layout_was_exact = self.preset_exact || self.grid_layout_exact;
        self.preset_exact = false;
        self.state_revision = self.state_revision.saturating_add(1);
        if message.action == pa::message_action::Enum::Delete as i32 {
            // Removals are applied to the retained chains, so a layout that
            // was exact stays exact.
            self.grid_layout_exact = layout_was_exact && self.preset.is_some();
            let mut states = self.apply_grid_removals(&preset);
            let footswitch_states = self.projected_footswitch_states();
            for state in &mut states {
                state.grid_layout_complete = Some(self.grid_layout_exact);
                state.footswitch_states.clone_from(&footswitch_states);
            }
            return Ok(states);
        }
        // An Update is not merged into the retained chains. The layout stays
        // proven only when the push names no block identity that the retained
        // chains do not already hold at the same coordinates (parameter,
        // bypass, tempo and routing pushes, such as the one that follows a
        // recall).
        self.grid_layout_exact = layout_was_exact
            && self
                .preset
                .as_ref()
                .is_some_and(|retained| grid_update_preserves_layout(retained, &preset));
        let bypass_projection = if self.grid_layout_exact {
            if let Some(retained) = &self.preset {
                let incoming = witnessed_scene_bypasses(retained, &preset.bypass);
                let projection = project_scene_bypasses(retained, &incoming, self.active_scene);
                self.scene_bypasses.extend(incoming);
                projection
            } else {
                Vec::new()
            }
        } else {
            // Topology no longer proves that any previous address still names
            // its old model. Publish unknown, never a new raw bool for it.
            let mut projection = self
                .preset
                .as_ref()
                .map(|retained| {
                    project_scene_bypasses(retained, &self.scene_bypasses, self.active_scene)
                })
                .unwrap_or_default();
            for block in &mut projection {
                block.bypassed = None;
            }
            self.scene_bypasses.clear();
            projection
        };
        let mut states = Vec::new();
        if layout_was_exact && !self.grid_layout_exact {
            let mut update = StateUpdate::new("gridTopology");
            update.grid_layout_complete = Some(false);
            states.push(update);
        }
        if let Some(delta) = preset.tempo_program_data.first() {
            self.apply_grid_tempo_update(delta);
        }
        let tempo = tempo_state(preset.tempo_program_data.first());
        if tempo.led_enabled.is_some() {
            self.metadata_observations.tempo_led = tempo.led_enabled;
        }
        if tempo.bpm.is_some() || tempo.led_enabled.is_some() {
            let mut update = StateUpdate::new("tempo");
            update.tempo = tempo.bpm;
            update.tempo_led_enabled = tempo.led_enabled;
            states.push(update);
        }
        let bypasses = bypass_updates(&preset.bypass, self.active_scene)
            .into_iter()
            .filter(|update| {
                bypass_projection.iter().any(|block| {
                    block.row == update.row
                        && block.column == update.column
                        && block.bypassed == Some(update.bypassed)
                })
            })
            .collect::<Vec<_>>();
        if !bypass_projection.is_empty() {
            let mut update = StateUpdate::new("bypassBatch");
            update.bypass_updates = Some(bypasses);
            // Unlike bool-only updates, this explicitly carries unknown for
            // malformed active values, duplicate addresses or lost topology.
            update.blocks = Some(bypass_projection);
            update.footswitch_states = self.projected_footswitch_states();
            states.push(update);
        }
        for (chain_index, chain) in preset.chains.iter().enumerate() {
            let row = chain_row(chain, chain_index as u32);
            if row > 3 {
                continue;
            }
            for (model_index, model) in chain.models.iter().enumerate() {
                let column = model_column(model, model_index as u32);
                if column > 7 {
                    continue;
                }
                self.record_parameter_overrides(model, row, column, "parameter", &mut states);
            }
            for (discriminator, controls) in [
                (10_u32, chain.input_control.as_slice()),
                (11_u32, chain.output_control.as_slice()),
            ] {
                for control in controls {
                    self.record_parameter_overrides(
                        control,
                        row,
                        discriminator,
                        "laneControlParameter",
                        &mut states,
                    );
                }
            }
            let splitter_updates = match (
                chain.splitter.is_empty(),
                chain.combined_splitter.is_empty(),
            ) {
                (false, true) => chain.splitter.as_slice(),
                (true, false) => chain.combined_splitter.as_slice(),
                _ => &[],
            };
            for (column, targets) in [(8_u32, splitter_updates), (9_u32, chain.mixer.as_slice())] {
                for target in targets {
                    self.record_parameter_overrides(target, row, column, "parameter", &mut states);
                }
            }
        }
        if self.apply_grid_route_updates(&preset) {
            states.extend(self.preset_update(false));
        }
        Ok(states)
    }

    fn retained_parameter_model(&self, row: u32, column: u32) -> Option<&Model> {
        let preset = self.preset.as_ref()?;
        if column < domain::GRID_COLUMNS {
            return locate_model(preset, row, column)
                .ok()
                .map(|(_, _, model)| model);
        }
        let (_, chain) = locate_chain(preset, row).ok()?;
        match column {
            8 => splitter_target(chain).map(|(model, _, _)| model),
            9 => chain.mixer.first(),
            10 => chain.input_control.first(),
            11 => chain.output_control.first(),
            _ => None,
        }
    }

    fn sparse_parameter_scene_mode(
        &self,
        row: u32,
        column: u32,
        index: u32,
        incoming: &Model,
        parameter: &Param,
    ) -> Option<bool> {
        let mode = parameter.scene_mode.as_ref().or_else(|| {
            if !self.grid_layout_exact {
                return None;
            }
            self.retained_parameter_model(row, column)
                .filter(|held| {
                    model_hash(incoming).is_none() || model_hash(incoming) == model_hash(held)
                })
                .and_then(|model| {
                    model
                        .params
                        .iter()
                        .enumerate()
                        .find(|(position, held)| param_index(held, *position as u32) == index)
                        .and_then(|(_, held)| held.scene_mode.as_ref())
                })
        });
        mode.map(|mode| matches!(mode, param::SceneMode::SceneMode(true)))
    }

    fn grid_parameter_mode_changes(&self, delta: &BinaryPreset) -> bool {
        for (position, chain) in delta.chains.iter().enumerate() {
            let row = chain_row(chain, position as u32);
            let models = chain
                .models
                .iter()
                .enumerate()
                .map(|(position, model)| (model_column(model, position as u32), model));
            let controls = [
                (8, chain.splitter.as_slice()),
                (8, chain.combined_splitter.as_slice()),
                (9, chain.mixer.as_slice()),
                (10, chain.input_control.as_slice()),
                (11, chain.output_control.as_slice()),
            ]
            .into_iter()
            .flat_map(|(column, models)| models.iter().map(move |model| (column, model)));
            for (column, model) in models.chain(controls) {
                for (position, parameter) in model.params.iter().enumerate() {
                    let index = param_index(parameter, position as u32);
                    if parameter.param_values.len() > 1 {
                        match self.sparse_parameter_scene_mode(row, column, index, model, parameter)
                        {
                            Some(true)
                                if parameter
                                    .param_values
                                    .get(self.active_scene as usize)
                                    .is_none() =>
                            {
                                return true
                            }
                            None => return true,
                            _ => {}
                        }
                        if self.sparse_parameter_scene_mode(row, column, index, model, parameter) == Some(true)
                            && parameter.param_values.get(self.active_scene as usize).is_some_and(|value| {
                                value.value.is_none() || matches!(value.value, Some(param_value::Value::FloatValue(value)) if !(0.0..=1.0).contains(&value))
                            }) { return true; }
                    }
                    if parameter.scene_mode.is_none() {
                        continue;
                    }
                    let held = self
                        .retained_parameter_model(row, column)
                        .filter(|held| {
                            model_hash(model).is_none() || model_hash(model) == model_hash(held)
                        })
                        .and_then(|held| {
                            held.params
                                .iter()
                                .enumerate()
                                .find(|(position, parameter)| {
                                    param_index(parameter, *position as u32) == index
                                })
                        })
                        .map(|(_, parameter)| parameter);
                    if held.is_none_or(|held| held.scene_mode != parameter.scene_mode) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn sparse_parameter_budget_fits(&self, delta: &BinaryPreset) -> bool {
        let tempo = self
            .preset
            .as_ref()
            .and_then(|preset| preset.tempo_program_data.first());
        let mut tempo_count = tempo.map_or(0, |model| model.params.len());
        if self.parameter_overrides.len().saturating_add(tempo_count)
            > RETAINED_SPARSE_PARAMETER_LIMIT
        {
            return false;
        }
        let mut keys: HashSet<_> = self.parameter_overrides.keys().copied().collect();
        let mut tempo_keys: HashSet<_> = tempo
            .into_iter()
            .flat_map(|model| model.params.iter().enumerate())
            .map(|(position, parameter)| param_index(parameter, position as u32))
            .collect();
        let numeric = |parameter: &Param| {
            parameter
                .param_values
                .iter()
                .filter_map(float_value)
                .any(|value| (0.0..=1.0).contains(&value))
        };
        if let Some(model) = delta.tempo_program_data.first() {
            for (position, parameter) in model
                .params
                .iter()
                .enumerate()
                .filter(|(_, parameter)| numeric(parameter))
            {
                if tempo_keys.insert(param_index(parameter, position as u32)) {
                    tempo_count = tempo_count.saturating_add(1);
                }
                if keys.len().saturating_add(tempo_count) > RETAINED_SPARSE_PARAMETER_LIMIT {
                    return false;
                }
            }
        }
        for (position, chain) in delta.chains.iter().enumerate() {
            let row = chain_row(chain, position as u32);
            if row >= domain::GRID_ROWS {
                continue;
            }
            let models = chain
                .models
                .iter()
                .enumerate()
                .filter_map(|(position, model)| {
                    let column = model_column(model, position as u32);
                    (column < domain::GRID_COLUMNS).then_some((column, model))
                });
            let splitter = match (
                chain.splitter.is_empty(),
                chain.combined_splitter.is_empty(),
            ) {
                (false, true) => chain.splitter.as_slice(),
                (true, false) => chain.combined_splitter.as_slice(),
                _ => &[],
            };
            let controls = [
                (10, chain.input_control.as_slice()),
                (11, chain.output_control.as_slice()),
                (8, splitter),
                (9, chain.mixer.as_slice()),
            ]
            .into_iter()
            .flat_map(|(column, models)| models.iter().map(move |model| (column, model)));
            for (column, model) in models.chain(controls) {
                for (position, parameter) in model
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, parameter)| numeric(parameter))
                {
                    keys.insert((row, column, param_index(parameter, position as u32)));
                    if keys.len().saturating_add(tempo_count) > RETAINED_SPARSE_PARAMETER_LIMIT {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Merge a sparse Grid UPDATE's tempo block into the retained preset.
    /// `set_tempo` writes the loaded preset's tempo through exactly this
    /// frame (parameter index 0), so a grid tempo is the preset tempo, not a
    /// snapshot-only value. Without the merge the snapshot moved while the
    /// retained preset kept the recalled tempo, and the next re-projection
    /// of the retained preset (a grid route change, a block move, a catalog
    /// install) put that stale tempo back. Parameters are matched by index;
    /// only in-range values the delta actually carries are applied.
    fn apply_grid_tempo_update(&mut self, delta: &Model) {
        let Some(retained) = self.preset.as_mut() else {
            return;
        };
        for (delta_pos, parameter) in delta.params.iter().enumerate() {
            let index = param_index(parameter, delta_pos as u32);
            let Some(value) = parameter
                .param_values
                .first()
                .filter(|value| float_value(value).is_some_and(|v| (0.0..=1.0).contains(&v)))
            else {
                continue;
            };
            if retained.tempo_program_data.is_empty() {
                retained.tempo_program_data.push(Model::default());
            }
            let block = &mut retained.tempo_program_data[0];
            let existing = block
                .params
                .iter()
                .enumerate()
                .position(|(pos, item)| param_index(item, pos as u32) == index);
            match existing {
                Some(pos) => match block.params[pos].param_values.first_mut() {
                    Some(first) => *first = value.clone(),
                    None => block.params[pos].param_values.push(value.clone()),
                },
                None => block.params.push(Param {
                    index: Some(param::Index::Index(index)),
                    param_values: vec![value.clone()],
                    ..Default::default()
                }),
            }
        }
    }

    /// Merge a sparse Grid UPDATE's chain routing (`in_portid`/`out_portid`)
    /// into the retained preset. A route chosen on the unit's own route picker
    /// arrives this way, and until it was merged the retained preset kept the
    /// old route (live test, app 0.1.174: the output chip stayed on USB 3 for
    /// minutes after the route was changed back on the device, and only a
    /// full recall such as a scene change refreshed it). The chain is matched
    /// by its explicit `row`; array position means nothing in a sparse
    /// message. Returns true when a retained route actually changed.
    fn apply_grid_route_updates(&mut self, delta: &BinaryPreset) -> bool {
        let Some(retained) = self.preset.as_mut() else {
            return false;
        };
        let mut changed = false;
        for chain in &delta.chains {
            let Some(chain::Row::Row(row)) = chain.row else {
                continue;
            };
            if chain.in_portid.is_none() && chain.out_portid.is_none() {
                continue;
            }
            let Ok((index, _)) = locate_chain(retained, row) else {
                continue;
            };
            let target = &mut retained.chains[index];
            if let Some(input) = chain.in_portid {
                if target.in_portid != Some(input) {
                    target.in_portid = Some(input);
                    changed = true;
                }
            }
            if let Some(output) = chain.out_portid {
                if target.out_portid != Some(output) {
                    target.out_portid = Some(output);
                    changed = true;
                }
            }
        }
        changed
    }

    fn apply_grid_removals(&mut self, delta: &BinaryPreset) -> Vec<StateUpdate> {
        let Some(retained) = self.preset.as_mut() else {
            return Vec::new();
        };
        // DELETE requires presence of Chain.row and Model.column.
        // Array positions are not sparse selectors.
        let mut targets = HashSet::new();
        for chain in &delta.chains {
            let Some(chain::Row::Row(row)) = chain.row else {
                continue;
            };
            if row >= domain::GRID_ROWS {
                continue;
            }
            for model in &chain.models {
                let Some(model::Column::Column(column)) = model.column else {
                    continue;
                };
                if column < domain::GRID_COLUMNS {
                    targets.insert((row, column));
                }
            }
        }
        let mut targets: Vec<_> = targets.into_iter().collect();
        targets.sort_unstable();
        for (chain_index, chain) in retained.chains.iter_mut().enumerate() {
            let row = chain_row(chain, chain_index as u32);
            // A full preset may omit coordinates. Preserve those coordinates
            // before removing a vector element, so later slots cannot shift.
            for (index, model) in chain.models.iter_mut().enumerate() {
                model.column = Some(model::Column::Column(model_column(model, index as u32)));
            }
            chain
                .models
                .retain(|model| !targets.contains(&(row, model_column(model, 0))));
        }
        self.parameter_overrides
            .retain(|&(row, column, _), _| !targets.contains(&(row, column)));
        self.scene_bypasses
            .retain(|address, _| !targets.contains(address));
        targets
            .into_iter()
            .map(|(row, column)| {
                let mut state = StateUpdate::new("blockRemoved");
                state.row = Some(row);
                state.column = Some(column);
                state
            })
            .collect()
    }

    fn decode_mode(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message = decode::<pa::ModeMessage>(profile::MESSAGE_TYPE_MODE, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        let Some(pa::mode_message::Mode::Mode(value)) = message.mode else {
            return Ok(Vec::new());
        };
        let mut update = StateUpdate::new("mode");
        update.mode = Some(mode_kind(value));
        update.raw_mode = Some(value);
        // The two base modes are named only for a value the library can split; for any other
        // value the pair stays absent rather than being filled with a guess.
        update.footswitch_modes = mode_pair(value).map(|pair| pair.map(mode_name).to_vec());
        if let Some(pa::mode_message::AvailableModes::AvailableModes(available)) =
            message.available_modes
        {
            let slots = available
                .modes
                .into_iter()
                .take(3)
                .enumerate()
                .map(|(slot, mode)| ModeSlot {
                    slot: slot as u32,
                    label: mode_label(mode),
                    mode: mode_kind(mode),
                    raw_mode: Some(mode),
                })
                .collect::<Vec<_>>();
            if !slots.is_empty() {
                update.mode_slots = Some(slots);
            }
        }
        Ok(vec![update])
    }

    fn decode_master_volume(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message =
            decode::<pa::MasterVolumeMessage>(profile::MESSAGE_TYPE_MASTER_VOLUME, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        let Some(pa::master_volume_message::Volume::Volume(value)) = message.volume else {
            return Ok(Vec::new());
        };
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(StateDecodeError::InvalidNormalizedValue {
                message_type: profile::MESSAGE_TYPE_MASTER_VOLUME,
                field: "volume",
                value,
            });
        }
        let mut update = StateUpdate::new("master");
        update.master_volume = Some(value);
        Ok(vec![update])
    }

    fn decode_dirty(&self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message =
            decode::<pa::PresetDirtyMessage>(profile::MESSAGE_TYPE_PRESET_DIRTY, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        let mut update = StateUpdate::new("dirty");
        update.dirty = Some(message.is_dirty);
        Ok(vec![update])
    }

    fn decode_scene_label(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message = decode::<pa::SceneLabelMessage>(profile::MESSAGE_TYPE_SCENE_LABEL, payload)?;
        if !is_update_action(message.action) || message.request_id.is_some() {
            return Ok(Vec::new());
        }
        if !(0..domain::SCENE_COUNT as i32).contains(&message.index) {
            return Ok(Vec::new());
        }
        let mut update = StateUpdate::new("sceneLabel");
        self.metadata_observations.labels[message.index as usize] = Some(message.label.clone());
        self.preset_exact = false;
        self.state_revision = self.state_revision.saturating_add(1);
        update.index = Some(message.index as u32);
        update.label = Some(message.label);
        Ok(vec![update])
    }

    fn decode_scene_color(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message = decode::<pa::SceneColorMessage>(profile::MESSAGE_TYPE_SCENE_COLOR, payload)?;
        if !is_update_action(message.action) || message.request_id.is_some() {
            return Ok(Vec::new());
        }
        if message.index < 0 || message.index as u32 >= domain::SCENE_COUNT {
            return Ok(Vec::new());
        }
        let mut update = StateUpdate::new("sceneColor");
        self.metadata_observations.colors[message.index as usize] = Some(message.color);
        self.preset_exact = false;
        self.state_revision = self.state_revision.saturating_add(1);
        update.index = Some(message.index as u32);
        update.color = Some(color_hex(message.color));
        update.raw_color = Some(message.color);
        Ok(vec![update])
    }

    fn decode_global_tempo(
        &mut self,
        payload: &[u8],
    ) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let message =
            decode::<pa::GlobalTempoMessage>(profile::MESSAGE_TYPE_GLOBAL_TEMPO, payload)?;
        if !is_update_action(message.action) {
            return Ok(Vec::new());
        }
        if message.request_id.is_some() {
            return Ok(Vec::new());
        }
        // GlobalTempo parameter 0 is the device-global BPM (the value
        // `set_global_tempo` writes and the GLOBAL-mode tempo), never the
        // loaded preset's. The snapshot's `tempo` is the preset tempo, so it
        // is left to the preset reads; the global BPM is retained separately
        // by the state runtime and reported as `globalBpm`. The metronome
        // controls this message also carries (parameter 2, the LED) belong
        // to the loaded preset's tempo block and still apply.
        let state = tempo_from_params(&message.params, AbsentIndex::Zero);
        if state.led_enabled.is_none() {
            return Ok(Vec::new());
        }
        let mut update = StateUpdate::new("tempo");
        self.metadata_observations.tempo_led = state.led_enabled;
        self.preset_exact = false;
        self.state_revision = self.state_revision.saturating_add(1);
        update.tempo_led_enabled = state.led_enabled;
        Ok(vec![update])
    }

    fn decode_model_repo(&mut self, payload: &[u8]) -> Result<Vec<StateUpdate>, StateDecodeError> {
        let catalog = parse_model_repo(payload)?;
        Ok(self.install_catalog(catalog))
    }

    fn preset_blocks(&self, preset: &BinaryPreset) -> Vec<GridBlock> {
        let bypasses = bypass_records(&preset.bypass);
        let assignments = preset
            .stomp_mode_assignments
            .iter()
            .enumerate()
            .filter(|(_, item)| item.stomp_index <= 7)
            .map(|(order, item)| {
                (
                    (item.row, item.column),
                    (item.stomp_index, order as u32, stomp_assignment_type(item)),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut blocks = Vec::new();
        for (chain_index, chain) in preset.chains.iter().enumerate() {
            let row = chain_row(chain, chain_index as u32);
            for (model_index, model) in chain.models.iter().enumerate() {
                let Some(model_id) = model_hash(model).filter(|value| *value != 0) else {
                    continue;
                };
                let column = model_column(model, model_index as u32);
                let info = self.catalog.get(&model_id);
                blocks.push(GridBlock {
                    id: format!("{row}:{column}"),
                    model_id: Some(model_id),
                    category_id: None,
                    name: info
                        .map(|item| item.name.clone())
                        .unwrap_or_else(|| format!("Model {model_id}")),
                    kind: info
                        .map(|item| item.kind.clone())
                        .unwrap_or_else(|| "utility".into()),
                    // list_models and block_details both publish the curated
                    // display vocabulary. Publishing the raw ModelRepo name here
                    // gave the same block two categories across endpoints
                    // ("Guitar Amplifier" vs "Guitar Amp"), so anything
                    // filtering by category across endpoints silently missed.
                    category: info.map(|item| display_category(&item.category)),
                    plugin: info.map(|item| item.plugin_id.is_some()),
                    plugin_id: info.and_then(|item| item.plugin_id.map(|id| id.to_string())),
                    row,
                    column,
                    bypassed: self
                        .scene_bypasses
                        .get(&(row, column))
                        .filter(|(witness, _)| *witness == model_id)
                        .and_then(|(_, record)| bypass_value_for_scene(record, self.active_scene)),
                    bypass_scene_mode: bypasses
                        .get(&(row, column))
                        .and_then(|record| record.scene_mode),
                    bypass_address: bypasses
                        .get(&(row, column))
                        .map(|record| record.address.as_str().to_string()),
                    catalog_category: info.map(|item| item.category.clone()),
                    bypass_expression: model_bypass_expression(model),
                    color: None,
                    glyph: None,
                    footswitch: assignments.get(&(row, column)).map(|value| value.0),
                    footswitch_order: assignments.get(&(row, column)).map(|value| value.1),
                    footswitch_assignment_type: assignments
                        .get(&(row, column))
                        .and_then(|value| value.2.clone()),
                });
            }
        }
        blocks
    }

    fn projected_footswitch_states(&self) -> Option<Vec<FootswitchState>> {
        let preset = self.preset.as_ref()?;
        Some(project_footswitch_states(
            preset,
            &self.preset_blocks(preset),
        ))
    }
    fn preset_routes(&self, preset: &BinaryPreset) -> Vec<GridRoute> {
        let mut routes = Vec::new();
        for (chain_index, chain) in preset.chains.iter().enumerate() {
            let row = chain_row(chain, chain_index as u32);
            let input_id = chain_input(chain);
            let output_id = chain_output(chain);
            routes.push(GridRoute {
                row,
                input_id,
                output_id,
                input: input_id.map(input_label).unwrap_or_default(),
                output: output_id.map(output_label).unwrap_or_default(),
                input_wire: input_id.and_then(input_wire_name).map(str::to_string),
                output_wire: output_id.and_then(output_wire_name).map(str::to_string),
                split_column: chain
                    .split_control_points
                    .first()
                    .and_then(|value| (value.split >= 0).then_some(value.split)),
                mix_column: chain
                    .split_control_points
                    .first()
                    .and_then(|value| (value.mix >= 0).then_some(value.mix)),
                splitter_model_id: splitter_target(chain).map(|(_, model_id, _)| model_id),
                split_muted: chain
                    .mix_bypass
                    .get(self.active_scene as usize)
                    .map(|value| value.bypass),
            });
        }
        routes
    }

    fn preset_update(&self, catalog_refresh: bool) -> Option<StateUpdate> {
        let preset = self.preset.as_ref()?;
        let mut update = StateUpdate::new("preset");
        update.grid_layout_complete = Some(self.grid_layout_exact);
        update.preset_name = match &preset.name {
            Some(binary_preset::Name::Name(value)) => Some(value.clone()),
            _ => None,
        };
        // The tempo block's first parameter, a normalised value, is where every preset read from a
        // unit carried its tempo; the plain `tempo` field of the preset has not been seen set. The
        // block is preferred and the plain field is the fallback.
        let tempo = tempo_state(preset.tempo_program_data.first());
        // The normalized tempo program value is preferred. The plain `tempo`
        // field is only a fallback, and reading it as BPM is an assumption: no
        // observation ties that integer to a tempo shown on the unit.
        update.tempo = tempo.bpm.or(match preset.tempo {
            Some(binary_preset::Tempo::Tempo(value)) => Some(value),
            _ => None,
        });
        update.tempo_led_enabled = tempo.led_enabled;
        // `scenes` is for display: a label that is empty or blank is replaced by `Scene A` to
        // `Scene H`, which is this library's text, not something the device sent.
        // `sceneLabelsRaw` is the same eight labels as stored. The snapshot applies this same
        // rule to a `sceneLabel` push, so the two paths cannot disagree.
        update.scenes = Some(
            (0..8)
                .map(|index| scene_display_label(index, preset.scene_labels.get(index)))
                .collect(),
        );
        update.scene_labels_raw = Some(
            (0..8)
                .map(|index| preset.scene_labels.get(index).cloned().unwrap_or_default())
                .collect(),
        );
        update.scene_colors = Some(
            preset
                .scene_colors
                .iter()
                .map(|value| color_hex(*value))
                .collect(),
        );
        update.scene_colors_argb = Some(preset.scene_colors.clone());
        let blocks = self.preset_blocks(preset);
        let routes = self.preset_routes(preset);
        update.footswitch_states = Some(project_footswitch_states(preset, &blocks));
        // Layout of a stored preset (observed on a CorOS 4.1.0 unit, 2026-10-07,
        // live read and a save-then-recall of a scratch slot): the general list
        // has 120 slots and source N occupies slots N * 12 to N * 12 + 11, so
        // chunk N of twelve is source N. The preset-load list is the separate
        // twelve-slot `midi_messages`, with positions preserved.
        update.midi_out = Some(
            preset
                .midi_messages_general_v2
                .chunks(12)
                .take(10)
                .enumerate()
                .filter_map(|(source, messages)| {
                    let messages = midi_slot_list(messages.iter());
                    (!messages.is_empty()).then_some(MidiOutSource {
                        source: source as u32,
                        messages,
                        // Where this source's group of twelve began in the flat list
                        // of 120, so a caller can name the row a message came from.
                        preset_flat_offset: Some((source * 12) as u32),
                    })
                })
                .collect(),
        );
        update.preset_load_midi_out = Some(midi_slot_list(preset.midi_messages.iter()));
        for index in 0..8 {
            if let Some(label) = &self.metadata_observations.labels[index] {
                update.scenes.as_mut().unwrap()[index] = scene_display_label(index, Some(label));
                update.scene_labels_raw.as_mut().unwrap()[index] = label.clone();
            }
            if let Some(color) = self.metadata_observations.colors[index] {
                let colors = update.scene_colors.as_mut().unwrap();
                colors.resize(8, String::new());
                colors[index] = color_hex(color);
                // Do not pad a short raw list with invented numeric colors.
                if let Some(raw) = update
                    .scene_colors_argb
                    .as_mut()
                    .and_then(|colors| colors.get_mut(index))
                {
                    *raw = color;
                }
            }
        }
        let sources: Vec<_> = self
            .metadata_observations
            .midi_sources
            .iter()
            .map(|(source, messages)| MidiOutSource {
                source: *source,
                messages: messages.clone(),
                preset_flat_offset: None,
            })
            .collect();
        merge_midi_out_sources(update.midi_out.as_mut().unwrap(), &sources);
        if let Some(messages) = &self.metadata_observations.preset_load {
            update.preset_load_midi_out = Some(messages.clone());
        }
        if let Some(led) = self.metadata_observations.tempo_led {
            update.tempo_led_enabled = Some(led);
        }
        update.blocks = Some(blocks);
        update.routes = Some(routes);
        if catalog_refresh {
            update.catalog_refresh = Some(true);
        }
        Some(update)
    }
}

pub(crate) fn merge_midi_out_sources(retained: &mut Vec<MidiOutSource>, updates: &[MidiOutSource]) {
    for update in updates {
        retained.retain(|source| source.source != update.source);
        if !update.messages.is_empty() {
            retained.push(update.clone());
        }
    }
    retained.sort_by_key(|source| source.source);
}

fn project_footswitch_states(preset: &BinaryPreset, blocks: &[GridBlock]) -> Vec<FootswitchState> {
    (0..domain::SCENE_COUNT)
        .map(|index| {
            let mut targets = blocks
                .iter()
                .filter(|block| block.footswitch == Some(index))
                .collect::<Vec<_>>();
            targets.sort_by_key(|block| block.footswitch_order.unwrap_or(u32::MAX));
            FootswitchState {
                index,
                active: match targets.first() {
                    Some(block) => block.bypassed.map(|bypassed| !bypassed),
                    None => Some(false),
                },
                assigned: !targets.is_empty(),
                color: stomp_color(&targets),
                momentary: Some(
                    preset
                        .stomp_is_momentary
                        .get(&index)
                        .copied()
                        .unwrap_or(false),
                ),
                label: Some(
                    preset
                        .single_stomp_labels
                        .get(&index)
                        .or_else(|| preset.stomp_labels.get(&index))
                        .cloned()
                        .unwrap_or_default(),
                ),
            }
        })
        .collect()
}

/// What an empty MIDI Out slot (wire type `0`) is published and compared as:
/// type `0`, channel `1` and zero parameters, so a published list can be sent
/// back unchanged and two lists of the same slots compare equal.
pub(crate) fn empty_midi_slot() -> MidiOutMessage {
    MidiOutMessage {
        r#type: 0,
        channel: 1,
        param1: 0,
        param2: 0,
        param3: 0,
    }
}

/// The canonical form of a list of MIDI Out slots, in the order given: every
/// type `0` slot becomes [`empty_midi_slot`] and trailing empty slots are
/// dropped, because the unit holds twelve slots and an absent trailing slot
/// is an empty one. A slot's position is its index, so an empty slot between
/// two active ones stays (observed on a CorOS 4.1.0 unit, 2026-10-07).
pub(crate) fn canonical_midi_slots(messages: &[MidiOutMessage]) -> Vec<MidiOutMessage> {
    let mut slots = messages
        .iter()
        .map(|message| {
            if message.r#type == 0 {
                empty_midi_slot()
            } else {
                message.clone()
            }
        })
        .collect::<Vec<_>>();
    while slots.last().is_some_and(|message| message.r#type == 0) {
        slots.pop();
    }
    slots
}

/// A device's slots as a published list: one entry per slot in the order the
/// unit holds them, an empty or undecodable slot as [`empty_midi_slot`], and
/// no trailing empties (see [`canonical_midi_slots`]).
fn midi_slot_list<'a>(
    slots: impl Iterator<Item = &'a crate::proto::MidiMessageInfo>,
) -> Vec<MidiOutMessage> {
    let slots = slots
        .map(|slot| midi_message(slot).unwrap_or_else(empty_midi_slot))
        .collect::<Vec<_>>();
    canonical_midi_slots(&slots)
}

fn midi_message(message: &crate::proto::MidiMessageInfo) -> Option<MidiOutMessage> {
    crate::commands::midi_out_fields_are_valid(
        message.r#type,
        message.channel,
        [message.param1, message.param2, message.param3],
    )
    .then_some(MidiOutMessage {
        r#type: message.r#type,
        channel: message.channel,
        param1: message.param1,
        param2: message.param2,
        param3: message.param3,
    })
}

fn model_bypass_expression(model: &Model) -> Option<BypassExpression> {
    let assignment = model.bypass_expression.first()?;
    if assignment.expression <= 0 {
        return None;
    }
    let info = model
        .expression_bypass_info
        .first()
        .cloned()
        .unwrap_or_default();
    Some(BypassExpression {
        pedal: assignment.expression as u32,
        minimum: assignment.expression_min,
        maximum: assignment.expression_max,
        mode: info.r#type,
        invert: info.invert,
        delay_ms: info.delay_ms,
        latch_emulation: info.latch_emulation,
    })
}

fn stomp_assignment_type(assignment: &crate::proto::StompModeAssignment) -> Option<String> {
    let crate::proto::stomp_mode_assignment::TypeOneOf::Type(value) = assignment.r#type.as_ref()?;
    match crate::proto::stomp_mode_assignment::Type::try_from(*value).ok()? {
        crate::proto::stomp_mode_assignment::Type::Primary => Some("primary".into()),
        crate::proto::stomp_mode_assignment::Type::Secondary => Some("secondary".into()),
    }
}

/// The physical STOMP lamp colour for a footswitch assignment.
///
/// Which category a block is, is decided by `domain::category_key`, generated
/// from `contracts/public/qc-domain.v1.json`; the TypeScript and Python surfaces are
/// emitted from the same rules, so this runtime cannot drift away from them.
/// Drive categories used to be substituted here to the pitch yellow; they take
/// the overdrive orange now, which is what the owner sees on a lit lamp and
/// what the unit's own screen paints their tiles and browser row with.
///
/// Every category whose 24-bit colour is the utility grey - Neural Capture,
/// Wah, FX Loop, Utility itself, and anything unclassified - lights the white
/// lamp instead; the unit has no grey emitter.
fn stomp_color(blocks: &[&GridBlock]) -> String {
    if blocks.len() != 1 {
        return if blocks.is_empty() {
            domain::visual_colors::IDLE_LED
        } else {
            domain::visual_colors::WHITE_LED
        }
        .into();
    }
    let block = blocks[0];
    let color = if block.plugin == Some(true) {
        domain::visual_colors::PLUGIN
    } else {
        let category = format!(
            "{} {}",
            block.category.as_deref().unwrap_or_default(),
            block.kind
        );
        domain::category_color(&category, &block.name)
    };
    if color == domain::visual_colors::UTILITY {
        domain::visual_colors::WHITE_LED.into()
    } else {
        color.into()
    }
}

fn routing_parameter_options(node: Option<&str>, index: u32) -> Vec<String> {
    let values: &[&str] = match (node, index) {
        (Some("splitter"), 0) => &["A/B", "BALANCE", "CROSSOVER"],
        (Some("splitter"), 1) => &["MONO", "STEREO"],
        (Some("splitter"), 6) => &["LOW / HIGH", "HIGH / LOW"],
        (Some("mixer"), 4) => &["NORMAL", "INVERTED"],
        _ => &[],
    };
    values.iter().map(|value| (*value).to_string()).collect()
}

/// Parses a ModelRepo message into a [`ModelCatalog`].
///
/// Parsing a large catalog is expensive, so hosts may call this off the receive path and install
/// the result with [`StateDecoder::install_catalog`]. A message without a repository payload yields
/// an empty catalog.
///
/// # Errors
///
/// Returns a [`StateDecodeError`] when the payload exceeds the ModelRepo size limit, cannot be inflated
/// or parsed, is not an update message, or holds an invalid archive or catalog XML.
pub fn parse_model_repo(payload: &[u8]) -> Result<ModelCatalog, StateDecodeError> {
    validate_wire_payload(profile::MESSAGE_TYPE_MODEL_REPO, payload)?;
    let payload = maybe_gunzip(payload)?;
    let message = decode::<pa::ModelRepoMessage>(profile::MESSAGE_TYPE_MODEL_REPO, &payload)?;
    if !is_update_action(message.action) {
        return Err(StateDecodeError::UnexpectedAction {
            message_type: profile::MESSAGE_TYPE_MODEL_REPO,
            action: message.action,
        });
    }
    let Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(repo)) =
        message.model_repo_payload
    else {
        return Ok(ModelCatalog(HashMap::new()));
    };
    let repo = maybe_gunzip(&repo)?;
    Ok(ModelCatalog(parse_catalog(extract_xml(&repo)?)?))
}

fn require_finite_projection(
    message_type: u16,
    field: &'static str,
    value: f32,
) -> Result<(), StateDecodeError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(StateDecodeError::NonFiniteProjection {
            message_type,
            field,
        })
    }
}

fn validate_preset_expression_projection(
    preset: &BinaryPreset,
    message_type: u16,
) -> Result<(), StateDecodeError> {
    let models = preset
        .chains
        .iter()
        .flat_map(|chain| {
            chain
                .models
                .iter()
                .chain(&chain.splitter)
                .chain(&chain.combined_splitter)
                .chain(&chain.mixer)
                .chain(&chain.input_control)
                .chain(&chain.output_control)
        })
        .chain(&preset.tempo_program_data);
    for model in models {
        for expression in &model.bypass_expression {
            require_finite_projection(
                message_type,
                "bypass expression minimum",
                expression.expression_min,
            )?;
            require_finite_projection(
                message_type,
                "bypass expression maximum",
                expression.expression_max,
            )?;
        }
        for parameter in &model.params {
            if let Some(param::ExpressionMin::ExpressionMin(value)) = parameter.expression_min {
                require_finite_projection(message_type, "parameter expression minimum", value)?;
            }
            if let Some(param::ExpressionMax::ExpressionMax(value)) = parameter.expression_max {
                require_finite_projection(message_type, "parameter expression maximum", value)?;
            }
        }
    }
    Ok(())
}

fn decode<M: Message + Default>(message_type: u16, payload: &[u8]) -> Result<M, StateDecodeError> {
    M::decode(payload).map_err(|source| StateDecodeError::Protobuf {
        message_type,
        source,
    })
}

/// Bound a payload the transport delivered. It is already inflated when its
/// envelope was compressed, so the bound is the type's inflation limit (or the
/// raw frame ceiling, whichever is larger), never the raw ceiling alone.
fn validate_wire_payload(message_type: u16, payload: &[u8]) -> Result<(), StateDecodeError> {
    if payload.len() > crate::compression::max_delivered_payload_bytes_for(message_type) {
        Err(StateDecodeError::PayloadLimit)
    } else {
        Ok(())
    }
}

/// A File UPDATE listing of at least `min_bytes`: one Impulse Response library
/// folder (file type 1) of ordinary records, several times the largest
/// captured listing. Shared by the tests that drive an inflated listing past the raw
/// frame ceiling through each consumer.
#[cfg(test)]
pub(crate) fn large_file_listing(min_bytes: usize) -> Vec<u8> {
    let folder_key = "/media/p4/IRs/Big Library";
    let mut folder = pa::FolderInfo {
        key: Some(pa::folder_info::Key::Key(folder_key.into())),
        name: Some(pa::folder_info::Name::Name("Big Library".into())),
        ..Default::default()
    };
    let mut approximate = 0_usize;
    let mut index = 0_i32;
    // The per-record estimate is approximate, so overshoot by a margin.
    while approximate <= min_bytes + 64 * 1024 {
        let name = format!("Impulse response {index:06} with a deliberately long name");
        let record = pa::ProductData {
            key: Some(pa::product_data::Key::Key(format!(
                "{folder_key}/{name}.pb"
            ))),
            index: Some(pa::product_data::Index::Index(index)),
            name: Some(pa::product_data::Name::Name(name)),
            ..Default::default()
        };
        approximate += record.encoded_len() + 4;
        folder.files.push(record);
        index += 1;
    }
    pa::FileMessage {
        action: pa::message_action::Enum::Update as i32,
        r#type: Some(pa::file_message::Type::Type(1)),
        folder: Some(pa::file_message::Folder::Folder(folder)),
        ..Default::default()
    }
    .encode_to_vec()
}

fn model_hash(model: &Model) -> Option<u32> {
    match model.hash {
        Some(model::Hash::Hash(value)) => Some(value),
        _ => None,
    }
}

/// Resolve the mutually exclusive splitter wire containers without inventing
/// precedence when a malformed preset supplies both.
fn splitter_target(chain: &Chain) -> Option<(&Model, u32, &'static str)> {
    match (chain.splitter.first(), chain.combined_splitter.first()) {
        (Some(model), None) => Some((model, model_hash(model)?, "legacySplitter")),
        (None, Some(model)) => Some((model, model_hash(model).unwrap_or(10_004), "splitter")),
        _ => None,
    }
}

/// True when `delta` can be represented without inventing or erasing topology.
/// Every model entry needs an explicit, unique coordinate already occupied in
/// the retained preset. A known nonzero model hash must also match. Hashless
/// entries at occupied cells can be parameter-only deltas; hashless entries at
/// empty cells cannot establish which model was added and invalidate the view.
fn grid_update_preserves_layout(retained: &BinaryPreset, delta: &BinaryPreset) -> bool {
    let mut seen = HashSet::new();
    delta.chains.iter().all(|chain| {
        chain.models.iter().all(|model| {
            let (Some(chain::Row::Row(row)), Some(model::Column::Column(column))) =
                (&chain.row, &model.column)
            else {
                return false;
            };
            if !seen.insert((*row, *column)) {
                return false;
            }
            let Ok((_, _, held)) = locate_model(retained, *row, *column) else {
                return false;
            };
            model_hash(model)
                .filter(|hash| *hash > 0)
                .is_none_or(|hash| model_hash(held) == Some(hash))
        })
    })
}

fn model_column(model: &Model, fallback: u32) -> u32 {
    match model.column {
        Some(model::Column::Column(value)) => value,
        _ => fallback,
    }
}
fn chain_row(chain: &Chain, fallback: u32) -> u32 {
    match chain.row {
        Some(chain::Row::Row(value)) => value,
        _ => fallback,
    }
}

/// A row/column grid lookup found no match, or found more than one. Callers
/// each phrase their own message (or, for a fail-closed no-op caller, ignore
/// the distinction entirely) -- this only carries which case occurred.
enum LocateError {
    Absent,
    Ambiguous,
}

/// Find the one model whose chain resolves to `row` (see `chain_row`) and
/// whose own position resolves to `column` (see `model_column`), in a single
/// pass over the whole preset. Row and column are matched jointly: two
/// chains sharing a row are not by themselves ambiguous here, only two
/// (chain, model) pairs that both land on the exact same (row, column).
fn locate_model(
    preset: &BinaryPreset,
    row: u32,
    column: u32,
) -> Result<(usize, usize, &Model), LocateError> {
    let mut matches = preset
        .chains
        .iter()
        .enumerate()
        .flat_map(|(chain_index, chain)| {
            let chain_row = chain_row(chain, chain_index as u32);
            chain
                .models
                .iter()
                .enumerate()
                .filter_map(move |(model_index, model)| {
                    (chain_row == row && model_column(model, model_index as u32) == column)
                        .then_some((chain_index, model_index, model))
                })
        });
    let found = matches.next().ok_or(LocateError::Absent)?;
    if matches.next().is_some() {
        return Err(LocateError::Ambiguous);
    }
    Ok(found)
}

/// Find the one chain that resolves to `row` (see `chain_row`). Distinct from
/// `locate_model`: a row shared by more than one chain is ambiguous here even
/// if only one of them actually has a model at some column of interest --
/// callers that need a model within the resolved chain search it separately.
fn locate_chain(preset: &BinaryPreset, row: u32) -> Result<(usize, &Chain), LocateError> {
    let mut chains = preset
        .chains
        .iter()
        .enumerate()
        .filter(|(index, chain)| chain_row(chain, *index as u32) == row);
    let found = chains.next().ok_or(LocateError::Absent)?;
    if chains.next().is_some() {
        return Err(LocateError::Ambiguous);
    }
    Ok(found)
}

/// The input port id a chain carries, or `None` when the device wrote none. An absent id is not
/// the same as id 0: a unit has been seen to write 0 explicitly for an unrouted row.
fn chain_input(chain: &Chain) -> Option<u32> {
    match chain.in_portid {
        Some(chain::InPortid::InPortid(value)) => Some(value),
        _ => None,
    }
}
/// The output port id a chain carries, or `None` when the device wrote none.
fn chain_output(chain: &Chain) -> Option<u32> {
    match chain.out_portid {
        Some(chain::OutPortid::OutPortid(value)) => Some(value),
        _ => None,
    }
}
/// How a parameter that names no index is addressed. The wire has two shapes and each needs its
/// own rule; using one for both misreads the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbsentIndex {
    /// A full preset record lists every parameter of a model in order and names no index; the
    /// parameter's place in the list is its index. A tempo block of 24 parameters, each with eight
    /// scene values and no index, was read from a unit this way.
    Position,
    /// A sparse push lists only the parameters that changed, and an index of zero is left
    /// off like any other zero scalar, so an absent index is 0. The desktop application's
    /// receiver for the global tempo message reads it so.
    Zero,
}

/// The index of `parameter`: the one it names, else `position` (a full record).
fn param_index(parameter: &Param, position: u32) -> u32 {
    param_index_by(parameter, position, AbsentIndex::Position)
}

fn param_index_by(parameter: &Param, position: u32, absent: AbsentIndex) -> u32 {
    match parameter.index {
        Some(param::Index::Index(value)) => value,
        _ => match absent {
            AbsentIndex::Position => position,
            AbsentIndex::Zero => 0,
        },
    }
}
fn param_scene_mode(parameter: &Param) -> bool {
    matches!(
        parameter.scene_mode,
        Some(param::SceneMode::SceneMode(true))
    )
}
fn float_value(value: &crate::proto::ParamValue) -> Option<f32> {
    match value.value {
        Some(param_value::Value::FloatValue(value)) => Some(value),
        _ => None,
    }
}

fn normalized_parameter(parameter: &Param, scene: u32, options: &[String]) -> Option<f64> {
    // A singleton is the established shared-value compatibility shape. A
    // multi-value scene-mode record must contain the active index itself;
    // another scene cannot supply current-value or verification evidence.
    let selected = if param_scene_mode(parameter) && parameter.param_values.len() != 1 {
        scene as usize
    } else {
        0
    };
    match parameter.param_values.get(selected)?.value.as_ref()? {
        param_value::Value::FloatValue(value) if (0.0..=1.0).contains(value) => Some(*value as f64),
        param_value::Value::StringValue(value) if !options.is_empty() => {
            options.iter().position(|item| item == value).map(|index| {
                if options.len() == 1 {
                    0.0
                } else {
                    index as f64 / (options.len() - 1) as f64
                }
            })
        }
        _ => None,
    }
}

fn effective_string_parameter(parameter: &Param, scene: u32) -> Option<&str> {
    let selected = if param_scene_mode(parameter) && parameter.param_values.len() != 1 {
        scene as usize
    } else {
        0
    };
    match parameter.param_values.get(selected)?.value.as_ref()? {
        param_value::Value::StringValue(value) => Some(value),
        _ => None,
    }
}

fn cab_microphone_options(model_id: u32, value: Option<&str>) -> Vec<String> {
    const MICROPHONES: &[&str] = &[
        "Condenser 184",
        "Condenser 414",
        "Dynamic 421",
        "Dynamic 57",
        "Ribbon 10",
        "Ribbon 160",
    ];
    if !matches!(model_id, 12024 | 32024) {
        return Vec::new();
    }
    let Some(prefix) = value.and_then(|value| value.rsplit_once('_').map(|pair| pair.0)) else {
        return Vec::new();
    };
    MICROPHONES
        .iter()
        .map(|microphone| format!("{prefix}_{microphone}"))
        .collect()
}

fn parameter_options(
    info: Option<&ModelInfo>,
    parameter: &Param,
    index: u32,
    model_id: u32,
    active_scene: u32,
    routing_node: Option<&str>,
) -> Vec<String> {
    let mut options = if parameter.dynamic_steps.is_empty() {
        info.and_then(|item| item.parameters.get(&index))
            .map(|item| item.options.clone())
            .unwrap_or_default()
    } else {
        parameter.dynamic_steps.clone()
    };
    if options.is_empty() && matches!(index, 1 | 9) {
        options = cab_microphone_options(
            model_id,
            effective_string_parameter(parameter, active_scene),
        );
        // Every other cab carries its microphone as the same kind of string
        // ("<cab>_<microphone>") but the device never publishes the list of
        // microphones it can switch to. The selection it reports is still the
        // one the screen shows, so expose it as a lone, fixed option: the name
        // reads correctly and nothing offers a switch the app cannot name.
        if options.is_empty()
            && info.is_some_and(|item| item.category.to_ascii_lowercase().contains("cabsim"))
        {
            if let Some(selected) = effective_string_parameter(parameter, active_scene)
                .filter(|value| !value.is_empty())
            {
                options = vec![selected.to_string()];
            }
        }
    }
    if options.is_empty() {
        options = routing_parameter_options(routing_node, index);
    }
    options
}

fn param_expression(parameter: &Param) -> Option<i32> {
    match parameter.expression {
        Some(param::Expression::Expression(value)) => Some(value),
        _ => None,
    }
}

fn param_expression_minimum(parameter: &Param) -> Option<f32> {
    match parameter.expression_min {
        Some(param::ExpressionMin::ExpressionMin(value)) => Some(value),
        _ => None,
    }
}

fn param_expression_maximum(parameter: &Param) -> Option<f32> {
    match parameter.expression_max {
        Some(param::ExpressionMax::ExpressionMax(value)) => Some(value),
        _ => None,
    }
}

fn tempo_state(model: Option<&Model>) -> TempoState {
    model
        .map(|item| tempo_from_params(&item.params, AbsentIndex::Position))
        .unwrap_or_default()
}
fn tempo_from_params(parameters: &[Param], absent: AbsentIndex) -> TempoState {
    let mut state = TempoState::default();
    for (fallback, parameter) in parameters.iter().enumerate() {
        let Some(value) = parameter
            .param_values
            .first()
            .and_then(float_value)
            .filter(|value| (0.0..=1.0).contains(value))
        else {
            continue;
        };
        match param_index_by(parameter, fallback as u32, absent) {
            0 => {
                state.bpm = Some(crate::commands::tempo_from_normalized(value));
            }
            2 => state.led_enabled = Some(value >= 0.5),
            _ => {}
        }
    }
    state
}

/// How a bypass record's grid address was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BypassAddress {
    /// The record names both its own row and its own column.
    Explicit,
    /// The record names neither; its place in the preset's bypass list is the address.
    Positional,
    /// The record names one of the two and takes the other from its place in the list.
    Mixed,
}

impl BypassAddress {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Positional => "positional",
            Self::Mixed => "mixed",
        }
    }
}

/// One block's bypass record: its per-scene flags and the scene-mode flag exactly as the device
/// wrote them, and how the record's grid address was found.
#[derive(Debug, Clone)]
struct BypassRecord {
    values: Vec<bool>,
    scene_mode: Option<bool>,
    address: BypassAddress,
}

/// Index a preset's bypass records by grid address.
///
/// A record that carries its own row or column is addressed by it. A record that carries
/// neither is addressed by its place in the list: the n-th record is row n, and the n-th entry
/// of a record is column n. Both rules apply per axis. The positional rule is what every full
/// preset examined needed, because those records name no row and no column at all; the explicit
/// rule is what a Grid push needs, because it names only the block it changes. Which rule a
/// record needed is reported, not hidden.
///
/// The per-scene flags of a record are all kept. That a record with its scene-mode flag false
/// stands for one value across all scenes, and which of its entries then counts, has not been
/// verified; callers take the entry for the active scene either way.
fn bypass_records(rows: &[crate::proto::Bypass]) -> HashMap<(u32, u32), BypassRecord> {
    let mut result: HashMap<(u32, u32), BypassRecord> = HashMap::new();
    for (row_index, row) in rows.iter().enumerate() {
        let explicit_row = match row.row {
            Some(bypass::Row::Row(value)) => Some(value),
            _ => None,
        };
        for (column_index, column) in row.col_bypass.iter().enumerate() {
            let explicit_column = match column.column {
                Some(col_bypass::Column::Column(value)) => Some(value),
                _ => None,
            };
            let address = match (explicit_row.is_some(), explicit_column.is_some()) {
                (true, true) => BypassAddress::Explicit,
                (false, false) => BypassAddress::Positional,
                _ => BypassAddress::Mixed,
            };
            let scene_mode = match column.scene_mode {
                Some(col_bypass::SceneMode::SceneMode(value)) => Some(value),
                _ => None,
            };
            let key = (
                explicit_row.unwrap_or(row_index as u32),
                explicit_column.unwrap_or(column_index as u32),
            );
            if let Some(existing) = result.get_mut(&key) {
                // Conflicting selectors provide no authoritative ordering.
                // Preserve the raw protobuf for writes, but never choose an
                // arbitrary record as observed/verification bypass evidence.
                // Further duplicates cannot restore a value for this key.
                existing.values.clear();
                continue;
            }
            result.insert(
                key,
                BypassRecord {
                    values: column.scene_bypass.iter().map(|item| item.bypass).collect(),
                    scene_mode,
                    address,
                },
            );
        }
    }
    result
}

fn project_scene_bypasses(
    preset: &BinaryPreset,
    records: &HashMap<(u32, u32), (u32, BypassRecord)>,
    active_scene: u32,
) -> Vec<GridBlock> {
    let mut blocks = records
        .iter()
        .filter_map(|(&(row, column), (model_id, record))| {
            if locate_model(preset, row, column)
                .ok()
                .and_then(|(_, _, model)| model_hash(model))
                != Some(*model_id)
            {
                return None;
            }
            Some(GridBlock {
                id: format!("{row}:{column}"),
                row,
                column,
                model_id: Some(*model_id),
                name: format!("Model {model_id}"),
                kind: "utility".into(),
                bypassed: bypass_value_for_scene(record, active_scene),
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    blocks.sort_by_key(|block| (block.row, block.column));
    blocks
}

fn bypass_value_for_scene(record: &BypassRecord, active_scene: u32) -> Option<bool> {
    let index = if record.scene_mode == Some(true) && record.values.len() != 1 {
        active_scene as usize
    } else {
        (active_scene as usize).min(record.values.len().saturating_sub(1))
    };
    record.values.get(index).copied()
}

fn witnessed_scene_bypasses(
    preset: &BinaryPreset,
    rows: &[crate::proto::Bypass],
) -> HashMap<(u32, u32), (u32, BypassRecord)> {
    bypass_records(rows)
        .into_iter()
        .filter_map(|((row, column), mut record)| {
            if row >= domain::GRID_ROWS || column >= domain::GRID_COLUMNS {
                return None;
            }
            let (_, _, model) = locate_model(preset, row, column).ok()?;
            let model_id = model_hash(model).filter(|id| *id != 0)?;
            // Values past the known selectable scene domain cannot be selected.
            // This is a bounded host projection, not a new firmware array-size claim.
            record.values.truncate(domain::SCENE_COUNT as usize);
            Some(((row, column), (model_id, record)))
        })
        .collect()
}

fn bypass_updates(rows: &[crate::proto::Bypass], active_scene: u32) -> Vec<BypassUpdate> {
    let mut updates = bypass_records(rows)
        .into_iter()
        .filter_map(|((row, column), record)| {
            let bypassed = bypass_value_for_scene(&record, active_scene)?;
            Some(BypassUpdate {
                row,
                column,
                bypassed,
            })
        })
        .collect::<Vec<_>>();
    updates.sort_by_key(|item| (item.row, item.column));
    updates
}

/// The name a footswitch mode value carries when it is not one of the nine values the vendor's
/// desktop application enumerates. The raw number travels beside it as `rawMode`; an unknown
/// value is never mapped to a known mode.
pub(crate) const UNKNOWN_MODE: &str = "UNKNOWN";

/// The base mode of a `ModeMessage.mode` wire value `0..=2`.
///
/// The wire schema carries no enum for this field. The numbering is the one the vendor's desktop
/// application uses for its own mode enum (Preset 0, Scene 1, Stomp 2, then six two-mode hybrids),
/// which is a static reading of that application, not an observation of the unit. One physical
/// observation sits awkwardly with it: an explicit write of `0` did not change the active mode
/// while a write of `1` did, and nothing explains why. The unit's own mode list was read back as
/// `[1, 0, 2]`.
fn input_wire_name(value: u32) -> Option<&'static str> {
    domain::INPUT_ROUTE_WIRE_NAMES
        .iter()
        .find(|(id, _)| *id == value)
        .map(|(_, name)| *name)
}

fn output_wire_name(value: u32) -> Option<&'static str> {
    domain::OUTPUT_ROUTE_WIRE_NAMES
        .iter()
        .find(|(id, _)| *id == value)
        .map(|(_, name)| *name)
}

fn input_label(value: u32) -> String {
    route_label(
        value,
        INPUT_LABEL_ALIASES,
        domain::INPUT_ROUTES,
        domain::INPUT_ROUTE_WIRE_NAMES,
        "Unknown input",
    )
}

/// Display label of an expression input: the reported id with an `EXP `
/// prefix. The ids reported in this project's runs are 1 and 2, and the unit's
/// own screens name two jacks, EXP 1 and EXP 2. That id 1 is the jack printed
/// EXP 1 is an inference and has not been checked per jack.
fn expression_port_label(exp_port_id: i32) -> String {
    format!("EXP {exp_port_id}")
}

fn output_label(value: u32) -> String {
    route_label(
        value,
        OUTPUT_LABEL_ALIASES,
        domain::OUTPUT_ROUTES,
        domain::OUTPUT_ROUTE_WIRE_NAMES,
        "Unknown output",
    )
}

fn extract_xml(payload: &[u8]) -> Result<&[u8], StateDecodeError> {
    let first = payload
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(payload.len());
    if payload.get(first) == Some(&b'<') {
        return Ok(payload);
    }
    let mut offset = 0usize;
    while offset + 512 <= payload.len() {
        let name = tar_text(&payload[offset..offset + 100]);
        if name.is_empty() {
            break;
        }
        let size_text = tar_text(&payload[offset + 124..offset + 136]);
        let size = usize::from_str_radix(size_text.trim(), 8)
            .map_err(|_| StateDecodeError::CatalogArchive("invalid tar size".into()))?;
        let content = offset + 512;
        let end = content
            .checked_add(size)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| StateDecodeError::CatalogArchive("truncated tar member".into()))?;
        if name.ends_with(".xml") {
            return Ok(&payload[content..end]);
        }
        offset = content + size.div_ceil(512) * 512;
    }
    Err(StateDecodeError::CatalogArchive("no XML member".into()))
}
fn tar_text(value: &[u8]) -> String {
    String::from_utf8_lossy(
        &value[..value
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(value.len())],
    )
    .trim()
    .into()
}

fn parse_catalog(xml: &[u8]) -> Result<HashMap<u32, ModelInfo>, StateDecodeError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut catalog = HashMap::new();
    let mut category = String::new();
    let mut category_hidden = false;
    let mut current: Option<u32> = None;
    let mut parameter_index = 0u32;
    let mut retained_cost = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                let name = event.local_name();
                let attrs = event
                    .attributes()
                    .filter_map(Result::ok)
                    .filter_map(|attribute| {
                        let key = String::from_utf8_lossy(attribute.key.local_name().as_ref())
                            .into_owned();
                        let value = attribute
                            .normalized_value(XmlVersion::Implicit1_0)
                            .ok()?
                            .into_owned();
                        Some((key, value))
                    })
                    .collect::<HashMap<_, _>>();
                if name.as_ref() == b"Category" {
                    category = attrs.get("name").cloned().unwrap_or_default();
                    category_hidden = attrs
                        .get("hidden")
                        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
                } else if name.as_ref() == b"Model" {
                    current = attrs.get("id").and_then(|value| value.parse::<u32>().ok());
                    parameter_index = 0;
                    if let Some(id) = current {
                        let model_name = attrs
                            .get("name")
                            .cloned()
                            .unwrap_or_else(|| format!("Model {id}"));
                        let lower = category.to_lowercase();
                        let kind = if lower.contains("amp") {
                            "amp"
                        } else if lower.contains("cab") || lower.contains("impulse") {
                            "cab"
                        } else if lower.contains("delay") {
                            "delay"
                        } else if lower.contains("reverb") {
                            "reverb"
                        } else if lower.contains("mod") || lower.contains("pitch") {
                            "mod"
                        } else if lower.contains("capture") {
                            "capture"
                        } else if lower.contains("input") {
                            "input"
                        } else if lower.contains("output") {
                            "output"
                        } else {
                            "utility"
                        };
                        let info = ModelInfo {
                            name: model_name,
                            category: category.clone(),
                            kind: kind.into(),
                            clone_id: attrs.get("clones").and_then(|value| value.parse().ok()),
                            hidden: category_hidden
                                || attrs
                                    .get("hidden")
                                    .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                                || attrs
                                    .get("internal")
                                    .is_some_and(|value| value.eq_ignore_ascii_case("true")),
                            based_on: attrs.get("basedOn").cloned().unwrap_or_default(),
                            plugin_id: attrs
                                .get("plugin_id")
                                .map(|value| value.parse::<u64>())
                                .transpose()
                                .map_err(|_| {
                                    StateDecodeError::CatalogXml("invalid plugin_id".into())
                                })?,
                            sku: attrs.get("sku").cloned(),
                            ..Default::default()
                        };
                        let next_cost = retained_cost
                            .saturating_sub(catalog.get(&id).map_or(0, catalog_model_cost))
                            .saturating_add(catalog_model_cost(&info));
                        if next_cost > MODEL_CATALOG_MEMORY_LIMIT
                            || (catalog.len() >= RETAINED_ENTRY_LIMIT && !catalog.contains_key(&id))
                        {
                            return Err(StateDecodeError::RetentionLimit("model catalog metadata"));
                        }
                        retained_cost = next_cost;
                        catalog.insert(id, info);
                    }
                } else if name.as_ref() == b"Parameter" {
                    validate_catalog_numeric_attributes(&attrs)?;
                    if let Some(model) = current.and_then(|id| catalog.get_mut(&id)) {
                        let replacement = attrs
                            .get("replaces")
                            .and_then(|value| value.parse::<u32>().ok());
                        let index = replacement.unwrap_or(parameter_index);
                        let kind = attrs.get("type").cloned().unwrap_or_else(|| "float".into());
                        let raw_name = attrs
                            .get("name")
                            .cloned()
                            .unwrap_or_else(|| format!("Parameter {index}"));
                        let mut minimum = attrs.get("min").and_then(|value| catalog_number(value));
                        let mut maximum = attrs.get("max").and_then(|value| catalog_number(value));
                        let options = split_text_list(attrs.get("stepNames"));
                        let x_values = number_list(attrs.get("xValues"));
                        let y_values = number_list(attrs.get("yValues"));
                        let scale_points =
                            if x_values.len() == y_values.len() && x_values.len() >= 2 {
                                x_values
                                    .into_iter()
                                    .zip(y_values)
                                    .map(|(normalized, real)| ScalePoint { normalized, real })
                                    .collect()
                            } else {
                                Vec::new()
                            };
                        let skew_raw = attrs.get("skew").map(String::as_str).unwrap_or("").trim();
                        let (value_scale, scale_exponent) = if !scale_points.is_empty() {
                            ("lookup".to_string(), None)
                        } else if !options.is_empty() {
                            ("options".to_string(), None)
                        } else {
                            // Despite its name, LOG_SKEW is the device's ordinary
                            // power law with skew 0.3, not a logarithmic scale.
                            let skew = if skew_raw == "LOG_SKEW" {
                                0.3
                            } else {
                                catalog_number(skew_raw).unwrap_or(1.0)
                            };
                            if (skew - 1.0).abs() <= f64::EPSILON {
                                ("linear".to_string(), None)
                            } else {
                                ("power".to_string(), Some(1.0 / skew))
                            }
                        };
                        if value_scale == "linear"
                            && minimum.zip(maximum).is_some_and(|(low, high)| low != high)
                            && ["min_string", "mid_string", "max_string"]
                                .iter()
                                .all(|name| attrs.get(*name).is_some_and(|value| !value.is_empty()))
                        {
                            // All three labels are the discriminator. One or two labels are
                            // common on unrelated controls (especially an OFF minimum), while
                            // exactly these pan-style controls carry the complete L/C/R set.
                            minimum = Some(LABELLED_END_SPAN.0);
                            maximum = Some(LABELLED_END_SPAN.1);
                        }
                        let dummy =
                            kind.eq_ignore_ascii_case("empty") && raw_name.starts_with("DUMMY");
                        let scale_known = minimum.is_some()
                            && maximum.is_some()
                            && (value_scale != "logarithmic"
                                || minimum.is_some_and(|value| value > 0.0));
                        let display_position_raw =
                            attrs.get("displayPos").and_then(|value| value.parse().ok());
                        let spec = ParameterSpec {
                            index,
                            name: raw_name.replace('_', " "),
                            r#type: kind.clone(),
                            minimum: if dummy { 0.0 } else { minimum.unwrap_or(0.0) },
                            maximum: if dummy { 1.0 } else { maximum.unwrap_or(1.0) },
                            scale_known: dummy || scale_known,
                            value_scale: if dummy {
                                "linear".into()
                            } else if scale_known {
                                value_scale
                            } else {
                                "unknown".into()
                            },
                            scale_exponent: if dummy { None } else { scale_exponent },
                            scale_points,
                            units: attrs.get("units").cloned().unwrap_or_default(),
                            steps: attrs
                                .get("steps")
                                .and_then(|value| value.parse().ok())
                                .filter(|value| *value > 0),
                            display_position: display_position_raw.unwrap_or(parameter_index),
                            display_position_raw,
                            screen_visible: true,
                            options,
                            hidden: attrs
                                .get("hidden")
                                .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                                || dummy,
                            minimum_label: attrs.get("min_string").cloned(),
                            midpoint_label: attrs.get("mid_string").cloned(),
                            maximum_label: attrs.get("max_string").cloned(),
                            show_as_integer: attrs
                                .get("showAsInteger")
                                .is_some_and(|value| value.eq_ignore_ascii_case("true")),
                            expression_assignable: !attrs
                                .get("expAssignable")
                                .is_some_and(|value| value.eq_ignore_ascii_case("false")),
                            linked_scene_mode: attrs
                                .get("linkedSceneMode")
                                .and_then(|value| value.parse().ok()),
                            enable_when_on: attrs
                                .get("toggleOn")
                                .and_then(|value| value.parse().ok()),
                            enable_when_off: attrs
                                .get("toggleOff")
                                .and_then(|value| value.parse().ok()),
                            enable_when_steps: split_u32_list(attrs.get("toggleStep")),
                            replaces: replacement,
                        };
                        retained_cost = retained_cost.saturating_add(catalog_parameter_cost(&spec));
                        if retained_cost > MODEL_CATALOG_MEMORY_LIMIT {
                            return Err(StateDecodeError::RetentionLimit(
                                "model catalog parameters",
                            ));
                        }
                        model.direct_parameters.push(spec);
                        if replacement.is_none() {
                            parameter_index = parameter_index.checked_add(1).ok_or(
                                StateDecodeError::RetentionLimit("model catalog parameter indices"),
                            )?;
                        }
                    }
                }
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == b"Model" => current = None,
            Ok(Event::Eof) => break,
            Err(error) => return Err(StateDecodeError::CatalogXml(error.to_string())),
            _ => {}
        }
    }
    let ids = catalog.keys().copied().collect::<Vec<_>>();
    let mut resolved = HashMap::new();
    for id in ids {
        resolve_catalog_model(
            id,
            &mut catalog,
            &mut HashSet::new(),
            &mut resolved,
            &mut retained_cost,
        )?;
    }
    for (model_id, model) in &mut catalog {
        let cab_category = model.category.to_ascii_lowercase().contains("cabsim");
        let has_explicit_layout = model
            .parameters
            .values()
            .any(|parameter| !parameter.hidden && parameter.display_position_raw.is_some());
        let mut implicit_position = 0;
        for parameter in model.parameters.values_mut() {
            let hidden = model.hidden || parameter.hidden;
            let trails = parameter.name.trim().eq_ignore_ascii_case("trails");
            parameter.screen_visible = !hidden
                && (!has_explicit_layout || parameter.display_position_raw.is_some() || trails);
            parameter.display_position =
                parameter.display_position_raw.unwrap_or(implicit_position);
            if cab_category {
                if let Some(maximum) = cab_level_maximum_from_skew(parameter) {
                    parameter.maximum = maximum;
                }
            }
            if *model_id == 4008 && parameter.index == 5 && parameter.display_position == 8 {
                parameter.display_position = 7;
            }
            parameter.hidden = hidden;
            if !hidden {
                implicit_position += 1;
            }
        }
    }
    Ok(catalog)
}

/// A cab LEVEL control is published as -40..+12 dB, but its skew is the one
/// that puts 0 dB at the exact centre of travel for a -40..+6 dB span (the same
/// skew the ROOM LEVEL controls carry beside it). Read literally, the default
/// (normalized 0.5) comes out as +5.2 dB while the unit shows 0.0 dB for it.
/// The skew is the part the unit's own screen is consistent with, so the top of
/// the span is taken from it: 0 dB at 0.5 means maximum = minimum - minimum /
/// 0.5^exponent, and only a whole-number result that disagrees with the
/// published maximum is applied.
fn cab_level_maximum_from_skew(parameter: &ParameterSpec) -> Option<f64> {
    if !parameter.name.trim().eq_ignore_ascii_case("level")
        || !parameter.units.trim().eq_ignore_ascii_case("db")
        || parameter.value_scale != "power"
        || parameter.minimum >= 0.0
    {
        return None;
    }
    let exponent = parameter.scale_exponent?;
    let centre = 0.5_f64.powf(exponent);
    if !(centre > 0.0 && centre < 1.0) {
        return None;
    }
    let implied = parameter.minimum - parameter.minimum / centre;
    ((implied - implied.round()).abs() < 0.01
        && (implied - parameter.maximum).abs() > 0.5
        && implied > 0.0
        && implied < parameter.maximum)
        .then_some(implied.round())
}

fn catalog_parameter_cost(spec: &ParameterSpec) -> usize {
    // Owned string payload plus record/vector elements, including the separately
    // retained direct and resolved copies. Container allocator slack is also
    // bounded by these operational byte/count ceilings; this is not firmware data.
    let strings = [&spec.name, &spec.r#type, &spec.value_scale, &spec.units]
        .into_iter()
        .chain(spec.options.iter())
        .chain(spec.minimum_label.iter())
        .chain(spec.midpoint_label.iter())
        .chain(spec.maximum_label.iter())
        .fold(0usize, |bytes, value| bytes.saturating_add(value.len()));
    std::mem::size_of::<(u32, ParameterSpec)>()
        .saturating_add(strings)
        .saturating_add(
            spec.scale_points
                .len()
                .saturating_mul(std::mem::size_of::<ScalePoint>()),
        )
        .saturating_add(
            spec.enable_when_steps
                .len()
                .saturating_mul(std::mem::size_of::<u32>()),
        )
        .saturating_add(
            spec.options
                .len()
                .saturating_mul(std::mem::size_of::<String>()),
        )
}

fn catalog_model_cost(info: &ModelInfo) -> usize {
    let metadata = [&info.name, &info.category, &info.kind, &info.based_on]
        .into_iter()
        .chain(info.sku.iter())
        .fold(std::mem::size_of::<(u32, ModelInfo)>(), |bytes, value| {
            bytes.saturating_add(value.len())
        });
    info.direct_parameters
        .iter()
        .chain(info.parameters.values())
        .fold(metadata, |bytes, spec| {
            bytes.saturating_add(catalog_parameter_cost(spec))
        })
}

fn resolve_catalog_model(
    id: u32,
    catalog: &mut HashMap<u32, ModelInfo>,
    visiting: &mut HashSet<u32>,
    resolved: &mut HashMap<u32, usize>,
    retained_cost: &mut usize,
) -> Result<(), StateDecodeError> {
    if resolved.contains_key(&id) {
        return Ok(());
    }
    if visiting.len() >= MODEL_CATALOG_CLONE_DEPTH_LIMIT {
        return Err(StateDecodeError::RetentionLimit(
            "model catalog clone depth",
        ));
    }
    if !visiting.insert(id) {
        return Err(StateDecodeError::CatalogXml(
            "cyclic model clone ancestry".into(),
        ));
    }
    let clone_id = catalog.get(&id).and_then(|item| item.clone_id);
    if let Some(base) = clone_id {
        resolve_catalog_model(base, catalog, visiting, resolved, retained_cost)?;
    }
    let depth = clone_id
        .and_then(|base| resolved.get(&base))
        .copied()
        .unwrap_or(0)
        .saturating_add(1);
    if depth > MODEL_CATALOG_CLONE_DEPTH_LIMIT {
        return Err(StateDecodeError::RetentionLimit(
            "model catalog clone depth",
        ));
    }
    if let Some(item) = catalog.get(&id) {
        // Plan with borrowed specs. Reject amplification BEFORE cloning inherited
        // payloads, and resolve each base exactly once regardless of map order.
        let mut parameters: BTreeMap<u32, &ParameterSpec> = clone_id
            .and_then(|base| catalog.get(&base))
            .map(|base| {
                base.parameters
                    .iter()
                    .map(|(index, spec)| (*index, spec))
                    .collect()
            })
            .unwrap_or_default();
        let mut next = parameters
            .keys()
            .next_back()
            .map(|value| value.checked_add(1))
            .unwrap_or(Some(0))
            .ok_or(StateDecodeError::CatalogXml(
                "model parameter index overflow".into(),
            ))?;
        for direct in &item.direct_parameters {
            let target = if let Some(replacement) = direct.replaces {
                replacement
            } else {
                let target = next;
                next = next.checked_add(1).ok_or(StateDecodeError::CatalogXml(
                    "model parameter index overflow".into(),
                ))?;
                target
            };
            parameters.insert(target, direct);
        }
        let old_cost = item.parameters.values().fold(0usize, |bytes, spec| {
            bytes.saturating_add(catalog_parameter_cost(spec))
        });
        let new_cost = parameters.values().fold(0usize, |bytes, spec| {
            bytes.saturating_add(catalog_parameter_cost(spec))
        });
        let projected = retained_cost
            .saturating_sub(old_cost)
            .saturating_add(new_cost);
        if projected > MODEL_CATALOG_MEMORY_LIMIT {
            return Err(StateDecodeError::RetentionLimit(
                "resolved model catalog parameters",
            ));
        }
        let parameters = parameters
            .into_iter()
            .map(|(index, spec)| {
                let mut owned = (*spec).clone();
                owned.index = index;
                (index, owned)
            })
            .collect();
        catalog
            .get_mut(&id)
            .expect("the model being resolved remains present")
            .parameters = parameters;
        *retained_cost = projected;
    }
    visiting.remove(&id);
    resolved.insert(id, depth);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_catalog_numeric_metadata_is_refused_before_known_scale_projection() {
        for attribute in [
            "min='NaN' max='1'",
            "min='0' max='inf'",
            "min='0' max='1' skew='NaN'",
            "min='0' max='1' skew='0'",
            "min='0' max='1' skew='-1'",
            "min='0' max='1' skew='1e-320'",
            "min='-1e308' max='1e308'",
            "min='0' max='1' xValues='0,NaN,1' yValues='0,0.5,1'",
            "min='0' max='1' xValues='0,0.5,1' yValues='0,inf,1'",
            "min='0' max='1' xValues='0,1' yValues='-1e308,1e308'",
        ] {
            let xml = format!(
                "<Models><Model id='100'><Parameter name='LEVEL' {attribute}/></Model></Models>"
            );
            assert!(
                matches!(
                    parse_catalog(xml.as_bytes()),
                    Err(StateDecodeError::CatalogXml(_))
                ),
                "{attribute}"
            );
        }
        let catalog = parse_catalog(
            br#"<Models><Model id="100">
          <Parameter name="LEVEL" min="-40" max="12" skew="LOG_SKEW"/>
          <Parameter name="TABLE" min="0" max="1" xValues="0,0.5,1" yValues="-12,0,12"/>
          <Parameter name="UNRESOLVED" min="MIN_INPUT_TRIM" max="MAX_INPUT_TRIM"/>
        </Model></Models>"#,
        )
        .unwrap();
        let specs = &catalog[&100].parameters;
        assert_eq!(
            specs.len(),
            3,
            "all declared numeric metadata fixtures must be checked"
        );
        assert_eq!(specs[&0].scale_exponent, Some(1.0 / 0.3));
        assert_eq!(normalized_to_real(0.5, Some(&specs[&1])), 0.0);
        assert!(
            !specs[&2].scale_known,
            "unknown firmware symbols remain unknown, not invented"
        );
        for spec in specs.values() {
            assert!(spec.minimum.is_finite() && spec.maximum.is_finite());
            assert!(spec.scale_exponent.is_none_or(f64::is_finite));
        }
    }

    #[test]
    fn sparse_parameter_growth_fails_atomically_and_stays_unavailable_until_full_recall() {
        let mut decoder = recalled_layout_decoder();
        let sparse = |params| {
            grid_push(
                pa::message_action::Enum::Update,
                vec![layout_chain(
                    0,
                    vec![Model {
                        column: Some(model::Column::Column(1)),
                        params,
                        ..Default::default()
                    }],
                )],
            )
        };
        let frame = sparse(
            (0..RETAINED_SPARSE_PARAMETER_LIMIT as u32)
                .map(|index| numeric_param(index, 0.5))
                .collect(),
        );
        assert!(frame.len() < profile::MAX_FRAME_BYTES);
        decoder.decode(profile::MESSAGE_TYPE_GRID, &frame).unwrap();
        assert_eq!(
            decoder.parameter_overrides.len(),
            RETAINED_SPARSE_PARAMETER_LIMIT
        );
        decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &sparse(vec![numeric_param(0, 0.75)]),
            )
            .unwrap();
        assert_eq!(decoder.parameter_overrides[&(0, 1, 0)], 0.75);
        assert!(matches!(
            decoder.decode(
                profile::MESSAGE_TYPE_GRID,
                &sparse(vec![numeric_param(u32::MAX, 0.5)])
            ),
            Err(StateDecodeError::RetentionLimit("sparse parameter records"))
        ));
        assert!(decoder.sparse_state_invalidated());
        assert!(decoder.parameter_overrides.is_empty());
        assert!(decoder.preset.is_none());
        assert!(decoder.block_details(0, 1).is_none());
        assert!(decoder.move_block_operation(0, 1, 0, 4, None).is_err());
        assert!(decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &sparse(vec![numeric_param(0, 0.1)])
            )
            .is_err());
        assert!(decoder.sparse_state_invalidated());
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(
                        BinaryPreset::default(),
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert!(!decoder.sparse_state_invalidated());
        decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &sparse(vec![numeric_param(u32::MAX, 0.5)]),
            )
            .unwrap();
        assert_eq!(
            decoder.parameter_overrides.len(),
            1,
            "budget does not invent a numeric firmware index ceiling"
        );
        decoder.reset();
        assert!(!decoder.sparse_state_invalidated());
        let mut model = layout_model(99, 1);
        model.params = vec![numeric_param(u32::MAX, 0.5)];
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                        chains: vec![layout_chain(0, vec![model])],
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(
            decoder.block_details(0, 1).unwrap().parameters[0].name,
            "Parameter 4294967296"
        );
    }

    #[test]
    fn sparse_tempo_growth_shares_the_retention_budget_and_never_partially_applies_overflow() {
        let mut decoder = recalled_layout_decoder();
        let tempo = |params| {
            pa::GridMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                    tempo_program_data: vec![Model {
                        params,
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec()
        };
        let frame = tempo(
            (0..RETAINED_SPARSE_PARAMETER_LIMIT as u32)
                .map(|index| numeric_param(index, 0.5))
                .collect(),
        );
        assert!(frame.len() < profile::MAX_FRAME_BYTES);
        decoder.decode(profile::MESSAGE_TYPE_GRID, &frame).unwrap();
        assert_eq!(
            decoder.preset.as_ref().unwrap().tempo_program_data[0]
                .params
                .len(),
            RETAINED_SPARSE_PARAMETER_LIMIT
        );
        assert!(matches!(
            decoder.decode(
                profile::MESSAGE_TYPE_GRID,
                &tempo(vec![numeric_param(0, 0.1), numeric_param(u32::MAX, 0.5)])
            ),
            Err(StateDecodeError::RetentionLimit(_))
        ));
        assert!(decoder.sparse_state_invalidated());
        assert!(decoder.preset.is_none());
        assert!(decoder.parameter_overrides.is_empty());
    }

    #[test]
    fn model_clone_resolution_resolves_each_base_once_and_rejects_cycles_depth_and_index_overflow()
    {
        let catalog = parse_catalog(br#"<Models><Model id="100"><Parameter name="A"/><Parameter name="B"/></Model><Model id="101" clones="100"/><Model id="102" clones="100"><Parameter name="C"/></Model></Models>"#).unwrap();
        assert_eq!(catalog[&100].parameters.len(), 2);
        assert_eq!(catalog[&101].parameters.len(), 2);
        assert_eq!(catalog[&102].parameters.len(), 3);
        assert_eq!(catalog[&102].parameters[&2].name, "C");
        assert!(matches!(
            parse_catalog(
                br#"<Models><Model id="1" clones="2"/><Model id="2" clones="1"/></Models>"#
            ),
            Err(StateDecodeError::CatalogXml(_))
        ));
        assert!(matches!(parse_catalog(br#"<Models><Model id="1"><Parameter name="A" replaces="4294967295"/></Model><Model id="2" clones="1"/></Models>"#), Err(StateDecodeError::CatalogXml(_))));
        let mut deep = String::from("<Models><Model id=\"0\"/>");
        for index in 1..=MODEL_CATALOG_CLONE_DEPTH_LIMIT {
            deep.push_str(&format!("<Model id=\"{index}\" clones=\"{}\"/>", index - 1));
        }
        deep.push_str("</Models>");
        assert!(matches!(
            parse_catalog(deep.as_bytes()),
            Err(StateDecodeError::RetentionLimit(
                "model catalog clone depth"
            ))
        ));
    }

    #[test]
    fn bounded_model_repo_input_cannot_amplify_retained_clone_or_metadata_payloads() {
        let name = "n".repeat(512 * 1024);
        let mut clones = format!("<Models><Model id=\"0\"><Parameter name=\"{name}\"/></Model>");
        for index in 1..150 {
            clones.push_str(&format!("<Model id=\"{index}\" clones=\"0\"/>"));
        }
        clones.push_str("</Models>");
        assert!(clones.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            parse_catalog(clones.as_bytes()),
            Err(StateDecodeError::RetentionLimit(
                "resolved model catalog parameters"
            ))
        ));
        let mut metadata = format!("<Models><Category name=\"{name}\">");
        for index in 0..150 {
            metadata.push_str(&format!("<Model id=\"{index}\"/>"));
        }
        metadata.push_str("</Category></Models>");
        assert!(metadata.len() < profile::MAX_FRAME_BYTES);
        assert!(matches!(
            parse_catalog(metadata.as_bytes()),
            Err(StateDecodeError::RetentionLimit("model catalog metadata"))
        ));
    }

    #[test]
    fn formerly_discarded_device_updates_publish_semantic_state() {
        let mut decoder = StateDecoder::new();

        let midi = pa::MidiSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            general_midi_messages: Some(
                pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(
                    pa::GeneralMidiMessages {
                        messages: Vec::new(),
                    },
                ),
            ),
            preset_load_messages: Some(
                pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(
                    pa::GeneralMidiMessages {
                        messages: Vec::new(),
                    },
                ),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        let midi_state = decoder
            .decode(profile::MESSAGE_TYPE_MIDI_SETTINGS, &midi)
            .unwrap()
            .remove(0);
        assert_eq!(midi_state.midi_out, Some(Vec::new()));
        assert_eq!(midi_state.preset_load_midi_out, Some(Vec::new()));

        let defaults = pa::DefaultParametersMessage {
            action: pa::message_action::Enum::Create as i32,
            row: 1,
            column: 2,
            all_defaults: Vec::new(),
            ..Default::default()
        }
        .encode_to_vec();
        let default_state = decoder
            .decode(profile::MESSAGE_TYPE_DEFAULT_PARAMETERS, &defaults)
            .unwrap()
            .remove(0);
        assert_eq!(default_state.row, Some(1));
        assert_eq!(default_state.column, Some(2));
        assert_eq!(default_state.default_parameter_count, Some(0));

        let history = pa::UndoRedoMessage {
            undo_frames: vec![1],
            redo_frames: vec![2],
            ..Default::default()
        }
        .encode_to_vec();
        let history_state = decoder
            .decode(profile::MESSAGE_TYPE_UNDO_REDO, &history)
            .unwrap()
            .remove(0);
        assert_eq!(history_state.undo_available, None);
        assert_eq!(history_state.redo_available, None);
        assert_eq!(history_state.undo_frames, Some(vec![1]));
        assert_eq!(history_state.redo_frames, Some(vec![2]));

        let scene_copy = pa::SceneCopyMessage {
            action: pa::message_action::Enum::Update as i32,
            from_index: 0,
            to_index: 1,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decoder
                .decode(profile::MESSAGE_TYPE_SCENE_COPY, &scene_copy)
                .unwrap()[0]
                .refresh_required,
            Some(true)
        );

        let gig = pa::ShowGigViewMessage {
            action: pa::message_action::Enum::Update as i32,
            show: true,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decoder
                .decode(profile::MESSAGE_TYPE_SHOW_GIG_VIEW, &gig)
                .unwrap()[0]
                .gig_view_shown,
            Some(true)
        );

        let cpu = pa::CpuLoadMessage {
            action: pa::message_action::Enum::Update as i32,
            cpu_total_load: Some(pa::cpu_load_message::CpuTotalLoad::CpuTotalLoad(0.42)),
            chains: vec![pa::CpuChainLoad {
                columns: vec![pa::CpuColumnLoad {
                    cpu_load: 0.21,
                    is_on_core2: true,
                }],
            }],
            ..Default::default()
        }
        .encode_to_vec();
        let cpu_state = decoder
            .decode(profile::MESSAGE_TYPE_CPU_LOAD, &cpu)
            .unwrap()
            .remove(0);
        assert_eq!(cpu_state.cpu_total_load, Some(0.42));
        assert_eq!(cpu_state.cpu_columns.unwrap()[0].core, 2);

        let models = pa::NewModelsMessage {
            models: vec![123, 456],
            ..Default::default()
        }
        .encode_to_vec();
        let catalog_state = decoder
            .decode(profile::MESSAGE_TYPE_NEW_MODELS, &models)
            .unwrap()
            .remove(0);
        assert_eq!(catalog_state.new_model_ids, Some(vec![123, 456]));
        assert_eq!(catalog_state.catalog_refresh, Some(true));

        let suspended = pa::SuspendConnectionMessage {
            suspend: Some(pa::suspend_connection_message::Suspend::Suspend(true)),
            reason: Some(pa::suspend_connection_message::ReasonOneOf::Reason(
                pa::suspend_connection_message::Reason::Backup as i32,
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let suspend_state = decoder
            .decode(profile::MESSAGE_TYPE_SUSPEND_CONNECTION, &suspended)
            .unwrap()
            .remove(0);
        assert_eq!(suspend_state.suspended, Some(true));
        assert_eq!(suspend_state.suspend_reason.as_deref(), Some("BACKUP"));

        let bulk = pa::BulkOperationMessage {
            progress_message: Some(
                pa::bulk_operation_message::ProgressMessage::ProgressMessage("Working".into()),
            ),
            blocking: Some(pa::bulk_operation_message::Blocking::Blocking(true)),
            cancellable: Some(pa::bulk_operation_message::Cancellable::Cancellable(false)),
            progress: Some(pa::bulk_operation_message::Progress::Progress(0.5)),
            finished: Some(pa::bulk_operation_message::Finished::Finished(false)),
            ..Default::default()
        }
        .encode_to_vec();
        let bulk_state = decoder
            .decode(profile::MESSAGE_TYPE_BULK_OPERATION, &bulk)
            .unwrap()
            .remove(0);
        let bulk_operation = bulk_state.bulk_operation.unwrap();
        assert_eq!(bulk_operation.message.as_deref(), Some("Working"));
        assert_eq!(bulk_operation.blocking, Some(true));
        assert_eq!(bulk_operation.cancellable, Some(false));
        assert_eq!(bulk_operation.progress, Some(0.5));
        assert_eq!(bulk_operation.finished, Some(false));

        let presets = pa::ModelPresetMessage {
            action: pa::message_action::Enum::Create as i32,
            presets: vec![pa::ModelPreset {
                id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                    value: Some(pa::model_preset_id::Value::Value("preset-id".into())),
                    is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(true)),
                    hash: Some(pa::model_preset_id::Hash::Hash(
                        crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
                    )),
                })),
                name: Some(pa::model_preset::Name::Name("Factory clean".into())),
                is_default: Some(pa::model_preset::IsDefault::IsDefault(false)),
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode_to_vec();
        let preset_state = decoder
            .decode(profile::MESSAGE_TYPE_MODEL_PRESET, &presets)
            .unwrap()
            .remove(0);
        assert_eq!(preset_state.kind, "modelPresetCatalog");
        let catalog = preset_state.model_preset_catalog.unwrap();
        assert_eq!(catalog.state, "folding");
        let preset = &catalog.entries.unwrap()[0];
        assert_eq!(preset.id, "preset-id");
        assert_eq!(
            preset.model_hash,
            Some(crate::state::GLOBAL_EQ_MODEL_PRESET_HASH)
        );
        assert!(preset.is_factory);

        let telemetry = decoder.inbound_telemetry().snapshot();
        assert!(telemetry.no_state_handler_types.is_empty());
    }

    #[test]
    fn decoder_folds_model_preset_create_update_and_delete() {
        const SCOPE: u32 = crate::state::GLOBAL_EQ_MODEL_PRESET_HASH;
        let preset = |value: &str, name: &str| pa::ModelPreset {
            id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                value: Some(pa::model_preset_id::Value::Value(value.into())),
                is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                hash: Some(pa::model_preset_id::Hash::Hash(SCOPE)),
            })),
            name: Some(pa::model_preset::Name::Name(name.into())),
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        let mut push = |message: pa::ModelPresetMessage| {
            let payload = message.encode_to_vec();
            let mut updates = decoder
                .decode(profile::MESSAGE_TYPE_MODEL_PRESET, &payload)
                .unwrap();
            assert_eq!(updates.len(), 1);
            updates.remove(0).model_preset_catalog.unwrap()
        };
        let listed = |catalog: &crate::generated_payloads::ModelPresetCatalogState| {
            catalog
                .entries
                .iter()
                .flatten()
                .map(|entry| format!("{}={}", entry.id, entry.name))
                .collect::<Vec<_>>()
        };

        let boot = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            presets: vec![preset("a", "Flat"), preset("b", "Warm")],
            ..Default::default()
        });
        assert_eq!(listed(&boot), ["a=Flat", "b=Warm"]);
        let created = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Create as i32,
            presets: vec![preset("c", "Bright")],
            ..Default::default()
        });
        assert_eq!(listed(&created), ["a=Flat", "b=Warm", "c=Bright"]);
        // One preset in an update renames; it never inserts.
        let renamed = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            presets: vec![preset("b", "Warmer")],
            ..Default::default()
        });
        assert_eq!(listed(&renamed), ["a=Flat", "b=Warmer", "c=Bright"]);
        let unknown = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            presets: vec![preset("zzz", "Ghost")],
            ..Default::default()
        });
        assert_eq!(listed(&unknown), listed(&renamed));
        // loaded_preset_id decides, whatever else the message lists.
        let loaded = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            presets: vec![preset("n1", "N1"), preset("n2", "N2")],
            loaded_preset_id: Some(pa::model_preset_message::LoadedPresetId::LoadedPresetId(
                pa::ModelPresetId {
                    value: Some(pa::model_preset_id::Value::Value("c".into())),
                    is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                    hash: Some(pa::model_preset_id::Hash::Hash(SCOPE)),
                },
            )),
            ..Default::default()
        });
        assert_eq!(listed(&loaded), listed(&renamed));
        assert_eq!(
            loaded.active_marker.as_ref().unwrap().id.as_deref(),
            Some("c")
        );
        let deleted = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Delete as i32,
            presets: vec![preset("a", "")],
            ..Default::default()
        });
        assert_eq!(listed(&deleted), ["b=Warmer", "c=Bright"]);
        let wiped = push(pa::ModelPresetMessage {
            action: pa::message_action::Enum::Delete as i32,
            ..Default::default()
        });
        assert_eq!(wiped.state, "folding");
        assert!(listed(&wiped).is_empty());
        assert!(wiped.active_marker.is_none());
    }

    #[test]
    fn decode_failure_of_a_model_preset_message_invalidates_the_fold() {
        let mut decoder = StateDecoder::new();
        let seed = pa::ModelPresetMessage {
            action: pa::message_action::Enum::Create as i32,
            presets: vec![pa::ModelPreset {
                id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                    value: Some(pa::model_preset_id::Value::Value("preset-id".into())),
                    is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                    hash: Some(pa::model_preset_id::Hash::Hash(
                        crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
                    )),
                })),
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode_to_vec();
        decoder
            .decode(profile::MESSAGE_TYPE_MODEL_PRESET, &seed)
            .unwrap();
        assert!(decoder
            .model_preset_catalog()
            .admits(
                "preset-id",
                false,
                crate::state::GLOBAL_EQ_MODEL_PRESET_HASH
            )
            .is_ok());

        // A truncated length-delimited field is not a protobuf message.
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_MODEL_PRESET, &[0x1a, 0x7f, 0x01])
            .is_err());
        let invalidated = decoder.model_preset_catalog_update();
        let catalog = invalidated.model_preset_catalog.unwrap();
        assert_eq!(catalog.state, "invalidated");
        assert_eq!(catalog.invalid_reason.as_deref(), Some("decode-failure"));

        // A later good message does not revive it; a reset does.
        decoder
            .decode(profile::MESSAGE_TYPE_MODEL_PRESET, &seed)
            .unwrap();
        assert!(decoder.model_preset_catalog().is_invalidated());
        decoder.reset();
        assert_eq!(decoder.model_preset_catalog().view().state, "unavailable");
    }

    #[test]
    fn a_declared_type_with_no_handler_is_counted_apart_from_a_corrupt_body() {
        let mut decoder = StateDecoder::new();

        // CloudProduct intentionally has no state handler: vendor cloud state
        // is not part of the local-control contract. An empty body is valid,
        // it, so the dispatch default sees a message it understood and did
        // nothing with -- which used to be indistinguishable from a handled
        // message that had nothing to report.
        assert_eq!(
            decoder
                .decode(profile::MESSAGE_TYPE_CLOUD_PRODUCT, &[])
                .unwrap(),
            Vec::new()
        );
        // The same unhandled type with a body that is not a valid encoding of
        // it at all. Wire tag 0xff is field 31 at reserved wire type 7.
        assert!(matches!(
            decoder.decode(profile::MESSAGE_TYPE_SHOW_GIG_VIEW, &[0xff, 0xff]),
            Err(StateDecodeError::Protobuf { .. })
        ));
        // An undeclared number reaches the same default; the transport owns
        // that bucket, so nothing is recorded twice here.
        assert_eq!(decoder.decode(73, &[]).unwrap(), Vec::new());

        let snapshot = decoder.inbound_telemetry().snapshot();
        assert_eq!(
            snapshot.no_state_handler_types[&profile::MESSAGE_TYPE_CLOUD_PRODUCT],
            1
        );
        assert_eq!(snapshot.no_state_handler_types.len(), 1);
        assert_eq!(
            snapshot.decode_failures[&profile::MESSAGE_TYPE_SHOW_GIG_VIEW],
            1
        );
        assert!(snapshot.undeclared_types.is_empty());
        // The corrupt body is unusable; the handler-less one may still be
        // read by another lane, so it is reported apart.
        assert_eq!(snapshot.unacted_frames, 1);
        assert_eq!(snapshot.no_state_handler_frames, 1);
        assert_eq!(
            snapshot.last_unacted.unwrap(),
            "type 24 (ShowGigView) did not decode"
        );
    }

    #[test]
    fn a_handled_type_that_does_not_decode_joins_the_same_tally() {
        let mut decoder = StateDecoder::new();
        assert!(matches!(
            decoder.decode(profile::MESSAGE_TYPE_SCENE, &[0xff, 0xff]),
            Err(StateDecodeError::Protobuf { .. })
        ));
        let snapshot = decoder.inbound_telemetry().snapshot();
        assert_eq!(snapshot.decode_failures[&profile::MESSAGE_TYPE_SCENE], 1);
        assert!(snapshot.no_state_handler_types.is_empty());
        assert_eq!(snapshot.unacted_frames, 1);
    }

    #[test]
    fn a_handled_type_that_decodes_is_not_counted_as_unacted() {
        let mut decoder = StateDecoder::new();
        decoder
            .decode(profile::MESSAGE_TYPE_SCENE, &[8, 1, 16, 2])
            .unwrap();
        let snapshot = decoder.inbound_telemetry().snapshot();
        assert_eq!(snapshot.unacted_frames, 0);
        assert_eq!(decoder.inbound_telemetry().revision(), 0);
    }

    #[test]
    fn preset_file_listing_is_occupied_matches_either_signal() {
        // The one definition `PresetLibrary::entry()` and `project_slots()`
        // both call. This test pins its exact truth table so an edit to the
        // predicate itself (as opposed to a call site) is caught here first.
        let base = PresetFileListing {
            position: 0,
            name: String::new(),
            instrument: 0,
            instrument_present: false,
            product_key: None,
            ..Default::default()
        };
        assert!(!base.is_occupied(), "no name and no product_key is free");
        assert!(
            PresetFileListing {
                product_key: Some("catalog-key".into()),
                ..base.clone()
            }
            .is_occupied(),
            "a product_key alone marks the slot occupied"
        );
        assert!(
            PresetFileListing {
                name: "Compatibility copy".into(),
                ..base.clone()
            }
            .is_occupied(),
            "a non-empty name alone marks the slot occupied (keyless compatibility record)"
        );
        assert!(
            PresetFileListing {
                name: "Both signals".into(),
                product_key: Some("catalog-key".into()),
                ..base
            }
            .is_occupied(),
            "both signals present is still occupied"
        );
    }

    #[test]
    fn plugin_models_are_admitted_without_license_data_while_unknown_and_hidden_are_refused() {
        // QC Remote does not establish plugin ownership; the device response
        // does. There is no local plugin precheck, so a plugin model with no
        // license data at all passes the guard.
        let mut decoder = StateDecoder::new();
        decoder.catalog = parse_catalog(br#"<Models><Category name="Wah"><Model id="9013" name="Built in"/><Model id="9022" name="Plugin" plugin_id="13" sku="30"/><Model id="9030" name="Hidden" hidden="true"/><Model id="9031" name="Internal" internal="true"/></Category><Category name="Secret" hidden="true"><Model id="9032" name="In hidden category"/></Category></Models>"#).unwrap();
        assert_eq!(decoder.license_observation(), None);
        assert!(decoder.assert_model_available(9013).is_ok());
        assert!(decoder.assert_model_available(9022).is_ok());
        assert!(decoder
            .assert_model_available(99999)
            .unwrap_err()
            .contains("absent from the synchronized QC catalog"));
        for hidden in [9030, 9031, 9032] {
            assert!(decoder
                .assert_model_available(hidden)
                .unwrap_err()
                .contains("not a selectable QC model"));
        }
        let list = decoder.model_list();
        let plugin = list.models.iter().find(|model| model.id == 9022).unwrap();
        assert!(plugin.available);
        assert_eq!(plugin.plugin_id, Some(13));
        assert_eq!(plugin.sku.as_deref(), Some("30"));
        assert!(list.models.iter().all(|model| model.available));
        assert!(list
            .models
            .iter()
            .all(|model| ![9030, 9031, 9032].contains(&model.id)));
        assert!(ModelCatalog(decoder.catalog.clone())
            .model_list()
            .models
            .iter()
            .all(|model| model.available));
        // No license state, however hostile, refuses a catalog model.
        for (enabled, deactivated) in [(false, true), (true, true), (false, false)] {
            let license = pa::LicenseMessage {
                action: pa::message_action::Enum::Update as i32,
                is_enabled: Some(pa::license_message::IsEnabled::IsEnabled(enabled)),
                is_deactivated: Some(pa::license_message::IsDeactivated::IsDeactivated(
                    deactivated,
                )),
                ..Default::default()
            };
            decoder.decode_license(&license.encode_to_vec()).unwrap();
            assert!(decoder.assert_model_available(9022).is_ok());
        }
        assert!(decoder.assert_model_available(99999).is_err());
        assert!(decoder.assert_model_available(9030).is_err());
    }

    #[test]
    fn license_updates_still_decode_replace_ownership_preserve_optional_flags_and_reset() {
        let mut decoder = StateDecoder::new();
        let mut message = pa::LicenseMessage {
            action: pa::message_action::Enum::Update as i32,
            is_enabled: Some(pa::license_message::IsEnabled::IsEnabled(true)),
            is_deactivated: Some(pa::license_message::IsDeactivated::IsDeactivated(false)),
            supported_plugin_ids: vec![13],
            new_plugin_ids: vec![13],
            ..Default::default()
        };
        decoder.decode_license(&message.encode_to_vec()).unwrap();
        // supported_plugin_ids and new_plugin_ids are firmware capability, not ownership.
        assert_eq!(
            decoder.license_observation(),
            Some(LicenseObservation {
                enabled: Some(true),
                deactivated: Some(false),
                plugin_ids: Vec::new(),
            })
        );
        message.licenses.push(pa::LicenseItem {
            plugin_id: 13,
            ..Default::default()
        });
        message.licenses.push(pa::LicenseItem {
            plugin_id: 3,
            ..Default::default()
        });
        decoder
            .decode(profile::MESSAGE_TYPE_LICENSE, &message.encode_to_vec())
            .unwrap();
        assert_eq!(
            decoder.license_observation().unwrap().plugin_ids,
            vec![3, 13]
        );
        // Absent optional flags keep their previous value.
        message.is_enabled = None;
        message.is_deactivated = None;
        decoder.decode_license(&message.encode_to_vec()).unwrap();
        let observed = decoder.license_observation().unwrap();
        assert_eq!(observed.enabled, Some(true));
        assert_eq!(observed.deactivated, Some(false));
        // Only UPDATE applies.
        for action in [
            pa::message_action::Enum::Read,
            pa::message_action::Enum::Delete,
            pa::message_action::Enum::Create,
        ] {
            let ignored = pa::LicenseMessage {
                action: action as i32,
                ..Default::default()
            };
            decoder.decode_license(&ignored.encode_to_vec()).unwrap();
            assert_eq!(decoder.license_observation(), Some(observed.clone()));
        }
        message.is_deactivated = Some(pa::license_message::IsDeactivated::IsDeactivated(true));
        decoder.decode_license(&message.encode_to_vec()).unwrap();
        assert_eq!(
            decoder.license_observation().unwrap().deactivated,
            Some(true)
        );
        // UPDATE replaces the list wholesale, including with an empty one.
        message.licenses.clear();
        decoder.decode_license(&message.encode_to_vec()).unwrap();
        assert!(decoder.license_observation().unwrap().plugin_ids.is_empty());
        decoder.reset();
        assert_eq!(decoder.license_observation(), None);
        assert!(decoder.decode_license(&[0xff]).is_err());
        assert!(parse_catalog(br#"<Model id="1" plugin_id="invalid"/>"#).is_err());
    }

    #[test]
    fn grid_delete_uses_explicit_selectors_without_shifting_or_publishing_parameters() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                models: (10..13)
                    .map(|id| Model {
                        hash: Some(model::Hash::Hash(id)),
                        params: vec![numeric_param(0, 0.5)],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.preset_exact = true;
        decoder.parameter_overrides.insert((0, 1, 0), 0.9);
        decoder.parameter_overrides.insert((0, 2, 0), 0.8);
        let mut delta = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![
                    Model {
                        column: Some(model::Column::Column(1)),
                        params: vec![numeric_param(0, 0.25)],
                        ..Default::default()
                    },
                    Model::default(),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        // Missing row must not silently select row zero, and missing column
        // must not silently select its position in this sparse vector.
        delta.chains.push(Chain {
            models: vec![Model {
                column: Some(model::Column::Column(0)),
                ..Default::default()
            }],
            ..Default::default()
        });
        let payload = pa::GridMessage {
            action: pa::message_action::Enum::Delete as i32,
            preset: Some(pa::grid_message::Preset::Preset(delta)),
            ..Default::default()
        }
        .encode_to_vec();
        let states = decoder
            .decode(profile::MESSAGE_TYPE_GRID, &payload)
            .unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].kind, "blockRemoved");
        assert_eq!((states[0].row, states[0].column), (Some(0), Some(1)));
        let models = &decoder.preset.as_ref().unwrap().chains[0].models;
        assert_eq!(
            models
                .iter()
                .map(|m| (model_hash(m), model_column(m, 99)))
                .collect::<Vec<_>>(),
            vec![(Some(10), 0), (Some(12), 2)]
        );
        assert!(!decoder.parameter_overrides.contains_key(&(0, 1, 0)));
        assert_eq!(decoder.parameter_overrides.get(&(0, 2, 0)), Some(&0.8));
        assert!(decoder.preset_source_snapshot().is_none());
        decoder.reset();
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_GRID, &payload)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn preset_source_equality_ignores_new_publication_revisions_and_map_insertion_order() {
        let mut first = BinaryPreset::default();
        first.stomp_labels.insert(1, "One".into());
        first.stomp_labels.insert(2, "Two".into());
        let mut second = BinaryPreset::default();
        second.stomp_labels.insert(2, "Two".into());
        second.stomp_labels.insert(1, "One".into());
        let payload = |preset| {
            pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                ..Default::default()
            }
            .encode_to_vec()
        };
        let mut decoder = StateDecoder::new();
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &payload(first))
            .unwrap();
        let source = decoder.preset_source_snapshot().unwrap();
        let revision = decoder.state_revision;
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &payload(second.clone()),
            )
            .unwrap();
        assert!(decoder.state_revision > revision);
        assert!(decoder.preset_source_matches(&source));
        second.volume = Some(binary_preset::Volume::Volume(0.5));
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &payload(second))
            .unwrap();
        assert!(!decoder.preset_source_matches(&source));
    }

    #[test]
    fn sparse_grid_cannot_supply_or_match_a_complete_save_source() {
        let mut decoder = StateDecoder::new();
        let payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(
                BinaryPreset::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &payload)
            .unwrap();
        let source = decoder.preset_source_snapshot().unwrap();
        let sparse = pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset::default())),
            ..Default::default()
        }
        .encode_to_vec();
        decoder.decode(profile::MESSAGE_TYPE_GRID, &sparse).unwrap();
        assert!(decoder.preset_source_snapshot().is_none());
        assert!(!decoder.preset_source_matches(&source));
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &payload)
            .unwrap();
        assert!(decoder.preset_source_matches(&source));
    }

    #[test]
    fn every_coros_category_resolves_to_its_official_lamp_color() {
        // The same nineteen rows the TypeScript and Python suites pin, so a
        // regeneration that moved one language cannot leave this one behind.
        // Drive is the orange the unit paints its Gig View tile and browser row
        // with and the owner sees on a lit lamp; Plugins is the green the
        // browser row paints, not the orange it used to carry.
        for (category, expected) in [
            ("Plugins", "#45f862"),
            ("Guitar Amplifier", "#ff2727"),
            ("Neural Capture", "#959595"),
            ("Guitar Cabinet", "#6954ff"),
            ("Guitar Overdrive", "#ff7000"),
            ("Delay", "#00ffdd"),
            ("Reverb", "#00ffdd"),
            ("Compressor", "#45f862"),
            ("Pitch", "#ffd236"),
            ("Modulation", "#3500f1"),
            ("Morph", "#87daff"),
            ("Synth", "#e44a5d"),
            ("Filter", "#87daff"),
            ("Equalizer", "#0a74e0"),
            ("IRLoaders", "#6954ff"),
            ("Wah", "#959595"),
            ("FX Loop", "#959595"),
            ("Loopers", "#ff2727"),
            ("Utility", "#959595"),
        ] {
            assert_eq!(
                domain::category_color(category, category),
                expected,
                "{category} must resolve to {expected}"
            );
        }
    }

    #[test]
    fn move_block_planning_preserves_exact_chain_and_model_order() {
        let model = |id, column| Model {
            hash: Some(model::Hash::Hash(id)),
            column: Some(model::Column::Column(column)),
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![
                Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![model(11, 1), model(12, 5)],
                    ..Default::default()
                },
                Chain {
                    row: Some(chain::Row::Row(2)),
                    models: vec![model(21, 3)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        decoder.preset_exact = true;

        let operation = decoder.move_block_operation(0, 1, 2, 6, Some(11)).unwrap();
        let DeviceOperation::MoveBlock {
            from_row,
            to_row,
            grid_model_ids,
            ..
        } = operation
        else {
            panic!("unexpected operation")
        };
        assert_eq!((from_row, to_row), (0, 2));
        assert_eq!(grid_model_ids, vec![vec![11, 12], vec![21]]);

        // Full preset replies retain hashless Model placeholders for empty
        // cells. They are not occupied blocks and are not part of the
        // ordered model-ID grid context retained for a move.
        decoder.preset.as_mut().unwrap().chains[0].models.insert(
            1,
            Model {
                hash: Some(model::Hash::Hash(0)),
                column: Some(model::Column::Column(3)),
                ..Default::default()
            },
        );
        let operation = decoder.move_block_operation(0, 1, 0, 3, Some(11)).unwrap();
        let DeviceOperation::MoveBlock { grid_model_ids, .. } = operation else {
            panic!("unexpected operation")
        };
        assert_eq!(grid_model_ids, vec![vec![11, 12], vec![21]]);

        decoder.preset_exact = false;
        assert!(decoder
            .move_block_operation(0, 1, 2, 6, Some(11))
            .unwrap_err()
            .contains("stale"));
    }

    fn layout_model(id: u32, column: u32) -> Model {
        Model {
            hash: Some(model::Hash::Hash(id)),
            column: Some(model::Column::Column(column)),
            ..Default::default()
        }
    }

    fn layout_chain(row: u32, models: Vec<Model>) -> Chain {
        Chain {
            row: Some(chain::Row::Row(row)),
            models,
            ..Default::default()
        }
    }

    fn recalled_layout_decoder() -> StateDecoder {
        let mut decoder = StateDecoder::new();
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                        chains: vec![
                            layout_chain(0, vec![layout_model(11, 1), layout_model(12, 5)]),
                            layout_chain(1, vec![layout_model(21, 3)]),
                        ],
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        decoder
    }

    fn grid_push(action: pa::message_action::Enum, chains: Vec<Chain>) -> Vec<u8> {
        pa::GridMessage {
            action: action as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains,
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn a_parameter_push_after_a_recall_keeps_the_move_layout_exact() {
        let mut decoder = recalled_layout_decoder();
        assert!(decoder.move_block_operation(0, 1, 0, 4, Some(11)).is_ok());

        // The Grid push the device sends after a recall: it names the same
        // blocks at the same cells and carries parameters, not structure.
        let mut parameter_only = layout_model(11, 1);
        parameter_only.params = vec![Param {
            index: Some(param::Index::Index(0)),
            param_values: vec![ParamValue {
                value: Some(param_value::Value::FloatValue(0.25)),
            }],
            ..Default::default()
        }];
        decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &grid_push(
                    pa::message_action::Enum::Update,
                    vec![layout_chain(0, vec![parameter_only])],
                ),
            )
            .unwrap();
        assert!(
            !decoder.preset_exact,
            "a sparse push must still withdraw full-preset exactness"
        );
        assert!(decoder.move_block_operation(0, 1, 0, 4, Some(11)).is_ok());
        assert!(decoder
            .move_grid_layout_diagnostic()
            .starts_with("exact=true"));
    }

    #[test]
    fn a_structural_push_stales_the_layout_until_the_next_full_preset() {
        for pushed in [
            // a block added at an empty cell
            vec![layout_chain(0, vec![layout_model(99, 6)])],
            // the observed sparse echo can omit model identity entirely; a
            // hashless entry at an empty coordinate still cannot be merged.
            vec![layout_chain(
                0,
                vec![Model {
                    column: Some(model::Column::Column(6)),
                    ..Default::default()
                }],
            )],
            // a different block at an occupied cell
            vec![layout_chain(0, vec![layout_model(99, 1)])],
            // repeated sparse coordinates are ambiguous even if both hashes
            // happen to agree with the retained cell.
            vec![layout_chain(
                0,
                vec![layout_model(11, 1), layout_model(11, 1)],
            )],
            // a block with no explicit row
            vec![Chain {
                models: vec![layout_model(11, 1)],
                ..Default::default()
            }],
            // a block with no explicit column
            vec![layout_chain(
                0,
                vec![Model {
                    hash: Some(model::Hash::Hash(11)),
                    ..Default::default()
                }],
            )],
        ] {
            let mut decoder = recalled_layout_decoder();
            let sparse_updates = decoder
                .decode(
                    profile::MESSAGE_TYPE_GRID,
                    &grid_push(pa::message_action::Enum::Update, pushed),
                )
                .unwrap();
            assert_eq!(sparse_updates[0].kind, "gridTopology");
            assert_eq!(sparse_updates[0].grid_layout_complete, Some(false));
            assert!(decoder
                .move_block_operation(0, 1, 0, 4, Some(11))
                .unwrap_err()
                .contains("stale"));
            // A following parameter-only push cannot launder the layout.
            decoder
                .decode(
                    profile::MESSAGE_TYPE_GRID,
                    &grid_push(
                        pa::message_action::Enum::Update,
                        vec![layout_chain(0, vec![layout_model(11, 1)])],
                    ),
                )
                .unwrap();
            assert!(decoder.move_block_operation(0, 1, 0, 4, Some(11)).is_err());
            // The next authoritative full preset re-establishes it.
            let full_updates = decoder
                .decode(
                    profile::MESSAGE_TYPE_RECALL_PRESET,
                    &pa::RecallPresetMessage {
                        action: pa::message_action::Enum::Update as i32,
                        preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                            chains: vec![layout_chain(
                                0,
                                vec![layout_model(11, 1), layout_model(99, 6)],
                            )],
                            ..Default::default()
                        })),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            assert!(full_updates.iter().any(|update| {
                update.kind == "preset" && update.grid_layout_complete == Some(true)
            }));
            assert!(decoder.preset_exact);
            assert!(decoder.move_block_operation(0, 1, 0, 4, Some(11)).is_ok());
        }
    }

    #[test]
    fn a_hashless_parameter_echo_at_one_known_occupied_coordinate_keeps_layout_exact() {
        let mut decoder = recalled_layout_decoder();
        let updates = decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &grid_push(
                    pa::message_action::Enum::Update,
                    vec![layout_chain(
                        0,
                        vec![Model {
                            column: Some(model::Column::Column(1)),
                            params: vec![Param {
                                param_values: vec![ParamValue {
                                    value: Some(param_value::Value::FloatValue(0.5)),
                                }],
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                    )],
                ),
            )
            .unwrap();
        assert!(updates.iter().all(|update| update.kind != "gridTopology"));
        assert!(decoder.grid_layout_exact);
    }

    #[test]
    fn a_sparse_update_cannot_use_an_ambiguous_retained_coordinate() {
        let retained = BinaryPreset {
            chains: vec![layout_chain(
                0,
                vec![layout_model(11, 1), layout_model(12, 1)],
            )],
            ..Default::default()
        };
        let delta = BinaryPreset {
            chains: vec![layout_chain(0, vec![layout_model(11, 1)])],
            ..Default::default()
        };
        assert!(!grid_update_preserves_layout(&retained, &delta));
    }

    #[test]
    fn a_grid_delete_keeps_an_exact_layout_and_removes_the_block_from_it() {
        let mut decoder = recalled_layout_decoder();
        decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &grid_push(
                    pa::message_action::Enum::Delete,
                    vec![layout_chain(0, vec![layout_model(12, 5)])],
                ),
            )
            .unwrap();
        assert!(!decoder.preset_exact);
        let operation = decoder.move_block_operation(0, 1, 0, 5, Some(11)).unwrap();
        let DeviceOperation::MoveBlock { grid_model_ids, .. } = operation else {
            panic!("unexpected operation")
        };
        assert_eq!(grid_model_ids, vec![vec![11], vec![21]]);
        assert!(decoder
            .move_block_operation(0, 5, 0, 6, None)
            .unwrap_err()
            .contains("absent"));
    }

    #[test]
    fn an_inbound_move_keeps_the_layout_only_when_it_was_applied() {
        let moved = |from_row| {
            pa::GridMoveMessage {
                r#move: vec![pa::GridMoveElement {
                    from_row,
                    from_col: 1,
                    to_row: 0,
                    to_col: 6,
                    is_drop: true,
                }],
                ..Default::default()
            }
            .encode_to_vec()
        };
        let mut decoder = recalled_layout_decoder();
        decoder
            .decode(profile::MESSAGE_TYPE_GRID_MOVE, &moved(0))
            .unwrap();
        assert!(!decoder.preset_exact);
        let operation = decoder.move_block_operation(0, 6, 0, 7, Some(11)).unwrap();
        let DeviceOperation::MoveBlock { grid_model_ids, .. } = operation else {
            panic!("unexpected operation")
        };
        assert_eq!(grid_model_ids, vec![vec![12, 11], vec![21]]);

        // A move the retained chains cannot resolve proves nothing.
        let mut decoder = recalled_layout_decoder();
        decoder
            .decode(profile::MESSAGE_TYPE_GRID_MOVE, &moved(3))
            .unwrap();
        assert!(decoder
            .move_block_operation(0, 1, 0, 4, Some(11))
            .unwrap_err()
            .contains("stale"));
    }

    #[test]
    fn move_grid_layout_diagnostic_exposes_the_row_index_versus_explicit_row_gap() {
        let model = |id, column| Model {
            hash: Some(model::Hash::Hash(id)),
            column: Some(model::Column::Column(column)),
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        assert_eq!(decoder.move_grid_layout_diagnostic(), "no-retained-preset");
        // A lone block on row 1: the grid context has one row (index 0) while the
        // move says from_row 1. The diagnostic must make that visible.
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(1)),
                models: vec![
                    Model {
                        hash: Some(model::Hash::Hash(0)),
                        column: Some(model::Column::Column(0)),
                        ..Default::default()
                    },
                    model(77, 2),
                ],
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.preset_exact = true;
        assert_eq!(
            decoder.move_grid_layout_diagnostic(),
            "exact=true chain#0:row=1[77@2]"
        );
        let DeviceOperation::MoveBlock {
            from_row,
            grid_model_ids,
            ..
        } = decoder.move_block_operation(1, 2, 1, 4, None).unwrap()
        else {
            panic!("unexpected operation")
        };
        assert_eq!((from_row, grid_model_ids.len()), (1, 1));
    }

    #[test]
    fn move_block_operation_reports_absent_and_ambiguous_source_blocks_by_exact_message() {
        // Regression for the locate_model extraction shared with
        // decode_grid_move: the two chains below both resolve to row 0 (one
        // explicitly, one by array-position fallback) and each has a model
        // at column 0, so (row 0, column 0) is genuinely ambiguous -- while
        // row 0's own two chains sharing a row is, by itself, not an error
        // here (contrast capture_retarget_operation's locate_chain, which
        // fails closed on that alone). Preserve both exact message texts.
        let model_at = |id, column| Model {
            hash: Some(model::Hash::Hash(id)),
            column: Some(model::Column::Column(column)),
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![
                Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![model_at(11, 0)],
                    ..Default::default()
                },
                Chain {
                    // No explicit row: falls back to array index 1, so this
                    // alone does not collide with the row above.
                    models: vec![model_at(21, 5)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        decoder.preset_exact = true;

        assert_eq!(
            decoder.move_block_operation(0, 3, 1, 0, None).unwrap_err(),
            "The source block is absent; refresh and retry"
        );

        decoder.preset.as_mut().unwrap().chains[1].row = Some(chain::Row::Row(0));
        decoder.preset.as_mut().unwrap().chains[1].models[0].column =
            Some(model::Column::Column(0));
        assert_eq!(
            decoder.move_block_operation(0, 0, 1, 0, None).unwrap_err(),
            "The source block is ambiguous; refresh and retry"
        );
    }

    #[test]
    fn capture_retarget_operation_reports_absent_and_ambiguous_row_or_model_by_exact_message() {
        // Regression for the locate_chain extraction: an ambiguous *row*
        // alone (two chains both resolving to row 0) must fail closed before
        // even looking at columns, unlike move_block_operation's combined
        // row+column locate_model above.
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(14_000)),
                    column: Some(model::Column::Column(1)),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.preset_exact = true;

        assert_eq!(
            decoder
                .capture_retarget_operation(1, 1, 14_000, "key")
                .unwrap_err(),
            "Current preset has no row 1"
        );
        assert_eq!(
            decoder
                .capture_retarget_operation(0, 4, 14_000, "key")
                .unwrap_err(),
            "Current preset has no model at row 0, column 4"
        );

        decoder.preset.as_mut().unwrap().chains.push(Chain {
            row: Some(chain::Row::Row(0)),
            models: vec![Model {
                hash: Some(model::Hash::Hash(14_001)),
                column: Some(model::Column::Column(2)),
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(
            decoder
                .capture_retarget_operation(0, 1, 14_000, "key")
                .unwrap_err(),
            "Current preset has ambiguous row 0",
            "an ambiguous row fails closed even though only one chain has a model at column 1"
        );
    }

    #[test]
    fn single_block_and_lane_control_parameter_detail_fail_closed_on_an_ambiguous_row() {
        // Regression for the locate_chain conversion: both narrow
        // write-verification accessors used to resolve the chain with a
        // plain `.find()`, which silently picked the first of two chains
        // reporting the same row instead of failing the way
        // `capture_retarget_operation`/`locate_chain` already do. Once two
        // chains share row 0, both accessors must report `None` rather than
        // returning a parameter detail from the wrong block.
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![
                Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![Model {
                        hash: Some(model::Hash::Hash(14_000)),
                        column: Some(model::Column::Column(1)),
                        params: vec![Param {
                            index: Some(param::Index::Index(0)),
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::FloatValue(0.5)),
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    input_control: vec![Model {
                        params: vec![Param {
                            index: Some(param::Index::Index(0)),
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::FloatValue(0.5)),
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![Model {
                        hash: Some(model::Hash::Hash(14_001)),
                        column: Some(model::Column::Column(1)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        assert!(
            decoder.single_block_parameter_detail(0, 1, 0).is_none(),
            "an ambiguous row must fail closed instead of picking the first matching chain"
        );
        assert!(
            decoder
                .single_lane_control_parameter_detail(0, "inputGate", 0)
                .is_none(),
            "an ambiguous row must fail closed instead of picking the first matching chain"
        );
    }

    #[test]
    fn grid_move_ambiguous_batches_never_fabricate_exact_retained_layout() {
        let seed = || {
            let mut decoder = StateDecoder::new();
            decoder.preset = Some(BinaryPreset {
                chains: vec![Chain {
                    row: Some(chain::Row::Row(0)),
                    models: [77, 88]
                        .into_iter()
                        .enumerate()
                        .map(|(column, id)| Model {
                            hash: Some(model::Hash::Hash(id)),
                            column: Some(model::Column::Column(column as u32)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            });
            decoder.preset_exact = true;
            decoder.grid_layout_exact = true;
            decoder
        };
        let movement = |from_col, to_col| pa::GridMoveElement {
            from_row: 0,
            from_col,
            to_row: 0,
            to_col,
            is_drop: true,
        };
        for moves in [
            vec![movement(0, 2), movement(0, 3)],
            vec![movement(0, 2), movement(1, 2)],
            vec![movement(0, 1)],
        ] {
            let mut decoder = seed();
            let before = decoder.preset.clone();
            let updates = decoder
                .decode(
                    profile::MESSAGE_TYPE_GRID_MOVE,
                    &pa::GridMoveMessage {
                        r#move: moves,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].kind, "gridTopology");
            assert_eq!(updates[0].grid_layout_complete, Some(false));
            assert!(
                updates[0].blocks.is_none(),
                "ambiguous batch cannot publish invented blocks"
            );
            assert_eq!(
                decoder.preset, before,
                "all retained bytes survive unknown topology"
            );
            assert!(!decoder.preset_exact);
            assert!(!decoder.grid_layout_exact);
            assert!(decoder.move_block_operation(0, 0, 0, 4, Some(77)).is_err());
        }
        for moves in [
            vec![movement(0, 2)],
            vec![movement(0, 2), movement(1, 3)],
            vec![movement(0, 1), movement(1, 0)],
        ] {
            let mut decoder = seed();
            let expected: Vec<_> = moves
                .iter()
                .map(|item| (item.to_col, if item.from_col == 0 { 77 } else { 88 }))
                .collect();
            let updates = decoder
                .decode(
                    profile::MESSAGE_TYPE_GRID_MOVE,
                    &pa::GridMoveMessage {
                        r#move: moves,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            assert_eq!(updates.len(), 1);
            assert!(!decoder.preset_exact);
            assert!(decoder.grid_layout_exact);
            let models = &decoder.preset.as_ref().unwrap().chains[0].models;
            assert_eq!(models.len(), 2, "moves preserve model count");
            for (column, id) in expected {
                assert_eq!(
                    models
                        .iter()
                        .filter(|model| {
                            model_column(model, u32::MAX) == column && model_hash(model) == Some(id)
                        })
                        .count(),
                    1
                );
            }
        }
    }

    #[test]
    fn grid_move_preserves_implicit_survivor_coordinates_and_consumes_empty_destination() {
        let mut decoder = StateDecoder::new();
        // Actual full-preset compatibility shape: array positions address
        // cells and a hash0 Model retains an empty destination.
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                models: [77, 88, 0]
                    .into_iter()
                    .map(|id| Model {
                        hash: Some(model::Hash::Hash(id)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.preset_exact = true;
        decoder.grid_layout_exact = true;
        let updates = decoder
            .decode(
                profile::MESSAGE_TYPE_GRID_MOVE,
                &pa::GridMoveMessage {
                    r#move: vec![pa::GridMoveElement {
                        from_row: 0,
                        from_col: 0,
                        to_row: 0,
                        to_col: 2,
                        is_drop: true,
                    }],
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert!(decoder.grid_layout_exact);
        let models = &decoder.preset.as_ref().unwrap().chains[0].models;
        assert_eq!(
            models
                .iter()
                .enumerate()
                .map(|(index, model)| { (model_hash(model), model_column(model, index as u32)) })
                .collect::<Vec<_>>(),
            [(Some(88), 1), (Some(77), 2)],
            "source removal must not shift the unmoved block or retain a duplicate empty cell",
        );
        assert!(decoder.move_block_operation(0, 1, 0, 3, Some(88)).is_ok());
        assert!(decoder.move_block_operation(0, 0, 0, 3, Some(77)).is_err());
        let blocks = updates[0].blocks.as_ref().unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(blocks
            .iter()
            .any(|block| block.column == 1 && block.model_id == Some(88)));
        assert!(blocks
            .iter()
            .any(|block| block.column == 2 && block.model_id == Some(77)));
    }
    #[test]
    fn grid_move_receiver_reconciles_cross_row_coordinates() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![
                Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![Model {
                        hash: Some(model::Hash::Hash(77)),
                        column: Some(model::Column::Column(1)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Chain {
                    row: Some(chain::Row::Row(2)),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        decoder.preset_exact = true;
        let updates = decoder
            .decode(
                profile::MESSAGE_TYPE_GRID_MOVE,
                &pa::GridMoveMessage {
                    r#move: vec![pa::GridMoveElement {
                        from_row: 0,
                        from_col: 1,
                        to_row: 2,
                        to_col: 6,
                        is_drop: true,
                    }],
                    request_id: Some(pa::grid_move_message::RequestId::RequestId(991)),
                    grid: Some(pa::grid_move_message::Grid::Grid(pa::GridMoveGrid {
                        rows: vec![pa::GridMoveRow {
                            model_ids: vec![999],
                        }],
                    })),
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert!(!decoder.preset_exact);
        let preset = decoder.preset.as_ref().unwrap();
        assert!(preset.chains[0].models.is_empty());
        assert_eq!(model_hash(&preset.chains[1].models[0]), Some(77));
        assert_eq!(model_column(&preset.chains[1].models[0], 0), 6);
    }

    #[test]
    fn grid_move_receiver_rejects_coordinates_before_mutating_state() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(77)),
                    column: Some(model::Column::Column(1)),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.preset_exact = true;
        let before = decoder.preset.clone();
        let result = decoder.decode(
            profile::MESSAGE_TYPE_GRID_MOVE,
            &pa::GridMoveMessage {
                r#move: vec![pa::GridMoveElement {
                    from_row: 0,
                    from_col: 1,
                    to_row: domain::GRID_ROWS,
                    to_col: 2,
                    is_drop: true,
                }],
                ..Default::default()
            }
            .encode_to_vec(),
        );
        assert!(matches!(
            result,
            Err(StateDecodeError::InvalidGridCoordinate { .. })
        ));
        assert_eq!(decoder.preset, before);
        assert!(decoder.preset_exact);
    }

    #[test]
    fn grid_move_receiver_resolves_every_destination_before_mutating_state() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(77)),
                    column: Some(model::Column::Column(1)),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.preset_exact = true;
        let before = decoder.preset.clone();
        let updates = decoder
            .decode(
                profile::MESSAGE_TYPE_GRID_MOVE,
                &pa::GridMoveMessage {
                    r#move: vec![pa::GridMoveElement {
                        from_row: 0,
                        from_col: 1,
                        to_row: 2,
                        to_col: 6,
                        is_drop: true,
                    }],
                    request_id: Some(pa::grid_move_message::RequestId::RequestId(991)),
                    grid: Some(pa::grid_move_message::Grid::Grid(pa::GridMoveGrid {
                        rows: vec![pa::GridMoveRow {
                            model_ids: vec![999],
                        }],
                    })),
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].kind, "gridTopology");
        assert_eq!(updates[0].grid_layout_complete, Some(false));
        assert!(updates[0].blocks.is_none());
        assert_eq!(decoder.preset, before);
        assert!(!decoder.preset_exact);
    }

    #[test]
    fn grid_move_receiver_rejects_an_ambiguous_destination_row() {
        // Regression: the destination lookup used `.find()` with no check for
        // a second chain reporting the same row, unlike the source lookup two
        // lines above, which already fails closed
        // (`if matches.next().is_some() { return Ok(Vec::new()); }`) on an
        // ambiguous source row. Two chains claiming the same destination row
        // must fail the same way, not silently move into whichever chain
        // `.find()` happened to see first.
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![
                Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![Model {
                        hash: Some(model::Hash::Hash(77)),
                        column: Some(model::Column::Column(1)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Chain {
                    row: Some(chain::Row::Row(2)),
                    ..Default::default()
                },
                Chain {
                    row: Some(chain::Row::Row(2)),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        decoder.preset_exact = true;
        let before = decoder.preset.clone();
        let updates = decoder
            .decode(
                profile::MESSAGE_TYPE_GRID_MOVE,
                &pa::GridMoveMessage {
                    r#move: vec![pa::GridMoveElement {
                        from_row: 0,
                        from_col: 1,
                        to_row: 2,
                        to_col: 6,
                        is_drop: true,
                    }],
                    request_id: Some(pa::grid_move_message::RequestId::RequestId(991)),
                    grid: Some(pa::grid_move_message::Grid::Grid(pa::GridMoveGrid {
                        rows: vec![pa::GridMoveRow {
                            model_ids: vec![999],
                        }],
                    })),
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].kind, "gridTopology");
        assert_eq!(updates[0].grid_layout_complete, Some(false));
        assert!(updates[0].blocks.is_none());
        assert_eq!(
            decoder.preset, before,
            "an ambiguous destination row must not mutate the preset"
        );
        assert!(!decoder.preset_exact);
    }

    #[test]
    fn master_volume_rejects_non_normalized_device_values() {
        for value in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            let result = StateDecoder::new().decode(
                profile::MESSAGE_TYPE_MASTER_VOLUME,
                &pa::MasterVolumeMessage {
                    action: pa::message_action::Enum::Update as i32,
                    volume: Some(pa::master_volume_message::Volume::Volume(value)),
                    ..Default::default()
                }
                .encode_to_vec(),
            );
            assert!(matches!(
                result,
                Err(StateDecodeError::InvalidNormalizedValue {
                    message_type: profile::MESSAGE_TYPE_MASTER_VOLUME,
                    field: "volume",
                    ..
                })
            ));
        }
        for value in [0.0, 0.57, 1.0] {
            let states = StateDecoder::new()
                .decode(
                    profile::MESSAGE_TYPE_MASTER_VOLUME,
                    &pa::MasterVolumeMessage {
                        action: pa::message_action::Enum::Update as i32,
                        volume: Some(pa::master_volume_message::Volume::Volume(value)),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            assert_eq!(states[0].master_volume, Some(value));
        }
    }

    #[test]
    fn capture_retarget_clones_full_model_and_fails_closed_after_sparse_grid_state() {
        let mut decoder = StateDecoder::new();
        decoder
            .decode(
                profile::MESSAGE_TYPE_SCENE,
                &pa::SceneMessage {
                    action: pa::message_action::Enum::Update as i32,
                    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(2)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        let scene_values = (0..8)
            .map(|scene| ParamValue {
                value: Some(param_value::Value::StringValue(format!("old/{scene}"))),
            })
            .collect();
        let full_model = Model {
            hash: Some(model::Hash::Hash(14_001)),
            column: Some(model::Column::Column(3)),
            params: vec![
                Param {
                    index: Some(param::Index::Index(0)),
                    param_values: vec![ParamValue {
                        value: Some(param_value::Value::FloatValue(0.75)),
                    }],
                    ..Default::default()
                },
                Param {
                    index: Some(param::Index::Index(5)),
                    scene_mode: Some(param::SceneMode::SceneMode(true)),
                    param_values: scene_values,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let full_preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(1)),
                models: vec![full_model],
                ..Default::default()
            }],
            // Bypass rows/columns resolve by array position (see
            // capture_retarget_operation's positional bypass lookup), so the
            // live record must sit at index 1/3 -- not merely carry explicit
            // row/column fields of 1/3 -- to be found by row 1, column 3.
            bypass: vec![
                crate::proto::Bypass::default(),
                crate::proto::Bypass {
                    row: Some(bypass::Row::Row(1)),
                    col_bypass: vec![
                        crate::proto::ColBypass::default(),
                        crate::proto::ColBypass::default(),
                        crate::proto::ColBypass::default(),
                        crate::proto::ColBypass {
                            column: Some(col_bypass::Column::Column(3)),
                            scene_mode: Some(col_bypass::SceneMode::SceneMode(true)),
                            scene_bypass: (0..8)
                                .map(|scene| crate::proto::SceneBypass { bypass: scene == 2 })
                                .collect(),
                        },
                    ],
                },
            ],
            ..Default::default()
        };
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(full_preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();

        let operation = decoder
            .capture_retarget_operation(1, 3, 14_001, "capture/new")
            .unwrap();
        let DeviceOperation::RetargetCapture {
            source_revision,
            model,
            bypass: Some(current_bypass),
            ..
        } = &operation
        else {
            panic!("expected full-model retarget")
        };
        assert!(decoder.capture_retarget_revision_is_current(*source_revision));
        assert_eq!(model.params.len(), 2);
        assert_eq!(model.params[1].param_values.len(), 8);
        assert!(matches!(
            model.params[1].param_values[2].value.as_ref(),
            Some(param_value::Value::StringValue(value)) if value == "capture/new"
        ));
        assert!(matches!(
            model.params[1].param_values[1].value.as_ref(),
            Some(param_value::Value::StringValue(value)) if value == "old/1"
        ));
        assert_eq!(current_bypass.scene_bypass.len(), 8);

        let messages = operation.clone().try_encode().unwrap();
        assert_eq!(messages.len(), 1);
        let grid = pa::GridMessage::decode(messages[0].payload.as_slice()).unwrap();
        let pa::grid_message::Preset::Preset(update) = grid.preset.unwrap();
        assert_eq!(update.chains[0].models[0].params.len(), 2);
        assert_eq!(update.bypass[0].col_bypass[0].scene_bypass.len(), 8);

        decoder
            .decode(
                profile::MESSAGE_TYPE_GRID,
                &pa::GridMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                        chains: vec![Chain {
                            row: Some(chain::Row::Row(1)),
                            models: vec![Model {
                                column: Some(model::Column::Column(3)),
                                params: vec![Param {
                                    index: Some(param::Index::Index(0)),
                                    param_values: vec![ParamValue {
                                        value: Some(param_value::Value::FloatValue(0.5)),
                                    }],
                                    ..Default::default()
                                }],
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert!(!decoder.capture_retarget_revision_is_current(*source_revision));
        assert!(decoder
            .capture_retarget_operation(1, 3, 14_001, "capture/again")
            .unwrap_err()
            .contains("stale"));
    }

    #[test]
    fn capture_retarget_resolves_bypass_by_array_position_not_explicit_row_or_column() {
        // Regression: preset_update resolves preset.bypass by array position
        // only (bypass_map(..., positional: true); see
        // normalizes_full_preset_and_live_updates), but
        // capture_retarget_operation used to prefer the explicit row/column
        // field instead when present. Both process the same preset.bypass
        // snapshot, so they could disagree about which record a given
        // row/column addresses. Build a decoy record sitting at position
        // row 0 whose explicit fields *claim* to be row 1/column 3 (the
        // coordinates requested below), and the real row-1/column-3 record
        // at the correct position but with unrelated explicit fields, then
        // confirm the lookup follows position, not the explicit fields.
        let mut decoder = StateDecoder::new();
        let full_model = Model {
            hash: Some(model::Hash::Hash(14_001)),
            column: Some(model::Column::Column(3)),
            params: vec![Param {
                index: Some(param::Index::Index(5)),
                param_values: vec![ParamValue {
                    value: Some(param_value::Value::StringValue("old".into())),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let full_preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(1)),
                models: vec![full_model],
                ..Default::default()
            }],
            bypass: vec![
                // Decoy at array position 0, but its explicit fields claim
                // to be exactly the row/column the lookup below asks for.
                crate::proto::Bypass {
                    row: Some(bypass::Row::Row(1)),
                    col_bypass: vec![crate::proto::ColBypass {
                        column: Some(col_bypass::Column::Column(3)),
                        scene_bypass: vec![crate::proto::SceneBypass { bypass: true }],
                        ..Default::default()
                    }],
                },
                // The real row-1 record, at array index 1, with unrelated
                // explicit row/column fields.
                crate::proto::Bypass {
                    row: Some(bypass::Row::Row(9)),
                    col_bypass: vec![
                        crate::proto::ColBypass::default(),
                        crate::proto::ColBypass::default(),
                        crate::proto::ColBypass::default(),
                        crate::proto::ColBypass {
                            column: Some(col_bypass::Column::Column(7)),
                            scene_bypass: vec![crate::proto::SceneBypass { bypass: false }],
                            ..Default::default()
                        },
                    ],
                },
            ],
            ..Default::default()
        };
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(full_preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();

        let operation = decoder
            .capture_retarget_operation(1, 3, 14_001, "capture/new")
            .unwrap();
        let DeviceOperation::RetargetCapture {
            bypass: Some(bypass),
            ..
        } = &operation
        else {
            panic!("expected a resolved bypass record");
        };
        assert_eq!(bypass.scene_bypass.len(), 1);
        assert!(
            !bypass.scene_bypass[0].bypass,
            "must resolve the row-1/column-3 array position, not the decoy with matching explicit fields"
        );
        assert_eq!(bypass.column, Some(col_bypass::Column::Column(3)));
    }

    #[test]
    fn public_state_decoders_reject_oversized_wire_payloads_before_copying() {
        // Each type is bounded by what the transport can deliver for it: its
        // inflation limit, or the raw frame ceiling, whichever is larger.
        let oversized = |message_type: u16| {
            vec![0_u8; crate::compression::max_delivered_payload_bytes_for(message_type) + 1]
        };
        assert!(matches!(
            StateDecoder::new().decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &oversized(profile::MESSAGE_TYPE_RECALL_PRESET)
            ),
            Err(StateDecodeError::PayloadLimit)
        ));
        assert!(matches!(
            decode_preset_folder(&oversized(profile::MESSAGE_TYPE_FILE)),
            Err(StateDecodeError::PayloadLimit)
        ));
        assert!(matches!(
            parse_model_repo(&oversized(profile::MESSAGE_TYPE_MODEL_REPO)),
            Err(StateDecodeError::PayloadLimit)
        ));
    }

    #[test]
    fn an_inflated_file_listing_past_the_raw_ceiling_is_within_its_type_limit() {
        // A File payload the transport inflated past 1 MiB, but within
        // the File inflation limit, must reach the decoders rather than be
        // rejected against the raw frame ceiling.
        let listing = large_file_listing(profile::MAX_FRAME_BYTES + 1);
        assert!(listing.len() > profile::MAX_FRAME_BYTES);
        assert!(
            listing.len() <= crate::compression::max_inflated_bytes_for(profile::MESSAGE_TYPE_FILE)
        );
        let message =
            decode_file_message(&listing).expect("a listing within the File limit decodes");
        assert!(matches!(
            message.folder,
            Some(pa::file_message::Folder::Folder(ref folder)) if folder.files.len() > 1_000
        ));
        decode_preset_folder(&listing).expect("the projection accepts it too");
        StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_FILE, &listing)
            .expect("the state decoder accepts the listing");
    }

    use crate::proto::{
        binary_preset, chain, col_bypass, model, param, param_value, Bypass, Chain, ColBypass,
        Param, ParamValue, SceneBypass, StompModeAssignment,
    };

    fn numeric_param(index: u32, value: f32) -> Param {
        Param {
            index: Some(param::Index::Index(index)),
            param_values: vec![ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn legacy_splitter_identity_is_projected_and_live_values_use_virtual_column_eight() {
        let legacy = Model {
            hash: Some(model::Hash::Hash(10_001)),
            params: vec![numeric_param(0, 0.25)],
            ..Default::default()
        };
        let chain = Chain {
            row: Some(chain::Row::Row(0)),
            splitter: vec![legacy.clone()],
            split_control_points: vec![crate::proto::SplitControlPoints { split: 2, mix: 6 }],
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![chain],
            ..Default::default()
        });

        let details = decoder.block_details(0, 8).unwrap();
        assert_eq!(details.model_id, 10_001);
        assert_eq!(details.parameters[0].normalized_value, Some(0.25));
        let preset = decoder.preset_update(false).unwrap();
        assert_eq!(preset.routes.unwrap()[0].splitter_model_id, Some(10_001));

        let sparse = pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains: vec![Chain {
                    row: Some(chain::Row::Row(0)),
                    splitter: vec![Model {
                        params: vec![numeric_param(1, 0.75)],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let updates = decoder.decode(profile::MESSAGE_TYPE_GRID, &sparse).unwrap();
        let parameter = updates
            .iter()
            .find(|update| update.kind == "parameter")
            .unwrap();
        assert_eq!((parameter.row, parameter.column), (Some(0), Some(8)));
        assert_eq!(parameter.parameter_index, Some(1));
        assert_eq!(parameter.normalized_value, Some(0.75));

        let ambiguous = pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains: vec![Chain {
                    splitter: vec![Model {
                        params: vec![numeric_param(2, 0.1)],
                        ..Default::default()
                    }],
                    combined_splitter: vec![Model {
                        params: vec![numeric_param(2, 0.9)],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_GRID, &ambiguous)
            .unwrap()
            .iter()
            .all(|update| update.kind != "parameter"));
    }

    #[test]
    fn live_grid_updates_record_model_and_lane_control_parameter_overrides_alike() {
        // Exercises decode_grid's three parameter-override sweeps (chain
        // models, lane input/output controls, and splitter/mixer) through
        // the shared record_parameter_overrides helper, via the real live
        // MESSAGE_TYPE_GRID entry point rather than a full RECALL_PRESET
        // decode -- the two use different code paths, and only this one
        // exercises the sweep this fix refactored.
        let mut decoder = StateDecoder::new();
        let sparse = pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains: vec![Chain {
                    row: Some(chain::Row::Row(1)),
                    models: vec![Model {
                        column: Some(model::Column::Column(2)),
                        params: vec![numeric_param(3, 0.4)],
                        ..Default::default()
                    }],
                    input_control: vec![Model {
                        params: vec![numeric_param(0, 0.35)],
                        ..Default::default()
                    }],
                    output_control: vec![Model {
                        params: vec![numeric_param(1, 0.6)],
                        ..Default::default()
                    }],
                    mixer: vec![Model {
                        params: vec![numeric_param(4, 0.2)],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let updates = decoder.decode(profile::MESSAGE_TYPE_GRID, &sparse).unwrap();

        let model_update = updates
            .iter()
            .find(|update| update.kind == "parameter" && update.column == Some(2))
            .unwrap();
        assert_eq!(model_update.row, Some(1));
        assert_eq!(model_update.parameter_index, Some(3));
        assert_eq!(model_update.normalized_value, Some(0.4));
        assert_eq!(
            decoder.parameter_overrides.get(&(1, 2, 3)),
            Some(&(0.4_f32 as f64))
        );

        let input_update = updates
            .iter()
            .find(|update| update.kind == "laneControlParameter" && update.column == Some(10))
            .unwrap();
        assert_eq!(input_update.row, Some(1));
        assert_eq!(input_update.parameter_index, Some(0));
        assert_eq!(input_update.normalized_value, Some(0.35));
        assert_eq!(
            decoder.parameter_overrides.get(&(1, 10, 0)),
            Some(&(0.35_f32 as f64))
        );

        let output_update = updates
            .iter()
            .find(|update| update.kind == "laneControlParameter" && update.column == Some(11))
            .unwrap();
        assert_eq!(output_update.row, Some(1));
        assert_eq!(output_update.parameter_index, Some(1));
        assert_eq!(output_update.normalized_value, Some(0.6));
        assert_eq!(
            decoder.parameter_overrides.get(&(1, 11, 1)),
            Some(&(0.6_f32 as f64))
        );

        let mixer_update = updates
            .iter()
            .find(|update| update.kind == "parameter" && update.column == Some(9))
            .unwrap();
        assert_eq!(mixer_update.row, Some(1));
        assert_eq!(mixer_update.parameter_index, Some(4));
        assert_eq!(mixer_update.normalized_value, Some(0.2));
        assert_eq!(
            decoder.parameter_overrides.get(&(1, 9, 4)),
            Some(&(0.2_f32 as f64))
        );

        // Out-of-range normalized values must still be dropped by every sweep.
        let out_of_range = pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains: vec![Chain {
                    row: Some(chain::Row::Row(1)),
                    models: vec![Model {
                        column: Some(model::Column::Column(2)),
                        params: vec![numeric_param(5, 1.5)],
                        ..Default::default()
                    }],
                    input_control: vec![Model {
                        params: vec![numeric_param(6, -0.1)],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let updates = decoder
            .decode(profile::MESSAGE_TYPE_GRID, &out_of_range)
            .unwrap();
        assert!(updates
            .iter()
            .all(|update| update.kind != "parameter" && update.kind != "laneControlParameter"));
        assert!(!decoder.parameter_overrides.contains_key(&(1, 2, 5)));
        assert!(!decoder.parameter_overrides.contains_key(&(1, 10, 6)));
    }

    #[test]
    fn combined_splitter_defaults_to_current_id_and_ambiguous_dual_storage_fails_closed() {
        let combined = Model {
            params: vec![numeric_param(0, 0.5)],
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                combined_splitter: vec![combined.clone()],
                split_control_points: vec![crate::proto::SplitControlPoints { split: 2, mix: 6 }],
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(decoder.block_details(0, 8).unwrap().model_id, 10_004);
        assert_eq!(
            decoder.preset_update(false).unwrap().routes.unwrap()[0].splitter_model_id,
            Some(10_004)
        );

        decoder.preset.as_mut().unwrap().chains[0].splitter = vec![Model {
            hash: Some(model::Hash::Hash(10_000)),
            ..Default::default()
        }];
        assert!(decoder.block_details(0, 8).is_none());
        assert_eq!(
            decoder.preset_update(false).unwrap().routes.unwrap()[0].splitter_model_id,
            None
        );
    }

    #[test]
    fn split_and_mix_control_points_are_projected_independently() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![
                Chain {
                    row: Some(chain::Row::Row(0)),
                    split_control_points: vec![crate::proto::SplitControlPoints {
                        split: 2,
                        mix: -1,
                    }],
                    ..Default::default()
                },
                Chain {
                    row: Some(chain::Row::Row(2)),
                    split_control_points: vec![crate::proto::SplitControlPoints {
                        split: -1,
                        mix: 7,
                    }],
                    mixer: vec![Model::default()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let routes = decoder.preset_update(false).unwrap().routes.unwrap();
        assert_eq!(
            (routes[0].split_column, routes[0].mix_column),
            (Some(2), None)
        );
        assert_eq!(
            (routes[1].split_column, routes[1].mix_column),
            (None, Some(7))
        );
        assert!(decoder.block_details(2, 9).is_some());
        assert!(decoder.block_details(2, 8).is_none());
    }

    #[test]
    fn device_initiated_route_change_updates_the_retained_routes() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                in_portid: Some(chain::InPortid::InPortid(1)),
                out_portid: Some(chain::OutPortid::OutPortid(20)),
                ..Default::default()
            }],
            ..Default::default()
        });
        let sparse = |output: u32| {
            pa::GridMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(0)),
                        out_portid: Some(chain::OutPortid::OutPortid(output)),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec()
        };
        let updates = decoder
            .decode(profile::MESSAGE_TYPE_GRID, &sparse(19))
            .unwrap();
        let routes = updates
            .iter()
            .find(|update| update.kind == "preset")
            .and_then(|update| update.routes.clone())
            .expect("a route change must republish the preset routes");
        assert_eq!(routes[0].output_id, Some(19));
        assert_eq!(routes[0].output, "Multi Out");
        assert_eq!(routes[0].input_id, Some(1));
        // The same route again changes nothing, so nothing is republished.
        let again = decoder
            .decode(profile::MESSAGE_TYPE_GRID, &sparse(19))
            .unwrap();
        assert!(again.iter().all(|update| update.kind != "preset"));
        // A row the retained preset does not have is ignored rather than guessed.
        let mut absent = pa::GridMessage::decode(sparse(2).as_slice()).unwrap();
        if let Some(pa::grid_message::Preset::Preset(preset)) = absent.preset.as_mut() {
            preset.chains[0].row = Some(chain::Row::Row(3));
        }
        let ignored = decoder
            .decode(profile::MESSAGE_TYPE_GRID, &absent.encode_to_vec())
            .unwrap();
        assert!(ignored.iter().all(|update| update.kind != "preset"));
    }

    #[test]
    fn normalizes_full_preset_and_live_updates() {
        let mut midi_messages_general_v2 = vec![crate::proto::MidiMessageInfo::default(); 120];
        midi_messages_general_v2[12] = crate::proto::MidiMessageInfo {
            r#type: 1,
            channel: 3,
            param1: 10,
            param2: 64,
            param3: 0,
        };
        // A second message two slots later: the empty slot between them keeps
        // its place in the published list.
        midi_messages_general_v2[14] = crate::proto::MidiMessageInfo {
            r#type: 3,
            channel: 4,
            param1: 0,
            param2: 32,
            param3: 5,
        };
        let preset = BinaryPreset {
            name: Some(binary_preset::Name::Name("Shared Decoder".into())),
            tempo: Some(binary_preset::Tempo::Tempo(99)),
            scene_labels: vec!["Clean".into()],
            scene_colors: vec![0xff00aa],
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                in_portid: Some(chain::InPortid::InPortid(1)),
                out_portid: Some(chain::OutPortid::OutPortid(2)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(1234)),
                    column: Some(model::Column::Column(3)),
                    params: vec![numeric_param(7, 0.25)],
                    bypass_expression: vec![crate::proto::Expression {
                        expression: 2,
                        expression_min: 0.0,
                        expression_max: 1.0,
                    }],
                    expression_bypass_info: vec![crate::proto::ExpressionBypassInfo {
                        r#type: 1,
                        invert: true,
                        delay_ms: 300,
                        latch_emulation: false,
                    }],
                    ..Default::default()
                }],
                input_control: vec![Model {
                    hash: Some(model::Hash::Hash(28_000)),
                    params: vec![numeric_param(0, 0.35)],
                    ..Default::default()
                }],
                output_control: vec![Model {
                    hash: Some(model::Hash::Hash(23_000)),
                    params: vec![numeric_param(1, 0.6)],
                    ..Default::default()
                }],
                mix_bypass: vec![SceneBypass { bypass: true }; 8],
                ..Default::default()
            }],
            // The shape a unit writes in a full preset: the record names no row and no
            // column, so its place in the list (row 0, column 3) is its address.
            bypass: vec![Bypass {
                row: None,
                col_bypass: vec![
                    ColBypass::default(),
                    ColBypass::default(),
                    ColBypass::default(),
                    ColBypass {
                        column: None,
                        scene_mode: Some(col_bypass::SceneMode::SceneMode(false)),
                        scene_bypass: vec![SceneBypass { bypass: true }],
                    },
                ],
            }],
            stomp_mode_assignments: vec![StompModeAssignment {
                row: 0,
                column: 3,
                stomp_index: 4,
                r#type: Some(crate::proto::stomp_mode_assignment::TypeOneOf::Type(
                    crate::proto::stomp_mode_assignment::Type::Secondary as i32,
                )),
            }],
            single_stomp_labels: HashMap::from([(4, "Gate".into())]),
            stomp_is_momentary: HashMap::from([(4, true)]),
            midi_messages_general_v2,
            midi_messages: vec![
                crate::proto::MidiMessageInfo {
                    r#type: 3,
                    channel: 5,
                    param1: 1,
                    param2: 2,
                    param3: 7,
                },
                crate::proto::MidiMessageInfo::default(),
                crate::proto::MidiMessageInfo {
                    r#type: 1,
                    channel: 2,
                    param1: 9,
                    param2: 8,
                    param3: 6,
                },
            ],
            ..Default::default()
        };
        let raw = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
            ..Default::default()
        }
        .encode_to_vec();
        let mut decoder = StateDecoder::new();
        let states = decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &raw)
            .unwrap();
        assert!(
            (decoder
                .lane_control_details(0, "inputGate")
                .unwrap()
                .parameters[0]
                .normalized_value
                .unwrap()
                - 0.35)
                .abs()
                < 1e-6
        );
        assert!(
            (decoder
                .lane_control_details(0, "laneOutput")
                .unwrap()
                .parameters[0]
                .normalized_value
                .unwrap()
                - 0.6)
                .abs()
                < 1e-6
        );
        assert_eq!(states[0].preset_name.as_deref(), Some("Shared Decoder"));
        assert_eq!(states[0].blocks.as_ref().unwrap()[0].column, 3);
        assert_eq!(states[0].blocks.as_ref().unwrap()[0].bypassed, Some(true));
        assert_eq!(
            states[0].blocks.as_ref().unwrap()[0]
                .bypass_address
                .as_deref(),
            Some("positional")
        );
        assert_eq!(
            states[0].blocks.as_ref().unwrap()[0].bypass_scene_mode,
            Some(false)
        );
        let bypass_expression = states[0].blocks.as_ref().unwrap()[0]
            .bypass_expression
            .as_ref()
            .unwrap();
        assert_eq!(bypass_expression.pedal, 2);
        assert_eq!(bypass_expression.mode, 1);
        assert!(bypass_expression.invert);
        assert_eq!(bypass_expression.delay_ms, 300);
        assert_eq!(
            states[0].blocks.as_ref().unwrap()[0].footswitch_order,
            Some(0)
        );
        assert_eq!(
            states[0].blocks.as_ref().unwrap()[0]
                .footswitch_assignment_type
                .as_deref(),
            Some("secondary")
        );
        assert_eq!(states[0].routes.as_ref().unwrap()[0].input, "In 1");
        assert_eq!(
            states[0].routes.as_ref().unwrap()[0].split_muted,
            Some(true)
        );
        let footswitch = &states[0].footswitch_states.as_ref().unwrap()[4];
        assert!(footswitch.assigned);
        assert!(footswitch.momentary.unwrap());
        assert_eq!(footswitch.label.as_deref(), Some("Gate"));
        assert_eq!(states[0].midi_out.as_ref().unwrap()[0].source, 1);
        assert_eq!(
            states[0].midi_out.as_ref().unwrap()[0].messages[0].param1,
            10
        );
        assert_eq!(
            states[0].midi_out.as_ref().unwrap()[0]
                .messages
                .iter()
                .map(|message| message.r#type)
                .collect::<Vec<_>>(),
            vec![1, 0, 3],
            "the empty slot between two active ones keeps its index"
        );
        assert_eq!(
            states[0].preset_load_midi_out.as_ref().unwrap()[0].param3,
            7
        );
        assert_eq!(
            states[0]
                .preset_load_midi_out
                .as_ref()
                .unwrap()
                .iter()
                .map(|message| message.r#type)
                .collect::<Vec<_>>(),
            vec![3, 0, 1]
        );
        assert_eq!(
            decoder.block_details(0, 3).unwrap().parameters[0].normalized_value,
            Some(0.25)
        );

        let scene = pa::SceneMessage {
            action: pa::message_action::Enum::Update as i32,
            selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(6)),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decoder.decode(profile::MESSAGE_TYPE_SCENE, &scene).unwrap()[0].active_scene,
            Some(6)
        );
        let dirty = pa::PresetDirtyMessage {
            action: pa::message_action::Enum::Update as i32,
            is_dirty: true,
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(
            decoder
                .decode(profile::MESSAGE_TYPE_PRESET_DIRTY, &dirty)
                .unwrap()[0]
                .dirty,
            Some(true)
        );
    }

    #[test]
    fn normalized_json_matches_the_shared_typescript_contract() {
        let mode = pa::ModeMessage {
            action: pa::message_action::Enum::Update as i32,
            mode: Some(pa::mode_message::Mode::Mode(6)),
            available_modes: Some(pa::mode_message::AvailableModes::AvailableModes(
                pa::AvailableModes {
                    modes: vec![0, 1, 6],
                },
            )),
            ..Default::default()
        };
        let states = StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_MODE, &mode.encode_to_vec())
            .unwrap();
        let json = serde_json::to_value(&states[0]).unwrap();
        assert_eq!(json["kind"], "mode");
        assert_eq!(json["mode"], "HYBRID");
        assert_eq!(json["modeSlots"][2]["label"], "SCENE / STOMP");
        assert!(json.get("activeScene").is_none());
    }

    #[test]
    fn decodes_physical_io_presence_from_native_io_settings() {
        let payload = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    in_port: vec![pa::InputPortSettings {
                        input_port_id: 1,
                        plugged: Some(pa::input_port_settings::Plugged::Plugged(true)),
                        ..Default::default()
                    }],
                    out_port: vec![pa::OutputPortSettings {
                        output_port_id: 4,
                        plugged: Some(pa::output_port_settings::Plugged::Plugged(false)),
                        ..Default::default()
                    }],
                    hp_port: Some(pa::port_settings::HpPort::HpPort(pa::HeadphonesSettings {
                        plugged: Some(pa::headphones_settings::Plugged::Plugged(true)),
                        ..Default::default()
                    })),
                    usb_port: Some(pa::port_settings::UsbPort::UsbPort(pa::UsbPortSettings {
                        plugged: Some(pa::usb_port_settings::Plugged::Plugged(true)),
                        ..Default::default()
                    })),
                    exp_port: vec![pa::ExpPortSettings {
                        exp_port_id: 1,
                        plugged: Some(pa::exp_port_settings::Plugged::Plugged(false)),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();

        let states = StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_IO_SETTINGS, &payload)
            .unwrap();
        let ports = states[0].io_ports.as_ref().unwrap();
        assert_eq!(states[0].kind, "ioPorts");
        assert!(ports
            .iter()
            .any(|port| port.kind == "input" && port.id == 1 && port.plugged));
        assert!(ports
            .iter()
            .any(|port| port.kind == "output" && port.id == 4 && !port.plugged));
        assert!(ports
            .iter()
            .any(|port| port.kind == "headphones" && port.plugged));
        assert!(ports.iter().any(|port| port.kind == "usb" && port.plugged));
        assert!(ports
            .iter()
            .any(|port| port.kind == "expression" && port.id == 1 && port.label == "EXP 1"));
    }

    #[test]
    fn io_settings_state_ignores_unowned_request_id() {
        for request_id in [0, 7] {
            let payload = pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: Some(pa::io_settings_message::RequestId::RequestId(request_id)),
                settings: Some(pa::io_settings_message::Settings::Settings(
                    pa::PortSettings {
                        in_port: vec![pa::InputPortSettings {
                            input_port_id: 1,
                            plugged: Some(pa::input_port_settings::Plugged::Plugged(true)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
            .encode_to_vec();

            assert!(StateDecoder::new()
                .decode(profile::MESSAGE_TYPE_IO_SETTINGS, &payload)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn expression_port_ids_one_and_two_label_as_the_units_two_jacks() {
        // The unit reports exactly two expression ports, ids 1 and 2, and names
        // its two jacks EXP 1 and EXP 2; a +1 offset printed "EXP 2"/"EXP 3".
        let payload = pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    exp_port: [1, 2]
                        .into_iter()
                        .map(|exp_port_id| pa::ExpPortSettings {
                            exp_port_id,
                            plugged: Some(pa::exp_port_settings::Plugged::Plugged(false)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();

        let states = StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_IO_SETTINGS, &payload)
            .unwrap();
        let labels: Vec<(i32, &str)> = states[0]
            .io_ports
            .as_ref()
            .unwrap()
            .iter()
            .filter(|port| port.kind == "expression")
            .map(|port| (port.id, port.label.as_str()))
            .collect();
        assert_eq!(labels, vec![(1, "EXP 1"), (2, "EXP 2")]);
    }

    #[test]
    fn decodes_preset_folder_pushes_for_the_background_library() {
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Update as i32,
            r#type: Some(pa::file_message::Type::Type(0)),
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into())),
                name: Some(pa::folder_info::Name::Name("Live".into())),
                is_plugin: Some(pa::folder_info::IsPlugin::IsPlugin(true)),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key(
                        "/media/p4/Presets/Live/Direct Rust.pb".into(),
                    )),
                    index: Some(pa::product_data::Index::Index(17)),
                    name: Some(pa::product_data::Name::Name("Direct Rust".into())),
                    author: Some(pa::product_data::Author::Author("Ada".into())),
                    date: Some(pa::product_data::Date::Date("2026-10-03".into())),
                    date_ms_since_epoch: Some(
                        pa::product_data::DateMsSinceEpoch::DateMsSinceEpoch(1_780_358_400_000),
                    ),
                    instrument: Some(pa::product_data::Instrument::Instrument(1)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let folder = decode_preset_folder(&payload).unwrap().unwrap();
        assert_eq!(folder.key, "/media/p4/Presets/Live");
        assert_eq!(folder.file_type, Some(0));
        assert!(folder.is_plugin);
        assert_eq!(folder.files[0].position, 17);
        assert_eq!(folder.files[0].name, "Direct Rust");
        assert_eq!(folder.files[0].instrument, 1);
        assert!(folder.files[0].instrument_present);
        assert_eq!(folder.files[0].author.as_deref(), Some("Ada"));
        assert_eq!(folder.files[0].date.as_deref(), Some("2026-10-03"));
        assert_eq!(folder.files[0].date_ms_since_epoch, Some(1_780_358_400_000));
        assert_eq!(
            folder.files[0].product_key.as_deref(),
            Some("/media/p4/Presets/Live/Direct Rust.pb")
        );
    }

    #[test]
    fn preset_instrument_absence_is_distinct_from_present_zero() {
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into())),
                files: vec![
                    pa::ProductData {
                        index: Some(pa::product_data::Index::Index(1)),
                        ..Default::default()
                    },
                    pa::ProductData {
                        index: Some(pa::product_data::Index::Index(2)),
                        instrument: Some(pa::product_data::Instrument::Instrument(0)),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let folder = decode_preset_folder(&payload).unwrap().unwrap();
        assert_eq!(folder.files[0].instrument, 0);
        assert!(!folder.files[0].instrument_present);
        assert_eq!(folder.files[1].instrument, 0);
        assert!(folder.files[1].instrument_present);
    }

    #[test]
    fn preset_folder_frames_are_independent_sorted_and_slot_bounded() {
        let product = |index: i32, name: &str| pa::ProductData {
            index: Some(pa::product_data::Index::Index(index)),
            name: Some(pa::product_data::Name::Name(name.into())),
            ..Default::default()
        };
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(
                    "/media/p4/Presets/Ordered".into(),
                )),
                // No name: the host derives the final path component.
                files: vec![
                    product(255, "Last"),
                    product(-1, "Negative"),
                    product(4, "Four"),
                    product(256, "Past End"),
                    product(2, "Two"),
                ],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();

        let folder = decode_preset_folder(&payload).unwrap().unwrap();
        assert_eq!(folder.name, "Ordered");
        assert_eq!(
            folder
                .files
                .iter()
                .map(|file| (file.position, file.name.as_str()))
                .collect::<Vec<_>>(),
            [(2, "Two"), (4, "Four"), (255, "Last")]
        );

        let header_only = pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            omit_factory_content: Some(pa::file_message::OmitFactoryContent::OmitFactoryContent(
                false,
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert_eq!(decode_preset_folder(&header_only).unwrap(), None);
        assert!(decode_preset_folder(&[0x22, 0x02, 0xff]).is_err());
    }

    #[test]
    fn compatibility_folder_projection_does_not_assign_action_semantics() {
        for action in [
            pa::message_action::Enum::Create,
            pa::message_action::Enum::Update,
            pa::message_action::Enum::Delete,
            pa::message_action::Enum::Read,
        ] {
            let payload = pa::FileMessage {
                action: action as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key("/media/p4/Presets/Live".into())),
                    files: vec![pa::ProductData {
                        index: Some(pa::product_data::Index::Index(7)),
                        name: Some(pa::product_data::Name::Name("Action fixture".into())),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec();
            assert_eq!(decode_file_message(&payload).unwrap().action, action as i32);
            let folder = decode_preset_folder(&payload).unwrap().unwrap();
            assert_eq!(folder.key, "/media/p4/Presets/Live");
            assert_eq!(folder.files[0].position, 7);
        }
    }

    #[test]
    fn model_repo_metadata_and_clone_replacements_drive_block_details() {
        let xml = br#"<Models><Category name="Drive">
          <Model id="100" name="Base Drive">
            <Parameter name="GAIN" type="float" min="0" max="10" displayPos="1" />
            <Parameter name="DUMMY SLOT" type="empty" hidden="true" />
            <Parameter name="RATE" type="float" min="0.1" max="10" skew="2" units="Hz" displayPos="3" min_string="Slow" max_string="Fast" expAssignable="false" linkedSceneMode="4" toggleOn="0" />
          </Model>
          <Model id="101" name="Child Drive" clones="100">
            <Parameter name="MODE" type="rotarySwitch" min="0" max="1" steps="2" stepNames="Off,On" displayPos="4" replaces="0" />
          </Model>
        </Category></Models>"#;
        let repo = pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: None,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                xml.to_vec(),
            )),
        };
        let catalog = parse_model_repo(&repo.encode_to_vec()).unwrap();
        assert_eq!(catalog.0.len(), 2);
        assert_eq!(catalog.0[&101].parameters[&0].name, "MODE");
        assert_eq!(catalog.0[&101].parameters[&0].options, ["Off", "On"]);

        let preset = BinaryPreset {
            name: Some(binary_preset::Name::Name("Catalog Test".into())),
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(101)),
                    column: Some(model::Column::Column(2)),
                    params: vec![
                        Param {
                            index: Some(param::Index::Index(0)),
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::StringValue("On".into())),
                            }],
                            ..Default::default()
                        },
                        numeric_param(1, 0.0),
                        Param {
                            index: Some(param::Index::Index(2)),
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::FloatValue(0.5)),
                            }],
                            expression: Some(param::Expression::Expression(2)),
                            expression_min: Some(param::ExpressionMin::ExpressionMin(0.1)),
                            expression_max: Some(param::ExpressionMax::ExpressionMax(0.9)),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(catalog);
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        let details = decoder.block_details(0, 2).unwrap();
        assert_eq!(details.name, "Child Drive");
        assert_eq!(details.category, "Drive");
        assert_eq!(details.parameters.len(), 2);
        assert_eq!(details.parameters[0].display_position, 4);
        assert_eq!(details.parameters[0].normalized_value, Some(1.0));
        assert_eq!(details.parameters[0].wire_value_kind, "text");
        let rate = &details.parameters[1];
        assert_eq!(rate.value_scale, "power");
        assert_eq!(rate.scale_exponent, Some(0.5));
        assert_eq!(rate.display_precision, Some(2));
        assert_eq!(rate.display_value, "7.10");
        assert!(!rate.expression_assignable);
        assert_eq!(rate.linked_scene_mode, Some(4));
        assert_eq!(rate.expression, Some(2));
        assert_eq!(rate.expression_minimum, Some(0.1));
        assert_eq!(rate.expression_maximum, Some(0.9));
    }

    fn parse_test_catalog(xml: &[u8]) -> ModelCatalog {
        parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn block_details_omit_protobuf_slots_the_model_never_declares() {
        let catalog = parse_test_catalog(
            br#"<Models><Category name="Wah">
              <Model id="400" name="Wah">
                <Parameter name="WAH" type="float" min="0" max="1" displayPos="0" />
              </Model>
            </Category><Category name="Cabsim Guitar (ST)">
              <Model id="401" name="Cab">
                <Parameter name="bypass" type="switch" min="0" max="1" steps="2" />
                <Parameter name="ir selector" type="string" min="0" max="999" steps="1000" />
              </Model>
            </Category></Models>"#,
        );
        let string_param = |index: u32, value: &str| Param {
            index: Some(param::Index::Index(index)),
            param_values: vec![ParamValue {
                value: Some(param_value::Value::StringValue(value.into())),
            }],
            ..Default::default()
        };
        let preset = BinaryPreset {
            name: Some(binary_preset::Name::Name("Undeclared".into())),
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![
                    Model {
                        hash: Some(model::Hash::Hash(400)),
                        column: Some(model::Column::Column(0)),
                        params: vec![numeric_param(0, 0.41), numeric_param(1, 0.0)],
                        ..Default::default()
                    },
                    Model {
                        hash: Some(model::Hash::Hash(401)),
                        column: Some(model::Column::Column(1)),
                        params: vec![
                            numeric_param(0, 0.0),
                            string_param(1, "412 Brit TV GB75Hz 69_Dynamic 57"),
                        ],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(catalog);
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        let wah = decoder.block_details(0, 0).unwrap();
        assert_eq!(
            wah.parameters
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>(),
            [0],
            "slot 1 is not declared by the model, so it is not a control"
        );
        assert_eq!(wah.parameters[0].display_value, "0.41");
        let cab = decoder.block_details(0, 1).unwrap();
        let microphone = cab.parameters.iter().find(|item| item.index == 1).unwrap();
        assert_eq!(microphone.options, ["412 Brit TV GB75Hz 69_Dynamic 57"]);
        assert_eq!(microphone.normalized_value, Some(0.0));
    }

    #[test]
    fn labelled_end_controls_read_as_a_magnitude_and_a_side() {
        let catalog = parse_test_catalog(
            br#"<Models><Category name="Cabsim Guitar (ST)">
              <Model id="300" name="Cab">
                <Parameter name="BALANCE" type="float" min="-1" max="1" skew="1" min_string="L" mid_string="C" max_string="R" />
              </Model>
            </Category></Models>"#,
        );
        let spec = catalog.0[&300].parameters.get(&0);
        let label = |normalized: f64| {
            labelled_end_value_label(normalized_to_real(normalized, spec), spec).unwrap()
        };
        assert_eq!(label(0.35), "15 L");
        assert_eq!(label(0.5), "C");
        assert_eq!(label(0.65), "15 R");
        assert_eq!(label(0.99), "49 R");
        // The extremes keep the bare label the unit prints there.
        assert_eq!(special_value_label(0.0, spec).as_deref(), Some("L"));
        assert_eq!(special_value_label(1.0, spec).as_deref(), Some("R"));
    }

    #[test]
    fn cab_level_takes_its_top_from_the_skew_that_centres_zero_db() {
        let catalog = parse_test_catalog(
            br#"<Models><Category name="Cabsim Guitar (ST)">
              <Model id="301" name="Cab">
                <Parameter name="LEVEL" type="float" min="-40" max="12" skew="4.95967" units="dB" min_string="OFF" />
              </Model>
            </Category><Category name="Guitar Amplifier">
              <Model id="302" name="Amp">
                <Parameter name="OUTPUT" type="float" min="-60" max="12" skew="3.80189" units="dB" min_string="OFF" />
              </Model>
            </Category></Models>"#,
        );
        let level = &catalog.0[&301].parameters[&0];
        assert_eq!(level.maximum, 6.0);
        assert!(
            normalized_to_real(0.5, Some(level)).abs() < 0.01,
            "the default reads 0.0 dB"
        );
        let output = &catalog.0[&302].parameters[&0];
        assert_eq!(output.maximum, 12.0, "an amp already centres zero at 12");
    }

    #[test]
    fn a_bare_zero_to_one_amount_reads_to_two_places() {
        let catalog = parse_test_catalog(
            br#"<Models><Category name="Wah">
              <Model id="303" name="Wah">
                <Parameter name="WAH" type="float" min="0" max="1" />
                <Parameter name="POSITION" type="float" min="0" max="1" />
                <Parameter name="DRIVE" type="float" min="0" max="10" />
              </Model>
            </Category></Models>"#,
        );
        let parameters = &catalog.0[&303].parameters;
        assert_eq!(parameters[&0].display_precision(), Some(2));
        assert_eq!(parameters[&1].display_precision(), Some(1));
        assert_eq!(parameters[&2].display_precision(), Some(1));
        assert_eq!(
            format_parameter_number(0.41, parameters[&0].display_precision()),
            "0.41"
        );
    }

    #[test]
    fn labelled_linear_pan_spans_use_the_fixed_span_not_catalog_numbers() {
        let xml = br#"<Models><Category name="Utility">
          <Model id="200" name="Labelled pans">
            <Parameter name="MONO PAN" type="float" min="0" max="10" min_string="L" mid_string="C" max_string="R" />
            <Parameter name="STEREO BALANCE" type="float" min="-1" max="1" skew="1" min_string="L" mid_string="C" max_string="R" />
            <Parameter name="OFF LEVEL" type="float" min="-40" max="12" min_string="OFF" />
            <Parameter name="INTEGER OFF" type="float" min="20" max="500" min_string="OFF" showAsInteger="true" />
            <Parameter name="LOG POWER" type="float" min="100" max="10000" skew="LOG_SKEW" min_string="OFF" showAsInteger="true" />
            <Parameter name="UNKNOWN TRIM" type="float" min="MIN_INPUT_TRIM" max="MAX_INPUT_TRIM" min_string="OFF" />
            <Parameter name="EXEMPT TAPER" type="float" min="0" max="1" skew="2" min_string="L" mid_string="C" max_string="R" />
            <Parameter name="A/B BALANCE" type="float" min="0" max="10" />
          </Model>
        </Category></Models>"#;
        let catalog = parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
        let parameters = &catalog.0[&200].parameters;

        for index in [0, 1] {
            let spec = &parameters[&index];
            assert_eq!((spec.minimum, spec.maximum), LABELLED_END_SPAN);
            assert_eq!(normalized_to_real(0.0, Some(spec)), -50.0);
            assert_eq!(normalized_to_real(0.5, Some(spec)), 0.0);
            assert_eq!(normalized_to_real(0.75, Some(spec)), 25.0);
            assert_eq!(normalized_to_real(1.0, Some(spec)), 50.0);
        }
        assert_eq!(
            (parameters[&2].minimum, parameters[&2].maximum),
            (-40.0, 12.0),
            "a lone OFF label is not a pan"
        );
        assert_eq!(
            (parameters[&6].minimum, parameters[&6].maximum),
            (0.0, 1.0),
            "a labelled taper remains on its own catalog law"
        );
        assert_eq!(
            (parameters[&7].minimum, parameters[&7].maximum),
            (0.0, 10.0),
            "an unlabeled A/B crossfade is not a pan"
        );
        let off = &parameters[&2];
        assert!(off.has_off_position());
        assert_eq!(off.floor_display(), Some(-39.99));
        assert!((off.floor_normalized().unwrap() - (0.01 / 52.0)).abs() < 1e-12);
        assert!(
            !parameters[&0].has_off_position(),
            "a pan midpoint is not Off"
        );
        assert_eq!(parameters[&0].floor_display(), None);

        let integer = &parameters[&3];
        assert_eq!(integer.floor_display(), Some(21.0));
        assert!((integer.floor_normalized().unwrap() - (1.0 / 480.0)).abs() < 1e-12);

        let log_power = &parameters[&4];
        assert_eq!(log_power.value_scale, "power");
        assert!((log_power.scale_exponent.unwrap() - (1.0 / 0.3)).abs() < 1e-12);
        assert_eq!(log_power.floor_display(), Some(101.0));
        assert!(
            (log_power.floor_normalized().unwrap() - (1.0_f64 / 9900.0).powf(0.3)).abs() < 1e-12
        );

        let unknown = &parameters[&5];
        assert!(!unknown.scale_known);
        assert_eq!(unknown.value_scale, "unknown");
        assert_eq!(unknown.floor_display(), None);
    }

    #[test]
    fn catalog_visibility_uses_true_values_instead_of_attribute_presence() {
        let xml = br#"<Models>
          <Category name="Visible" hidden="false">
            <Model id="1129" name="Ordinary" hidden="false" internal="false">
              <Parameter name="VISIBLE" type="float" hidden="false" />
              <Parameter name="MINI ONLY" type="switch" hidden="atma" />
              <Parameter name="QC HIDDEN" type="switch" hidden="true" />
            </Model>
          </Category>
          <Category name="Hidden" hidden="true">
            <Model id="1130" name="Hidden by category" />
          </Category>
          <Category name="Visible too">
            <Model id="1131" name="Hidden model" hidden="true" />
            <Model id="1132" name="Internal model" internal="true" />
          </Category>
        </Models>"#;
        let catalog = parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();

        assert!(!catalog.0[&1129].hidden);
        assert!(!catalog.0[&1129].parameters[&0].hidden);
        assert!(!catalog.0[&1129].parameters[&1].hidden);
        assert!(catalog.0[&1129].parameters[&2].hidden);
        assert!(catalog.0[&1130].hidden);
        assert!(catalog.0[&1131].hidden);
        assert!(catalog.0[&1132].hidden);
        assert_eq!(
            catalog
                .model_list()
                .models
                .iter()
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            [1129]
        );
    }

    #[test]
    fn model_list_and_audit_order_are_deterministic() {
        let xml = br#"<Models>
          <Category name="Reverb"><Model id="20" name="Zulu"><Parameter name="Z" type="unknownZ" /></Model></Category>
          <Category name="Delay"><Model id="10" name="Alpha"><Parameter name="A" type="unknownA" /></Model></Category>
          <Category name="Delay"><Model id="11" name="alpha"><Parameter name="OK" type="float" min="0" max="1" /></Model></Category>
        </Models>"#;
        let catalog = parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
        let list = catalog.model_list();
        assert_eq!(
            list.models.iter().map(|model| model.id).collect::<Vec<_>>(),
            [10, 11, 20]
        );
        assert_eq!(
            list.audit
                .exceptions
                .iter()
                .map(|exception| exception.model_id)
                .collect::<Vec<_>>(),
            [10, 20]
        );
    }

    #[test]
    fn block_details_projects_exact_block_splitter_mixer_and_value_oneofs() {
        let xml = br#"<Models><Category name="Drive">
          <Model id="700" name="Audit Drive">
            <Parameter name="GAIN" type="float" min="0" max="10" units="dB" displayPos="2" />
            <Parameter name="MODE" type="rotarySwitch" min="0" max="1" stepNames="Clean,Hot" displayPos="1" />
            <Parameter name="LEGACY INTEGER" type="int" min="0" max="10" displayPos="3" />
          </Model>
        </Category></Models>"#;
        let catalog = parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
        let parameter = |index, values, scene_mode| Param {
            index: Some(param::Index::Index(index)),
            param_values: values,
            scene_mode: Some(param::SceneMode::SceneMode(scene_mode)),
            ..Default::default()
        };
        let float = |value| ParamValue {
            value: Some(param_value::Value::FloatValue(value)),
        };
        let text = |value: &str| ParamValue {
            value: Some(param_value::Value::StringValue(value.into())),
        };
        let integer = |value| ParamValue {
            value: Some(param_value::Value::IntValue(value)),
        };
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(catalog);
        decoder.active_scene = 1;
        let preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(2)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(700)),
                    column: Some(model::Column::Column(4)),
                    params: vec![
                        parameter(0, vec![float(0.2), float(0.8)], true),
                        parameter(1, vec![text("Hot")], false),
                        parameter(2, vec![integer(7)], false),
                    ],
                    ..Default::default()
                }],
                combined_splitter: vec![Model {
                    params: vec![parameter(0, vec![float(0.5)], false)],
                    ..Default::default()
                }],
                mixer: vec![Model {
                    params: vec![parameter(1, vec![float(0.25)], false)],
                    ..Default::default()
                }],
                split_control_points: vec![crate::proto::SplitControlPoints { split: 3, mix: 7 }],
                ..Default::default()
            }],
            ..Default::default()
        };
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();

        let block = decoder.block_details(2, 4).unwrap();
        assert_eq!(
            (block.model_id, block.name.as_str(), block.category.as_str()),
            (700, "Audit Drive", "Drive")
        );
        assert_eq!(block.scene, 1);
        assert_eq!(
            block.parameters.len(),
            2,
            "unsupported integer oneofs fail closed"
        );
        assert!((block.parameters[0].normalized_value.unwrap() - 0.8).abs() < 1e-6);
        assert_eq!(block.parameters[0].display_value, "8.0");
        assert_eq!(block.parameters[0].display_position, 2);
        assert_eq!(block.parameters[0].wire_value_kind, "numeric");
        assert_eq!(block.parameters[1].index, 1);
        assert_eq!(block.parameters[1].options, ["Clean", "Hot"]);
        assert_eq!(block.parameters[1].normalized_value, Some(1.0));
        assert_eq!(block.parameters[1].display_value, "Hot");
        assert_eq!(block.parameters[1].wire_value_kind, "text");

        let splitter = decoder.block_details(2, 8).unwrap();
        assert_eq!(splitter.model_id, 10_004);
        assert_eq!(splitter.parameters[0].normalized_value, Some(0.5));
        let mixer = decoder.block_details(2, 9).unwrap();
        assert_eq!(mixer.model_id, 11_000);
        assert_eq!(mixer.parameters[0].normalized_value, Some(0.25));
        assert!(decoder.block_details(2, 3).is_none());
        assert!(decoder.block_details(4, 4).is_none());

        let ignored = pa::GridMessage {
            action: pa::message_action::Enum::Read as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset::default())),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_GRID, &ignored)
            .unwrap()
            .is_empty());
        assert!(
            (decoder.block_details(2, 4).unwrap().parameters[0]
                .normalized_value
                .unwrap()
                - 0.8)
                .abs()
                < 1e-6
        );
        assert!(matches!(
            decoder.decode(profile::MESSAGE_TYPE_GRID, &[0xff, 0xff]),
            Err(StateDecodeError::Protobuf { .. })
        ));
    }

    #[test]
    fn single_parameter_target_detail_matches_the_full_projection_for_the_matched_parameter() {
        // `single_parameter_target_detail` exists purely as a performance
        // shortcut for `advance_gateway_verification`'s per-tick polling: it
        // must never diverge from what `block_details`/`lane_control_details`
        // would have returned for the same (row, column, index), whether the
        // block is a plain model, a splitter, a mixer, or a lane control, and
        // it must report `None` exactly where the full projection would have
        // filtered the parameter out or the block doesn't exist.
        let xml = br#"<Models><Category name="Drive">
          <Model id="700" name="Audit Drive">
            <Parameter name="GAIN" type="float" min="0" max="10" units="dB" displayPos="2" />
            <Parameter name="MODE" type="rotarySwitch" min="0" max="1" stepNames="Clean,Hot" displayPos="1" />
            <Parameter name="LEGACY INTEGER" type="int" min="0" max="10" displayPos="3" />
          </Model>
        </Category></Models>"#;
        let catalog = parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
        let parameter = |index, values, scene_mode| Param {
            index: Some(param::Index::Index(index)),
            param_values: values,
            scene_mode: Some(param::SceneMode::SceneMode(scene_mode)),
            ..Default::default()
        };
        let float = |value| ParamValue {
            value: Some(param_value::Value::FloatValue(value)),
        };
        let text = |value: &str| ParamValue {
            value: Some(param_value::Value::StringValue(value.into())),
        };
        let integer = |value| ParamValue {
            value: Some(param_value::Value::IntValue(value)),
        };
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(catalog);
        decoder.active_scene = 1;
        let preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(2)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(700)),
                    column: Some(model::Column::Column(4)),
                    params: vec![
                        parameter(0, vec![float(0.2), float(0.8)], true),
                        parameter(1, vec![text("Hot")], false),
                        parameter(2, vec![integer(7)], false),
                    ],
                    ..Default::default()
                }],
                combined_splitter: vec![Model {
                    params: vec![parameter(0, vec![float(0.5)], false)],
                    ..Default::default()
                }],
                mixer: vec![Model {
                    params: vec![parameter(1, vec![float(0.25)], false)],
                    ..Default::default()
                }],
                split_control_points: vec![crate::proto::SplitControlPoints { split: 3, mix: 7 }],
                ..Default::default()
            }],
            ..Default::default()
        };
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();

        // Plain block: both the visible float and the visible enum parameter
        // must match their full-projection counterpart exactly.
        let block = decoder.block_details(2, 4).unwrap();
        // Guard against a fixture regression silently making this loop a no-op.
        assert!(!block.parameters.is_empty());
        for expected in &block.parameters {
            assert_eq!(
                decoder
                    .single_parameter_target_detail(2, 4, expected.index)
                    .as_ref(),
                Some(expected),
                "index {} mismatched the full block projection",
                expected.index
            );
        }
        // The unsupported integer oneof (index 2) is filtered out of the full
        // projection and must be filtered out here too, not silently exposed.
        assert!(decoder.single_parameter_target_detail(2, 4, 2).is_none());

        // Splitter and mixer routing nodes go through a different lookup path
        // in both the full and narrow accessors; both must agree here too.
        let splitter = decoder.block_details(2, 8).unwrap();
        assert_eq!(
            decoder.single_parameter_target_detail(2, 8, 0).as_ref(),
            Some(&splitter.parameters[0])
        );
        let mixer = decoder.block_details(2, 9).unwrap();
        assert_eq!(
            decoder.single_parameter_target_detail(2, 9, 1).as_ref(),
            Some(&mixer.parameters[0])
        );

        // Nonexistent block/row and out-of-range index all report None,
        // exactly as `.find()` over the full (possibly absent) list would.
        assert!(decoder.single_parameter_target_detail(2, 3, 0).is_none());
        assert!(decoder.single_parameter_target_detail(4, 4, 0).is_none());
        assert!(decoder.single_parameter_target_detail(2, 4, 99).is_none());
    }

    #[test]
    fn every_endpoint_publishes_one_category_vocabulary_for_the_same_model() {
        // Regression: the preset snapshot published the raw ModelRepo category
        // ("Guitar Amplifier", "Cabsim Guitar (ST)") while block_details and
        // list_models published the curated display vocabulary ("Guitar Amp",
        // "Guitar Cab"). The same block therefore reported two different
        // categories depending on which endpoint was asked, so anything
        // filtering blocks by category across endpoints silently missed.
        let xml = br#"<Models><Category name="Guitar Amplifier">
          <Model id="200" name="Test Amp">
            <Parameter name="GAIN" type="float" min="0" max="10" displayPos="1" />
          </Model>
        </Category>
        <Category name="Cabsim Guitar (ST)">
          <Model id="201" name="Test Cab">
            <Parameter name="LEVEL" type="float" min="0" max="10" displayPos="1" />
          </Model>
        </Category></Models>"#;
        let repo = pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: None,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                xml.to_vec(),
            )),
        };
        let catalog = parse_model_repo(&repo.encode_to_vec()).unwrap();
        let preset = BinaryPreset {
            name: Some(binary_preset::Name::Name("Category Test".into())),
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![
                    Model {
                        hash: Some(model::Hash::Hash(200)),
                        column: Some(model::Column::Column(0)),
                        params: vec![numeric_param(0, 0.5)],
                        ..Default::default()
                    },
                    Model {
                        hash: Some(model::Hash::Hash(201)),
                        column: Some(model::Column::Column(1)),
                        params: vec![numeric_param(0, 0.5)],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(catalog);
        let updates = decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        let blocks = updates
            .iter()
            .rev()
            .find_map(|update| update.blocks.clone())
            .expect("preset snapshot blocks");
        let models = decoder.model_list();
        for (model_id, column, expected) in [(200u32, 0u32, "Guitar Amp"), (201, 1, "Guitar Cab")] {
            let snapshot = blocks
                .iter()
                .find(|block| block.column == column)
                .expect("snapshot block");
            let details = decoder.block_details(0, column).expect("block details");
            let listed = models
                .models
                .iter()
                .find(|entry| entry.id == model_id)
                .expect("listed model");
            assert_eq!(snapshot.category.as_deref(), Some(expected));
            assert_eq!(details.category, expected);
            assert_eq!(listed.category, expected);
        }
    }

    #[test]
    fn hidden_lane_catalog_models_expose_documented_controls_but_not_unknown_wire_values() {
        let xml = br#"<Models><Category name="InputGateControl" hidden="true">
          <Model id="28000" name="Input Gate Control">
            <Parameter name="NOISE REDUCTION" type="float" min="0" max="100" units="%" steps="1001" />
            <Parameter name="BYPASS" type="switch" min="0" max="1" steps="2" />
            <Parameter name="GAIN REDUCTION" type="grMeter" min="-40" max="0" units="dB" steps="180" />
            <Parameter name="INPUT GAIN" type="float" min="-24" max="24" units="dB" steps="481" />
          </Model>
        </Category></Models>"#;
        let catalog = parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: None,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
            }
            .encode_to_vec(),
        )
        .unwrap();
        let positional = |value| Param {
            param_values: vec![ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        };
        let preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                input_control: vec![Model {
                    hash: Some(model::Hash::Hash(28_000)),
                    // Full preset records address these parameters by position.
                    // CorOS also carries an undocumented fifth value.
                    params: vec![
                        Param {
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::FloatValue(0.3)),
                            }],
                            scene_mode: Some(param::SceneMode::SceneMode(true)),
                            expression: Some(param::Expression::Expression(2)),
                            expression_min: Some(param::ExpressionMin::ExpressionMin(0.1)),
                            expression_max: Some(param::ExpressionMax::ExpressionMax(0.9)),
                            ..Default::default()
                        },
                        Param {
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::StringValue("On".into())),
                            }],
                            dynamic_steps: vec!["Off".into(), "On".into()],
                            ..Default::default()
                        },
                        positional(0.0),
                        positional(0.5),
                        Param {
                            param_values: vec![ParamValue {
                                value: Some(param_value::Value::IntValue(1)),
                            }],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(catalog);
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();

        let details = decoder.lane_control_details(0, "inputGate").unwrap();
        assert_eq!(details.name, "Input Gate Control");
        assert_eq!(
            details
                .parameters
                .iter()
                .map(|parameter| parameter.index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(details.parameters[0].name, "NOISE REDUCTION");
        assert_eq!(details.parameters[0].display_value, "30.0");
        assert!(details.parameters[0].writable);
        assert!(details.parameters[0].scene_mode);
        assert_eq!(details.parameters[0].expression, Some(2));
        assert_eq!(details.parameters[0].expression_minimum, Some(0.1));
        assert_eq!(details.parameters[0].expression_maximum, Some(0.9));
        assert_eq!(details.parameters[1].normalized_value, Some(1.0));
        assert_eq!(details.parameters[1].options, ["Off", "On"]);
        assert!(!details.parameters[2].writable, "GAIN REDUCTION is a meter");
        assert_eq!(details.parameters[3].display_value, "0.0");
    }

    #[test]
    fn live_state_dispatch_matches_expected_action_gates() {
        let create = pa::message_action::Enum::Create as i32;
        let ignored = [
            (
                profile::MESSAGE_TYPE_IO_SETTINGS,
                pa::IoSettingsMessage {
                    action: create,
                    settings: Some(pa::io_settings_message::Settings::Settings(
                        Default::default(),
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_SCENE,
                pa::SceneMessage {
                    action: create,
                    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(3)),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_SETLIST_POSITION,
                pa::SetlistPositionMessage {
                    action: create,
                    position: Some(pa::setlist_position_message::Position::Position(4)),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_RECALL_PRESET,
                pa::RecallPresetMessage {
                    action: create,
                    preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_GRID,
                pa::GridMessage {
                    action: create,
                    preset: Some(pa::grid_message::Preset::Preset(Default::default())),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_MODE,
                pa::ModeMessage {
                    action: create,
                    mode: Some(pa::mode_message::Mode::Mode(2)),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_MASTER_VOLUME,
                pa::MasterVolumeMessage {
                    action: create,
                    volume: Some(pa::master_volume_message::Volume::Volume(0.5)),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_PRESET_DIRTY,
                pa::PresetDirtyMessage {
                    action: create,
                    is_dirty: true,
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_SCENE_LABEL,
                pa::SceneLabelMessage {
                    action: create,
                    index: 1,
                    label: "ignored".into(),
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_SCENE_COLOR,
                pa::SceneColorMessage {
                    action: create,
                    index: 1,
                    color: 0xff00ff,
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
            (
                profile::MESSAGE_TYPE_GLOBAL_TEMPO,
                pa::GlobalTempoMessage {
                    action: create,
                    params: vec![numeric_param(0, 0.5)],
                    ..Default::default()
                }
                .encode_to_vec(),
            ),
        ];
        let mut decoder = StateDecoder::new();
        for (message_type, payload) in ignored {
            assert!(
                decoder.decode(message_type, &payload).unwrap().is_empty(),
                "message type {message_type} accepted CREATE"
            );
        }

        let correlated_tempo = pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: Some(pa::global_tempo_message::RequestId::RequestId(9)),
            params: vec![numeric_param(0, 0.5), numeric_param(2, 1.0)],
            ..Default::default()
        }
        .encode_to_vec();
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_GLOBAL_TEMPO, &correlated_tempo)
            .unwrap()
            .is_empty());

        let delete = pa::GridMessage {
            action: pa::message_action::Enum::Delete as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains: vec![Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![Model {
                        column: Some(model::Column::Column(1)),
                        params: vec![numeric_param(0, 0.25)],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_GRID, &delete)
            .unwrap()
            .is_empty());

        let wrong_repo_action = pa::ModelRepoMessage {
            action: create,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(matches!(
            parse_model_repo(&wrong_repo_action),
            Err(StateDecodeError::UnexpectedAction {
                message_type: profile::MESSAGE_TYPE_MODEL_REPO,
                action
            }) if action == create
        ));
    }

    #[test]
    fn scene_color_receiver_rejects_out_of_range_scene_index() {
        let payload = pa::SceneColorMessage {
            action: pa::message_action::Enum::Update as i32,
            index: domain::SCENE_COUNT as i32,
            color: 0xffff02c2,
            ..Default::default()
        }
        .encode_to_vec();
        assert!(StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_SCENE_COLOR, &payload)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn midi_out_readback_drops_empty_and_malformed_slots() {
        assert!(midi_message(&crate::proto::MidiMessageInfo {
            r#type: 0,
            channel: 16,
            param1: 127,
            param2: 127,
            param3: 127,
        })
        .is_none());
        assert!(midi_message(&crate::proto::MidiMessageInfo {
            r#type: 2,
            channel: 17,
            ..Default::default()
        })
        .is_none());
        assert_eq!(
            midi_message(&crate::proto::MidiMessageInfo {
                r#type: 3,
                channel: 16,
                param1: 127,
                param2: 127,
                param3: 127,
            })
            .unwrap()
            .r#type,
            3
        );
    }

    fn info(kind: u32, param1: u32) -> crate::proto::MidiMessageInfo {
        if kind == 0 {
            return Default::default();
        }
        crate::proto::MidiMessageInfo {
            r#type: kind,
            channel: 6,
            param1,
            param2: 1,
            param3: 2,
        }
    }

    #[test]
    fn split_mute_requires_the_exact_active_scene_record() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                mix_bypass: vec![SceneBypass { bypass: true }; 2],
                ..Default::default()
            }],
            ..Default::default()
        });
        decoder.active_scene = 1;
        assert_eq!(
            decoder.preset_update(false).unwrap().routes.unwrap()[0].split_muted,
            Some(true)
        );
        decoder.active_scene = 7;
        assert_eq!(
            decoder.preset_update(false).unwrap().routes.unwrap()[0].split_muted,
            None
        );
        decoder.preset.as_mut().unwrap().chains[0]
            .mix_bypass
            .clear();
        decoder.active_scene = 0;
        assert_eq!(
            decoder.preset_update(false).unwrap().routes.unwrap()[0].split_muted,
            None
        );
        decoder.preset.as_mut().unwrap().chains[0].mix_bypass =
            vec![SceneBypass { bypass: false }; 8];
        decoder.active_scene = 7;
        assert_eq!(
            decoder.preset_update(false).unwrap().routes.unwrap()[0].split_muted,
            Some(false)
        );
    }

    #[test]
    fn split_mute_scene_ingest_updates_snapshot_without_replacing_routing() {
        let mut decoder = StateDecoder::new();
        decoder.grid_layout_exact = true;
        decoder.preset = Some(BinaryPreset {
            chains: vec![Chain {
                mix_bypass: vec![SceneBypass { bypass: false }, SceneBypass { bypass: true }],
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut snapshot = crate::device::GatewaySnapshot::default();
        snapshot.apply(&decoder.preset_update(false).unwrap());
        assert_eq!(snapshot.routes[0].split_muted, Some(false));
        snapshot.routes[0].input = "Retained topology label".into();
        for (scene, expected) in [(1, Some(true)), (7, None), (0, Some(false))] {
            let updates = decoder
                .decode(
                    profile::MESSAGE_TYPE_SCENE,
                    &pa::SceneMessage {
                        action: pa::message_action::Enum::Update as i32,
                        selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(
                            scene,
                        )),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            for update in updates {
                snapshot.apply(&update);
            }
            assert_eq!(snapshot.active_scene, scene);
            assert_eq!(snapshot.routes[0].split_muted, expected);
            assert_eq!(snapshot.routes[0].input, "Retained topology label");
        }
        decoder.grid_layout_exact = false;
        snapshot.apply(
            &decoder
                .decode_scene(
                    &pa::SceneMessage {
                        action: pa::message_action::Enum::Update as i32,
                        selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(1)),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap()
                .remove(0),
        );
        assert_eq!(
            snapshot.routes[0].split_muted, None,
            "incomplete topology cannot witness a mute operand"
        );
    }

    fn preset_update_from(preset: BinaryPreset) -> StateUpdate {
        let mut decoder = StateDecoder::new();
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap()
            .remove(0)
    }

    #[test]
    fn sparse_metadata_survives_reprojection_and_only_valid_full_replacement_clears_it() {
        let mut decoder = StateDecoder::new();
        let full = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                scene_labels: vec!["Original".into(); 8],
                scene_colors: vec![0xff000000; 8],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &full)
            .unwrap();
        let source = decoder.preset_source_snapshot().unwrap();
        decoder
            .decode(
                profile::MESSAGE_TYPE_SCENE_LABEL,
                &pa::SceneLabelMessage {
                    action: pa::message_action::Enum::Update as i32,
                    index: 3,
                    label: "Observed".into(),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        decoder
            .decode(
                profile::MESSAGE_TYPE_SCENE_COLOR,
                &pa::SceneColorMessage {
                    action: pa::message_action::Enum::Update as i32,
                    index: 3,
                    color: 0xff123456,
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        decoder
            .decode(
                profile::MESSAGE_TYPE_GLOBAL_TEMPO,
                &pa::GlobalTempoMessage {
                    action: pa::message_action::Enum::Update as i32,
                    params: vec![numeric_param(2, 1.0)],
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert!(!decoder.preset_source_matches(&source));
        assert!(decoder.preset_source_snapshot().is_none());
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &[0xff])
            .is_err());
        for catalog in [false, true] {
            let update = decoder.preset_update(catalog).unwrap();
            assert_eq!(update.scene_labels_raw.unwrap()[3], "Observed");
            assert_eq!(update.scene_colors_argb.unwrap()[3], 0xff123456);
            assert_eq!(update.tempo_led_enabled, Some(true));
        }
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &full)
            .unwrap();
        let update = decoder.preset_update(false).unwrap();
        assert_eq!(update.scene_labels_raw.unwrap()[3], "Original");
        assert_eq!(update.scene_colors_argb.unwrap()[3], 0xff000000);
        assert_eq!(update.tempo_led_enabled, None);
        assert!(decoder.preset_source_snapshot().is_some());
    }

    #[test]
    fn sparse_metadata_midi_merges_sources_and_refuses_unowned_or_invalid_batches_atomically() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset::default());
        decoder.preset_exact = true;
        let mut snapshot = crate::device::GatewaySnapshot::default();
        let frame = |source, messages| pa::MidiSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            general_midi_messages: Some(
                pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(
                    pa::GeneralMidiMessages {
                        messages: vec![pa::GeneralMidiMessage {
                            source: Some(pa::general_midi_message::Source::Source(source)),
                            msg: messages,
                        }],
                    },
                ),
            ),
            ..Default::default()
        };
        for source in [1, 4] {
            let updates = decoder
                .decode(
                    profile::MESSAGE_TYPE_MIDI_SETTINGS,
                    &frame(source, vec![info(1, source)]).encode_to_vec(),
                )
                .unwrap();
            snapshot.apply(&updates[0]);
        }
        assert_eq!(
            snapshot
                .midi_out
                .as_ref()
                .unwrap()
                .iter()
                .map(|s| s.source)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
        let clear = decoder
            .decode(
                profile::MESSAGE_TYPE_MIDI_SETTINGS,
                &frame(1, Vec::new()).encode_to_vec(),
            )
            .unwrap();
        snapshot.apply(&clear[0]);
        assert_eq!(
            snapshot
                .midi_out
                .as_ref()
                .unwrap()
                .iter()
                .map(|s| s.source)
                .collect::<Vec<_>>(),
            vec![4]
        );
        assert_eq!(
            decoder.preset_update(false).unwrap().midi_out.unwrap(),
            snapshot.midi_out.clone().unwrap()
        );
        let revision = decoder.state_revision;
        let mut tagged = frame(4, vec![info(1, 99)]);
        tagged.request_id = Some(pa::midi_settings_message::RequestId::RequestId(7));
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_MIDI_SETTINGS, &tagged.encode_to_vec())
            .unwrap()
            .is_empty());
        tagged.request_id = None;
        tagged.action = pa::message_action::Enum::Read as i32;
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_MIDI_SETTINGS, &tagged.encode_to_vec())
            .unwrap()
            .is_empty());
        assert!(decoder
            .decode(
                profile::MESSAGE_TYPE_MIDI_SETTINGS,
                &frame(10, vec![info(1, 9)]).encode_to_vec()
            )
            .is_err());
        assert!(decoder
            .decode(
                profile::MESSAGE_TYPE_MIDI_SETTINGS,
                &frame(4, vec![info(1, 9); 13]).encode_to_vec()
            )
            .is_err());
        assert_eq!(decoder.state_revision, revision);
        assert_eq!(
            decoder.preset_update(false).unwrap().midi_out.unwrap()[0].messages[0].param1,
            4
        );
    }

    #[test]
    fn sparse_metadata_scene_copy_withdraws_source_and_requests_explicit_topology_repair() {
        let mut decoder = StateDecoder::new();
        decoder.preset = Some(BinaryPreset {
            chains: vec![one_block_chain(0, 0)],
            ..Default::default()
        });
        decoder.preset_exact = true;
        decoder.grid_layout_exact = true;
        let source = decoder.preset_source_snapshot().unwrap();
        let message = pa::SceneCopyMessage {
            action: pa::message_action::Enum::Update as i32,
            from_index: 0,
            to_index: 7,
            ..Default::default()
        };
        let mut invalid = message;
        invalid.to_index = 8;
        assert!(decoder
            .decode(profile::MESSAGE_TYPE_SCENE_COPY, &invalid.encode_to_vec())
            .unwrap()
            .is_empty());
        assert!(decoder.preset_source_matches(&source));
        let update = decoder
            .decode(profile::MESSAGE_TYPE_SCENE_COPY, &message.encode_to_vec())
            .unwrap()
            .remove(0);
        assert_eq!(update.kind, "gridTopology");
        assert_eq!(update.grid_layout_complete, Some(false));
        assert_eq!(update.refresh_required, Some(true));
        assert!(!decoder.preset_source_matches(&source));
        assert!(decoder.block_details(0, 0).is_none());
        let mut snapshot = crate::device::GatewaySnapshot {
            grid_layout_complete: true,
            ..Default::default()
        };
        snapshot.apply(&update);
        assert!(!snapshot.grid_layout_complete);
    }

    #[test]
    fn sparse_scene_parameter_transition_revokes_source_until_real_full_refresh() {
        for column in [0, 8, 9, 10, 11] {
            let model = |a: f32| Model {
                hash: Some(model::Hash::Hash(1234)),
                column: Some(model::Column::Column(0)),
                params: vec![Param {
                    scene_mode: Some(param::SceneMode::SceneMode(true)),
                    param_values: (0..8)
                        .map(|scene| crate::proto::ParamValue {
                            value: Some(param_value::Value::FloatValue(if scene == 0 {
                                a
                            } else {
                                0.8
                            })),
                        })
                        .collect(),
                    ..numeric_param(0, a)
                }],
                ..Default::default()
            };
            let chain = |model: Model| Chain {
                row: Some(chain::Row::Row(0)),
                models: if column == 0 {
                    vec![model.clone()]
                } else {
                    Vec::new()
                },
                splitter: if column == 8 {
                    vec![model.clone()]
                } else {
                    Vec::new()
                },
                mixer: if column == 9 {
                    vec![model.clone()]
                } else {
                    Vec::new()
                },
                split_control_points: if matches!(column, 8 | 9) {
                    vec![crate::proto::SplitControlPoints { split: 2, mix: 7 }]
                } else {
                    Vec::new()
                },
                input_control: if column == 10 {
                    vec![model.clone()]
                } else {
                    Vec::new()
                },
                output_control: if column == 11 {
                    vec![model]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let full = |a| {
                pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                        chains: vec![chain(model(a))],
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec()
            };
            let mut runtime = crate::device::state_runtime::DeviceStateRuntime::new();
            runtime
                .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &full(0.2))
                .unwrap();
            let detail = |state: &crate::device::state_runtime::DeviceStateRuntime| {
                if column < 10 {
                    state.block_details(0, column)
                } else {
                    state.lane_control_details(
                        0,
                        if column == 10 {
                            "inputGate"
                        } else {
                            "laneOutput"
                        },
                    )
                }
            };
            assert!(
                (detail(&runtime).unwrap().parameters[0]
                    .normalized_value
                    .unwrap()
                    - 0.2)
                    .abs()
                    < 0.00001
            );
            let sparse = Model {
                params: vec![numeric_param(0, 0.4)],
                ..model(0.2)
            };
            runtime
                .ingest(
                    profile::MESSAGE_TYPE_GRID,
                    &pa::GridMessage {
                        action: pa::message_action::Enum::Update as i32,
                        preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                            chains: vec![chain(sparse)],
                            ..Default::default()
                        })),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            assert!(
                (detail(&runtime).unwrap().parameters[0]
                    .normalized_value
                    .unwrap()
                    - 0.4)
                    .abs()
                    < 0.00001
            );
            let scene = |scene| {
                pa::SceneMessage {
                    action: pa::message_action::Enum::Update as i32,
                    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(scene)),
                    ..Default::default()
                }
                .encode_to_vec()
            };
            runtime
                .ingest(profile::MESSAGE_TYPE_SCENE, &scene(7))
                .unwrap();
            assert!(
                detail(&runtime).is_none(),
                "stale scalar is unavailable before full refresh, target {column}"
            );
            assert!(!runtime.snapshot().has_preset);
            assert!(runtime.snapshot().preset_identity.is_none());
            runtime
                .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &full(0.4))
                .unwrap();
            assert!(
                (detail(&runtime).unwrap().parameters[0]
                    .normalized_value
                    .unwrap()
                    - 0.8)
                    .abs()
                    < 0.00001
            );
            // A sparse publication may carry a complete scene vector. Its
            // active H value, not element A, owns both UI and verification.
            // An omitted mode inherits the witnessed target's scene mode;
            // singleton and non-scene vectors retain compatibility semantics.
            for (count, held_mode, incoming_mode, expected) in [
                (8, true, Some(true), Some(0.9_f64)),
                (8, true, None, Some(0.9)),
                (1, true, Some(true), Some(0.3)),
                (2, true, Some(true), None),
                (8, false, Some(false), Some(0.3)),
            ] {
                let mut held = model(0.2);
                held.params[0].scene_mode = Some(param::SceneMode::SceneMode(held_mode));
                runtime
                    .ingest(
                        profile::MESSAGE_TYPE_RECALL_PRESET,
                        &pa::RecallPresetMessage {
                            action: pa::message_action::Enum::Update as i32,
                            preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                                chains: vec![chain(held)],
                                ..Default::default()
                            })),
                            ..Default::default()
                        }
                        .encode_to_vec(),
                    )
                    .unwrap();
                let mut incoming = model(0.3);
                incoming.params[0].scene_mode = incoming_mode.map(param::SceneMode::SceneMode);
                incoming.params[0].param_values = (0..count)
                    .map(|scene| crate::proto::ParamValue {
                        value: Some(param_value::Value::FloatValue(if scene == 7 {
                            0.9
                        } else {
                            0.3
                        })),
                    })
                    .collect();
                let observed = runtime
                    .ingest(
                        profile::MESSAGE_TYPE_GRID,
                        &pa::GridMessage {
                            action: pa::message_action::Enum::Update as i32,
                            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                                chains: vec![chain(incoming)],
                                ..Default::default()
                            })),
                            ..Default::default()
                        }
                        .encode_to_vec(),
                    )
                    .unwrap();
                if let Some(expected) = expected {
                    let current = detail(&runtime).unwrap().parameters[0]
                        .normalized_value
                        .unwrap();
                    assert!(
                        (current - expected).abs() < 0.00001,
                        "target {column}, count {count}, mode {incoming_mode:?}"
                    );
                    assert!(observed.states.iter().any(|state| state
                        .normalized_value
                        .is_some_and(|value| (f64::from(value) - expected).abs() < 0.00001)));
                    let predicate = crate::device::request::GatewayVerification::Parameter {
                        row: 0,
                        column,
                        parameter_index: 0,
                        value: expected,
                        routing_model_id: None,
                    };
                    assert!(predicate.matches(
                        runtime.snapshot(),
                        detail(&runtime)
                            .as_ref()
                            .and_then(|details| details.parameters.first())
                    ));
                } else {
                    assert!(
                        detail(&runtime).is_none(),
                        "absent H cannot be replaced by A at target {column}"
                    );
                    assert!(!runtime.snapshot().has_preset);
                    assert!(observed
                        .states
                        .iter()
                        .any(|state| state.refresh_required == Some(true)));
                }
            }
            for case in [
                "different-model",
                "lost-layout",
                "invalid-active-float",
                "nonfinite-active-float",
            ] {
                runtime
                    .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &full(0.4))
                    .unwrap();
                let grid = |incoming| {
                    pa::GridMessage {
                        action: pa::message_action::Enum::Update as i32,
                        preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                            chains: vec![chain(incoming)],
                            ..Default::default()
                        })),
                        ..Default::default()
                    }
                    .encode_to_vec()
                };
                if case == "lost-layout" {
                    // A refused batch withdraws topology for every target,
                    // including controls outside the eight block columns.
                    runtime
                        .ingest(
                            profile::MESSAGE_TYPE_GRID_MOVE,
                            &pa::GridMoveMessage {
                                r#move: [2, 3]
                                    .into_iter()
                                    .map(|to_col| pa::GridMoveElement {
                                        from_row: 0,
                                        from_col: 0,
                                        to_row: 0,
                                        to_col,
                                        is_drop: true,
                                    })
                                    .collect(),
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        )
                        .unwrap();
                    assert!(!runtime.snapshot().grid_layout_complete);
                }
                let mut incoming = model(0.3);
                if matches!(case, "different-model" | "lost-layout") {
                    incoming.params[0].scene_mode = None;
                    if case == "different-model" {
                        incoming.hash = Some(model::Hash::Hash(5678));
                    }
                } else {
                    incoming.params[0].param_values[7].value = Some(
                        param_value::Value::FloatValue(if case == "nonfinite-active-float" {
                            f32::NAN
                        } else {
                            -0.1
                        }),
                    );
                }
                let observed = runtime
                    .ingest(profile::MESSAGE_TYPE_GRID, &grid(incoming))
                    .unwrap();
                assert!(
                    detail(&runtime).is_none(),
                    "{case}: unknown active authority at target {column}"
                );
                assert!(!runtime.snapshot().has_preset);
                assert!(observed
                    .states
                    .iter()
                    .any(|state| state.refresh_required == Some(true)));
                assert!(observed
                    .states
                    .iter()
                    .all(|state| state.normalized_value.is_none()));
            }
            runtime
                .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &full(0.4))
                .unwrap();
            runtime
                .ingest(profile::MESSAGE_TYPE_SCENE, &scene(0))
                .unwrap();
            assert!(
                (detail(&runtime).unwrap().parameters[0]
                    .normalized_value
                    .unwrap()
                    - 0.4)
                    .abs()
                    < 0.00001,
                "return must not resurrect old raw A"
            );
            let mut mode_only = numeric_param(0, 0.4);
            mode_only.param_values.clear();
            mode_only.scene_mode = Some(param::SceneMode::SceneMode(false));
            runtime
                .ingest(
                    profile::MESSAGE_TYPE_GRID,
                    &pa::GridMessage {
                        action: pa::message_action::Enum::Update as i32,
                        preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                            chains: vec![chain(Model {
                                params: vec![mode_only],
                                ..model(0.4)
                            })],
                            ..Default::default()
                        })),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
            assert!(
                !runtime.snapshot().has_preset,
                "mode-only transition revokes even without a scalar override"
            );
            assert!(detail(&runtime).is_none());
        }
    }

    fn one_block_chain(row: u32, column: u32) -> Chain {
        Chain {
            row: Some(chain::Row::Row(row)),
            models: vec![Model {
                hash: Some(model::Hash::Hash(1234)),
                column: Some(model::Column::Column(column)),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn midi_out_readback_keeps_slot_positions_and_trims_only_trailing_empties() {
        // Observed on a CorOS 4.1.0 unit, 2026-10-07: the unit keeps a type 0 row
        // between active rows in place, in the live grid and in a saved preset.
        let slots = vec![
            info(1, 10),
            info(0, 0),
            info(3, 30),
            info(0, 0),
            info(2, 50),
            info(0, 0),
            info(0, 0),
            info(0, 0),
            info(0, 0),
            info(0, 0),
            info(0, 0),
            info(0, 0),
        ];
        let list = midi_slot_list(slots.iter());
        let types = list
            .iter()
            .map(|message| message.r#type)
            .collect::<Vec<_>>();
        assert_eq!(
            types,
            vec![1, 0, 3, 0, 2],
            "interior empties stay, trailing ones go"
        );
        assert_eq!(list[1], empty_midi_slot());
        assert_eq!((list[2].r#type, list[2].param1), (3, 30));
        // A malformed slot (an undefined type) is read as an empty one, in place.
        let malformed = [info(1, 1), info(4, 9), info(2, 2)];
        let list = midi_slot_list(malformed.iter());
        assert_eq!(
            list.iter().map(|m| m.r#type).collect::<Vec<_>>(),
            vec![1, 0, 2]
        );
        // An all-empty list is the empty list, and leading empties stay.
        assert!(midi_slot_list(vec![info(0, 0); 12].iter()).is_empty());
        let list = midi_slot_list([info(0, 0), info(1, 5)].iter());
        assert_eq!(
            list.iter().map(|m| m.r#type).collect::<Vec<_>>(),
            vec![0, 1]
        );
        // What is read back can be written back: it passes the writer's check.
        assert!(crate::commands::DeviceOperation::SetPresetLoadMidiOut {
            messages: midi_slot_list(slots.iter())
        }
        .try_encode()
        .is_ok());

        let group = |msg: Vec<crate::proto::MidiMessageInfo>, source: Option<u32>| {
            pa::GeneralMidiMessages {
                messages: vec![pa::GeneralMidiMessage {
                    source: source.map(pa::general_midi_message::Source::Source),
                    msg,
                }],
            }
        };
        let payload = pa::MidiSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            general_midi_messages: Some(
                pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(group(
                    slots.clone(),
                    Some(2),
                )),
            ),
            preset_load_messages: Some(
                pa::midi_settings_message::PresetLoadMessages::PresetLoadMessages(group(
                    slots.clone(),
                    None,
                )),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        let update = StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_MIDI_SETTINGS, &payload)
            .unwrap()
            .remove(0);
        let live = &update.midi_out.as_ref().unwrap()[0];
        assert_eq!(live.source, 2);
        assert_eq!(
            live.messages.iter().map(|m| m.r#type).collect::<Vec<_>>(),
            vec![1, 0, 3, 0, 2]
        );
        assert_eq!(
            update
                .preset_load_midi_out
                .as_ref()
                .unwrap()
                .iter()
                .map(|m| m.r#type)
                .collect::<Vec<_>>(),
            vec![1, 0, 3, 0, 2]
        );
    }

    #[test]
    fn a_bypass_record_naming_its_own_row_and_column_is_addressed_by_them() {
        // The record sits at list position (0, 0) but says it belongs to row 1, column 3.
        let update = preset_update_from(BinaryPreset {
            chains: vec![Chain::default(), one_block_chain(1, 3)],
            bypass: vec![Bypass {
                row: Some(bypass::Row::Row(1)),
                col_bypass: vec![ColBypass {
                    column: Some(col_bypass::Column::Column(3)),
                    scene_mode: Some(col_bypass::SceneMode::SceneMode(true)),
                    scene_bypass: vec![SceneBypass { bypass: true }],
                }],
            }],
            ..Default::default()
        });
        let block = &update.blocks.unwrap()[0];
        assert_eq!((block.row, block.column), (1, 3));
        assert_eq!(block.bypassed, Some(true));
        assert_eq!(block.bypass_address.as_deref(), Some("explicit"));
        assert_eq!(block.bypass_scene_mode, Some(true));
    }

    #[test]
    fn a_bypass_record_naming_only_one_axis_takes_the_other_from_its_place() {
        // Position gives row 1; the record itself names column 3.
        let update = preset_update_from(BinaryPreset {
            chains: vec![Chain::default(), one_block_chain(1, 3)],
            bypass: vec![
                Bypass::default(),
                Bypass {
                    row: None,
                    col_bypass: vec![ColBypass {
                        column: Some(col_bypass::Column::Column(3)),
                        scene_mode: None,
                        scene_bypass: vec![SceneBypass { bypass: true }],
                    }],
                },
            ],
            ..Default::default()
        });
        let block = &update.blocks.unwrap()[0];
        assert_eq!(block.bypassed, Some(true));
        assert_eq!(block.bypass_address.as_deref(), Some("mixed"));
        assert_eq!(block.bypass_scene_mode, None);
    }

    #[test]
    fn a_block_without_a_bypass_record_reports_no_bypass_state_and_no_address() {
        let update = preset_update_from(BinaryPreset {
            chains: vec![one_block_chain(0, 2)],
            ..Default::default()
        });
        let block = &update.blocks.unwrap()[0];
        assert_eq!(block.bypassed, None);
        assert_eq!(block.bypass_address, None);
        assert_eq!(block.bypass_scene_mode, None);
    }

    #[test]
    fn mode_values_outside_the_nine_named_ones_surface_as_unknown_with_the_raw_number() {
        let decode_mode = |mode: u32, available: Vec<u32>| {
            StateDecoder::new()
                .decode(
                    profile::MESSAGE_TYPE_MODE,
                    &pa::ModeMessage {
                        action: pa::message_action::Enum::Update as i32,
                        mode: Some(pa::mode_message::Mode::Mode(mode)),
                        available_modes: Some(pa::mode_message::AvailableModes::AvailableModes(
                            pa::AvailableModes { modes: available },
                        )),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap()
                .remove(0)
        };
        for (mode, kind, rows) in [
            (0, "PRESET", ["PRESET", "PRESET"]),
            (1, "SCENE", ["SCENE", "SCENE"]),
            (2, "STOMP", ["STOMP", "STOMP"]),
            (3, "HYBRID", ["PRESET", "SCENE"]),
            (8, "HYBRID", ["STOMP", "SCENE"]),
        ] {
            let update = decode_mode(mode, vec![1, 0, 2, 9]);
            assert_eq!(update.mode.as_deref(), Some(kind), "mode {mode}");
            assert_eq!(update.raw_mode, Some(mode));
            assert_eq!(
                update.footswitch_modes.unwrap(),
                rows.map(String::from).to_vec()
            );
        }
        // A value outside 0..=8 is UNKNOWN, keeps its number, and names no per-row modes.
        for mode in [9, 255] {
            let update = decode_mode(mode, vec![1, 0, 2, 9]);
            assert_eq!(update.mode.as_deref(), Some("UNKNOWN"), "mode {mode}");
            assert_eq!(update.raw_mode, Some(mode));
            assert_eq!(update.footswitch_modes, None);
        }
        // Only the first three entries of the configured list are slots, and each keeps its
        // raw value; an unknown entry is reported as such, not as a base mode.
        let update = decode_mode(2, vec![1, 12, 2, 0]);
        let slots = update.mode_slots.unwrap();
        assert_eq!(
            slots
                .iter()
                .map(|slot| (slot.mode.as_str(), slot.label.as_str(), slot.raw_mode))
                .collect::<Vec<_>>(),
            [
                ("SCENE", "SCENE", Some(1)),
                ("UNKNOWN", "UNKNOWN", Some(12)),
                ("STOMP", "STOMP", Some(2)),
            ]
        );
    }

    #[test]
    fn scene_colours_and_labels_keep_the_stored_values_beside_the_derived_ones() {
        let update = preset_update_from(BinaryPreset {
            scene_labels: vec!["Clean".into(), "  ".into(), String::new()],
            scene_colors: vec![0xffff_2727, 0xff0a_74e0],
            ..Default::default()
        });
        assert_eq!(update.scene_colors.unwrap(), ["#ff2727", "#0a74e0"]);
        assert_eq!(
            update.scene_colors_argb.unwrap(),
            [0xffff_2727, 0xff0a_74e0]
        );
        let scenes = update.scenes.unwrap();
        assert_eq!(&scenes[..4], ["Clean", "Scene B", "Scene C", "Scene D"]);
        // The stored labels are not given default text, and a missing label is empty.
        assert_eq!(
            update.scene_labels_raw.unwrap(),
            ["Clean", "  ", "", "", "", "", "", ""]
        );

        let pushed = StateDecoder::new()
            .decode(
                profile::MESSAGE_TYPE_SCENE_COLOR,
                &pa::SceneColorMessage {
                    action: pa::message_action::Enum::Update as i32,
                    index: 2,
                    color: 0xff45_f862,
                    request_id: None,
                }
                .encode_to_vec(),
            )
            .unwrap()
            .remove(0);
        assert_eq!(pushed.color.as_deref(), Some("#45f862"));
        assert_eq!(pushed.raw_color, Some(0xff45_f862));
    }

    #[test]
    fn route_ids_keep_their_wire_value_and_name_and_an_absent_id_stays_absent() {
        let update = preset_update_from(BinaryPreset {
            chains: vec![
                Chain {
                    in_portid: Some(chain::InPortid::InPortid(1)),
                    out_portid: Some(chain::OutPortid::OutPortid(19)),
                    ..Default::default()
                },
                // A unit writes an unrouted row's ids explicitly, as zero.
                Chain {
                    in_portid: Some(chain::InPortid::InPortid(0)),
                    out_portid: Some(chain::OutPortid::OutPortid(0)),
                    ..Default::default()
                },
                // A chain that names neither id.
                Chain::default(),
                Chain {
                    in_portid: Some(chain::InPortid::InPortid(14)),
                    out_portid: Some(chain::OutPortid::OutPortid(23)),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let routes = update.routes.unwrap();
        let view = |index: usize| {
            let route = &routes[index];
            (
                route.input_id,
                route.input.as_str(),
                route.input_wire.as_deref(),
                route.output_id,
                route.output.as_str(),
                route.output_wire.as_deref(),
            )
        };
        assert_eq!(
            view(0),
            (
                Some(1),
                "In 1",
                Some("INPUT_1"),
                Some(19),
                "Multi Out",
                Some("MULTIPLE_OUTS")
            )
        );
        assert_eq!(
            view(1),
            (
                Some(0),
                "Internal",
                Some("EMPTY"),
                Some(0),
                "Internal",
                Some("EMPTY")
            )
        );
        assert_eq!(view(2), (None, "", None, None, "", None));
        // 14 is declared by the wire but is no selectable route, so it reads as its wire name;
        // 23 is the output enum's count marker and is not a declared port at all.
        assert_eq!(
            view(3),
            (
                Some(14),
                "SIDECHAIN_BUFFER",
                Some("SIDECHAIN_BUFFER"),
                Some(23),
                "Unknown output 23",
                None
            )
        );
    }

    #[test]
    fn route_labels_come_from_one_table_and_the_only_exceptions_are_pinned() {
        assert!(!domain::INPUT_ROUTES.is_empty() && !domain::OUTPUT_ROUTES.is_empty());
        assert!(
            !domain::INPUT_ROUTE_WIRE_NAMES.is_empty()
                && !domain::OUTPUT_ROUTE_WIRE_NAMES.is_empty()
        );
        for (id, label) in domain::INPUT_ROUTES {
            let expected = INPUT_LABEL_ALIASES
                .iter()
                .find(|(alias, _)| alias == id)
                .map_or(*label, |(_, text)| *text);
            assert_eq!(input_label(*id), expected, "input {id}");
            assert!(
                input_wire_name(*id).is_some(),
                "input {id} has no wire name"
            );
        }
        for (id, label) in domain::OUTPUT_ROUTES {
            let expected = OUTPUT_LABEL_ALIASES
                .iter()
                .find(|(alias, _)| alias == id)
                .map_or(*label, |(_, text)| *text);
            assert_eq!(output_label(*id), expected, "output {id}");
            assert!(
                output_wire_name(*id).is_some(),
                "output {id} has no wire name"
            );
        }
        // Every wire-declared id reads as something other than a guess.
        for (id, _) in domain::INPUT_ROUTE_WIRE_NAMES {
            assert!(!input_label(*id).starts_with("Unknown"), "input {id}");
        }
        for (id, _) in domain::OUTPUT_ROUTE_WIRE_NAMES {
            assert!(!output_label(*id).starts_with("Unknown"), "output {id}");
        }
        // These are the two places where the decoded label is not the table's label. The Grid
        // screens compare these strings, so each is a deliberate compatibility alias.
        assert_eq!(INPUT_LABEL_ALIASES, &[(0, "Internal")]);
        assert_eq!(OUTPUT_LABEL_ALIASES, &[(18, "Rows 3/4")]);
        assert_eq!(input_label(99), "Unknown input 99");
        assert_eq!(output_label(99), "Unknown output 99");
    }

    #[test]
    fn preset_midi_out_reads_the_group_ordinal_as_the_source_keeps_positions_and_reports_the_flat_offset(
    ) {
        // The mapping, pinned so a change to it is deliberate. The preset holds 12 preset-load
        // rows in `midi_messages` and 120 rows in `midi_messages_general_v2`; group s of the 120 is
        // source s (observed on a CorOS 4.1.0 unit, 2026-10-07). Each source's list is positional:
        // a message sits at the index of the row it occupied, an empty row between two messages
        // stays as an empty entry and trailing empties are dropped.
        let message = |channel: u32| crate::proto::MidiMessageInfo {
            r#type: 1,
            channel,
            param1: 1,
            param2: 2,
            param3: 3,
        };
        let mut general = vec![crate::proto::MidiMessageInfo::default(); 120];
        general[12 + 5] = message(2); // group 1, row 5
        general[12 + 7] = message(3); // group 1, row 7
        general[108 + 11] = message(4); // group 9, row 11
        let mut preset_load = vec![crate::proto::MidiMessageInfo::default(); 12];
        preset_load[4] = message(5);
        let update = preset_update_from(BinaryPreset {
            midi_messages_general_v2: general,
            midi_messages: preset_load,
            ..Default::default()
        });
        let sources = update.midi_out.unwrap();
        assert_eq!(
            sources
                .iter()
                .map(|source| (
                    source.source,
                    source.preset_flat_offset,
                    source
                        .messages
                        .iter()
                        .map(|m| (m.r#type, m.channel))
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            [
                (
                    1,
                    Some(12),
                    vec![
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (1, 2),
                        (0, 1),
                        (1, 3)
                    ]
                ),
                (
                    9,
                    Some(108),
                    vec![
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (0, 1),
                        (1, 4)
                    ]
                ),
            ]
        );
        let preset_load = update.preset_load_midi_out.unwrap();
        assert_eq!(preset_load.len(), 5, "rows 0 to 3 stay as empties");
        assert_eq!(preset_load[4].channel, 5);
    }

    #[test]
    fn a_midi_settings_push_with_a_group_that_has_no_source_is_the_preset_load_list() {
        // The shape of a push a unit sent after a preset-load edit: one group of twelve rows in
        // the general field, the first row set, and no source on the group.
        let mut rows = vec![crate::proto::MidiMessageInfo::default(); 12];
        rows[0] = crate::proto::MidiMessageInfo {
            r#type: 1,
            channel: 1,
            param1: 1,
            param2: 10,
            param3: 64,
        };
        let sourced = pa::GeneralMidiMessage {
            msg: vec![crate::proto::MidiMessageInfo {
                r#type: 2,
                channel: 3,
                ..Default::default()
            }],
            source: Some(pa::general_midi_message::Source::Source(4)),
        };
        let update = StateDecoder::new()
            .decode(
                profile::MESSAGE_TYPE_MIDI_SETTINGS,
                &pa::MidiSettingsMessage {
                    action: pa::message_action::Enum::Update as i32,
                    general_midi_messages: Some(
                        pa::midi_settings_message::GeneralMidiMessages::GeneralMidiMessages(
                            pa::GeneralMidiMessages {
                                messages: vec![
                                    pa::GeneralMidiMessage {
                                        msg: rows,
                                        source: None,
                                    },
                                    sourced,
                                ],
                            },
                        ),
                    ),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap()
            .remove(0);
        let preset_load = update.preset_load_midi_out.unwrap();
        assert_eq!(
            (
                preset_load.len(),
                preset_load[0].param2,
                preset_load[0].param3
            ),
            (1, 10, 64)
        );
        let sources = update.midi_out.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source, 4);
        assert_eq!(sources[0].preset_flat_offset, None);
    }

    #[test]
    fn undo_and_redo_frames_travel_unmodified_without_inventing_availability() {
        let decode_history = |undo: Vec<i32>, redo: Vec<i32>| {
            StateDecoder::new()
                .decode(
                    profile::MESSAGE_TYPE_UNDO_REDO,
                    &pa::UndoRedoMessage {
                        action: pa::message_action::Enum::Update as i32,
                        undo_frames: undo,
                        redo_frames: redo,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap()
                .remove(0)
        };
        // Nonempty, zero-filled and empty lists all remain uninterpreted raw evidence.
        let mut frames = vec![0; 20];
        frames[1] = 5;
        frames[2] = 5;
        let busy = decode_history(frames.clone(), Vec::new());
        assert_eq!(busy.undo_available, None);
        assert_eq!(busy.redo_available, None);
        assert_eq!(busy.undo_frames.as_deref(), Some(&frames[..]));
        assert_eq!(busy.redo_frames.as_deref(), Some(&[][..]));
        let idle = decode_history(vec![0; 20], Vec::new());
        assert_eq!(idle.undo_available, None);
        assert_eq!(idle.redo_available, None);
        assert_eq!(idle.undo_frames.unwrap(), vec![0; 20]);
        let empty = decode_history(Vec::new(), Vec::new());
        assert_eq!(empty.undo_available, None);
        assert_eq!(empty.redo_available, None);
        assert_eq!(empty.undo_frames, Some(Vec::new()));
        assert_eq!(empty.redo_frames, Some(Vec::new()));
    }

    #[test]
    fn a_sparse_global_tempo_push_reads_an_absent_index_as_zero_not_as_its_position() {
        let unindexed = |value: f32| Param {
            index: None,
            param_values: vec![ParamValue {
                value: Some(param_value::Value::FloatValue(value)),
            }],
            ..Default::default()
        };
        let decode_tempo = |params: Vec<Param>| {
            StateDecoder::new()
                .decode(
                    profile::MESSAGE_TYPE_GLOBAL_TEMPO,
                    &pa::GlobalTempoMessage {
                        action: pa::message_action::Enum::Update as i32,
                        params,
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap()
        };
        // The unindexed parameter is third in the list. Read by position it would be index 2,
        // the tempo LED, and publish a LED change. Read as the sparse push it is, it is index 0,
        // the global tempo itself, which the snapshot does not take from this message.
        let sparse = decode_tempo(vec![
            numeric_param(1, 0.0),
            numeric_param(3, 0.0),
            unindexed(0.9),
        ]);
        assert!(sparse.is_empty());
        // The same shape with an explicit index 2 still publishes the LED.
        let led = decode_tempo(vec![numeric_param(2, 0.9)]);
        assert_eq!(led[0].tempo_led_enabled, Some(true));
    }
}
