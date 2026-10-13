//! Request ownership before catalog and complete-source acquisition.
use super::*;
use crate::device::catalog::CatalogVerificationPolicy;
use crate::device::correlation::RequestDeadline;
use crate::device::request::{
    assert_expected_preset, expected_position, plan_gateway_read, plan_preset_mutation,
    required_text, validate_setlist_key, validate_setlist_name,
};
use crate::device::{GatewaySnapshot, PresetLibrary};
use crate::profile;
use crate::state::PresetFolderListing;
use crate::sync::allocate_next_id;
use serde_json::Value;
use std::collections::BTreeMap;

/// Hosts execute read/timer actions; native observations, not read-success
/// booleans or cache projections, decide when the retained request can be planned.
/// Read actions use the internal native read planner. In particular currentPreset
/// means a complete-preset readback here, not the public cache-only RPC route.
#[derive(Debug, Clone)]
pub enum OwnedPresetPreparationAction {
    /// Perform a native read, then call `advance_preset_preparation` again.
    Read {
        /// Gateway read method to run.
        method: String,
        /// Parameters for the read.
        params: Value,
        /// Session context the read must run in.
        context: Box<DispatchContext>,
        /// Milliseconds left before the read's deadline.
        remaining_ms: u64,
    },
    /// Nothing to do until the given time.
    Wait {
        /// Time at which to call `advance_preset_preparation` again.
        until_ms: u64,
    },
    /// Preparation is finished and the preset workflow has been installed.
    Ready {
        /// Identity of the installed workflow, for `advance_preset_workflow`.
        workflow_id: u64,
    },
}

/// A preparation is really a tri-state: still working towards a plan,
/// permanently failed, or done and handed off to a preset workflow. Modeling
/// it as one field instead of two independent `Option`s makes the mutual
/// exclusivity a type-level fact instead of a convention every reader and
/// writer has to maintain by hand.
#[derive(Debug, Clone)]
enum PreparationStatus {
    Active,
    Failed(String),
    Ready(u64),
}

pub(in crate::device::owner) struct PresetPreparationEntry {
    id: u64,
    method: String,
    params: Value,
    original: GatewaySnapshot,
    source: SourceCheckpoint,
    /// Host policy (not a Cortex Control frame): the request carries an
    /// expected-active-preset guard, so the active preset must stay exactly as
    /// observed while the request is prepared. Only requests that act on the
    /// active preset have one.
    guards_active_preset: bool,
    /// Complete-preset (type 15) source acquisition. Only a Save needs it,
    /// because Save writes the active Grid. COPY and UPDATE name their source
    /// by file key and never read the active preset.
    needs_source: bool,
    full_directory: bool,
    required_keys: Vec<String>,
    context: DispatchContext,
    started_ms: u64,
    preparation_ms: u64,
    deadline: RequestDeadline,
    source_requested: bool,
    source_deadline: RequestDeadline,
    source_observation: Option<(ObservationTicket, SourceCheckpoint)>,
    last_sequence: u64,
    folders: BTreeMap<String, PresetFolderListing>,
    last_file_ms: Option<u64>,
    status: PreparationStatus,
}

impl PresetPreparationEntry {
    pub(in crate::device::owner) fn is_active(&self) -> bool {
        matches!(self.status, PreparationStatus::Active)
    }
    pub(in crate::device::owner) fn invalidate(&mut self, reason: String) {
        if self.is_active() {
            self.status = PreparationStatus::Failed(reason);
        }
    }

    /// Whether an inbound recall-preset frame (already known to publish a
    /// "preset" state kind) is fresh enough to be accepted as *the* complete
    /// preset source: it must be a newer observation than the last one this
    /// preparation looked at, its ingress must have actually started after
    /// preparation began and finished no earlier than it started, and there
    /// must still be time left on the source-acquisition deadline as of when
    /// it finished. Each condition is independent and fails closed on its
    /// own so none of it hides inside one compound boolean.
    fn accepts_source_candidate(&self, ticket: &ObservationTicket) -> bool {
        if ticket.sequence <= self.last_sequence {
            return false;
        }
        let Some(time) = ticket.ingress else {
            return false;
        };
        if time.first_report_at_ms <= self.started_ms {
            return false;
        }
        if time.first_report_at_ms > time.last_report_at_ms {
            return false;
        }
        self.source_deadline.remaining_ms(time.last_report_at_ms) > 0
    }
    fn navigation_matches(&self, state: &crate::device::state_runtime::DeviceStateRuntime) -> bool {
        let now = state.snapshot();
        now.has_preset
            && now
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(self.original.setlist_key.trim_end_matches('/'))
            && now.preset_position == self.original.preset_position
            && now.active_scene == self.original.active_scene
            && now.dirty == self.original.dirty
    }

