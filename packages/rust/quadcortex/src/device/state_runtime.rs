//! Atomic device-state reducer shared by every native host.
//!
//! The platform adapter decides which thread performs expensive ModelRepo
//! parsing and how observations are published to its UI. Decoding, snapshot
//! reduction, preset-catalog ingestion, and projections are one Rust concern.

use crate::commands::DeviceOperation;
use crate::device::native_catalog::{NativeCatalogChange, NativePresetCatalog};
use crate::device::{
    GatewaySnapshot, PresetEntry, PresetFolder, PresetLibrary, PresetList, PresetSlotList,
};
use crate::profile;
use crate::proto::{cortex_protobuf_v2 as pa, param};
use crate::responses::{
    decode_favorites, decode_pinned_models, decode_tempo_mode, decode_tempo_settings,
    merge_general_settings_publication, merge_io_settings_publication,
    settings_publication_is_retainable,
};
use crate::responses::{decode_tempo_clock, TempoClock};
use crate::state::{
    decode_file_message, BlockDetails, BlockParameter, ModelCatalog, ModelList,
    PresetFolderListing, StateDecoder, StateUpdate,
};
use prost::Message;
use serde_json::{json, Value};

/// The result of ingesting one assembled QC message into a [`DeviceStateRuntime`].
#[derive(Debug, Clone, PartialEq)]
pub struct StateObservation {
    /// Normalized state updates produced by the message. Empty for File messages.
    pub states: Vec<StateUpdate>,
    /// Tempo clock reading decoded from a global tempo message, if it carried one.
    pub tempo_clock: Option<TempoClock>,
    /// Whether a File message updated or removed at least one preset folder.
    pub preset_catalog_observed: bool,
    /// Key of the first preset folder updated by the message, if any.
    pub preset_folder_key: Option<String>,
    /// The original native envelope. Materialized retained rows cannot stand
    /// in for the action or field presence of this particular observation.
    pub file_message: Option<Box<pa::FileMessage>>,
    /// The folder updates, removals and rename the native preset catalog applied. Empty for
    /// messages that are not File messages and for capture and IR file types.
    pub native_catalog_change: NativeCatalogChange,
}

/// Retained device state reduced from native messages: the decoder, the gateway snapshot, the
/// preset library and native preset catalog, and the global tempo, pinned model, favorites and
/// settings publications that write preconditions rely on.
#[derive(Default)]
pub struct DeviceStateRuntime {
    decoder: StateDecoder,
    snapshot: GatewaySnapshot,
    presets: PresetLibrary,
    native_presets: NativePresetCatalog,
    global_mode: Option<String>,
    global_bpm: Option<u32>,
    pinned_models: Option<Value>,
    favorites: Option<Value>,
    // CorOS publishes general and I/O settings once at handshake and then
    // answers writes with a sparse echo; that it never answers an on-demand
    // READ of type 9 or type 3 is BELIEVED (recorded without a date or
    // artifact). Retaining and merging those publications here -
    // in the state runtime both hosts drive - is what lets either host serve
    // the read without putting a request on the wire the device ignores.
    settings_publications: std::collections::HashMap<u16, Vec<u8>>,
}

impl DeviceStateRuntime {
    /// Creates a runtime with no retained state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards all retained state, including the installed model catalog, returning the runtime to
    /// its initial state.
    pub fn reset(&mut self) {
        self.decoder.reset();
        self.snapshot = GatewaySnapshot::default();
        self.presets.clear();
        self.native_presets.clear();
        self.global_mode = None;
        self.global_bpm = None;
        self.pinned_models = None;
        self.favorites = None;
        self.settings_publications.clear();
    }

    /// The dispatch half of the inbound telemetry. See
    /// [`crate::inbound_telemetry`].
    pub fn inbound_telemetry(&self) -> &crate::inbound_telemetry::InboundTelemetry {
        self.decoder.inbound_telemetry()
    }

    /// Decode and atomically reduce one assembled QC message. ModelRepo is
    /// installed separately so hosts can parse that large payload off the hot
    /// receive lane without changing reduction semantics.
    pub fn ingest(
        &mut self,
        message_type: u16,
        payload: &[u8],
    ) -> Result<StateObservation, String> {
        if message_type == profile::MESSAGE_TYPE_MODEL_REPO {
            return Err("ModelRepo must be parsed off-lane and installed atomically".into());
        }
        self.observe_external_preflight(message_type, payload);
        self.retain_settings_publication(message_type, payload);
        if message_type == profile::MESSAGE_TYPE_FILE {
            let message = decode_file_message(payload).map_err(|error| {
                let error = error.to_string();
                self.native_presets.invalidate(error.clone());
                error
            })?;
            let native_catalog_change =
                if matches!(message.r#type, Some(pa::file_message::Type::Type(1 | 2))) {
                    // An unhealthy preset catalog cannot block an independently
                    // typed capture/IR update after that update has been applied.
                    NativeCatalogChange::default()
                } else {
                    self.native_presets.apply(&message)?
                };
            for key in &native_catalog_change.removed {
                self.presets.remove_folder(key);
            }
            for key in &native_catalog_change.updated {
                if let Some(listing) = self.native_presets.folder(key) {
                    self.presets.ingest(listing.clone());
                }
            }
            if let Some((source, destination)) = &native_catalog_change.renamed {
                if self.snapshot.setlist_key.trim_end_matches('/') == source {
                    self.snapshot.setlist_key = destination.clone();
                    self.snapshot.address_revision =
                        self.snapshot.address_revision.saturating_add(1);
                }
            }
            let preset_folder_key = native_catalog_change.updated.first().cloned();
            let observed = !native_catalog_change.updated.is_empty()
                || !native_catalog_change.removed.is_empty();
            self.reconcile_current_preset_identity();
            return Ok(StateObservation {
                states: Vec::new(),
                tempo_clock: None,
                preset_catalog_observed: observed,
                preset_folder_key,
                file_message: Some(Box::new(message)),
                native_catalog_change,
            });
        }
        let states = self
            .decoder
            .decode(message_type, payload)
            .map_err(|error| error.to_string())?;
        for state in &states {
            self.snapshot.apply(state);
        }
        if message_type == profile::MESSAGE_TYPE_RECALL_PRESET
            && states.iter().any(|state| state.kind == "preset")
        {
            self.snapshot.full_preset_revision =
                self.snapshot.full_preset_revision.saturating_add(1);
        }
        self.reconcile_current_preset_identity();
        let tempo_clock = if message_type == profile::MESSAGE_TYPE_GLOBAL_TEMPO {
            decode_tempo_clock(payload).map_err(|error| error.to_string())?
        } else {
            None
        };
        Ok(StateObservation {
            states,
            tempo_clock,
            preset_catalog_observed: false,
            preset_folder_key: None,
            file_message: None,
            native_catalog_change: NativeCatalogChange::default(),
        })
    }

