#![cfg(test)]

//! Controls for `device.createModelPreset`: it plans with exactly its published arguments, and the
//! model hash it carries is bound to the block it saves from at planning, at capture and again at
//! the first report. The catalog is not consulted, so a source model the catalog hides stays usable.
//!
//! The six planning probes of `request_create_model_preset_probe_tests.rs` and
//! `request_create_model_preset_name_probe_tests.rs` supply the remaining positive controls (the
//! published arguments with the name omitted, empty and matching) and the mismatching-name and
//! empty-identity negatives.

use super::*;
use crate::device::operands::DispatchOperands;
use crate::device::owner::SessionRuntime;
use crate::device::session::SessionDriver;
use crate::device::state_runtime::DeviceStateRuntime;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::state::GridBlock;
use prost::Message;
use serde_json::json;

const METHOD: &str = "device.createModelPreset";

/// A model the catalog lists and hides, as a unit's ModelRepo lists the default cabsims.
const HIDDEN_CATALOG: &[u8] = br#"<Models><Category name="Utility"><Model id="12000" name="Default Cabsim" hidden="true"/></Category></Models>"#;

fn arguments(model_hash: u32, name: Option<&str>) -> Value {
    let mut params = json!({"row": 0, "column": 0, "modelHash": model_hash, "name": "My drive",
        "expectedPresetIdentity": "identity-1"});
    if let Some(name) = name {
        params["expectedPresetName"] = json!(name);
    }
    params
}

/// A complete current preset whose block at (0, 0) holds `model_id`.
fn preset_with_block(model_id: u32) -> GatewaySnapshot {
    GatewaySnapshot {
        preset_name: "Live".into(),
        preset_identity: Some("identity-1".into()),
        grid_layout_complete: true,
        has_preset: true,
        blocks: vec![GridBlock {
            id: "0:0".into(),
            model_id: Some(model_id),
            row: 0,
            column: 0,
            ..Default::default()
        }],
        ..GatewaySnapshot::default()
    }
}

fn state_holding(snapshot: GatewaySnapshot) -> DeviceStateRuntime {
    let mut state = DeviceStateRuntime::new();
    *state.snapshot_mut() = snapshot;
    state
}

fn install(state: &mut DeviceStateRuntime, xml: &[u8]) {
    let payload = pa::ModelRepoMessage {
        action: pa::message_action::Enum::Update as i32,
        model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
            xml.to_vec(),
        )),
        ..Default::default()
    }
    .encode_to_vec();
    state.install_model_catalog(crate::state::parse_model_repo(&payload).unwrap());
}

/// A ModelRepo update that carries no payload, so it installs a catalog without any model.
fn install_empty(state: &mut DeviceStateRuntime) {
    let payload = pa::ModelRepoMessage {
        action: pa::message_action::Enum::Update as i32,
        ..Default::default()
    }
    .encode_to_vec();
    state.install_model_catalog(crate::state::parse_model_repo(&payload).unwrap());
}

fn write(model_hash: u32) -> PlannedWrite {
    PlannedWrite::HidOperation(DeviceOperation::CreateModelPreset {
        row: 0,
        column: 0,
        model_hash,
        name: "My drive".into(),
    })
}

#[test]
fn the_exact_signed_model_id_maximum_plans_and_captures() {
    let model_hash = i32::MAX as u32;
    let params = arguments(model_hash, None);
    crate::device::generated_gateway::validate_params(METHOD, &params).unwrap();
    let state = state_holding(preset_with_block(model_hash));
    let plan = plan_gateway_write_with_state(METHOD, &params, &state).unwrap();
    assert!(matches!(plan.write,
        PlannedWrite::HidOperation(DeviceOperation::CreateModelPreset { model_hash: actual, .. })
        if actual == model_hash));
    DispatchOperands::capture(METHOD, &params, &plan.write, &state).unwrap();
}

