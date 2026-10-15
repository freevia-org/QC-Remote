#![cfg(test)]

use super::*;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::proto::{
    binary_preset, chain, model, param, param_value, BinaryPreset, Chain, Model, Param, ParamValue,
};
use crate::state::{
    parse_model_repo, GridBlock, GridRoute, ModeSlot, PresetFileListing, PresetFolderListing,
};
use prost::Message;
use serde_json::json;

#[test]
fn require_confirmation_accepts_only_an_explicit_true_and_preserves_the_message() {
    assert_eq!(
        require_confirmation(&json!({"go": true}), "go", "must confirm"),
        Ok(())
    );
    for params in [
        json!({"go": false}),
        json!({}),
        json!({"go": "true"}),
        json!({"go": 1}),
    ] {
        assert_eq!(
            require_confirmation(&params, "go", "must confirm"),
            Err("must confirm".to_string())
        );
    }
}

#[test]
fn confirmation_guards_still_produce_their_original_verbatim_messages() {
    assert_eq!(
        operation("setTunerInput", &json!({"inputPortId": 1})).unwrap_err(),
        "Changing tuner input is believed to engage the tuner invisibly; confirmTunerActivation must be true."
    );
    assert_eq!(
        operation("setTunerMute", &json!({"muted": true})).unwrap_err(),
        "Changing tuner mute is believed to engage the tuner invisibly and may silence every output; confirmTunerActivation must be true."
    );
    assert_eq!(
        operation("setTunerReference", &json!({"referenceOffsetHz": 2.0})).unwrap_err(),
        "Changing tuner reference is believed to engage the tuner invisibly; confirmTunerActivation must be true."
    );
    assert_eq!(
        operation("setTunerMeter", &json!({"enabled": true})).unwrap_err(),
        "Changing tuner meter reporting is believed to engage the tuner invisibly; confirmTunerActivation must be true."
    );
}

#[test]
fn block_parameter_callers_must_supply_the_model_they_observed() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    state
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    name: Some(binary_preset::Name::Name("Live".into())),
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(0)),
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(41)),
                            column: Some(model::Column::Column(0)),
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
    let mut snapshot = state.snapshot().clone();
    snapshot.blocks[0].bypassed = Some(false);
    snapshot.grid_layout_complete = true;
    let params = json!({"row":0,"column":0,"parameterIndex":0,"value":0.5,
            "expectedValue":0.25,"expectedScene":0,"expectedPresetName":"Live",
            "enabled":true,"pedal":1,"minimum":0.0,"maximum":1.0,
            "desiredBypassed":true,"mode":0,"invert":false,"delayMs":0,"latchEmulation":false});
    for method in [
        "device.setParameter",
        "device.previewParameter",
        "device.resetBlockParameters",
        "device.setParameterSceneMode",
        "device.setParameterExpression",
        "device.toggleBypass",
        "device.setExpressionBypass",
    ] {
        let mut current = params.clone();
        if method == "device.toggleBypass" {
            current["expectedBypassed"] = json!(snapshot.blocks[0].bypassed.unwrap_or(false));
        }
        for missing_or_null in [None, Some(Value::Null)] {
            let mut request = current.clone();
            if let Some(value) = missing_or_null {
                request["expectedModelId"] = value;
            }
            let error = plan_gateway_write(method, &request, Some(&snapshot))
                .expect_err("unbound block write reached planning");
            assert!(error.contains("expectedModelId"), "{method}: {error}");
        }
        current["expectedModelId"] = json!(41);
        plan_gateway_write(method, &current, Some(&snapshot))
            .unwrap_or_else(|error| panic!("valid {method}: {error}"));
        current["expectedModelId"] = json!(42);
        assert!(
            plan_gateway_write(method, &current, Some(&snapshot)).is_err(),
            "replacement model accepted by {method}"
        );
    }
}

#[test]
fn grid_addressed_writes_refuse_incomplete_topology_but_preset_controls_remain_available() {
    let methods = [
        "device.toggleBypass",
        "device.setParameter",
        "device.previewParameter",
        "device.resetBlockParameters",
        "device.setParameterSceneMode",
        "device.setParameterExpression",
        "device.setExpressionBypass",
        "device.moveBlock",
        "device.addBlock",
        "device.replaceBlock",
        "device.createModelPreset",
        "device.removeBlock",
        "device.loadCapture",
        "device.loadIr",
        "device.setBlockFootswitch",
        "device.setChainInput",
        "device.setChainOutput",
        "device.setChainSplit",
        "device.setSplitMute",
        "device.setStompMomentary",
        "device.setStompLabel",
        "device.setLaneControlParameter",
        "device.previewLaneControlParameter",
        "device.setLaneControlSceneMode",
        "device.setLaneControlExpression",
    ];
    let incomplete = GatewaySnapshot::default();
    for method in methods {
        let error = plan_gateway_write(method, &json!({}), Some(&incomplete))
            .expect_err("Grid-addressed write must not use a sparse stale layout");
        assert!(error.starts_with("STATE_UNAVAILABLE:"), "{method}: {error}");

        let mut complete = incomplete.clone();
        complete.grid_layout_complete = true;
        let error = plan_gateway_write(method, &json!({}), Some(&complete))
            .expect_err("empty arguments should still fail the method's normal guards");
        assert!(
            !error.starts_with("STATE_UNAVAILABLE:"),
            "{method}: {error}"
        );
    }
    assert!(!gateway_method_requires_exact_grid("device.setTempo"));
}

#[test]
fn malformed_required_guards_cannot_disable_authoritative_planner_comparisons() {
    let snapshot = GatewaySnapshot {
        has_preset: true,
        preset_name: "Guard fixture".into(),
        master_volume: 80,
        tempo: 120,
        active_scene: 0,
        mode: "STOMP".into(),
        grid_layout_complete: true,
        blocks: vec![fixture_block(0, 0, 41)],
        ..Default::default()
    };
    let requests = [
        (
            "device.setMasterVolume",
            json!({"value":20,"expectedValue":80}),
            "expectedValue",
        ),
        (
            "device.setTempo",
            json!({"bpm":130,"expectedTempo":120,"expectedPresetName":"Guard fixture"}),
            "expectedTempo",
        ),
        (
            "device.toggleBypass",
            json!({"row":0,"column":0,"desiredBypassed":true,"expectedBypassed":false,"expectedModelId":41,"expectedScene":0,"expectedPresetName":"Guard fixture"}),
            "expectedBypassed",
        ),
        (
            "device.toggleBypass",
            json!({"row":0,"column":0,"desiredBypassed":true,"expectedBypassed":false,"expectedModelId":41,"expectedScene":0,"expectedPresetName":"Guard fixture"}),
            "expectedScene",
        ),
        (
            "device.tapTempo",
            json!({"expectedMode":"STOMP","expectedPresetName":"Guard fixture"}),
            "expectedMode",
        ),
    ];
    for (method, request, field) in requests {
        assert!(
            plan_gateway_write(method, &request, Some(&snapshot)).is_ok(),
            "{method}"
        );
        for invalid in [
            Value::Null,
            json!("wrong"),
            json!([]),
            json!({}),
            json!(4_294_967_296_u64),
            json!(-1),
            json!(0.5),
        ] {
            let mut malformed = request.clone();
            malformed[field] = invalid;
            let error = plan_gateway_write(method, &malformed, Some(&snapshot))
                .expect_err("malformed guard must reject before transport planning");
            assert!(error.contains(field), "{method}: {error}");
        }
    }
    let wrapped = json!({"value":20,"expectedValue":4_294_967_376_u64});
    assert!(plan_gateway_write("device.setMasterVolume", &wrapped, Some(&snapshot)).is_err());
    let wrapped =
        json!({"bpm":130,"expectedTempo":4_294_967_416_u64,"expectedPresetName":"Guard fixture"});
    assert!(plan_gateway_write("device.setTempo", &wrapped, Some(&snapshot)).is_err());
}

#[test]
fn required_guard_validation_preserves_intentional_nullable_and_text_domains() {
    for (method, field, params) in [
        (
            "device.addBlock",
            "expectedModelId",
            json!({"expectedModelId":null}),
        ),
        (
            "device.setBlockFootswitch",
            "expectedFootswitch",
            json!({"expectedFootswitch":null}),
        ),
        (
            "device.setBlockFootswitch",
            "expectedAssignmentType",
            json!({"expectedAssignmentType":null}),
        ),
        (
            "device.setChainInput",
            "expectedInputId",
            json!({"expectedInputId":null}),
        ),
        (
            "device.setChainOutput",
            "expectedOutputId",
            json!({"expectedOutputId":null}),
        ),
        (
            "device.setChainSplit",
            "expectedMixColumn",
            json!({"expectedMixColumn":null}),
        ),
        (
            "device.setParameter",
            "expectedValue",
            json!({"expectedValue":null,"value":"capture/key"}),
        ),
    ] {
        validate_required_guard(method, field, &params).unwrap();
    }
    assert!(validate_required_guard(
        "device.setParameter",
        "expectedValue",
        &json!({"expectedValue":null,"value":0.5})
    )
    .is_err());
}

fn fixture_block(row: u32, column: u32, model_id: u32) -> GridBlock {
    GridBlock {
        id: format!("{row}:{column}"),
        model_id: Some(model_id),
        category_id: None,
        name: "Fixture".into(),
        kind: "utility".into(),
        category: None,
        plugin: None,
        plugin_id: None,
        row,
        column,
        bypassed: Some(false),
        bypass_expression: None,
        color: None,
        glyph: None,
        footswitch: None,
        footswitch_order: None,
        footswitch_assignment_type: None,
        ..Default::default()
    }
}

fn observed_parameter(normalized_value: f64) -> BlockParameter {
    BlockParameter {
        index: 7,
        display_position: 7,
        name: "Test".into(),
        normalized_value: Some(normalized_value),
        display_value: normalized_value.to_string(),
        units: String::new(),
        r#type: "float".into(),
        minimum: 0.0,
        maximum: 1.0,
        value_scale: "linear".into(),
        scale_exponent: None,
        scale_points: Vec::new(),
        scale_known: true,
        display_precision: None,
        minimum_label: None,
        midpoint_label: None,
        maximum_label: None,
        show_as_integer: false,
        has_off_position: false,
        floor_display: None,
        floor_normalized: None,
        steps: None,
        scene_mode: false,
        options: Vec::new(),
        writable: true,
        enabled: true,
        expression_assignable: true,
        linked_scene_mode: None,
        expression: None,
        expression_minimum: None,
        expression_maximum: None,
        led_value: None,
        wire_value_kind: "float".into(),
    }
}

#[test]
fn retained_scene_color_guard_rejects_changes_after_planning() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    *state.snapshot_mut() = GatewaySnapshot {
        has_preset: true,
        preset_name: "Scene guard".into(),
        scene_colors: Some(vec!["#abcdef".into(); 8]),
        ..Default::default()
    };
    let params = json!({"scene": 2, "color": 0xffff02c2_u32,
            "expectedColor": "#abcdef", "expectedPresetName": "Scene guard"});
    let plan = plan_gateway_write_with_state("device.setSceneColor", &params, &state).unwrap();
    let operands = crate::device::operands::DispatchOperands::capture(
        "device.setSceneColor",
        &params,
        &plan.write,
        &state,
    )
    .unwrap();
    assert!(operands.validate(&state).is_ok());
    state.snapshot_mut().scene_colors.as_mut().unwrap()[2] = "#123456".into();
    assert!(operands.validate(&state).unwrap_err().contains("color"));
}

#[test]
fn scene_color_admission_requires_the_observed_rgb_before_transport() {
    let snapshot = GatewaySnapshot {
        preset_name: "Scene guard".into(),
        scene_colors: Some(vec!["#abcdef".into(); 8]),
        ..Default::default()
    };
    let params = json!({"scene": 2, "color": 0xffff02c2_u32,
            "expectedColor": "#ABCDEF", "expectedPresetName": "Scene guard"});
    assert!(plan_gateway_write("device.setSceneColor", &params, Some(&snapshot)).is_ok());
    for invalid in [
        Value::Null,
        json!(7),
        json!(""),
        json!("abcdef"),
        json!("#abcdef0"),
        json!("#abcdeg"),
        json!("#123456"),
    ] {
        let mut changed = params.clone();
        changed["expectedColor"] = invalid;
        assert!(
            plan_gateway_write("device.setSceneColor", &changed, Some(&snapshot)).is_err(),
            "invalid or stale expected color must reject: {changed}"
        );
    }
    let mut missing = params.clone();
    missing.as_object_mut().unwrap().remove("expectedColor");
    assert!(plan_gateway_write("device.setSceneColor", &missing, Some(&snapshot)).is_err());
    let unavailable = GatewaySnapshot {
        scene_colors: None,
        ..snapshot.clone()
    };
    assert!(plan_gateway_write("device.setSceneColor", &params, Some(&unavailable)).is_err());
    let short = GatewaySnapshot {
        scene_colors: Some(vec![]),
        ..snapshot
    };
    assert!(plan_gateway_write("device.setSceneColor", &params, Some(&short)).is_err());
}

#[test]
fn favorite_identity_and_plugin_flag_are_strict_at_the_runtime_boundary() {
    let base = json!({
        "name": "Stage",
        "folderKey": "/media/p4/Presets/Live",
        "productKey": "preset-stage",
        "isFactory": false,
        "favorite": true,
        "expectedFavorite": false
    });
    assert!(matches!(
        operation("setFavorite", &base).unwrap(),
        DeviceOperation::SetFavorite {
            is_plugin: false,
            ..
        }
    ));
    let mut plugin = base.clone();
    plugin["isPlugin"] = json!(true);
    assert!(matches!(
        operation("setFavorite", &plugin).unwrap(),
        DeviceOperation::SetFavorite {
            is_plugin: true,
            ..
        }
    ));
    assert!(
        crate::device::generated_gateway::validate_params("device.setFavorite", &plugin).is_ok()
    );

    let absent = json!({"entries": []});
    assert_eq!(
        gateway_write_preflight_method("device.setFavorite"),
        Some("device.favorites")
    );
    assert_eq!(
        gateway_write_preflight_decision("device.setFavorite", &base, &absent),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    let present = json!({"entries": [{
        "name": "Stage",
        "folderKey": "/media/p4/Presets/Live",
        "productKey": "preset-stage",
        "isFactory": false,
        "isPlugin": false
    }]});
    assert!(
        gateway_write_preflight_decision("device.setFavorite", &base, &present)
            .unwrap_err()
            .contains("stale favorite state")
    );
    // Cortex Control sends the CREATE or DELETE without comparing the
    // current state, so a write that already matches is still sent: the
    // guard only checks the caller's expectation and never suppresses.
    let mut already = base.clone();
    already["expectedFavorite"] = json!(true);
    assert_eq!(
        gateway_write_preflight_decision("device.setFavorite", &already, &present),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    let mut same_target = already.clone();
    same_target["favorite"] = json!(true);
    assert_eq!(
        gateway_write_preflight_decision("device.setFavorite", &same_target, &present),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    // No readback of any kind: the frame is sent and the result is
    // accepted_unverified (the QC's own T20 push is not read back).
    assert_eq!(gateway_write_readback_method("device.setFavorite"), None);
    assert!(!gateway_write_readback_matches(
        "device.setFavorite",
        &base,
        &present
    ));

    let plugin_without_product_key = json!({"entries": [{
        "name": "Stage",
        "folderKey": "/media/p4/Presets/Live",
        "isFactory": false,
        "isPlugin": true
    }]});
    // The guard still resolves a plugin entry that carries no product key.
    assert_eq!(
        gateway_write_preflight_decision(
            "device.setFavorite",
            &plugin,
            &plugin_without_product_key
        )
        .unwrap_err(),
        "write was based on stale favorite state"
    );

    for (field, value) in [
        ("name", json!("../Stage")),
        ("folderKey", json!("/media/p4/Presets/../Factory")),
        ("productKey", json!(" preset-stage")),
        ("isPlugin", json!(1)),
    ] {
        let mut invalid = plugin.clone();
        invalid[field] = value;
        assert!(operation("setFavorite", &invalid).is_err(), "{field}");
    }
}

#[test]
fn every_inline_guarded_public_write_requires_its_optimistic_token() {
    let complete_grid = GatewaySnapshot {
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    for (method, first_guard) in [
        ("device.selectScene", "expectedPresetName"),
        ("device.copyScene", "expectedPresetName"),
        ("device.setSceneLabel", "expectedLabel"),
        ("device.setSceneColor", "expectedColor"),
        ("device.pressFootswitch", "expectedMode"),
        ("device.tapTempo", "expectedMode"),
        ("device.selectModeSlot", "expectedPresetName"),
        ("device.setMasterVolume", "expectedValue"),
        ("device.setTempo", "expectedTempo"),
        ("device.toggleBypass", "expectedModelId"),
        ("device.setParameter", "expectedModelId"),
        ("device.previewParameter", "expectedModelId"),
        ("device.setParameterSceneMode", "expectedModelId"),
        ("device.setParameterExpression", "expectedModelId"),
        ("device.setLaneControlParameter", "expectedValue"),
        ("device.previewLaneControlParameter", "expectedValue"),
        ("device.setLaneControlSceneMode", "expectedPresetName"),
        ("device.setLaneControlExpression", "expectedPresetName"),
        ("device.setExpressionBypass", "expectedModelId"),
        ("device.moveBlock", "expectedModelId"),
        ("device.addBlock", "expectedModelId"),
        ("device.removeBlock", "expectedModelId"),
        ("device.setBlockFootswitch", "expectedFootswitch"),
        ("device.setStompMomentary", "expectedPresetName"),
        // Guarded like its sibling setSceneLabel: the stale-label check
        // is reported before the preset-identity one.
        ("device.setStompLabel", "expectedLabel"),
        ("device.setMidiOut", "expectedPresetName"),
        ("device.setPresetLoadMidiOut", "expectedPresetName"),
        ("device.setChainInput", "expectedInputId"),
        ("device.setChainOutput", "expectedOutputId"),
        ("device.setChainSplit", "expectedSplitColumn"),
        ("device.setSplitMute", "expectedMuted"),
        ("device.loadCapture", "expectedModelId"),
        ("device.loadIr", "expectedModelId"),
    ] {
        let error = plan_gateway_write(method, &json!({}), Some(&complete_grid)).expect_err(method);
        assert!(error.contains(first_guard), "{method}: {error}");
    }

    // This loop only pins whichever guard is reported first, so satisfying
    // that one must not be enough to plan a write. setStompLabel carries
    // two guards and is the row that drifted once already.
    let error = plan_gateway_write(
        "device.setStompLabel",
        &json!({"footswitch": 4, "label": "Solo", "expectedLabel": "Gate"}),
        Some(&complete_grid),
    )
    .expect_err("device.setStompLabel without expectedPresetName");
    assert!(error.contains("expectedPresetName"), "{error}");
}

#[test]
fn a_read_only_general_toggle_says_which_setting_and_why() {
    // "unsupported GeneralSettings toggle" named neither the setting nor the
    // reason, so a caller could not tell an unsupported setting from a
    // broken device - which is the exact wording of the contract's own note
    // on why every read-only entry carries a reason.
    for (setting, reason) in READ_ONLY_GENERAL_TOGGLES {
        let error = plan_gateway_write(
            "device.setGeneralToggle",
            &json!({"setting": setting, "enabled": true}),
            Some(&GatewaySnapshot::default()),
        )
        .expect_err("a read-only toggle must be refused");
        assert!(error.contains(setting), "{error}");
        assert!(error.contains("read-only"), "{error}");
        assert!(error.contains(reason), "{error}");
    }
    let unknown = plan_gateway_write(
        "device.setGeneralToggle",
        &json!({"setting": "notASetting", "enabled": true}),
        Some(&GatewaySnapshot::default()),
    )
    .expect_err("an unknown name must still be refused");
    assert!(unknown.contains("notASetting"), "{unknown}");
    assert!(
        !unknown.contains("read-only"),
        "an unknown name is not a read-only setting: {unknown}"
    );
}

#[test]
fn a_list_expectation_survives_an_option_count_change() {
    // SYNC NOTE is an options list, stored as index / (count - 1), and the
    // list is not the same length on every model - 21, 17, 14, 13 and 11 all
    // occur. On a block-enumerating list, adding or removing a block changes
    // the count on a row nobody wrote to, so the same selected option reads
    // back as a different float. A flat 1e-5 comparison calls that "the
    // parameter changed" where nothing changed.
    let params = json!({"expectedValue": 0.25});

    // Index 5 of 21 is 5/20 = 0.25; index 5 of 17 is 5/16 = 0.3125.
    assert!(
        assert_expected_parameter(Some(0.3125), &params).is_err(),
        "the flat comparison is what rejects an unchanged selection"
    );
    assert!(
        assert_expected_parameter_with_options(Some(0.3125), &params, Some((21, 17))).is_ok(),
        "the same selected option must pass across a count change"
    );

    // A genuinely different option is still refused: 0.375 * 16 = index 6.
    assert!(
        assert_expected_parameter_with_options(Some(0.375), &params, Some((21, 17))).is_err(),
        "rescaling must not turn the guard off"
    );

    // With the count unchanged, the index comparison also absorbs the
    // float32 storage error that the flat tolerance rejects.
    assert!(assert_expected_parameter(Some(0.250_04), &params).is_err());
    assert!(
        assert_expected_parameter_with_options(Some(0.250_04), &params, Some((21, 21))).is_ok()
    );

    // A continuous parameter keeps the flat comparison.
    assert!(assert_expected_parameter_with_options(Some(0.25), &params, None).is_ok());
    assert!(assert_expected_parameter_with_options(Some(0.30), &params, None).is_err());
    // A degenerate count cannot be rescaled and must not silently pass.
    assert!(assert_expected_parameter_with_options(Some(0.3125), &params, Some((1, 1))).is_err());
}

#[test]
fn an_output_level_readback_compares_decibels_against_decibels() {
    // `setOutputPort`'s `level` is a dB value from -40 through 12
    // (`optional_output_level`), but the readback compared it against the
    // response's `level`, which is the normalized 0..1 wire value. The two
    // agree at exactly one point, 0, which is the value that pulls the mains
    // down - so the only output level that ever reported a successful
    // readback was the one nobody wants. The input sibling at
    // `device.setInputPort` already compares ("levelDb", "levelDb").
    let response = |level: f64, level_db: f64| json!({"outputs": [{"outputPortId": 1, "level": level, "levelDb": level_db}]});
    assert!(
        gateway_write_readback_matches(
            "device.setOutputPort",
            &json!({"outputPortId": 1, "level": -6.0}),
            // -6 dB is normalized 0.0833 on the decoder's -12 + 72 * v ramp.
            &response(0.083_333, -6.0)
        ),
        "a -6 dB write must verify against the -6 dB the device reports"
    );
    assert!(
        !gateway_write_readback_matches(
            "device.setOutputPort",
            &json!({"outputPortId": 1, "level": -6.0}),
            &response(0.083_333, 3.0)
        ),
        "a device sitting at a different level must not verify"
    );
    assert!(
        !gateway_write_readback_matches(
            "device.setOutputPort",
            &json!({"outputPortId": 1, "level": 0.0}),
            // 0 dB is normalized 0.1667; the old comparison matched the
            // requested 0 against a normalized 0, which is -12 dB.
            &response(0.0, -12.0)
        ),
        "requesting 0 dB must not be satisfied by a normalized 0"
    );
}

#[test]
fn correlated_global_readbacks_match_only_the_fields_written() {
    // The pinned-model write has no readback of any kind any more.
    assert_eq!(gateway_write_readback_method("device.setModelPinned"), None);
    assert_eq!(
        gateway_write_readback_method("device.setMasterVolumeAssignment"),
        Some("device.generalSettings")
    );
    assert!(gateway_write_readback_matches(
        "device.setMasterVolumeAssignment",
        &json!({"out12": true, "out34": false, "send12": true, "headphones": false}),
        &json!({"masterVolumeAssignment": {"out12": true, "out34": false, "send12": true, "headphones": false}})
    ));
    assert!(!gateway_write_readback_matches(
        "device.setMasterVolumeAssignment",
        &json!({"out12": true, "out34": false, "send12": true, "headphones": false}),
        &json!({"masterVolumeAssignment": {"out12": true, "out34": true, "send12": true, "headphones": false}})
    ));
    assert!(gateway_write_readback_matches(
        "device.setInputPort",
        &json!({"inputPortId": 2, "levelDb": -10.0, "impedance": null}),
        &json!({"inputs": [{"inputPortId": 2, "levelDb": -10.0, "impedance": 1.0}]})
    ));
    assert!(!gateway_write_readback_matches(
        "device.setInputPort",
        &json!({"inputPortId": 2, "inputType": 0.5, "groundLift": 1.0}),
        &json!({"inputs": [{"inputPortId": 2, "inputType": 0.5, "groundLift": 0.0}]})
    ));
    assert!(!gateway_write_readback_matches(
        "device.setUsbPort",
        &json!({"level": null, "headphonesSource": 1.0 / 3.0, "dryWet": 1.0}),
        &json!({"usb": {"level": 0.75, "headphonesSource": 1.0 / 3.0, "dryWet": 0.0}})
    ));
    // Tempo, metronome and Global EQ writes have no readback either.
    for method in [
        "device.setTempoMetronome",
        "device.setGlobalEqBand",
        "device.setGlobalEqOutput",
        "device.setGlobalEqBypassed",
        "device.setModeCycle",
    ] {
        assert_eq!(gateway_write_readback_method(method), None, "{method}");
    }
}

#[test]
fn model_pinned_preflight_is_identity_scoped_and_stale_safe_but_never_suppresses() {
    let current = json!({"models": [7, 42], "captures": ["capture/key"]});
    assert_eq!(
        gateway_write_preflight_method("device.setModelPinned"),
        Some("device.pinnedModels")
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.setModelPinned",
            &json!({"modelId": 42, "pinned": true, "expectedPinned": true}),
            &current
        ),
        // Cortex Control sends the pin without comparing: so does the host.
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.setModelPinned",
            &json!({"modelId": 42, "pinned": false, "expectedPinned": true}),
            &current
        ),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    assert!(gateway_write_preflight_decision(
        "device.setModelPinned",
        &json!({"modelId": 42, "pinned": false, "expectedPinned": false}),
        &current
    )
    .is_err());
    assert!(gateway_write_preflight_decision(
        "device.setModelPinned",
        &json!({"modelId": 42, "pinned": false, "expectedPinned": true}),
        &json!({"captures": []})
    )
    .is_err());
}

#[test]
fn realtime_completion_policy_is_shared_for_performance_controls() {
    for method in [
        "device.selectScene",
        "device.toggleBypass",
        "device.previewParameter",
        "device.previewLaneControlParameter",
        "device.setTempo",
        "device.setMasterVolume",
        "device.pressFootswitch",
        "device.tapTempo",
        "device.showTuner",
        "device.showGigView",
        "device.controlLooper",
    ] {
        assert!(
            gateway_write_is_realtime(method),
            "{method} must complete immediately"
        );
    }
    for method in [
        "device.setParameter",
        "device.moveBlock",
        "device.recallPreset",
        "device.setSceneLabel",
        "device.createSetlist",
        "device.deleteSetlist",
        "device.deletePreset",
        "device.movePreset",
    ] {
        assert!(
            !gateway_write_is_realtime(method),
            "{method} requires its existing workflow"
        );
    }
}

#[test]
fn preview_parameter_is_the_real_write_with_host_local_no_readback_policy() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    state
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    name: Some(binary_preset::Name::Name("Live".into())),
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(2)),
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(41)),
                            column: Some(model::Column::Column(5)),
                            params: vec![Param {
                                index: Some(param::Index::Index(7)),
                                param_values: vec![ParamValue {
                                    value: Some(param_value::Value::FloatValue(0.25)),
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
    let params = json!({
        "row": 2,
        "column": 5,
        "parameterIndex": 7,
        "value": 0.75,
        "expectedValue": 0.25,
        "expectedScene": 0,
        "expectedModelId": 41, "expectedPresetName": "Live"
    });
    let preview =
        plan_gateway_write_with_state("device.previewParameter", &params, &state).unwrap();
    let committed = plan_gateway_write_with_state("device.setParameter", &params, &state).unwrap();
    assert_eq!(preview.write, committed.write);
    let PlannedWrite::HidCommand(command) = &preview.write else {
        panic!("ordinary preview must use the native Grid command");
    };
    let message = command.clone().try_encode().unwrap();
    assert_eq!(message.message_type, profile::MESSAGE_TYPE_GRID);
    assert_eq!(
        message.payload,
        [
            0x08, 0x01, 0x1a, 0x13, 0x5a, 0x11, 0x2a, 0x0d, 0x12, 0x09, 0x2a, 0x05, 0x15, 0x00,
            0x00, 0x40, 0x3f, 0x30, 0x07, 0x28, 0x05, 0x70, 0x02,
        ]
    );
    assert_eq!(preview.verification, GatewayVerification::None);
    assert!(matches!(
        committed.verification,
        GatewayVerification::Parameter {
            row: 2,
            column: 5,
            parameter_index: 7,
            value: 0.75,
            ..
        }
    ));
    assert!(gateway_write_is_realtime("device.previewParameter"));
    assert!(!gateway_write_is_realtime("device.setParameter"));

    let mut stale = params.clone();
    stale["expectedValue"] = json!(0.5);
    let error =
        plan_gateway_write_with_state("device.previewParameter", &stale, &state).unwrap_err();
    assert!(error.contains("parameter changed"));

    let missing = json!({
        "row": 2, "column": 5, "parameterIndex": 99, "value": 0.5,
        "expectedValue": 0.25, "expectedScene": 0, "expectedModelId": 41, "expectedPresetName": "Live"
    });
    let error =
        plan_gateway_write_with_state("device.previewParameter", &missing, &state).unwrap_err();
    assert!(error.contains("no synchronized numeric value"));
}

/// While a Delay block's SYNC toggle is engaged the device reports TIME
/// disabled and takes timing from SYNC NOTE; what the unit would do with a
/// numeric write to the disabled control is BELIEVED (a 2026-09-11
/// measurement recorded without an artifact), not established. The disabled state is
/// driven by the same `toggleOff`/`toggleOn` model-catalog wiring.
fn install_test_delay_catalog(state: &mut crate::device::state_runtime::DeviceStateRuntime) {
    let xml = br#"<Models><Category name="Delay">
          <Model id="9001" name="Test Delay">
            <Parameter name="SYNC" type="float" min="0" max="1" displayPos="0" />
            <Parameter name="TIME" type="float" min="1" max="4000" units="ms" displayPos="1" toggleOff="0" />
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
    state.install_model_catalog(catalog);
}

fn ingest_test_delay_preset(
    state: &mut crate::device::state_runtime::DeviceStateRuntime,
    sync_on: bool,
) {
    state
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    name: Some(binary_preset::Name::Name("Synced".into())),
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(0)),
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(9001)),
                            column: Some(model::Column::Column(3)),
                            params: vec![
                                Param {
                                    index: Some(param::Index::Index(0)),
                                    param_values: vec![ParamValue {
                                        value: Some(param_value::Value::FloatValue(if sync_on {
                                            1.0
                                        } else {
                                            0.0
                                        })),
                                    }],
                                    ..Default::default()
                                },
                                Param {
                                    index: Some(param::Index::Index(1)),
                                    param_values: vec![ParamValue {
                                        value: Some(param_value::Value::FloatValue(0.5)),
                                    }],
                                    ..Default::default()
                                },
                            ],
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
}

#[test]
fn set_parameter_rejects_a_sync_gated_time_write_and_names_sync() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_test_delay_catalog(&mut state);
    ingest_test_delay_preset(&mut state, true);

    let params = json!({
        "row": 0,
        "column": 3,
        "parameterIndex": 1,
        "value": 0.75,
        "expectedValue": 0.5,
        "expectedScene": 0,
        "expectedModelId": 9001, "expectedPresetName": "Synced"
    });

    let error = plan_gateway_write_with_state("device.setParameter", &params, &state).unwrap_err();
    assert!(error.contains("SYNC"), "error should name SYNC: {error}");
    assert!(
        error.contains("TIME"),
        "error should name the disabled parameter: {error}"
    );

    let error =
        plan_gateway_write_with_state("device.previewParameter", &params, &state).unwrap_err();
    assert!(error.contains("SYNC"), "error should name SYNC: {error}");
}

#[test]
fn set_parameter_allows_the_same_time_write_once_sync_is_off() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_test_delay_catalog(&mut state);
    ingest_test_delay_preset(&mut state, false);

    let params = json!({
        "row": 0,
        "column": 3,
        "parameterIndex": 1,
        "value": 0.75,
        "expectedValue": 0.5,
        "expectedScene": 0,
        "expectedModelId": 9001, "expectedPresetName": "Synced"
    });

    let plan = plan_gateway_write_with_state("device.setParameter", &params, &state).unwrap();
    assert!(matches!(plan.write, PlannedWrite::HidCommand(_)));
}

#[test]
fn retained_write_operands_detect_sync_changes_after_planning() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_test_delay_catalog(&mut state);
    ingest_test_delay_preset(&mut state, false);
    let params = json!({
        "row": 0, "column": 3, "parameterIndex": 1, "value": 0.75,
        "expectedValue": 0.5, "expectedScene": 0, "expectedModelId": 9001, "expectedPresetName": "Synced"
    });
    let plan = plan_gateway_write_with_state("device.setParameter", &params, &state).unwrap();
    let operands = crate::device::operands::DispatchOperands::capture(
        "device.setParameter",
        &params,
        &plan.write,
        &state,
    )
    .unwrap();
    assert!(operands.validate(&state).is_ok());
    ingest_test_delay_preset(&mut state, true);
    // TIME's value is unchanged. Its sibling SYNC now disables the target.
    assert_eq!(
        state.block_details(0, 3).unwrap().parameters[1].normalized_value,
        Some(0.5)
    );
    assert!(operands
        .validate(&state)
        .unwrap_err()
        .contains("target changed"));
}

#[test]
fn retained_parameter_guard_checks_the_normalized_value_again() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_test_delay_catalog(&mut state);
    ingest_test_delay_preset(&mut state, false);
    let params = json!({
        "row": 0, "column": 3, "parameterIndex": 0, "value": 1.0,
        "expectedValue": 0.0, "expectedScene": 0, "expectedModelId": 9001, "expectedPresetName": "Synced"
    });
    let plan = plan_gateway_write_with_state("device.setParameter", &params, &state).unwrap();
    let operands = crate::device::operands::DispatchOperands::capture(
        "device.setParameter",
        &params,
        &plan.write,
        &state,
    )
    .unwrap();
    ingest_test_delay_preset(&mut state, true);
    // The SYNC target and its metadata remain the same; only its value moves.
    let error = operands.validate(&state).unwrap_err();
    assert!(error.contains("changed"));
    assert!(!error.contains("target changed"));
}

