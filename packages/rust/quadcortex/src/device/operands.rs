//! State read by a native write plan, retained until the operation starts.
//! These checks concern semantic targets, not a revision of unrelated telemetry.

use crate::commands::{DeviceCommand, DeviceOperation};
use crate::device::request::{
    assert_expected_parameter, assert_expected_preset, assert_expected_state, PlannedWrite,
};
use crate::device::state_runtime::DeviceStateRuntime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Source {
    /// A model the session's catalog must list and not hide: the model of an added or replacing
    /// block, or a model being pinned. An empty catalog answers `STATE_UNAVAILABLE`.
    ModelAvailability {
        model_id: u32,
    },
    /// A Global EQ preset recall, planned against the session's retained model preset fold.
    GlobalEqPreset {
        value: String,
        is_factory: bool,
        hash: u32,
    },
    Preset,
    ActiveScene,
    PresetDirty,
    Block {
        row: u32,
        column: u32,
    },
    Grid,
    FootswitchAssignment {
        row: u32,
        column: u32,
    },
    Routing {
        row: u32,
    },
    Parameter {
        row: u32,
        column: u32,
        index: u32,
        control: Option<String>,
    },
    ModeSlots,
    PresetLoadMidi,
}

impl Source {
    fn read(&self, state: &DeviceStateRuntime) -> Value {
        let snapshot = state.snapshot();
        match self {
            Self::ModelAvailability { model_id } => {
                json!(state.assert_model_available(*model_id).is_ok())
            }
            Self::Preset => json!([
                snapshot.has_preset,
                snapshot.setlist_key.trim_end_matches('/'),
                snapshot.preset_position,
                snapshot.address_revision
            ]),
            Self::ActiveScene => json!(snapshot.active_scene),
            Self::PresetDirty => json!(snapshot.dirty),
            Self::Block { row, column } => json!(snapshot
                .blocks
                .iter()
                .find(|block| block.row == *row && block.column == *column)
                .map(|block| (&block.id, block.model_id))),
            Self::Grid => {
                let mut cells: Vec<_> = snapshot
                    .blocks
                    .iter()
                    .map(|block| (block.row, block.column, block.model_id))
                    .collect();
                cells.sort_unstable();
                json!(cells)
            }
            Self::FootswitchAssignment { row, column } => json!(snapshot
                .blocks
                .iter()
                .find(|block| block.row == *row && block.column == *column)
                .map(|block| (block.footswitch, &block.footswitch_assignment_type))),
            Self::Routing { row } => json!(snapshot
                .routes
                .iter()
                .find(|route| route.row == *row)
                .map(|route| (
                    route.input_id,
                    route.output_id,
                    route.split_column,
                    route.mix_column,
                    route.splitter_model_id
                ))),
            Self::Parameter {
                row,
                column,
                index,
                control,
            } => {
                let details = match control {
                    Some(control) => state.lane_control_details(*row, control),
                    None => state.block_details(*row, *column),
                };
                details
                    .and_then(|details| {
                        details
                            .parameters
                            .iter()
                            .find(|parameter| parameter.index == *index)
                            .map(|parameter| {
                                json!([
                                    details.model_id,
                                    details.scene,
                                    parameter.enabled,
                                    parameter.writable,
                                    parameter.scene_mode,
                                    parameter.r#type,
                                    parameter.value_scale,
                                    parameter.steps
                                ])
                            })
                    })
                    .unwrap_or(Value::Null)
            }
            Self::ModeSlots => json!(snapshot.mode_slots),
            Self::PresetLoadMidi => json!(snapshot.preset_load_midi_out),
            Self::GlobalEqPreset {
                value,
                is_factory,
                hash,
            } => json!(state
                .model_preset_catalog()
                .admits(value, *is_factory, *hash)
                .is_ok()),
        }
    }
}