#[test]
fn every_published_argument_shape_plans() {
    // Name omitted, empty and matching: the optional name is never required.
    for name in [None, Some(""), Some("Live")] {
        let params = arguments(41, name);
        crate::device::generated_gateway::validate_params(METHOD, &params)
            .expect("the published contract admits these arguments");
        let plan = plan_gateway_write(METHOD, &params, Some(&preset_with_block(41)))
            .unwrap_or_else(|error| panic!("{name:?}: {error}"));
        assert!(matches!(
            plan.write,
            PlannedWrite::HidOperation(DeviceOperation::CreateModelPreset {
                row: 0,
                column: 0,
                model_hash: 41,
                ..
            })
        ));
    }
    // The host planner, which plans against the owner's state, accepts them too.
    let state = state_holding(preset_with_block(41));
    plan_gateway_write_with_state(METHOD, &arguments(41, None), &state).unwrap();
}

#[test]
fn a_missing_identity_is_refused_by_the_planner() {
    for name in [None, Some("Live")] {
        let mut params = arguments(41, name);
        params
            .as_object_mut()
            .unwrap()
            .remove("expectedPresetIdentity");
        let refused = plan_gateway_write(METHOD, &params, Some(&preset_with_block(41)))
            .expect_err("the identity is required");
        assert_eq!(
            refused,
            "expectedPresetIdentity is required to guard device.createModelPreset"
        );
    }
}

#[test]
fn a_supplied_wrong_identity_or_name_still_refuses() {
    let snapshot = preset_with_block(41);
    let mut params = arguments(41, Some("Live"));
    params["expectedPresetIdentity"] = json!("identity-2");
    let refused = plan_gateway_write(METHOD, &params, Some(&snapshot)).unwrap_err();
    assert!(
        refused.contains("preset or QC session changed"),
        "{refused}"
    );
    let refused =
        plan_gateway_write(METHOD, &arguments(41, Some("Another")), Some(&snapshot)).unwrap_err();
    assert!(
        refused.contains("Preset changed on the Quad Cortex"),
        "{refused}"
    );
}

#[test]
fn a_model_hash_differing_from_the_block_is_refused_at_planning() {
    let snapshot = preset_with_block(41);
    let refused = plan_gateway_write(METHOD, &arguments(42, None), Some(&snapshot)).unwrap_err();
    assert!(
        refused.contains("expected model Some(42), found Some(41)"),
        "{refused}"
    );
    // Also when the name is supplied and correct: the name does not stand in for the model.
    let refused =
        plan_gateway_write(METHOD, &arguments(42, Some("Live")), Some(&snapshot)).unwrap_err();
    assert!(
        refused.contains("expected model Some(42), found Some(41)"),
        "{refused}"
    );
    // No block at the addressed cell.
    let mut empty = preset_with_block(41);
    empty.blocks.clear();
    let refused = plan_gateway_write(METHOD, &arguments(41, None), Some(&empty)).unwrap_err();
    assert!(
        refused.contains("expected model Some(41), found None"),
        "{refused}"
    );
}

#[test]
fn a_model_hash_that_is_not_a_number_is_left_to_the_operation_parser() {
    let mut params = arguments(41, None);
    params["modelHash"] = json!("41");
    assert!(plan_gateway_write(METHOD, &params, Some(&preset_with_block(41))).is_err());
    params.as_object_mut().unwrap().remove("modelHash");
    assert!(plan_gateway_write(METHOD, &params, Some(&preset_with_block(41))).is_err());
}

#[test]
fn capture_binds_the_model_hash_whatever_the_caller_carried() {
    // The write names model 42 while the block holds 41. The caller's arguments carry no guard at
    // all, so only the operands that capture retains can refuse it.
    let state = state_holding(preset_with_block(41));
    let refused = DispatchOperands::capture(METHOD, &json!({}), &write(42), &state).unwrap_err();
    assert!(
        refused.contains("expected model Some(42), found Some(41)"),
        "{refused}"
    );
    assert!(DispatchOperands::capture(METHOD, &json!({}), &write(41), &state).is_ok());
}