/// The catalogue names the sync switch differently on different models.
/// Enumerated over all 633 models and 8851 parameters in
/// `docs/reference/qc-live-parameter-scales.json`, the only two-position
/// switches whose name carries "SYNC" and which actually gate a sibling
/// through `toggleOff` are `SYNC` (68 models), and Dual Delay's `SYNC L`
/// and `SYNC R` -- one per side, each gating only its own
/// `DELAY TIME L`/`DELAY TIME R`.
fn install_test_dual_delay_catalog(state: &mut crate::device::state_runtime::DeviceStateRuntime) {
    let xml = br#"<Models><Category name="Delay">
          <Model id="9002" name="Test Dual Delay">
            <Parameter name="SYNC L" type="switch" min="0" max="1" steps="2" stepNames="Off,On" displayPos="0" />
            <Parameter name="SYNC NOTE L" type="rotarySwitch" min="0" max="1" steps="2" stepNames="1/8,1/4" displayPos="1" toggleOn="0" />
            <Parameter name="DELAY TIME L" type="float" min="7" max="6000" units="ms" displayPos="2" toggleOff="0" />
            <Parameter name="SYNC R" type="switch" min="0" max="1" steps="2" stepNames="Off,On" displayPos="3" />
            <Parameter name="SYNC NOTE R" type="rotarySwitch" min="0" max="1" steps="2" stepNames="1/8,1/4" displayPos="4" toggleOn="3" />
            <Parameter name="DELAY TIME R" type="float" min="7" max="6000" units="ms" displayPos="5" toggleOff="3" />
          </Model>
        </Category></Models>"#;
    install_catalog_xml(state, xml);
}

fn install_catalog_xml(state: &mut crate::device::state_runtime::DeviceStateRuntime, xml: &[u8]) {
    let repo = pa::ModelRepoMessage {
        action: pa::message_action::Enum::Update as i32,
        request_id: None,
        model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
            xml.to_vec(),
        )),
    };
    let catalog = parse_model_repo(&repo.encode_to_vec()).unwrap();
    state.install_model_catalog(catalog);
}

fn ingest_test_block_preset(
    state: &mut crate::device::state_runtime::DeviceStateRuntime,
    model_id: u32,
    values: &[f64],
) {
    state
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    name: Some(binary_preset::Name::Name("Synced".into())),
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(0)),
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(model_id)),
                            column: Some(model::Column::Column(3)),
                            params: values
                                .iter()
                                .enumerate()
                                .map(|(index, value)| Param {
                                    index: Some(param::Index::Index(index as u32)),
                                    param_values: vec![ParamValue {
                                        value: Some(param_value::Value::FloatValue(*value as f32)),
                                    }],
                                    ..Default::default()
                                })
                                .collect(),
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
}

fn write_block_parameter(model_id: u32, parameter_index: u32, expected: f64) -> Value {
    json!({
        "expectedModelId": model_id, "row": 0,
        "column": 3,
        "parameterIndex": parameter_index,
        "value": 0.75,
        "expectedValue": expected,
        "expectedScene": 0,
        "expectedPresetName": "Synced"
    })
}

#[test]
fn set_parameter_rejects_a_per_side_sync_gated_time_write_naming_that_side() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_test_dual_delay_catalog(&mut state);
    // SYNC L on, SYNC R off.
    ingest_test_block_preset(&mut state, 9002, &[1.0, 0.0, 0.5, 0.0, 0.0, 0.5]);

    let error = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9002, 2, 0.5),
        &state,
    )
    .unwrap_err();
    assert!(
        error.contains("SYNC L"),
        "error should name the engaged side's switch: {error}"
    );
    assert!(
        error.contains("DELAY TIME L"),
        "error should name the disabled parameter: {error}"
    );

    // The right side is untouched: its own SYNC R is off, so its time
    // parameter must still be writable. Gating it on the left switch
    // would be the wrong-side failure a substring match invites.
    let plan = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9002, 5, 0.5),
        &state,
    )
    .unwrap();
    assert!(matches!(plan.write, PlannedWrite::HidCommand(_)));
}

#[test]
fn set_parameter_gates_each_dual_delay_side_on_its_own_sync_switch() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_test_dual_delay_catalog(&mut state);
    // SYNC L off, SYNC R on -- the mirror of the test above.
    ingest_test_block_preset(&mut state, 9002, &[0.0, 0.0, 0.5, 1.0, 0.0, 0.5]);

    let plan = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9002, 2, 0.5),
        &state,
    )
    .unwrap();
    assert!(matches!(plan.write, PlannedWrite::HidCommand(_)));

    let error = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9002, 5, 0.5),
        &state,
    )
    .unwrap_err();
    assert!(
        error.contains("SYNC R"),
        "error should name the engaged side's switch: {error}"
    );
}

/// Two catalogue parameters carry "SYNC" in their name without being a
/// note-division sync switch at all: Glitch's `SYNC SOURCE`
/// (`Global`/`Block`, selecting where the block takes its tempo) and
/// Harmonic Tremolo's `SYNC ON` (`Lows`/`Highs`, selecting which band the
/// tremolo syncs). Neither may be read as "SYNC is on", and neither
/// block's disabled control is gated by a note-division sync -- Glitch
/// has no SYNC NOTE rotary at all.
#[test]
fn a_disabled_control_behind_a_sync_named_selector_is_not_reported_as_sync_gated() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_catalog_xml(
            &mut state,
            br#"<Models><Category name="Morph">
          <Model id="9003" name="Test Glitch">
            <Parameter name="MODE" type="comboBox" min="0" max="3" steps="4" stepNames="Sequence,Pitch Delay,Interfere,Texturizer" displayPos="0" />
            <Parameter name="SYNC SOURCE" type="switch" min="0" max="1" steps="2" stepNames="Global,Block" displayPos="1" />
            <Parameter name="BLOCK TEMPO" type="float" min="40" max="240" units="BPM" displayPos="2" toggleOn="1" />
          </Model>
        </Category></Models>"#,
        );
    // SYNC SOURCE = Global, so BLOCK TEMPO is disabled -- for an
    // unrelated reason, and with nothing to redirect a write to.
    ingest_test_block_preset(&mut state, 9003, &[0.0, 0.0, 0.5]);

    let plan = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9003, 2, 0.5),
        &state,
    );
    assert!(
        plan.is_ok(),
        "a SYNC SOURCE selector must not be read as a sync gate: {:?}",
        plan.err()
    );
}

#[test]
fn harmonic_tremolos_band_selector_is_not_mistaken_for_its_sync_switch() {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    install_catalog_xml(
            &mut state,
            br#"<Models><Category name="Modulation">
          <Model id="9004" name="Test Harmonic Tremolo">
            <Parameter name="RATE" type="float" min="0.1" max="10" units="Hz" displayPos="0" toggleOff="3" />
            <Parameter name="SYNC NOTE" type="rotarySwitch" min="0" max="1" steps="2" stepNames="1/8,1/4" displayPos="1" toggleOn="3" />
            <Parameter name="SYNC ON" type="switch" min="0" max="1" steps="2" stepNames="Lows,Highs" displayPos="2" toggleOn="3" />
            <Parameter name="SYNC" type="switch" min="0" max="1" steps="2" stepNames="Off,On" displayPos="3" />
          </Model>
        </Category></Models>"#,
        );
    // SYNC off: RATE is live, and SYNC ON is the control the device
    // disables -- writing it is refused by nothing here, and certainly
    // not with "SYNC is on".
    ingest_test_block_preset(&mut state, 9004, &[0.5, 0.0, 0.0, 0.0]);
    assert!(plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9004, 0, 0.5),
        &state
    )
    .is_ok());
    let plan = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9004, 2, 0.0),
        &state,
    );
    assert!(
        plan.is_ok(),
        "a Lows/Highs selector must not be reported as SYNC-gated: {:?}",
        plan.err()
    );

    // SYNC on: RATE is the device-disabled control and must be refused.
    ingest_test_block_preset(&mut state, 9004, &[0.5, 0.0, 0.0, 1.0]);
    let error = plan_gateway_write_with_state(
        "device.setParameter",
        &write_block_parameter(9004, 0, 0.5),
        &state,
    )
    .unwrap_err();
    assert!(error.contains("SYNC"), "error should name SYNC: {error}");
    assert!(
        error.contains("RATE"),
        "error should name the disabled parameter: {error}"
    );
    assert!(
        error.contains("host policy") && error.contains("BELIEVED"),
        "error should label the refusal as host policy and not claim a device behaviour: {error}"
    );
    assert!(
        !error.to_lowercase().contains("redirect"),
        "error must not assert that the unit redirects the write: {error}"
    );
}

#[test]
fn preset_tempo_is_guarded_bounded_and_never_same_value_elided() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        tempo: 100,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let params = json!({
        "bpm": 120,
        "expectedTempo": 100,
        "expectedPresetName": "Live"
    });
    let first = plan_gateway_write("device.setTempo", &params, Some(&snapshot)).unwrap();
    assert_eq!(
        first,
        GatewayWritePlan {
            write: PlannedWrite::HidCommand(DeviceCommand::SetTempo(120)),
            detail: "Tempo set to 120 BPM".into(),
            verification: GatewayVerification::Tempo { bpm: 120 },
        }
    );
    assert_eq!(gateway_write_preflight_method("device.setTempo"), None);
    assert!(gateway_write_is_realtime("device.setTempo"));

    let same_value = json!({
        "bpm": 100,
        "expectedTempo": 100,
        "expectedPresetName": "Live"
    });
    assert!(matches!(
        plan_gateway_write("device.setTempo", &same_value, Some(&snapshot))
            .unwrap()
            .write,
        PlannedWrite::HidCommand(DeviceCommand::SetTempo(100))
    ));
    for bpm in [39, 241] {
        let mut invalid = params.clone();
        invalid["bpm"] = json!(bpm);
        assert!(plan_gateway_write("device.setTempo", &invalid, Some(&snapshot)).is_err());
    }
    let mut stale_tempo = params.clone();
    stale_tempo["expectedTempo"] = json!(99);
    assert!(plan_gateway_write("device.setTempo", &stale_tempo, Some(&snapshot)).is_err());
    let mut stale_preset = params;
    stale_preset["expectedPresetName"] = json!("Other");
    assert!(plan_gateway_write("device.setTempo", &stale_preset, Some(&snapshot)).is_err());
}

#[test]
fn show_gig_view_is_a_one_frame_unverified_desired_state_write() {
    for shown in [false, true] {
        let first =
            plan_gateway_write("device.showGigView", &json!({"shown": shown}), None).unwrap();
        let repeated =
            plan_gateway_write("device.showGigView", &json!({"shown": shown}), None).unwrap();
        assert_eq!(first, repeated, "same-value calls are not elided");
        assert_eq!(
            first.write,
            PlannedWrite::HidCommand(DeviceCommand::ShowGigView(shown))
        );
        assert_eq!(first.verification, GatewayVerification::None);
        assert!(gateway_write_is_realtime("device.showGigView"));
    }

    for invalid in [json!({}), json!({"shown": 1}), json!({"shown": null})] {
        assert_eq!(
            plan_gateway_write("device.showGigView", &invalid, None).unwrap_err(),
            "shown must be a boolean"
        );
    }
}

#[test]
fn block_removal_observation_is_specific_fresh_and_bounded() {
    let remove = GatewayVerification::Block {
        row: 0,
        column: 1,
        model_id: None,
        present: false,
    };
    assert!(remove.observes_state_kinds(["blockRemoved"]));
    assert!(!GatewayVerification::Block {
        row: 0,
        column: 1,
        model_id: Some(9013),
        present: true
    }
    .observes_state_kinds(["blockRemoved"]));
    assert!(!GatewayVerification::Tempo { bpm: 120 }.observes_state_kinds(["blockRemoved"]));
    let policy = gateway_write_verification_policy("device.removeBlock");
    assert!(policy.readback_method.is_none());
    let mut wait = GatewayVerificationRuntime::new(remove, 7, 100, &policy);
    let absent = GatewaySnapshot::default();
    assert!(matches!(
        wait.advance(Some(&absent), None, 7, 101),
        GatewayVerificationAction::Wait { .. }
    ));
    assert_eq!(
        wait.advance(Some(&absent), None, 8, 102),
        GatewayVerificationAction::Verified
    );
    assert_eq!(
        wait.advance(Some(&absent), None, 7, 100 + policy.timeout_ms),
        GatewayVerificationAction::TimedOut
    );
}

#[test]
fn verification_is_passive_and_its_deadlines_are_shared_by_hosts() {
    // No write schedules a READ of any kind: the policy has no refresh
    // schedule at all (the type-15 verification READs at 0/250/1500 ms and
    // the preset-transition ladders are removed), and the only readbacks
    // left poll a retained publication and send nothing.
    let command = gateway_write_verification_policy("device.setMidiOut");
    assert_eq!(command.timeout_ms, profile::COMMAND_CONFIRMATION_TIMEOUT_MS);
    assert_eq!(
        command.correlated_readback_retry_intervals_ms,
        profile::CORRELATED_WRITE_READBACK_RETRY_INTERVALS_MS
    );
    assert_eq!(
        gateway_correlated_readback_delay("device.setParameter", 0),
        Some(0)
    );
    assert_eq!(
        gateway_correlated_readback_delay("device.setParameter", 4),
        Some(1_000)
    );
    assert_eq!(
        gateway_correlated_readback_delay("device.setParameter", 5),
        None
    );
    assert_eq!(command.preflight_method, None);
    assert_eq!(command.readback_method, None);

    // undo and redo send one UndoRedo UPDATE frame and no read after it:
    // like every other write they carry no readback and no preflight.
    for method in ["device.undo", "device.redo"] {
        let history = gateway_write_verification_policy(method);
        assert_eq!(history.preflight_method, None);
        assert_eq!(history.readback_method, None);
    }

    // Only the two settings families CorOS publishes have a readback, and it
    // is a poll of the retained publication. Every other write family that
    // used to poll a READ (global EQ, mode cycle, tempo and metronome,
    // favorites, pinned models) is accepted_unverified after delivery.
    for method in [
        "device.setGlobalEqBypassed",
        "device.setGlobalEqBand",
        "device.setGlobalEqOutput",
        "device.setModeCycle",
        "device.setTempoMetronome",
        "device.setTempoMode",
        "device.setGlobalTempo",
        "device.setFavorite",
        "device.setModelPinned",
    ] {
        assert_eq!(gateway_write_readback_method(method), None, "{method}");
        assert_eq!(
            gateway_write_verification_policy(method).readback_method,
            None,
            "{method}"
        );
    }
    for (method, published) in [
        ("device.setGeneralInteger", "device.generalSettings"),
        ("device.setMasterVolumeAssignment", "device.generalSettings"),
        ("device.setInputPort", "device.ioSettings"),
        ("device.setOutputPairing", "device.ioSettings"),
    ] {
        assert_eq!(gateway_write_readback_method(method), Some(published));
        assert!(gateway_readback_is_published(published));
    }

    // The tempo guards consult decoded state; none of them reads the wire,
    // and none of them carries a readback.
    let global_tempo = gateway_write_verification_policy("device.setGlobalTempo");
    assert_eq!(
        global_tempo.preflight_method,
        Some("device.globalTempoSettings")
    );
    assert_eq!(
        global_tempo.preflight_observation_source,
        Some(GatewayPreflightObservationSource::DecodedSnapshot)
    );
    assert_eq!(global_tempo.readback_method, None);
    // Favorite and pinned guards read the retained lists, never the wire.
    for method in ["device.setFavorite", "device.setModelPinned"] {
        assert_eq!(
            gateway_write_verification_policy(method).preflight_observation_source,
            Some(GatewayPreflightObservationSource::DecodedSnapshot)
        );
    }

    // Screen gestures take no preflight. The capture they used to force
    // was never read, so it bought a framebuffer, not a decision.
    for method in [
        "device.tapScreen",
        "device.swipeScreen",
        "device.dragScreen",
    ] {
        assert_eq!(
            gateway_write_verification_policy(method).preflight_method,
            None
        );
    }

    // A preset transition keeps its longer deadline and nothing else: a
    // recall receives type-2 position and type-15 preset frames unasked.
    for method in [
        "device.recallPreset",
        "device.navigateBank",
        "device.reloadPreset",
    ] {
        let preset = gateway_write_verification_policy(method);
        assert_eq!(preset.timeout_ms, profile::PRESET_SYNC_TIMEOUT_MS);
    }
}

#[test]
fn no_preflight_ever_names_a_wire_read_budget() {
    // The write budget is dispatch plus verification: no preflight read.
    let write = plan_gateway_write(
        "device.setFavorite",
        &json!({
            "name": "Stage", "folderKey": "/media/p4/Presets/Live",
            "productKey": "preset-stage", "isFactory": false,
            "favorite": true, "expectedFavorite": false
        }),
        None,
    )
    .unwrap();
    let dispatch = planned_write_dispatch_budget_ms(&write.write).unwrap();
    assert_eq!(
        gateway_write_budget_ms("device.setFavorite", &write.write).unwrap(),
        dispatch + profile::COMMAND_CONFIRMATION_TIMEOUT_MS
    );
}

#[test]
fn redo_device_is_one_non_replayable_unverified_history_write() {
    let first = plan_gateway_write("device.redo", &json!({}), None).unwrap();
    let repeated = plan_gateway_write("device.redo", &json!({}), None).unwrap();
    assert_eq!(
        first, repeated,
        "each invocation creates the same one-shot plan"
    );
    assert_eq!(
        first.write,
        PlannedWrite::HidOperation(DeviceOperation::Redo)
    );
    assert_eq!(first.verification, GatewayVerification::None);
    assert_eq!(first.detail, "Device redo sent");
    assert!(!gateway_write_is_realtime("device.redo"));

    let frames = match first.write {
        PlannedWrite::HidOperation(operation) => operation.try_encode().unwrap(),
        other => panic!("unexpected redo lane: {other:?}"),
    };
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_UNDO_REDO);
    assert_eq!(frames[0].payload, [0x08, 0x01, 0x30, 0x01]);

    let policy = gateway_write_verification_policy("device.redo");
    assert!(policy.preflight_method.is_none());
    assert!(policy.readback_method.is_none());
}