    /// Installs a parsed model catalog, applies the resulting updates to the snapshot and returns
    /// them.
    pub fn install_model_catalog(&mut self, catalog: ModelCatalog) -> Vec<StateUpdate> {
        let states = self.decoder.install_catalog(catalog);
        for state in &states {
            self.snapshot.apply(state);
        }
        self.reconcile_current_preset_identity();
        states
    }

    /// The retained fold of the model preset list. It belongs to the session this runtime holds
    /// and is unavailable again after [`Self::reset`].
    pub fn model_preset_catalog(&self) -> &crate::state::ModelPresetCatalog {
        self.decoder.model_preset_catalog()
    }

    /// Records the transport's loss total as this runtime starts a session, before any ingress.
    pub fn seed_model_preset_loss(&mut self, total: u64) {
        self.decoder.seed_inbound_loss(total);
    }

    /// Feeds the transport's inbound loss total to the model preset fold. When that invalidated
    /// it, the projection is applied to the snapshot and returned for publication.
    pub fn observe_model_preset_loss(&mut self, total: u64) -> Option<StateUpdate> {
        let update = self.decoder.observe_inbound_loss(total)?;
        self.snapshot.apply(&update);
        Some(update)
    }

    /// The current model preset projection, applied to the snapshot and returned for
    /// publication. Used when a failed message changed the fold without producing an update.
    pub fn model_preset_catalog_update(&mut self) -> StateUpdate {
        let update = self.decoder.model_preset_catalog_update();
        self.snapshot.apply(&update);
        update
    }

    /// Marks the list a write precondition compares against as unconfirmed: the next guard that
    /// needs it fails closed until a complete list arrives. Only the favorites and pinned-model
    /// lists are retained as lists; the other preconditions are single values the device pushes.
    pub fn invalidate_external_preflight(&mut self, method: &str) {
        match method {
            "device.setFavorite" => self.favorites = None,
            "device.setModelPinned" => self.pinned_models = None,
            _ => {}
        }
    }

    /// The latest observed values relevant to external write preconditions.
    /// This is session state, not proof of a reply to a particular request.
    pub fn external_preflight_observation(&self, method: &str) -> Option<Value> {
        match method {
            "device.selectScene" => {
                let snapshot = self.snapshot();
                snapshot.has_preset.then(|| {
                    json!({
                        "presetName": snapshot.preset_name,
                        "activeScene": snapshot.active_scene,
                    })
                })
            }
            "device.setTempoMode" | "device.setGlobalTempo" => self
                .global_mode
                .as_ref()
                .map(|mode| json!({"mode": mode, "globalBpm": self.global_bpm})),
            "device.setModelPinned" => self.pinned_models.clone(),
            "device.setFavorite" => self.favorites.clone(),
            _ => None,
        }
    }

    /// Retain the settings families CorOS publishes rather than answers.
    /// The first frame of a type is the handshake publication; later frames
    /// are sparse write echoes, folded with the shared merge so an omitted
    /// field cannot erase a retained one. Both hosts drive this, so both can
    /// serve `device.generalSettings` and `device.ioSettings` without putting
    /// a READ on the wire that the QC is believed never to answer (BELIEVED, no
    /// dated record in the repository).
    fn retain_settings_publication(&mut self, message_type: u16, payload: &[u8]) {
        // A tagged type-3 is not this host's settings: keep the last good one.
        if !settings_publication_is_retainable(message_type, payload) {
            return;
        }
        let merge = match message_type {
            profile::MESSAGE_TYPE_GENERAL_SETTINGS => merge_general_settings_publication,
            profile::MESSAGE_TYPE_IO_SETTINGS => merge_io_settings_publication,
            _ => return,
        };
        match self.settings_publications.get(&message_type) {
            Some(previous) => {
                // A frame that will not fold is not evidence; keep the last
                // good publication rather than replacing it with a partial.
                if let Ok(merged) = merge(previous, payload) {
                    self.settings_publications.insert(message_type, merged);
                }
            }
            None => {
                self.settings_publications
                    .insert(message_type, payload.to_vec());
            }
        }
    }

    /// The retained publication for a settings family, if one has arrived.
    pub fn published_settings(&self, message_type: u16) -> Option<&[u8]> {
        self.settings_publications
            .get(&message_type)
            .map(Vec::as_slice)
    }