#[test]
fn a_block_replaced_after_capture_is_refused_by_the_operands() {
    let mut state = state_holding(preset_with_block(41));
    let operands = DispatchOperands::capture(METHOD, &json!({}), &write(41), &state).unwrap();
    assert!(operands.validate(&state).is_ok());
    state.snapshot_mut().blocks[0].model_id = Some(43);
    let refused = operands.validate(&state).unwrap_err();
    assert!(
        refused.contains("expected model Some(41), found Some(43)"),
        "{refused}"
    );
    state.snapshot_mut().blocks.clear();
    assert!(operands.validate(&state).is_err());
}

#[test]
fn a_hidden_installed_source_model_is_accepted_without_any_catalog() {
    let snapshot = preset_with_block(12000);
    let params = arguments(12000, None);
    let plan = plan_gateway_write(METHOD, &params, Some(&snapshot)).unwrap();
    // No catalog installed, then a catalog that hides the model: neither is consulted.
    let mut state = state_holding(snapshot);
    DispatchOperands::capture(METHOD, &params, &plan.write, &state).unwrap();
    install(&mut state, HIDDEN_CATALOG);
    let operands = DispatchOperands::capture(METHOD, &params, &plan.write, &state).unwrap();
    assert!(operands.validate(&state).is_ok());
    // An installed but empty catalog is not consulted either.
    install_empty(&mut state);
    assert!(operands.validate(&state).is_ok());
}

/// A live owner (an active lifecycle, no USB) holding `snapshot`, with a create from the block at
/// (0, 0) queued up to, and not including, its first report.
fn queued_create(
    snapshot: GatewaySnapshot,
    model_hash: u32,
) -> (SessionRuntime, crate::device::delivery::DeliveryTicket) {
    let mut owner = SessionRuntime::new(0);
    owner.opened(0).unwrap();
    owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
    *owner.state.snapshot_mut() = snapshot;
    let context = owner
        .context_for_write(METHOD, &json!({}), &write(model_hash))
        .unwrap();
    let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
    let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
    (owner, delivery)
}

#[test]
fn an_unchanged_block_starts_its_first_report() {
    let (mut owner, delivery) = queued_create(preset_with_block(41), 41);
    owner.begin_native_report(&delivery, 0, 1).unwrap();
    assert_eq!(
        owner.delivery_record(&delivery).unwrap().started_at_ms,
        Some(1)
    );
}

#[test]
fn a_hidden_installed_source_model_starts_its_first_report() {
    let (mut owner, delivery) = queued_create(preset_with_block(12000), 12000);
    install(&mut owner.state, HIDDEN_CATALOG);
    owner.begin_native_report(&delivery, 0, 1).unwrap();
    assert_eq!(
        owner.delivery_record(&delivery).unwrap().started_at_ms,
        Some(1)
    );
}

#[test]
fn a_block_replaced_between_plan_and_first_frame_is_refused_at_the_first_report() {
    use crate::device::delivery::{DeliveryStatus, DeliveryStop};
    for replace in [true, false] {
        let (mut owner, delivery) = queued_create(preset_with_block(41), 41);
        let fence = owner.session_fence().unwrap();
        if replace {
            owner.state.snapshot_mut().blocks[0].model_id = Some(43);
        } else {
            owner.state.snapshot_mut().blocks.clear();
        }
        assert!(
            owner.session_fence().unwrap() == fence,
            "the fence must be untouched for this test to isolate the operand re-check"
        );
        let refused = owner.begin_native_report(&delivery, 0, 1).unwrap_err();
        assert!(refused.contains("changed"), "{refused}");
        let record = owner.delivery_record(&delivery).unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled, "{record:?}");
        assert_eq!(record.stop, Some(DeliveryStop::AdmissionRevoked));
        assert_eq!(record.started_at_ms, None, "no report may have started");
    }
}

/// The block at (0, 0) holds `first`, and a second block at (0, 1) holds `second`.
fn preset_with_two_blocks(first: u32, second: u32) -> GatewaySnapshot {
    let mut snapshot = preset_with_block(first);
    snapshot.blocks.push(GridBlock {
        id: "0:1".into(),
        model_id: Some(second),
        row: 0,
        column: 1,
        ..Default::default()
    });
    snapshot
}

