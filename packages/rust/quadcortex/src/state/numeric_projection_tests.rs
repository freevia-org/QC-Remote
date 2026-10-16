//! Actual protobuf numeric projections reject malformed IEEE values without inventing firmware ranges.
use super::*;

#[test]
fn an_unproven_grid_move_withdraws_published_layout_authority_without_inventing_blocks() {
    use crate::device::state_runtime::DeviceStateRuntime;
    let mut runtime = DeviceStateRuntime::new();
    runtime
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: 1,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    chains: vec![Chain {
                        models: [11, 22]
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
                })),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
    assert!(runtime.snapshot().grid_layout_complete);
    let before = runtime.snapshot().clone();
    let observation = runtime
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
    assert!(
        !runtime.snapshot().grid_layout_complete,
        "cached readers cannot retain complete-layout authority after decoder refused the batch"
    );
    assert_eq!(
        runtime.snapshot().blocks,
        before.blocks,
        "unknown topology never fabricates/moves models"
    );
    assert_eq!(runtime.snapshot().preset_identity, before.preset_identity);
    assert_eq!(runtime.snapshot().routes, before.routes);
    assert_eq!(observation.states.len(), 1);
    assert_eq!(observation.states[0].kind, "gridTopology");
    assert_eq!(observation.states[0].grid_layout_complete, Some(false));
    assert!(observation.states[0].blocks.is_none());
    assert!(runtime.move_block_operation(0, 0, 0, 4, Some(11)).is_err());
}