    fn observe_external_preflight(&mut self, message_type: u16, payload: &[u8]) {
        match message_type {
            profile::MESSAGE_TYPE_GLOBAL_TEMPO => {
                let Ok(message) = pa::GlobalTempoMessage::decode(payload) else {
                    self.global_mode = None;
                    self.global_bpm = None;
                    return;
                };
                if message.action != pa::message_action::Enum::Update as i32
                    || message.request_id.is_some()
                {
                    return;
                }
                // Sparse updates replace only the present precondition. A clock
                // tick or metronome edit cannot invalidate unrelated tempo state.
                let has_index = |index| {
                    message.params.iter().any(|parameter| {
                        let actual = match parameter.index {
                            Some(param::Index::Index(value)) => value,
                            None => 0,
                        };
                        actual == index
                    })
                };
                if has_index(0) {
                    self.global_bpm = decode_tempo_settings(&message.params).bpm;
                }
                if has_index(1) {
                    self.global_mode = decode_tempo_mode(payload).ok().map(|value| value.mode);
                }
            }
            profile::MESSAGE_TYPE_PINNED_MODELS => {
                // A CREATE or DELETE carries only the model that changed (measured on CorOS
                // 4.1.0, 2026-10-07: one id within about 30 ms of a pin or unpin), so it is not
                // a list. Keeping it as the retained list would make every other pinned model
                // read as unpinned. Mark the list unconfirmed instead; an explicit list read
                // (or any other publication) restores it. Every other action is kept as
                // decoded, including an empty list; no invented full-list discriminator exists.
                let partial = pa::PinnedModelsMessage::decode(payload).is_ok_and(|message| {
                    message.action == pa::message_action::Enum::Create as i32
                        || message.action == pa::message_action::Enum::Delete as i32
                });
                self.pinned_models = if partial {
                    None
                } else {
                    decode_pinned_models(payload)
                        .ok()
                        .and_then(|value| serde_json::to_value(value).ok())
                };
            }
            profile::MESSAGE_TYPE_RECENTS_FAVORITES => {
                let Ok(message) = pa::RecentsFavoritesMessage::decode(payload) else {
                    self.favorites = None;
                    return;
                };
                if !message.is_favorites
                    || message.r#type
                        != pa::recents_favorites_message::RecentFavoriteType::Preset as i32
                {
                    return;
                }
                // CREATE/DELETE is not a complete list. Invalidate rather than
                // assign absent entries a membership from a partial publication.
                self.favorites = message
                    .request_id
                    .and_then(|id| {
                        let pa::recents_favorites_message::RequestId::RequestId(id) = id;
                        decode_favorites(payload, id).ok()
                    })
                    .and_then(|value| serde_json::to_value(value).ok());
            }
            _ => {}
        }
    }

    /// Install a local preset projection and reconcile current identity.
    pub fn ingest_preset_listing(&mut self, listing: PresetFolderListing) {
        self.presets.ingest(listing);
        self.reconcile_current_preset_identity();
    }

    /// Native retained state before presentation overlays or local installation.
    /// It is not a claim that every slot or every folder has been observed.
    pub fn native_preset_catalog(&self) -> &NativePresetCatalog {
        &self.native_presets
    }

    /// The Quad Cortex working buffer carries no name of its own, so a preset
    /// that *is* saved reads back as "Unsaved" once the post-save patch from
    /// `reconcile_saved_preset_snapshot` is gone — and a session reset rebuilds
    /// this snapshot from the device, dropping it. The File catalog keeps the
    /// stored name, which is why `list_preset_slots` stayed correct while
    /// `get_current_preset` reverted to "Unsaved" for the same slot. Resolve
    /// the current preset's identity from that same durable source so the two
    /// endpoints cannot disagree about one preset.
    ///
    /// Identity and divergence are separate questions: `dirty` still reports
    /// whether the buffer has been edited away from the stored preset, so
    /// adopting the stored name here never claims unsaved work is saved.
    fn reconcile_current_preset_identity(&mut self) {
        if !self.snapshot.has_preset || !is_unnamed_preset(&self.snapshot.preset_name) {
            return;
        }
        let Some(entry) = self
            .presets
            .entry(&self.snapshot.setlist_key, self.snapshot.preset_position)
        else {
            return;
        };
        if is_unnamed_preset(&entry.name) {
            return;
        }
        self.snapshot.preset_name = entry.name;
    }

    /// Returns the current gateway snapshot.
    pub fn snapshot(&self) -> &GatewaySnapshot {
        &self.snapshot
    }

    /// Returns the gateway snapshot for in-place adjustment by the session owner.
    pub fn snapshot_mut(&mut self) -> &mut GatewaySnapshot {
        &mut self.snapshot
    }

    /// Returns the retained preset folder listings.
    pub fn preset_library(&self) -> &PresetLibrary {
        &self.presets
    }

    /// Returns the retained preset folder listings for in-place adjustment.
    pub fn preset_library_mut(&mut self) -> &mut PresetLibrary {
        &mut self.presets
    }

    /// Returns the parameters and values of the block at grid `row` and `column` of the current
    /// preset, or `None` when it is not retained. See [`StateDecoder::block_details`].
    pub fn block_details(&self, row: u32, column: u32) -> Option<BlockDetails> {
        self.decoder.block_details(row, column)
    }

    /// The sibling switch the model catalog declares as the reason a
    /// parameter is disabled. See
    /// [`StateDecoder::parameter_disabled_when_sibling_on`].
    pub fn parameter_disabled_when_sibling_on(
        &self,
        model_id: u32,
        parameter_index: u32,
    ) -> Option<u32> {
        self.decoder
            .parameter_disabled_when_sibling_on(model_id, parameter_index)
    }