/// The state a native write plan depends on, captured when the write is planned and re-checked
/// before it is admitted.
///
/// It retains the caller's `expected*` guard arguments and target coordinates, a snapshot of the
/// device state the write reads (such as the current preset address, active scene or the targeted
/// block), the expected parameter value, if any, and the external preflight observation, if the
/// method needs one. A write is refused when any of these has changed since planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchOperands {
    method: String,
    guards: Value,
    sources: Vec<(Source, Value)>,
    parameter_value: Option<(u32, u32, u32, Option<String>)>,
    external_preflight: Option<ExternalPreflight>,
}

/// A write may change its own operands, but its readback still belongs to the
/// original preset. Navigation uses its explicit destination predicate instead.
#[derive(Debug, Clone)]
pub(crate) struct PresetVerificationScope(Value);

impl PresetVerificationScope {
    pub(crate) fn validate(&self, state: &DeviceStateRuntime) -> Result<(), String> {
        if Source::Preset.read(state) != self.0 {
            return Err("QC preset changed during write verification".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ExternalPreflight {
    params: Value,
    expected: Option<Value>,
}

impl DispatchOperands {
    /// The method of a write that edits a list the session retains for its own stale-state guard
    /// (favorites, pinned models), once the write carries that guard.
    pub(crate) fn retained_list_method(&self) -> Option<&str> {
        matches!(
            self.method.as_str(),
            "device.setFavorite" | "device.setModelPinned"
        )
        .then_some(self.method.as_str())
        .filter(|_| self.external_preflight.is_some())
    }

    pub(crate) fn verification_scope(&self) -> Option<PresetVerificationScope> {
        self.sources.iter().find_map(|(source, expected)| {
            matches!(source, Source::Preset).then(|| PresetVerificationScope(expected.clone()))
        })
    }

    /// Captures the operands of the write `write` planned for `method` with `params`, against the
    /// current device state `state`.
    ///
    /// # Errors
    ///
    /// Returns a message when a preset-dependent write is planned before a current preset exists,
    /// when the write would delete the active setlist, or when the caller's expected-state guards
    /// or targets do not match the current state.
    pub fn capture(
        method: &str,
        params: &Value,
        write: &PlannedWrite,
        state: &DeviceStateRuntime,
    ) -> Result<Self, String> {
        let mut guards = Map::new();
        let target_keys = [
            "row",
            "column",
            "fromColumn",
            "scene",
            "footswitch",
            "parameterIndex",
            "control",
        ];
        // Preserve every caller expectation through queueing. A separate list
        // of guards can silently omit a newly introduced concurrency token.
        if let Some(params) = params.as_object() {
            for (key, value) in params {
                if key.starts_with("expected") || target_keys.contains(&key.as_str()) {
                    guards.insert(key.clone(), value.clone());
                }
            }
        }
        if let PlannedWrite::HidOperation(DeviceOperation::CreateModelPreset {
            row,
            column,
            model_hash,
            ..
        }) = write
        {
            // The preset is saved from the block at (row, column) and keyed by modelHash, which is
            // that block's model. Retain it as an expected model, so that capture and every later
            // re-check compare the block with the hash, whatever arguments the caller carried.
            // A caller's fromColumn is dropped: assert_expected_state prefers it over `column`, and
            // the cell compared must be the operation's own.
            guards.remove("fromColumn");
            guards.insert("row".into(), json!(row));
            guards.insert("column".into(), json!(column));
            guards.insert("expectedModelId".into(), json!(model_hash));
        }
        let mut sources = Vec::new();
        match write {
            PlannedWrite::HidCommand(command) => command_sources(command, &mut sources),
            PlannedWrite::HidOperation(operation) => operation_sources(operation, &mut sources),
            PlannedWrite::MidiControlChange { .. } => {}
        }
        if let PlannedWrite::HidOperation(DeviceOperation::LoadGlobalEqModelPreset {
            value,
            is_factory,
            hash,
        }) = write
        {
            // The planner admitted this recall against the session's retained model preset fold.
            // Native admission asks the fold again, so a delete, a counted transport loss or a
            // session change that the owner has observed before it admits the first frame
            // refuses the write. That closes the owner-visible admission window only: reports
            // still queued below the owner, a half-received delete and the gap between the
            // owner's admission and the USB write are not visible here.
            sources.push(Source::GlobalEqPreset {
                value: value.clone(),
                is_factory: *is_factory,
                hash: *hash,
            });
        }
        if method == "device.selectModeSlot" {
            sources.push(Source::ModeSlots);
        }
        if matches!(
            method,
            "device.recallPreset" | "device.navigateBank" | "device.reloadPreset"
        ) {
            sources.push(Source::Preset);
            sources.push(Source::PresetDirty);
        }
        if params.get("expectedPresetName").is_some() && !sources.contains(&Source::Preset) {
            sources.push(Source::Preset);
        }
        if sources.contains(&Source::Preset) && !state.snapshot().has_preset {
            return Err("A current preset is required to bind native write targets".into());
        }
        if let PlannedWrite::HidOperation(DeviceOperation::DeleteSetlist { name }) = write {
            let target = format!("/media/p4/Presets/{name}");
            if state
                .snapshot()
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(&target)
            {
                return Err(
                    "The active setlist cannot be deleted. Recall another setlist first.".into(),
                );
            }
        }
        let parameter_value = if matches!(
            method,
            "device.setParameter"
                | "device.previewParameter"
                | "device.setLaneControlParameter"
                | "device.previewLaneControlParameter"
        ) && params.get("expectedValue").is_some()
        {
            let row = params
                .get("row")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or("native parameter guard has no row")?;
            let index = params
                .get("parameterIndex")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or("native parameter guard has no index")?;
            let control = params
                .get("control")
                .and_then(Value::as_str)
                .map(str::to_string);
            let column = params
                .get("column")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(0);
            Some((row, column, index, control))
        } else {
            None
        };
        let operands = Self {
            method: method.into(),
            guards: Value::Object(guards),
            sources: sources
                .into_iter()
                .map(|source| {
                    let expected = source.read(state);
                    (source, expected)
                })
                .collect(),
            parameter_value,
            external_preflight: crate::device::request::external_preflight_parameters(
                method, params,
            )
            .map(|params| ExternalPreflight {
                params,
                expected: None,
            }),
        };
        // The plan may precede its authoritative read. Native admission below
        // requires that the external preflight has subsequently been bound.
        operands.validate_targets(state)?;
        Ok(operands)
    }

    /// Re-checks the captured operands against the current device state.
    ///
    /// # Errors
    ///
    /// Returns a message when a guarded target or a captured state source changed since planning,
    /// when the external preflight was never bound to this plan, or when the observation it was
    /// bound to is gone or changed.
    pub fn validate(&self, state: &DeviceStateRuntime) -> Result<(), String> {
        self.validate_targets(state)?;
        if let Some(preflight) = &self.external_preflight {
            let expected = preflight
                .expected
                .as_ref()
                .ok_or("QC external preflight was not bound before native admission")?;
            let response = state
                .external_preflight_observation(&self.method)
                .ok_or("QC external preflight state is no longer available")?;
            if crate::device::request::external_preflight_target(
                &self.method,
                &preflight.params,
                &response,
            )? != *expected
            {
                return Err("QC external preflight target changed before native admission".into());
            }
        }
        Ok(())
    }

    /// Binds the response of a preflight read to this plan and returns the decision taken on it.
    ///
    /// For methods with an external preflight the bound target is recorded and re-checked by
    /// [`DispatchOperands::validate`].
    ///
    /// # Errors
    ///
    /// Returns a message when `method` is not the method this plan was captured for, when the
    /// preflight response is rejected or its observed target differs from the session's current
    /// observation, or when the captured operands no longer match the device state.
    pub fn bind_preflight(
        &mut self,
        method: &str,
        params: &Value,
        response: &Value,
        state: &DeviceStateRuntime,
    ) -> Result<crate::device::request::GatewayWritePreflightDecision, String> {
        if method != self.method {
            return Err("QC preflight does not belong to this write plan".into());
        }
        self.validate_targets(state)?;
        let decision = if let Some(preflight) = self.external_preflight.as_mut() {
            let decision = crate::device::request::gateway_write_preflight_decision(
                method,
                &preflight.params,
                response,
            )?;
            let expected = crate::device::request::external_preflight_target(
                method,
                &preflight.params,
                response,
            )?;
            let current = state
                .external_preflight_observation(method)
                .ok_or("QC preflight observation has not reached the session owner")?;
            if crate::device::request::external_preflight_target(
                method,
                &preflight.params,
                &current,
            )? != expected
            {
                return Err("QC external preflight changed before binding to the write".into());
            }
            preflight.expected = Some(expected);
            decision
        } else {
            crate::device::request::gateway_write_preflight_decision(method, params, response)?
        };
        self.validate(state)?;
        Ok(decision)
    }

    fn validate_targets(&self, state: &DeviceStateRuntime) -> Result<(), String> {
        let snapshot = state.snapshot();
        let snapshot = snapshot.has_preset.then_some(snapshot);
        assert_expected_preset(snapshot, &self.guards)?;
        assert_expected_state(&self.method, snapshot, &self.guards)?;
        for (source, expected) in &self.sources {
            if let Source::ModelAvailability { model_id } = source {
                state.assert_model_available(*model_id)?;
            }
            if let Source::GlobalEqPreset {
                value,
                is_factory,
                hash,
            } = source
            {
                // A false capture must never compare equal to a false re-read.
                state
                    .model_preset_catalog()
                    .admits(value, *is_factory, *hash)
                    .map_err(|refusal| refusal.message())?;
            }
            if source.read(state) != *expected {
                return Err("QC write target changed after planning; refresh before starting a new operation".into());
            }
        }
        if let Some((row, column, index, control)) = &self.parameter_value {
            let details = match control {
                Some(control) => state.lane_control_details(*row, control),
                None => state.block_details(*row, *column),
            };
            let actual = details
                .and_then(|details| {
                    details
                        .parameters
                        .into_iter()
                        .find(|parameter| parameter.index == *index)
                })
                .and_then(|parameter| parameter.normalized_value);
            assert_expected_parameter(actual, &self.guards)?;
        }
        Ok(())
    }
}

fn block(sources: &mut Vec<Source>, row: u32, column: u32) {
    sources.push(Source::Preset);
    sources.push(Source::Block { row, column });
}

fn parameter(
    sources: &mut Vec<Source>,
    row: u32,
    column: u32,
    index: u32,
    control: Option<String>,
) {
    sources.push(Source::ActiveScene);
    if control.is_none() {
        block(sources, row, column);
    } else {
        sources.push(Source::Preset);
    }
    sources.push(Source::Parameter {
        row,
        column,
        index,
        control,
    });
}

fn command_sources(command: &DeviceCommand, sources: &mut Vec<Source>) {
    match command {
        DeviceCommand::SetBypass { row, column, .. } => {
            block(sources, *row, *column);
            sources.push(Source::ActiveScene);
        }
        DeviceCommand::SetParameterNumeric {
            row,
            column,
            parameter_index,
            ..
        }
        | DeviceCommand::SetParameterText {
            row,
            column,
            parameter_index,
            ..
        } => parameter(sources, *row, *column, *parameter_index, None),
        DeviceCommand::SelectScene(_) | DeviceCommand::SetTempo(_) => sources.push(Source::Preset),
        DeviceCommand::SelectMode(_)
        | DeviceCommand::ShowTuner(_)
        | DeviceCommand::ShowGigView(_)
        | DeviceCommand::SetlistPosition { .. }
        | DeviceCommand::SetMasterVolume(_)
        | DeviceCommand::Disconnect => {}
    }
}

fn operation_sources(operation: &DeviceOperation, sources: &mut Vec<Source>) {
    match operation {
        DeviceOperation::Command(command) => command_sources(command, sources),
        DeviceOperation::AddBlock {
            row,
            column,
            model_id,
        } => {
            block(sources, *row, *column);
            sources.push(Source::ModelAvailability {
                model_id: *model_id,
            });
        }
        // The replacement model needs the same proof as an added one: the session's catalog lists
        // it and does not hide it. The occupied cell and its current model stay bound by the
        // caller's expectedModelId and expectedPresetName guards.
        DeviceOperation::ReplaceBlock {
            row,
            column,
            model_id,
        } => {
            block(sources, *row, *column);
            sources.push(Source::ModelAvailability {
                model_id: *model_id,
            });
        }
        // Pinning puts the model on the browser's pinned list, which shows listed models only.
        // Unpinning is cleanup of an entry that may already be stale, so it needs no catalog.
        DeviceOperation::SetModelPinned {
            model_id,
            pinned: true,
        } => sources.push(Source::ModelAvailability {
            model_id: *model_id,
        }),
        // The block a model preset is saved from: its position and model are bound here and
        // compared with modelHash through the retained expectedModelId (see `capture`). There is
        // deliberately no catalog source: a model the catalog hides stays usable as a source.
        DeviceOperation::CreateModelPreset { row, column, .. } => block(sources, *row, *column),
        DeviceOperation::RemoveBlock { row, column }
        | DeviceOperation::ResetBlockParameters { row, column }
        | DeviceOperation::LoadCapture { row, column, .. }
        | DeviceOperation::RetargetCapture { row, column, .. }
        | DeviceOperation::LoadIr { row, column, .. }
        | DeviceOperation::SetExpressionBypass { row, column, .. } => block(sources, *row, *column),
        DeviceOperation::SetFootswitch { row, column, .. } => {
            block(sources, *row, *column);
            sources.push(Source::FootswitchAssignment {
                row: *row,
                column: *column,
            });
        }
        DeviceOperation::SetParameterSceneMode {
            row,
            column,
            parameter_index,
            ..
        }
        | DeviceOperation::SetParameterExpression {
            row,
            column,
            parameter_index,
            ..
        } => parameter(sources, *row, *column, *parameter_index, None),
        DeviceOperation::MoveBlock { .. } => {
            sources.push(Source::Preset);
            sources.push(Source::Grid);
        }
        DeviceOperation::SetChainInput { row, .. }
        | DeviceOperation::SetChainOutput { row, .. }
        | DeviceOperation::SetChainSplit { row, .. }
        | DeviceOperation::SetSplitMute { row, .. }
        | DeviceOperation::SetRoutingParameter { row, .. } => {
            sources.push(Source::Preset);
            sources.push(Source::Routing { row: *row });
        }
        DeviceOperation::SetLaneControlParameter {
            row,
            control,
            parameter_index,
            ..
        }
        | DeviceOperation::SetLaneControlSceneMode {
            row,
            control,
            parameter_index,
            ..
        }
        | DeviceOperation::SetLaneControlExpression {
            row,
            control,
            parameter_index,
            ..
        } => parameter(sources, *row, 0, *parameter_index, Some(control.clone())),
        DeviceOperation::SetPresetLoadMidiOut { .. } => {
            sources.push(Source::Preset);
            sources.push(Source::PresetLoadMidi);
        }
        DeviceOperation::SavePreset { .. }
        | DeviceOperation::DeleteSetlist { .. }
        | DeviceOperation::CopyScene { .. }
        | DeviceOperation::SetSceneLabel { .. }
        | DeviceOperation::SetSceneColor { .. }
        | DeviceOperation::SetStompMomentary { .. }
        | DeviceOperation::SetStompLabel { .. }
        | DeviceOperation::SetMidiOut { .. }
        | DeviceOperation::Undo
        | DeviceOperation::Redo => sources.push(Source::Preset),
        // Absolute global settings, addressed library operations, read requests,
        // and UI gestures do not derive a target from the active preset.
        _ => {}
    }
}

#[cfg(test)]
#[path = "operands_model_guard_tests.rs"]
mod model_guard_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{profile, proto::cortex_protobuf_v2 as pa};
    use prost::Message;

    #[test]
    fn a_queued_write_cannot_regain_its_target_after_an_aba_transition() {
        let mut state = DeviceStateRuntime::new();
        state.snapshot_mut().has_preset = true;
        state.snapshot_mut().setlist_key = "/user/A".into();
        let operands = DispatchOperands::capture(
            "device.setTempo",
            &json!({}),
            &PlannedWrite::HidCommand(DeviceCommand::SetTempo(130)),
            &state,
        )
        .unwrap();
        for key in ["/user/B", "/user/A"] {
            state.snapshot_mut().apply(
                &serde_json::from_value(json!({"kind":"position","position":0,"setlistKey":key}))
                    .unwrap(),
            );
        }
        assert!(operands
            .validate(&state)
            .unwrap_err()
            .contains("target changed"));
        let fresh = DispatchOperands::capture(
            "device.setTempo",
            &json!({}),
            &PlannedWrite::HidCommand(DeviceCommand::SetTempo(130)),
            &state,
        )
        .unwrap();
        state.snapshot_mut().apply(
            &serde_json::from_value(
                json!({"kind":"position","position":0,"setlistKey":"/user/A/"}),
            )
            .unwrap(),
        );
        assert!(
            fresh.validate(&state).is_ok(),
            "ordinary same-address observations do not revoke a write"
        );
    }
    #[test]
    fn plugin_add_needs_only_catalog_presence_at_capture_and_again_at_dispatch_for_all_methods() {
        let mut state = DeviceStateRuntime::new();
        state.snapshot_mut().has_preset = true;
        let catalog = |xml: &[u8]| {
            pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.to_vec()),
                ),
                ..Default::default()
            }
            .encode_to_vec()
        };
        state.install_model_catalog(
            crate::state::parse_model_repo(&catalog(
                br#"<Models><Category name="Wah"><Model id="9013" name="Built in"/><Model id="9022" name="Plugin" plugin_id="13"/></Category></Models>"#,
            ))
            .unwrap(),
        );
        let write = |model_id| {
            PlannedWrite::HidOperation(DeviceOperation::AddBlock {
                row: 0,
                column: 1,
                model_id,
            })
        };
        let builtin =
            DispatchOperands::capture("device.addBlock", &json!({}), &write(9013), &state).unwrap();
        assert!(
            DispatchOperands::capture("device.addBlock", &json!({}), &write(99999), &state)
                .unwrap_err()
                .contains("absent")
        );
        // No License message has been observed: the plugin is still admitted.
        let plans: Vec<_> = ["device.addBlock"]
            .into_iter()
            .map(|method| {
                DispatchOperands::capture(method, &json!({}), &write(9022), &state).unwrap()
            })
            .collect();
        assert_eq!(plans.len(), 1);
        // A license state that lists no plugins does not revoke it either.
        let license = pa::LicenseMessage {
            action: pa::message_action::Enum::Update as i32,
            is_enabled: Some(pa::license_message::IsEnabled::IsEnabled(false)),
            is_deactivated: Some(pa::license_message::IsDeactivated::IsDeactivated(true)),
            supported_plugin_ids: vec![13],
            ..Default::default()
        };
        state
            .ingest(profile::MESSAGE_TYPE_LICENSE, &license.encode_to_vec())
            .unwrap();
        for plan in &plans {
            assert!(plan.validate(&state).is_ok());
        }
        // The catalog check still runs at dispatch: a model the catalog now
        // hides is refused before the first report.
        state.install_model_catalog(
            crate::state::parse_model_repo(&catalog(
                br#"<Models><Category name="Wah"><Model id="9013" name="Built in"/><Model id="9022" name="Plugin" plugin_id="13" hidden="true"/></Category></Models>"#,
            ))
            .unwrap(),
        );
        for plan in &plans {
            assert!(plan
                .validate(&state)
                .unwrap_err()
                .contains("not a selectable QC model"));
        }
        assert!(builtin.validate(&state).is_ok());
    }
}