#[test]
fn duplicate_bypass_addresses_are_not_current_or_scene_verification_evidence() {
    use crate::device::request::GatewayVerification;
    use crate::device::state_runtime::DeviceStateRuntime;
    let rows = |values: &[bool]| {
        values
            .iter()
            .map(|value| crate::proto::Bypass {
                row: Some(bypass::Row::Row(0)),
                col_bypass: vec![crate::proto::ColBypass {
                    column: Some(col_bypass::Column::Column(0)),
                    scene_mode: Some(col_bypass::SceneMode::SceneMode(true)),
                    scene_bypass: vec![crate::proto::SceneBypass { bypass: *value }; 8],
                }],
            })
            .collect::<Vec<_>>()
    };
    for sparse in [false, true] {
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: 1,
                    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                        chains: vec![Chain {
                            models: vec![Model {
                                hash: Some(model::Hash::Hash(1234)),
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        bypass: if sparse {
                            Vec::new()
                        } else {
                            rows(&[false, true, true])
                        },
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        if sparse {
            runtime
                .ingest(
                    profile::MESSAGE_TYPE_GRID,
                    &pa::GridMessage {
                        action: 1,
                        preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                            bypass: rows(&[false, true, true]),
                            ..Default::default()
                        })),
                        ..Default::default()
                    }
                    .encode_to_vec(),
                )
                .unwrap();
        }
        assert_eq!(
            runtime.snapshot().blocks[0].bypassed,
            None,
            "duplicate address has no source precedence, sparse={sparse}"
        );
        assert!(!GatewayVerification::Bypass {
            row: 0,
            column: 0,
            bypassed: true
        }
        .matches(runtime.snapshot(), None));
        runtime
            .ingest(
                profile::MESSAGE_TYPE_SCENE,
                &pa::SceneMessage {
                    action: 1,
                    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(7)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(runtime.snapshot().blocks[0].bypassed, None);
    }
}

#[test]
fn malformed_sparse_bypass_withdraws_prior_current_value_before_another_scene_message() {
    use crate::device::request::GatewayVerification;
    use crate::device::state_runtime::DeviceStateRuntime;
    let mut runtime = DeviceStateRuntime::new();
    runtime
        .ingest(
            profile::MESSAGE_TYPE_SCENE,
            &pa::SceneMessage {
                action: 1,
                selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(7)),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
    runtime
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: 1,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    chains: vec![Chain {
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(1234)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    bypass: vec![crate::proto::Bypass {
                        col_bypass: vec![crate::proto::ColBypass {
                            scene_mode: Some(col_bypass::SceneMode::SceneMode(true)),
                            scene_bypass: vec![crate::proto::SceneBypass { bypass: true }; 8],
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
    assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(true));
    let before = runtime.snapshot().clone();
    let observation = runtime
        .ingest(
            profile::MESSAGE_TYPE_GRID,
            &pa::GridMessage {
                action: 1,
                preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                    bypass: vec![crate::proto::Bypass {
                        col_bypass: vec![crate::proto::ColBypass {
                            scene_mode: Some(col_bypass::SceneMode::SceneMode(true)),
                            scene_bypass: vec![crate::proto::SceneBypass { bypass: true }; 2],
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
    assert_eq!(runtime.snapshot().active_scene, 7);
    assert_eq!(
        runtime.snapshot().blocks[0].bypassed,
        None,
        "scene H was withdrawn by this sparse record, not a later scene event"
    );
    assert!(observation.states.iter().any(|state| {
        state.kind == "bypassBatch"
            && state.blocks.as_ref().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block.row == 0
                        && block.column == 0
                        && block.model_id == Some(1234)
                        && block.bypassed.is_none()
                })
            })
    }));
    assert!(!GatewayVerification::Bypass {
        row: 0,
        column: 0,
        bypassed: true
    }
    .matches(runtime.snapshot(), None));
    assert_eq!(runtime.snapshot().blocks[0].name, before.blocks[0].name);
    assert_eq!(
        runtime.snapshot().grid_layout_complete,
        before.grid_layout_complete
    );
    assert_eq!(runtime.snapshot().preset_identity, before.preset_identity);
}

fn scene_projection_bypass_row(count: usize, value: bool) -> crate::proto::Bypass {
    crate::proto::Bypass {
        col_bypass: vec![crate::proto::ColBypass {
            scene_mode: Some(col_bypass::SceneMode::SceneMode(true)),
            scene_bypass: (0..count)
                .map(|index| crate::proto::SceneBypass {
                    bypass: if count == 1 || index == 0 {
                        value
                    } else {
                        !value
                    },
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn scene_projection_select(
    runtime: &mut crate::device::state_runtime::DeviceStateRuntime,
    scene: u32,
) {
    let observation = runtime
        .ingest(
            profile::MESSAGE_TYPE_SCENE,
            &pa::SceneMessage {
                action: 1,
                selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(scene)),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
    assert_eq!(observation.states.len(), 1, "no fake preset publication");
    assert_eq!(observation.states[0].kind, "scene");
}

fn scene_projection_recall(
    runtime: &mut crate::device::state_runtime::DeviceStateRuntime,
    count: usize,
    value: bool,
) {
    runtime
        .ingest(
            profile::MESSAGE_TYPE_RECALL_PRESET,
            &pa::RecallPresetMessage {
                action: 1,
                preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                    chains: vec![Chain {
                        models: vec![Model {
                            hash: Some(model::Hash::Hash(1234)),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    bypass: vec![scene_projection_bypass_row(count, value)],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
}

#[test]
fn scene_bypass_projection_uses_current_scene_and_latest_sparse_evidence() {
    use crate::device::request::GatewayVerification;
    use crate::device::state_runtime::DeviceStateRuntime;
    for count in [1, 2, 8] {
        let mut runtime = DeviceStateRuntime::new();
        scene_projection_recall(&mut runtime, count, false);
        assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(false));
        let before = runtime.snapshot().clone();
        scene_projection_select(&mut runtime, 7);
        let expected = match count {
            1 => Some(false),
            8 => Some(true),
            _ => None,
        };
        assert_eq!(
            runtime.snapshot().blocks[0].bypassed,
            expected,
            "scene H count={count}"
        );
        let predicate = GatewayVerification::Bypass {
            row: 0,
            column: 0,
            bypassed: true,
        };
        assert_eq!(
            predicate.matches(runtime.snapshot(), None),
            expected == Some(true)
        );
        assert_eq!(runtime.snapshot().preset_identity, before.preset_identity);
        assert_eq!(
            runtime.snapshot().full_preset_revision,
            before.full_preset_revision
        );
        assert_eq!(
            runtime.snapshot().grid_layout_complete,
            before.grid_layout_complete
        );
        assert_eq!(
            runtime.snapshot().blocks[0].model_id,
            before.blocks[0].model_id
        );
        assert_eq!(runtime.snapshot().blocks[0].name, before.blocks[0].name);
        assert_eq!(runtime.snapshot().routes, before.routes);
        scene_projection_select(&mut runtime, 0);
        assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(false));

        // A real sparse bypass record replaces the earlier full-recall evidence.
        runtime
            .ingest(
                profile::MESSAGE_TYPE_GRID,
                &pa::GridMessage {
                    action: 1,
                    preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                        bypass: vec![scene_projection_bypass_row(count, true)],
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(true));
        scene_projection_select(&mut runtime, 7);
        assert_eq!(
            runtime.snapshot().blocks[0].bypassed,
            match count {
                1 => Some(true),
                8 => Some(false),
                _ => None,
            },
            "latest sparse record, not obsolete full recall, owns scene H"
        );
        scene_projection_select(&mut runtime, 0);
        assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(true));

        // Full recall replaces the fold rather than retaining sparse overrides.
        scene_projection_recall(&mut runtime, 8, false);
        scene_projection_select(&mut runtime, 7);
        assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(true));
    }
}

#[test]
fn scene_bypass_projection_does_not_follow_moved_or_deleted_model_addresses() {
    use crate::device::state_runtime::DeviceStateRuntime;
    let mut runtime = DeviceStateRuntime::new();
    scene_projection_recall(&mut runtime, 8, false);
    runtime
        .ingest(
            profile::MESSAGE_TYPE_GRID_MOVE,
            &pa::GridMoveMessage {
                r#move: vec![pa::GridMoveElement {
                    from_row: 0,
                    from_col: 0,
                    to_row: 0,
                    to_col: 1,
                    is_drop: true,
                }],
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
    assert_eq!(runtime.snapshot().blocks[0].column, 1);
    scene_projection_select(&mut runtime, 7);
    assert_eq!(
        runtime.snapshot().blocks[0].bypassed,
        None,
        "move proves model placement, not raw bypass-address remapping"
    );
    runtime
        .ingest(
            profile::MESSAGE_TYPE_GRID,
            &pa::GridMessage {
                action: pa::message_action::Enum::Delete as i32,
                preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                    chains: vec![Chain {
                        row: Some(chain::Row::Row(0)),
                        models: vec![Model {
                            column: Some(model::Column::Column(1)),
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
    assert!(runtime.snapshot().blocks.is_empty());
    scene_projection_select(&mut runtime, 0);
    assert!(
        runtime.snapshot().blocks.is_empty(),
        "scene projection cannot recreate deleted topology"
    );
    scene_projection_recall(&mut runtime, 8, false);
    scene_projection_select(&mut runtime, 7);
    assert_eq!(runtime.snapshot().blocks[0].column, 0);
    assert_eq!(runtime.snapshot().blocks[0].bypassed, Some(true));
}

fn assert_missing_active_parameter_is_not_verification_evidence(text: bool) {
    use crate::device::request::GatewayVerification;
    use crate::device::state_runtime::DeviceStateRuntime;
    let seed = |count: usize, scene_mode: bool| {
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(
                profile::MESSAGE_TYPE_SCENE,
                &pa::SceneMessage {
                    action: 1,
                    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(7)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        runtime
            .ingest(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: 1,
                    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                        chains: vec![Chain {
                            models: vec![Model {
                                hash: Some(model::Hash::Hash(1234)),
                                params: vec![Param {
                                    scene_mode: Some(param::SceneMode::SceneMode(scene_mode)),
                                    dynamic_steps: if text {
                                        vec!["Low".into(), "High".into()]
                                    } else {
                                        Vec::new()
                                    },
                                    param_values: (0..count)
                                        .map(|index| crate::proto::ParamValue {
                                            value: Some(if text {
                                                param_value::Value::StringValue(
                                                    if index == 0 { "Low" } else { "High" }.into(),
                                                )
                                            } else {
                                                param_value::Value::FloatValue(if index == 0 {
                                                    0.2
                                                } else {
                                                    0.8
                                                })
                                            }),
                                        })
                                        .collect(),
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
        runtime
    };
    let malformed = seed(2, true);
    let predicate = GatewayVerification::Parameter {
        row: 0,
        column: 0,
        parameter_index: 0,
        value: if text { 1.0 } else { 0.8 },
        routing_model_id: None,
    };
    let detail = malformed.single_parameter_target_detail(0, 0, 0);
    assert!(
        !predicate.matches(malformed.snapshot(), detail.as_ref()),
        "scene7 has no value: index1 must not certify the active parameter or write readback"
    );
    assert!(detail.is_none());
    assert!(malformed.block_details(0, 0).unwrap().parameters.is_empty());
    for (count, scene_mode, expected) in [
        (1, true, if text { 0.0 } else { 0.2 }),
        (8, true, if text { 1.0 } else { 0.8 }),
        (2, false, if text { 0.0 } else { 0.2 }),
    ] {
        let healthy = seed(count, scene_mode);
        let detail = healthy.single_parameter_target_detail(0, 0, 0).unwrap();
        assert!((detail.normalized_value.unwrap() - expected).abs() < 0.0005);
        let predicate = GatewayVerification::Parameter {
            row: 0,
            column: 0,
            parameter_index: 0,
            value: expected,
            routing_model_id: None,
        };
        assert!(predicate.matches(healthy.snapshot(), Some(&detail)));
        assert_eq!(
            healthy.block_details(0, 0).unwrap().parameters,
            vec![detail]
        );
    }
}

#[test]
fn missing_numeric_active_scene_is_not_current_parameter_or_verification_evidence() {
    assert_missing_active_parameter_is_not_verification_evidence(false);
}

#[test]
fn missing_text_active_scene_is_not_current_parameter_or_verification_evidence() {
    assert_missing_active_parameter_is_not_verification_evidence(true);
}

#[test]
fn native_lookup_projection_marks_unrepresentable_scale_unknown_without_clamping() {
    let project = |x: &str, y: &str, maximum: &str| {
        let xml = format!(
            "<Models><Category name='Utility'><Model id='17' name='Lookup'><Parameter name='Level' type='float' min='0' max='{maximum}' xValues='{x}' yValues='{y}'/></Model></Category></Models>"
        );
        let mut decoder = StateDecoder::new();
        decoder.install_catalog(ModelCatalog(parse_catalog(xml.as_bytes()).unwrap()));
        let preset = BinaryPreset {
            chains: vec![Chain {
                models: vec![Model {
                    hash: Some(model::Hash::Hash(17)),
                    params: vec![Param {
                        param_values: vec![crate::proto::ParamValue {
                            value: Some(param_value::Value::FloatValue(0.5)),
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let wire = pa::RecallPresetMessage {
            action: 1,
            preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
            ..Default::default()
        }
        .encode_to_vec();
        decoder
            .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &wire)
            .unwrap();
        decoder.block_details(0, 0).unwrap().parameters.remove(0)
    };
    let invalid = project("0.75,0.7500000000000001", "0,1e308", "1e308");
    assert!(
        !invalid.scale_known,
        "finite catalog inputs must not certify a nonfinite public display"
    );
    assert_eq!(invalid.value_scale, "unknown");
    assert_eq!(invalid.normalized_value, Some(0.5));
    assert_eq!(invalid.display_value, "0.5");
    assert!(invalid.scale_points.is_empty());
    assert!(invalid.scale_exponent.is_none());
    assert!(invalid.floor_display.is_none());
    assert!(invalid.floor_normalized.is_none());
    let within = project("0.25,0.75", "10,20", "20");
    assert!(within.scale_known);
    assert_eq!(within.display_value.parse::<f64>().unwrap(), 15.0);
    let extrapolated = project("0.75,0.875", "10,20", "20");
    assert!(extrapolated.scale_known);
    assert_eq!(extrapolated.display_value.parse::<f64>().unwrap(), -10.0);
    assert_eq!(extrapolated.value_scale, "lookup");
}

#[test]
fn malformed_selected_scene_never_becomes_scene_h_or_replaces_authority() {
    use crate::device::state_runtime::DeviceStateRuntime;
    let wire = |scene| {
        pa::SceneMessage {
            action: pa::message_action::Enum::Update as i32,
            selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(scene)),
            ..Default::default()
        }
        .encode_to_vec()
    };
    for malformed in [crate::domain::SCENE_COUNT, u32::MAX] {
        let bytes = wire(malformed);
        assert!(pa::SceneMessage::decode(bytes.as_slice()).is_ok());
        let mut decoder = StateDecoder::new();
        decoder
            .decode(profile::MESSAGE_TYPE_SCENE, &wire(2))
            .unwrap();
        let revision = decoder.state_revision;
        assert!(decoder.decode(profile::MESSAGE_TYPE_SCENE, &bytes).is_err());
        assert_eq!(decoder.active_scene, 2);
        assert_eq!(decoder.state_revision, revision);
        assert!(crate::responses::decode_selected_scene(&bytes).is_err());
        let mut runtime = DeviceStateRuntime::new();
        runtime
            .ingest(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: 1,
                    preset: Some(pa::recall_preset_message::Preset::Preset(
                        BinaryPreset::default(),
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        runtime
            .ingest(profile::MESSAGE_TYPE_SCENE, &wire(2))
            .unwrap();
        let before = serde_json::to_value(runtime.snapshot()).unwrap();
        assert!(runtime.ingest(profile::MESSAGE_TYPE_SCENE, &bytes).is_err());
        assert_eq!(serde_json::to_value(runtime.snapshot()).unwrap(), before);
        assert_eq!(
            runtime
                .external_preflight_observation("device.selectScene")
                .unwrap()["activeScene"],
            2
        );
    }
    for healthy in [0, crate::domain::SCENE_COUNT - 1] {
        assert_eq!(
            StateDecoder::new()
                .decode(profile::MESSAGE_TYPE_SCENE, &wire(healthy))
                .unwrap()[0]
                .active_scene,
            Some(healthy)
        );
        assert_eq!(
            crate::responses::decode_selected_scene(&wire(healthy)).unwrap(),
            Some(healthy)
        );
    }
}

#[test]
fn native_capture_retarget_refuses_an_absent_active_scene_without_losing_singleton_compatibility() {
    let model = |count: usize, scene_mode: bool| Model {
        hash: Some(model::Hash::Hash(14_001)),
        column: Some(model::Column::Column(0)),
        params: vec![Param {
            index: Some(param::Index::Index(5)),
            scene_mode: Some(param::SceneMode::SceneMode(scene_mode)),
            param_values: (0..count)
                .map(|scene| crate::proto::ParamValue {
                    value: Some(param_value::Value::StringValue(format!("retained/{scene}"))),
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let seed = |current: Model| {
        let mut decoder = StateDecoder::new();
        decoder
            .decode(
                profile::MESSAGE_TYPE_SCENE,
                &pa::SceneMessage {
                    action: 1,
                    selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(7)),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        decoder
            .decode(
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: 1,
                    preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                        chains: vec![Chain {
                            models: vec![current],
                            ..Default::default()
                        }],
                        ..Default::default()
                    })),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        decoder
    };
    let malformed = seed(model(2, true));
    let before = malformed.preset.as_ref().unwrap().encode_to_vec();
    assert!(
        malformed
            .capture_retarget_operation(0, 0, 14_001, "replacement")
            .is_err(),
        "a scene-mode multi-value record without active index7 must not overwrite index1"
    );
    assert_eq!(malformed.preset.as_ref().unwrap().encode_to_vec(), before);
    for (count, scene_mode, selected) in [(1, true, 0), (8, true, 7), (2, false, 0)] {
        let healthy = seed(model(count, scene_mode));
        let operation = healthy
            .capture_retarget_operation(0, 0, 14_001, "replacement")
            .unwrap();
        let DeviceOperation::RetargetCapture {
            model: ref changed,
            active_scene,
            ..
        } = operation
        else {
            panic!("retarget operation");
        };
        assert_eq!(active_scene, 7);
        for (index, parameter_value) in changed.params[0].param_values.iter().enumerate() {
            let expected = if index == selected {
                "replacement".into()
            } else {
                format!("retained/{index}")
            };
            assert_eq!(
                parameter_value.value,
                Some(param_value::Value::StringValue(expected))
            );
        }
        let messages = operation.try_encode().unwrap();
        let message = pa::GridMessage::decode(messages[0].payload.as_slice()).unwrap();
        let Some(pa::grid_message::Preset::Preset(encoded)) = message.preset else {
            panic!("encoded model");
        };
        assert_eq!(
            encoded.chains[0].models[0].params[0].param_values.len(),
            count
        );
    }
}

#[test]
fn typed_capture_retarget_refuses_a_missing_active_scene_even_when_the_fallback_key_matches() {
    let operation = |count: usize| DeviceOperation::RetargetCapture {
        row: 0,
        column: 0,
        active_scene: 7,
        key: "replacement".into(),
        source_revision: 1,
        model: Model {
            hash: Some(model::Hash::Hash(14_001)),
            column: Some(model::Column::Column(0)),
            params: vec![Param {
                index: Some(param::Index::Index(5)),
                scene_mode: Some(param::SceneMode::SceneMode(true)),
                param_values: (0..count)
                    .map(|_| crate::proto::ParamValue {
                        value: Some(param_value::Value::StringValue("replacement".into())),
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        },
        bypass: None,
    };
    assert!(
        operation(2).try_encode().is_err(),
        "typed admission must not certify an absent scene7 through index1"
    );
    assert!(
        operation(1).try_encode().is_ok(),
        "explicit shared-singleton compatibility remains"
    );
    assert!(operation(8).try_encode().is_ok());
}

#[test]
fn cpu_and_bulk_nonfinite_values_are_refused_before_publication() {
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for slot in 0..2 {
            let message = pa::CpuLoadMessage {
                cpu_total_load: Some(pa::cpu_load_message::CpuTotalLoad::CpuTotalLoad(
                    if slot == 0 { invalid } else { 0.5 },
                )),
                chains: vec![pa::CpuChainLoad {
                    columns: vec![pa::CpuColumnLoad {
                        cpu_load: if slot == 1 { invalid } else { 0.5 },
                        is_on_core2: false,
                    }],
                }],
                ..Default::default()
            };
            let wire = message.encode_to_vec();
            assert!(pa::CpuLoadMessage::decode(wire.as_slice()).is_ok());
            assert!(StateDecoder::new()
                .decode(profile::MESSAGE_TYPE_CPU_LOAD, &wire)
                .is_err());
        }
        let wire = pa::BulkOperationMessage {
            progress: Some(pa::bulk_operation_message::Progress::Progress(invalid)),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(pa::BulkOperationMessage::decode(wire.as_slice()).is_ok());
        assert!(StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_BULK_OPERATION, &wire)
            .is_err());
    }
    for value in [-2.0, 0.0, f32::MAX] {
        let wire = pa::CpuLoadMessage {
            cpu_total_load: Some(pa::cpu_load_message::CpuTotalLoad::CpuTotalLoad(value)),
            ..Default::default()
        }
        .encode_to_vec();
        let update = StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_CPU_LOAD, &wire)
            .unwrap()
            .remove(0);
        assert_eq!(update.cpu_total_load, Some(value));
        let wire = pa::BulkOperationMessage {
            progress: Some(pa::bulk_operation_message::Progress::Progress(value)),
            ..Default::default()
        }
        .encode_to_vec();
        let update = StateDecoder::new()
            .decode(profile::MESSAGE_TYPE_BULK_OPERATION, &wire)
            .unwrap()
            .remove(0);
        assert_eq!(update.bulk_operation.unwrap().progress, Some(value));
    }
}

#[test]
fn nonfinite_expression_metadata_never_replaces_an_exact_retained_preset() {
    let good = BinaryPreset {
        name: Some(binary_preset::Name::Name("retained numeric fixture".into())),
        chains: vec![Chain {
            models: vec![Model {
                hash: Some(model::Hash::Hash(1234)),
                params: vec![Param {
                    param_values: vec![crate::proto::ParamValue {
                        value: Some(param_value::Value::FloatValue(0.5)),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let recall = |preset| {
        pa::RecallPresetMessage {
            action: 1,
            preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
            ..Default::default()
        }
        .encode_to_vec()
    };
    for group in 0..7 {
        for slot in 0..4 {
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut model = good.chains[0].models[0].clone();
                model.bypass_expression = vec![crate::proto::Expression {
                    expression: 1,
                    expression_min: 0.0,
                    expression_max: 1.0,
                }];
                match slot {
                    0 => model.bypass_expression[0].expression_min = invalid,
                    1 => model.bypass_expression[0].expression_max = invalid,
                    2 => {
                        model.params[0].expression_min =
                            Some(param::ExpressionMin::ExpressionMin(invalid))
                    }
                    3 => {
                        model.params[0].expression_max =
                            Some(param::ExpressionMax::ExpressionMax(invalid))
                    }
                    _ => unreachable!(),
                }
                let mut bad = good.clone();
                match group {
                    0 => bad.chains[0].models = vec![model],
                    1 => bad.chains[0].splitter = vec![model],
                    2 => bad.chains[0].combined_splitter = vec![model],
                    3 => bad.chains[0].mixer = vec![model],
                    4 => bad.chains[0].input_control = vec![model],
                    5 => bad.chains[0].output_control = vec![model],
                    6 => bad.tempo_program_data = vec![model],
                    _ => unreachable!(),
                }
                let mut decoder = StateDecoder::new();
                decoder
                    .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &recall(good.clone()))
                    .unwrap();
                let before = decoder.preset.as_ref().unwrap().encode_to_vec();
                assert!(decoder.preset_exact);
                assert!(
                    decoder
                        .decode(profile::MESSAGE_TYPE_RECALL_PRESET, &recall(bad.clone()))
                        .is_err(),
                    "group{group}/slot{slot}"
                );
                assert_eq!(decoder.preset.as_ref().unwrap().encode_to_vec(), before);
                assert!(decoder.preset_exact);
                let sparse = pa::GridMessage {
                    action: 1,
                    preset: Some(pa::grid_message::Preset::Preset(bad)),
                    ..Default::default()
                }
                .encode_to_vec();
                assert!(decoder.decode(profile::MESSAGE_TYPE_GRID, &sparse).is_err());
                assert_eq!(decoder.preset.as_ref().unwrap().encode_to_vec(), before);
                assert!(decoder.preset_exact);
            }
        }
    }
}