#[test]
fn event_transactions_share_freshness_matching_and_timeout_policy() {
    let before = GatewaySnapshot {
        tempo: 100,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let after = GatewaySnapshot {
        tempo: 120,
        ..before.clone()
    };
    let transaction =
        GatewayTransaction::new(GatewayVerification::Tempo { bpm: 120 }, 5_000, 5_000, 650);
    assert_eq!(
        transaction.state(&after, None, 5_000, 5_001),
        GatewayTransactionState::Pending,
        "a matching snapshot that predates the write is not authoritative"
    );
    assert_eq!(
        transaction.state(&before, None, 5_001, 5_100),
        GatewayTransactionState::Pending,
        "a fresh event with the wrong value does not acknowledge the write"
    );
    assert_eq!(
        transaction.state(&after, None, 5_001, 5_100),
        GatewayTransactionState::Verified
    );
    assert_eq!(transaction.remaining_ms(5_600), 50);
    assert_eq!(
        transaction.state(&after, None, 5_700, 5_700),
        GatewayTransactionState::TimedOut
    );
}

#[test]
fn a_tempo_clock_tick_is_not_a_write_observation() {
    // Live evidence, `device.stateEvents`: sequences 542-545 all carried
    // `"states":[]` with only a tempo tick, inside one millisecond, and
    // `latestSequence` moved 258 -> 633 in ~6 minutes on an idle device.
    // `worker.rs` publishes such a frame to the same subscriber list as a
    // real state update (it drops a frame only when states *and* tempo
    // clock are both absent), and the verifier took any newer sequence as
    // freshness. With an absolute predicate against the cached snapshot,
    // that certified every write whose requested value already equalled the
    // current one — including Pattern Tremolo's RATE, a SYNC-gated control
    // that stays `enabled: true` with SYNC on.
    let parameter = GatewayVerification::Parameter {
        row: 1,
        column: 3,
        parameter_index: 7,
        value: 0.5,
        routing_model_id: None,
    };
    assert!(
        !parameter.observes_state_kinds(std::iter::empty::<&str>()),
        "a tempo-clock-only frame carries no state update and proves nothing"
    );
    assert!(
        parameter.observes_state_kinds(["preset"]),
        "the verification refresh is device.currentPreset, which republishes the block"
    );
    assert!(
        !parameter.observes_state_kinds(["dirty"]),
        "the write dirties the preset, but a dirty flag carries no parameter value"
    );

    // Each verification accepts the kinds that can actually carry its
    // evidence, and a full preset publication re-establishes all of them.
    for (verification, own_kind) in [
        (GatewayVerification::Tempo { bpm: 120 }, "tempo"),
        (GatewayVerification::MasterVolume { value: 50 }, "master"),
        (GatewayVerification::Scene { scene: 2 }, "scene"),
        (GatewayVerification::Mode { mode: 1 }, "mode"),
        (
            GatewayVerification::Bypass {
                row: 0,
                column: 1,
                bypassed: true,
            },
            "bypass",
        ),
    ] {
        assert!(
            verification.observes_state_kinds([own_kind]),
            "{verification:?} must accept its own {own_kind} update"
        );
        assert!(
            verification.observes_state_kinds(["preset"]),
            "{verification:?} must accept a full preset publication"
        );
        assert!(
            !verification.observes_state_kinds(std::iter::empty::<&str>()),
            "{verification:?} must reject a frame with no state update at all"
        );
    }
}

#[test]
fn verification_runtime_is_passive_and_owns_waits_and_deadline() {
    let policy = GatewayWriteVerificationPolicy {
        timeout_ms: 650,
        correlated_readback_retry_intervals_ms: Vec::new(),
        preflight_method: None,
        preflight_observation_source: None,
        readback_method: None,
    };
    let mut runtime =
        GatewayVerificationRuntime::new(GatewayVerification::Tempo { bpm: 120 }, 5, 1_000, &policy);
    let before = GatewaySnapshot {
        tempo: 100,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let after = GatewaySnapshot {
        tempo: 120,
        ..before.clone()
    };
    // It never asks the host to send anything: with no evidence it only
    // waits, for exactly the time left.
    assert_eq!(
        runtime.advance(Some(&before), None, 5, 1_000),
        GatewayVerificationAction::Wait { delay_ms: 650 }
    );
    assert_eq!(
        runtime.advance(Some(&before), None, 6, 1_001),
        GatewayVerificationAction::Wait { delay_ms: 649 }
    );
    assert_eq!(
        runtime.advance(Some(&after), None, 6, 1_100),
        GatewayVerificationAction::Verified
    );

    let mut timeout =
        GatewayVerificationRuntime::new(GatewayVerification::Tempo { bpm: 120 }, 5, 1_000, &policy);
    assert_eq!(
        timeout.advance(Some(&before), None, 5, 1_650),
        GatewayVerificationAction::TimedOut
    );
}

#[test]
fn restrict_deadline_shrinks_the_wait_delay_not_just_the_timeout() {
    // Regression test for the old two-field deadline: restrict_deadline
    // used to shrink the runtime's own copy while the Wait-delay branch
    // of advance() kept reading that same outer copy directly. With a
    // single shared GatewayTransaction deadline there is no second copy
    // left to fall out of sync.
    let policy = GatewayWriteVerificationPolicy {
        timeout_ms: 5_000,
        correlated_readback_retry_intervals_ms: Vec::new(),
        preflight_method: None,
        preflight_observation_source: None,
        readback_method: None,
    };
    let mut runtime =
        GatewayVerificationRuntime::new(GatewayVerification::Tempo { bpm: 120 }, 5, 1_000, &policy);
    let snapshot = GatewaySnapshot {
        tempo: 100,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };

    // Shrink the deadline well below the policy's own 5 s.
    runtime.restrict_deadline(crate::device::correlation::RequestDeadline::new(1_000, 300));

    // The Wait branch must report the shrunk deadline (300ms after the
    // 1_000ms start, so 200ms of remaining time at now=1_100).
    assert_eq!(
        runtime.advance(Some(&snapshot), None, 5, 1_100),
        GatewayVerificationAction::Wait { delay_ms: 200 }
    );

    // And the transaction itself now times out at the shrunk deadline.
    assert_eq!(
        runtime.advance(Some(&snapshot), None, 5, 1_300),
        GatewayVerificationAction::TimedOut
    );
}

#[test]
fn unverified_writes_never_match_an_unrelated_snapshot() {
    let snapshot = GatewaySnapshot::default();
    let verification = GatewayVerification::None;
    assert!(!verification.requires_authoritative_readback());
    assert!(!verification.matches(&snapshot, None));

    let transaction = GatewayTransaction::new(verification, 5_000, 5_000, 650);
    assert_eq!(
        transaction.state(&snapshot, None, 5_001, 5_100),
        GatewayTransactionState::Pending
    );
    assert_eq!(
        transaction.state(&snapshot, None, 5_001, 5_700),
        GatewayTransactionState::TimedOut
    );
}

#[test]
fn expected_state_merge_is_shared_and_preserves_explicit_arguments() {
    let merged = merge_expected_state(
        &json!({"scene": 2, "expectedTempo": 90}),
        &json!({"presetName": "Live", "presetPosition": 7, "activeScene": 1, "tempo": 120}),
    );
    assert_eq!(merged["expectedPresetName"], "Live");
    assert_eq!(merged["expectedPosition"], 7);
    assert_eq!(merged["expectedScene"], 1);
    assert_eq!(merged["expectedTempo"], 90);
}

#[test]
fn expected_state_and_ranges_are_validated_once() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    assert!(plan_gateway_write(
        "device.selectScene",
        &json!({"scene": 2, "expectedPresetName": "Live"}),
        Some(&snapshot)
    )
    .is_ok());
    assert!(plan_gateway_write(
        "device.selectScene",
        &json!({"scene": 8, "expectedPresetName": "Live"}),
        Some(&snapshot)
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.selectScene",
        &json!({"scene": 2, "expectedPresetName": "Other"}),
        Some(&snapshot)
    )
    .is_err());
}

#[test]
fn select_scene_preflight_suppresses_unchanged_and_rejects_stale_state() {
    assert_eq!(
        gateway_write_preflight_method("device.selectScene"),
        Some("device.currentPreset")
    );
    let params = json!({"scene": 3, "expectedPresetName": "Live"});
    assert_eq!(
        gateway_write_preflight_decision(
            "device.selectScene",
            &params,
            &json!({"presetName": "Live", "activeScene": 3})
        ),
        Ok(GatewayWritePreflightDecision::AlreadyMatched)
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.selectScene",
            &params,
            &json!({"presetName": "Live", "activeScene": 2})
        ),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    for stale in [
        json!({"presetName": "Other", "activeScene": 2}),
        json!({"presetName": "Live"}),
        json!({"presetName": "Live", "activeScene": 8}),
    ] {
        assert!(gateway_write_preflight_decision("device.selectScene", &params, &stale).is_err());
    }
}

#[test]
fn hid_and_midi_execution_lanes_are_host_independent() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        mode: "STOMP".into(),
        mode_slots: Some(vec![
            ModeSlot {
                slot: 0,
                label: "STOMP / SCENE".into(),
                mode: "HYBRID".into(),
                ..Default::default()
            },
            ModeSlot {
                slot: 1,
                label: "PRESET".into(),
                mode: "PRESET".into(),
                ..Default::default()
            },
            ModeSlot {
                slot: 2,
                label: "SCENE / STOMP".into(),
                mode: "HYBRID".into(),
                ..Default::default()
            },
        ]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let scene = plan_gateway_write(
        "device.selectScene",
        &json!({"scene": 3, "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(matches!(
        scene.write,
        PlannedWrite::HidCommand(DeviceCommand::SelectScene(3))
    ));

    let footswitch = plan_gateway_write(
        "device.pressFootswitch",
        &json!({"index": 7, "expectedMode": "STOMP", "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        footswitch.write,
        PlannedWrite::MidiControlChange {
            controller: profile::FOOTSWITCH_BASE_CONTROLLER + 7,
            value: profile::MIDI_PRESSED_VALUE,
        }
    );

    assert!(plan_gateway_write(
        "device.pressFootswitch",
        &json!({"index": 8, "expectedMode": "STOMP", "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .is_err());

    let tap = plan_gateway_write(
        "device.tapTempo",
        &json!({"expectedMode": "STOMP", "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        tap.write,
        PlannedWrite::MidiControlChange {
            controller: profile::TAP_TEMPO_CONTROLLER,
            value: profile::MIDI_PRESSED_VALUE,
        }
    );

    let mode = plan_gateway_write(
        "device.selectModeSlot",
        &json!({"slot": 1, "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        mode.write,
        PlannedWrite::HidCommand(DeviceCommand::SelectMode(0))
    );
    assert_eq!(mode.verification, GatewayVerification::Mode { mode: 0 });
    // Slot C holds the hybrid SCENE / STOMP, whose wire value is unmeasured.
    assert!(plan_gateway_write(
        "device.selectModeSlot",
        &json!({"slot": 2, "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap_err()
    .contains("has not been measured"));

    for shown in [false, true] {
        let tuner = plan_gateway_write("device.showTuner", &json!({"shown": shown}), None).unwrap();
        assert_eq!(
            tuner.write,
            PlannedWrite::HidOperation(DeviceOperation::ShowTuner(shown))
        );
        assert_eq!(tuner.verification, GatewayVerification::None);
        assert_eq!(tuner.write.inter_message_interval_ms(), 0);
    }
    let gig_view = plan_gateway_write(
        "device.showGigView",
        &json!({"shown": false}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        gig_view.write,
        PlannedWrite::HidCommand(DeviceCommand::ShowGigView(false))
    );
    assert!(plan_gateway_write("device.showTuner", &json!({}), Some(&snapshot)).is_err());
    assert!(plan_gateway_write(
        "device.pressFootswitch",
        &json!({"index": 0, "expectedMode": "SCENE", "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .is_err());
}

#[test]
fn select_mode_slot_resolves_exact_configured_modes_and_fails_closed() {
    let labels = [
        ("PRESET", "PRESET"),
        ("SCENE", "SCENE"),
        ("STOMP", "STOMP"),
        ("PRESET / SCENE", "HYBRID"),
        ("PRESET / STOMP", "HYBRID"),
        ("SCENE / PRESET", "HYBRID"),
        ("SCENE / STOMP", "HYBRID"),
        ("STOMP / PRESET", "HYBRID"),
        ("STOMP / SCENE", "HYBRID"),
    ];
    for (mode, (label, kind)) in labels.into_iter().enumerate() {
        let snapshot = GatewaySnapshot {
            preset_name: "Live".into(),
            mode: kind.into(),
            mode_slots: Some(vec![ModeSlot {
                slot: 0,
                label: label.into(),
                mode: kind.into(),
                ..Default::default()
            }]),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        };
        let planned = plan_gateway_write(
            "device.selectModeSlot",
            &json!({"slot": 0, "expectedPresetName": "Live"}),
            Some(&snapshot),
        );
        if kind == "HYBRID" {
            // The hybrid wire values 3 to 8 are unmeasured: refused before any frame exists.
            let error = planned.unwrap_err();
            assert!(error.contains("Mode Slot 1"), "{error}");
            assert!(error.contains(label), "{error}");
            assert!(error.contains("has not been measured"), "{error}");
            continue;
        }
        let plan = planned.unwrap();
        assert_eq!(
            plan.write,
            PlannedWrite::HidCommand(DeviceCommand::SelectMode(mode as u32))
        );
        assert_eq!(
            plan.verification,
            GatewayVerification::Mode { mode: mode as u32 }
        );
    }

    let reordered = GatewaySnapshot {
        preset_name: "Live".into(),
        mode: "STOMP".into(),
        mode_slots: Some(vec![
            ModeSlot {
                slot: 0,
                label: "SCENE".into(),
                mode: "SCENE".into(),
                ..Default::default()
            },
            ModeSlot {
                slot: 1,
                label: "PRESET".into(),
                mode: "PRESET".into(),
                ..Default::default()
            },
            ModeSlot {
                slot: 2,
                label: "STOMP".into(),
                mode: "STOMP".into(),
                ..Default::default()
            },
        ]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    for (slot, expected_mode) in [(0, 1), (1, 0), (2, 2)] {
        let plan = plan_gateway_write(
            "device.selectModeSlot",
            &json!({"slot": slot, "expectedPresetName": "Live"}),
            Some(&reordered),
        )
        .unwrap();
        assert_eq!(
            plan.write,
            PlannedWrite::HidCommand(DeviceCommand::SelectMode(expected_mode))
        );
        assert_eq!(
            plan.verification,
            GatewayVerification::Mode {
                mode: expected_mode,
            }
        );
    }

    let params = json!({"slot": 0, "expectedPresetName": "Live"});
    assert!(plan_gateway_write("device.selectModeSlot", &params, None).is_err());
    for mode_slots in [
        vec![],
        vec![ModeSlot {
            slot: 0,
            label: "PRESET / SCENE".into(),
            mode: "PRESET".into(),
            ..Default::default()
        }],
        vec![ModeSlot {
            slot: 0,
            label: "UNKNOWN".into(),
            mode: "HYBRID".into(),
            ..Default::default()
        }],
        vec![ModeSlot {
            slot: 1,
            label: "SCENE".into(),
            mode: "SCENE".into(),
            ..Default::default()
        }],
    ] {
        let snapshot = GatewaySnapshot {
            preset_name: "Live".into(),
            mode_slots: Some(mode_slots),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        };
        assert!(plan_gateway_write("device.selectModeSlot", &params, Some(&snapshot)).is_err());
    }
}

#[test]
fn select_mode_slot_refuses_a_configured_hybrid_slot_before_any_frame_is_encoded() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        mode: "STOMP".into(),
        mode_slots: Some(vec![
            ModeSlot {
                slot: 0,
                label: "PRESET".into(),
                mode: "PRESET".into(),
                ..Default::default()
            },
            ModeSlot {
                slot: 1,
                label: "PRESET / SCENE".into(),
                mode: "HYBRID".into(),
                ..Default::default()
            },
            ModeSlot {
                slot: 2,
                label: "STOMP".into(),
                mode: "STOMP".into(),
                ..Default::default()
            },
        ]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let select = |slot: u32| {
        plan_gateway_write(
            "device.selectModeSlot",
            &json!({"slot": slot, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
    };
    // No plan means no PlannedWrite, so nothing reaches an encoder or the wire.
    let error = select(1).unwrap_err();
    assert_eq!(
        error,
        "Mode Slot 2 is configured as the hybrid mode \"PRESET / SCENE\". Selecting a hybrid mode slot is unsupported: the wire value for hybrid modes has not been measured. Nothing was sent."
    );
    // The base modes beside it keep their exact wire values.
    for (slot, mode) in [(0, 0), (2, 2)] {
        let plan = select(slot).unwrap();
        assert_eq!(
            plan.write,
            PlannedWrite::HidCommand(DeviceCommand::SelectMode(mode))
        );
        assert_eq!(plan.verification, GatewayVerification::Mode { mode });
    }
    // A direct encoder call cannot send a hybrid value either.
    for mode in 3..=8 {
        assert!(DeviceCommand::SelectMode(mode).try_encode().is_err());
    }
}

#[test]
fn select_mode_slot_completes_at_enqueue_like_the_desktop_sender() {
    // Desktop send completion is enqueue-only; no command ACK, request
    // correlation, readback, or timeout exists.
    let policy = gateway_write_verification_policy("device.selectModeSlot");
    assert!(gateway_write_is_realtime("device.selectModeSlot"));
    assert_eq!(policy.timeout_ms, profile::COMMAND_CONFIRMATION_TIMEOUT_MS);

    let before = GatewaySnapshot {
        mode: "STOMP".into(),
        footswitch_modes: Some(vec!["STOMP".into(), "STOMP".into()]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let after = GatewaySnapshot {
        mode: "SCENE".into(),
        footswitch_modes: Some(vec!["SCENE".into(), "SCENE".into()]),
        ..before.clone()
    };
    // The Mode predicate itself is retained and still discriminates an
    // exact base or directed-hybrid identity; the realtime path simply
    // never consumes it. Refresh-ladder timing is covered by
    // verification_runtime_owns_refresh_order_waits_and_deadline.
    assert!(!GatewayVerification::Mode { mode: 1 }.matches(&before, None));
    assert!(GatewayVerification::Mode { mode: 1 }.matches(&after, None));
    assert!(!GatewayVerification::Mode { mode: 0 }.matches(&before, None));

    let wrong_hybrid = GatewaySnapshot {
        mode: "HYBRID".into(),
        footswitch_modes: Some(vec!["PRESET".into(), "SCENE".into()]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    assert!(!GatewayVerification::Mode { mode: 6 }.matches(&wrong_hybrid, None));
}

#[test]
fn press_footswitch_uses_exact_a_through_h_host_midi_contract() {
    for index in 0_u8..8 {
        assert_eq!(
            plan_host_midi(
                "device.pressFootswitch",
                &json!({"index": index, "expectedMode": "STOMP"}),
            )
            .unwrap(),
            HostMidiPlan {
                controller: 35 + index,
                value: 127,
                detail: format!("Footswitch index {index} sent"),
            }
        );
    }
    for params in [
        json!({"index": 8, "expectedMode": "STOMP"}),
        json!({"index": 0, "expectedMode": "UNKNOWN"}),
    ] {
        assert!(plan_host_midi("device.pressFootswitch", &params).is_err());
    }

    let dirty_preset = GatewaySnapshot {
        preset_name: "Live".into(),
        mode: "PRESET".into(),
        dirty: true,
        has_preset: true,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let error = plan_gateway_write(
        "device.pressFootswitch",
        &json!({"index": 0, "expectedMode": "PRESET", "expectedPresetName": "Live"}),
        Some(&dirty_preset),
    )
    .unwrap_err();
    assert!(error.contains("unsaved changes"));

    let mut dirty_scene = dirty_preset;
    dirty_scene.mode = "SCENE".into();
    assert!(plan_gateway_write(
        "device.pressFootswitch",
        &json!({"index": 0, "expectedMode": "SCENE", "expectedPresetName": "Live"}),
        Some(&dirty_scene),
    )
    .is_ok());
}

#[test]
fn tap_tempo_is_one_repeatable_guarded_host_midi_event() {
    for method in ["device.pressFootswitch", "device.tapTempo"] {
        let params = if method == "device.pressFootswitch" {
            json!({"index": 0, "expectedMode": "SCENE", "expectedPresetName": "Live", "expectedPresetIdentity": "fixture-native-A"})
        } else {
            json!({"expectedMode": "SCENE", "expectedPresetName": "Live", "expectedPresetIdentity": "fixture-native-A"})
        };
        crate::device::generated_gateway::validate_params(method, &params).unwrap_or_else(
            |error| panic!("{method} must accept its contracted string mode: {error}"),
        );
    }
    let expected = HostMidiPlan {
        controller: 44,
        value: 127,
        detail: "Tap Tempo sent".into(),
    };
    for _ in 0..2 {
        assert_eq!(
            plan_host_midi("device.tapTempo", &json!({"expectedMode": "SCENE"})).unwrap(),
            expected
        );
    }
    for mode in ["", "GLOBAL", "scene", "UNKNOWN"] {
        assert!(plan_host_midi("device.tapTempo", &json!({"expectedMode": mode})).is_err());
    }

    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        mode: "SCENE".into(),
        has_preset: true,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write(
        "device.tapTempo",
        &json!({"expectedMode": "SCENE", "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        plan.write,
        PlannedWrite::MidiControlChange {
            controller: 44,
            value: 127,
        }
    );
    assert!(plan.verification.is_none());
    assert!(plan_gateway_write(
        "device.tapTempo",
        &json!({"expectedMode": "STOMP", "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .is_err());
}

#[test]
fn move_block_planning_fails_closed_without_exact_grid_state() {
    let snapshot = GatewaySnapshot::default();
    let public = plan_gateway_write(
        "device.moveBlock",
        &json!({"row": 1, "fromColumn": 2, "toColumn": 4, "expectedModelId": null, "expectedPresetName": "Unsaved"}),
        Some(&snapshot),
    );
    assert!(public.unwrap_err().starts_with("STATE_UNAVAILABLE:"));
}

#[test]
fn general_toggle_variants_are_exhaustive_and_use_direct_readback_polarity() {
    let settings = [
        "lockScreenAndVolumeKnob",
        "midiOverUsb",
        "ignoreDuplicatePc",
        "stompModeAutoAssign",
        "swapTempoTunerAccess",
        "disableInternetConnectionCheck",
        "dynamicDelayCompensation",
        "presetDimmed",
        "sceneDimmed",
        "stompDimmed",
        "midiClockIn",
        "gigViewStompAccess",
    ];
    for setting in settings {
        let params = json!({"setting": setting, "enabled": true});
        let plan = plan_gateway_write("device.setGeneralToggle", &params, None).unwrap();
        assert!(matches!(
            plan.write,
            PlannedWrite::HidOperation(DeviceOperation::SetGeneralToggle {
                setting: ref actual_setting,
                enabled: true,
            }) if actual_setting == setting
        ));
        assert!(gateway_write_readback_matches(
            "device.setGeneralToggle",
            &params,
            &json!({setting: true}),
        ));
        assert!(!gateway_write_readback_matches(
            "device.setGeneralToggle",
            &params,
            &json!({setting: false}),
        ));
    }
    assert!(plan_gateway_write(
        "device.setGeneralToggle",
        &json!({"setting": "notASetting", "enabled": true}),
        None,
    )
    .is_err());
}

#[test]
fn looper_assignments_are_compared_with_the_decoded_general_settings() {
    let current = vec![0, 1, 2, 3, 4, 5, 6, 7];
    let swapped = vec![1, 0, 2, 3, 4, 5, 6, 7];
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    // No decoded general settings yet: fail closed with a clear error.
    let params = json!({"assignments": swapped, "expectedAssignments": current});
    let error = plan_gateway_write_with_state("device.setLooperStompAssignments", &params, &state)
        .unwrap_err();
    assert!(error.contains("unavailable"), "{error}");
    // Without a state runtime at all the guard cannot be checked either.
    assert!(plan_gateway_write("device.setLooperStompAssignments", &params, None).is_err());

    state
        .ingest(
            profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            &crate::commands::set_looper_stomp_assignments(current.clone()).payload,
        )
        .unwrap();
    let plan =
        plan_gateway_write_with_state("device.setLooperStompAssignments", &params, &state).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetLooperStompAssignments { ref assignments })
            if assignments == &swapped
    ));
    // The caller saw a different permutation than the device holds.
    let stale = json!({"assignments": swapped, "expectedAssignments": [7, 6, 5, 4, 3, 2, 1, 0]});
    let error = plan_gateway_write_with_state("device.setLooperStompAssignments", &stale, &state)
        .unwrap_err();
    assert!(error.contains("stale Looper X assignments"), "{error}");
    // A malformed guard is rejected rather than ignored.
    for bad in [
        json!({"assignments": swapped}),
        json!({"assignments": swapped, "expectedAssignments": "0,1"}),
    ] {
        assert!(
            plan_gateway_write_with_state("device.setLooperStompAssignments", &bad, &state)
                .is_err()
        );
    }
}

#[test]
fn global_bypass_requires_four_rows_per_supplied_family_and_compares_only_those() {
    let params = json!({
        "cab": [true, false, true, false],
        "ir": [false, true, false, true],
    });
    let plan = plan_gateway_write("device.setGlobalBypass", &params, None).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalBypass {
            cab: Some([true, false, true, false]),
            ir: Some([false, true, false, true]),
        })
    ));
    // Either family alone is planned as that family only; null leaves the
    // other family unsent.
    let cab_only = json!({"cab": [true, false, true, false], "ir": null});
    let plan = plan_gateway_write("device.setGlobalBypass", &cab_only, None).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalBypass {
            cab: Some([true, false, true, false]),
            ir: None,
        })
    ));
    let ir_only = json!({"ir": [false, true, false, true]});
    let plan = plan_gateway_write("device.setGlobalBypass", &ir_only, None).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalBypass {
            cab: None,
            ir: Some([false, true, false, true]),
        })
    ));
    for neither in [json!({"cab": null, "ir": null}), json!({})] {
        assert!(plan_gateway_write("device.setGlobalBypass", &neither, None).is_err());
    }
    assert!(gateway_write_readback_matches(
        "device.setGlobalBypass",
        &cab_only,
        &json!({"globalBypassCab": {"row1": true, "row2": false, "row3": true, "row4": false}}),
    ));
    let response = json!({
        "globalBypassCab": {"row1": true, "row2": false, "row3": true, "row4": false},
        "globalBypassIr": {"row1": false, "row2": true, "row3": false, "row4": true},
    });
    assert!(gateway_write_readback_matches(
        "device.setGlobalBypass",
        &params,
        &response,
    ));
    let wrong = json!({
        "globalBypassCab": {"row1": true, "row2": false, "row3": true, "row4": false},
        "globalBypassIr": {"row1": false, "row2": true, "row3": true, "row4": true},
    });
    assert!(!gateway_write_readback_matches(
        "device.setGlobalBypass",
        &params,
        &wrong,
    ));
    for invalid in [
        json!({"cab": [true, false, true], "ir": [false, false, false, false]}),
        json!({"cab": [true, false, true, false], "ir": [false, true, false, 1]}),
    ] {
        assert!(plan_gateway_write("device.setGlobalBypass", &invalid, None).is_err());
    }
}

#[test]
fn scene_management_is_validated_and_planned_in_shared_rust() {
    let snapshot = GatewaySnapshot {
        preset_name: "Scene Test".into(),
        scenes: vec!["A".into(), "B".into(), "C".into(), "D".into()],
        scene_colors: Some(vec!["#000000".into(); 8]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let copy = plan_gateway_write(
        "device.copyScene",
        &json!({"fromScene": 1, "toScene": 3, "swap": true, "expectedPresetName": "Scene Test"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(matches!(
        copy.write,
        PlannedWrite::HidOperation(DeviceOperation::CopyScene {
            from_index: 1,
            to_index: 3,
            swap: true
        })
    ));
    let copied = GatewaySnapshot {
        scenes: vec!["A".into(), "D".into(), "C".into(), "B".into()],
        scene_colors: Some(vec!["#000000".into(); 8]),
        dirty: true,
        ..snapshot.clone()
    };
    assert!(copy.verification.matches(&copied, None));

    let same_scene = plan_gateway_write(
        "device.copyScene",
        &json!({"fromScene": 2, "toScene": 2, "swap": false, "expectedPresetName": "Scene Test"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(matches!(
        same_scene.write,
        PlannedWrite::HidOperation(DeviceOperation::CopyScene {
            from_index: 2,
            to_index: 2,
            swap: false
        })
    ));
    assert!(same_scene.verification.matches(&snapshot, None));

    let restore = plan_gateway_write(
        "device.copyScene",
        &json!({"fromScene": 1, "toScene": 3, "swap": true, "expectedPresetName": "Scene Test"}),
        Some(&copied),
    )
    .unwrap();
    assert!(!snapshot.dirty);
    assert!(restore.verification.matches(&snapshot, None));

    let label = plan_gateway_write(
            "device.setSceneLabel",
            &json!({"scene": 2, "label": "Lead", "expectedLabel": "C", "expectedPresetName": "Scene Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(
        matches!(label.verification, GatewayVerification::SceneLabel { scene: 2, ref label } if label.as_deref() == Some("Lead"))
    );

    let clear = plan_gateway_write(
            "device.setSceneLabel",
            &json!({"scene": 2, "label": null, "expectedLabel": "C", "expectedPresetName": "Scene Test"}),
            Some(&snapshot),
        )
        .unwrap();
    let cleared = GatewaySnapshot {
        scenes: vec![
            "Scene A".into(),
            "Scene B".into(),
            "Scene C".into(),
            "Scene D".into(),
        ],
        ..snapshot.clone()
    };
    assert!(clear.verification.matches(&cleared, None));
    let pushed_clear = GatewaySnapshot {
        scenes: vec!["A".into(), "B".into(), "".into(), "D".into()],
        ..snapshot.clone()
    };
    assert!(clear.verification.matches(&pushed_clear, None));
    assert!(plan_gateway_write(
            "device.setSceneLabel",
            &json!({"scene": 2, "label": "Lead", "expectedLabel": "stale", "expectedPresetName": "Scene Test"}),
            Some(&snapshot),
        )
        .unwrap_err()
        .contains("scene label changed"));

    let color = plan_gateway_write(
            "device.setSceneColor",
            &json!({"scene": 3, "color": 4294902466_u64, "expectedColor": "#000000", "expectedPresetName": "Scene Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(
        color.verification,
        GatewayVerification::SceneColor {
            scene: 3,
            color: 0xffff_02c2
        }
    ));
    assert!(plan_gateway_write(
        "device.copyScene",
        &json!({"fromScene": 8, "toScene": 0}),
        Some(&snapshot)
    )
    .is_err());
}

#[test]
fn parameter_assignments_are_validated_and_planned_in_shared_rust() {
    let snapshot = GatewaySnapshot {
        preset_name: "Assignment Test".into(),
        blocks: vec![fixture_block(1, 4, 42)],
        routes: vec![
            GridRoute {
                row: 2,
                input_id: None,
                output_id: None,
                input: String::new(),
                output: String::new(),
                split_column: Some(4),
                mix_column: Some(7),
                splitter_model_id: Some(10004),
                split_muted: Some(false),
                ..Default::default()
            },
            GridRoute {
                row: 0,
                input_id: None,
                output_id: None,
                input: String::new(),
                output: String::new(),
                split_column: Some(4),
                mix_column: Some(7),
                splitter_model_id: Some(10_004),
                split_muted: Some(false),
                ..Default::default()
            },
        ],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let scene_mode = plan_gateway_write(
        "device.setParameterSceneMode",
        &json!({"expectedModelId": 42,
            "row": 1,
            "column": 4,
            "parameterIndex": 7,
            "enabled": true,
            "expectedPresetName": "Assignment Test"
        }),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        scene_mode.write,
        PlannedWrite::HidOperation(DeviceOperation::SetParameterSceneMode {
            row: 1,
            column: 4,
            parameter_index: 7,
            enabled: true,
        })
    );
    assert!(matches!(
        scene_mode.verification,
        GatewayVerification::ParameterSceneMode {
            row: 1,
            column: 4,
            parameter_index: 7,
            enabled: true
        }
    ));
    let mut observed = observed_parameter(0.5);
    observed.scene_mode = true;
    assert!(scene_mode.verification.matches(&snapshot, Some(&observed)));

    let expression = plan_gateway_write(
        "device.setParameterExpression",
        &json!({"expectedModelId": 42,
            "row": 1,
            "column": 4,
            "parameterIndex": 7,
            "pedal": 2,
            "minimum": 0.9,
            "maximum": 0.1,
            "expectedPresetName": "Assignment Test"
        }),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        expression.write,
        PlannedWrite::HidOperation(DeviceOperation::SetParameterExpression {
            row: 1,
            column: 4,
            parameter_index: 7,
            pedal: 2,
            minimum: 0.9,
            maximum: 0.1,
        })
    );
    assert!(matches!(
        expression.verification,
        GatewayVerification::ParameterExpression {
            row: 1,
            column: 4,
            parameter_index: 7,
            pedal: 2,
            ..
        }
    ));
    observed.expression = Some(2);
    observed.expression_minimum = Some(0.9);
    observed.expression_maximum = Some(0.1);
    assert!(expression.verification.matches(&snapshot, Some(&observed)));

    let lane_scene = plan_gateway_write(
        "device.setLaneControlSceneMode",
        &json!({"row": 0, "control": "inputGate", "parameterIndex": 7,
                "enabled": true, "expectedPresetName": "Assignment Test"}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(lane_scene.verification.parameter_target(), Some((0, 10, 7)));
    assert!(lane_scene.verification.matches(&snapshot, Some(&observed)));
    let lane_expression = plan_gateway_write(
        "device.setLaneControlExpression",
        &json!({"row": 2, "control": "laneOutput", "parameterIndex": 7,
                "pedal": 1, "minimum": 0.75, "maximum": 0.25,
                "expectedPresetName": "Assignment Test"}),
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(
        lane_expression.write,
        PlannedWrite::HidOperation(DeviceOperation::SetLaneControlExpression {
            row: 2,
            control: "laneOutput".into(),
            parameter_index: 7,
            pedal: 1,
            minimum: 0.75,
            maximum: 0.25,
        })
    );
    assert_eq!(
        lane_expression.verification.parameter_target(),
        Some((2, 11, 7))
    );
    observed.expression = Some(1);
    observed.expression_minimum = Some(0.75);
    observed.expression_maximum = Some(0.25);
    assert!(lane_expression
        .verification
        .matches(&snapshot, Some(&observed)));
    let splitter_scene = plan_gateway_write(
            "device.setParameterSceneMode",
            &json!({"expectedModelId": 10004, "row": 0, "column": 8, "parameterIndex": 4, "enabled": true,
                "expectedPresetName": "Assignment Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(
        splitter_scene.write,
        PlannedWrite::HidOperation(DeviceOperation::SetParameterSceneMode {
            row: 0,
            column: 8,
            parameter_index: 4,
            enabled: true
        })
    ));
    let mixer_expression = plan_gateway_write(
        "device.setParameterExpression",
        &json!({"expectedModelId": 11000, "row": 2, "column": 9, "parameterIndex": 3, "pedal": 1,
                "minimum": 0.2, "maximum": 0.9, "expectedPresetName": "Assignment Test"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(matches!(
        mixer_expression.write,
        PlannedWrite::HidOperation(DeviceOperation::SetParameterExpression {
            row: 2,
            column: 9,
            parameter_index: 3,
            pedal: 1,
            ..
        })
    ));
    assert!(plan_gateway_write(
            "device.setParameterSceneMode",
            &json!({"expectedModelId": 10004, "row": 1, "column": 8, "parameterIndex": 4, "enabled": true}),
            Some(&snapshot),
        )
        .is_err());
    assert!(plan_gateway_write(
            "device.setParameterExpression",
            &json!({"expectedModelId": 42, "row": 1, "column": 4, "parameterIndex": 7, "pedal": 3, "minimum": 0.0, "maximum": 1.0}),
            Some(&snapshot),
        )
        .is_err());
    assert!(plan_gateway_write(
            "device.setParameterExpression",
            &json!({"expectedModelId": 42, "row": 1, "column": 4, "parameterIndex": 7, "pedal": 1, "minimum": -0.1, "maximum": 1.0}),
            Some(&snapshot),
        )
        .is_err());
}

#[test]
fn parameter_scene_mode_orders_stale_route_and_readback_failures() {
    let snapshot = GatewaySnapshot {
        preset_name: "Current".into(),
        routes: vec![GridRoute {
            row: 0,
            input_id: None,
            output_id: None,
            input: String::new(),
            output: String::new(),
            split_column: Some(4),
            mix_column: Some(7),
            splitter_model_id: Some(10_000),
            split_muted: Some(false),
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let mut params = json!({"expectedModelId": 10000,
        "row": 0,
        "column": 8,
        "parameterIndex": 3,
        "enabled": true,
        "expectedPresetName": "Stale"
    });
    let error =
        plan_gateway_write("device.setParameterSceneMode", &params, Some(&snapshot)).unwrap_err();
    assert!(error.contains("Preset changed on the Quad Cortex"));

    params["expectedPresetName"] = json!("Current");
    let error =
        plan_gateway_write("device.setParameterSceneMode", &params, Some(&snapshot)).unwrap_err();
    assert!(error.contains("no legacy-container builder"));

    let verification = GatewayVerification::ParameterSceneMode {
        row: 2,
        column: 9,
        parameter_index: 3,
        enabled: true,
    };
    let mut observed = observed_parameter(0.5);
    observed.index = 3;
    observed.scene_mode = false;
    assert!(!verification.matches(&snapshot, Some(&observed)));
    observed.scene_mode = true;
    assert!(verification.matches(&snapshot, Some(&observed)));
    assert!(!verification.matches(&snapshot, None));
}

#[test]
fn parameter_expression_preserves_range_and_uses_push_readback_without_elision() {
    let snapshot = GatewaySnapshot {
        preset_name: "Expression Test".into(),
        routes: vec![GridRoute {
            row: 2,
            input_id: None,
            output_id: None,
            input: String::new(),
            output: String::new(),
            split_column: Some(4),
            mix_column: Some(7),
            splitter_model_id: Some(10004),
            split_muted: Some(false),
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let params = json!({"expectedModelId": 11000,
        "row": 2,
        "column": 9,
        "parameterIndex": 3,
        "pedal": 2,
        "minimum": 0.85,
        "maximum": 0.1,
        "expectedPresetName": "Expression Test"
    });
    let first =
        plan_gateway_write("device.setParameterExpression", &params, Some(&snapshot)).unwrap();
    let repeated =
        plan_gateway_write("device.setParameterExpression", &params, Some(&snapshot)).unwrap();
    assert_eq!(first, repeated, "same-value writes are not elided");
    assert!(matches!(
        first.write,
        PlannedWrite::HidOperation(DeviceOperation::SetParameterExpression {
            row: 2,
            column: 9,
            parameter_index: 3,
            pedal: 2,
            minimum,
            maximum,
        }) if minimum == 0.85 && maximum == 0.1
    ));
    let mut observed = observed_parameter(0.5);
    observed.expression = Some(2);
    observed.expression_minimum = Some(0.85);
    observed.expression_maximum = Some(0.1);
    assert!(first.verification.matches(&snapshot, Some(&observed)));
    assert_eq!(
        gateway_write_preflight_method("device.setParameterExpression"),
        None
    );

    let mut stale = params.clone();
    stale["expectedPresetName"] = json!("Other");
    assert!(plan_gateway_write("device.setParameterExpression", &stale, Some(&snapshot)).is_err());
    for (field, value) in [
        ("pedal", json!(3)),
        ("minimum", json!(-0.1)),
        ("maximum", json!(1.1)),
    ] {
        let mut invalid = params.clone();
        invalid[field] = value;
        assert!(
            plan_gateway_write("device.setParameterExpression", &invalid, Some(&snapshot)).is_err()
        );
    }
}

#[test]
fn lane_control_preview_reuses_committed_write_but_skips_post_write_readback() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let params = json!({"row":2,"control":"inputGate","parameterIndex":3,
            "value":0.25,"expectedValue":0.5,"expectedPresetName":"Live"});
    let preview = plan_gateway_write(
        "device.previewLaneControlParameter",
        &params,
        Some(&snapshot),
    )
    .unwrap();
    let committed =
        plan_gateway_write("device.setLaneControlParameter", &params, Some(&snapshot)).unwrap();
    assert_eq!(preview.write, committed.write);
    assert_eq!(preview.verification, GatewayVerification::None);
    assert!(committed.verification.requires_authoritative_readback());
    assert!(gateway_write_is_realtime(
        "device.previewLaneControlParameter"
    ));
    assert!(!gateway_write_is_realtime("device.setLaneControlParameter"));

    // Preview is a write policy, not a same-value no-op.
    let repeated = plan_gateway_write(
        "device.previewLaneControlParameter",
        &params,
        Some(&snapshot),
    )
    .unwrap();
    assert_eq!(preview, repeated);

    for invalid in [
        json!({"row":2,"control":"inputGate","parameterIndex":3,
                "value":1.1,"expectedValue":0.5,"expectedPresetName":"Live"}),
        json!({"row":2,"control":"inputGate","parameterIndex":3,
                "value":0.25,"expectedValue":0.5,"expectedPresetName":"Stale"}),
        json!({"row":2,"control":"mixer","parameterIndex":3,
                "value":0.25,"expectedValue":0.5,"expectedPresetName":"Live"}),
    ] {
        assert!(plan_gateway_write(
            "device.previewLaneControlParameter",
            &invalid,
            Some(&snapshot),
        )
        .is_err());
    }
}

#[test]
fn lane_control_scene_mode_orders_stale_state_and_readback_failures() {
    let snapshot = GatewaySnapshot {
        preset_name: "Current".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let mut params = json!({
        "row": 3,
        "control": "laneOutput",
        "parameterIndex": 0,
        "enabled": false,
        "expectedPresetName": "Stale"
    });
    let error =
        plan_gateway_write("device.setLaneControlSceneMode", &params, Some(&snapshot)).unwrap_err();
    assert!(error.contains("Preset changed on the Quad Cortex"));

    params["expectedPresetName"] = json!("Current");
    let plan =
        plan_gateway_write("device.setLaneControlSceneMode", &params, Some(&snapshot)).unwrap();
    assert_eq!(plan.verification.parameter_target(), Some((3, 11, 0)));
    let mut observed = observed_parameter(0.5);
    observed.index = 0;
    observed.scene_mode = true;
    assert!(!plan.verification.matches(&snapshot, Some(&observed)));
    observed.scene_mode = false;
    assert!(plan.verification.matches(&snapshot, Some(&observed)));
    assert!(!plan.verification.matches(&snapshot, None));

    params["control"] = json!("input");
    assert!(
        plan_gateway_write("device.setLaneControlSceneMode", &params, Some(&snapshot),).is_err()
    );
}

#[test]
fn splitter_writes_follow_authoritative_container_identity_and_fail_closed() {
    let route = |model_id| GridRoute {
        row: 0,
        input_id: None,
        output_id: None,
        input: String::new(),
        output: String::new(),
        split_column: Some(4),
        mix_column: Some(7),
        splitter_model_id: model_id,
        split_muted: Some(false),
        ..Default::default()
    };
    let snapshot = |model_id| GatewaySnapshot {
        preset_name: "Routing Test".into(),
        routes: vec![route(model_id)],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let mut parameter = json!({
        "row": 0,
        "column": 8,
        "parameterIndex": 3,
        "value": 0.25,
        "expectedValue": 0.5,
        "expectedScene": 0,
        "expectedPresetName": "Routing Test"
    });

    for model_id in 10_000..=10_002 {
        let legacy = snapshot(Some(model_id));
        parameter["expectedModelId"] = json!(model_id);
        for method in ["device.setParameter", "device.previewParameter"] {
            let plan = plan_gateway_write(method, &parameter, Some(&legacy)).unwrap();
            assert!(matches!(
                plan.write,
                PlannedWrite::HidOperation(DeviceOperation::SetRoutingParameter {
                    row: 0,
                    ref node,
                    parameter_index: 3,
                    value: 0.25,
                }) if node == "legacySplitter"
            ));
        }
        for method_params in [
            (
                "device.setParameterSceneMode",
                json!({"row": 0, "column": 8, "parameterIndex": 3, "enabled": true,
                        "expectedModelId":model_id,"expectedPresetName": "Routing Test"}),
            ),
            (
                "device.setParameterExpression",
                json!({"row": 0, "column": 8, "parameterIndex": 3, "pedal": 1,
                        "minimum": 0.0, "maximum": 1.0,
                        "expectedModelId":model_id,"expectedPresetName": "Routing Test"}),
            ),
        ] {
            let error =
                plan_gateway_write(method_params.0, &method_params.1, Some(&legacy)).unwrap_err();
            assert!(error.contains("no legacy-container builder"));
        }
    }

    parameter["expectedModelId"] = json!(10004);
    let combined = plan_gateway_write(
        "device.setParameter",
        &parameter,
        Some(&snapshot(Some(10_004))),
    )
    .unwrap();
    assert!(matches!(
        combined.write,
        PlannedWrite::HidOperation(DeviceOperation::SetRoutingParameter { ref node, .. })
            if node == "splitter"
    ));
    assert!(matches!(
        combined.verification,
        GatewayVerification::Parameter {
            routing_model_id: Some(10_004),
            ..
        }
    ));

    for unavailable in [
        snapshot(None),
        GatewaySnapshot {
            preset_name: "Routing Test".into(),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        },
    ] {
        let error =
            plan_gateway_write("device.setParameter", &parameter, Some(&unavailable)).unwrap_err();
        assert!(error.contains("changed"), "{error}");
    }
}

#[test]
fn stomp_metadata_uses_assignment_count_and_authoritative_readback() {
    let snapshot = GatewaySnapshot {
        preset_name: "Stomp Test".into(),
        blocks: vec![GridBlock {
            id: "gate".into(),
            model_id: Some(1),
            category_id: None,
            name: "Gate".into(),
            kind: "utility".into(),
            category: Some("Utility".into()),
            plugin: None,
            plugin_id: None,
            row: 0,
            column: 1,
            bypassed: Some(false),
            bypass_expression: None,
            color: None,
            glyph: None,
            footswitch: Some(4),
            footswitch_order: Some(0),
            footswitch_assignment_type: Some("primary".into()),
            ..Default::default()
        }],
        footswitch_states: Some(vec![crate::state::FootswitchState {
            index: 4,
            active: Some(true),
            assigned: true,
            color: "#f4f4f4".into(),
            momentary: Some(false),
            label: Some("Gate".into()),
        }]),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let momentary = plan_gateway_write(
            "device.setStompMomentary",
            &json!({"footswitch": 4, "momentary": true, "expectedMomentary": false, "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(
        momentary.write,
        PlannedWrite::HidOperation(DeviceOperation::SetStompMomentary {
            footswitch: 4,
            momentary: true
        })
    ));
    let readback = GatewaySnapshot {
        footswitch_states: Some(vec![crate::state::FootswitchState {
            momentary: Some(true),
            ..snapshot.footswitch_states.as_ref().unwrap()[0].clone()
        }]),
        ..snapshot.clone()
    };
    assert!(momentary.verification.matches(&readback, None));
    assert!(!momentary.verification.matches(
        &GatewaySnapshot {
            footswitch_states: None,
            ..readback.clone()
        },
        None
    ));
    let same_value = plan_gateway_write(
            "device.setStompMomentary",
            &json!({"footswitch": 4, "momentary": false, "expectedMomentary": false, "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(same_value.write, PlannedWrite::HidOperation(_)));
    assert!(plan_gateway_write(
            "device.setStompMomentary",
            &json!({"footswitch": 4, "momentary": true, "expectedMomentary": true, "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .unwrap_err()
        .contains("stale stomp-momentary state"));

    let missing_momentary = GatewaySnapshot {
        footswitch_states: Some(vec![crate::state::FootswitchState {
            momentary: None,
            ..snapshot.footswitch_states.as_ref().unwrap()[0].clone()
        }]),
        ..snapshot.clone()
    };
    assert!(plan_gateway_write(
            "device.setStompMomentary",
            &json!({"footswitch": 4, "momentary": true, "expectedMomentary": false, "expectedPresetName": "Stomp Test"}),
            Some(&missing_momentary),
        )
        .is_err());

    let label = plan_gateway_write(
            "device.setStompLabel",
            &json!({"footswitch": 4, "label": "Solo", "expectedLabel": "Gate", "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(
        label.write,
        PlannedWrite::HidOperation(DeviceOperation::SetStompLabel {
            footswitch: 4,
            ref label,
            single: true
        }) if label == "Solo"
    ));
    let same_value_label = plan_gateway_write(
            "device.setStompLabel",
            &json!({"footswitch": 4, "label": "Gate", "expectedLabel": "Gate", "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(
        same_value_label.write,
        PlannedWrite::HidOperation(DeviceOperation::SetStompLabel { .. })
    ));
    assert!(plan_gateway_write(
            "device.setStompLabel",
            &json!({"footswitch": 4, "label": "Solo", "expectedLabel": "stale", "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .unwrap_err()
        .contains("STOMP label changed"));
    assert!(plan_gateway_write(
            "device.setStompLabel",
            &json!({"footswitch": 4, "label": "12345678901234567", "expectedLabel": "Gate", "expectedPresetName": "Stomp Test"}),
            Some(&snapshot),
        )
        .is_err());

    let unassigned = GatewaySnapshot {
        blocks: Vec::new(),
        ..snapshot
    };
    assert!(plan_gateway_write(
            "device.setStompMomentary",
            &json!({"footswitch": 4, "momentary": true, "expectedMomentary": false, "expectedPresetName": "Stomp Test"}),
            Some(&unassigned),
        )
        .is_err());
    assert!(plan_gateway_write(
            "device.setStompLabel",
            &json!({"footswitch": 4, "label": "Solo", "expectedLabel": "Gate", "expectedPresetName": "Stomp Test"}),
            Some(&unassigned),
        )
        .is_err());
}

#[test]
fn preset_midi_out_is_bounded_and_verified_from_preset_readback() {
    let params = json!({
        "source": 8,
        "messages": [{"type": 1, "channel": 3, "param1": 10, "param2": 5, "param3": 120}],
        "expectedPresetName": "MIDI Test"
    });
    let before = GatewaySnapshot {
        preset_name: "MIDI Test".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write("device.setMidiOut", &params, Some(&before)).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetMidiOut { source: 8, ref messages })
            if messages.len() == 1 && messages[0].channel == 3
    ));
    let after = GatewaySnapshot {
        midi_out: Some(vec![crate::state::MidiOutSource {
            source: 8,
            messages: vec![MidiOutMessage {
                r#type: 1,
                channel: 3,
                param1: 10,
                param2: 5,
                param3: 120,
            }],
            ..Default::default()
        }]),
        ..before.clone()
    };
    assert!(plan.verification.matches(&after, None));
    assert!(plan_gateway_write(
        "device.setMidiOut",
        &json!({"source": 10, "messages": [], "expectedPresetName": "MIDI Test"}),
        Some(&before)
    )
    .is_err());
    assert!(plan_gateway_write(
            "device.setPresetLoadMidiOut",
            &json!({"messages": [{"type": 4, "channel": 1, "param1": 0, "param2": 0, "param3": 0}], "expectedPresetName": "MIDI Test"}),
            Some(&before)
        )
        .is_err());
    let preset_load_before = GatewaySnapshot {
        preset_name: "MIDI Test".into(),
        preset_load_midi_out: Some(Vec::new()),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let preset_load = plan_gateway_write(
            "device.setPresetLoadMidiOut",
            &json!({"messages": [{"type": 1, "channel": 3, "param1": 10, "param2": 5, "param3": 120}], "expectedMessages": [], "expectedPresetName": "MIDI Test"}),
            Some(&preset_load_before),
        )
        .unwrap();
    assert!(!preset_load.verification.matches(&preset_load_before, None));
    assert!(!preset_load.verification.matches(
        &GatewaySnapshot {
            preset_load_midi_out: None,
            ..preset_load_before.clone()
        },
        None,
    ));
    let same_value = plan_gateway_write(
        "device.setPresetLoadMidiOut",
        &json!({"messages": [], "expectedMessages": [], "expectedPresetName": "MIDI Test"}),
        Some(&preset_load_before),
    )
    .unwrap();
    assert!(matches!(same_value.write, PlannedWrite::HidOperation(_)));
    assert!(plan_gateway_write(
            "device.setPresetLoadMidiOut",
            &json!({"messages": [], "expectedMessages": [{"type": 1, "channel": 3, "param1": 10, "param2": 5, "param3": 120}], "expectedPresetName": "MIDI Test"}),
            Some(&preset_load_before),
        )
        .unwrap_err()
        .contains("stale preset-load MIDI Out state"));
}

#[test]
fn midi_out_planner_keeps_empty_rows_in_position_and_verifies_by_position() {
    let row = |kind: u32, param1: u32| MidiOutMessage {
        r#type: kind,
        channel: if kind == 0 { 1 } else { 2 },
        param1,
        param2: 3,
        param3: 4,
    };
    let empty = || row(0, 0);
    // The editor sends twelve rows in order, type 0 placeholders in place.
    let sent = json!([
        {"type": 1, "channel": 2, "param1": 10, "param2": 3, "param3": 4},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 3, "channel": 2, "param1": 30, "param2": 3, "param3": 4},
        {"type": 0, "channel": 0, "param1": 0, "param2": 0, "param3": 0},
        {"type": 2, "channel": 2, "param1": 50, "param2": 3, "param3": 4},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0},
        {"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0}
    ]);
    let before = GatewaySnapshot {
        preset_name: "MIDI Test".into(),
        preset_load_midi_out: Some(Vec::new()),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write(
        "device.setMidiOut",
        &json!({"source": 4, "messages": sent, "expectedPresetName": "MIDI Test"}),
        Some(&before),
    )
    .unwrap();
    let PlannedWrite::HidOperation(DeviceOperation::SetMidiOut {
        source: 4,
        ref messages,
    }) = plan.write
    else {
        panic!("expected a MIDI Out write");
    };
    assert_eq!(
        messages.len(),
        12,
        "the planner neither compacts nor trims the rows"
    );
    let types = messages
        .iter()
        .map(|message| message.r#type)
        .collect::<Vec<_>>();
    assert_eq!(types, vec![1, 0, 3, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(messages[2].param1, 30);
    assert_eq!(messages[4].param1, 50);

    // A unit that keeps positions reads back the list with its interior
    // empty rows and without the trailing ones; that confirms the write.
    let positional = vec![row(1, 10), empty(), row(3, 30), empty(), row(2, 50)];
    let confirmed = GatewaySnapshot {
        midi_out: Some(vec![crate::state::MidiOutSource {
            source: 4,
            messages: positional.clone(),
            ..Default::default()
        }]),
        ..before.clone()
    };
    assert!(plan.verification.matches(&confirmed, None));
    // The same active messages in other positions are a different list.
    for shifted in [
        vec![row(1, 10), row(3, 30), row(2, 50)],
        vec![row(1, 10), empty(), empty(), row(3, 30), row(2, 50)],
        vec![
            empty(),
            row(1, 10),
            empty(),
            row(3, 30),
            empty(),
            row(2, 50),
        ],
    ] {
        let moved = GatewaySnapshot {
            midi_out: Some(vec![crate::state::MidiOutSource {
                source: 4,
                messages: shifted,
                ..Default::default()
            }]),
            ..before.clone()
        };
        assert!(!plan.verification.matches(&moved, None));
    }
    // Clearing the source with twelve placeholders reads back as no group.
    let clear = plan_gateway_write(
            "device.setMidiOut",
            &json!({"source": 4, "messages": vec![json!({"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0}); 12], "expectedPresetName": "MIDI Test"}),
            Some(&before),
        )
        .unwrap();
    assert!(clear.verification.matches(&before, None));
    assert!(!clear.verification.matches(&confirmed, None));

    // The preset-load list uses the same positions, and its stale check
    // compares by position too.
    let loaded = GatewaySnapshot {
        preset_load_midi_out: Some(positional.clone()),
        ..before.clone()
    };
    let load_plan = plan_gateway_write(
        "device.setPresetLoadMidiOut",
        &json!({
            "messages": [{"type": 3, "channel": 2, "param1": 1, "param2": 2, "param3": 3}],
            "expectedMessages": sent,
            "expectedPresetName": "MIDI Test"
        }),
        Some(&loaded),
    )
    .unwrap();
    assert!(matches!(load_plan.write, PlannedWrite::HidOperation(_)));
    let compacted = json!([
        {"type": 1, "channel": 2, "param1": 10, "param2": 3, "param3": 4},
        {"type": 3, "channel": 2, "param1": 30, "param2": 3, "param3": 4},
        {"type": 2, "channel": 2, "param1": 50, "param2": 3, "param3": 4}
    ]);
    assert!(plan_gateway_write(
        "device.setPresetLoadMidiOut",
        &json!({"messages": [], "expectedMessages": compacted, "expectedPresetName": "MIDI Test"}),
        Some(&loaded),
    )
    .unwrap_err()
    .contains("stale preset-load MIDI Out state"));
    // Undefined types and out-of-range values are still refused beside placeholders.
    for bad in [
        json!({"type": 4, "channel": 1, "param1": 0, "param2": 0, "param3": 0}),
        json!({"type": 1, "channel": 0, "param1": 0, "param2": 0, "param3": 0}),
        json!({"type": 0, "channel": 17, "param1": 0, "param2": 0, "param3": 0}),
        json!({"type": 0, "channel": 1, "param1": 128, "param2": 0, "param3": 0}),
    ] {
        assert!(plan_gateway_write(
                "device.setMidiOut",
                &json!({"source": 4, "messages": [{"type": 0, "channel": 1, "param1": 0, "param2": 0, "param3": 0}, bad], "expectedPresetName": "MIDI Test"}),
                Some(&before),
            )
            .is_err());
    }
}

#[test]
fn expression_bypass_is_validated_and_verified_from_block_state() {
    let before = GatewaySnapshot {
        preset_name: "Expression Test".into(),
        blocks: vec![GridBlock {
            id: "amp".into(),
            model_id: Some(42),
            category_id: None,
            name: "Amp".into(),
            kind: "amp".into(),
            category: Some("Amp".into()),
            plugin: None,
            plugin_id: None,
            row: 0,
            column: 2,
            bypassed: Some(false),
            bypass_expression: None,
            color: None,
            glyph: None,
            footswitch: None,
            footswitch_order: None,
            footswitch_assignment_type: None,
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let params = json!({"row":0,"column":2,"pedal":2,"mode":1,"invert":true,
            "delayMs":250,"latchEmulation":true,"expectedModelId":42,"expectedPresetName":"Expression Test"});
    let plan = plan_gateway_write("device.setExpressionBypass", &params, Some(&before)).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetExpressionBypass {
            row: 0,
            column: 2,
            pedal: 2,
            mode: 1,
            invert: true,
            delay_ms: 250,
            latch_emulation: true
        })
    ));
    let mut after = before.clone();
    after.blocks[0].bypass_expression = Some(crate::state::BypassExpression {
        pedal: 2,
        minimum: 0.0,
        maximum: 1.0,
        mode: 1,
        invert: true,
        delay_ms: 250,
        latch_emulation: true,
    });
    assert!(plan.verification.matches(&after, None));
    // Pedal zero clears the assignment. A cleared assignment reads back as
    // no assignment at all, so its verification has to match the snapshot
    // that carries none and reject the one that still carries settings.
    let clear = json!({"row":0,"column":2,"pedal":0,"mode":1,"invert":false,
            "delayMs":0,"latchEmulation":false,"expectedModelId":42,"expectedPresetName":"Expression Test"});

    // The predicate is the same whichever pre-state the clear is planned
    // against, and it is an authoritative readback in both cases. `verified`
    // means the readback shows the state that was asked for; `changed` is what
    // says whether anything moved. Making the
    // already-unassigned case report accepted_unverified instead was tried, which the
    // assistant guide defines as "nothing confirmed it landed" - it tells a
    // caller to go and verify a state that had just been verified, and it
    // drops the snapshot. This pins that it does not come back.
    for pre_state in [&after, &before] {
        let cleared =
            plan_gateway_write("device.setExpressionBypass", &clear, Some(pre_state)).unwrap();
        assert!(cleared.verification.matches(&before, None));
        assert!(!cleared.verification.matches(&after, None));
        assert!(
            cleared.verification.requires_authoritative_readback(),
            "a clear reads its state back whether or not the block carried an assignment",
        );
        assert!(!cleared.verification.is_none());
    }
    let bad = json!({"row":0,"column":2,"pedal":3,"mode":1,"invert":false,
            "delayMs":0,"latchEmulation":false,"expectedModelId":42,"expectedPresetName":"Expression Test"});
    assert!(plan_gateway_write("device.setExpressionBypass", &bad, Some(&before)).is_err());
    for bad in [
        json!({"row":0,"column":2,"pedal":1,"mode":3,"invert":false,
                "delayMs":0,"latchEmulation":false,"expectedModelId":42,"expectedPresetName":"Expression Test"}),
        json!({"row":0,"column":2,"pedal":1,"mode":0,"invert":false,
                "delayMs":5001,"latchEmulation":false,"expectedModelId":42,"expectedPresetName":"Expression Test"}),
        json!({"row":0,"column":2,"pedal":1,"mode":0,"invert":false,
                "delayMs":0,"latchEmulation":false,"expectedModelId":42,"expectedPresetName":"Stale Preset"}),
    ] {
        assert!(plan_gateway_write("device.setExpressionBypass", &bad, Some(&before)).is_err());
    }

    // A same-value request is still a write; Cortex has no equality guard.
    let same = plan_gateway_write("device.setExpressionBypass", &params, Some(&after)).unwrap();
    assert!(matches!(
        same.write,
        PlannedWrite::HidOperation(DeviceOperation::SetExpressionBypass { .. })
    ));
}

#[test]
fn optimistic_tokens_guard_blocks_routes_and_global_state() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        active_scene: 3,
        tempo: 122,
        master_volume: 64,
        blocks: vec![GridBlock {
            id: "block-1".into(),
            model_id: Some(42),
            category_id: None,
            name: "Drive".into(),
            kind: "Drive".into(),
            category: Some("Drive".into()),
            plugin: None,
            plugin_id: None,
            row: 1,
            column: 2,
            bypassed: Some(false),
            bypass_expression: None,
            color: None,
            glyph: None,
            footswitch: Some(5),
            footswitch_order: None,
            footswitch_assignment_type: Some("primary".into()),
            ..Default::default()
        }],
        routes: vec![GridRoute {
            row: 1,
            input_id: Some(1),
            output_id: Some(5),
            input: "In 1".into(),
            output: "Out 1/2".into(),
            split_column: Some(4),
            mix_column: Some(8),
            split_muted: Some(false),
            splitter_model_id: None,
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let current = json!({
        "row": 1, "column": 2, "expectedModelId":42, "desiredBypassed": true,
        "expectedPresetName": "Live", "expectedScene": 3,
        "expectedBypassed": false
    });
    let bypass = plan_gateway_write("device.toggleBypass", &current, Some(&snapshot)).unwrap();
    assert!(matches!(
        bypass.write,
        PlannedWrite::HidCommand(DeviceCommand::SetBypass {
            row: 1,
            column: 2,
            bypassed: true,
        })
    ));
    assert!(gateway_write_is_realtime("device.toggleBypass"));
    assert!(!bypass.verification.matches(&snapshot, None));
    let mut bypassed = snapshot.clone();
    bypassed.blocks[0].bypassed = Some(true);
    assert!(bypass.verification.matches(&bypassed, None));
    assert!(plan_gateway_write(
            "device.toggleBypass",
            &json!({"row": 1, "column": 2, "expectedModelId":42, "desiredBypassed": true, "expectedScene": 2}),
            Some(&snapshot)
        )
        .is_err());
    assert!(assert_expected_state(
            "device.setBlockFootswitch",
            Some(&snapshot),
            &json!({"row": 1, "column": 2, "expectedModelId": 42, "expectedFootswitch": 5, "expectedAssignmentType": "primary"})
        )
        .is_ok());
    let removal = plan_gateway_write(
        "device.removeBlock",
        &json!({"row": 1, "column": 2, "expectedModelId": 42, "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(matches!(
        removal.write,
        PlannedWrite::HidOperation(DeviceOperation::RemoveBlock { row: 1, column: 2 })
    ));
    assert!(!removal.verification.matches(&snapshot, None));
    let removed = GatewaySnapshot {
        blocks: vec![],
        ..snapshot.clone()
    };
    assert!(removal.verification.matches(&removed, None));
    assert!(plan_gateway_write(
        "device.removeBlock",
        &json!({"row": 1, "column": 2, "expectedModelId": 41, "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .is_err());
    let assignment = plan_gateway_write(
            "device.setBlockFootswitch",
            &json!({"row": 1, "column": 2, "footswitch": 6, "assignmentType": "secondary", "expectedModelId": 42, "expectedFootswitch": 5, "expectedAssignmentType": "primary", "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(
        assignment.write,
        PlannedWrite::HidOperation(DeviceOperation::SetFootswitch {
            assignment_type: StompAssignmentType::Secondary,
            previous_footswitch: Some(5),
            previous_assignment_type: Some(StompAssignmentType::Primary),
            ..
        })
    ));
    let mut updated = snapshot.clone();
    updated.blocks[0].footswitch = Some(6);
    updated.blocks[0].footswitch_assignment_type = Some("secondary".into());
    assert!(assignment.verification.matches(&updated, None));
    updated.blocks[0].footswitch_assignment_type = Some("primary".into());
    assert!(!assignment.verification.matches(&updated, None));
    let unchanged = plan_gateway_write(
            "device.setBlockFootswitch",
            &json!({"row": 1, "column": 2, "footswitch": 5, "assignmentType": "primary", "expectedModelId": 42, "expectedFootswitch": 5, "expectedAssignmentType": "primary", "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    let PlannedWrite::HidOperation(unchanged) = unchanged.write else {
        panic!("expected a shared HID operation");
    };
    assert!(unchanged.try_encode().unwrap().is_empty());
    assert!(assert_expected_state(
        "device.setChainSplit",
        Some(&snapshot),
        &json!({"row": 1, "expectedSplitColumn": 3, "expectedMixColumn": 8})
    )
    .is_err());
    assert!(assert_expected_state(
        "device.setMasterVolume",
        Some(&snapshot),
        &json!({"expectedValue": 63})
    )
    .is_err());
    // Regression: a type-mismatched (non-integer, non-null) route-guard
    // field used to fall through `Value::as_i64()` to `None` silently,
    // the same shape as an intentionally absent field, instead of
    // reporting a real error the way expectedModelId already does.
    for field in [
        "expectedInputId",
        "expectedOutputId",
        "expectedSplitColumn",
        "expectedMixColumn",
    ] {
        let error = assert_expected_state(
            "device.setChainInput",
            Some(&snapshot),
            &json!({"row": 1, field: "not-an-integer"}),
        )
        .unwrap_err();
        assert!(
            error.contains(&format!("{field} must be an integer or null")),
            "{field} produced {error:?}"
        );
    }
}

#[test]
fn master_volume_plan_is_strict_sparse_and_authoritatively_reconciled() {
    let before = GatewaySnapshot {
        master_volume: 64,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write(
        "device.setMasterVolume",
        &json!({"value": 57, "expectedValue": 64}),
        Some(&before),
    )
    .unwrap();
    let PlannedWrite::HidCommand(command) = plan.write else {
        panic!("expected one shared HID command");
    };
    let message = command.try_encode().unwrap();
    assert_eq!(message.message_type, profile::MESSAGE_TYPE_MASTER_VOLUME);
    assert_eq!(message.payload, [0x08, 0x01, 0x1d, 0x85, 0xeb, 0x11, 0x3f]);
    assert!(!plan.verification.matches(&before, None));
    let mut echoed = before.clone();
    echoed.master_volume = 57;
    assert!(plan.verification.matches(&echoed, None));

    // An explicit unchanged invocation is not suppressed here.
    // The host guard protects against stale state, not no-ops.
    let unchanged = plan_gateway_write(
        "device.setMasterVolume",
        &json!({"value": 64, "expectedValue": 64}),
        Some(&before),
    )
    .unwrap();
    assert!(matches!(unchanged.write, PlannedWrite::HidCommand(_)));
    assert!(unchanged.verification.matches(&before, None));

    for value in [json!(-1), json!(101), json!(57.0), json!("57"), Value::Null] {
        assert!(plan_gateway_write(
            "device.setMasterVolume",
            &json!({"value": value, "expectedValue": 64}),
            Some(&before),
        )
        .is_err());
    }
    assert!(plan_gateway_write(
        "device.setMasterVolume",
        &json!({"value": 57, "expectedValue": 63}),
        Some(&before),
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.setMasterVolume",
        &json!({"value": 57}),
        Some(&before),
    )
    .is_err());
}

#[test]
fn add_block_requires_empty_cell_and_authoritative_model_readback() {
    let before = GatewaySnapshot {
        preset_name: "Empty Cell".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let params = json!({
        "row": 2,
        "column": 5,
        "modelId": 12_345,
        "expectedModelId": null,
        "expectedPresetName": "Empty Cell"
    });
    let plan = plan_gateway_write("device.addBlock", &params, Some(&before)).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::AddBlock {
            row: 2,
            column: 5,
            model_id: 12_345
        })
    ));
    assert!(!plan.verification.matches(&before, None));
    let after = GatewaySnapshot {
        blocks: vec![GridBlock {
            id: "added".into(),
            model_id: Some(12_345),
            category_id: None,
            name: "Added model".into(),
            kind: "Model".into(),
            category: None,
            plugin: None,
            plugin_id: None,
            row: 2,
            column: 5,
            bypassed: Some(false),
            bypass_expression: None,
            color: None,
            glyph: None,
            footswitch: None,
            footswitch_order: None,
            footswitch_assignment_type: None,
            ..Default::default()
        }],
        ..before.clone()
    };
    assert!(plan.verification.matches(&after, None));
    assert!(plan_gateway_write(
        "device.addBlock",
        &json!({
            "row": 2,
            "column": 5,
            "modelId": 12_346,
            "expectedModelId": null,
            "expectedPresetName": "Empty Cell"
        }),
        Some(&after),
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.addBlock",
        &json!({
            "row": 2,
            "column": 5,
            "modelId": 12_346,
            "expectedModelId": 12_345,
            "expectedPresetName": "Empty Cell"
        }),
        Some(&after),
    )
    .unwrap_err()
    .contains("expectedModelId must be null"));
}

#[test]
fn verified_plugin_add_reports_its_readback_and_a_neutral_note_only_when_bypassed() {
    let before = GatewaySnapshot {
        preset_name: "Empty Cell".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write(
        "device.addBlock",
        &json!({
            "row": 1,
            "column": 3,
            "modelId": 5024,
            "expectedModelId": null,
            "expectedPresetName": "Empty Cell"
        }),
        Some(&before),
    )
    .unwrap();
    let result = |block: Value| {
        json!({
            "accepted": true,
            "verified": true,
            "verification": "authoritative_readback",
            "changed": true,
            "detail": "addBlock sent to the Quad Cortex",
            "snapshot": {
                "presetName": "Empty Cell",
                "blocks": [
                    {"id": "other", "modelId": 9013, "row": 0, "column": 0, "plugin": true, "bypassed": true},
                    block
                ]
            }
        })
    };
    let annotated = |block: Value| {
        let mut value = result(block);
        annotate_plugin_block_readback(&plan.write, &mut value);
        value
    };

    // Locked-looking: plugin, read back bypassed, bypass not requested.
    let locked = annotated(
        json!({"id": "added", "modelId": 5024, "row": 1, "column": 3, "plugin": true, "bypassed": true}),
    );
    assert_eq!(
        locked["pluginBlockReadback"],
        json!({
            "plugin": true,
            "bypassed": true,
            "bypassRequested": false,
            "note": PLUGIN_BLOCK_BYPASSED_NOTE
        })
    );
    assert_eq!(
        locked["detail"],
        format!("addBlock sent to the Quad Cortex. {PLUGIN_BLOCK_BYPASSED_NOTE}")
    );
    assert_eq!(locked["verified"], true, "the outcome itself is unchanged");
    assert_eq!(locked["verification"], "authoritative_readback");
    assert_eq!(locked["changed"], true);
    assert!(!PLUGIN_BLOCK_BYPASSED_NOTE.contains("probably"));
    assert!(PLUGIN_BLOCK_BYPASSED_NOTE.contains("cannot read plugin ownership"));

    // An active plugin block is reported as read back, with no note.
    let active = annotated(
        json!({"id": "added", "modelId": 5024, "row": 1, "column": 3, "plugin": true, "bypassed": false}),
    );
    assert_eq!(
        active["pluginBlockReadback"],
        json!({"plugin": true, "bypassed": false, "bypassRequested": false})
    );
    assert_eq!(active["detail"], "addBlock sent to the Quad Cortex");

    // A readback without a bypass state reports only what it carried.
    let unknown =
        annotated(json!({"id": "added", "modelId": 5024, "row": 1, "column": 3, "plugin": true}));
    assert_eq!(
        unknown["pluginBlockReadback"],
        json!({"plugin": true, "bypassRequested": false})
    );
    assert_eq!(unknown["detail"], "addBlock sent to the Quad Cortex");

    // Built-in models, a different model in the cell, and an empty cell are untouched.
    for block in [
        json!({"id": "added", "modelId": 5024, "row": 1, "column": 3, "plugin": false, "bypassed": true}),
        json!({"id": "added", "modelId": 5024, "row": 1, "column": 3, "bypassed": true}),
        json!({"id": "added", "modelId": 5025, "row": 1, "column": 3, "plugin": true, "bypassed": true}),
        json!({"id": "added", "modelId": 5024, "row": 1, "column": 4, "plugin": true, "bypassed": true}),
    ] {
        assert_eq!(annotated(block.clone()), result(block));
    }

    // An unverified outcome carries no readback to report.
    let plugin_block = json!({"id": "added", "modelId": 5024, "row": 1, "column": 3, "plugin": true, "bypassed": true});
    let mut unverified = json!({
        "accepted": true,
        "verified": false,
        "verification": "accepted_unverified",
        "detail": "addBlock sent to the Quad Cortex"
    });
    let untouched = unverified.clone();
    annotate_plugin_block_readback(&plan.write, &mut unverified);
    assert_eq!(unverified, untouched);

    // Only add_block is annotated, even when another write reads back a bypassed plugin.
    let remove = plan_gateway_write(
        "device.removeBlock",
        &json!({
            "row": 1,
            "column": 3,
            "expectedModelId": 5024,
            "expectedPresetName": "Empty Cell"
        }),
        Some(&GatewaySnapshot {
            blocks: vec![GridBlock {
                id: "added".into(),
                model_id: Some(5024),
                category_id: None,
                name: "Plugin".into(),
                kind: "Model".into(),
                category: None,
                plugin: Some(true),
                plugin_id: Some("13".into()),
                row: 1,
                column: 3,
                bypassed: Some(true),
                bypass_expression: None,
                color: None,
                glyph: None,
                footswitch: None,
                footswitch_order: None,
                footswitch_assignment_type: None,
                ..Default::default()
            }],
            ..before.clone()
        }),
    )
    .unwrap();
    let mut removed = result(plugin_block.clone());
    annotate_plugin_block_readback(&remove.write, &mut removed);
    assert_eq!(removed, result(plugin_block));
}

#[test]
fn verified_add_reports_the_same_plugin_readback_for_hosts_without_a_result_object() {
    let before = GatewaySnapshot {
        preset_name: "Empty Cell".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write(
        "device.addBlock",
        &json!({
            "row": 1,
            "column": 3,
            "modelId": 5024,
            "expectedModelId": null,
            "expectedPresetName": "Empty Cell"
        }),
        Some(&before),
    )
    .unwrap();
    let placed = |plugin: Option<bool>, bypassed: Option<bool>| GatewaySnapshot {
        blocks: vec![GridBlock {
            id: "added".into(),
            model_id: Some(5024),
            category_id: None,
            name: "Plugin".into(),
            kind: "Model".into(),
            category: None,
            plugin,
            plugin_id: plugin.filter(|plugin| *plugin).map(|_| "13".into()),
            row: 1,
            column: 3,
            bypassed,
            bypass_expression: None,
            color: None,
            glyph: None,
            footswitch: None,
            footswitch_order: None,
            footswitch_assignment_type: None,
            ..Default::default()
        }],
        ..before.clone()
    };
    let locked = placed(Some(true), Some(true));
    assert!(plan.verification.matches(&locked, None));
    let readback =
        verified_plugin_block_readback("device.addBlock", &plan.verification, &locked).unwrap();
    assert_eq!(
        serde_json::to_value(&readback).unwrap(),
        json!({
            "plugin": true,
            "bypassed": true,
            "bypassRequested": false,
            "note": PLUGIN_BLOCK_BYPASSED_NOTE
        })
    );
    // The same readback the desktop broker attaches to its result object.
    let mut desktop = json!({
        "verified": true,
        "detail": "addBlock sent to the Quad Cortex",
        "snapshot": serde_json::to_value(&locked).unwrap()
    });
    annotate_plugin_block_readback(&plan.write, &mut desktop);
    assert_eq!(
        desktop["pluginBlockReadback"],
        serde_json::to_value(&readback).unwrap()
    );
    assert_eq!(
        desktop["detail"],
        detail_with_plugin_note(
            "addBlock sent to the Quad Cortex",
            PLUGIN_BLOCK_BYPASSED_NOTE
        )
    );

    let active = verified_plugin_block_readback(
        "device.addBlock",
        &plan.verification,
        &placed(Some(true), Some(false)),
    )
    .unwrap();
    assert_eq!(active.bypassed, Some(false));
    assert_eq!(active.note, None);

    assert!(verified_plugin_block_readback(
        "device.addBlock",
        &plan.verification,
        &placed(Some(false), Some(true))
    )
    .is_none());
    assert!(
        verified_plugin_block_readback("device.replaceBlock", &plan.verification, &locked)
            .is_none()
    );
    assert!(verified_plugin_block_readback(
        "device.addBlock",
        &GatewayVerification::Block {
            row: 1,
            column: 3,
            model_id: None,
            present: true
        },
        &locked
    )
    .is_none());
    assert_eq!(detail_with_plugin_note("", "Note."), "Note.");
    assert_eq!(detail_with_plugin_note("Done.", "Note."), "Done. Note.");
}

#[test]
fn chain_input_is_domain_bounded_stale_guarded_and_still_sends_same_value() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        routes: vec![GridRoute {
            row: 0,
            input_id: Some(1),
            output_id: None,
            input: "In 1".into(),
            output: String::new(),
            split_column: None,
            mix_column: None,
            splitter_model_id: None,
            split_muted: Some(false),
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let same = plan_gateway_write(
        "device.setChainInput",
        &json!({"row":0,"inputId":1,"expectedInputId":1,"expectedPresetName":"Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(matches!(
        same.write,
        PlannedWrite::HidOperation(DeviceOperation::SetChainInput {
            row: 0,
            input_id: 1
        })
    ));
    assert!(same.verification.matches(&snapshot, None));
    assert!(plan_gateway_write(
        "device.setChainInput",
        &json!({"row":0,"inputId":14,"expectedInputId":1,"expectedPresetName":"Live"}),
        Some(&snapshot),
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.setChainInput",
        &json!({"row":0,"inputId":2,"expectedInputId":3,"expectedPresetName":"Live"}),
        Some(&snapshot),
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.setChainInput",
        &json!({"row":1,"inputId":2,"expectedInputId":1,"expectedPresetName":"Live"}),
        Some(&snapshot),
    )
    .is_err());
}

#[test]
fn chain_output_is_qc_bounded_and_authoritatively_verified() {
    let before = GatewaySnapshot {
        preset_name: "Routing".into(),
        routes: vec![GridRoute {
            row: 2,
            input_id: Some(1),
            output_id: Some(5),
            input: "In 1".into(),
            output: "Out 2".into(),
            split_column: None,
            mix_column: None,
            split_muted: Some(false),
            splitter_model_id: None,
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let plan = plan_gateway_write(
        "device.setChainOutput",
        &json!({
            "row": 2,
            "outputId": 20,
            "expectedOutputId": 5,
            "expectedPresetName": "Routing"
        }),
        Some(&before),
    )
    .unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetChainOutput {
            row: 2,
            output_id: 20
        })
    ));
    assert!(!plan.verification.matches(&before, None));
    let after = GatewaySnapshot {
        routes: vec![GridRoute {
            output_id: Some(20),
            output: "USB".into(),
            ..before.routes[0].clone()
        }],
        ..before.clone()
    };
    assert!(plan.verification.matches(&after, None));
    // Every route the device reports must round-trip: a row loaded on Multi
    // Out (19) has to be restorable after it is moved to USB 3 (20).
    let restore_after = GatewaySnapshot {
        routes: vec![GridRoute {
            output_id: Some(20),
            output: "USB 3".into(),
            ..before.routes[0].clone()
        }],
        ..before.clone()
    };
    let restore = plan_gateway_write(
        "device.setChainOutput",
        &json!({
            "row": 2,
            "outputId": 19,
            "expectedOutputId": 20,
            "expectedPresetName": "Routing"
        }),
        Some(&restore_after),
    )
    .unwrap();
    assert!(matches!(
        restore.write,
        PlannedWrite::HidOperation(DeviceOperation::SetChainOutput {
            row: 2,
            output_id: 19
        })
    ));
    // An id the device never reports is still refused, and the message names
    // the device's own route table rather than a stale whitelist.
    let error = plan_gateway_write(
        "device.setChainOutput",
        &json!({
            "row": 2,
            "outputId": 23,
            "expectedOutputId": 5,
            "expectedPresetName": "Routing"
        }),
        Some(&before),
    )
    .unwrap_err();
    assert!(
        error.contains("0 through 22"),
        "error should name the device's route ids: {error}"
    );
    assert!(plan_gateway_write(
        "device.setChainOutput",
        &json!({
            "row": 2,
            "outputId": 20,
            "expectedOutputId": 4,
            "expectedPresetName": "Routing"
        }),
        Some(&before),
    )
    .is_err());
}

#[test]
fn preset_recall_plans_and_verification_are_host_independent() {
    let before = GatewaySnapshot {
        preset_name: "Live".into(),
        setlist_key: "/user/live".into(),
        preset_position: 8,
        position_revision: 14,
        preset_revision: 14,
        full_preset_revision: 14,
        grid_layout_complete: true,
        // Every field observed, the dirty flag included.
        unobserved: Vec::new(),
        ..GatewaySnapshot::default()
    };
    let plan = plan_preset_recall(
        "device.navigateBank",
        &json!({"direction": 1, "expectedPresetName": "Live", "expectedPosition": 8}),
        Some(&before),
    )
    .unwrap();
    assert_eq!(plan.position, 16);
    assert!(!plan.matches(&before));
    assert!(!plan.verification().matches(
        &GatewaySnapshot {
            setlist_key: "/user/live".into(),
            preset_position: 16,
            position_revision: 14,
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        },
        None
    ));
    let after = GatewaySnapshot {
        setlist_key: "/user/live/".into(),
        preset_position: 16,
        position_revision: 15,
        preset_revision: 15,
        full_preset_revision: 15,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    assert!(plan.matches(&after));
    assert!(plan.verification().matches(&after, None));
    assert!(plan.matches_recovered(&GatewaySnapshot {
        setlist_key: "/user/live".into(),
        preset_position: 16,
        position_revision: 1,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    }));

    let cross_setlist = plan_preset_recall(
        "device.recallPreset",
        &json!({
            "setlistKey": "/user/destination",
            "position": 3,
            "expectedPresetName": "Live",
            "expectedPosition": 8,
            "expectedSetlistKey": "/user/live/"
        }),
        Some(&before),
    )
    .unwrap();
    assert_eq!(cross_setlist.setlist_key, "/user/destination");
    assert_eq!(cross_setlist.position, 3);
    assert!(cross_setlist.matches(&GatewaySnapshot {
        setlist_key: "/user/destination/".into(),
        preset_position: 3,
        position_revision: 15,
        preset_revision: 15,
        full_preset_revision: 15,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    }));

    let stale_setlist = plan_preset_recall(
        "device.recallPreset",
        &json!({
            "setlistKey": "/user/destination",
            "position": 3,
            "expectedPresetName": "Live",
            "expectedPosition": 8,
            "expectedSetlistKey": "/user/stale"
        }),
        Some(&before),
    )
    .unwrap_err();
    assert!(stale_setlist.contains("active setlist changed"));
}

#[test]
fn recall_preset_enforces_absolute_domain_dirty_and_stale_guards() {
    let before = GatewaySnapshot {
        preset_name: "Current".into(),
        setlist_key: "/user/current".into(),
        preset_position: 17,
        position_revision: 21,
        preset_revision: 21,
        grid_layout_complete: true,
        unobserved: Vec::new(),
        ..GatewaySnapshot::default()
    };
    let params = |position| {
        json!({
            "setlistKey": "/opt/factory",
            "position": position,
            "expectedPresetName": "Current",
            "expectedPosition": 17,
            "expectedSetlistKey": "/user/current"
        })
    };

    let plan = plan_preset_recall("device.recallPreset", &params(255), Some(&before)).unwrap();
    assert_eq!(plan.position, 255);
    assert_eq!(
        plan.command,
        DeviceCommand::SetlistPosition {
            setlist_key: "/opt/factory".into(),
            position: 255,
            is_factory: true,
        }
    );
    assert_eq!(plan.after_position_revision, 21);
    assert!(
        plan_preset_recall("device.recallPreset", &params(256), Some(&before))
            .unwrap_err()
            .contains("position must be an integer from 0 through 255")
    );

    let dirty = GatewaySnapshot {
        dirty: true,
        ..before.clone()
    };
    assert!(
        plan_preset_recall("device.recallPreset", &params(18), Some(&dirty))
            .unwrap_err()
            .contains("unsaved changes")
    );
    let same = plan_preset_recall("device.recallPreset", &params(17), Some(&before)).unwrap();
    assert_eq!(same.position, 17, "same-position recall is not elided");
    assert!(
        !same.verification().matches(&before, None),
        "a newer position revision is required"
    );

    for stale in [
        json!({"setlistKey":"/user/next","position":18,"expectedPresetName":"Stale","expectedPosition":17,"expectedSetlistKey":"/user/current"}),
        json!({"setlistKey":"/user/next","position":18,"expectedPresetName":"Current","expectedPosition":16,"expectedSetlistKey":"/user/current"}),
        json!({"setlistKey":"/user/next","position":18,"expectedPresetName":"Current","expectedPosition":17,"expectedSetlistKey":"/user/stale"}),
    ] {
        assert!(plan_preset_recall("device.recallPreset", &stale, Some(&before)).is_err());
    }
}

#[test]
fn reload_preset_resends_exact_active_address_and_requires_fresh_clean_state() {
    let before = GatewaySnapshot {
        preset_name: "Live".into(),
        setlist_key: "/user/live".into(),
        preset_position: 8,
        position_revision: 14,
        preset_revision: 14,
        full_preset_revision: 14,
        dirty: true,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let params = json!({"expectedPresetName":"Live", "expectedPosition":8});
    let plan = plan_preset_recall("device.reloadPreset", &params, Some(&before)).unwrap();
    assert_eq!(plan.setlist_key, "/user/live");
    assert_eq!(plan.position, 8);
    assert!(plan.require_clean);
    let message = plan.command.clone().try_encode().unwrap();
    assert_eq!(
        message.payload,
        b"\x08\x01\x1a\x0a/user/live\x20\x08\x28\x00"
    );

    // The desktop sender has no same-value guard: a clean snapshot still
    // produces the same one-frame resend rather than becoming a no-op.
    let clean_before = GatewaySnapshot {
        dirty: false,
        ..before.clone()
    };
    assert_eq!(
        plan_preset_recall("device.reloadPreset", &params, Some(&clean_before))
            .unwrap()
            .command,
        plan.command
    );

    let fresh_clean = GatewaySnapshot {
        dirty: false,
        position_revision: 15,
        full_preset_revision: 15,
        ..before.clone()
    };
    assert!(plan.matches(&fresh_clean));
    assert!(!plan.matches(&GatewaySnapshot {
        position_revision: 14,
        ..fresh_clean.clone()
    }));
    assert!(!plan.matches(&GatewaySnapshot {
        dirty: true,
        ..fresh_clean.clone()
    }));
    assert!(!plan.matches(&GatewaySnapshot {
        preset_position: 9,
        ..fresh_clean.clone()
    }));

    assert!(plan_preset_recall(
        "device.reloadPreset",
        &json!({"expectedPresetName":"Stale", "expectedPosition":8}),
        Some(&before),
    )
    .is_err());
    assert!(plan_preset_recall(
        "device.reloadPreset",
        &json!({"expectedPresetName":"Live", "expectedPosition":256}),
        Some(&before),
    )
    .is_err());

    let factory = GatewaySnapshot {
        setlist_key: "/opt/factory".into(),
        dirty: false,
        ..before
    };
    let factory_plan = plan_preset_recall("device.reloadPreset", &params, Some(&factory)).unwrap();
    assert_eq!(
        factory_plan.command,
        DeviceCommand::SetlistPosition {
            setlist_key: "/opt/factory".into(),
            position: 8,
            is_factory: true,
        }
    );
}

#[test]
fn navigate_bank_preserves_slot_and_fails_closed_before_enqueue() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        setlist_key: "/opt/factory".into(),
        preset_position: 10,
        position_revision: 7,
        preset_revision: 7,
        grid_layout_complete: true,
        unobserved: Vec::new(),
        ..GatewaySnapshot::default()
    };
    let plan = |direction, before: &GatewaySnapshot| {
        plan_preset_recall(
            "device.navigateBank",
            &json!({
                "direction": direction,
                "expectedPresetName": "Live",
                "expectedPosition": before.preset_position
            }),
            Some(before),
        )
    };

    let up = plan(1, &snapshot).unwrap();
    assert_eq!(up.position, 18);
    assert_eq!(
        up.command,
        DeviceCommand::SetlistPosition {
            setlist_key: "/opt/factory".into(),
            position: 18,
            is_factory: true,
        }
    );
    assert_eq!(plan(-1, &snapshot).unwrap().position, 2);

    for invalid in [-2, 0, 2] {
        assert_eq!(
            plan(invalid, &snapshot).unwrap_err(),
            "direction must be -1 or 1"
        );
    }

    let first_bank = GatewaySnapshot {
        preset_position: 3,
        ..snapshot.clone()
    };
    assert_eq!(
        plan(-1, &first_bank).unwrap_err(),
        "Already at the first or last preset bank."
    );
    let last_bank = GatewaySnapshot {
        preset_position: 250,
        ..snapshot.clone()
    };
    assert_eq!(
        plan(1, &last_bank).unwrap_err(),
        "Already at the first or last preset bank."
    );

    let dirty = GatewaySnapshot {
        dirty: true,
        ..snapshot.clone()
    };
    assert!(plan(1, &dirty).unwrap_err().contains("unsaved changes"));
    assert!(plan_preset_recall(
        "device.navigateBank",
        &json!({"direction": 1, "expectedPresetName": "Stale", "expectedPosition": 10}),
        Some(&snapshot),
    )
    .unwrap_err()
    .contains("Preset changed"));
    assert!(plan_preset_recall(
        "device.navigateBank",
        &json!({"direction": 1, "expectedPresetName": "Live", "expectedPosition": 9}),
        Some(&snapshot),
    )
    .unwrap_err()
    .contains("active preset slot changed"));
    assert_eq!(
        plan_preset_recall(
            "device.navigateBank",
            &json!({"direction": 1, "expectedPresetName": "Live", "expectedPosition": 10}),
            None,
        )
        .unwrap_err(),
        "No Quad Cortex preset has been synchronized yet"
    );
}

/// Plans a recall, a bank change and a footswitch press in the snapshot's mode.
fn plan_preset_changes(snapshot: &GatewaySnapshot) -> [Result<(), String>; 3] {
    let mode = snapshot.mode.clone();
    [
        plan_preset_recall(
            "device.recallPreset",
            &json!({"setlistKey": "/user/live", "position": 9,
                "expectedPresetName": "Live", "expectedPosition": 8}),
            Some(snapshot),
        )
        .map(|_| ()),
        plan_preset_recall(
            "device.navigateBank",
            &json!({"direction": 1, "expectedPresetName": "Live", "expectedPosition": 8}),
            Some(snapshot),
        )
        .map(|_| ()),
        plan_gateway_write(
            "device.pressFootswitch",
            &json!({"index": 1, "expectedMode": mode, "expectedPresetName": "Live"}),
            Some(snapshot),
        )
        .map(|_| ()),
    ]
}

#[test]
fn preset_changes_wait_until_the_dirty_flag_has_been_observed() {
    // `dirty: false` is the placeholder until the unit reports the flag.
    let unobserved = GatewaySnapshot {
        preset_name: "Live".into(),
        setlist_key: "/user/live".into(),
        preset_position: 8,
        mode: "PRESET".into(),
        has_preset: true,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    assert!(unobserved.unobserved.contains(&"dirty"));
    assert!(!unobserved.dirty);
    let results = plan_preset_changes(&unobserved);
    assert_eq!(results.len(), 3);
    for result in results {
        let error = result.unwrap_err();
        assert!(
            error.starts_with(UNSAVED_STATE_NOT_YET_OBSERVED_PREFIX),
            "{error}"
        );
        assert!(error.contains("has not yet reported whether"), "{error}");
    }

    // Observed clean: admitted.
    let mut observed = unobserved.clone();
    observed.apply(&serde_json::from_value(json!({"kind": "dirty", "dirty": false})).unwrap());
    assert!(!observed.unobserved.contains(&"dirty"));
    assert_eq!(plan_preset_changes(&observed), [Ok(()), Ok(()), Ok(())]);

    // Observed dirty: refused with the existing messages, unchanged.
    let mut dirty = unobserved.clone();
    dirty.apply(&serde_json::from_value(json!({"kind": "dirty", "dirty": true})).unwrap());
    let [recall, bank, footswitch] = plan_preset_changes(&dirty);
    for error in [recall.unwrap_err(), bank.unwrap_err()] {
        assert_eq!(
            error,
            "The current preset has unsaved changes. Save or revert them before recalling another preset."
        );
    }
    assert_eq!(
        footswitch.unwrap_err(),
        "The current preset has unsaved changes. Save or revert them before pressing a PRESET-mode footswitch."
    );

    // SCENE and STOMP presses never change the preset: admitted while the flag is unobserved.
    for mode in ["SCENE", "STOMP"] {
        let other = GatewaySnapshot {
            mode: mode.into(),
            ..unobserved.clone()
        };
        assert_eq!(plan_preset_changes(&other)[2], Ok(()));
    }

    // reloadPreset admission is unchanged whether the flag is unobserved,
    // clean or dirty.
    for snapshot in [&unobserved, &observed, &dirty] {
        assert!(plan_preset_recall(
            "device.reloadPreset",
            &json!({"expectedPresetName": "Live", "expectedPosition": 8}),
            Some(snapshot),
        )
        .is_ok());
    }
}

#[test]
fn nullable_gateway_assignments_are_preserved_as_explicit_clears() {
    let complete_snapshot = GatewaySnapshot {
        grid_layout_complete: true,
        ..Default::default()
    };
    let footswitch = plan_gateway_write(
            "device.setBlockFootswitch",
            &json!({"row": 0, "column": 1, "footswitch": null, "assignmentType": "secondary", "expectedFootswitch": null, "expectedAssignmentType": null, "expectedModelId": null, "expectedPresetName": "Unsaved"}),
            Some(&complete_snapshot),
        )
        .unwrap();
    assert_eq!(
        footswitch.write,
        PlannedWrite::HidOperation(DeviceOperation::SetFootswitch {
            row: 0,
            column: 1,
            footswitch: None,
            assignment_type: StompAssignmentType::Secondary,
            previous_footswitch: None,
            previous_assignment_type: None,
        })
    );
    let split = plan_gateway_write(
            "device.setChainSplit",
            &json!({"row": 2, "splitColumn": null, "mixColumn": null, "expectedSplitColumn": null, "expectedMixColumn": null, "expectedPresetName": "Unsaved"}),
            Some(&GatewaySnapshot {
                routes: vec![GridRoute {
                    row: 2,
                    input_id: None,
                    output_id: None,
                    input: String::new(),
                    output: String::new(),
                    split_column: None,
                    mix_column: None,
            split_muted: Some(false),
                    splitter_model_id: None,
                    ..Default::default()
                }],
                grid_layout_complete: true,
        ..GatewaySnapshot::default()
            }),
        )
        .unwrap();
    assert_eq!(
        split.write,
        PlannedWrite::HidOperation(DeviceOperation::SetChainSplit {
            row: 2,
            split_column: None,
            mix_column: None,
        })
    );

    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        active_scene: 3,
        routes: vec![GridRoute {
            row: 2,
            input_id: Some(1),
            output_id: Some(1),
            input: "In 1".into(),
            output: "Out 1/2".into(),
            split_column: Some(2),
            mix_column: Some(7),
            split_muted: Some(false),
            splitter_model_id: None,
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let mute = plan_gateway_write(
            "device.setSplitMute",
            &json!({"row": 2, "muted": true, "expectedMuted": false, "expectedScene": 3, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    assert_eq!(
        mute.write,
        PlannedWrite::HidOperation(DeviceOperation::SetSplitMute {
            row: 2,
            muted: true,
        })
    );
    assert!(!mute.verification.matches(&snapshot, None));
    let muted = GatewaySnapshot {
        routes: vec![GridRoute {
            split_muted: Some(true),
            ..snapshot.routes[0].clone()
        }],
        ..snapshot.clone()
    };
    assert!(mute.verification.matches(&muted, None));
    let unavailable = GatewaySnapshot {
        routes: vec![GridRoute {
            split_muted: None,
            ..snapshot.routes[0].clone()
        }],
        ..snapshot.clone()
    };
    assert!(!mute.verification.matches(&unavailable, None));
    let unmute = plan_gateway_write(
        "device.setSplitMute",
        &json!({"row": 2, "muted": false, "expectedMuted": true, "expectedScene": 3, "expectedPresetName": "Live"}),
        Some(&muted),
    ).unwrap();
    assert!(!unmute.verification.matches(&unavailable, None));
    let unavailable_error = plan_gateway_write(
        "device.setSplitMute",
        &json!({"row": 2, "muted": true, "expectedMuted": false, "expectedScene": 3, "expectedPresetName": "Live"}),
        Some(&unavailable),
    ).unwrap_err();
    assert!(
        unavailable_error.starts_with("STATE_UNAVAILABLE:"),
        "{unavailable_error}"
    );
    assert!(!mute.verification.matches(
        &GatewaySnapshot {
            active_scene: 2,
            ..muted.clone()
        },
        None,
    ));
    assert!(plan_gateway_write(
            "device.setSplitMute",
            &json!({"row": 2, "muted": true, "expectedMuted": false, "expectedScene": 2, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .is_err());
    assert!(plan_gateway_write(
        "device.setSplitMute",
        &json!({"row": 1, "muted": true}),
        None,
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.setChainSplit",
        &json!({"row": 0, "splitColumn": 5, "mixColumn": 3}),
        None,
    )
    .is_err());
    let mixer_only = plan_gateway_write(
            "device.setChainSplit",
            &json!({"row": 0, "splitColumn": null, "mixColumn": 7, "expectedSplitColumn": null, "expectedMixColumn": null, "expectedPresetName": "Live"}),
            Some(&GatewaySnapshot {
                routes: vec![GridRoute {
                    row: 0,
                    input_id: None,
                    output_id: None,
                    input: String::new(),
                    output: String::new(),
                    split_column: None,
                    mix_column: None,
                    splitter_model_id: None,
            split_muted: Some(false),
                    ..Default::default()
                }],
                preset_name: "Live".into(),
                grid_layout_complete: true,
        ..GatewaySnapshot::default()
            }),
        )
        .unwrap();
    assert_eq!(
        mixer_only.write,
        PlannedWrite::HidOperation(DeviceOperation::SetChainSplit {
            row: 0,
            split_column: None,
            mix_column: Some(7),
        })
    );
    assert!(plan_gateway_write(
        "device.setChainSplit",
        &json!({"row": 0, "splitColumn": -1, "mixColumn": null}),
        None,
    )
    .is_err());
}

#[test]
fn planned_write_readback_predicates_are_host_independent() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        active_scene: 2,
        tempo: 126,
        blocks: vec![GridBlock {
            id: "block-1".into(),
            model_id: Some(42),
            category_id: None,
            name: "Drive".into(),
            kind: "Drive".into(),
            category: Some("Drive".into()),
            plugin: None,
            plugin_id: None,
            row: 1,
            column: 4,
            bypassed: Some(true),
            bypass_expression: None,
            color: None,
            glyph: None,
            footswitch: Some(3),
            footswitch_order: None,
            footswitch_assignment_type: None,
            ..Default::default()
        }],
        routes: vec![GridRoute {
            row: 0,
            input_id: None,
            output_id: None,
            input: String::new(),
            output: String::new(),
            split_column: Some(4),
            mix_column: Some(7),
            splitter_model_id: Some(10_004),
            split_muted: Some(false),
            ..Default::default()
        }],
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let scene = plan_gateway_write(
        "device.selectScene",
        &json!({"scene": 2, "expectedPresetName": "Live"}),
        Some(&snapshot),
    )
    .unwrap();
    assert!(scene.verification.matches(&snapshot, None));

    let bypass = plan_gateway_write(
            "device.toggleBypass",
            &json!({"expectedModelId": 42, "row": 1, "column": 4, "desiredBypassed": true, "expectedBypassed": true, "expectedScene": 2, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(bypass.verification.matches(&snapshot, None));

    let parameter = plan_gateway_write(
            "device.setParameter",
            &json!({"expectedModelId": 42, "row": 1, "column": 4, "parameterIndex": 7, "value": 0.75, "expectedValue": 0.5, "expectedScene": 2, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    let close = observed_parameter(0.7504);
    let far = observed_parameter(0.76);
    assert!(parameter.verification.matches(&snapshot, Some(&close)));
    assert!(!parameter.verification.matches(&snapshot, Some(&far)));

    let splitter = plan_gateway_write(
            "device.setParameter",
            &json!({"expectedModelId": 10004, "row": 0, "column": 8, "parameterIndex": 5, "value": 0.25, "expectedValue": 0.5, "expectedScene": 2, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(splitter.write,
            PlannedWrite::HidOperation(DeviceOperation::SetRoutingParameter {
                row: 0, ref node, parameter_index: 5, value
            }) if node == "splitter" && value == 0.25));
    let mut changed_identity = snapshot.clone();
    changed_identity.routes[0].splitter_model_id = Some(10_001);
    assert!(!splitter
        .verification
        .matches(&changed_identity, Some(&observed_parameter(0.25))));
    assert!(plan_gateway_write(
            "device.setParameter",
            &json!({"expectedModelId": 11000, "row": 1, "column": 9, "parameterIndex": 1, "value": 0.5}),
            Some(&snapshot),
        )
        .is_err());

    let lane = plan_gateway_write(
            "device.setLaneControlParameter",
            &json!({"row": 3, "control": "laneOutput", "parameterIndex": 0, "value": 0.64, "expectedValue": 0.5, "expectedPresetName": "Live"}),
            Some(&snapshot),
        )
        .unwrap();
    assert!(matches!(lane.write,
            PlannedWrite::HidOperation(DeviceOperation::SetLaneControlParameter {
                row: 3, ref control, parameter_index: 0, value
            }) if control == "laneOutput" && value == 0.64));
    assert_eq!(lane.verification.parameter_target(), Some((3, 11, 0)));
    assert!(lane.verification.requires_authoritative_readback());
    assert!(plan_gateway_write(
        "device.setLaneControlParameter",
        &json!({"row": 0, "control": "models", "parameterIndex": 0, "value": 0.5}),
        Some(&snapshot),
    )
    .is_err());
}

#[test]
fn history_write_is_single_frame_unverified_and_unpaced() {
    let plan = plan_gateway_write("device.undo", &Value::Null, None).unwrap();
    assert_eq!(plan.detail, "Device undo sent");
    assert_eq!(plan.verification, GatewayVerification::None);
    assert_eq!(plan.write.inter_message_interval_ms(), 0);
    let PlannedWrite::HidOperation(operation) = plan.write else {
        panic!("undo must use the HID operation lane");
    };
    assert_eq!(operation, DeviceOperation::Undo);
    let frames = operation.try_encode().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_UNDO_REDO);
    assert_eq!(frames[0].payload, [0x08, 0x01, 0x28, 0x01]);

    let policy = gateway_write_verification_policy("device.undo");
    assert_eq!(policy.preflight_method, None);
    assert_eq!(policy.readback_method, None);
}

#[test]
fn persistent_preset_workflows_share_validation_and_stages() {
    let mut library = PresetLibrary::default();
    library.ingest(PresetFolderListing {
        order: None,
        key: "/media/p4/Presets/Live".into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![
            PresetFileListing {
                position: 8,
                name: "Current".into(),
                instrument: 2,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/Current.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 9,
                name: "Source".into(),
                instrument: 3,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/Source.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 255,
                name: "Unsaved".into(),
                instrument: 0,
                instrument_present: false,
                product_key: None,
                ..Default::default()
            },
        ],
    });
    let snapshot = GatewaySnapshot {
        preset_name: "Current".into(),
        setlist_key: "/media/p4/Presets/Live".into(),
        preset_position: 8,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let save = plan_preset_mutation(
        "device.savePresetAs",
        &json!({
            "setlistKey": "/media/p4/Presets/Live", "position": 10,
            "name": "Saved", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmOverwrite": false
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap();
    assert_eq!(save.stages.len(), 1);
    assert_eq!(save.stages[0].settle_ms, 3_000);
    assert_eq!(save.instrument, 2);
    assert_eq!(
        save.catalog_policy,
        CatalogVerificationPolicy::persistent_file_mutation()
    );

    let rename = plan_preset_mutation(
        "device.renameCurrentPreset",
        &json!({
            "name": "Renamed", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmRename": true
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap();
    // Cortex Control's rename is one File UPDATE addressed by the opaque
    // product key: no Save, no recall, no cleanup stage.
    assert_eq!(rename.stages.len(), 1);
    assert!(rename.cleanup_stages.is_empty());
    assert_eq!(rename.stages[0].settle_ms, 0);
    assert!(matches!(
        rename.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::RenamePreset {
            ref setlist_key,
            ref product_key,
            ref name,
            instrument,
        }) if setlist_key == "/media/p4/Presets/Live"
            && product_key == "/media/p4/Presets/Live/Current.pb"
            && name == "Renamed"
            && instrument == 2
    ));
    assert!(matches!(
        rename.stages[0].verification,
        GatewayVerification::None
    ));
    assert_eq!(rename.saved_presets[0].name, "Renamed");
    assert_eq!(rename.saved_presets[0].position, 8);
    // Verification wants the exact name the UPDATE carried: no suffix
    // tolerance is assumed for a direct frame.
    assert!(!rename.saved_presets[0].allow_collision_suffix);
    assert_eq!(
        rename.catalog_policy,
        CatalogVerificationPolicy::persistent_file_mutation()
    );

    // The unit does not de-duplicate a rename (2026-10-07, CorOS 4.1.0): a
    // target name another slot already holds is refused before any frame.
    let colliding = plan_preset_mutation(
        "device.renameCurrentPreset",
        &json!({
            "name": "Source", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmRename": true
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap_err();
    assert!(
        colliding.contains("does not de-duplicate a rename"),
        "{colliding}"
    );

    // Copy sends Cortex Control's single File COPY frame. It names the
    // source by its opaque product key, never recalls it, never saves, and
    // never touches the active preset, so it needs no snapshot and plans
    // no cleanup stage.
    let copy = plan_preset_mutation(
        "device.copyPreset",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
            "sourceName": "Source", "destinationSetlistKey": "/media/p4/Presets/Live",
            "destinationPosition": 10, "confirmOverwrite": true
        }),
        None,
        &library,
    )
    .unwrap();
    assert_eq!(copy.stages.len(), 1);
    assert!(copy.cleanup_stages.is_empty());
    assert!(matches!(
        copy.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::CopyPreset {
            ref source_setlist_key,
            ref source_product_key,
            source_is_factory: false,
            ref destination_setlist_key,
            destination_position: 10,
        }) if source_setlist_key == "/media/p4/Presets/Live"
            && source_product_key == "/media/p4/Presets/Live/Source.pb"
            && destination_setlist_key == "/media/p4/Presets/Live"
    ));
    assert!(matches!(
        copy.stages[0].verification,
        GatewayVerification::None
    ));
    assert_eq!(copy.stages[0].settle_ms, 0);
    assert_eq!(copy.saved_name, "Source");
    assert_eq!(copy.instrument, 3);
    assert_eq!(copy.saved_presets[0].position, 10);
    assert_eq!(copy.saved_presets[0].name, "Source");
    assert!(copy.saved_presets[0].allow_collision_suffix);
    assert_eq!(
        copy.catalog_policy,
        CatalogVerificationPolicy::persistent_file_mutation()
    );

    let moved = plan_preset_mutation(
        "device.movePreset",
        &json!({
            "setlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
            "sourceName": "Source", "destinationPosition": 8
        }),
        None,
        &library,
    )
    .unwrap();
    assert_eq!(moved.stages.len(), 1);
    assert!(matches!(
        moved.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::SwapPreset {
            ref setlist_key,
            ref product_key,
            position: 8,
        }) if setlist_key == "/media/p4/Presets/Live"
            && product_key == "/media/p4/Presets/Live/Source.pb"
    ));
    assert_eq!(moved.stages[0].verification, GatewayVerification::None);
    assert_eq!(moved.saved_presets.len(), 2);
    assert_eq!(moved.saved_presets[0].position, 8);
    assert_eq!(moved.saved_presets[0].name, "Source");
    assert!(!moved.saved_presets[0].allow_collision_suffix);
    assert_eq!(moved.saved_presets[1].position, 9);
    assert_eq!(moved.saved_presets[1].name, "Current");
    assert!(!moved.saved_presets[1].allow_collision_suffix);
    assert_eq!(
        moved.catalog_policy,
        CatalogVerificationPolicy::persistent_file_mutation()
    );

    let moved_to_keyless_placeholder = plan_preset_mutation(
        "device.movePreset",
        &json!({
            "setlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
            "sourceName": "Source", "destinationPosition": 255
        }),
        None,
        &library,
    )
    .unwrap();
    assert!(matches!(
        moved_to_keyless_placeholder.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::MovePreset {
            ref setlist_key,
            ref product_key,
            position: 255,
        }) if setlist_key == "/media/p4/Presets/Live"
            && product_key == "/media/p4/Presets/Live/Source.pb"
    ));
    assert_eq!(moved_to_keyless_placeholder.saved_presets.len(), 1);
    assert_eq!(moved_to_keyless_placeholder.saved_presets[0].position, 255);
    assert_eq!(moved_to_keyless_placeholder.saved_presets[0].name, "Source");
    for invalid in [
        json!({"setlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
                "sourceName": "Stale", "destinationPosition": 8}),
        json!({"setlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
                "sourceName": "Source", "destinationPosition": 9}),
        json!({"setlistKey": "/opt/neuraldsp/Factory Library", "sourcePosition": 9,
                "sourceName": "Source", "destinationPosition": 8}),
    ] {
        assert!(plan_preset_mutation("device.movePreset", &invalid, None, &library).is_err());
    }

    // The active preset is irrelevant to copy: copying the active slot,
    // overwriting the active slot, or copying while the active preset has
    // unsaved edits all plan the same single COPY stage. None of them
    // plans a recall, a Save or a restore.
    let dirty = GatewaySnapshot {
        dirty: true,
        ..snapshot.clone()
    };
    for (snapshot_during_copy, source_position, destination_position) in [
        (Some(&snapshot), 9_u32, 10_u32),
        (Some(&dirty), 9, 10),
        (Some(&snapshot), 9, 8),
        (Some(&snapshot), 8, 10),
        (None, 9, 10),
    ] {
        let plan = plan_preset_mutation(
            "device.copyPreset",
            &json!({
                "sourceSetlistKey": "/media/p4/Presets/Live",
                "sourcePosition": source_position,
                "sourceName": if source_position == 9 { "Source" } else { "Current" },
                "destinationSetlistKey": "/media/p4/Presets/Live",
                "destinationPosition": destination_position,
                "confirmOverwrite": true
            }),
            snapshot_during_copy,
            &library,
        )
        .unwrap();
        assert_eq!(plan.stages.len(), 1);
        assert!(plan.cleanup_stages.is_empty());
        assert!(matches!(
            plan.stages[0].write,
            PlannedWrite::HidOperation(DeviceOperation::CopyPreset { .. })
        ));
        assert!(plan
            .stages
            .iter()
            .chain(plan.cleanup_stages.iter())
            .all(|stage| !matches!(
                stage.write,
                PlannedWrite::HidCommand(DeviceCommand::SetlistPosition { .. })
                    | PlannedWrite::HidOperation(DeviceOperation::SavePreset { .. })
            )));
    }
    assert!(plan_preset_mutation(
        "device.renameCurrentPreset",
        &json!({
            "name": "Renamed", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmRename": true
        }),
        Some(&dirty),
        &library,
    )
    .unwrap_err()
    .contains("unsaved changes"));

    let duplicate = plan_preset_mutation(
        "device.duplicateSetlist",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live",
            "expectedSourceName": "Live"
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap();
    assert_eq!(duplicate.stages.len(), 1);
    assert_eq!(duplicate.stages[0].settle_ms, 0);
    assert!(matches!(
        duplicate.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::DuplicateSetlist {
            ref source_setlist_key
        }) if source_setlist_key == "/media/p4/Presets/Live"
    ));
    assert!(duplicate.saved_presets.is_empty());
    let folder_copy = duplicate.folder_copy.unwrap();
    assert_eq!(folder_copy.operation, "copy");
    assert_eq!(folder_copy.source_name, "Live");
    assert_eq!(folder_copy.before_folder_keys, ["/media/p4/Presets/Live"]);
    assert_eq!(folder_copy.expected_presets.len(), 2);
    assert_eq!(folder_copy.expected_presets[0].position, 8);
    assert_eq!(folder_copy.expected_presets[0].name, "Current");
    assert_eq!(folder_copy.expected_presets[1].position, 9);
    assert_eq!(folder_copy.expected_presets[1].name, "Source");
    assert_eq!(folder_copy.catalog_policy.timeout_ms, 94_000);
    assert_eq!(folder_copy.catalog_policy.initial_retry_ms, 2_000);
    assert_eq!(folder_copy.catalog_policy.maximum_retry_ms, 30_000);
    let complete_copy = library
        .list("/media/p4/Presets/Live", &snapshot)
        .expect("source catalog");
    assert!(duplicate_setlist_contents_match(
        &folder_copy.expected_presets,
        &complete_copy
    ));
    let mut folder_only = complete_copy.clone();
    folder_only.presets[8].product_key = None;
    assert!(!duplicate_setlist_contents_match(
        &folder_copy.expected_presets,
        &folder_only
    ));
    let mut wrong_name = complete_copy;
    wrong_name.presets[8].name = "Current_1".into();
    assert!(!duplicate_setlist_contents_match(
        &folder_copy.expected_presets,
        &wrong_name
    ));
    let mismatch = duplicate_setlist_contents_mismatch(&folder_copy.expected_presets, &wrong_name)
        .expect("wrong name must explain the mismatch");
    assert!(mismatch.contains("expected 2 occupied preset(s), observed 2"));
    assert!(mismatch.contains("name mismatches [8]"));
    assert!(!mismatch.contains("Current"));
    let expected_many = (0..12)
        .map(|position| PresetMutationRecord {
            setlist_key: "/media/p4/Presets/Live".into(),
            position,
            name: format!("Private preset {position}"),
            instrument: 1,
            allow_collision_suffix: false,
        })
        .collect::<Vec<_>>();
    let mut empty_copy = wrong_name;
    empty_copy
        .presets
        .iter_mut()
        .for_each(|preset| preset.product_key = None);
    let bounded = duplicate_setlist_contents_mismatch(&expected_many, &empty_copy)
        .expect("empty copy must explain the mismatch");
    assert!(bounded.contains("missing positions [0,1,2,3,4,5,6,7,+4 more]"));
    assert!(!bounded.contains("Private preset"));
    assert!(plan_preset_mutation(
        "device.duplicateSetlist",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live",
            "expectedSourceName": "Stale name"
        }),
        Some(&snapshot),
        &library,
    )
    .is_err());

    library.ingest(PresetFolderListing {
        order: None,
        key: "/media/p4/Presets/Live".into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![
            PresetFileListing {
                position: 9,
                name: "Source".into(),
                instrument: 3,
                instrument_present: true,
                product_key: Some("source-key".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 10,
                name: "Source".into(),
                instrument: 3,
                instrument_present: true,
                product_key: Some("stale-destination-key".into()),
                ..Default::default()
            },
        ],
    });
    assert!(plan_preset_mutation(
        "device.copyPreset",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
            "sourceName": "Source", "destinationSetlistKey": "/media/p4/Presets/Live",
            "destinationPosition": 10, "confirmOverwrite": true
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap_err()
    .contains("cannot prove that overwrite changed"));

    let mut missing_key_library = PresetLibrary::default();
    missing_key_library.ingest(PresetFolderListing {
        order: None,
        key: "/media/p4/Presets/Live".into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![PresetFileListing {
            position: 9,
            name: "Source".into(),
            instrument: 3,
            instrument_present: true,
            product_key: None,
            ..Default::default()
        }],
    });
    let keyless_copy = plan_preset_mutation(
        "device.copyPreset",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
            "sourceName": "Source", "destinationSetlistKey": "/media/p4/Presets/Other",
            "destinationPosition": 10, "confirmOverwrite": true
        }),
        None,
        &missing_key_library,
    )
    .unwrap_err();
    assert!(
        keyless_copy.contains("no authoritative device file key"),
        "{keyless_copy:?}: COPY names its source by key, so no frame may be planned without one"
    );

    let mut missing_rename_key_library = PresetLibrary::default();
    missing_rename_key_library.ingest(PresetFolderListing {
        order: None,
        key: "/media/p4/Presets/Live".into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![PresetFileListing {
            position: 8,
            name: "Current".into(),
            instrument: 2,
            instrument_present: true,
            product_key: None,
            ..Default::default()
        }],
    });
    let keyless_rename = plan_preset_mutation(
        "device.renameCurrentPreset",
        &json!({
            "name": "Renamed", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmRename": true
        }),
        Some(&snapshot),
        &missing_rename_key_library,
    )
    .unwrap_err();
    assert!(
        keyless_rename.contains("no authoritative device file key"),
        "{keyless_rename:?}: UPDATE addresses the preset by key, so no frame may be planned without one"
    );
}

/// The planner's single stage, encoded exactly as the host would send it
/// before the final-send boundary stamps `request_id`.
fn planned_frame_hex(stage: &PresetMutationStage) -> String {
    let PlannedWrite::HidOperation(operation) = &stage.write else {
        panic!("expected a File operation, got {:?}", stage.write);
    };
    let frames = operation.clone().try_encode().unwrap();
    assert_eq!(frames.len(), 1, "one File frame, as Cortex Control sends");
    assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_FILE);
    frames[0]
        .payload
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn planned_rename_and_copy_encode_the_cortex_control_file_frames() {
    // Expected bytes are literals, built by hand from the proto field
    // numbers and the Cortex Control builder shapes (File UPDATE for
    // EditPresetTask, File COPY for copyUserFiles), never produced by the
    // encoder under test. `request_id` (field 2) is absent here: it is
    // stamped at the send boundary.
    const RENAME_BUILDER_HEX: &str = "0801180022420a162f6d656469612f70342f507265736574732f4c69766520003a260a196f70617175652d6465766963652d70726f647563742d6b65791a0752656e616d65647802";
    const COPY_BUILDER_HEX: &str = "08051800223f0a1c2f6d656469612f70342f507265736574732f4d792050726573657473200028003a1b0a196f70617175652d6465766963652d70726f647563742d6b65792a1f0a192f6d656469612f70342f507265736574732f536372617463683a02101f";
    let listing = |key: &str, files: Vec<PresetFileListing>| PresetFolderListing {
        order: None,
        key: key.into(),
        name: key.rsplit('/').next().unwrap().into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files,
    };
    let file = |position: u32, name: &str, instrument: i32| PresetFileListing {
        position,
        name: name.into(),
        instrument,
        instrument_present: true,
        product_key: Some("opaque-device-product-key".into()),
        ..Default::default()
    };
    let mut library = PresetLibrary::default();
    library.ingest(listing(
        "/media/p4/Presets/Live",
        vec![file(1, "Current", 2)],
    ));
    library.ingest(listing(
        "/media/p4/Presets/My Presets",
        vec![file(3, "Source", 1)],
    ));
    library.ingest(listing("/media/p4/Presets/Scratch", Vec::new()));
    let snapshot = GatewaySnapshot {
        preset_name: "Current".into(),
        setlist_key: "/media/p4/Presets/Live".into(),
        preset_position: 1,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };

    let rename = plan_preset_mutation(
        "device.renameCurrentPreset",
        &json!({
            "name": "Renamed", "expectedPresetName": "Current",
            "expectedPosition": 1, "confirmRename": true
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap();
    assert_eq!(rename.stages.len(), 1);
    assert!(rename.cleanup_stages.is_empty());
    assert_eq!(planned_frame_hex(&rename.stages[0]), RENAME_BUILDER_HEX);

    // The copy has no active-preset operands at all: it plans without a
    // snapshot, and never emits a recall or a Save.
    let copy = plan_preset_mutation(
        "device.copyPreset",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/My Presets", "sourcePosition": 3,
            "sourceName": "Source",
            "destinationSetlistKey": "/media/p4/Presets/Scratch",
            "destinationPosition": 31, "confirmOverwrite": true
        }),
        None,
        &library,
    )
    .unwrap();
    assert_eq!(copy.stages.len(), 1);
    assert!(copy.cleanup_stages.is_empty());
    assert_eq!(planned_frame_hex(&copy.stages[0]), COPY_BUILDER_HEX);
    for plan in [&rename, &copy] {
        assert!(plan.stages.iter().chain(&plan.cleanup_stages).all(|stage| {
            !matches!(
                stage.write,
                PlannedWrite::HidCommand(DeviceCommand::SetlistPosition { .. })
                    | PlannedWrite::HidOperation(DeviceOperation::SavePreset { .. })
            )
        }));
    }
}

#[test]
fn save_preset_as_requires_overwrite_confirmation_for_an_empty_named_occupied_slot() {
    // Regression: entry() used to gate occupancy on `!file.name.is_empty()`
    // instead of the codebase's `product_key.is_some()` occupancy rule, so
    // an empty-named but occupied destination slot (a real device state,
    // see `preset_slot_occupancy_comes_from_catalog_identity_not_display_name`
    // in device/mod.rs) silently bypassed this guard and let savePresetAs
    // overwrite it without confirmOverwrite.
    let mut library = PresetLibrary::default();
    library.ingest(PresetFolderListing {
        order: None,
        key: "/media/p4/Presets/Live".into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![
            PresetFileListing {
                position: 8,
                name: "Current".into(),
                instrument: 2,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/Current.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 10,
                name: String::new(),
                instrument: 1,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/empty-name.pb".into()),
                ..Default::default()
            },
        ],
    });
    let snapshot = GatewaySnapshot {
        preset_name: "Current".into(),
        setlist_key: "/media/p4/Presets/Live".into(),
        preset_position: 8,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    let error = plan_preset_mutation(
        "device.savePresetAs",
        &json!({
            "setlistKey": "/media/p4/Presets/Live", "position": 10,
            "name": "Saved", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmOverwrite": false
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap_err();
    assert!(
        error.contains("explicit overwrite confirmation is required"),
        "{error:?} must refuse an empty-named but occupied slot without confirmOverwrite"
    );

    let confirmed = plan_preset_mutation(
        "device.savePresetAs",
        &json!({
            "setlistKey": "/media/p4/Presets/Live", "position": 10,
            "name": "Saved", "expectedPresetName": "Current",
            "expectedPosition": 8, "confirmOverwrite": true
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap();
    assert!(matches!(
        confirmed.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::SavePreset { position: 10, .. })
    ));
}

#[test]
fn copy_preset_does_not_treat_two_independently_empty_names_as_a_duplicate() {
    // Regression: once entry() was widened (see device/mod.rs's occupancy
    // comment) to treat a product-keyed-but-empty-named slot as occupied,
    // the copyPreset duplicate-name guard's `destination.name ==
    // source.name` started matching two unrelated empty-named occupied
    // slots ("" == ""), wrongly refusing the copy as a duplicate even
    // though the two presets have nothing in common. An empty name is not
    // a meaningful name to protect against duplication.
    let mut library = PresetLibrary::default();
    library.ingest(PresetFolderListing {
        order: None,
        key: "/media/p4/Presets/Live".into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![
            PresetFileListing {
                position: 8,
                name: "Current".into(),
                instrument: 2,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/Current.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 9,
                name: String::new(),
                instrument: 1,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/source-empty-name.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 10,
                name: String::new(),
                instrument: 3,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/destination-empty-name.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 11,
                name: "Shared name".into(),
                instrument: 1,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/shared-a.pb".into()),
                ..Default::default()
            },
            PresetFileListing {
                position: 12,
                name: "Shared name".into(),
                instrument: 1,
                instrument_present: true,
                product_key: Some("/media/p4/Presets/Live/shared-b.pb".into()),
                ..Default::default()
            },
        ],
    });
    let snapshot = GatewaySnapshot {
        preset_name: "Current".into(),
        setlist_key: "/media/p4/Presets/Live".into(),
        preset_position: 8,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };

    // Two unrelated empty-named occupied slots must not be treated as a
    // "duplicate name" copy.
    let empty_to_empty = plan_preset_mutation(
        "device.copyPreset",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live", "sourcePosition": 9,
            "sourceName": "", "destinationSetlistKey": "/media/p4/Presets/Live",
            "destinationPosition": 10, "confirmOverwrite": true
        }),
        Some(&snapshot),
        &library,
    );
    assert!(
        empty_to_empty.is_ok(),
        "copying between two independently empty-named slots must not be refused as a duplicate: {:?}",
        empty_to_empty.err()
    );

    // Two slots that genuinely share a real, non-empty name must still be
    // refused.
    let real_duplicate = plan_preset_mutation(
        "device.copyPreset",
        &json!({
            "sourceSetlistKey": "/media/p4/Presets/Live", "sourcePosition": 11,
            "sourceName": "Shared name", "destinationSetlistKey": "/media/p4/Presets/Live",
            "destinationPosition": 12, "confirmOverwrite": true
        }),
        Some(&snapshot),
        &library,
    )
    .unwrap_err();
    assert!(
        real_duplicate.contains("cannot prove that overwrite changed"),
        "{real_duplicate:?} must still refuse a copy between two slots sharing a real name"
    );
}

#[test]
fn create_setlist_matches_desktop_capacity_collision_and_utf8_plan() {
    let mut library = PresetLibrary::default();
    for (name, factory) in [("Factory Library", true), ("Тур", false), ("Тур2", false)] {
        let key = if factory {
            "/opt/neuraldsp/Factory Library".to_string()
        } else {
            format!("/media/p4/Presets/{name}")
        };
        library.ingest(PresetFolderListing {
            order: None,
            key,
            name: name.into(),
            file_type: Some(0),
            is_factory: factory,
            is_plugin: false,
            files: Vec::new(),
        });
    }
    let plan = plan_preset_mutation(
        "device.createSetlist",
        &json!({"name": "Тур"}),
        None,
        &library,
    )
    .unwrap();
    assert_eq!(plan.saved_name, "Тур3");
    assert_eq!(plan.setlist_key, "/media/p4/Presets/Тур3");
    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].verification, GatewayVerification::None);
    assert_eq!(plan.stages[0].settle_ms, 0);
    assert_eq!(plan.catalog_policy, CatalogVerificationPolicy::default());
    assert!(matches!(
        plan.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::CreateSetlist { ref name })
            if name == "Тур3"
    ));
    let creation = plan.folder_copy.unwrap();
    assert_eq!(creation.operation, "create");
    assert_eq!(creation.source_name, "Тур");
    assert_eq!(
        creation.catalog_policy,
        CatalogVerificationPolicy::default()
    );
    assert!(creation
        .before_folder_keys
        .iter()
        .any(|key| key == "/opt/neuraldsp/Factory Library"));

    let mut full = PresetLibrary::default();
    full.ingest(PresetFolderListing {
        order: None,
        key: "/opt/neuraldsp/Factory Library".into(),
        name: "Factory Library".into(),
        file_type: Some(0),
        is_factory: true,
        is_plugin: false,
        files: Vec::new(),
    });
    for index in 0..12 {
        full.ingest(PresetFolderListing {
            order: None,
            key: format!("/media/p4/Presets/User{index}"),
            name: format!("User{index}"),
            file_type: Some(0),
            is_factory: false,
            is_plugin: false,
            files: Vec::new(),
        });
    }
    let error = plan_preset_mutation(
        "device.createSetlist",
        &json!({"name": "Thirteen"}),
        None,
        &full,
    )
    .unwrap_err();
    assert!(error.contains("maximum of 12"));
}

#[test]
fn delete_preset_resolves_an_authoritative_key_and_rejects_stale_identity() {
    let setlist_key = "/media/p4/Presets/Live";
    let product_key = "/media/p4/Presets/Live/opaque-7f3d.pb";
    let mut library = PresetLibrary::default();
    library.ingest(PresetFolderListing {
        order: None,
        key: setlist_key.into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![PresetFileListing {
            position: 37,
            name: "Visible Name".into(),
            instrument: 4,
            instrument_present: true,
            product_key: Some(product_key.into()),
            ..Default::default()
        }],
    });
    let snapshot = GatewaySnapshot::default();
    let params = json!({
        "setlistKey": setlist_key,
        "position": 37,
        "expectedName": "Visible Name"
    });
    let plan =
        plan_preset_mutation("device.deletePreset", &params, Some(&snapshot), &library).unwrap();
    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].verification, GatewayVerification::None);
    assert_eq!(plan.stages[0].settle_ms, 0);
    assert_eq!(
        plan.catalog_policy,
        CatalogVerificationPolicy::persistent_file_mutation()
    );
    assert!(matches!(
        plan.stages[0].write,
        PlannedWrite::HidOperation(DeviceOperation::DeletePreset {
            ref setlist_key,
            ref product_key,
        }) if setlist_key == "/media/p4/Presets/Live"
            && product_key == "/media/p4/Presets/Live/opaque-7f3d.pb"
    ));
    let deletion = plan.deletion.unwrap();
    assert_eq!(deletion.expected_position, 37);
    assert_eq!(deletion.expected_name, "Visible Name");
    assert_eq!(deletion.product_key, product_key);

    for invalid in [
        json!({"setlistKey": setlist_key, "position": 37, "expectedName": "Stale"}),
        json!({"setlistKey": setlist_key, "position": 38, "expectedName": "Visible Name"}),
        json!({"setlistKey": setlist_key, "position": 256, "expectedName": "Visible Name"}),
        json!({"setlistKey": "/opt/neuraldsp/Factory Library", "position": 37, "expectedName": "Visible Name"}),
    ] {
        assert!(
            plan_preset_mutation("device.deletePreset", &invalid, Some(&snapshot), &library,)
                .is_err()
        );
    }

    let mut missing_key = PresetLibrary::default();
    missing_key.ingest(PresetFolderListing {
        order: None,
        key: setlist_key.into(),
        name: "Live".into(),
        file_type: Some(0),
        is_factory: false,
        is_plugin: false,
        files: vec![PresetFileListing {
            position: 37,
            name: "Visible Name".into(),
            instrument: 4,
            instrument_present: true,
            product_key: None,
            ..Default::default()
        }],
    });
    assert!(plan_preset_mutation(
        "device.deletePreset",
        &params,
        Some(&snapshot),
        &missing_key,
    )
    .is_err());
}

#[test]
fn retained_library_reads_select_opaque_keys_without_a_wire_fallback() {
    use crate::device::native_library::LibraryKind;
    for params in [json!({}), json!({"folder": null}), json!({"folder": ""})] {
        let target = plan_retained_library_read("device.irs", &params).unwrap();
        assert_eq!(target.kind, LibraryKind::Ir);
        assert_eq!(target.key, "local_ir_root");
    }
    for key in [
        "local_ir_root/",
        "folder//child/",
        "../literal",
        " key ",
        "folder\\child",
    ] {
        let target = plan_retained_library_read("device.irs", &json!({"folder": key})).unwrap();
        assert_eq!(target.key, key);
    }
    for folder in [
        json!(false),
        json!(3),
        json!([]),
        json!({}),
        json!("bad\nkey"),
        json!("x".repeat(513)),
    ] {
        assert!(plan_retained_library_read("device.irs", &json!({"folder": folder})).is_err());
    }
    let capture =
        plan_retained_library_read("device.captures", &json!({"folder": "other"})).unwrap();
    assert_eq!(capture.kind, LibraryKind::Capture);
    assert_eq!(capture.key, "local_nc_root");
    assert!(plan_retained_library_read("device.identity", &json!({})).is_err());
    for method in ["device.captures", "device.irs"] {
        assert!(plan_gateway_read(method, &json!({}), 84).is_err());
    }
}

#[test]
fn native_library_reads_and_writes_are_planned_with_strict_guards() {
    let recents = plan_gateway_read("device.recents", &Value::Null, 81).unwrap();
    assert_eq!(recents.response_type, 20);
    assert_eq!(recents.timeout_ms, 10_000);
    assert!(matches!(
        recents.operation,
        DeviceOperation::ReadRecentsFavorites {
            favorites: false,
            request_id: 81
        }
    ));
    let messages = recents.operation.try_encode().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_type, 20);
    assert_eq!(messages[0].payload, [0x08, 0x03, 0x10, 0x51]);
    let favorites = plan_gateway_read("device.favorites", &Value::Null, 82).unwrap();
    assert_eq!(
        favorites.response_type,
        profile::MESSAGE_TYPE_RECENTS_FAVORITES
    );
    assert_eq!(favorites.timeout_ms, 20_000);
    assert_eq!(favorites.projection.expected_request_id(), Some(82));
    assert!(matches!(
        favorites.operation,
        DeviceOperation::ReadRecentsFavorites {
            favorites: true,
            request_id: 82
        }
    ));
    assert!(matches!(
        favorites.projection,
        GatewayResponseProjection::Favorites { request_id: 82 }
    ));
    let frames = favorites.operation.try_encode().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].message_type,
        profile::MESSAGE_TYPE_RECENTS_FAVORITES
    );
    assert_eq!(frames[0].payload, [0x08, 0x03, 0x10, 0x52, 0x18, 0x01]);
    // Public library reads must never fall back to an invented correlated
    // READ when retained native evidence has not arrived.
    assert!(plan_gateway_read("device.captures", &Value::Null, 83).is_err());
    assert!(plan_gateway_read("device.irs", &json!({"folder":"factory_ir_root"}), 84).is_err());

    assert!(plan_gateway_write(
        "device.deletePreset",
        &json!({"setlistKey": "/opt/Presets/Factory", "name": "Factory"}),
        None,
    )
    .is_err());
    let retarget = plan_gateway_write(
        "device.loadCapture",
        &json!({
            "row": 1, "column": 2, "key": "capture/", "name": "Crunch",
            "modelId": null, "expectedModelId": null, "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    );
    assert!(retarget.is_err());

    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    state
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    name: Some(binary_preset::Name::Name("Current".into())),
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(1)),
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(14_000)),
                            column: Some(model::Column::Column(2)),
                            params: vec![Param {
                                index: Some(param::Index::Index(5)),
                                param_values: vec![ParamValue {
                                    value: Some(param_value::Value::StringValue(
                                        "capture/old".into(),
                                    )),
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
    let retarget = plan_gateway_write_with_state(
        "device.loadCapture",
        &json!({
            "row": 1, "column": 2, "key": "capture/new", "name": "New",
            "modelId": null, "expectedModelId": 14000, "expectedPresetName": "Current"
        }),
        &state,
    )
    .unwrap();
    assert!(matches!(
        retarget.write,
        PlannedWrite::HidOperation(DeviceOperation::RetargetCapture {
            row: 1, column: 2, ref key, ..
        }) if key == "capture/new"
    ));
    let verified_capture = plan_gateway_write(
        "device.loadCapture",
        &json!({
            "row": 1, "column": 2, "key": "capture/", "name": "Crunch",
            "modelId": 14000, "expectedModelId": null, "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    )
    .unwrap();
    assert!(matches!(
        verified_capture.write,
        PlannedWrite::HidOperation(DeviceOperation::LoadCapture {
            row: 1, column: 2, ref key, model_id: Some(14000)
        }) if key == "capture/"
    ));
    assert!(matches!(
        verified_capture.verification,
        GatewayVerification::Block {
            row: 1,
            column: 2,
            model_id: Some(14000),
            present: true
        }
    ));
    assert!(plan_gateway_write(
        "device.loadCapture",
        &json!({
            "row": 1, "column": 2, "key": "capture/", "name": "Crunch",
            "modelId": 14000, "expectedModelId": 14000, "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            blocks: vec![crate::device::GridBlock {
                id: "capture".into(),
                model_id: Some(14000),
                category_id: None,
                name: "Crunch".into(),
                kind: "capture".into(),
                category: None,
                plugin: None,
                plugin_id: None,
                row: 1,
                column: 2,
                bypassed: Some(false),
                bypass_expression: None,
                color: None,
                glyph: None,
                footswitch: None,
                footswitch_order: None,
                footswitch_assignment_type: None,
                ..Default::default()
            }],
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    )
    .is_err());
    assert!(plan_gateway_write(
        "device.loadIr",
        &json!({
            "row": 1, "column": 2, "key": "ir/key", "name": "Room",
            "slot": 2, "modelId": null, "expectedModelId": 13000, "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            blocks: vec![crate::device::GridBlock {
                id: "ir".into(),
                row: 1,
                column: 2,
                model_id: Some(13000),
                category_id: None,
                name: "IR Loader".into(),
                kind: "ir".into(),
                category: None,
                plugin: None,
                plugin_id: None,
                bypassed: Some(false),
                bypass_expression: None,
                color: None,
                glyph: None,
                footswitch: None,
                footswitch_order: None,
                footswitch_assignment_type: None,
                ..Default::default()
            }],
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    )
    .is_err());

    let ir = plan_gateway_write(
        "device.loadIr",
        &json!({
            "row": 1, "column": 2, "key": "ir/key", "name": "Room",
            "slot": 1, "modelId": null, "expectedModelId": 13000,
            "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            blocks: vec![crate::device::GridBlock {
                id: "ir".into(),
                row: 1,
                column: 2,
                model_id: Some(13000),
                category_id: None,
                name: "IR Loader".into(),
                kind: "ir".into(),
                category: None,
                plugin: None,
                plugin_id: None,
                bypassed: Some(false),
                bypass_expression: None,
                color: None,
                glyph: None,
                footswitch: None,
                footswitch_order: None,
                footswitch_assignment_type: None,
                ..Default::default()
            }],
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    )
    .unwrap();
    assert!(matches!(
        ir.write,
        PlannedWrite::HidOperation(DeviceOperation::LoadIr {
            row: 1,
            column: 2,
            ref key,
            slot: 1,
        }) if key == "ir/key"
    ));
    assert_eq!(ir.verification, GatewayVerification::None);
    assert!(plan_gateway_write(
        "device.loadIr",
        &json!({
            "row": 1, "column": 2, "key": "ir/key", "name": "Room",
            "slot": 0, "modelId": 13000, "expectedModelId": null,
            "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    )
    .expect_err("loadIr must not place a model")
    .contains("separate unsupported workflow"));
    assert!(plan_gateway_write(
        "device.loadCapture",
        &json!({
            "row": 1, "column": 2, "key": "capture/", "name": "Crunch",
            "modelId": null, "expectedPresetName": "Current"
        }),
        Some(&GatewaySnapshot {
            preset_name: "Current".into(),
            grid_layout_complete: true,
            ..GatewaySnapshot::default()
        }),
    )
    .is_err());
}

#[test]
fn correlated_reads_and_remote_screen_writes_are_planned_once() {
    use crate::proto::cortex_protobuf_v2 as pa;
    use prost::Message;

    let diagnostics = plan_gateway_read("device.diagnostics", &Value::Null, 73).unwrap();
    assert_eq!(diagnostics.response_type, profile::MESSAGE_TYPE_DIAGNOSTICS);
    assert_eq!(diagnostics.timeout_ms, 5_000);
    assert!(matches!(
        diagnostics.operation,
        DeviceOperation::ReadDiagnostics { request_id: 73 }
    ));
    assert_eq!(diagnostics.projection.expected_request_id(), Some(73));
    let diagnostics_frames = diagnostics.operation.try_encode().unwrap();
    assert_eq!(diagnostics_frames.len(), 1);
    assert_eq!(diagnostics_frames[0].message_type, 7);
    assert_eq!(diagnostics_frames[0].payload, [0x08, 0x03, 0x10, 0x49]);
    let payload = pa::DiagnosticsMessage {
        action: pa::message_action::Enum::Update as i32,
        request_id: Some(pa::diagnostics_message::RequestId::RequestId(73)),
        soc2arm_core_diagnostics: Some(
            pa::diagnostics_message::Soc2armCoreDiagnostics::Soc2armCoreDiagnostics(
                pa::Soc2armCommsDiagnosticsMessage {
                    usb_audio_in_rd_wr_distance_average: 7,
                    usb_audio_out_gaps_detected: 8,
                    usb_audio_out_gap_resets: 9,
                    usb_audio_out_buffer_resets: 10,
                    ..Default::default()
                },
            ),
        ),
        soc1_to_soc2_dropped_count: Some(
            pa::diagnostics_message::Soc1ToSoc2DroppedCount::Soc1ToSoc2DroppedCount(4),
        ),
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        diagnostics.projection.decode(&payload).unwrap()["soc1ToSoc2DroppedCount"],
        4
    );
    let decoded = diagnostics.projection.decode(&payload).unwrap();
    assert_eq!(decoded["soc2Arm"]["audioInputDistanceAverage"], 7);
    assert_eq!(decoded["soc2Arm"]["audioOutputGapsDetected"], 8);
    assert_eq!(decoded["soc2Arm"]["audioOutputGapResets"], 9);
    assert_eq!(decoded["soc2Arm"]["audioOutputBufferResets"], 10);

    let tuner = plan_gateway_read("device.tunerSettings", &Value::Null, 0).unwrap();
    assert_eq!(tuner.response_type, 6);
    assert_eq!(tuner.timeout_ms, 5_000);
    assert!(matches!(tuner.operation, DeviceOperation::ReadTuner));
    assert!(matches!(
        tuner.projection,
        GatewayResponseProjection::TunerSettings
    ));

    // The desktop General Settings read carries no request ID.
    let general = plan_gateway_read("device.generalSettings", &Value::Null, 73).unwrap();
    assert_eq!(
        general.response_type,
        profile::MESSAGE_TYPE_GENERAL_SETTINGS
    );
    assert_eq!(general.timeout_ms, 5_000);
    assert!(matches!(
        general.operation,
        DeviceOperation::ReadGeneralSettings
    ));
    assert_eq!(general.projection.expected_request_id(), None);
    assert!(matches!(
        general.projection,
        GatewayResponseProjection::GeneralSettings
    ));

    let io = plan_gateway_read("device.ioSettings", &json!({}), 99).unwrap();
    assert_eq!(io.response_type, profile::MESSAGE_TYPE_IO_SETTINGS);
    assert_eq!(io.timeout_ms, 10_000);
    assert!(matches!(io.operation, DeviceOperation::ReadIoSettings));
    assert!(matches!(
        io.projection,
        GatewayResponseProjection::IoSettings
    ));
    assert_eq!(io.projection.expected_request_id(), None);
    let io_frames = io.operation.try_encode().unwrap();
    assert_eq!(io_frames.len(), 1);
    assert_eq!(io_frames[0].message_type, profile::MESSAGE_TYPE_IO_SETTINGS);
    assert_eq!(io_frames[0].payload, [0x08, 0x03]);

    let pinned = plan_gateway_read("device.pinnedModels", &json!({}), 101).unwrap();
    assert_eq!(pinned.response_type, profile::MESSAGE_TYPE_PINNED_MODELS);
    assert_eq!(pinned.timeout_ms, 8_000);
    assert!(matches!(
        pinned.operation,
        DeviceOperation::ReadPinnedModels
    ));
    assert!(matches!(
        pinned.projection,
        GatewayResponseProjection::PinnedModels
    ));
    assert_eq!(pinned.projection.expected_request_id(), None);
    let pinned_frames = pinned.operation.try_encode().unwrap();
    assert_eq!(pinned_frames.len(), 1);
    assert_eq!(
        pinned_frames[0].message_type,
        profile::MESSAGE_TYPE_PINNED_MODELS
    );
    assert_eq!(pinned_frames[0].payload, [0x08, 0x03]);

    let preset_refresh = plan_gateway_read("device.currentPreset", &Value::Null, 74).unwrap();
    assert_eq!(
        preset_refresh.response_type,
        profile::MESSAGE_TYPE_RECALL_PRESET
    );
    assert!(matches!(
        preset_refresh.operation,
        DeviceOperation::ReadCurrentPreset
    ));
    // The preset read is the bare boot frame: nothing to correlate by id.
    assert_eq!(preset_refresh.projection.expected_request_id(), None);
    assert!(matches!(
        preset_refresh.projection,
        GatewayResponseProjection::CurrentPreset
    ));

    let screenshot = plan_gateway_read(
        "device.presetScreenshot",
        &json!({"folderName": "Live", "position": 12, "isFactory": false}),
        91,
    )
    .unwrap();
    assert_eq!(screenshot.response_type, 25);
    assert_eq!(screenshot.timeout_ms, 10_000);
    assert_eq!(screenshot.projection.expected_request_id(), Some(91));
    let screenshot_frames = match &screenshot.operation {
        DeviceOperation::PresetScreenshot { .. } => screenshot.operation.try_encode().unwrap(),
        _ => panic!("unexpected screenshot operation"),
    };
    assert_eq!(screenshot_frames.len(), 1);
    assert_eq!(
        screenshot_frames[0].payload,
        [0x08, 0x03, 0x10, 0x5b, 0x1a, 0x04, b'L', b'i', b'v', b'e', 0x28, 0x0c]
    );
    assert!(matches!(
        screenshot.projection,
        GatewayResponseProjection::PresetScreenshot { request_id: 91, .. }
    ));
    assert!(plan_gateway_read(
        "device.presetScreenshot",
        &json!({"folderName": "Live", "position": 256, "isFactory": false}),
        92,
    )
    .is_err());

    let capture = plan_gateway_read("device.captureScreen", &Value::Null, 93).unwrap();
    assert_eq!(capture.response_type, profile::MESSAGE_TYPE_REMOTE_CONTROL);
    assert_eq!(capture.timeout_ms, 10_000);
    assert_eq!(capture.projection.expected_request_id(), None);
    assert!(matches!(
        capture.projection,
        GatewayResponseProjection::CapturedScreen
    ));
    let capture_frames = capture.operation.try_encode().unwrap();
    assert_eq!(capture_frames.len(), 1);
    assert_eq!(capture_frames[0].payload, [0x08, 0x03, 0x22, 0x00]);

    // A hold is a touch-down, a wait and a touch-up: two frames the host sends
    // REMOTE_HOLD_INTERVAL_MS apart. Its long-press effect on the unit is not
    // hardware-verified.
    let hold = plan_gateway_write(
        "device.longTapScreen",
        &json!({"x": 799.0, "y": 479.0}),
        None,
    )
    .unwrap();
    assert_eq!(hold.verification, GatewayVerification::None);
    assert_eq!(
        hold.write.inter_message_interval_ms(),
        profile::REMOTE_HOLD_INTERVAL_MS
    );
    assert_eq!(gateway_write_preflight_method("device.longTapScreen"), None);
    for params in [
        json!({"x": 800.0, "y": 0.0}),
        json!({"x": 0.5, "y": 0.0}),
        json!({"x": 0.0, "y": 480.0}),
        json!({"x": "0", "y": 0.0}),
    ] {
        assert!(plan_gateway_write("device.longTapScreen", &params, None).is_err());
    }

    let tap =
        plan_gateway_write("device.tapScreen", &json!({"x": 799.0, "y": 479.0}), None).unwrap();
    assert_eq!(tap.verification, GatewayVerification::None);
    // A tap is the single TAP frame: nothing to pace.
    assert_eq!(tap.write.inter_message_interval_ms(), 0);
    let PlannedWrite::HidOperation(tap_operation) = &tap.write else {
        panic!("a tap is a HID operation");
    };
    assert_eq!(tap_operation.clone().try_encode().unwrap().len(), 1);
    assert_eq!(gateway_write_preflight_method("device.tapScreen"), None);
    assert!(matches!(
        tap.write,
        PlannedWrite::HidOperation(DeviceOperation::ScreenTap { .. })
    ));
    for params in [
        json!({"x": 800.0, "y": 10.0}),
        json!({"x": 10.5, "y": 10.0}),
        json!({"x": 10.0, "y": 479.5}),
    ] {
        assert!(plan_gateway_write("device.tapScreen", &params, None).is_err());
    }

    assert!(plan_gateway_write("device.setDeviceName", &json!({"name": "Stage QC"}), None).is_ok());
    assert!(
        plan_gateway_write("device.setDeviceName", &json!({"name": "bad\nname"}), None).is_err()
    );
}

#[test]
fn set_device_name_enforces_unicode_bounds_and_sends_one_unread_frame() {
    let maximum = "🚀".repeat(64);
    let plan = plan_gateway_write("device.setDeviceName", &json!({"name": maximum}), None).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetDeviceName(ref name))
            if name == &"🚀".repeat(64)
    ));
    for invalid in [
        "".to_string(),
        "x".repeat(65),
        "bad\nname".to_string(),
        "bad\u{7f}name".to_string(),
    ] {
        assert!(
            plan_gateway_write("device.setDeviceName", &json!({"name": invalid}), None,).is_err()
        );
    }

    // The name is carried verbatim: no Unicode normalisation, one frame,
    // and no readback of any kind (a device extension with no Cortex
    // Control sender; hardware verification pending).
    let composed =
        plan_gateway_write("device.setDeviceName", &json!({"name": "Café"}), None).unwrap();
    let decomposed = plan_gateway_write(
        "device.setDeviceName",
        &json!({"name": "Cafe\u{301}"}),
        None,
    )
    .unwrap();
    assert_ne!(composed.write, decomposed.write);
    assert_eq!(composed.verification, GatewayVerification::None);
    let PlannedWrite::HidOperation(operation) = composed.write else {
        panic!("setDeviceName must use the HID operation lane");
    };
    let frames = operation.try_encode().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_VERSION);
    let policy = gateway_write_verification_policy("device.setDeviceName");
    assert_eq!(policy.readback_method, None);
    assert_eq!(policy.preflight_method, None);
    assert_eq!(gateway_write_readback_method("device.setDeviceName"), None);
}

#[test]
fn tuner_writes_require_explicit_hazard_acknowledgement() {
    let rejected = plan_gateway_write("device.setTunerMute", &json!({"muted": true}), None);
    assert!(rejected.is_err());

    for muted in [false, true] {
        let mute = plan_gateway_write(
            "device.setTunerMute",
            &json!({"muted": muted, "confirmTunerActivation": true}),
            None,
        )
        .unwrap();
        assert!(matches!(
            mute.write,
            PlannedWrite::HidOperation(DeviceOperation::SetTunerMute(value)) if value == muted
        ));
    }

    let input = plan_gateway_write(
        "device.setTunerInput",
        &json!({"inputPortId": 8, "confirmTunerActivation": true}),
        None,
    )
    .unwrap();
    assert!(matches!(
        input.write,
        PlannedWrite::HidOperation(DeviceOperation::SetTunerInput(8))
    ));
    assert!(plan_gateway_write(
        "device.setTunerInput",
        &json!({"inputPortId": 6, "confirmTunerActivation": true}),
        None,
    )
    .is_err());

    let reference = plan_gateway_write(
        "device.setTunerReference",
        &json!({"referenceOffsetHz": 2.0, "confirmTunerActivation": true}),
        None,
    )
    .unwrap();
    assert!(matches!(
        reference.write,
        PlannedWrite::HidOperation(DeviceOperation::SetTunerReference(value)) if (value - 2.0).abs() < f32::EPSILON
    ));
    for value in [-40.0, 40.0] {
        assert!(plan_gateway_write(
            "device.setTunerReference",
            &json!({
                "referenceOffsetHz": value,
                "confirmTunerActivation": true
            }),
            None,
        )
        .is_ok());
    }
    for value in [-40.01, 40.01] {
        assert!(plan_gateway_write(
            "device.setTunerReference",
            &json!({
                "referenceOffsetHz": value,
                "confirmTunerActivation": true
            }),
            None,
        )
        .is_err());
    }
}

#[test]
fn tuner_writes_send_one_frame_and_are_never_read_back() {
    // Cortex Control never reads a tuner write back. Each of the four
    // sends its single TunerMessage UPDATE and carries no readback, no
    // preflight and no verification predicate, so the broker returns
    // accepted_unverified once the frame is delivered.
    let cases: [(&str, Value, &[u8]); 5] = [
        (
            "device.setTunerInput",
            json!({"inputPortId": 3, "confirmTunerActivation": true}),
            &[0x08, 0x01, 0x18, 0x03],
        ),
        (
            "device.setTunerMute",
            json!({"muted": false, "confirmTunerActivation": true}),
            &[0x08, 0x01, 0x28, 0x00],
        ),
        (
            "device.setTunerMute",
            json!({"muted": true, "confirmTunerActivation": true}),
            &[0x08, 0x01, 0x28, 0x01],
        ),
        (
            "device.setTunerMeter",
            json!({"enabled": true, "confirmTunerActivation": true}),
            &[0x08, 0x01, 0x30, 0x01],
        ),
        (
            "device.setTunerReference",
            json!({"referenceOffsetHz": 2.0, "confirmTunerActivation": true}),
            &[0x08, 0x01, 0x25, 0x00, 0x00, 0x00, 0x40],
        ),
    ];
    for (method, params, payload) in cases {
        let plan = plan_gateway_write(method, &params, None).unwrap();
        assert_eq!(plan.verification, GatewayVerification::None, "{method}");
        let PlannedWrite::HidOperation(operation) = plan.write else {
            panic!("{method} must use the HID operation lane");
        };
        let frames = operation.try_encode().unwrap();
        assert_eq!(frames.len(), 1, "{method}");
        assert_eq!(frames[0].message_type, profile::MESSAGE_TYPE_TUNER);
        assert_eq!(frames[0].payload, payload, "{method}");
        let policy = gateway_write_verification_policy(method);
        assert_eq!(policy.readback_method, None, "{method}");
        assert_eq!(policy.preflight_method, None, "{method}");
        assert_eq!(gateway_write_readback_method(method), None, "{method}");
        // No matcher exists for them any more: an unrelated reply can
        // never be read as confirmation, or as a mismatch.
        assert!(
            !gateway_write_readback_matches(method, &params, &json!({"muted": false})),
            "{method}"
        );
    }
}

#[test]
fn restore_tuner_audio_is_not_a_planned_write() {
    // It was a byte-identical alias of setTunerMute(false); callers send
    // setTunerMute with muted=false instead.
    assert!(plan_gateway_write(
        "device.restoreTunerAudio",
        &json!({"confirmPreferenceReset": true}),
        None,
    )
    .is_err());
}

#[test]
fn input_port_writes_use_the_physical_domain_and_exact_wire_enums() {
    for input_port_id in [0, 3, 6, 14] {
        assert!(plan_gateway_write(
            "device.setInputPort",
            &json!({"inputPortId": input_port_id, "levelDb": 0.0}),
            None,
        )
        .is_err());
    }
    for params in [
        json!({"inputPortId": 1, "impedance": 0.1}),
        json!({"inputPortId": 2, "inputType": 0.25}),
        json!({"inputPortId": 4, "groundLift": 0.5}),
    ] {
        assert!(plan_gateway_write("device.setInputPort", &params, None).is_err());
    }

    let plan = plan_gateway_write(
        "device.setInputPort",
        &json!({
            "inputPortId": 5,
            "levelDb": 60.0,
            "impedance": 0.875,
            "inputType": 0.5,
            "groundLift": 1.0
        }),
        None,
    )
    .unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetInputPort {
            input_port_id: 5,
            level: Some(1.0),
            impedance: Some(0.875),
            input_type: Some(0.5),
            ground_lift: Some(1.0),
        })
    ));
}

#[test]
fn global_eq_mode_cycle_and_looper_are_planned_and_bounded() {
    // This uncorrelated read deliberately ignores the allocator's request ID.
    let inhibited = plan_gateway_read("device.inhibitedModules", &Value::Null, 77).unwrap();
    assert_eq!(inhibited.response_type, 42);
    assert_eq!(inhibited.timeout_ms, 5_000);
    assert!(matches!(
        inhibited.operation,
        DeviceOperation::ReadInhibitedModules
    ));
    assert!(matches!(
        inhibited.projection,
        GatewayResponseProjection::InhibitedModules
    ));

    let global_eq = plan_gateway_read("device.globalEq", &Value::Null, 0).unwrap();
    assert_eq!(global_eq.response_type, 38);
    assert!(matches!(global_eq.operation, DeviceOperation::ReadGlobalEq));
    assert!(matches!(
        global_eq.projection,
        GatewayResponseProjection::GlobalEq
    ));
    assert_eq!(global_eq.projection.expected_request_id(), None);
    assert_eq!(
        global_eq.operation.try_encode().unwrap()[0].payload,
        [0x08, 0x03]
    );
    let global_eq_reply = pa::GlobalEqMessage {
        action: pa::message_action::Enum::Update as i32,
        parameters: vec![
            pa::GlobalEqParameter {
                parameter_index: -1,
                value: 0.25,
            },
            pa::GlobalEqParameter {
                parameter_index: 0,
                value: 1.25,
            },
            pa::GlobalEqParameter {
                parameter_index: 27,
                value: -0.5,
            },
            pa::GlobalEqParameter {
                parameter_index: 28,
                value: 0.75,
            },
        ],
        ..Default::default()
    }
    .encode_to_vec();
    let global_eq_value = global_eq.projection.decode(&global_eq_reply).unwrap();
    assert_eq!(
        global_eq_value["parameters"],
        json!([
            {"parameterIndex": 0, "value": 1.25},
            {"parameterIndex": 27, "value": -0.5}
        ])
    );
    assert!(global_eq_value.get("bypassed").is_none());

    let mode_cycle = plan_gateway_read("device.modeCycle", &Value::Null, 0).unwrap();
    assert_eq!(mode_cycle.response_type, 14);
    assert_eq!(mode_cycle.timeout_ms, 5_000);
    assert_eq!(
        mode_cycle.operation.clone().try_encode().unwrap()[0].payload,
        [0x08, 0x03]
    );
    let valid_mode_cycle = pa::ModeMessage {
        action: pa::message_action::Enum::Update as i32,
        available_modes: Some(pa::mode_message::AvailableModes::AvailableModes(
            pa::AvailableModes {
                modes: vec![8, 0, 6],
            },
        )),
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        mode_cycle.projection.decode(&valid_mode_cycle).unwrap(),
        json!({"slots": [8, 0, 6]})
    );
    for slots in [vec![1, 1], vec![9], vec![0, 1, 2, 3]] {
        let invalid_mode_cycle = pa::ModeMessage {
            action: pa::message_action::Enum::Update as i32,
            available_modes: Some(pa::mode_message::AvailableModes::AvailableModes(
                pa::AvailableModes { modes: slots },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(mode_cycle.projection.decode(&invalid_mode_cycle).is_err());
    }
    assert!(matches!(
        mode_cycle.operation,
        DeviceOperation::ReadModeCycle
    ));

    // Regression: this test asserted PERFORMANCE_MIDI_METHODS.len() == 4.
    // The show-tuner audit legitimately moved a method off the host-MIDI
    // dispatch, leaving the count stale at 3 != 4, and the assertion was
    // deleted rather than corrected — removing the only guard on which
    // methods are dispatched as host MIDI rather than over USB. Assert the
    // set by name instead of its size, so a change has to be deliberate and
    // reads as a diff of method names.
    assert_eq!(
        crate::device::generated_gateway::PERFORMANCE_MIDI_METHODS,
        [
            "device.controlLooper",
            "device.pressFootswitch",
            "device.tapTempo",
        ]
    );

    let looper = plan_gateway_read("device.looperStatus", &Value::Null, 0).unwrap();
    assert_eq!(looper.response_type, 28);
    assert_eq!(looper.timeout_ms, 5_000);
    assert_eq!(looper.projection.expected_request_id(), None);
    assert!(matches!(
        looper.operation,
        DeviceOperation::ReadLooperStatus
    ));
    assert_eq!(
        looper.operation.try_encode().unwrap()[0].payload,
        [0x08, 0x03]
    );
    let looper_reply = pa::LooperMessage {
        action: pa::message_action::Enum::Update as i32,
        status: Some(pa::looper_message::Status::Status(pa::LooperStatus {
            one_shot_stopped: 2,
            in_reverse: 0,
            ..Default::default()
        })),
        update_type: Some(pa::looper_message::UpdateType::UpdateType(1)),
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        looper.projection.decode(&looper_reply).unwrap()["oneShotStopped"],
        true
    );
    assert_eq!(
        looper.projection.decode(&looper_reply).unwrap()["inReverse"],
        false
    );
    assert_eq!(
        looper.projection.decode(&looper_reply).unwrap()["updateType"],
        1
    );

    let band = plan_gateway_write(
        "device.setGlobalEqBand",
        &json!({"band": 2, "gain": 0.25, "filterType": 4, "enabled": false}),
        None,
    )
    .unwrap();
    assert!(matches!(
        band.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalEqParameters(ref controls))
            if controls == &vec![(5, 0.25), (8, 1.0), (9, 0.0)]
    ));
    let enabled = plan_gateway_write(
        "device.setGlobalEqBand",
        &json!({"band": 5, "enabled": true}),
        None,
    )
    .unwrap();
    assert!(matches!(
        enabled.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalEqParameters(ref controls))
            if controls == &vec![(24, 1.0)]
    ));
    assert!(plan_gateway_write(
        "device.setGlobalEqBand",
        &json!({"band": 0, "gain": 0.5}),
        None,
    )
    .is_err());

    let modes =
        plan_gateway_write("device.setModeCycle", &json!({"slots": [2, 0, 1]}), None).unwrap();
    assert!(matches!(
        modes.write,
        PlannedWrite::HidOperation(DeviceOperation::SetModeCycle(ref slots))
            if slots == &vec![2, 0, 1]
    ));
    assert!(plan_gateway_write("device.setModeCycle", &json!({"slots": [1, 1]}), None,).is_err());

    let record = plan_host_midi("device.controlLooper", &json!({"command": "record"})).unwrap();
    assert_eq!((record.controller, record.value), (53, 127));
    let routing = plan_host_midi(
        "device.controlLooper",
        &json!({"command": "routingMode", "value": 13}),
    )
    .unwrap();
    assert_eq!((routing.controller, routing.value), (61, 13));
    assert!(plan_host_midi(
        "device.controlLooper",
        &json!({"command": "routingMode", "value": 14}),
    )
    .is_err());
    assert!(plan_host_midi(
        "device.controlLooper",
        &json!({"command": "record", "value": 1}),
    )
    .is_err());

    let fixed = [
        ("open", profile::LOOPER_MENU_CONTROLLER, 0),
        ("close", profile::LOOPER_MENU_CONTROLLER, 127),
        ("duplicate", profile::LOOPER_DUPLICATE_CONTROLLER, 127),
        ("oneShot", profile::LOOPER_ONE_SHOT_CONTROLLER, 127),
        ("halfSpeed", profile::LOOPER_HALF_SPEED_CONTROLLER, 127),
        ("punch", profile::LOOPER_PUNCH_CONTROLLER, 127),
        ("record", profile::LOOPER_RECORD_CONTROLLER, 127),
        ("play", profile::LOOPER_PLAY_CONTROLLER, 127),
        ("reverse", profile::LOOPER_REVERSE_CONTROLLER, 127),
        ("undoRedo", profile::LOOPER_UNDO_REDO_CONTROLLER, 127),
    ];
    for (command, controller, value) in fixed {
        let first = plan_host_midi("device.controlLooper", &json!({"command": command})).unwrap();
        let repeated =
            plan_host_midi("device.controlLooper", &json!({"command": command})).unwrap();
        assert_eq!((first.controller, first.value), (controller, value));
        assert_eq!(repeated, first, "event commands must not be suppressed");
    }
    for (command, controller, maximum) in [
        (
            "duplicateMode",
            profile::LOOPER_DUPLICATE_MODE_CONTROLLER,
            1,
        ),
        ("quantize", profile::LOOPER_QUANTIZE_CONTROLLER, 9),
        (
            "midiClockStart",
            profile::LOOPER_MIDI_CLOCK_START_CONTROLLER,
            1,
        ),
        ("performMode", profile::LOOPER_PERFORM_MODE_CONTROLLER, 1),
        ("routingMode", profile::LOOPER_ROUTING_MODE_CONTROLLER, 13),
    ] {
        for value in [0, maximum] {
            let plan = plan_host_midi(
                "device.controlLooper",
                &json!({"command": command, "value": value}),
            )
            .unwrap();
            assert_eq!((plan.controller, plan.value), (controller, value));
        }
        for invalid in [json!(-1), json!(maximum + 1), json!(0.5), json!("0")] {
            assert!(plan_host_midi(
                "device.controlLooper",
                &json!({"command": command, "value": invalid}),
            )
            .is_err());
        }
        assert!(plan_host_midi(
            "device.controlLooper",
            &json!({"command": command, "value": null}),
        )
        .is_err());
    }
    assert!(plan_host_midi(
        "device.controlLooper",
        &json!({"command": "unsupported", "value": null}),
    )
    .is_err());
    assert_eq!(
        crate::device::generated_gateway::PERFORMANCE_MIDI_METHODS.len(),
        3
    );
    assert!(crate::device::generated_gateway::PERFORMANCE_MIDI_METHODS
        .iter()
        .all(|method| is_host_midi_method(method)));
    assert!(!is_host_midi_method("device.setTempo"));
    assert!(!is_host_midi_method("device.showTuner"));
}

#[test]
fn general_integer_planner_enforces_each_desktop_model_domain() {
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
        assert!(plan_gateway_write(
            "device.setGeneralInteger",
            &json!({"setting": setting, "value": value}),
            None,
        )
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
        assert!(plan_gateway_write(
            "device.setGeneralInteger",
            &json!({"setting": setting, "value": value}),
            None,
        )
        .is_ok());
    }
}

#[test]
fn global_eq_output_uses_physical_db_and_sparse_cortex_order() {
    let plan = plan_gateway_write(
        "device.setGlobalEqOutput",
        &json!({"level": -10.0, "out12": true, "out34": false}),
        None,
    )
    .unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalEqParameters(ref controls))
            if controls == &vec![(25, 0.25), (26, 1.0), (27, 0.0)]
    ));
    for invalid in [
        json!({"level": -20.01}),
        json!({"level": 20.01}),
        json!({"level": "0"}),
    ] {
        assert!(plan_gateway_write("device.setGlobalEqOutput", &invalid, None).is_err());
    }
    assert_eq!(
        gateway_write_readback_method("device.setGlobalEqOutput"),
        None
    );
}

#[test]
fn tempo_metronome_volume_writes_the_raw_value_behind_the_displayed_db() {
    // (dB the unit shows, raw parameter measured for it)
    for (db, raw) in [
        (-60.0, 0.0),
        (-3.4, 0.5),
        (0.0, 0.613),
        (6.3, 0.87),
        (9.0, 1.0),
    ] {
        let plan =
            plan_gateway_write("device.setTempoMetronome", &json!({"volumeDb": db}), None).unwrap();
        let PlannedWrite::HidOperation(DeviceOperation::SetTempoParameters(values)) = plan.write
        else {
            panic!("wrong operation")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0, 3);
        assert!(
            (values[0].1 - raw).abs() <= 1e-4,
            "{db} dB wrote raw {}",
            values[0].1
        );
    }
    for out_of_range in [-60.5, 9.5] {
        assert!(plan_gateway_write(
            "device.setTempoMetronome",
            &json!({"volumeDb": out_of_range}),
            None
        )
        .is_err());
    }
}

#[test]
fn tempo_metronome_preserves_signature_before_beats_and_validates_semantics() {
    let plan = plan_gateway_write(
        "device.setTempoMetronome",
        &json!({
            "timeSignature":"7/8 (2+2+3)", "ledEnabled":true, "volumeDb":-20.0,
            "running":false, "pan":0.25, "subdivision":"1/16T", "sound":"COWBELL",
            "routing":"OUT 3/4", "beats":["ACCENT 1","NORMAL","MUTE"]
        }),
        None,
    )
    .unwrap();
    let PlannedWrite::HidOperation(DeviceOperation::SetTempoParameters(values)) = plan.write else {
        panic!("wrong operation")
    };
    assert_eq!(values[0].0, 6);
    assert_eq!(
        values.iter().find(|value| value.0 == 7).unwrap().1,
        2.0 / 3.0
    );
    assert_eq!(
        values.iter().find(|value| value.0 == 10).unwrap().1,
        2.0 / 3.0
    );
    assert_eq!(
        values
            .iter()
            .rev()
            .take(3)
            .map(|value| value.0)
            .collect::<Vec<_>>(),
        vec![12, 11, 10]
    );
    // Regression: this error text previously read "OFF, MUTE, DOWN, or
    // ON" -- none of which are accepted values. The real whitelist is
    // ["NORMAL", "MUTE", "ACCENT 1", "ACCENT 2"] (see the BEATS constant
    // in responses.rs and the "NORMAL"/"MUTE"/"ACCENT 1"/"ACCENT 2"
    // values exercised elsewhere in this test).
    let invalid_beat =
        plan_gateway_write("device.setTempoMetronome", &json!({"beats":["LOUD"]}), None)
            .unwrap_err();
    assert!(invalid_beat.contains("NORMAL, MUTE, ACCENT 1, or ACCENT 2"));
    assert!(plan_gateway_write(
        "device.setTempoMetronome",
        &json!({"subdivision":"1/8"}),
        None,
    )
    .is_err());
    let unambiguous = plan_gateway_write(
        "device.setTempoMetronome",
        &json!({"subdivision":"1/32", "beats":["NORMAL"]}),
        None,
    )
    .unwrap();
    assert!(matches!(
        unambiguous.write,
        PlannedWrite::HidOperation(DeviceOperation::SetTempoParameters(ref values))
            if values == &vec![(7, 1.0), (10, 0.0)]
    ));
    assert!(plan_gateway_write(
        "device.setTempoMetronome",
        &json!({"timeSignature":"3/8", "beats":["NORMAL", "MUTE", "ACCENT 1", "ACCENT 2"]}),
        None,
    )
    .is_err());
    assert!(matches!(
        plan_gateway_write("device.setTempoMode", &json!({"mode":"GLOBAL"}), None)
            .unwrap()
            .write,
        PlannedWrite::HidOperation(DeviceOperation::SetTempoMode(true))
    ));
    assert!(plan_gateway_write("device.setTempoMode", &json!({"mode":"AUTO"}), None).is_err());
}

#[test]
fn global_tempo_composition_and_guarding_are_shared_by_native_hosts() {
    assert_eq!(
        gateway_read_followup_method("device.globalTempoSettings"),
        Some("device.presetTempoSettings")
    );
    assert_eq!(gateway_read_followup_method("device.globalEq"), None);
    let composed = compose_global_tempo_settings(
        &json!({"mode":"GLOBAL", "bpm":120, "beats":[]}),
        &json!({"bpm":96, "ledEnabled":true, "beats":["ACCENT 1"]}),
    )
    .unwrap();
    assert_eq!(composed["mode"], "GLOBAL");
    assert_eq!(composed["bpm"], 96);
    assert_eq!(composed["globalBpm"], 120);
    let params = json!({"bpm":121, "expectedMode":"GLOBAL", "expectedGlobalBpm":120});
    assert_eq!(
        gateway_write_preflight_method("device.setGlobalTempo"),
        Some("device.globalTempoSettings")
    );
    assert!(gateway_write_preflight_matches(
        "device.setGlobalTempo",
        &params,
        &composed
    ));
    // The desktop controller always sends an accepted BPM edit, even when
    // its requested value already equals the model value.
    let same_value = json!({"bpm":120, "expectedMode":"GLOBAL", "expectedGlobalBpm":120});
    assert_eq!(
        gateway_write_preflight_decision("device.setGlobalTempo", &same_value, &composed),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    // A mode that differs from the one the caller saw is stale state.
    assert!(!gateway_write_preflight_matches(
        "device.setGlobalTempo",
        &params,
        &json!({"mode":"PRESET", "globalBpm":120})
    ));
    // The frame is not refused in PRESET mode: a caller that saw PRESET and
    // the current global BPM proceeds. ESTABLISHED (CorOS 4.1.0, direct USB,
    // 2026-10-07): the unit does not ignore it there; it edits the loaded
    // preset's tempo (tempoProgramData parameter 0) and dirties the preset.
    let preset_params = json!({"bpm":121, "expectedMode":"PRESET", "expectedGlobalBpm":120});
    let preset_observed = json!({"mode":"PRESET", "globalBpm":120});
    assert_eq!(
        gateway_write_preflight_decision("device.setGlobalTempo", &preset_params, &preset_observed),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.setGlobalTempo",
            &preset_params,
            &json!({"mode":"PRESET", "globalBpm":130})
        ),
        Err("write was based on stale global tempo state".to_string())
    );
    assert!(plan_gateway_write(
        "device.setGlobalTempo",
        &json!({"bpm":121, "expectedMode":"GLOBAL"}),
        None
    )
    .is_err());
    let plan = plan_gateway_write("device.setGlobalTempo", &params, None).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::SetGlobalTempo(121))
    ));
    // No readback of any kind: in PRESET mode the unit answers this frame
    // with Grid/UndoRedo/PresetDirty pushes, not a type-33 echo, so a type-33
    // poll could never have seen it (measured 2026-10-07, CorOS 4.1.0).
    assert_eq!(gateway_write_readback_method("device.setGlobalTempo"), None);
    for bpm in [39, 241] {
        assert!(plan_gateway_write(
            "device.setGlobalTempo",
            &json!({"bpm":bpm, "expectedMode":"GLOBAL", "expectedGlobalBpm":120}),
            None,
        )
        .is_err());
    }
}

#[test]
fn tempo_mode_preflight_distinguishes_noop_change_and_malformed_state() {
    assert_eq!(
        gateway_write_preflight_method("device.setTempoMode"),
        Some("device.globalTempoSettings")
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.setTempoMode",
            &json!({"mode":"GLOBAL"}),
            &json!({"mode":"GLOBAL", "globalBpm":120}),
        ),
        Ok(GatewayWritePreflightDecision::AlreadyMatched)
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.setTempoMode",
            &json!({"mode":"GLOBAL"}),
            &json!({"mode":"PRESET", "globalBpm":120}),
        ),
        Ok(GatewayWritePreflightDecision::Proceed)
    );
    for malformed in [json!({}), json!({"mode":"AUTO"}), json!({"mode":1})] {
        assert!(gateway_write_preflight_decision(
            "device.setTempoMode",
            &json!({"mode":"GLOBAL"}),
            &malformed,
        )
        .is_err());
    }
    assert!(gateway_write_preflight_decision(
        "device.setTempoMode",
        &json!({"mode":"AUTO"}),
        &json!({"mode":"GLOBAL"}),
    )
    .is_err());
}

#[test]
fn graphics_tree_tuner_meter_and_screen_drag_use_shared_plans() {
    let graphics = plan_gateway_read("device.graphicsTree", &json!({}), 7).unwrap();
    assert!(matches!(
        graphics.operation,
        DeviceOperation::ReadGraphicsTree
    ));
    assert_eq!(graphics.response_type, 72);
    assert_eq!(graphics.timeout_ms, 5_000);
    assert_eq!(graphics.projection.expected_request_id(), None);
    let frames = graphics.operation.try_encode().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].message_type, 72);
    assert_eq!(frames[0].payload, [0x08, 0x03, 0x2a, 0x00]);
    assert!(matches!(
        graphics.projection,
        GatewayResponseProjection::GraphicsTree
    ));

    assert!(plan_gateway_write(
        "device.setTunerMeter",
        &json!({"enabled":true, "confirmTunerActivation":false}),
        None,
    )
    .is_err());
    assert!(matches!(
        plan_gateway_write(
            "device.setTunerMeter",
            &json!({"enabled":true, "confirmTunerActivation":true}),
            None,
        )
        .unwrap()
        .write,
        PlannedWrite::HidOperation(DeviceOperation::SetTunerMeter(true))
    ));

    let swipe = plan_gateway_write(
        "device.swipeScreen",
        &json!({"x":10, "y":20, "toX":30, "toY":40}),
        None,
    )
    .unwrap();
    assert!(matches!(
        swipe.write,
        PlannedWrite::HidOperation(DeviceOperation::ScreenDrag {
            x: 10.0,
            y: 20.0,
            to_x: 30.0,
            to_y: 40.0
        })
    ));
    // One DRAG frame, so nothing is paced.
    assert_eq!(swipe.write.inter_message_interval_ms(), 0);
    assert!(plan_gateway_write(
        "device.swipeScreen",
        &json!({"x":10, "y":20, "toX":10, "toY":20}),
        None,
    )
    .is_err());
}

#[test]
fn swipe_screen_requires_integer_finite_distinct_800_by_480_pixels_and_fresh_capture() {
    let plan = plan_gateway_write(
        "device.swipeScreen",
        &json!({"x":0, "y":0, "toX":799, "toY":479}),
        None,
    )
    .unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::ScreenDrag {
            x: 0.0,
            y: 0.0,
            to_x: 799.0,
            to_y: 479.0
        })
    ));
    let policy = gateway_write_verification_policy("device.swipeScreen");
    assert_eq!(policy.preflight_method, None);
    assert_eq!(policy.readback_method, None);
    assert_eq!(plan.verification, GatewayVerification::None);
    // A swipe is exactly one DRAG frame with no begin or end frame, so there
    // is no inter-frame interval and the dispatch budget carries no pacing.
    assert_eq!(plan.write.inter_message_interval_ms(), 0);
    let PlannedWrite::HidOperation(swipe_operation) = &plan.write else {
        panic!("a swipe is a HID operation");
    };
    assert_eq!(swipe_operation.clone().try_encode().unwrap().len(), 1);
    assert_eq!(
        planned_write_dispatch_budget_ms(&plan.write).unwrap(),
        profile::COMMAND_CONFIRMATION_TIMEOUT_MS
    );
    assert_eq!(
        gateway_write_preflight_decision(
            "device.swipeScreen",
            &json!({"x":0, "y":0, "toX":799, "toY":479}),
            &json!({"payload":"fresh-screen"}),
        ),
        Ok(GatewayWritePreflightDecision::Proceed)
    );

    for invalid in [
        json!({"x":0.5, "y":0, "toX":1, "toY":1}),
        json!({"x":-1, "y":0, "toX":1, "toY":1}),
        json!({"x":800, "y":0, "toX":1, "toY":1}),
        json!({"x":0, "y":480, "toX":1, "toY":1}),
        json!({"x":0, "y":0, "toX":799.25, "toY":479}),
        json!({"x":0, "y":0, "toX":799, "toY":480}),
        json!({"x":12, "y":34, "toX":12, "toY":34}),
    ] {
        assert!(plan_gateway_write("device.swipeScreen", &invalid, None).is_err());
    }
}

#[test]
fn mouse_frame_plans_one_raw_frame_of_each_type_with_drag_only_destinations() {
    use crate::commands::ScreenMouseType;
    for (name, kind) in [
        ("press", ScreenMouseType::Press),
        ("release", ScreenMouseType::Release),
        ("move", ScreenMouseType::Move),
        ("tap", ScreenMouseType::Tap),
    ] {
        // A null destination is the same as an absent one: the TypeScript client and
        // the MCP layer send null for the fields a non-drag frame does not use.
        for params in [
            json!({"type": name, "x": 10, "y": 20}),
            json!({"type": name, "x": 10, "y": 20, "toX": null, "toY": null}),
        ] {
            let plan = plan_gateway_write("device.mouseFrame", &params, None).unwrap();
            assert_eq!(
                plan.write,
                PlannedWrite::HidOperation(DeviceOperation::ScreenMouseFrame {
                    kind,
                    x: 10.0,
                    y: 20.0,
                    to: None,
                })
            );
            assert_eq!(plan.verification, GatewayVerification::None);
            // One frame, nothing to pace, no preflight.
            assert_eq!(plan.write.inter_message_interval_ms(), 0);
            let PlannedWrite::HidOperation(operation) = &plan.write else {
                panic!("a raw frame is a HID operation");
            };
            assert_eq!(operation.clone().try_encode().unwrap().len(), 1);
            assert!(plan.detail.contains(name));
            // Any destination at all is refused for a non-drag frame, never ignored.
            let mut with_destination = params.clone();
            with_destination["toX"] = json!(30);
            with_destination["toY"] = json!(40);
            assert!(
                plan_gateway_write("device.mouseFrame", &with_destination, None).is_err(),
                "{name} must refuse toX/toY"
            );
        }
    }
    let drag = plan_gateway_write(
        "device.mouseFrame",
        &json!({"type": "drag", "x": 10, "y": 20, "toX": 30, "toY": 40}),
        None,
    )
    .unwrap();
    assert_eq!(
        drag.write,
        PlannedWrite::HidOperation(DeviceOperation::ScreenMouseFrame {
            kind: ScreenMouseType::Drag,
            x: 10.0,
            y: 20.0,
            to: Some((30.0, 40.0)),
        })
    );
    assert_eq!(gateway_write_preflight_method("device.mouseFrame"), None);
    let budget = planned_write_dispatch_budget_ms(&drag.write).unwrap();
    assert_eq!(budget, profile::COMMAND_CONFIRMATION_TIMEOUT_MS);
}

#[test]
fn mouse_frame_fails_closed_on_bad_type_pixels_or_destinations() {
    for invalid in [
        json!({"x": 10, "y": 20}),
        json!({"type": null, "x": 10, "y": 20}),
        json!({"type": "PRESS", "x": 10, "y": 20}),
        json!({"type": "touch", "x": 10, "y": 20}),
        json!({"type": 0, "x": 10, "y": 20}),
        json!({"type": "tap", "x": 800, "y": 20}),
        json!({"type": "tap", "x": 10, "y": 480}),
        json!({"type": "tap", "x": 10.5, "y": 20}),
        json!({"type": "tap", "x": -1, "y": 20}),
        json!({"type": "tap", "x": "10", "y": 20}),
        json!({"type": "tap", "y": 20}),
        // A drag needs both destination fields, on screen, and a different pixel.
        json!({"type": "drag", "x": 10, "y": 20}),
        json!({"type": "drag", "x": 10, "y": 20, "toX": 30}),
        json!({"type": "drag", "x": 10, "y": 20, "toX": null, "toY": null}),
        json!({"type": "drag", "x": 10, "y": 20, "toX": 800, "toY": 40}),
        json!({"type": "drag", "x": 10, "y": 20, "toX": 30, "toY": 480}),
        json!({"type": "drag", "x": 10, "y": 20, "toX": 10, "toY": 20}),
        json!({"type": "drag", "x": 10, "y": 20, "toX": 30.5, "toY": 40}),
    ] {
        assert!(
            plan_gateway_write("device.mouseFrame", &invalid, None).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn tap_screen_plans_the_single_tap_frame_and_long_tap_plans_press_wait_release() {
    let tap = plan_gateway_write("device.tapScreen", &json!({"x": 10, "y": 20}), None).unwrap();
    let PlannedWrite::HidOperation(tap_operation) = &tap.write else {
        panic!("a tap is a HID operation");
    };
    let frames = tap_operation.clone().try_encode().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames,
        vec![crate::commands::screen_mouse_frame(
            crate::commands::ScreenMouseType::Tap,
            10.0,
            20.0,
            None
        )]
    );
    let hold =
        plan_gateway_write("device.longTapScreen", &json!({"x": 10, "y": 20}), None).unwrap();
    let PlannedWrite::HidOperation(hold_operation) = &hold.write else {
        panic!("a hold is a HID operation");
    };
    // Touch-down (PRESS), the wait, then touch-up (RELEASE): never the other way round.
    assert_eq!(
        hold_operation.clone().try_encode().unwrap(),
        vec![
            crate::commands::screen_mouse_frame(
                crate::commands::ScreenMouseType::Press,
                10.0,
                20.0,
                None
            ),
            crate::commands::screen_mouse_frame(
                crate::commands::ScreenMouseType::Release,
                10.0,
                20.0,
                None
            ),
        ]
    );
    assert_eq!(
        hold.write.inter_message_interval_ms(),
        profile::REMOTE_HOLD_INTERVAL_MS
    );
}

#[test]
fn drag_screen_plans_the_same_single_drag_frame_as_swipe_screen_and_ignores_the_stepped_options() {
    let defaulted = plan_gateway_write(
        "device.dragScreen",
        &json!({"x":400, "y":100, "toX":400, "toY":300}),
        None,
    )
    .unwrap();
    // One DRAG frame, exactly what the swipe plans: no PRESS, no MOVE stream,
    // no RELEASE, no pacing. MOVE had no observed effect on CorOS 4.1.0
    // (2026-10-06), so the old stepped stream was not the unit's drag.
    assert!(matches!(
        defaulted.write,
        PlannedWrite::HidOperation(DeviceOperation::ScreenDrag {
            x: 400.0,
            y: 100.0,
            to_x: 400.0,
            to_y: 300.0,
        })
    ));
    assert_eq!(defaulted.verification, GatewayVerification::None);
    assert_eq!(defaulted.write.inter_message_interval_ms(), 0);
    let PlannedWrite::HidOperation(operation) = &defaulted.write else {
        panic!("a drag is a HID operation");
    };
    assert_eq!(operation_inter_message_interval_ms(operation), 0);
    assert_eq!(operation.abort_release_frame(), None);
    let frames = operation.clone().try_encode().unwrap();
    assert_eq!(
        frames,
        vec![crate::commands::screen_mouse_frame(
            crate::commands::ScreenMouseType::Drag,
            400.0,
            100.0,
            Some((400.0, 300.0)),
        )]
    );
    assert!(defaulted.detail.contains("not verified"));
    assert!(defaulted.detail.contains("ignored"));

    // Explicit steps and holdMs are accepted (inclusive bounds) and change
    // nothing: the plan is the same one frame.
    for (steps, hold_ms) in [(2, 0), (40, 1500), (18, 200), (2, 1500)] {
        let explicit = plan_gateway_write(
            "device.dragScreen",
            &json!({"x":400, "y":100, "toX":400, "toY":300, "steps":steps, "holdMs":hold_ms}),
            None,
        )
        .unwrap();
        assert_eq!(explicit.write, defaulted.write);
    }

    // And it is the swipe's plan, frame for frame.
    let swipe = plan_gateway_write(
        "device.swipeScreen",
        &json!({"x":400, "y":100, "toX":400, "toY":300}),
        None,
    )
    .unwrap();
    assert_eq!(swipe.write, defaulted.write);
}

#[test]
fn drag_screen_fails_closed_on_out_of_range_or_malformed_input() {
    let base = json!({"x":10, "y":20, "toX":30, "toY":40});
    let with = |field: &str, value: Value| {
        let mut params = base.clone();
        params[field] = value;
        params
    };
    for invalid in [
        json!({"x":0.5, "y":0, "toX":1, "toY":1}),
        json!({"x":-1, "y":0, "toX":1, "toY":1}),
        json!({"x":800, "y":0, "toX":1, "toY":1}),
        json!({"x":0, "y":480, "toX":1, "toY":1}),
        json!({"x":0, "y":0, "toX":799.25, "toY":479}),
        json!({"x":0, "y":0, "toX":799, "toY":480}),
        json!({"x":12, "y":34, "toX":12, "toY":34}),
        json!({"x":12, "y":34, "toX":12}),
        with("steps", json!(1)),
        with("steps", json!(0)),
        with("steps", json!(41)),
        with("steps", json!(-3)),
        with("steps", json!(18.5)),
        with("steps", json!("18")),
        with("steps", Value::Null),
        with("holdMs", json!(1501)),
        with("holdMs", json!(-1)),
        with("holdMs", json!(200.5)),
        with("holdMs", json!("200")),
        with("holdMs", Value::Null),
        with("holdMs", json!(u64::MAX)),
    ] {
        assert!(
            plan_gateway_write("device.dragScreen", &invalid, None).is_err(),
            "{invalid} must be refused"
        );
    }
    // Boundaries are inclusive and nothing is clamped.
    for (steps, hold_ms) in [(2, 0), (40, 1500), (2, 1500), (40, 0)] {
        let mut params = base.clone();
        params["steps"] = json!(steps);
        params["holdMs"] = json!(hold_ms);
        assert!(plan_gateway_write("device.dragScreen", &params, None).is_ok());
    }
    let message =
        plan_gateway_write("device.dragScreen", &with("steps", json!(41)), None).unwrap_err();
    assert!(message.contains("steps") && message.contains("2 through 40"));
}

#[test]
fn backup_validation_and_naming_are_shared_by_native_hosts() {
    assert_eq!(
        validate_backup_name(&"x".repeat(64)).unwrap(),
        "x".repeat(64)
    );
    assert!(validate_backup_name(&"x".repeat(65)).is_err());
    assert!(validate_backup_name("bad\nname").is_err());
    assert!(validate_backup_name("\u{007f}").is_err());

    let backup = finalize_device_backup(
            r#"{"type":"backup","creator":"quad","name":"device","payload":"AQID","payload_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
            "Tour Backup",
        )
        .unwrap();
    assert_eq!(backup["name"], "Tour Backup");
    assert!(finalize_device_backup("{}", "Tour").is_err());
    assert!(finalize_device_backup("not json", "Tour").is_err());
    let wrapper = |payload: &str, hash: &str| {
        format!(
            r#"{{"type":"backup","creator":"quad","payload":"{payload}","payload_hash":"{hash}"}}"#
        )
    };
    let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert!(finalize_device_backup(&wrapper("AQID", hash), "\n").is_err());
    assert!(finalize_device_backup(&wrapper("AQID", hash), &"x".repeat(65)).is_err());
    assert!(finalize_device_backup(&wrapper("%%%", hash), "Tour").is_err());
    assert!(finalize_device_backup(&wrapper("AQID", "not-a-hash"), "Tour").is_err());
    assert!(finalize_device_backup(
            r#"{"type":"backup","creator":"quad","payload_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
            "Tour",
        )
        .is_err());
}

#[test]
fn user_supplied_names_and_library_keys_reject_path_injection() {
    for method in ["device.createSetlist", "device.deleteSetlist"] {
        for name in ["../Tour", "Tour/Encore", "Tour\\Encore", "My Presets"] {
            assert!(plan_gateway_write(method, &json!({"name": name}), None).is_err());
        }
    }

    let deletion = plan_gateway_write("device.deleteSetlist", &json!({"name": "Tour"}), None)
        .expect("valid deletion plan");
    assert_eq!(deletion.verification, GatewayVerification::None);
    assert!(matches!(
        deletion.write,
        PlannedWrite::HidOperation(DeviceOperation::DeleteSetlist { ref name })
            if name == "Tour"
    ));

    for name in ["../Stage", "Stage/Lead", "Stage\nLead"] {
        assert!(operation(
            "deletePreset",
            &json!({"setlistKey": "/media/p4/Presets/Live", "name": name}),
        )
        .is_err());
    }

    for key in [
        "/media/p4/Presets/../Factory",
        "/media/p4/Presets/Live/Child",
        "/media/p4/Presets/Live\\Child",
    ] {
        assert!(operation(
            "movePreset",
            &json!({
                "setlistKey": key,
                "productKey": "device-product-key-42",
                "destinationPosition": 1
            }),
        )
        .is_err());
    }

    assert!(operation(
        "loadCapture",
        &json!({"row": 0, "column": 0, "key": "capture/../", "name": "Lead"}),
    )
    .is_err());
    assert!(plan_gateway_read("device.irs", &json!({"folder": "factory/../private"}), 1,).is_err());
    assert!(plan_gateway_write(
        "device.setDeviceName",
        &json!({"name": "x".repeat(65)}),
        None,
    )
    .is_err());
}

#[test]
fn output_port_settings_follow_the_expected_model_and_control_matrix() {
    for output_port_id in [1, 2, 4, 5, 6, 7, 8, 9] {
        assert!(operation(
            "setOutputPort",
            &json!({"outputPortId": output_port_id, "level": -40.0}),
        )
        .is_ok());
        assert!(operation(
            "setOutputPort",
            &json!({"outputPortId": output_port_id, "level": 12.0}),
        )
        .is_ok());
    }
    for output_port_id in [0, 3, 10, 19, 22, 23] {
        assert!(operation(
            "setOutputPort",
            &json!({"outputPortId": output_port_id, "level": 0.0}),
        )
        .is_err());
    }
    for level in [-40.001, 12.001, f64::NAN, f64::INFINITY] {
        assert!(operation("setOutputPort", &json!({"outputPortId": 1, "level": level}),).is_err());
    }
    for ground_lift in [0.25, -1.0, 2.0] {
        assert!(operation(
            "setOutputPort",
            &json!({"outputPortId": 1, "groundLift": ground_lift}),
        )
        .is_err());
    }
    assert!(operation(
        "setOutputPort",
        &json!({"outputPortId": 2, "groundLift": 1.0}),
    )
    .is_err());
    assert!(operation("setOutputPort", &json!({"outputPortId": 8, "mute": true}),).is_err());
}

#[test]
fn saved_preset_collision_names_retain_the_complete_requested_name() {
    assert!(stored_preset_name_matches("Crying Wah", "Crying Wah"));
    assert!(stored_preset_name_matches("Crying Wah", "Crying Wah_1"));
    assert!(stored_preset_name_matches("Crying Wah", "Crying Wah_27"));
    assert!(stored_preset_name_matches("Scratch_1", "Scratch_2"));
    assert!(stored_preset_name_matches("Amp_2", "Amp_3"));
    assert!(stored_preset_name_matches("Amp_1", "Amp_9"));
    assert!(stored_preset_name_matches("Amp_9", "Amp_10"));
    assert!(stored_preset_name_matches(
        "A_99999999999999999",
        "A_100000000000000000"
    ));
    assert!(!stored_preset_name_matches(
        "Crying Wah Plus",
        "Crying Wah_1"
    ));
    assert!(!stored_preset_name_matches("Crying Wah", "Crying Wah copy"));
    assert!(!stored_preset_name_matches("Crying Wah", "Crying Wah_"));
    assert!(!stored_preset_name_matches("Crying Wah", "Crying Wah_x"));
    assert!(!stored_preset_name_matches("Amp_2", "Amp_1"));
    assert!(!stored_preset_name_matches("Scratch_1", "Other_2"));
    assert!(!stored_preset_name_matches("Scratch_1", "scratch_2"));
    assert!(!stored_preset_name_matches("Amp_01", "Amp_02"));
    assert!(!stored_preset_name_matches("Scratch_1", "Scratch_2 copy"));
    assert!(stored_preset_name_matches(
        "123456789012345678901",
        "123456789012345678_2"
    ));
    assert!(!stored_preset_name_matches(
        "123456789012345678901",
        "12345678901234567_2"
    ));
    assert!(stored_preset_name_matches(
        "123456789012345678_9",
        "12345678901234567_10"
    ));
    assert!(!stored_preset_name_matches(
        "123456789012345678_01",
        "12345678901234567_02"
    ));
}

#[test]
fn authoritative_saved_entry_reconciles_only_the_active_slot_name() {
    let mut snapshot = GatewaySnapshot {
        setlist_key: "/media/p4/Presets/My Presets".into(),
        preset_position: 15,
        preset_name: "Old".into(),
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    assert!(reconcile_saved_preset_snapshot(
        &mut snapshot,
        "/media/p4/Presets/My Presets/",
        15,
        "Renamed"
    ));
    assert_eq!(snapshot.preset_name, "Renamed");
    assert!(!reconcile_saved_preset_snapshot(
        &mut snapshot,
        "/media/p4/Presets/Other",
        15,
        "Wrong"
    ));
    assert_eq!(snapshot.preset_name, "Renamed");
}

#[test]
fn usb_port_planning_rejects_values_between_enum_ordinals() {
    for headphones_source in [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0] {
        assert!(operation(
            "setUsbPort",
            &json!({"level": null, "headphonesSource": headphones_source, "dryWet": null}),
        )
        .is_ok());
    }
    assert!(operation(
        "setUsbPort",
        &json!({"level": null, "headphonesSource": 0.5, "dryWet": null}),
    )
    .is_err());
    assert!(operation(
        "setUsbPort",
        &json!({"level": null, "headphonesSource": null, "dryWet": 0.5}),
    )
    .is_err());
    assert!(operation(
        "setUsbPort",
        &json!({"level": 0.25, "headphonesSource": null, "dryWet": 1.0}),
    )
    .is_ok());
}
#[test]
fn setlist_deletion_plan_requires_one_canonical_nonfactory_target() {
    let before = GatewaySnapshot {
        has_preset: true,
        setlist_key: "/media/p4/Presets/Existing".into(),
        ..Default::default()
    };
    let listing = |key: &str, name: &str, factory| crate::state::PresetFolderListing {
        order: None,
        key: key.into(),
        name: name.into(),
        is_factory: factory,
        is_plugin: false,
        file_type: Some(0),
        files: vec![],
    };
    let target = "/media/p4/Presets/Delete";
    for folders in [
        vec![],
        vec![listing(target, "Delete", true)],
        vec![listing("/media/p4/Presets/Wrong", "Delete", false)],
        vec![
            listing(target, "Delete", false),
            listing("/media/p4/Presets/Alias", "Delete", false),
        ],
    ] {
        let mut library = PresetLibrary::default();
        for folder in folders {
            library.ingest(folder);
        }
        assert!(plan_preset_mutation(
            "device.deleteSetlist",
            &json!({"name":"Delete"}),
            Some(&before),
            &library
        )
        .is_err());
    }
    let mut library = PresetLibrary::default();
    library.ingest(listing(target, "Delete", false));
    let partial = plan_preset_mutation(
        "device.deleteSetlist",
        &json!({"name":"Delete"}),
        Some(&before),
        &library,
    )
    .unwrap_err();
    assert!(partial.contains("known active setlist"));
    library.ingest(listing(&before.setlist_key, "Existing", false));
    let plan = plan_preset_mutation(
        "device.deleteSetlist",
        &json!({"name":"Delete"}),
        Some(&before),
        &library,
    )
    .unwrap();
    assert_eq!(plan.stages.len(), 1);
    assert!(
        matches!(&plan.stages[0].write, PlannedWrite::HidOperation(DeviceOperation::DeleteSetlist { name }) if name == "Delete")
    );
    assert_eq!(
        plan.setlist_deletion.unwrap().setlist_key,
        "/media/p4/Presets/Delete"
    );
    assert!(plan.cleanup_stages.is_empty());
}

#[test]
fn planner_midi_out_bounds_come_from_the_shared_definition() {
    let midi_types: Vec<_> = crate::commands::MIDI_OUT_TYPES.collect();
    assert_eq!(midi_types.len(), 3);
    let message = |r#type: u64, channel: u64| json!({"type": r#type, "channel": channel, "param1": 0, "param2": 0, "param3": 0});
    for method in ["device.setMidiOut", "device.setPresetLoadMidiOut"] {
        let refusal = |messages: Value| {
            operation(method, &json!({"source": 0, "messages": messages}))
                .expect_err("an undefined MIDI Out message is refused")
        };
        assert_eq!(
            refusal(json!([message(4, 1)])),
            "messages[0].type must be 0 through 3"
        );
        assert_eq!(
            refusal(json!([message(1, 17)])),
            "messages[0].channel must be 1 through 16"
        );
        for &r#type in &midi_types {
            assert!(operation(
                method,
                &json!({"source": 9, "messages": [message(u64::from(r#type), 16)]})
            )
            .is_ok());
        }
    }
    assert!(operation("device.setMidiOut", &json!({"source": 10, "messages": []})).is_err());
}

#[test]
fn save_preset_instrument_is_limited_to_the_defined_codes() {
    let save = |instrument: Option<Value>| {
        let mut params = json!({
            "setlistKey": "/media/p4/Presets/My Presets",
            "position": 3,
            "name": "Saved"
        });
        if let Some(instrument) = instrument {
            params["instrument"] = instrument;
        }
        operation("savePreset", &params)
    };
    let instrument = |result: Result<DeviceOperation, String>| match result {
        Ok(DeviceOperation::SavePreset { instrument, .. }) => instrument,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(instrument(save(None)), 0);
    assert_eq!(instrument(save(Some(Value::Null))), 0);
    let instruments: Vec<_> = crate::commands::PRESET_INSTRUMENTS.collect();
    assert_eq!(instruments.len(), 6);
    for code in instruments {
        assert_eq!(instrument(save(Some(json!(code)))), code);
    }
    for refused in [
        json!(-1),
        json!(6),
        json!(i64::MAX),
        json!("Guitar"),
        json!(1.5),
    ] {
        assert_eq!(
            save(Some(refused.clone())).expect_err("refused"),
            "instrument must be an integer from 0 through 5",
            "{refused}"
        );
    }
}
#[test]
fn planner_literal_methods_belong_to_the_generated_gateway_contract() {
    let source = [
        include_str!("request.rs"),
        include_str!("request/host_midi.rs"),
        include_str!("request/backup.rs"),
    ]
    .join("\n");
    let production = source.split("#[cfg(test)]").next().unwrap();
    let methods: Vec<_> = production
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|text| {
            (text.starts_with("device.") || text.starts_with("system."))
                && !text.ends_with('.')
                && text
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '.')
        })
        .collect();
    // Keep the source scan non-vacuous when planner methods move between modules.
    assert!(methods.len() >= 50);
    for method in methods {
        // This read is the planner's private follow-up for global tempo;
        // it must not become a public gateway RPC.
        assert!(
            super::super::generated_gateway::METHODS.contains(&method)
                || method == "device.presetTempoSettings",
            "undeclared planner method: {method}"
        );
    }
}

#[test]
fn scene_color_verification_requires_the_exact_argb_including_alpha() {
    let verification = GatewayVerification::SceneColor {
        scene: 0,
        color: 0xff12_3456,
    };
    let mut snapshot = GatewaySnapshot {
        scene_colors: Some(vec!["#123456".into()]),
        scene_colors_argb: Some(vec![0x0012_3456]),
        ..Default::default()
    };
    assert!(!verification.matches(&snapshot, None));
    snapshot.scene_colors_argb = Some(vec![0xff12_3456]);
    assert!(verification.matches(&snapshot, None));
    snapshot.scene_colors_argb = None;
    assert!(!verification.matches(&snapshot, None));
}

#[test]
fn footswitch_noop_matches_only_an_encoder_transition_that_sends_nothing() {
    let make = |next, previous, prior_type| GatewayWritePlan {
        write: PlannedWrite::HidOperation(DeviceOperation::SetFootswitch {
            row: 0,
            column: 0,
            footswitch: next,
            assignment_type: StompAssignmentType::Primary,
            previous_footswitch: previous,
            previous_assignment_type: prior_type,
        }),
        detail: "fixture".into(),
        verification: GatewayVerification::None,
    };
    let same = make(Some(2), Some(2), Some(StompAssignmentType::Primary));
    assert!(same.is_noop());
    let PlannedWrite::HidOperation(operation) = &same.write else {
        panic!("HID fixture")
    };
    assert!(operation.clone().try_encode().unwrap().is_empty());
    assert!(make(None, None, None).is_noop());
    assert!(!make(Some(2), Some(2), Some(StompAssignmentType::Secondary)).is_noop());
    assert!(!make(Some(3), Some(2), Some(StompAssignmentType::Primary)).is_noop());
    assert!(!make(None, Some(2), Some(StompAssignmentType::Primary)).is_noop());
}

#[test]
fn settings_echo_cannot_borrow_the_requested_field_from_retained_state() {
    let params = json!({"setting": "midiChannel", "value": 1});
    let unrelated = pa::GeneralSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        available_disk_space: Some(
            pa::general_settings_message::AvailableDiskSpace::AvailableDiskSpace(99.0),
        ),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(gateway_write_readback_matches(
        "device.setGeneralInteger",
        &params,
        &json!({"midiChannel": 1})
    ));
    assert!(!gateway_write_echo_matches(
        "device.setGeneralInteger",
        &params,
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        &unrelated
    ));
    let relevant = pa::GeneralSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        midi_channel: Some(pa::general_settings_message::MidiChannel::MidiChannel(1)),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(gateway_write_echo_matches(
        "device.setGeneralInteger",
        &params,
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        &relevant
    ));
    assert!(!gateway_write_echo_matches(
        "device.setGeneralInteger",
        &params,
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &relevant
    ));
    assert!(!gateway_write_echo_matches(
        "device.setLooperStompAssignments",
        &json!({"assignments": []}),
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        &unrelated
    ));
}

#[test]
fn output_only_echo_proves_its_own_field_but_is_not_a_complete_io_read() {
    let payload = pa::IoSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        settings: Some(pa::io_settings_message::Settings::Settings(
            pa::PortSettings {
                out_port: vec![pa::OutputPortSettings {
                    output_port_id: 1,
                    mute: Some(pa::output_port_settings::Mute::Mute(true)),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(decode_io_settings(&payload).is_err());
    assert!(gateway_write_echo_matches(
        "device.setOutputPort",
        &json!({"outputPortId": 1, "mute": true}),
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &payload
    ));
    assert!(!gateway_write_echo_matches(
        "device.setOutputPort",
        &json!({"outputPortId": 1, "mute": true, "level": 0}),
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &payload
    ));
    assert!(!gateway_write_echo_matches(
        "device.setOutputPort",
        &json!({"outputPortId": 2, "mute": true}),
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &payload
    ));
    let pairing = pa::IoSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        xlr1_2_linked: Some(pa::io_settings_message::Xlr12Linked::Xlr12Linked(true)),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(gateway_write_echo_matches(
        "device.setOutputPairing",
        &json!({"xlr12Linked": true}),
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &pairing
    ));
    assert!(!gateway_write_echo_matches(
        "device.setMidiThru",
        &json!({"enabled": true}),
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &pairing
    ));
}

#[test]
fn a_sparse_echo_with_duplicate_port_addresses_cannot_verify_a_write() {
    let payload = pa::IoSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        settings: Some(pa::io_settings_message::Settings::Settings(
            pa::PortSettings {
                out_port: vec![
                    pa::OutputPortSettings {
                        output_port_id: 1,
                        mute: Some(pa::output_port_settings::Mute::Mute(true)),
                        ..Default::default()
                    },
                    pa::OutputPortSettings {
                        output_port_id: 1,
                        mute: Some(pa::output_port_settings::Mute::Mute(false)),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        )),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(!gateway_write_echo_matches(
        "device.setOutputPort",
        &json!({"outputPortId": 1, "mute": true}),
        profile::MESSAGE_TYPE_IO_SETTINGS,
        &payload
    ));
    assert!(!gateway_write_readback_matches(
        "device.setInputPort",
        &json!({"inputPortId": 1, "groundLift": 1.0}),
        &json!({"inputs": [
            {"inputPortId": 1, "groundLift": 1.0}, {"inputPortId": 1, "groundLift": 0.0}
        ]})
    ));
}

// Recall-proof fixtures. A type-15 body carries no address and no device identity, so these
// feed the real reducer the exact wire frames a stale-body sequence delivers.
fn recall_proof_position(setlist_key: &str, position: u32) -> Vec<u8> {
    pa::SetlistPositionMessage {
        action: pa::message_action::Enum::Update as i32,
        folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
            setlist_key.into(),
        )),
        position: Some(pa::setlist_position_message::Position::Position(position)),
        ..Default::default()
    }
    .encode_to_vec()
}

fn recall_proof_body(name: &str, model_id: u32) -> Vec<u8> {
    pa::RecallPresetMessage {
        action: pa::message_action::Enum::Update as i32,
        preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
            name: Some(binary_preset::Name::Name(name.into())),
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![Model {
                    hash: Some(model::Hash::Hash(model_id)),
                    column: Some(model::Column::Column(0)),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        })),
        ..Default::default()
    }
    .encode_to_vec()
}

fn recall_proof_dirty(dirty: bool) -> Vec<u8> {
    pa::PresetDirtyMessage {
        action: pa::message_action::Enum::Update as i32,
        is_dirty: dirty,
        ..Default::default()
    }
    .encode_to_vec()
}

/// Feeds `frames` (SetlistPosition, preset body or dirty flag) through the real reducer.
fn recall_proof_ingest(
    state: &mut crate::device::state_runtime::DeviceStateRuntime,
    frames: &[(u16, Vec<u8>)],
) {
    for (message_type, payload) in frames {
        state.ingest(*message_type, payload).unwrap();
    }
}

/// Preset A ("Preset A", model 41) at /user/live slot 8, clean.
fn recall_proof_preset_a_at_8() -> crate::device::state_runtime::DeviceStateRuntime {
    let mut state = crate::device::state_runtime::DeviceStateRuntime::new();
    recall_proof_ingest(
        &mut state,
        &[
            (
                profile::MESSAGE_TYPE_SETLIST_POSITION,
                recall_proof_position("/user/live", 8),
            ),
            (
                profile::MESSAGE_TYPE_RECALL_PRESET,
                recall_proof_body("Preset A", 41),
            ),
            (
                profile::MESSAGE_TYPE_PRESET_DIRTY,
                recall_proof_dirty(false),
            ),
        ],
    );
    assert_eq!(state.snapshot().preset_position, 8);
    assert_eq!(state.snapshot().preset_name, "Preset A");
    state
}

/// Asserts `result` is the honest address-only outcome for `position`, valid for `method`.
fn assert_address_observed_only(method: &str, result: &Value, position: u32) {
    assert_eq!(result["accepted"], true, "{method}: {result}");
    assert_eq!(result["verified"], false, "{method}: {result}");
    assert_eq!(result["verification"], "accepted_unverified", "{method}");
    assert_ne!(result["verification"], "authoritative_readback", "{method}");
    assert!(
        result.get("changed").is_none(),
        "{method}: an unbound body cannot say whether the preset changed"
    );
    let observation = &result["observation"];
    assert_eq!(observation["kind"], PRESET_ADDRESS_OBSERVED, "{method}");
    assert_eq!(observation["setlistKey"], "/user/live", "{method}");
    assert_eq!(observation["position"], position, "{method}");
    assert_eq!(observation["address"], "observed", "{method}");
    assert_eq!(observation["body"], "unbound", "{method}");
    assert_eq!(observation["causallyCorrelated"], false, "{method}");
    let detail = result["detail"].as_str().unwrap();
    assert!(
        detail.contains(&format!("/user/live position {position}")),
        "{method}: the observed address is reported: {detail}"
    );
    assert!(
        !detail.contains("Preset A") && !detail.contains("Preset B"),
        "{method}: no body-derived name is presented as the loaded preset: {detail}"
    );
    crate::device::generated_gateway::validate_result(method, result)
        .unwrap_or_else(|error| panic!("{method} result must satisfy its contract: {error}"));
}

#[test]
fn a_recall_or_bank_change_answered_by_the_old_body_reports_the_address_not_the_body() {
    // (a) and (d). Preset A at slot 8; the host recalls slot 9 (or pages a bank to 16). The
    // QC's SetlistPosition for the destination arrives, then A's OLD full body (a late reply
    // to an earlier bare read, or a large publication completing late). The address predicate
    // holds, so the broker used to return verified:true, authoritative_readback, with a
    // snapshot claiming the destination holds Preset A (model 41). The result must report
    // the observed address and leave the body unbound.
    for (method, params, target) in [
        (
            "device.recallPreset",
            json!({"setlistKey": "/user/live", "position": 9,
                "expectedPresetName": "Preset A", "expectedPosition": 8,
                "expectedSetlistKey": "/user/live"}),
            9,
        ),
        (
            "device.navigateBank",
            json!({"direction": 1, "expectedPresetName": "Preset A", "expectedPosition": 8}),
            16,
        ),
    ] {
        let mut state = recall_proof_preset_a_at_8();
        let plan = plan_preset_recall(method, &params, Some(state.snapshot())).unwrap();
        assert_eq!(plan.position, target, "{method}");
        recall_proof_ingest(
            &mut state,
            &[
                (
                    profile::MESSAGE_TYPE_SETLIST_POSITION,
                    recall_proof_position("/user/live", target),
                ),
                (
                    profile::MESSAGE_TYPE_RECALL_PRESET,
                    recall_proof_body("Preset A", 41),
                ),
            ],
        );
        let after = state.snapshot();
        assert_eq!(after.preset_position, target, "{method}");
        assert_eq!(
            after.preset_name, "Preset A",
            "{method}: the stale body is projected"
        );
        assert_eq!(after.blocks[0].model_id, Some(41), "{method}");
        // The address predicate (what the host waits on) still holds: this is exactly the
        // state that used to be reported as an authoritative readback.
        assert!(plan.verification().matches(after, None), "{method}");
        assert!(plan.matches(after), "{method}");
        assert!(plan.matches_recovered(after), "{method}");

        let result = plan.address_observed_result(after).unwrap();
        assert_address_observed_only(method, &result, target);
        assert!(
            result.get("snapshot").is_none(),
            "{method}: the shared result carries no body; the host attaches its mirror"
        );
    }
}

#[test]
fn a_recall_answered_by_the_right_body_reports_exactly_the_same_unbound_outcome() {
    // (b). The same recall answered by B's genuine body (model 42). The host still cannot
    // prove the body is bound to slot 9: nothing in it names an address, so this sequence is
    // indistinguishable from (a) on the wire. It gets the identical outcome, not a verified
    // one; reporting it as verified would make (a) verified too.
    let params = json!({"setlistKey": "/user/live", "position": 9,
        "expectedPresetName": "Preset A", "expectedPosition": 8,
        "expectedSetlistKey": "/user/live"});
    let outcome = |name: &str, model_id: u32| {
        let mut state = recall_proof_preset_a_at_8();
        let plan =
            plan_preset_recall("device.recallPreset", &params, Some(state.snapshot())).unwrap();
        recall_proof_ingest(
            &mut state,
            &[
                (
                    profile::MESSAGE_TYPE_SETLIST_POSITION,
                    recall_proof_position("/user/live", 9),
                ),
                (
                    profile::MESSAGE_TYPE_RECALL_PRESET,
                    recall_proof_body(name, model_id),
                ),
            ],
        );
        assert!(plan.verification().matches(state.snapshot(), None));
        assert_eq!(state.snapshot().preset_name, name);
        plan.address_observed_result(state.snapshot()).unwrap()
    };
    let genuine = outcome("Preset B", 42);
    assert_address_observed_only("device.recallPreset", &genuine, 9);
    assert_eq!(
        genuine,
        outcome("Preset A", 41),
        "the outcome must not depend on a body the host cannot bind"
    );
}

#[test]
fn a_reload_answered_by_a_stale_body_and_a_clean_flag_is_not_an_authoritative_discard() {
    // (c). Preset A at slot 8 carries an unsaved edit (the working body has model 43). The
    // reload resends slot 8; the QC republishes the position and a clean flag, then the STALE
    // edited body arrives. require_clean and both revisions hold, so the
    // discard used to be reported as an authoritative readback while the projected body still carries the edit.
    let mut state = recall_proof_preset_a_at_8();
    recall_proof_ingest(
        &mut state,
        &[
            (
                profile::MESSAGE_TYPE_RECALL_PRESET,
                recall_proof_body("Preset A", 43),
            ),
            (profile::MESSAGE_TYPE_PRESET_DIRTY, recall_proof_dirty(true)),
        ],
    );
    let params = json!({"expectedPresetName": "Preset A", "expectedPosition": 8});
    let plan = plan_preset_recall("device.reloadPreset", &params, Some(state.snapshot())).unwrap();
    assert!(plan.require_clean);
    recall_proof_ingest(
        &mut state,
        &[
            (
                profile::MESSAGE_TYPE_SETLIST_POSITION,
                recall_proof_position("/user/live", 8),
            ),
            (
                profile::MESSAGE_TYPE_PRESET_DIRTY,
                recall_proof_dirty(false),
            ),
            (
                profile::MESSAGE_TYPE_RECALL_PRESET,
                recall_proof_body("Preset A", 43),
            ),
        ],
    );
    let after = state.snapshot();
    assert!(!after.dirty);
    assert_eq!(
        after.blocks[0].model_id,
        Some(43),
        "the discarded edit is projected"
    );
    assert!(plan.verification().matches(after, None));

    let result = plan.address_observed_result(after).unwrap();
    assert_address_observed_only("device.reloadPreset", &result, 8);
    assert_eq!(
        result["observation"]["dirty"], false,
        "the clean flag is reported as observed, not as proof"
    );
    assert!(result["detail"]
        .as_str()
        .unwrap()
        .contains("the discard is not verified"));
}

#[test]
fn recall_waits_failures_and_saves_keep_their_existing_outcomes() {
    // (e). The address-only outcome is reached only after the predicate matched; every other
    // branch is untouched.
    let before = recall_proof_preset_a_at_8().snapshot().clone();
    let recall = |position: u32| {
        plan_preset_recall(
            "device.recallPreset",
            &json!({"setlistKey": "/user/live", "position": position,
                "expectedPresetName": "Preset A", "expectedPosition": 8}),
            Some(&before),
        )
        .unwrap()
    };

    // A same-slot recall with no new position event never matches, so the host still reaches
    // its unchanged timeout result (accepted_unverified with the mirror) and never this one.
    let same = recall(8);
    assert!(!same.verification().matches(&before, None));
    // A same-slot recall that does observe a new position event is a recall like any other:
    // the body is no more bound to it, so it gets the address-only outcome.
    let mut same_state = recall_proof_preset_a_at_8();
    recall_proof_ingest(
        &mut same_state,
        &[
            (
                profile::MESSAGE_TYPE_SETLIST_POSITION,
                recall_proof_position("/user/live", 8),
            ),
            (
                profile::MESSAGE_TYPE_RECALL_PRESET,
                recall_proof_body("Preset A", 41),
            ),
        ],
    );
    assert!(same.verification().matches(same_state.snapshot(), None));
    assert_address_observed_only(
        "device.recallPreset",
        &same.address_observed_result(same_state.snapshot()).unwrap(),
        8,
    );

    // A recall that lands on another slot fails the predicate and matches_recovered, so the
    // host keeps returning its existing error, not an observation.
    let plan = recall(9);
    let mut elsewhere = recall_proof_preset_a_at_8();
    recall_proof_ingest(
        &mut elsewhere,
        &[
            (
                profile::MESSAGE_TYPE_SETLIST_POSITION,
                recall_proof_position("/user/live", 10),
            ),
            (
                profile::MESSAGE_TYPE_RECALL_PRESET,
                recall_proof_body("Preset A", 41),
            ),
        ],
    );
    assert!(!plan.verification().matches(elsewhere.snapshot(), None));
    assert!(!plan.matches_recovered(elsewhere.snapshot()));

    // Refusals before any frame are unchanged.
    let dirty = GatewaySnapshot {
        dirty: true,
        ..before.clone()
    };
    assert!(plan_preset_recall(
        "device.recallPreset",
        &json!({"setlistKey": "/user/live", "position": 9,
            "expectedPresetName": "Preset A", "expectedPosition": 8}),
        Some(&dirty),
    )
    .unwrap_err()
    .contains("unsaved changes"));

    // The save predicate (name, clean, no revisions) and every non-preset predicate are out of
    // scope: their hosts keep their own outcomes.
    let save = GatewayVerification::Preset {
        setlist_key: "/user/live".into(),
        position: 9,
        name: Some("Preset B".into()),
        require_clean: true,
        after_position_revision: None,
        after_full_preset_revision: None,
    };
    assert_eq!(preset_address_observed_result(&save, &before), None);
    assert_eq!(
        preset_address_observed_result(&GatewayVerification::Scene { scene: 1 }, &before),
        None
    );
    assert_eq!(
        preset_address_observed_result(&GatewayVerification::None, &before),
        None
    );
}

#[test]
fn recall_details_name_the_request_and_unverified_details_never_claim_verification() {
    // The shared rule every host asserts: a detail reported with verified:false says
    // "verified" only inside "not verified" or "unverified". The old recall wording broke it
    // on every timeout ("Preset recalled and verified; ... so the effect is not verified").
    assert!(detail_claims_verification(
        "Preset recalled and verified; the QC pushed no confirming position and preset within 2500 ms, so the effect is not verified"
    ));
    assert!(detail_claims_verification(
        "Active preset address resent and clean readback verified"
    ));
    assert!(!detail_claims_verification(
        "Preset recall requested; the effect is not verified"
    ));
    assert!(!detail_claims_verification("accepted_unverified"));

    let mut before = GatewaySnapshot {
        preset_name: "Live".into(),
        setlist_key: "/user/live".into(),
        preset_position: 8,
        position_revision: 14,
        full_preset_revision: 14,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    // The unit has reported its dirty flag (clean), as the initial-dirty guard requires.
    before.unobserved.retain(|field| *field != "dirty");
    for (method, params, detail, timeout_text) in [
        (
            "device.recallPreset",
            json!({"setlistKey": "/user/live", "position": 9,
                "expectedPresetName": "Live", "expectedPosition": 8}),
            "Preset recall requested",
            "Preset recall requested; the frame was sent, but within 2500 ms the host did not observe all of the required conditions together (a newer position update naming /user/live position 9 and a newer full preset publication), so the effect is not verified",
        ),
        (
            "device.navigateBank",
            json!({"direction": 1, "expectedPresetName": "Live", "expectedPosition": 8}),
            "Preset bank change requested",
            "Preset bank change requested; the frame was sent, but within 2500 ms the host did not observe all of the required conditions together (a newer position update naming /user/live position 16 and a newer full preset publication), so the effect is not verified",
        ),
        (
            "device.reloadPreset",
            json!({"expectedPresetName": "Live", "expectedPosition": 8}),
            "Active preset address resent",
            "Active preset address resent; the frame was sent, but within 2500 ms the host did not observe all of the required conditions together (a newer position update naming /user/live position 8, a newer full preset publication and a clean flag), so the discard is not verified",
        ),
    ] {
        let plan = plan_preset_recall(method, &params, Some(&before)).unwrap();
        assert_eq!(
            plan.detail, detail,
            "{method}: the plan names the requested action"
        );
        assert!(!detail_claims_verification(&plan.detail), "{method}");
        let timeout = plan.timeout_detail(2500);
        assert_eq!(timeout, timeout_text, "{method}");
        assert!(!detail_claims_verification(&timeout), "{method}: {timeout}");

        let mut after = before.clone();
        after.preset_position = plan.position;
        after.position_revision = 15;
        after.full_preset_revision = 15;
        let observed = plan.address_observed_result(&after).unwrap();
        assert_eq!(observed["verified"], false, "{method}");
        let observed_detail = observed["detail"].as_str().unwrap();
        assert!(
            !detail_claims_verification(observed_detail),
            "{method}: {observed_detail}"
        );
    }
}

/// Asserts a recall timeout detail is true whatever partial evidence the host held: it names
/// every condition the predicate requires and asserts none of them absent.
fn assert_timeout_detail_true_for_any_partial_evidence(label: &str, detail: &str, reload: bool) {
    assert!(!detail_claims_verification(detail), "{label}: {detail}");
    assert!(
        detail.contains("did not observe all of the required conditions together"),
        "{label}: {detail}"
    );
    assert!(
        detail.contains("a newer position update naming /user/live position"),
        "{label}: {detail}"
    );
    assert!(
        detail.contains("a newer full preset publication"),
        "{label}: {detail}"
    );
    assert_eq!(detail.contains("a clean flag"), reload, "{label}: {detail}");
    for asserted_absence in [
        "no position update",
        "no newer",
        "no full preset",
        "no clean flag",
        "without a clean flag",
        "did not arrive",
    ] {
        assert!(!detail.contains(asserted_absence), "{label}: {detail}");
    }
}

#[test]
fn a_recall_timeout_is_true_for_every_partial_observation_the_predicate_rejects() {
    // The predicate needs a newer position update naming the address AND a newer full preset
    // publication AND, for a reload, a clean flag. Each case drives the shared verification
    // runtime with exactly that evidence to a timeout; the detail must not contradict it.
    let mut before = GatewaySnapshot {
        preset_name: "Live".into(),
        setlist_key: "/user/live".into(),
        preset_position: 8,
        position_revision: 14,
        full_preset_revision: 14,
        grid_layout_complete: true,
        ..GatewaySnapshot::default()
    };
    before.unobserved.retain(|field| *field != "dirty");
    let recall = plan_preset_recall(
        "device.recallPreset",
        &json!({"setlistKey": "/user/live", "position": 9,
            "expectedPresetName": "Live", "expectedPosition": 8}),
        Some(&before),
    )
    .unwrap();
    let dirty_before = GatewaySnapshot {
        dirty: true,
        ..before.clone()
    };
    let reload = plan_preset_recall(
        "device.reloadPreset",
        &json!({"expectedPresetName": "Live", "expectedPosition": 8}),
        Some(&dirty_before),
    )
    .unwrap();
    let held = |position: u32, position_revision: u64, full_preset_revision: u64, dirty: bool| {
        GatewaySnapshot {
            preset_position: position,
            position_revision,
            full_preset_revision,
            dirty,
            ..before.clone()
        }
    };
    for (label, method, plan, evidence) in [
        (
            "(a) newer position 9, old body",
            "device.recallPreset",
            &recall,
            held(9, 15, 14, false),
        ),
        (
            "(b) newer body, no newer position",
            "device.recallPreset",
            &recall,
            held(8, 14, 15, false),
        ),
        (
            "(c) reload: newer position and body, clean flag missing",
            "device.reloadPreset",
            &reload,
            held(8, 15, 15, true),
        ),
        (
            "(d) nothing arrived",
            "device.recallPreset",
            &recall,
            before.clone(),
        ),
        (
            "(d) reload: nothing arrived",
            "device.reloadPreset",
            &reload,
            dirty_before.clone(),
        ),
    ] {
        let policy = gateway_write_verification_policy(method);
        let mut runtime = GatewayVerificationRuntime::new(plan.verification(), 5, 1_000, &policy);
        // A fresh observation carrying exactly this evidence does not satisfy the predicate...
        assert!(
            matches!(
                runtime.advance(Some(&evidence), None, 6, 1_001),
                GatewayVerificationAction::Wait { .. }
            ),
            "{label}: the partial evidence must not satisfy the predicate"
        );
        assert!(!plan.matches(&evidence), "{label}");
        // ...so the wait ends in a timeout, and the timeout detail must be true for it.
        assert_eq!(
            runtime.advance(Some(&evidence), None, 6, 1_000 + policy.timeout_ms),
            GatewayVerificationAction::TimedOut,
            "{label}"
        );
        let detail = plan.timeout_detail(policy.timeout_ms);
        assert!(detail.starts_with(&plan.detail), "{label}: {detail}");
        assert_timeout_detail_true_for_any_partial_evidence(label, &detail, plan.require_clean);
    }
}
