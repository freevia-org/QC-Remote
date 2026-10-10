#![cfg(test)]

//! Probe, part 2: the optional `expectedPresetName` of `device.createModelPreset`.
//!
//! The gateway contract publishes `expectedPresetName` as optional (default `""`) and
//! `expectedPresetIdentity` as required. An eventual planner correction must therefore accept the
//! call without the name, keep refusing a supplied name that does not match, and treat an empty
//! name as absent. `assert_expected_preset` already behaves that way: it validates the identity and
//! returns early for an absent or empty name.
//!
//! The planning tests were `#[ignore]`d while the planner demanded `expectedModelId` (all of them
//! failed with "expectedModelId is required"). The planner now binds `modelHash` to the block, so
//! they run as normal tests and are positive and negative controls. The remaining tests pin the existing `assert_expected_preset` behaviour.

use super::*;
use crate::state::GridBlock;
use serde_json::json;

const METHOD: &str = "device.createModelPreset";

fn arguments(name: Option<&str>) -> Value {
    let mut params = json!({"row": 0, "column": 0, "modelHash": 41, "name": "My drive",
        "expectedPresetIdentity": "identity-1"});
    if let Some(name) = name {
        params["expectedPresetName"] = json!(name);
    }
    params
}

fn current_preset() -> GatewaySnapshot {
    GatewaySnapshot {
        preset_name: "Live".into(),
        preset_identity: Some("identity-1".into()),
        grid_layout_complete: true,
        has_preset: true,
        blocks: vec![GridBlock {
            id: "0:0".into(),
            model_id: Some(41),
            row: 0,
            column: 0,
            ..Default::default()
        }],
        ..GatewaySnapshot::default()
    }
}

fn published(params: &Value) {
    crate::device::generated_gateway::validate_params(METHOD, params)
        .expect("the published contract admits these arguments");
}

#[test]
fn create_model_preset_plans_when_the_optional_preset_name_is_omitted() {
    let params = arguments(None);
    published(&params);
    plan_gateway_write(METHOD, &params, Some(&current_preset()))
        .unwrap_or_else(|error| panic!("an omitted optional name must plan: {error}"));
}

#[test]
fn create_model_preset_plans_with_a_supplied_matching_name() {
    let params = arguments(Some("Live"));
    published(&params);
    plan_gateway_write(METHOD, &params, Some(&current_preset()))
        .unwrap_or_else(|error| panic!("a matching supplied name must plan: {error}"));
}

#[test]
fn create_model_preset_refuses_a_supplied_mismatching_name() {
    let params = arguments(Some("Another preset"));
    published(&params);
    let refused = plan_gateway_write(METHOD, &params, Some(&current_preset()))
        .expect_err("a supplied wrong name must still refuse");
    // The refusal must be the preset-name mismatch, not some other missing argument.
    assert!(
        refused.contains("Preset changed on the Quad Cortex"),
        "{refused}"
    );
}

#[test]
fn create_model_preset_treats_an_empty_preset_name_as_absent() {
    let params = arguments(Some(""));
    published(&params);
    plan_gateway_write(METHOD, &params, Some(&current_preset()))
        .unwrap_or_else(|error| panic!("an empty optional name must plan: {error}"));
}

#[test]
fn assert_expected_preset_ignores_an_omitted_or_empty_name_and_refuses_a_mismatch() {
    // The existing behaviour the planner correction relies on.
    let snapshot = current_preset();
    assert_eq!(
        assert_expected_preset(Some(&snapshot), &arguments(None)),
        Ok(())
    );
    assert_eq!(
        assert_expected_preset(Some(&snapshot), &arguments(Some(""))),
        Ok(())
    );
    assert_eq!(
        assert_expected_preset(Some(&snapshot), &arguments(Some("Live"))),
        Ok(())
    );
    let refused =
        assert_expected_preset(Some(&snapshot), &arguments(Some("Another preset"))).unwrap_err();
    assert!(
        refused.contains("Preset changed on the Quad Cortex"),
        "{refused}"
    );
    // The identity is validated whether or not a name is supplied.
    let mut wrong_identity = arguments(None);
    wrong_identity["expectedPresetIdentity"] = json!("identity-2");
    assert!(assert_expected_preset(Some(&snapshot), &wrong_identity).is_err());
}

// The preset identity is required, unlike the name. An empty identity is a malformed guard.

#[test]
fn create_model_preset_refuses_an_empty_preset_identity() {
    let mut params = arguments(Some("Live"));
    params["expectedPresetIdentity"] = json!("");
    // The gateway contract types the identity as a string, so an empty one passes `validate_params`;
    // the planner is what must refuse it, with the identity's own text and not a missing argument.
    published(&params);
    let refused = plan_gateway_write(METHOD, &params, Some(&current_preset()))
        .expect_err("an empty identity must be refused");
    assert!(
        refused.contains("A preset identity from current device state is required"),
        "{refused}"
    );
}

#[test]
fn assert_expected_preset_refuses_an_empty_or_non_string_identity() {
    // The existing behaviour the correction relies on. A present identity must be a
    // non-empty string, whether or not a name is supplied.
    let snapshot = current_preset();
    for identity in [json!(""), json!(7), json!(null)] {
        for name in [None, Some("Live")] {
            let mut params = arguments(name);
            params["expectedPresetIdentity"] = identity.clone();
            let refused = assert_expected_preset(Some(&snapshot), &params).unwrap_err();
            assert_eq!(
                refused, "A preset identity from current device state is required",
                "{identity}"
            );
        }
    }
}

#[test]
fn create_model_preset_contract_requires_the_preset_identity() {
    // Leaving the identity out entirely is refused by the contract, so the planner
    // never sees that case from a published call.
    let mut params = arguments(Some("Live"));
    params
        .as_object_mut()
        .unwrap()
        .remove("expectedPresetIdentity");
    assert!(crate::device::generated_gateway::validate_params(METHOD, &params).is_err());
}