    fn has_required_records(&self) -> bool {
        let slot = |key: &str, position: u32, metadata: bool| {
            self.folders
                .get(key.trim_end_matches('/'))
                .and_then(|folder| folder.files.iter().find(|file| file.position == position))
                .is_some_and(|file| {
                    // The desktop preset model defaults an absent instrument
                    // to "" (enum 0). This is a planning default, not new wire
                    // evidence: keep instrument_present false in the catalog.
                    !metadata
                        || file.product_key.is_none()
                        || file.instrument_present
                        || file.instrument == 0
                })
        };
        let key = |field: &str| {
            self.params
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let position = |field: &str| {
            self.params
                .get(field)
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(u32::MAX)
        };
        match self.method.as_str() {
            "device.savePresetAs" => {
                slot(key("setlistKey"), self.original.preset_position, true)
                    && (self.params.get("confirmOverwrite").and_then(Value::as_bool) == Some(true)
                        || slot(key("setlistKey"), position("position"), false))
            }
            "device.renameCurrentPreset" => slot(
                &self.original.setlist_key,
                self.original.preset_position,
                true,
            ),
            "device.copyPreset" => slot(key("sourceSetlistKey"), position("sourcePosition"), true),
            "device.movePreset" => {
                slot(key("setlistKey"), position("sourcePosition"), true)
                    && slot(key("setlistKey"), position("destinationPosition"), true)
            }
            "device.deletePreset" => slot(key("setlistKey"), position("position"), false),
            "device.duplicateSetlist" => self
                .folders
                .get(key("sourceSetlistKey").trim_end_matches('/'))
                .is_some_and(|folder| {
                    folder.files.len() == 256
                        && folder.files.iter().all(|file| {
                            file.product_key.is_none()
                                || file.instrument_present
                                || file.instrument == 0
                        })
                }),
            _ => true,
        }
    }
}

impl SessionRuntime {
    /// Starts preparing a persistent preset or setlist method and returns the preparation id.
    ///
    /// Preparation acquires the complete source preset and directory catalog evidence needed to plan the request before a workflow is installed. Fails if the parameters are invalid, the method is not a supported persistent preset method, the native catalog is unhealthy, another preparation or workflow is active, or the active preset does not match the request.
    pub fn begin_preset_preparation(
        &mut self,
        method: &str,
        params: Value,
        started_ms: u64,
    ) -> Result<u64, String> {
        crate::device::generated_gateway::validate_params(method, &params)?;
        self.state.native_preset_catalog().health()?;
        // Save persists the active Grid, so it alone needs the complete-preset
        // source read. Rename (File UPDATE) and copy (File COPY) are keyed
        // File frames: they read no preset source.
        let needs_source = method == "device.savePresetAs";
        // Host policy (not a Cortex Control frame): save and rename act on the
        // active preset and require the caller's expected-state inputs. Copy
        // acts on no active preset and carries none.
        let guards_active_preset =
            matches!(method, "device.savePresetAs" | "device.renameCurrentPreset");
        let names_persistent_preset =
            needs_source || matches!(method, "device.renameCurrentPreset" | "device.copyPreset");
        let full_directory = matches!(
            method,
            "device.createSetlist" | "device.duplicateSetlist" | "device.deleteSetlist"
        );
        if !names_persistent_preset
            && !full_directory
            && !matches!(method, "device.movePreset" | "device.deletePreset")
        {
            return Err(format!(
                "Persistent preset method is not supported: {method}"
            ));
        }
        if self
            .preset_preparation
            .as_ref()
            .is_some_and(PresetPreparationEntry::is_active)
            || self.preset_workflow.as_ref().is_some_and(|entry| {
                !entry.runtime.is_terminal()
                    || entry.native.as_ref().is_some_and(|native| {
                        self.deliveries.operation_in_flight(&native.operation)
                    })
            })
        {
            return Err("A QC preset workflow is already active".into());
        }
        let original = self.state.snapshot().clone();
        if method == "device.deleteSetlist" {
            let name = validate_setlist_name(required_text(&params, "name")?)?;
            let target = format!("/media/p4/Presets/{name}");
            if !original.has_preset || original.setlist_key.is_empty() {
                return Err("The active setlist is not synchronized; no deletion was sent".into());
            }
            if original
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(&target)
            {
                return Err(
                    "The active setlist cannot be deleted. Recall another setlist first.".into(),
                );
            }
        }

        if guards_active_preset {
            if !original.has_preset {
                return Err("No Quad Cortex preset has been synchronized yet".into());
            }
            assert_expected_preset(Some(&original), &params)?;
            if original.preset_position != expected_position(&params)? {
                return Err(
                    "The active preset slot changed on the Quad Cortex. Refresh and retry.".into(),
                );
            }
        }
        let fields: &[&str] = match method {
            "device.copyPreset" => &["sourceSetlistKey", "destinationSetlistKey"],
            "device.duplicateSetlist" => &["sourceSetlistKey"],
            "device.savePresetAs" | "device.movePreset" | "device.deletePreset" => &["setlistKey"],
            _ => &[],
        };
        let mut required_keys = fields
            .iter()
            .map(|field| {
                validate_setlist_key(required_text(&params, field)?, field)
                    .map(|key| key.trim_end_matches('/').to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if matches!(
            method,
            "device.renameCurrentPreset" | "device.deleteSetlist"
        ) {
            // Keep the known active folder in the planning operands. The
            // post-attempt deletion predicate uses an explicit target DELETE.
            required_keys.push(original.setlist_key.trim_end_matches('/').to_string());
        }
        required_keys.sort();
        required_keys.dedup();
        let source_ms = if needs_source {
            plan_gateway_read("device.currentPreset", &Value::Null, 0)?.timeout_ms
        } else {
            0
        };
        // Startup catalog collection overlaps complete-source acquisition.
        // Neither extends the other's original ceiling.
        let preparation_ms = source_ms.max(CatalogVerificationPolicy::default().timeout_ms);
        // Every supported family is one single-message persistent operation
        // followed by the host's catalog verification. The concrete budget is
        // checked before installation and can only narrow this ceiling.
        let write_ms = profile::COMMAND_CONFIRMATION_TIMEOUT_MS;
        let ceiling = match method {
            "device.savePresetAs" => {
                write_ms
                    + 15_000
                    + 3_000
                    + CatalogVerificationPolicy::persistent_file_mutation().timeout_ms
            }
            "device.duplicateSetlist" => {
                write_ms + CatalogVerificationPolicy::duplicate_setlist(256).timeout_ms
            }
            "device.createSetlist" => write_ms + CatalogVerificationPolicy::default().timeout_ms,
            _ => write_ms + CatalogVerificationPolicy::persistent_file_mutation().timeout_ms,
        };
        let mut context = self.session_context()?;
        context.deadline = Some(RequestDeadline::new(
            started_ms,
            preparation_ms.saturating_add(ceiling),
        ));
        let deadline = RequestDeadline::new(started_ms, preparation_ms);
        let id = allocate_next_id(&mut self.next_workflow, "QC preset workflow")?;
        self.preset_preparation = Some(PresetPreparationEntry {
            id,
            method: method.into(),
            params,
            source: SourceCheckpoint::capture(&self.state),
            original,
            guards_active_preset,
            needs_source,
            full_directory,
            required_keys,
            context,
            started_ms,
            preparation_ms,
            deadline,
            source_requested: false,
            source_deadline: RequestDeadline::new(started_ms, source_ms),
            source_observation: None,
            last_sequence: self.next_observation.saturating_sub(1),
            folders: BTreeMap::new(),
            last_file_ms: None,
            status: PreparationStatus::Active,
        });
        Ok(id)
    }

    /// Cancels preparation `id` if it is the active preparation and is still in progress.
    pub fn cancel_preset_preparation(&mut self, id: u64) {
        if let Some(entry) = self
            .preset_preparation
            .as_mut()
            .filter(|entry| entry.id == id)
        {
            entry.invalidate("QC preset preparation was cancelled".into());
        }
    }

    pub(in crate::device::owner) fn observe_preset_preparation(
        &mut self,
        ticket: &ObservationTicket,
        message_type: u16,
        observation: &crate::device::state_runtime::StateObservation,
    ) {
        let Some(entry) = self
            .preset_preparation
            .as_mut()
            .filter(|entry| entry.is_active())
        else {
            return;
        };
        if ticket.connection != entry.context.session.connection
            || ticket.epoch.as_ref() != Some(&entry.context.session.epoch)
        {
            return;
        }
        if entry.guards_active_preset && !entry.navigation_matches(&self.state) {
            entry.invalidate(
                "QC navigation or dirty state changed during preset preparation".into(),
            );
            return;
        }
        if entry.needs_source {
            if entry.source.source.is_some() {
                if !entry.source.matches(&self.state) {
                    entry.invalidate("QC preset source changed during preparation".into());
                    return;
                }
            } else if entry
                .source_observation
                .as_ref()
                .is_some_and(|(_, source)| !source.matches(&self.state))
            {
                entry.invalidate("QC preset source changed after its preparation readback".into());
                return;
            } else if message_type == profile::MESSAGE_TYPE_RECALL_PRESET
                && observation
                    .states
                    .iter()
                    .any(|state| state.kind == "preset")
            {
                if entry.accepts_source_candidate(ticket) {
                    entry.source_observation =
                        Some((ticket.clone(), SourceCheckpoint::capture(&self.state)));
                }
            } else if matches!(
                message_type,
                profile::MESSAGE_TYPE_GRID | profile::MESSAGE_TYPE_GRID_MOVE
            ) || observation.states.iter().any(|state| {
                !matches!(
                    state.kind.as_str(),
                    "position" | "master" | "tempo" | "ioPorts"
                )
            }) {
                entry
                    .invalidate("QC source state changed while acquiring a complete preset".into());
                return;
            }
        }
        if observation.file_message.is_none() {
            return;
        }
        let Some(time) = ticket.ingress else {
            return;
        };
        if ticket.sequence <= entry.last_sequence
            || time.first_report_at_ms > time.last_report_at_ms
            || entry.deadline.remaining_ms(time.last_report_at_ms) == 0
        {
            return;
        }
        // Quiet collection is only a scheduling heuristic. It does not turn
        // omitted records into empty slots or establish a directory snapshot.
        entry.last_file_ms = Some(entry.last_file_ms.unwrap_or(0).max(time.last_report_at_ms));
    }

    /// Advances preparation `id` at `now_ms` and returns the next action.
    ///
    /// Fails if `id` is no longer the active preparation, or if preparation has failed, for example because its deadline expired, the session changed or the device state it depends on changed.
    pub fn advance_preset_preparation(
        &mut self,
        id: u64,
        now_ms: u64,
    ) -> Result<OwnedPresetPreparationAction, String> {
        if self
            .preset_preparation
            .as_ref()
            .is_none_or(|entry| entry.id != id)
        {
            return Err("QC preset preparation identity is no longer active".into());
        }
        let mut entry = self
            .preset_preparation
            .take()
            .expect("validated preparation");
        let result = self.advance_owned_preset_preparation(&mut entry, now_ms);
        if let Err(error) = &result {
            entry.invalidate(error.clone());
        }
        self.preset_preparation = Some(entry);
        result
    }

    fn advance_owned_preset_preparation(
        &mut self,
        entry: &mut PresetPreparationEntry,
        now_ms: u64,
    ) -> Result<OwnedPresetPreparationAction, String> {
        match &entry.status {
            PreparationStatus::Failed(error) => return Err(error.clone()),
            PreparationStatus::Ready(workflow_id) => {
                return Ok(OwnedPresetPreparationAction::Ready {
                    workflow_id: *workflow_id,
                });
            }
            PreparationStatus::Active => {}
        }
        if !self.accepts_fence(entry.context.session_fence()) {
            return Err("QC session changed during preset preparation".into());
        }
        if entry.deadline.remaining_ms(now_ms) == 0 {
            return Err("QC preset preparation deadline expired".into());
        }
        if entry.guards_active_preset && !entry.navigation_matches(&self.state) {
            return Err("QC navigation or dirty state changed during preset preparation".into());
        }
        if entry.needs_source {
            if entry.source.source.is_none() {
                if entry.source_deadline.remaining_ms(now_ms) == 0 {
                    return Err("QC complete-source acquisition expired".into());
                }
                if let Some((ticket, source)) = &entry.source_observation {
                    if ticket
                        .ingress
                        .is_some_and(|time| time.last_report_at_ms <= now_ms)
                        && source.matches(&self.state)
                    {
                        entry.source = source.clone();
                    }
                }
                if entry.source.source.is_none() && !entry.source_requested {
                    entry.source_requested = true;
                    let mut context = entry.context.clone();
                    let source_deadline = entry.source_deadline;
                    context.deadline = Some(source_deadline);
                    return Ok(OwnedPresetPreparationAction::Read {
                        method: "device.currentPreset".into(),
                        params: Value::Null,
                        context: Box::new(context),
                        remaining_ms: source_deadline.remaining_ms(now_ms),
                    });
                }
            }
            if entry.source.source.is_some() && !entry.source.matches(&self.state) {
                return Err("QC preset source changed during preparation".into());
            }
        }
        self.state.native_preset_catalog().health()?;
        let retained = self.catalog_observations.retained_catalog(
            |ticket| {
                ticket.connection == entry.context.session.connection
                    && ticket.epoch.as_ref() == Some(&entry.context.session.epoch)
            },
            now_ms,
        );
        let Some((folders, last)) = retained else {
            return Ok(OwnedPresetPreparationAction::Wait {
                until_ms: now_ms
                    .saturating_add(100)
                    .min(entry.deadline.expires_at_ms()),
            });
        };
        entry.folders = folders
            .into_iter()
            .filter(|(key, _)| entry.full_directory || entry.required_keys.contains(key))
            .collect();
        entry.last_file_ms = Some(entry.last_file_ms.unwrap_or(0).max(last));
        let have_folders = !entry.folders.is_empty()
            && entry
                .required_keys
                .iter()
                .all(|key| entry.folders.contains_key(key));
        let quiet_until = entry.last_file_ms.map(|last| {
            last.saturating_add(if entry.full_directory {
                profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS
            } else {
                0
            })
        });
        if have_folders
            && entry.has_required_records()
            && quiet_until.is_some_and(|end| now_ms >= end)
            && (!entry.needs_source || entry.source.source.is_some())
        {
            let mut library = PresetLibrary::default();
            for folder in entry.folders.values() {
                library.ingest(folder.clone());
            }
            let plan = plan_preset_mutation(
                &entry.method,
                &entry.params,
                Some(&entry.original),
                &library,
            )?;
            let budget = crate::device::preset_workflow::PresetWorkflowBudget::for_plan(&plan)?;
            let total = entry
                .preparation_ms
                .saturating_add(budget.execution_ms)
                .saturating_add(budget.restoration_ms);
            if total
                > entry
                    .context
                    .deadline
                    .expect("retained ceiling")
                    .expires_at_ms()
                    .saturating_sub(entry.started_ms)
            {
                return Err("Concrete preset plan exceeded its original family budget".into());
            }
            let workflow_id = self.install_preset_workflow(
                &entry.context,
                plan,
                entry.started_ms,
                entry.preparation_ms,
            )?;
            self.preset_workflow
                .as_mut()
                .expect("installed workflow")
                .catalog_guard = Some(CatalogGuard::new(
                &entry.folders,
                &entry.required_keys,
                entry.full_directory,
            ));
            entry.status = PreparationStatus::Ready(workflow_id);
            return Ok(OwnedPresetPreparationAction::Ready { workflow_id });
        }
        if have_folders {
            return Ok(OwnedPresetPreparationAction::Wait {
                until_ms: quiet_until
                    .unwrap_or(now_ms)
                    .max(now_ms.saturating_add(100))
                    .min(entry.deadline.expires_at_ms()),
            });
        }
        Ok(OwnedPresetPreparationAction::Wait {
            until_ms: now_ms
                .saturating_add(100)
                .min(entry.deadline.expires_at_ms()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::session::SessionDriver;
    use crate::proto::{cortex_protobuf_v2 as pa, BinaryPreset};
    use prost::Message;
    use serde_json::json;
    const KEY: &str = "/media/p4/Presets/Test";
    const DEST: &str = "/media/p4/Presets/Destination";

    fn publish(
        owner: &mut SessionRuntime,
        kind: u16,
        payload: Vec<u8>,
        first: Option<u64>,
        last: u64,
    ) {
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                first.map(|first_report_at_ms| crate::device::transport::IngressTime {
                    first_report_at_ms,
                    last_report_at_ms: last,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, kind, &payload)
            .unwrap()
            .unwrap();
    }
    fn preset(owner: &mut SessionRuntime, sparse: bool, time: u64) {
        let payload = if sparse {
            pa::GridMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::grid_message::Preset::Preset(BinaryPreset::default())),
                ..Default::default()
            }
            .encode_to_vec()
        } else {
            pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(
                    BinaryPreset::default(),
                )),
                ..Default::default()
            }
            .encode_to_vec()
        };
        publish(
            owner,
            if sparse {
                profile::MESSAGE_TYPE_GRID
            } else {
                profile::MESSAGE_TYPE_RECALL_PRESET
            },
            payload,
            Some(time),
            time,
        );
    }
    fn position(owner: &mut SessionRuntime, pos: u32, time: u64) {
        publish(
            owner,
            profile::MESSAGE_TYPE_SETLIST_POSITION,
            pa::SetlistPositionMessage {
                action: pa::message_action::Enum::Update as i32,
                folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                    KEY.into(),
                )),
                position: Some(pa::setlist_position_message::Position::Position(pos)),
                ..Default::default()
            }
            .encode_to_vec(),
            Some(time),
            time,
        );
    }
    fn owner(sparse: bool) -> SessionRuntime {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        preset(&mut owner, false, 0);
        position(&mut owner, 1, 0);
        if sparse {
            preset(&mut owner, true, 0);
        }
        assert!(owner.state.snapshot().has_preset);
        owner
    }
    /// Same fixture as `owner`, under a name a test can use after it has bound a local called `owner`.
    fn owner_fixture(sparse: bool) -> SessionRuntime {
        owner(sparse)
    }
    fn folder(owner: &mut SessionRuntime, key: &str, name: &str, first: Option<u64>, last: u64) {
        publish(
            owner,
            profile::MESSAGE_TYPE_FILE,
            pa::FileMessage {
                action: pa::message_action::Enum::Create as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(key.into())),
                    name: Some(pa::folder_info::Name::Name(
                        key.rsplit('/').next().unwrap().into(),
                    )),
                    files: (0..256)
                        .map(|index| pa::ProductData {
                            key: (index == 1)
                                .then(|| pa::product_data::Key::Key(format!("{key}/source"))),
                            name: (index == 1).then(|| pa::product_data::Name::Name(name.into())),
                            instrument: (index == 1)
                                .then_some(pa::product_data::Instrument::Instrument(0)),
                            index: Some(pa::product_data::Index::Index(index)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec(),
            first,
            last,
        );
    }
    fn delete_folder(owner: &mut SessionRuntime, key: &str, now: u64) {
        publish(
            owner,
            profile::MESSAGE_TYPE_FILE,
            pa::FileMessage {
                action: pa::message_action::Enum::Delete as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(key.into())),
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec(),
            Some(now),
            now,
        );
    }

    #[test]
    fn missing_instrument_uses_desktop_default_for_preparation_without_forging_presence() {
        let mut owner = owner(false);
        publish(
            &mut owner,
            profile::MESSAGE_TYPE_FILE,
            pa::FileMessage {
                action: pa::message_action::Enum::Create as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(KEY.into())),
                    files: vec![pa::ProductData {
                        key: Some(pa::product_data::Key::Key(format!("{KEY}/source"))),
                        index: Some(pa::product_data::Index::Index(1)),
                        name: Some(pa::product_data::Name::Name("Source".into())),
                        instrument: None,
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec(),
            Some(1),
            1,
        );
        let id = owner
            .begin_preset_preparation(
                "device.copyPreset",
                json!({
                    "sourceSetlistKey": KEY, "sourcePosition": 1, "sourceName": "Source",
                    "destinationSetlistKey": KEY, "destinationPosition": 2,
                    "confirmOverwrite": true
                }),
                10,
            )
            .unwrap();
        assert!(matches!(
            owner.advance_preset_preparation(id, 11).unwrap(),
            OwnedPresetPreparationAction::Ready { .. }
        ));
        let row = owner
            .state
            .native_preset_catalog()
            .folder(KEY)
            .unwrap()
            .files
            .iter()
            .find(|file| file.position == 1)
            .unwrap();
        assert_eq!(row.instrument, 0);
        assert!(
            !row.instrument_present,
            "a desktop planning default is not observed device metadata"
        );
    }
    fn save(owner: &SessionRuntime) -> Value {
        json!({"setlistKey":KEY,"position":2,"name":"Saved","expectedPresetIdentity":owner.caller_preset_identity().unwrap(),"expectedPresetName":"Unsaved","expectedPosition":1,"confirmOverwrite":false})
    }
    fn ready(owner: &mut SessionRuntime, id: u64, now: u64) -> u64 {
        let OwnedPresetPreparationAction::Ready { workflow_id } =
            owner.advance_preset_preparation(id, now).unwrap()
        else {
            panic!("expected prepared workflow");
        };
        workflow_id
    }

    #[test]
    fn recased_position_echo_does_not_cancel_owned_preset_preparation() {
        for different_address in [false, true] {
            let mut owner = owner(false);
            let id = owner
                .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
                .unwrap();
            let key = if different_address {
                "/media/p4/Presets/Other"
            } else {
                "/MEDIA/p4/Presets/TEST/"
            };
            publish(
                &mut owner,
                profile::MESSAGE_TYPE_SETLIST_POSITION,
                pa::SetlistPositionMessage {
                    action: pa::message_action::Enum::Update as i32,
                    folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                        key.into(),
                    )),
                    position: Some(pa::setlist_position_message::Position::Position(1)),
                    ..Default::default()
                }
                .encode_to_vec(),
                Some(11),
                11,
            );
            folder(&mut owner, KEY, "Unsaved", Some(12), 12);
            if different_address {
                assert!(owner.advance_preset_preparation(id, 13).is_err());
                assert!(owner.preset_workflow.is_none());
            } else {
                let workflow_id = ready(&mut owner, id, 13);
                assert!(owner
                    .advance_preset_workflow(workflow_id, 14)
                    .unwrap()
                    .operation
                    .is_some());
            }
        }
    }
    #[test]
    fn startup_catalog_plans_without_a_second_read_and_cannot_cross_sessions() {
        let mut owner = owner(false);
        folder(&mut owner, KEY, "Unsaved", Some(0), 0);
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        let workflow = ready(&mut owner, id, 11);
        assert!(owner
            .advance_preset_workflow(workflow, 12)
            .unwrap()
            .operation
            .is_some());
        owner.opened(13).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 13));
        preset(&mut owner, false, 14);
        position(&mut owner, 1, 14);
        let next = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 15)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_preparation(next, 16).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
    }

    fn sparse_folder(
        owner: &mut SessionRuntime,
        files: Vec<pa::ProductData>,
        first: Option<u64>,
        last: u64,
    ) {
        publish(
            owner,
            profile::MESSAGE_TYPE_FILE,
            pa::FileMessage {
                action: pa::message_action::Enum::Create as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(KEY.into())),
                    files,
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec(),
            first,
            last,
        );
    }

    #[test]
    fn an_unobserved_destination_is_not_a_writable_empty_slot() {
        let mut owner = owner(false);
        sparse_folder(
            &mut owner,
            vec![pa::ProductData {
                index: Some(pa::product_data::Index::Index(1)),
                key: Some(pa::product_data::Key::Key("source".into())),
                name: Some(pa::product_data::Name::Name("Unsaved".into())),
                instrument: Some(pa::product_data::Instrument::Instrument(0)),
                ..Default::default()
            }],
            Some(0),
            0,
        );
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_preparation(id, 11).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        sparse_folder(
            &mut owner,
            vec![pa::ProductData {
                index: Some(pa::product_data::Index::Index(2)),
                ..Default::default()
            }],
            Some(12),
            12,
        );
        ready(&mut owner, id, 13);
    }

    #[test]
    fn partial_updates_cannot_repair_unknown_provenance_of_inherited_metadata() {
        let mut owner = owner(false);
        folder(&mut owner, KEY, "Unsaved", None, 0);
        sparse_folder(
            &mut owner,
            vec![pa::ProductData {
                index: Some(pa::product_data::Index::Index(1)),
                key: Some(pa::product_data::Key::Key("source".into())),
                name: Some(pa::product_data::Name::Name("Unsaved".into())),
                ..Default::default()
            }],
            Some(1),
            1,
        );
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_preparation(id, 11).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
    }

    #[test]
    fn unknown_ingress_revokes_prepared_catalog_operands() {
        let mut owner = owner(false);
        folder(&mut owner, KEY, "Unsaved", Some(0), 0);
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        let workflow = ready(&mut owner, id, 11);
        let action = owner.advance_preset_workflow(workflow, 12).unwrap();
        folder(&mut owner, KEY, "Unsaved", None, 13);
        assert!(owner
            .prepare_dispatch(action.operation.as_ref(), None, 14)
            .is_err());
    }

    #[test]
    fn preparation_keeps_original_session_and_rejects_navigation_even_after_return() {
        for replacement in [false, true] {
            let mut owner = owner(false);
            let id = owner
                .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
                .unwrap();
            let context = owner.preset_preparation.as_ref().unwrap().context.clone();
            assert!(matches!(
                owner.advance_preset_preparation(id, 11).unwrap(),
                OwnedPresetPreparationAction::Wait { .. }
            ));
            if replacement {
                owner.opened(12).unwrap();
                owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 12));
                assert!(owner
                    .begin_gateway_read_scope_for(&context, 13, 999_999)
                    .is_err());
            } else {
                position(&mut owner, 9, 12);
                position(&mut owner, 1, 13);
            }
            folder(&mut owner, KEY, "Unsaved", Some(14), 14);
            assert!(owner.advance_preset_preparation(id, 15).is_err());
            assert!(owner.preset_workflow.is_none());
        }
    }

