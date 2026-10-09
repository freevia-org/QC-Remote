#![cfg(test)]

//! Probe: can `device.createModelPreset` pass planning with only the arguments its contract
//! publishes?
//!
//! The planner used to list the method among those that require `expectedModelId`, while the
//! generated contract (`generated_gateway::validate_params`, the MCP schema and the TypeScript
//! transport) neither lists nor admits that argument, so no caller could plan this method.
//!
//! The probe was `#[ignore]`d while the planner still demanded `expectedModelId` (it failed six
//! times, all with "expectedModelId is required"). The planner now binds `modelHash` to the block
//! instead, the probe is a normal test, and it is a positive control.

use super::*;
use crate::state::GridBlock;
use serde_json::json;

const METHOD: &str = "device.createModelPreset";

/// Exactly the arguments the contract declares: `expectedPresetName` is optional there, and
/// `expectedModelId` is not an argument of the method.
fn published_arguments() -> Value {
    json!({"row": 0, "column": 0, "modelHash": 41, "name": "My drive",
        "expectedPresetName": "Live", "expectedPresetIdentity": "identity-1"})
}

/// A complete current preset whose block at (0, 0) is the model the arguments name.
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

#[test]
fn create_model_preset_plans_with_only_its_published_arguments() {
    let params = published_arguments();
    crate::device::generated_gateway::validate_params(METHOD, &params)
        .expect("these are exactly the arguments the published contract admits");
    let plan = plan_gateway_write(METHOD, &params, Some(&current_preset()))
        .unwrap_or_else(|error| panic!("the published arguments do not plan: {error}"));
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

#[test]
fn create_model_preset_contract_does_not_admit_expected_model_id() {
    // The other half of the contradiction, which holds before and after any planner correction:
    // a caller cannot supply expectedModelId, and the corrected planner does not require it.
    let mut params = published_arguments();
    params["expectedModelId"] = json!(41);
    assert!(crate::device::generated_gateway::validate_params(METHOD, &params).is_err());
}