#[test]
fn a_stray_from_column_never_moves_the_compared_cell_at_planning() {
    // A caller that bypasses validate_params can carry fromColumn, which assert_expected_state
    // prefers over `column`. The comparison must still look at the operation's own cell (0, 0).
    let snapshot = preset_with_two_blocks(41, 99);
    let mut params = arguments(41, None);
    params["fromColumn"] = json!(1);
    plan_gateway_write(METHOD, &params, Some(&snapshot))
        .expect("hash 41 equals the block at column 0, whatever fromColumn says");
    let mut params = arguments(99, None);
    params["fromColumn"] = json!(1);
    let refused = plan_gateway_write(METHOD, &params, Some(&snapshot)).unwrap_err();
    assert!(
        refused.contains("column 0 changed: expected model Some(99), found Some(41)"),
        "{refused}"
    );
}

#[test]
fn a_stray_from_column_never_moves_the_compared_cell_at_capture() {
    let bypass = json!({"row": 0, "column": 0, "fromColumn": 1});
    let mut state = state_holding(preset_with_two_blocks(41, 99));
    let operands = DispatchOperands::capture(METHOD, &bypass, &write(41), &state).unwrap();
    let refused = DispatchOperands::capture(METHOD, &bypass, &write(99), &state).unwrap_err();
    assert!(
        refused.contains("expected model Some(99), found Some(41)"),
        "{refused}"
    );
    // The retained guards follow the operation's own cell on every later re-check as well.
    state.snapshot_mut().blocks[1].model_id = Some(7);
    assert!(operands.validate(&state).is_ok());
    state.snapshot_mut().blocks[0].model_id = Some(43);
    assert!(operands.validate(&state).is_err());
}

#[test]
fn a_preset_change_after_capture_is_refused_when_the_name_is_omitted() {
    // This test captures empty params, omitting both name and identity. Published calls retain
    // expectedPresetIdentity; here only the `Source::Preset` that `operation_sources` pushes for the
    // source block notices that the preset changed. Without that arm none of these refuse.
    type Change = fn(&mut DeviceStateRuntime);
    let changes: [Change; 2] = [
        |state| state.snapshot_mut().address_revision += 1,
        |state| state.snapshot_mut().setlist_key = "/media/p4/Other".into(),
    ];
    for change in changes {
        let mut state = state_holding(preset_with_block(41));
        let operands = DispatchOperands::capture(METHOD, &json!({}), &write(41), &state).unwrap();
        assert!(operands.validate(&state).is_ok());
        change(&mut state);
        let refused = operands.validate(&state).unwrap_err();
        assert!(
            refused.contains("target changed after planning"),
            "{refused}"
        );
    }
}

#[test]
fn a_preset_change_between_plan_and_first_report_is_refused_when_the_name_is_omitted() {
    use crate::device::delivery::{DeliveryStatus, DeliveryStop};
    let (mut owner, delivery) = queued_create(preset_with_block(41), 41);
    let fence = owner.session_fence().unwrap();
    owner.state.snapshot_mut().address_revision += 1;
    assert!(
        owner.session_fence().unwrap() == fence,
        "the fence must be untouched for this test to isolate the operand re-check"
    );
    let refused = owner.begin_native_report(&delivery, 0, 1).unwrap_err();
    assert!(
        refused.contains("target changed after planning"),
        "{refused}"
    );
    let record = owner.delivery_record(&delivery).unwrap();
    assert_eq!(record.status, DeliveryStatus::Cancelled, "{record:?}");
    assert_eq!(record.stop, Some(DeliveryStop::AdmissionRevoked));
    assert_eq!(record.started_at_ms, None, "no report may have started");
}

#[test]
fn an_out_of_range_model_hash_is_reported_as_model_hash() {
    // The published argument is modelHash; the error must not name expectedModelId, which the
    // contract does not admit.
    for hash in [2_147_483_648_u64, 4_294_967_296_u64] {
        let mut params = arguments(41, None);
        params["modelHash"] = json!(hash);
        let refused =
            plan_gateway_write(METHOD, &params, Some(&preset_with_block(41))).unwrap_err();
        assert!(refused.contains("modelHash"), "{refused}");
        assert!(!refused.contains("expectedModelId"), "{refused}");
    }
}