    #[test]
    fn copy_uses_both_retained_folders_and_native_content_not_cache_edits() {
        let mut owner = owner(false);
        folder(&mut owner, DEST, "Stale", Some(0), 0);
        let args = json!({"sourceSetlistKey":KEY,"sourcePosition":1,"sourceName":"Source","destinationSetlistKey":DEST,"destinationPosition":2,"confirmOverwrite":true});
        let id = owner
            .begin_preset_preparation("device.copyPreset", args, 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Source", Some(12), 12);
        // A presentation edit must not change the native source name used by planning.
        let mut cached = owner
            .state
            .preset_library()
            .observed_listing(KEY)
            .unwrap()
            .clone();
        cached
            .files
            .iter_mut()
            .find(|file| file.position == 1)
            .unwrap()
            .name = "Cache-only replacement".into();
        owner.state.ingest_preset_listing(cached);
        folder(&mut owner, DEST, "Different", Some(14), 14);
        let workflow_id = ready(&mut owner, id, 15);
        let workflow = owner.preset_workflow.as_ref().unwrap();
        assert_eq!(workflow.id, workflow_id);
        assert_eq!(workflow.runtime.plan().saved_name, "Source");
        // One File COPY frame naming the native source key; no recall, no
        // Save and no cleanup stage.
        assert_eq!(workflow.runtime.plan().stages.len(), 1);
        assert!(workflow.runtime.plan().cleanup_stages.is_empty());
        assert!(matches!(&workflow.runtime.plan().stages[0].write,
            PlannedWrite::HidOperation(crate::commands::DeviceOperation::CopyPreset {
                source_setlist_key, source_product_key, destination_setlist_key, destination_position: 2, ..
            }) if source_setlist_key == KEY
                && source_product_key == &format!("{KEY}/source")
                && destination_setlist_key == DEST));
        assert_eq!(ready(&mut owner, id, 16), workflow_id);
    }

    #[test]
    fn sparse_source_is_acquired_once_under_the_original_deadline() {
        let mut owner = owner(true);
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        let OwnedPresetPreparationAction::Read {
            method,
            context,
            remaining_ms,
            ..
        } = owner.advance_preset_preparation(id, 20).unwrap()
        else {
            panic!("expected source read");
        };
        assert_eq!(method, "device.currentPreset");
        assert_eq!(remaining_ms, 14_990);
        assert_eq!(
            owner
                .begin_gateway_read_scope_for(&context, 30, 99_999)
                .unwrap()
                .remaining_ms(30),
            14_980
        );
        assert!(matches!(
            owner.advance_preset_preparation(id, 21).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        folder(&mut owner, KEY, "Unsaved", Some(22), 22);
        assert!(matches!(
            owner.advance_preset_preparation(id, 23).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        preset(&mut owner, false, 24);
        let workflow_id = ready(&mut owner, id, 25);
        assert!(owner
            .advance_preset_workflow(workflow_id, 26)
            .unwrap()
            .operation
            .is_some());
    }

    #[test]
    fn source_readback_cannot_adopt_an_edit_or_arrive_after_its_deadline() {
        for case in 0..3 {
            let mut owner = owner(true);
            let id = owner
                .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
                .unwrap();
            owner.advance_preset_preparation(id, 11).unwrap();
            owner.advance_preset_preparation(id, 12).unwrap();
            folder(&mut owner, KEY, "Unsaved", Some(13), 13);
            if case == 0 {
                preset(&mut owner, true, 14);
            }
            preset(&mut owner, false, if case == 1 { 15_010 } else { 15 });
            let now = if case == 0 { 16 } else { 15_011 };
            assert!(owner.advance_preset_preparation(id, now).is_err());
            assert!(owner.preset_workflow.is_none());
        }
    }

    #[test]
    fn catalog_proof_requires_ordered_native_ingress_and_timely_evaluation() {
        for (first, last, now) in [
            (None, 12, 13),
            (Some(13), 12, 14),
            (Some(12), 20, 13),
            (Some(12), 12, 45_010),
        ] {
            let mut owner = owner(false);
            let id = owner
                .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
                .unwrap();
            owner.advance_preset_preparation(id, 11).unwrap();
            folder(&mut owner, KEY, "Unsaved", first, last);
            assert!(!matches!(
                owner.advance_preset_preparation(id, now),
                Ok(OwnedPresetPreparationAction::Ready { .. })
            ));
            assert!(owner.preset_workflow.is_none());
        }
    }

    #[test]
    fn directory_preparation_waits_for_quiet_and_ignores_other_providers_as_candidates() {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.createSetlist", json!({"name":"New"}), 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Unsaved", Some(12), 12);
        folder(
            &mut owner,
            "/media/p4/Captures/Other",
            "Capture",
            Some(100),
            100,
        );
        assert!(matches!(
            owner.advance_preset_preparation(id, 349).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        let workflow_id = ready(&mut owner, id, 350);
        let plan = owner.preset_workflow.as_ref().unwrap().runtime.plan();
        assert_eq!(
            plan.folder_copy.as_ref().unwrap().before_folder_keys,
            vec![KEY.to_string()]
        );
        assert!(owner
            .advance_preset_workflow(workflow_id, 351)
            .unwrap()
            .operation
            .is_some());
    }

    #[test]
    fn cancellation_and_overlapping_preparations_cannot_replace_the_active_request() {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        assert!(owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 11)
            .is_err());
        owner.cancel_preset_preparation(id + 1);
        assert!(owner.advance_preset_preparation(id, 12).is_ok());
        owner.cancel_preset_preparation(id);
        assert!(owner.advance_preset_preparation(id, 13).is_err());
        let replacement = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 14)
            .unwrap();
        assert_ne!(id, replacement);
        assert!(owner.advance_preset_preparation(id, 15).is_err());
    }

    #[test]
    fn all_mutation_families_refine_the_original_budget_without_renewing_time() {
        let fixture = owner(false);
        let cases = [
            ("device.savePresetAs", save(&fixture)),
            (
                "device.renameCurrentPreset",
                json!({"name":"Renamed","expectedPresetIdentity":fixture.caller_preset_identity().unwrap(),"expectedPresetName":"Unsaved","expectedPosition":1,"confirmRename":true}),
            ),
            (
                "device.copyPreset",
                json!({"sourceSetlistKey":KEY,"sourcePosition":1,"sourceName":"Source","destinationSetlistKey":DEST,"destinationPosition":2,"confirmOverwrite":true}),
            ),
            ("device.createSetlist", json!({"name":"New"})),
            (
                "device.duplicateSetlist",
                json!({"sourceSetlistKey":KEY,"expectedSourceName":"Test"}),
            ),
            (
                "device.movePreset",
                json!({"setlistKey":KEY,"sourcePosition":1,"destinationPosition":2,"sourceName":"Source"}),
            ),
            (
                "device.deletePreset",
                json!({"setlistKey":KEY,"position":1,"expectedName":"Source"}),
            ),
        ];
        for (method, args) in cases {
            let mut owner = owner(false);
            let id = owner.begin_preset_preparation(method, args, 10).unwrap();
            let entry = owner.preset_preparation.as_ref().unwrap();
            let original_ceiling = entry.context.deadline.unwrap().expires_at_ms();
            let preparation_ms = entry.preparation_ms;
            owner.advance_preset_preparation(id, 1_000).unwrap();
            folder(&mut owner, KEY, "Source", Some(1_001), 1_001);
            if method == "device.copyPreset" {
                folder(&mut owner, DEST, "Other", Some(1_002), 1_002);
            }
            let workflow_id = ready(&mut owner, id, 1_500);
            let entry = owner.preset_workflow.as_ref().unwrap();
            let budget = crate::device::preset_workflow::PresetWorkflowBudget::for_plan(
                entry.runtime.plan(),
            )
            .unwrap();
            let concrete_end = 10 + preparation_ms + budget.execution_ms + budget.restoration_ms;
            assert!(concrete_end <= original_ceiling, "{method}");
            // Polling late does not add the concrete allowance to the late clock.
            let step = owner
                .advance_preset_workflow(workflow_id, concrete_end + 1)
                .unwrap();
            assert!(
                step.operation.is_none(),
                "{method} renewed its native allowance"
            );
        }
    }

    #[test]
    fn directory_capacity_exhaustion_cannot_authorize_a_truncated_baseline() {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.createSetlist", json!({"name":"New"}), 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        for i in 0..128 {
            folder(
                &mut owner,
                &format!("/media/p4/Presets/Folder{i}"),
                "Source",
                Some(12 + i),
                12 + i,
            );
        }
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(
                    "/media/p4/Presets/Overflow".into(),
                )),
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: 200,
                    last_report_at_ms: 200,
                }),
            )
            .unwrap();
        assert!(owner
            .observe_state(&ticket, 4, &payload)
            .unwrap_err()
            .contains("capacity"));
        assert!(owner
            .advance_preset_preparation(id, 500)
            .unwrap_err()
            .contains("capacity"));
        assert!(owner.preset_workflow.is_none());
    }