    /// Builds the operation that points the Neural Capture block at `row` and `column` at the
    /// capture identified by `key`.
    ///
    /// # Errors
    ///
    /// Returns a message when the coordinates are out of range, `expected_model_id` is not a Neural
    /// Capture model, the retained preset is not an exact full preset, or the block is absent,
    /// ambiguous or no longer that model. See [`StateDecoder::capture_retarget_operation`].
    pub fn capture_retarget_operation(
        &self,
        row: u32,
        column: u32,
        expected_model_id: u32,
        key: &str,
    ) -> Result<DeviceOperation, String> {
        self.decoder
            .capture_retarget_operation(row, column, expected_model_id, key)
    }

    /// Builds the operation that moves the block at (`from_row`, `from_column`) to (`to_row`,
    /// `to_column`), optionally checking that it is still the model `expected_model_id`.
    ///
    /// # Errors
    ///
    /// Returns a message when a coordinate is outside rows 0-3 and columns 0-7, the retained grid
    /// is stale, the source block is absent, ambiguous or changed, or the destination is occupied.
    /// See [`StateDecoder::move_block_operation`].
    pub fn move_block_operation(
        &self,
        from_row: u32,
        from_column: u32,
        to_row: u32,
        to_column: u32,
        expected_model_id: Option<u32>,
    ) -> Result<DeviceOperation, String> {
        self.decoder.move_block_operation(
            from_row,
            from_column,
            to_row,
            to_column,
            expected_model_id,
        )
    }

    /// See [`StateDecoder::move_grid_layout_diagnostic`].
    pub fn move_grid_layout_diagnostic(&self) -> String {
        self.decoder.move_grid_layout_diagnostic()
    }

    /// Returns whether the retained preset is still exact and at `revision`. See
    /// [`StateDecoder::capture_retarget_revision_is_current`].
    pub fn capture_retarget_revision_is_current(&self, revision: u64) -> bool {
        self.decoder.capture_retarget_revision_is_current(revision)
    }

    /// Returns an opaque snapshot of the exact current preset, or `None` when none is available.
    /// See [`StateDecoder::preset_source_snapshot`].
    pub fn preset_source_snapshot(&self) -> Option<crate::state::PresetSourceSnapshot> {
        self.decoder.preset_source_snapshot()
    }

    /// Returns whether the current preset is still exact and equal to `source`. See
    /// [`StateDecoder::preset_source_matches`].
    pub fn preset_source_matches(&self, source: &crate::state::PresetSourceSnapshot) -> bool {
        self.decoder.preset_source_matches(source)
    }

    /// Returns the parameters of the row-level control `control` (`inputGate` or `laneOutput`) on
    /// `row`, or `None` when absent or `control` is unknown. See
    /// [`StateDecoder::lane_control_details`].
    pub fn lane_control_details(&self, row: u32, control: &str) -> Option<BlockDetails> {
        self.decoder.lane_control_details(row, control)
    }

    /// Narrow accessor for write-verification polling: see
    /// [`crate::state::StateDecoder::single_parameter_target_detail`].
    pub fn single_parameter_target_detail(
        &self,
        row: u32,
        column: u32,
        index: u32,
    ) -> Option<BlockParameter> {
        self.decoder
            .single_parameter_target_detail(row, column, index)
    }

    /// Returns the number of models in the installed catalog.
    pub fn model_count(&self) -> usize {
        self.decoder.model_count()
    }

    /// Checks that `model_id` is a selectable model of the synchronized catalog. Plugin ownership
    /// is not checked here; the device enforces it.
    ///
    /// A catalog that holds no model at all (none installed in this session, or the session was
    /// reset since) cannot vouch for any id, so it is reported as unavailable, with the
    /// `STATE_UNAVAILABLE: ` prefix hosts map to a typed error. That is a different answer from
    /// "this catalog does not list the model", whose plain messages are unchanged.
    ///
    /// # Errors
    ///
    /// Returns a `STATE_UNAVAILABLE: ` message when no catalog is installed, and otherwise a plain
    /// message when the model is absent from the catalog or hidden.
    pub fn assert_model_available(&self, model_id: u32) -> Result<(), String> {
        if self.decoder.model_count() == 0 {
            return Err(
                "STATE_UNAVAILABLE: The Quad Cortex model catalog has not been synchronized in this session"
                    .into(),
            );
        }
        self.decoder.assert_model_available(model_id)
    }

    /// Lists the selectable models with their availability.
    ///
    /// # Errors
    ///
    /// Returns a message when no model catalog has been synchronized yet.
    pub fn model_list(&self) -> Result<ModelList, String> {
        let models = self.decoder.model_list();
        if models.audit.model_count == 0 {
            Err("The Quad Cortex model catalog has not been synchronized yet".into())
        } else {
            Ok(models)
        }
    }

    /// Returns the known preset setlist folders, factory library first.
    pub fn preset_folders(&self) -> Vec<PresetFolder> {
        self.presets.folders()
    }

    /// Returns the 256-slot listing of the setlist `key` relative to the current preset, or `None`
    /// when that setlist is not retained.
    pub fn preset_list(&self, key: &str) -> Option<PresetList> {
        self.presets.list(key, &self.snapshot)
    }

    /// Returns the destination slots of the active setlist for saving a preset.
    ///
    /// # Errors
    ///
    /// Returns a message when the active preset is not in a writable user setlist. Returns
    /// `Ok(None)` when the setlist listing is not fully retained.
    pub fn preset_slots(&self) -> Result<Option<PresetSlotList>, String> {
        self.presets.writable_slots(&self.snapshot)
    }

    /// Checks that the active preset is in a writable user setlist, so its destination slots can be
    /// listed.
    ///
    /// # Errors
    ///
    /// Returns the refusal message when it is not.
    pub fn validate_preset_slots_target(&self) -> Result<(), String> {
        PresetLibrary::validate_writable_slot_target(&self.snapshot)
    }

