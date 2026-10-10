use async_trait::async_trait;
use qc_remote_mcp::{ACTIONS, BackendError, PrincipalRoute, QcBackend, QcMcp};
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingBackend(Mutex<Vec<(&'static str, Map<String, Value>)>>);

struct FailingBackend;

struct MalformedResultBackend;

#[async_trait]
impl QcBackend for FailingBackend {
    async fn request(
        &self,
        _: &PrincipalRoute,
        _: &'static str,
        _: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        Err(BackendError::unavailable("USB disconnected"))
    }
}

#[async_trait]
impl QcBackend for MalformedResultBackend {
    async fn request(
        &self,
        _: &PrincipalRoute,
        _: &'static str,
        _: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        Ok(json!({
            "detail": "sent",
            "accepted": true,
            "verified": false,
            "verification": "authoritative_readback"
        }))
    }
}

#[async_trait]
impl QcBackend for RecordingBackend {
    async fn request(
        &self,
        _: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        self.0.lock().unwrap().push((rpc, params.clone()));
        Ok(if rpc == "device.listModels" {
            json!({"models":[
                {"id":1,"name":"Vintage Delay","category":"Delay"},
                {"id":2,"name":"Plini Lead","category":"Plugin","basedOn":"Archetype Plini"},
                {"id":3,"name":"UK C30 TopBoost","category":"Guitar Amp","basedOn":""},
                {"id":4,"name":"1x12 Blue","category":"Guitar Cab","basedOn":""},
                {"id":5,"name":"Studio EQ","category":"EQ","basedOn":""}
            ],"audit":{"modelCount":5}})
        } else if matches!(rpc, "device.snapshot" | "device.currentPreset") {
            json!({"presetName":"Clean","blocks":[]})
        } else if ACTIONS.iter().any(|action| {
            action.rpc == rpc && action.classification != qc_remote_mcp::Classification::Read
        }) && !matches!(
            rpc,
            "device.reconnect"
                | "device.resetSession"
                | "device.disconnect"
                | "device.previewParameter"
                | "device.previewLaneControlParameter"
                | "device.createBackup"
        ) {
            json!({"method":rpc,"params":params,"detail":"accepted","accepted":true,"verified":false,"verification":"accepted_unverified"})
        } else {
            json!({"method":rpc,"params":params})
        })
    }
}

fn server() -> (QcMcp, Arc<RecordingBackend>) {
    let backend = Arc::new(RecordingBackend::default());
    (QcMcp::new(backend.clone()), backend)
}

fn route() -> PrincipalRoute {
    PrincipalRoute::new("user-1", "device-1")
}

#[test]
fn static_map_has_exact_contract_names_and_rpcs() {
    let contract: Value =
        serde_json::from_str(include_str!("../../../contracts/public/qc-actions.v1.json")).unwrap();
    assert_eq!(
        qc_remote_mcp::MCP_INSTRUCTIONS,
        contract["mcpInstructions"].as_str().unwrap()
    );
    let expected: Vec<_> = contract["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["name"].as_str().unwrap(), a["rpc"].as_str().unwrap()))
        .collect();
    let actual: Vec<_> = ACTIONS.iter().map(|a| (a.name, a.rpc)).collect();
    assert_eq!(actual, expected);

    for (spec, action) in ACTIONS.iter().zip(contract["actions"].as_array().unwrap()) {
        let contract_properties = action["properties"].as_object().unwrap();
        let schema = spec.parsed_input_schema();
        let actual_properties = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        let expected_properties = contract_properties
            .keys()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            actual_properties, expected_properties,
            "property drift for {}",
            spec.name
        );
        let actual_required = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        let expected_required = action["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            actual_required, expected_required,
            "required-field drift for {}",
            spec.name
        );
        let classification = match spec.classification {
            qc_remote_mcp::Classification::Read => "read",
            qc_remote_mcp::Classification::LiveWrite => "live-write",
            qc_remote_mcp::Classification::PersistentWrite => "persistent-write",
            qc_remote_mcp::Classification::RiskyWrite => "risky-write",
        };
        assert_eq!(
            classification,
            action["classification"].as_str().unwrap(),
            "classification drift for {}",
            spec.name
        );
    }
}

#[test]
fn surface_has_only_intent_tools_and_reference_resources() {
    let (server, _) = server();
    let tools = server.tools();
    assert_eq!(tools.len(), ACTIONS.len() + 1);
    for (spec, tool) in ACTIONS.iter().zip(&tools) {
        let annotations = tool.annotations.as_ref().unwrap();
        let is_read = spec.classification == qc_remote_mcp::Classification::Read;
        assert_eq!(
            annotations.read_only_hint,
            Some(is_read),
            "{} read-only annotation",
            spec.name
        );
        assert_eq!(
            annotations.idempotent_hint,
            Some(is_read),
            "{} idempotence annotation",
            spec.name
        );
    }
    assert!(
        tools
            .iter()
            .all(|t| !t.name.contains("raw") && !t.name.contains("protobuf"))
    );
    assert_eq!(
        tools
            .iter()
            .find(|t| t.name == "get_current_preset")
            .unwrap()
            .annotations
            .as_ref()
            .unwrap()
            .read_only_hint,
        Some(true)
    );
    assert_eq!(
        tools
            .iter()
            .find(|t| t.name == "copy_preset")
            .unwrap()
            .annotations
            .as_ref()
            .unwrap()
            .destructive_hint,
        Some(true)
    );
    let device_knowledge = tools
        .iter()
        .find(|tool| tool.name == "search_qc_device_knowledge")
        .unwrap();
    let annotations = device_knowledge.annotations.as_ref().unwrap();
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
}

#[tokio::test]
async fn expected_state_and_parameter_translation_match_reference_gateway_contract() {
    let (server, backend) = server();
    server
        .execute(
            &route(),
            "set_parameter",
            Some(
                json!({
                    "row":2,"column":3,"parameter_index":7,"value":0.75,"expected_value":0.5,
                    "expected_model_id":41,"expected_scene":4,"expected_preset_identity": "fixture-native-A", "expected_preset_name":"Lead"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.setParameter");
    assert_eq!(calls[0].1["parameterIndex"], 7);
    assert_eq!(calls[0].1["expectedPresetName"], "Lead");
}

#[tokio::test]
async fn invalid_state_and_unconfirmed_writes_never_reach_backend() {
    let (server, backend) = server();
    assert!(
        server
            .execute(
                &route(),
                "select_scene",
                Some(
                    json!({"scene":2,"expected_preset_identity": "fixture-native-A", "expected_preset_name":""})
                        .as_object()
                        .unwrap()
                        .clone()
                )
            )
            .await
            .is_err()
    );
    assert!(
        server
            .execute(
                &route(),
                "set_master_volume",
                Some(
                    json!({"value":80,"expected_value":50,"confirm_risky_operation":false})
                        .as_object()
                        .unwrap()
                        .clone()
                )
            )
            .await
            .is_err()
    );
    assert!(
        server
            .execute(
                &route(),
                "copy_preset",
                Some(
                    json!({
                        "source_setlist_key":"user","source_position":1,"source_name":"A",
                        "destination_setlist_key":"user","destination_position":2,
                        "expected_preset_identity": "fixture-native-A", "expected_preset_name":"A","expected_position":1,"confirm_overwrite":true,
                        "confirm_persistent_write":false
                    })
                    .as_object()
                    .unwrap()
                    .clone()
                )
            )
            .await
            .is_err()
    );
    assert!(backend.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn persistent_confirmation_is_consumed_not_forwarded() {
    let (server, backend) = server();
    server
        .execute(
            &route(),
            "rename_current_preset",
            Some(
                json!({
                    "new_name":"New name","expected_preset_identity": "fixture-native-A", "expected_preset_name":"Clean","expected_position":4,
                    "confirm_persistent_write":true
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].1["name"], "New name");
    assert_eq!(calls[0].1["confirmRename"], true);
    assert!(!calls[0].1.contains_key("confirmPersistentWrite"));
}

#[tokio::test]
async fn delete_setlist_requires_confirmation_and_forwards_only_the_name() {
    let (server, backend) = server();
    assert!(
        server
            .execute(
                &route(),
                "delete_setlist",
                Some(json!({"name":"Tour"}).as_object().unwrap().clone()),
            )
            .await
            .is_err()
    );
    assert!(backend.0.lock().unwrap().is_empty());

    server
        .execute(
            &route(),
            "delete_setlist",
            Some(
                json!({"name":"Tour", "confirm_persistent_write":true})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.deleteSetlist");
    assert_eq!(calls[0].1["name"], "Tour");
    assert_eq!(calls[0].1.len(), 1);
}

#[tokio::test]
async fn model_search_matches_the_whole_word_behind_a_category_abbreviation() {
    // Categories use the catalog's standard abbreviations - "Guitar Amp",
    // "Guitar Cab", "EQ" - while a caller naturally types the whole word.
    // Plain substring matching answered "Amplifier" with nothing at all
    // across a 598-model catalog, which reads as an empty device rather than
    // as a near miss.
    let (server, _) = server();
    let ids = |result: &Value| {
        result["models"]
            .as_array()
            .expect("models")
            .iter()
            .map(|model| model["id"].as_u64().expect("id"))
            .collect::<Vec<_>>()
    };
    let search = |query: &str| {
        let server = &server;
        let query = query.to_string();
        async move {
            server
                .execute(
                    &route(),
                    "list_models",
                    Some(json!({ "query": query }).as_object().unwrap().clone()),
                )
                .await
                .unwrap()
        }
    };

    assert_eq!(ids(&search("amplifier").await), vec![3]);
    assert_eq!(ids(&search("cabinet").await), vec![4]);
    assert_eq!(ids(&search("equalizer").await), vec![5]);
    // The abbreviations themselves must keep working.
    assert_eq!(ids(&search("guitar amp").await), vec![3]);
    assert_eq!(ids(&search("eq").await), vec![5]);
    // Widening the category match must not start matching everything.
    assert!(ids(&search("reverb").await).is_empty());
    // A model's real-world inspiration is deliberately not in the catalog, so
    // this stays empty rather than being faked with a trademark mapping.
    assert!(ids(&search("ac30").await).is_empty());
    // Name matching is unchanged.
    assert_eq!(ids(&search("topboost").await), vec![3]);
}

#[tokio::test]
async fn model_query_is_not_forwarded_to_parameterless_gateway_method() {
    let (server, backend) = server();
    let result = server
        .execute(
            &route(),
            "list_models",
            Some(json!({"query":"delay"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result["models"].as_array().unwrap().len(), 1);
    assert_eq!(result["models"][0]["id"], 1);
    assert_eq!(result["models"][0]["name"], "Vintage Delay");
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.listModels");
    assert!(calls[0].1.is_empty());
}

#[tokio::test]
async fn preview_expected_value_is_validated_and_forwarded() {
    let (server, backend) = server();
    server
        .execute(
            &route(),
            "preview_parameter",
            Some(
                json!({
                    "row":0,"column":1,"parameter_index":2,"value":0.6,
                    "expected_value":0.4,"expected_model_id":41,"expected_scene":1,"expected_preset_identity": "fixture-native-A", "expected_preset_name":"Test"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.previewParameter");
    assert_eq!(calls[0].1["expectedValue"], 0.4);
    assert_eq!(calls[0].1["parameterIndex"], 2);
}

#[tokio::test]
async fn newer_device_actions_enforce_confirmation_and_screen_bounds() {
    let (server, backend) = server();

    assert!(
        server
            .execute(
                &route(),
                "set_device_name",
                Some(
                    json!({"name":"Stage QC","confirm_persistent_write":false})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .is_err()
    );
    assert!(
        server
            .execute(
                &route(),
                "tap_screen",
                Some(
                    json!({"x":800,"y":100,"confirm_risky_operation":true})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .is_err()
    );
    for args in [
        json!({"x":100,"y":100,"confirm_risky_operation":false}),
        json!({"x":1.5,"y":100,"confirm_risky_operation":true}),
        json!({"x":100,"y":2.5,"confirm_risky_operation":true}),
    ] {
        assert!(
            server
                .execute(
                    &route(),
                    "tap_screen",
                    Some(args.as_object().unwrap().clone()),
                )
                .await
                .is_err()
        );
    }
    assert!(backend.0.lock().unwrap().is_empty());

    server
        .execute(
            &route(),
            "tap_screen",
            Some(
                json!({"x":799,"y":479,"confirm_risky_operation":true})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.tapScreen");
    assert_eq!(
        calls[0].1,
        json!({"x":799,"y":479}).as_object().unwrap().clone()
    );
}

#[tokio::test]
async fn midi_out_messages_are_bounded_and_forwarded_without_schema_drift() {
    let (server, backend) = server();
    let invalid = json!({
        "source": 0,
        "messages": [{"type":1,"channel":17,"param1":0,"param2":0,"param3":0}],
        "expected_preset_identity": "fixture-native-A", "expected_preset_name": "Test"
    });
    assert!(
        server
            .execute(
                &route(),
                "set_midi_out",
                Some(invalid.as_object().unwrap().clone()),
            )
            .await
            .is_err()
    );
    assert!(backend.0.lock().unwrap().is_empty());

    let messages = json!([{"type":1,"channel":16,"param1":119,"param2":1,"param3":0}]);
    server
        .execute(
            &route(),
            "set_midi_out",
            Some(
                json!({"source":0,"messages":messages,"expected_preset_identity": "fixture-native-A", "expected_preset_name":"Test"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.setMidiOut");
    assert_eq!(calls[0].1["messages"], messages);
    assert_eq!(calls[0].1["expectedPresetName"], "Test");
}

#[tokio::test]
async fn midi_out_empty_rows_keep_their_position_through_the_tool() {
    let (server, backend) = server();
    let call = |tool: &'static str, messages: serde_json::Value| {
        let mut arguments = json!({
            "messages": messages,
            "expected_preset_identity": "fixture-native-A", "expected_preset_name": "Test"
        });
        if tool == "set_midi_out" {
            arguments["source"] = json!(2);
        } else {
            arguments["expected_messages"] = json!([]);
        }
        let server = &server;
        async move {
            server
                .execute(&route(), tool, Some(arguments.as_object().unwrap().clone()))
                .await
        }
    };
    let row = |kind: u32, param1: u32| json!({"type":kind,"channel":1,"param1":param1,"param2":0,"param3":0});
    // Observed on a CorOS 4.1.0 unit, 2026-10-07: a type 0 row between two active
    // rows keeps its index, so the tool forwards the rows exactly as sent.
    let messages = json!([row(1, 10), row(0, 0), row(3, 30), row(0, 0), row(2, 50)]);
    call("set_midi_out", messages.clone()).await.unwrap();
    call("set_preset_load_midi_out", messages.clone())
        .await
        .unwrap();
    {
        let calls = backend.0.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1["messages"], messages);
        assert_eq!(calls[1].1["messages"], messages);
    }
    // An undefined type stays refused, beside an empty row or not, and nothing is sent.
    assert!(
        call("set_midi_out", json!([row(1, 1), row(0, 0), row(4, 1)]))
            .await
            .is_err()
    );
    assert!(
        call("set_preset_load_midi_out", json!([row(0, 0), row(5, 1)]))
            .await
            .is_err()
    );
    assert_eq!(backend.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn read_only_device_introspection_excludes_persistent_identity_and_maps_safe_rpcs() {
    let (server, backend) = server();
    assert!(
        server
            .execute(&route(), "get_device_identity", None)
            .await
            .is_err()
    );
    server
        .execute(&route(), "get_inhibited_modules", None)
        .await
        .unwrap();
    server
        .execute(&route(), "capture_screen", None)
        .await
        .unwrap();
    server
        .execute(&route(), "get_device_diagnostics", None)
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.0).collect::<Vec<_>>(),
        [
            "device.inhibitedModules",
            "device.captureScreen",
            "device.diagnostics"
        ]
    );
}

#[tokio::test]
async fn nullable_grid_guards_are_forwarded_instead_of_dropped() {
    let (server, backend) = server();
    server
        .execute(
            &route(),
            "set_chain_split",
            Some(
                json!({
                    "row":0,"split_column":null,"mix_column":null,
                    "expected_split_column":2,"expected_mix_column":null,
                    "expected_preset_identity": "fixture-native-A", "expected_preset_name":"Clean"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    {
        let calls = backend.0.lock().unwrap();
        assert_eq!(calls[0].0, "device.setChainSplit");
        assert!(calls[0].1.contains_key("splitColumn"));
        assert!(calls[0].1["splitColumn"].is_null());
        assert!(calls[0].1["expectedMixColumn"].is_null());
    }
    assert!(
        server
            .execute(
                &route(),
                "set_chain_split",
                Some(
                    json!({
                        "row":1,"split_column":null,"mix_column":null,
                        "expected_split_column":null,"expected_mix_column":null,
                        "expected_preset_identity": "fixture-native-A", "expected_preset_name":"Clean"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn backend_errors_preserve_code_message_and_retryability() {
    let server = QcMcp::new(Arc::new(FailingBackend));
    let error = server
        .execute(&route(), "get_current_preset", None)
        .await
        .unwrap_err();
    let detail: Value = serde_json::from_str(&error).unwrap();
    assert_eq!(detail["code"], "device_unavailable");
    assert_eq!(detail["message"], "USB disconnected");
    assert_eq!(detail["retryable"], true);
}

#[tokio::test]
async fn model_results_remove_persistent_device_identifiers_recursively() {
    struct IdentityBackend;
    #[async_trait]
    impl QcBackend for IdentityBackend {
        async fn request(
            &self,
            _: &PrincipalRoute,
            _: &'static str,
            _: Map<String, Value>,
        ) -> Result<Value, BackendError> {
            Ok(json!({
                "deviceName":"Test QC",
                "serialNumber":"private",
                "presetName":"Clean",
                "blocks":[],
                "nested":{"deviceSerial":"private","kept":true}
            }))
        }
    }
    let result = QcMcp::new(Arc::new(IdentityBackend))
        .execute(&route(), "get_current_preset", None)
        .await
        .unwrap();
    assert!(result.get("deviceName").is_none());
    assert!(result.get("serialNumber").is_none());
    assert!(result["nested"].get("deviceSerial").is_none());
    assert_eq!(result["nested"]["kept"], true);
}

#[tokio::test]
async fn backend_results_reject_contradictory_action_outcomes() {
    let server = QcMcp::new(Arc::new(MalformedResultBackend));
    let error = server
        .execute(&route(), "get_current_preset", None)
        .await
        .unwrap_err();
    assert!(error.contains("malformed"));
}

#[tokio::test]
async fn add_block_plugin_readback_reaches_the_tool_result_intact() {
    const NOTE: &str = "The Quad Cortex added this plugin block bypassed. QC Remote cannot read plugin ownership; if the plugin is not licensed on this unit, the device shows it locked.";
    struct PluginBackend;
    #[async_trait]
    impl QcBackend for PluginBackend {
        async fn request(
            &self,
            _: &PrincipalRoute,
            rpc: &'static str,
            _: Map<String, Value>,
        ) -> Result<Value, BackendError> {
            assert_eq!(rpc, "device.addBlock");
            Ok(json!({
                "accepted": true,
                "verified": true,
                "verification": "authoritative_readback",
                "changed": true,
                "detail": format!("addBlock sent to the Quad Cortex. {NOTE}"),
                "snapshot": {"presetName": "Clean", "blocks": [
                    {"id": "added", "modelId": 5024, "name": "Plugin", "kind": "Model",
                     "plugin": true, "row": 0, "column": 1, "bypassed": true}
                ]},
                "pluginBlockReadback": {
                    "plugin": true, "bypassed": true, "bypassRequested": false, "note": NOTE
                }
            }))
        }
    }
    let result = QcMcp::new(Arc::new(PluginBackend))
        .execute(
            &route(),
            "add_block",
            Some(
                json!({
                    "row": 0, "column": 1, "model_id": 5024, "expected_model_id": null,
                    "expected_preset_name": "Clean", "expected_preset_identity": "fixture-native-A"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("a plugin add is not refused by the MCP layer");
    assert_eq!(
        result["pluginBlockReadback"],
        json!({"plugin": true, "bypassed": true, "bypassRequested": false, "note": NOTE})
    );
    assert!(result["detail"].as_str().unwrap().ends_with(NOTE));
    assert_eq!(result["verified"], true);
}

#[tokio::test]
async fn split_mute_forwards_desired_and_expected_state() {
    let (server, backend) = server();
    server
        .execute(
            &route(),
            "set_split_mute",
            Some(
                json!({
                    "row":2,"muted":true,"expected_muted":false,"expected_scene":3,
                    "expected_preset_identity": "fixture-native-A", "expected_preset_name":"Clean"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[0].0, "device.setSplitMute");
    assert_eq!(calls[0].1["muted"], true);
    assert_eq!(calls[0].1["expectedMuted"], false);
    assert_eq!(calls[0].1["expectedScene"], 3);
    assert_eq!(calls[0].1["expectedPresetName"], "Clean");
}

#[tokio::test]
async fn scene_management_validates_and_preserves_nullable_labels() {
    let (server, backend) = server();
    server
        .execute(
            &route(),
            "set_scene_label",
            Some(
                json!({"scene":2,"label":null,"expected_label":"Scene C","expected_preset_identity": "fixture-native-A", "expected_preset_name":"Lead"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    {
        let calls = backend.0.lock().unwrap();
        assert_eq!(calls[0].0, "device.setSceneLabel");
        assert!(calls[0].1.contains_key("label"));
        assert!(calls[0].1["label"].is_null());
    }

    server
        .execute(
            &route(),
            "copy_scene",
            Some(
                json!({"from_scene":3,"to_scene":3,"swap":false,"expected_preset_identity": "fixture-native-A", "expected_preset_name":"Lead"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls[1].0, "device.copyScene");
    assert_eq!(calls[1].1["fromScene"], 3);
    assert_eq!(calls[1].1["toScene"], 3);
}

#[tokio::test]
async fn canonical_label_and_identity_constraints_apply_before_dispatch() {
    let (server, backend) = server();
    let mut arguments = json!({"scene":0, "label":"Lead", "expected_label":"",
        "expected_preset_name":"Clean", "expected_preset_identity":"fixture-native-A"});
    server
        .execute(&route(), "set_scene_label", arguments.as_object().cloned())
        .await
        .unwrap();
    assert_eq!(backend.0.lock().unwrap()[0].1["expectedLabel"], "");
    arguments["expected_preset_identity"] = json!("x".repeat(1025));
    assert!(
        server
            .execute(&route(), "set_scene_label", arguments.as_object().cloned())
            .await
            .is_err()
    );
    assert_eq!(
        backend.0.lock().unwrap().len(),
        1,
        "oversized identity reached backend"
    );
}

#[tokio::test]
async fn metronome_beat_count_is_bounded_by_selected_time_signature() {
    let (server, backend) = server();
    let mut arguments = json!({"led_enabled":null,"volume_db":null,"running":null,"pan":null,
        "time_signature":"2/4","subdivision":null,"sound":null,"routing":null,
        "beats":["NORMAL","NORMAL","NORMAL"],"confirm_persistent_write":true});
    assert!(
        server
            .execute(
                &route(),
                "set_tempo_metronome",
                arguments.as_object().cloned()
            )
            .await
            .is_err()
    );
    assert!(backend.0.lock().unwrap().is_empty());
    arguments["beats"] = json!(["NORMAL", "ACCENT 1"]);
    server
        .execute(
            &route(),
            "set_tempo_metronome",
            arguments.as_object().cloned(),
        )
        .await
        .unwrap();
    assert_eq!(backend.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn parameter_model_identity_is_required_and_forwarded_before_backend_dispatch() {
    let (server, backend) = server();
    let mut args = json!({"row":0,"column":1,"parameter_index":2,"value":0.6,
        "expected_value":0.4,"expected_scene":1,"expected_preset_identity":"fixture-native-A",
        "expected_preset_name":"Test"});
    for invalid in [Value::Null, json!(0), json!(-1), json!(1.5), json!(true)] {
        args["expected_model_id"] = invalid;
        assert!(
            server
                .execute(&route(), "set_parameter", args.as_object().cloned())
                .await
                .is_err()
        );
    }
    args.as_object_mut().unwrap().remove("expected_model_id");
    assert!(
        server
            .execute(&route(), "set_parameter", args.as_object().cloned())
            .await
            .is_err()
    );
    assert!(backend.0.lock().unwrap().is_empty());
    args["expected_model_id"] = json!(41);
    server
        .execute(&route(), "set_parameter", args.as_object().cloned())
        .await
        .unwrap();
    assert_eq!(backend.0.lock().unwrap()[0].1["expectedModelId"], 41);
}