    #[test]
    fn copy_sends_one_file_frame_and_never_recalls_or_depends_on_the_active_preset() {
        let mut owner = owner(false);
        position(&mut owner, 0, 1);
        let args = json!({"sourceSetlistKey":KEY,"sourcePosition":1,"sourceName":"Source","destinationSetlistKey":DEST,"destinationPosition":2,"confirmOverwrite":true});
        let id = owner
            .begin_preset_preparation("device.copyPreset", args, 10)
            .unwrap();
        // No type-15 source read is ever requested: COPY names its source by
        // file key, so preparation only waits for the File catalog.
        assert!(!owner.preset_preparation.as_ref().unwrap().needs_source);
        assert!(
            !owner
                .preset_preparation
                .as_ref()
                .unwrap()
                .guards_active_preset
        );
        assert!(!matches!(
            owner.advance_preset_preparation(id, 11).unwrap(),
            OwnedPresetPreparationAction::Read { .. }
        ));
        folder(&mut owner, KEY, "Source", Some(12), 12);
        folder(&mut owner, DEST, "Other", Some(13), 13);
        let workflow_id = ready(&mut owner, id, 14);
        // An edit to the active preset, or navigation away from it, after
        // preparation neither blocks nor changes the copy: it does not touch
        // the active preset.
        owner.state.snapshot_mut().dirty = true;
        let step = owner.advance_preset_workflow(workflow_id, 15).unwrap();
        assert!(matches!(
            step.action,
            PresetWorkflowAction::Dispatch {
                write: PlannedWrite::HidOperation(crate::commands::DeviceOperation::CopyPreset {
                    destination_position: 2,
                    ..
                }),
                cleanup: false,
                ..
            }
        ));
        let dispatch = owner
            .prepare_dispatch(step.operation.as_ref(), None, 16)
            .unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, profile::MESSAGE_TYPE_FILE, 1, 16)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        assert!(owner.begin_native_report(&delivery, 0, 17).is_ok());
    }

    #[test]
    fn rename_reads_no_source_but_keeps_the_active_preset_guard_as_host_policy() {
        let mut owner = owner(false);
        let args = json!({"name":"Renamed","expectedPresetIdentity":owner.caller_preset_identity().unwrap(),"expectedPresetName":"Unsaved","expectedPosition":1,"confirmRename":true});
        let id = owner
            .begin_preset_preparation("device.renameCurrentPreset", args.clone(), 10)
            .unwrap();
        // Host policy (not a Cortex Control frame): the expected-state guard
        // stays, because rename addresses the active preset.
        assert!(!owner.preset_preparation.as_ref().unwrap().needs_source);
        assert!(
            owner
                .preset_preparation
                .as_ref()
                .unwrap()
                .guards_active_preset
        );
        assert!(!matches!(
            owner.advance_preset_preparation(id, 11).unwrap(),
            OwnedPresetPreparationAction::Read { .. }
        ));
        folder(&mut owner, KEY, "Unsaved", Some(12), 12);
        let workflow_id = ready(&mut owner, id, 13);
        let step = owner.advance_preset_workflow(workflow_id, 14).unwrap();
        assert!(matches!(
            step.action,
            PresetWorkflowAction::Dispatch {
                write: PlannedWrite::HidOperation(crate::commands::DeviceOperation::RenamePreset {
                    ref name,
                    ..
                }),
                cleanup: false,
                ..
            } if name == "Renamed"
        ));

        // A stale expected position is still refused before anything is sent.
        let mut stale = args.clone();
        stale["expectedPosition"] = json!(7);
        let mut other = owner_fixture(false);
        assert!(other
            .begin_preset_preparation("device.renameCurrentPreset", stale, 10)
            .is_err());

        // An edit made to the active preset while the rename is being
        // prepared aborts it: the dirty refusal is evaluated on the state the
        // caller guarded, not on a later one.
        let mut edited = owner_fixture(false);
        let args = json!({"name":"Renamed","expectedPresetIdentity":edited.caller_preset_identity().unwrap(),"expectedPresetName":"Unsaved","expectedPosition":1,"confirmRename":true});
        let id = edited
            .begin_preset_preparation("device.renameCurrentPreset", args, 10)
            .unwrap();
        folder(&mut edited, KEY, "Unsaved", Some(12), 12);
        edited.state.snapshot_mut().dirty = true;
        assert!(edited.advance_preset_preparation(id, 13).is_err());
    }

    #[test]
    fn incomplete_startup_catalog_waits_without_reads_or_renewing_its_deadline() {
        let mut owner = owner(false);
        let args = json!({"sourceSetlistKey":KEY,"sourcePosition":1,"sourceName":"Source","destinationSetlistKey":DEST,"destinationPosition":2,"confirmOverwrite":true});
        let id = owner
            .begin_preset_preparation("device.copyPreset", args, 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Source", Some(12), 12);
        let quiet = 12 + profile::PRESET_LIBRARY_SETTLEMENT_QUIET_MS;
        assert!(matches!(
            owner.advance_preset_preparation(id, quiet).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        let retry = quiet + CatalogVerificationPolicy::default().maximum_retry_ms;
        assert!(matches!(
            owner.advance_preset_preparation(id, retry - 1).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        assert!(matches!(
            owner.advance_preset_preparation(id, retry).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        assert!(owner
            .preset_preparation
            .as_ref()
            .unwrap()
            .folders
            .contains_key(KEY));
        let end = owner
            .preset_preparation
            .as_ref()
            .unwrap()
            .deadline
            .expires_at_ms();
        assert!(owner
            .advance_preset_preparation(id, end)
            .unwrap_err()
            .contains("deadline"));
    }

    #[test]
    fn queued_save_rejects_changed_catalog_operands_and_cannot_revive_when_they_return() {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Source", Some(12), 12);
        let workflow_id = ready(&mut owner, id, 13);
        let step = owner.advance_preset_workflow(workflow_id, 14).unwrap();
        let dispatch = owner
            .prepare_dispatch(step.operation.as_ref(), None, 15)
            .unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, profile::MESSAGE_TYPE_FILE, 1, 15)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        folder(&mut owner, KEY, "Changed", Some(16), 16);
        assert!(owner.begin_native_report(&delivery, 0, 17).is_err());
        assert!(owner
            .delivery_record(&delivery)
            .unwrap()
            .started_at_ms
            .is_none());
        folder(&mut owner, KEY, "Source", Some(18), 18);
        assert!(owner.begin_native_report(&delivery, 0, 19).is_err());
        assert!(matches!(
            owner
                .advance_preset_workflow(workflow_id, 20)
                .unwrap()
                .action,
            PresetWorkflowAction::Finished(_)
        ));
        let result = owner.preset_workflow_result(workflow_id).unwrap().unwrap();
        assert_eq!(result.persistent_delivery, PersistentDelivery::NotAttempted);
        assert!(result
            .gateway_result()
            .unwrap_err()
            .contains("No persistent USB write was attempted"));
    }

    #[test]
    fn destination_catalog_change_aborts_copy_before_its_frame_and_plans_no_cleanup() {
        let mut owner = owner(false);
        let args = json!({"sourceSetlistKey":KEY,"sourcePosition":1,"sourceName":"Source","destinationSetlistKey":DEST,"destinationPosition":2,"confirmOverwrite":true});
        let id = owner
            .begin_preset_preparation("device.copyPreset", args, 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Source", Some(12), 12);
        folder(&mut owner, DEST, "Other", Some(13), 13);
        let workflow_id = ready(&mut owner, id, 14);
        folder(&mut owner, DEST, "Replacement", Some(15), 15);
        let rejected = owner.advance_preset_workflow(workflow_id, 16).unwrap();
        assert!(rejected.operation.is_none());
        // Copy never recalled anything, so there is no navigation to restore:
        // the workflow simply finishes with nothing sent.
        let finished = owner.advance_preset_workflow(workflow_id, 17).unwrap();
        assert!(finished.operation.is_none());
        assert!(matches!(
            finished.action,
            PresetWorkflowAction::Finished(crate::device::preset_workflow::PresetWorkflowOutcome {
                primary_error: Some(_),
                restoration: crate::device::preset_workflow::RestorationOutcome::NotRequired,
                ..
            })
        ));
    }

    #[test]
    fn presentation_cache_edits_do_not_replace_or_revoke_native_catalog_operands() {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.savePresetAs", save(&owner), 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Source", Some(12), 12);
        let workflow_id = ready(&mut owner, id, 13);
        let mut cached = owner
            .state
            .preset_library()
            .observed_listing(KEY)
            .unwrap()
            .clone();
        cached
            .files
            .iter_mut()
            .find(|file| file.position == 1)
            .unwrap()
            .name = "Cache-only".into();
        owner.state.ingest_preset_listing(cached);
        let step = owner.advance_preset_workflow(workflow_id, 14).unwrap();
        let dispatch = owner
            .prepare_dispatch(step.operation.as_ref(), None, 15)
            .unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, profile::MESSAGE_TYPE_FILE, 1, 15)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, 16).unwrap();
        // Its own post-attempt catalog echo must not revoke an in-flight frame.
        folder(&mut owner, KEY, "Saved", Some(17), 17);
        assert!(owner
            .validate_operation(step.operation.as_ref().unwrap(), 18)
            .is_ok());
        owner
            .finish_native_report(
                &delivery,
                0,
                crate::device::delivery::ReportOutcome::Delivered,
                19,
            )
            .unwrap();
        assert!(matches!(
            owner
                .advance_preset_workflow(workflow_id, 20)
                .unwrap()
                .action,
            PresetWorkflowAction::Verify { .. }
        ));
    }

    #[test]
    fn catalog_confirmation_does_not_replace_unrelated_presentation_folders() {
        let mut owner = owner(false);
        let stale = "/media/p4/Presets/Stale";
        folder(&mut owner, stale, "Old", Some(0), 0);
        let id = owner
            .begin_preset_preparation("device.createSetlist", json!({"name":"New"}), 10)
            .unwrap();
        owner.advance_preset_preparation(id, 11).unwrap();
        folder(&mut owner, KEY, "Source", Some(12), 12);
        let workflow_id = ready(&mut owner, id, 262);
        assert!(owner
            .state
            .preset_folders()
            .iter()
            .any(|folder| folder.key == stale));
        // Preparation and confirmation both preserve unrelated native folders.
        owner.state.preset_library_mut().ensure_setlist(stale);
        let step = owner.advance_preset_workflow(workflow_id, 263).unwrap();
        let dispatch = owner
            .prepare_dispatch(step.operation.as_ref(), None, 264)
            .unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, profile::MESSAGE_TYPE_FILE, 1, 264)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, 265).unwrap();
        owner
            .finish_native_report(
                &delivery,
                0,
                crate::device::delivery::ReportOutcome::Delivered,
                266,
            )
            .unwrap();
        assert!(matches!(
            owner
                .advance_preset_workflow(workflow_id, 267)
                .unwrap()
                .action,
            PresetWorkflowAction::VerifyCatalog { .. }
        ));
        folder(
            &mut owner,
            "/media/p4/Presets/New",
            "New preset",
            Some(268),
            268,
        );
        folder(&mut owner, KEY, "Source", Some(269), 269);
        assert!(matches!(
            owner
                .advance_preset_workflow(workflow_id, 519)
                .unwrap()
                .action,
            PresetWorkflowAction::Finished(_)
        ));
        assert!(owner
            .state
            .preset_folders()
            .iter()
            .any(|folder| folder.key == stale));
        let value = owner
            .preset_workflow_result(workflow_id)
            .unwrap()
            .unwrap()
            .gateway_result()
            .unwrap();
        assert_eq!(value["setlistKey"], "/media/p4/Presets/New");
        assert_eq!(value["verified"], true);
    }
    fn prepared_deletion() -> (SessionRuntime, u64) {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.deleteSetlist", json!({"name":"Delete"}), 0)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_preparation(id, 0).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        folder(&mut owner, KEY, "Source", Some(1), 1);
        folder(&mut owner, "/media/p4/Presets/Other", "Other", Some(2), 2);
        folder(&mut owner, "/media/p4/Presets/Delete", "Stored", Some(3), 3);
        let workflow = ready(&mut owner, id, 253);
        (owner, workflow)
    }

    fn deliver_deletion(owner: &mut SessionRuntime, workflow: u64) {
        use crate::device::delivery::ReportOutcome;
        let action = owner.advance_preset_workflow(workflow, 254).unwrap();
        assert!(matches!(
            action.action,
            PresetWorkflowAction::Dispatch {
                write: PlannedWrite::HidOperation(
                    crate::commands::DeviceOperation::DeleteSetlist { .. }
                ),
                ..
            }
        ));
        let operation = action.operation.unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 255).unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, profile::MESSAGE_TYPE_FILE, 1, 255)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, 255).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 256)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_workflow(workflow, 257).unwrap().action,
            PresetWorkflowAction::VerifyCatalog { .. }
        ));
        // Passive first: no directory READ before the passive window has elapsed.
        assert!(matches!(
            owner
                .advance_preset_workflow(workflow, 258)
                .unwrap()
                .catalog
                .unwrap()
                .action,
            CatalogVerificationAction::Wait { .. }
        ));
        assert_eq!(
            owner
                .advance_preset_workflow(workflow, 258 + crate::device::catalog::PASSIVE_WINDOW_MS)
                .unwrap()
                .catalog
                .unwrap()
                .action,
            CatalogVerificationAction::Request
        );
        assert!(owner.prepare_dispatch(Some(&operation), None, 259).is_err());
    }

    #[test]
    fn deletion_waits_for_explicit_target_deletion_and_clears_local_edits() {
        let (mut owner, workflow) = prepared_deletion();
        let target = "/media/p4/Presets/Delete";
        let mut recreated = owner
            .state
            .preset_library()
            .observed_listing(target)
            .unwrap()
            .clone();
        recreated.files.clear();
        owner
            .state
            .preset_library_mut()
            .edit_cached_preset(target, 1, "Local edit", 0);
        deliver_deletion(&mut owner, workflow);
        folder(&mut owner, KEY, "Source", Some(259), 259);
        // A quiet interval with only one unrelated folder is incomplete.
        assert!(owner
            .advance_preset_workflow(workflow, 509)
            .unwrap()
            .catalog
            .is_some());
        assert!(owner.preset_workflow_result(workflow).unwrap().is_none());
        assert!(owner.state.preset_list(target).is_some());
        folder(
            &mut owner,
            "/media/p4/Presets/Other",
            "Other",
            Some(510),
            510,
        );
        owner.advance_preset_workflow(workflow, 760).unwrap();
        assert!(owner.preset_workflow_result(workflow).unwrap().is_none());
        delete_folder(&mut owner, target, 761);
        let action = owner.advance_preset_workflow(workflow, 1011).unwrap();
        assert!(matches!(action.action, PresetWorkflowAction::Finished(_)));
        let result = owner
            .preset_workflow_result(workflow)
            .unwrap()
            .unwrap()
            .gateway_result()
            .unwrap();
        assert_eq!(result["verified"], true);
        assert!(result.get("savedName").is_none());
        crate::device::generated_gateway::validate_result("device.deleteSetlist", &result).unwrap();
        assert!(owner.state.preset_list(target).is_none());
        owner.state.ingest_preset_listing(recreated);
        assert!(!owner
            .state
            .preset_list(target)
            .unwrap()
            .presets
            .iter()
            .any(|entry| entry.name == "Local edit"));
    }

    #[test]
    fn another_read_and_target_omission_do_not_confirm_deletion() {
        let (mut owner, workflow) = prepared_deletion();
        deliver_deletion(&mut owner, workflow);
        folder(&mut owner, KEY, "Source", Some(259), 259);
        folder(
            &mut owner,
            "/media/p4/Presets/Other",
            "Other",
            Some(260),
            260,
        );
        folder(
            &mut owner,
            "/media/p4/Presets/Delete",
            "Stored",
            Some(261),
            261,
        );
        assert!(owner
            .advance_preset_workflow(workflow, 511)
            .unwrap()
            .catalog
            .is_some());
        assert!(owner.preset_workflow_result(workflow).unwrap().is_none());
        assert_eq!(
            owner
                .advance_preset_workflow(workflow, 15511)
                .unwrap()
                .catalog
                .unwrap()
                .action,
            CatalogVerificationAction::Request
        );
        folder(&mut owner, KEY, "Source", Some(15512), 15512);
        folder(
            &mut owner,
            "/media/p4/Presets/Other",
            "Other",
            Some(15513),
            15513,
        );
        owner.advance_preset_workflow(workflow, 15763).unwrap();
        assert!(owner.preset_workflow_result(workflow).unwrap().is_none());
        delete_folder(&mut owner, "/media/p4/Presets/Delete", 15764);
        assert!(matches!(
            owner
                .advance_preset_workflow(workflow, 16014)
                .unwrap()
                .action,
            PresetWorkflowAction::Finished(_)
        ));
        assert!(owner
            .preset_workflow_result(workflow)
            .unwrap()
            .unwrap()
            .gateway_result()
            .is_ok());
    }

    #[test]
    fn deletion_rejects_active_targets_before_reads_and_before_native_attempt() {
        let mut initial = owner(false);
        for name in ["Test", "../Delete", "My Presets", " Delete"] {
            assert!(initial
                .begin_preset_preparation("device.deleteSetlist", json!({"name":name}), 0)
                .is_err());
        }
        let (mut owner, workflow) = prepared_deletion();
        let action = owner.advance_preset_workflow(workflow, 254).unwrap();
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Delete".into();
        assert!(owner
            .prepare_dispatch(action.operation.as_ref(), None, 255)
            .is_err());
        assert!(matches!(
            owner.advance_preset_workflow(workflow, 256).unwrap().action,
            PresetWorkflowAction::Finished(_)
        ));
        let error = owner
            .preset_workflow_result(workflow)
            .unwrap()
            .unwrap()
            .gateway_result()
            .unwrap_err();
        assert!(error.contains("No persistent USB write was attempted"));
    }
    #[test]
    fn deletion_cannot_publish_when_navigation_is_unsafe_after_native_delivery() {
        for active_target in [true, false] {
            let (mut owner, workflow) = prepared_deletion();
            deliver_deletion(&mut owner, workflow);
            folder(&mut owner, KEY, "Source", Some(259), 259);
            folder(
                &mut owner,
                "/media/p4/Presets/Other",
                "Other",
                Some(260),
                260,
            );
            delete_folder(&mut owner, "/media/p4/Presets/Delete", 261);
            if active_target {
                owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Delete".into();
            } else {
                owner.state.snapshot_mut().has_preset = false;
            }
            owner.advance_preset_workflow(workflow, 511).unwrap();
            let result = owner.preset_workflow_result(workflow).unwrap().unwrap();
            assert!(result.catalog.is_none());
            let error = result.gateway_result().unwrap_err();
            assert!(error.contains("reports were delivered"));
            assert!(error.contains("active navigation is unavailable"));
            assert!(owner
                .state
                .preset_library()
                .observed_listing("/media/p4/Presets/Delete")
                .is_none());
        }
    }

    #[test]
    fn deletion_cannot_publish_an_old_catalog_match_in_a_replacement_session() {
        let (mut owner, workflow) = prepared_deletion();
        deliver_deletion(&mut owner, workflow);
        folder(&mut owner, KEY, "Source", Some(259), 259);
        folder(
            &mut owner,
            "/media/p4/Presets/Other",
            "Other",
            Some(260),
            260,
        );
        delete_folder(&mut owner, "/media/p4/Presets/Delete", 261);
        owner.opened(261).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 261));
        folder(
            &mut owner,
            "/media/p4/Presets/Delete",
            "Replacement",
            Some(262),
            262,
        );
        owner.advance_preset_workflow(workflow, 512).unwrap();
        let result = owner.preset_workflow_result(workflow).unwrap().unwrap();
        assert!(result.catalog.is_none());
        assert!(result
            .gateway_result()
            .unwrap_err()
            .contains("reports were delivered"));
        assert!(owner
            .state
            .preset_library()
            .observed_listing("/media/p4/Presets/Delete")
            .is_some());
    }
    #[test]
    fn deletion_preparation_waits_for_the_known_active_folder() {
        let mut owner = owner(false);
        let id = owner
            .begin_preset_preparation("device.deleteSetlist", json!({"name":"Delete"}), 0)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_preparation(id, 0).unwrap(),
            OwnedPresetPreparationAction::Wait { .. }
        ));
        folder(&mut owner, "/media/p4/Presets/Delete", "Stored", Some(1), 1);
        assert!(!matches!(
            owner.advance_preset_preparation(id, 251).unwrap(),
            OwnedPresetPreparationAction::Ready { .. }
        ));
        folder(&mut owner, KEY, "Source", Some(252), 252);
        let workflow = ready(&mut owner, id, 502);
        assert!(matches!(
            owner.advance_preset_workflow(workflow, 503).unwrap().action,
            PresetWorkflowAction::Dispatch {
                write: PlannedWrite::HidOperation(
                    crate::commands::DeviceOperation::DeleteSetlist { .. }
                ),
                ..
            }
        ));
    }
}