    /// Returns the occupied slot at `position` of the setlist `key`, or `None` when the slot is
    /// empty or not retained.
    pub fn preset_entry(&self, key: &str, position: u32) -> Option<PresetEntry> {
        self.presets.entry(key, position)
    }
}

/// CorOS leaves the name empty for a buffer or slot with no stored identity;
/// the gateway snapshot surfaces that as "Unsaved".
fn is_unnamed_preset(name: &str) -> bool {
    name.is_empty() || name == "Unsaved"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::cortex_protobuf_v2 as pa;
    use prost::Message;

    const SETLIST: &str = "/media/p4/Presets/My Presets";

    #[test]
    fn a_tagged_io_settings_publication_is_never_retained() {
        let io = |request_id: Option<u64>, out_ports: &[u32]| {
            pa::IoSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                request_id: request_id.map(pa::io_settings_message::RequestId::RequestId),
                settings: Some(pa::io_settings_message::Settings::Settings(
                    pa::PortSettings {
                        in_port: if out_ports.len() > 1 {
                            vec![pa::InputPortSettings {
                                input_port_id: 1,
                                ..Default::default()
                            }]
                        } else {
                            Vec::new()
                        },
                        out_port: out_ports
                            .iter()
                            .map(|&output_port_id| pa::OutputPortSettings {
                                output_port_id,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
            .encode_to_vec()
        };
        for request_id in [0, 7] {
            // First publication tagged: nothing is retained.
            let mut runtime = DeviceStateRuntime::new();
            let _ = runtime.ingest(
                profile::MESSAGE_TYPE_IO_SETTINGS,
                &io(Some(request_id), &[1, 4]),
            );
            assert!(runtime
                .published_settings(profile::MESSAGE_TYPE_IO_SETTINGS)
                .is_none());

            // Full, then a tagged sparse update: the full publication still reads.
            let full = io(None, &[1, 4]);
            let mut runtime = DeviceStateRuntime::new();
            let _ = runtime.ingest(profile::MESSAGE_TYPE_IO_SETTINGS, &full);
            let _ = runtime.ingest(
                profile::MESSAGE_TYPE_IO_SETTINGS,
                &io(Some(request_id), &[4]),
            );
            let retained = runtime
                .published_settings(profile::MESSAGE_TYPE_IO_SETTINGS)
                .unwrap();
            assert_eq!(retained, full.as_slice());
            assert!(crate::responses::decode_io_settings(retained).is_ok());
        }
    }

    #[test]
    fn preset_setlist_publication_reaches_the_native_preset_catalog() {
        let mut runtime = DeviceStateRuntime::new();
        let preset_frame = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(SETLIST.into())),
                name: Some(pa::folder_info::Name::Name("My Presets".into())),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key("preset-key".into())),
                    name: Some(pa::product_data::Name::Name("Numb".into())),
                    index: Some(pa::product_data::Index::Index(52)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();

        runtime
            .ingest(profile::MESSAGE_TYPE_FILE, &preset_frame)
            .unwrap();

        assert!(!runtime.preset_folders().is_empty());
    }

    /// A recalled preset whose payload carries no name of its own, which is
    /// what the device sends for the working buffer after a Save As.
    fn unnamed_preset() -> Vec<u8> {
        pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn recall_confirmation_requires_new_position_and_full_preset_in_either_order() {
        use crate::device::request::plan_preset_recall;
        let mut runtime = DeviceStateRuntime::new();
        runtime.ingest(15, &unnamed_preset()).unwrap();
        // The seed's clean dirty publication: a recall is admitted only on an observed flag.
        let clean = pa::PresetDirtyMessage {
            action: pa::message_action::Enum::Update as i32,
            is_dirty: false,
            ..Default::default()
        };
        runtime.ingest(34, &clean.encode_to_vec()).unwrap();
        runtime.snapshot_mut().setlist_key = SETLIST.into();
        runtime.snapshot_mut().preset_position = 8;
        let plan = plan_preset_recall(
            "device.recallPreset",
            &serde_json::json!({"setlistKey": SETLIST, "position": 9, "expectedPosition": 8, "expectedPresetName": "Unsaved"}),
            Some(runtime.snapshot()),
        )
        .unwrap();
        let position = |slot| {
            pa::SetlistPositionMessage {
                action: pa::message_action::Enum::Update as i32,
                position: Some(pa::setlist_position_message::Position::Position(slot)),
                folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                    SETLIST.into(),
                )),
                ..Default::default()
            }
            .encode_to_vec()
        };
        runtime.ingest(2, &position(9)).unwrap();
        assert!(!plan.matches(runtime.snapshot()));
        assert!(!plan.verification().matches(runtime.snapshot(), None));
        let metadata = pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                br#"<Models><Category name="Delay"><Model id="7" name="Echo" /></Category></Models>"#.to_vec())),
            ..Default::default()
        }.encode_to_vec();
        let revision = runtime.snapshot().preset_revision;
        runtime.install_model_catalog(crate::state::parse_model_repo(&metadata).unwrap());
        assert!(runtime.snapshot().preset_revision > revision);
        assert_eq!(runtime.snapshot().full_preset_revision, 1);
        assert!(!plan.verification().matches(runtime.snapshot(), None));
        runtime.ingest(15, &unnamed_preset()).unwrap();
        assert!(plan.matches(runtime.snapshot()));
        assert!(plan.verification().matches(runtime.snapshot(), None));

        let next = plan_preset_recall(
            "device.recallPreset",
            &serde_json::json!({"setlistKey": SETLIST, "position": 10, "expectedPosition": 9, "expectedPresetName": "Unsaved"}),
            Some(runtime.snapshot()),
        )
        .unwrap();
        runtime.ingest(15, &unnamed_preset()).unwrap();
        assert!(!next.matches(runtime.snapshot()));
        runtime.ingest(2, &position(10)).unwrap();
        assert!(next.matches(runtime.snapshot()));
        assert!(next.verification().matches(runtime.snapshot(), None));
        runtime.reset();
        assert_eq!(runtime.snapshot().full_preset_revision, 0);
    }

    fn listing_with_numb_at_52() -> PresetFolderListing {
        PresetFolderListing {
            key: SETLIST.into(),
            name: "My Presets".into(),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: (0..256)
                .map(|position| crate::state::PresetFileListing {
                    position,
                    name: if position == 52 {
                        "Numb".into()
                    } else {
                        String::new()
                    },
                    instrument: 0,
                    instrument_present: true,
                    product_key: (position == 52).then(|| format!("{SETLIST}/Numb.pb")),
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn on_slot_52(runtime: &mut DeviceStateRuntime) {
        let snapshot = runtime.snapshot_mut();
        snapshot.setlist_key = SETLIST.into();
        snapshot.preset_position = 52;
    }

    #[test]
    fn a_saved_preset_keeps_its_name_when_the_catalog_arrives_after_the_preset() {
        // Regression: save_preset_as wrote "Numb" to slot 52, the session then
        // reset, and afterwards list_preset_slots reported 52 as "Numb" while
        // get_current_preset reported "Unsaved" for the very same slot. The
        // saved name only ever lived in a transient snapshot patch that the
        // reset discarded. A caller reading the current preset's name to decide
        // whether work is saved would conclude, wrongly, that it is not.
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &unnamed_preset())
            .expect("preset ingest");
        assert!(runtime.snapshot().has_preset);
        assert_eq!(runtime.snapshot().preset_name, "Unsaved");

        on_slot_52(&mut runtime);
        runtime.ingest_preset_listing(listing_with_numb_at_52());

        assert_eq!(runtime.snapshot().preset_name, "Numb");
        // The catalog view and the current-preset view must agree.
        let slots = runtime.preset_slots().expect("slots").expect("slot list");
        let slot = slots
            .slots
            .iter()
            .find(|slot| slot.position == 52)
            .expect("slot 52");
        assert_eq!(slot.name, runtime.snapshot().preset_name);
        assert!(slot.occupied);
    }

    #[test]
    fn a_saved_preset_keeps_its_name_when_the_preset_arrives_after_the_catalog() {
        // After a reset both arrive fresh and the order is not guaranteed, so
        // the identity has to be resolved from whichever lands second.
        let mut runtime = DeviceStateRuntime::new();
        runtime.ingest_preset_listing(listing_with_numb_at_52());
        on_slot_52(&mut runtime);
        runtime
            .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &unnamed_preset())
            .expect("preset ingest");
        assert_eq!(runtime.snapshot().preset_name, "Numb");
    }

    #[test]
    fn catalog_identity_never_overwrites_a_named_buffer_or_invents_one() {
        // A buffer that reports its own name is authoritative for itself.
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &unnamed_preset())
            .expect("preset ingest");
        on_slot_52(&mut runtime);
        runtime.snapshot_mut().preset_name = "Something Else".into();
        runtime.ingest_preset_listing(listing_with_numb_at_52());
        assert_eq!(runtime.snapshot().preset_name, "Something Else");

        // A genuinely empty slot stays unnamed rather than borrowing a name.
        let mut empty = DeviceStateRuntime::new();
        empty
            .ingest(profile::MESSAGE_TYPE_RECALL_PRESET, &unnamed_preset())
            .expect("preset ingest");
        {
            let snapshot = empty.snapshot_mut();
            snapshot.setlist_key = SETLIST.into();
            snapshot.preset_position = 7;
        }
        empty.ingest_preset_listing(listing_with_numb_at_52());
        assert_eq!(empty.snapshot().preset_name, "Unsaved");

        // And with no preset synchronized at all, nothing is claimed.
        let mut bare = DeviceStateRuntime::new();
        on_slot_52(&mut bare);
        bare.ingest_preset_listing(listing_with_numb_at_52());
        assert!(!bare.snapshot().has_preset);
        assert_eq!(bare.snapshot().preset_name, "Unsaved");
    }

    /// The loaded preset's TempoControl block as a full preset read carries
    /// it: 24 ordered parameters with no index field, eight per-scene values
    /// each. Parameter 0 is 0.86 (212 BPM); 2 is the LED, 3 the metronome
    /// volume, 5 the pan, 6 the time signature, 10 the first beat cell.
    const PRESET_TEMPO_BLOCK: [f32; 24] = [
        0.86, 0.0, 1.0, 0.613137, 0.0, 0.5, 0.1, 0.0, 0.0, 0.0, 0.666667, 0.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ];
    const PRESET_BPM: u32 = 212;
    const GLOBAL_BPM: u32 = 120;

    fn full_preset_with_tempo(request_id: Option<u64>) -> Vec<u8> {
        use crate::proto::{param_value, BinaryPreset, Model, Param, ParamValue};
        pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: request_id.map(pa::recall_preset_message::RequestId::RequestId),
            preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                name: Some(crate::proto::binary_preset::Name::Name(
                    "Tempo Preset".into(),
                )),
                tempo_program_data: vec![Model {
                    params: PRESET_TEMPO_BLOCK
                        .iter()
                        .map(|value| Param {
                            index: None,
                            param_values: vec![
                                ParamValue {
                                    value: Some(param_value::Value::FloatValue(*value)),
                                };
                                8
                            ],
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec()
    }

    /// An uncorrelated GlobalTempo publication with explicit indices, one
    /// value per parameter, as the device publishes it at connect and in
    /// answer to a global tempo read.
    fn global_tempo_publication(params: &[(u32, f32)]) -> Vec<u8> {
        use crate::proto::{param, param_value, Param, ParamValue};
        pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params: params
                .iter()
                .map(|(index, value)| Param {
                    index: Some(param::Index::Index(*index)),
                    param_values: vec![ParamValue {
                        value: Some(param_value::Value::FloatValue(*value)),
                    }],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
        .encode_to_vec()
    }

    /// The full 25-parameter GlobalTempo block: global BPM 0.4 (120) at
    /// index 0, `mode` (0.0 PRESET, 1.0 GLOBAL) at index 1, and the same
    /// metronome controls the preset block carries.
    fn full_global_tempo(mode: f32) -> Vec<u8> {
        let mut params: Vec<(u32, f32)> = (0..25u32)
            .map(|index| {
                (
                    index,
                    PRESET_TEMPO_BLOCK
                        .get(index as usize)
                        .copied()
                        .unwrap_or(0.0),
                )
            })
            .collect();
        params[0].1 = 0.4;
        params[1].1 = mode;
        global_tempo_publication(&params)
    }

    fn global_observation(runtime: &DeviceStateRuntime) -> Value {
        runtime
            .external_preflight_observation("device.setGlobalTempo")
            .expect("the GlobalTempo publication is retained")
    }

    #[test]
    fn preset_tempo_survives_the_global_tempo_publication_that_follows_it() {
        // Connect order: full preset first, the GlobalTempo block ~10 ms later.
        let mut runtime = DeviceStateRuntime::new();
        runtime.ingest(15, &full_preset_with_tempo(None)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        runtime.ingest(33, &full_global_tempo(0.0)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        assert!(runtime.snapshot().tempo_led_enabled);
        assert_eq!(
            global_observation(&runtime),
            serde_json::json!({"mode": "PRESET", "globalBpm": GLOBAL_BPM})
        );
    }

    #[test]
    fn preset_tempo_is_reported_when_the_global_tempo_publication_arrives_first() {
        let mut runtime = DeviceStateRuntime::new();
        let observation = runtime.ingest(33, &full_global_tempo(0.0)).unwrap();
        // The global BPM is not a preset tempo: no state update carries it.
        assert!(observation.states.iter().all(|state| state.tempo.is_none()));
        runtime.ingest(15, &full_preset_with_tempo(None)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        runtime.ingest(33, &full_global_tempo(0.0)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        assert_eq!(global_observation(&runtime)["globalBpm"], GLOBAL_BPM);
    }

    #[test]
    fn global_tempo_mode_keeps_the_snapshot_on_the_preset_tempo() {
        // `tempo` is the loaded preset's tempo in either mode; the BPM the
        // unit plays in GLOBAL mode is `globalBpm`, reported separately.
        let mut runtime = DeviceStateRuntime::new();
        runtime.ingest(15, &full_preset_with_tempo(None)).unwrap();
        runtime.ingest(33, &full_global_tempo(1.0)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        assert_eq!(
            global_observation(&runtime),
            serde_json::json!({"mode": "GLOBAL", "globalBpm": GLOBAL_BPM})
        );
        // A sparse global BPM echo (what set_global_tempo sends) moves only
        // the global value.
        runtime
            .ingest(33, &global_tempo_publication(&[(0, 0.5)]))
            .unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        assert_eq!(global_observation(&runtime)["globalBpm"], 140);
    }

    #[test]
    fn global_tempo_settings_read_does_not_flip_the_preset_tempo() {
        // A globalTempoSettings read: the uncorrelated GlobalTempo block,
        // then the correlated full preset read, and the next publication.
        let mut runtime = DeviceStateRuntime::new();
        runtime.ingest(15, &full_preset_with_tempo(None)).unwrap();
        runtime.ingest(33, &full_global_tempo(0.0)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        runtime
            .ingest(15, &full_preset_with_tempo(Some(8)))
            .unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        runtime.ingest(33, &full_global_tempo(0.0)).unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
    }

    #[test]
    fn global_tempo_publication_still_carries_the_tempo_led() {
        let mut runtime = DeviceStateRuntime::new();
        runtime.ingest(15, &full_preset_with_tempo(None)).unwrap();
        assert!(runtime.snapshot().tempo_led_enabled);
        let observation = runtime
            .ingest(33, &global_tempo_publication(&[(2, 0.0)]))
            .unwrap();
        assert_eq!(observation.states.len(), 1);
        assert_eq!(observation.states[0].kind, "tempo");
        assert_eq!(observation.states[0].tempo, None);
        assert!(!runtime.snapshot().tempo_led_enabled);
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
    }

    const GRID_BPM: u32 = 150;

    /// The full tempo preset with one chain at row 0 routed to `output`, so
    /// a sparse grid route push has a retained chain to change.
    fn full_preset_with_tempo_and_chain(output: u32) -> Vec<u8> {
        use crate::proto::{chain, Chain};
        let mut message =
            pa::RecallPresetMessage::decode(full_preset_with_tempo(None).as_slice()).unwrap();
        let Some(pa::recall_preset_message::Preset::Preset(preset)) = message.preset.as_mut()
        else {
            unreachable!("the helper always carries a preset");
        };
        preset.chains.push(Chain {
            row: Some(chain::Row::Row(0)),
            out_portid: Some(chain::OutPortid::OutPortid(output)),
            ..Default::default()
        });
        message.encode_to_vec()
    }

    /// A sparse Grid UPDATE that moves row 0's output, as the unit's own
    /// route picker publishes it, optionally carrying a tempo block too.
    fn grid_route_push(output: u32, tempo: Option<u32>) -> Vec<u8> {
        use crate::proto::{chain, BinaryPreset, Chain};
        let mut preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                out_portid: Some(chain::OutPortid::OutPortid(output)),
                ..Default::default()
            }],
            ..Default::default()
        };
        if let Some(bpm) = tempo {
            let message =
                pa::GridMessage::decode(crate::commands::set_tempo(bpm).payload.as_slice())
                    .unwrap();
            let Some(pa::grid_message::Preset::Preset(tempo_preset)) = message.preset else {
                unreachable!("set_tempo always carries a preset");
            };
            preset.tempo_program_data = tempo_preset.tempo_program_data;
        }
        pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(preset)),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn grid_tempo_survives_a_route_change_that_re_emits_the_retained_preset() {
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(15, &full_preset_with_tempo_and_chain(1))
            .unwrap();
        assert_eq!(runtime.snapshot().tempo, PRESET_BPM);
        // set_tempo's own frame: a type-1 grid push of tempo parameter 0.
        let set_tempo = crate::commands::set_tempo(GRID_BPM);
        runtime
            .ingest(set_tempo.message_type, &set_tempo.payload)
            .unwrap();
        assert_eq!(runtime.snapshot().tempo, GRID_BPM);
        // A later route push re-projects the retained preset, tempo included.
        let observation = runtime
            .ingest(profile::MESSAGE_TYPE_GRID, &grid_route_push(2, None))
            .unwrap();
        let preset = observation
            .states
            .iter()
            .find(|state| state.kind == "preset")
            .expect("a changed route re-emits the retained preset");
        assert_eq!(preset.tempo, Some(GRID_BPM));
        assert_eq!(runtime.snapshot().tempo, GRID_BPM);
        assert!(runtime.snapshot().tempo_led_enabled);
    }

    #[test]
    fn grid_tempo_and_route_in_one_push_report_the_new_tempo() {
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(15, &full_preset_with_tempo_and_chain(1))
            .unwrap();
        runtime
            .ingest(
                profile::MESSAGE_TYPE_GRID,
                &grid_route_push(2, Some(GRID_BPM)),
            )
            .unwrap();
        assert_eq!(runtime.snapshot().tempo, GRID_BPM);
    }

    #[test]
    fn reset_clears_all_atomic_state_projections() {
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .preset_library_mut()
            .ensure_setlist("/media/p4/Presets/Test");
        runtime.snapshot_mut().has_preset = true;
        runtime.snapshot_mut().grid_layout_complete = true;
        runtime.reset();
        assert!(!runtime.snapshot().has_preset);
        assert!(!runtime.snapshot().grid_layout_complete);
        assert!(runtime.preset_folders().is_empty());
    }

    #[test]
    fn the_model_preset_fold_is_retained_for_snapshot_reads_and_dies_with_the_session() {
        let preset = |value: &str| pa::ModelPreset {
            id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                value: Some(pa::model_preset_id::Value::Value(value.into())),
                is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                hash: Some(pa::model_preset_id::Hash::Hash(
                    crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
                )),
            })),
            ..Default::default()
        };
        let payload = pa::ModelPresetMessage {
            action: pa::message_action::Enum::Create as i32,
            presets: vec![preset("one")],
            ..Default::default()
        }
        .encode_to_vec();
        let mut runtime = DeviceStateRuntime::new();
        assert!(runtime.snapshot().model_preset_catalog.is_none());
        let observation = runtime
            .ingest(profile::MESSAGE_TYPE_MODEL_PRESET, &payload)
            .unwrap();
        assert_eq!(observation.states.len(), 1);
        assert_eq!(observation.states[0].kind, "modelPresetCatalog");
        // The late-subscriber copy equals what was published.
        assert_eq!(
            runtime.snapshot().model_preset_catalog,
            observation.states[0].model_preset_catalog
        );
        // A catalog message says nothing about the current preset.
        assert!(!runtime.snapshot().has_preset);
        assert!(!runtime.snapshot().grid_layout_complete);
        assert!(runtime
            .model_preset_catalog()
            .admits("one", false, crate::state::GLOBAL_EQ_MODEL_PRESET_HASH)
            .is_ok());

        // A loss after the seeded baseline invalidates and publishes.
        runtime.seed_model_preset_loss(2);
        assert!(runtime.observe_model_preset_loss(2).is_none());
        let invalidated = runtime.observe_model_preset_loss(3).unwrap();
        assert_eq!(
            invalidated.model_preset_catalog.as_ref().unwrap().state,
            "invalidated"
        );
        assert_eq!(
            runtime.snapshot().model_preset_catalog,
            invalidated.model_preset_catalog
        );

        runtime.reset();
        assert!(runtime.snapshot().model_preset_catalog.is_none());
        assert!(runtime
            .model_preset_catalog()
            .admits("one", false, crate::state::GLOBAL_EQ_MODEL_PRESET_HASH)
            .unwrap_err()
            .message()
            .starts_with("STATE_UNAVAILABLE:"));
    }

    #[test]
    fn model_list_fails_closed_until_a_nonempty_catalog_is_installed() {
        let mut runtime = DeviceStateRuntime::new();
        assert_eq!(
            runtime.model_list().unwrap_err(),
            "The Quad Cortex model catalog has not been synchronized yet"
        );

        let payload = pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(
                pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                    br#"<Models><Category name="Delay"><Model id="7" name="Echo" /></Category></Models>"#.to_vec(),
                ),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        runtime.install_model_catalog(crate::state::parse_model_repo(&payload).unwrap());
        let models = runtime.model_list().unwrap();
        assert_eq!(models.audit.model_count, 1);
        assert_eq!(models.models[0].id, 7);

        runtime.reset();
        assert!(runtime.model_list().is_err());
    }
}
