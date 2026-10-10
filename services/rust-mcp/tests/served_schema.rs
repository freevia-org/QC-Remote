//! The MCP schema a client actually receives, asserted over the real transport.
//!
//! Regression: `set_block_footswitch` can arrive at a client as
//! `"footswitch": {}` with a truncated `required` array, which makes every
//! action that needs an explicit JSON `null` unreachable — clearing a
//! footswitch, assigning one to a block that has none, `load_capture` into an
//! empty cell, and clearing a chain split. The generator was already correct
//! and so was the in-process `ActionSpec::tool()` builder, so asserting either
//! of those would not have caught this. This test drives `initialize` +
//! `tools/list` through the same axum router the app serves and asserts the
//! bytes that reach the wire against the contract itself.

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use qc_remote_mcp::{BackendError, PrincipalRoute, QcBackend, QcMcp, mcp_router};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Stub;

#[async_trait]
impl QcBackend for Stub {
    async fn request(
        &self,
        _: &PrincipalRoute,
        _: &'static str,
        _: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        Err(BackendError::unavailable("schema-only test backend"))
    }
}

async fn post(
    router: &axum::Router,
    body: Value,
    session: Option<&str>,
) -> (Option<String>, String) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, "127.0.0.1:41000")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(id) = session {
        builder = builder.header("mcp-session-id", id);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let id = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("body");
    (id, String::from_utf8_lossy(&bytes).into_owned())
}

/// Streamable HTTP answers as an SSE stream, so pull the JSON-RPC payload out.
fn payload(body: &str) -> Value {
    let marker = format!("{{{}", "\"jsonrpc\"");
    let start = body.find(&marker).expect("jsonrpc payload");
    let line = body[start..].lines().next().expect("payload line");
    serde_json::from_str(line).expect("payload json")
}

async fn served_tools() -> Vec<Value> {
    let backend: Arc<dyn QcBackend> = Arc::new(Stub);
    let router = mcp_router(move || Ok(QcMcp::new(backend.clone())));
    let (session, _) = post(
        &router,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "served-schema-test", "version": "1"}
            }
        }),
        None,
    )
    .await;
    let session = session.expect("mcp-session-id");
    post(
        &router,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        Some(&session),
    )
    .await;
    let (_, listed) = post(
        &router,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        Some(&session),
    )
    .await;
    payload(&listed)["result"]["tools"]
        .as_array()
        .expect("served tools")
        .clone()
}

fn contract() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/public/qc-actions.v1.json");
    serde_json::from_slice(&std::fs::read(&path).expect("qc-actions contract")).expect("contract")
}

/// The declared types a value may take, whether the schema spells that as a
/// `type` array or as an `anyOf` union.
fn declared_types(schema: &Value) -> Vec<String> {
    let from_type = |value: &Value| match value {
        Value::String(name) => vec![name.clone()],
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    let mut types = from_type(&schema["type"]);
    if let Some(branches) = schema["anyOf"].as_array() {
        for branch in branches {
            types.extend(from_type(&branch["type"]));
        }
    }
    types
}

fn served_tool<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("{name} is not served over MCP"))
}

#[tokio::test]
async fn served_schema_declares_every_nullable_kind_as_a_union_with_null() {
    let tools = served_tools().await;
    let contract = contract();
    let mut checked = 0usize;

    for action in contract["actions"].as_array().expect("actions") {
        let name = action["name"].as_str().expect("action name");
        let served = &served_tool(&tools, name)["inputSchema"]["properties"];
        for (property, kind) in action["properties"].as_object().expect("properties") {
            let kind = kind.as_str().unwrap_or_default();
            if !kind.starts_with("nullable-") {
                continue;
            }
            checked += 1;
            let schema = &served[property];
            assert!(
                schema.as_object().is_some_and(|object| !object.is_empty()),
                "{name}.{property} ({kind}) is served as an empty schema, so a client \
                 cannot know null is legal: {schema}"
            );
            let types = declared_types(schema);
            assert!(
                types.iter().any(|declared| declared == "null"),
                "{name}.{property} ({kind}) must declare null in its served type; got {schema}"
            );
            assert!(
                types.iter().any(|declared| declared != "null"),
                "{name}.{property} ({kind}) must also declare its non-null type; got {schema}"
            );
        }
    }

    assert!(
        checked > 0,
        "the contract declares no nullable kinds, so this test proved nothing"
    );

    // The exact shape this must serve, pinned so it cannot silently drift.
    // Spelled as an `anyOf` union rather than a `type` array: see `nullable()`
    // in src/actions.rs. A `type` array is legal JSON Schema and was what this
    // server used to serve, but clients translate it to `{}` and then send the
    // string `"null"` for want of a type to coerce to. `anyOf` carries the
    // range and the null through intact.
    let footswitch = served_tool(&tools, "set_block_footswitch");
    for property in ["footswitch", "expected_footswitch"] {
        assert_eq!(
            footswitch["inputSchema"]["properties"][property],
            json!({"anyOf": [{"type": "integer", "minimum": 0, "maximum": 7}, {"type": "null"}]}),
            "{property} must stay a nullable integer union on the wire"
        );
    }
    assert_eq!(
        served_tool(&tools, "set_block_footswitch")["inputSchema"]["properties"]["expected_assignment_type"],
        json!({
            "anyOf": [
                {"type": "string", "enum": ["primary", "secondary"]},
                {"type": "null"}
            ]
        }),
        "an unassigned block is addressed with a null assignment type, so its \
         enum and its null must both survive to the client"
    );
}

/// The regression itself: a `type` array is what clients collapse to `{}`, so
/// no served property may spell its type that way, however it came to be.
#[tokio::test]
async fn no_served_property_declares_its_type_as_an_array() {
    let tools = served_tools().await;
    let mut offenders = Vec::new();
    let mut checked = 0usize;

    fn array_typed(schema: &Value, path: &str, offenders: &mut Vec<String>, checked: &mut usize) {
        *checked += 1;
        if schema["type"].is_array() {
            offenders.push(format!("{path}: {schema}"));
        }
        for key in ["anyOf", "allOf", "oneOf"] {
            if let Some(branches) = schema[key].as_array() {
                for (index, branch) in branches.iter().enumerate() {
                    array_typed(
                        branch,
                        &format!("{path}.{key}[{index}]"),
                        offenders,
                        checked,
                    );
                }
            }
        }
        if let Some(properties) = schema["properties"].as_object() {
            for (name, child) in properties {
                array_typed(child, &format!("{path}.{name}"), offenders, checked);
            }
        }
        if schema["items"].is_object() {
            array_typed(
                &schema["items"],
                &format!("{path}.items"),
                offenders,
                checked,
            );
        }
    }

    for tool in &tools {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        array_typed(&tool["inputSchema"], name, &mut offenders, &mut checked);
    }

    assert!(checked > tools.len(), "the walk visited no nested schemas");
    assert!(
        offenders.is_empty(),
        "these served schemas spell their type as an array, which clients drop \
         to an empty schema - use the `anyOf` union instead:\n{}",
        offenders.join("\n")
    );
}

#[tokio::test]
async fn served_required_arrays_match_the_contract() {
    let tools = served_tools().await;
    let contract = contract();

    for action in contract["actions"].as_array().expect("actions") {
        let name = action["name"].as_str().expect("action name");
        let tool = served_tool(&tools, name);
        let expected = action["required"].as_array().expect("contract required");
        let served = tool["inputSchema"]["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} is served without a required array"));
        assert_eq!(
            served, expected,
            "{name} serves a required array that disagrees with the contract"
        );
        // Every declared property must also reach the client with a usable schema.
        for property in action["properties"].as_object().expect("properties").keys() {
            let schema = &tool["inputSchema"]["properties"][property];
            assert!(
                schema.as_object().is_some_and(|object| !object.is_empty()),
                "{name}.{property} is served without a schema"
            );
        }
    }
}

#[tokio::test]
async fn served_io_schemas_include_port_dependencies_and_non_null_sparse_branches() {
    let tools = served_tools().await;
    for name in [
        "set_input_port",
        "set_output_port",
        "set_usb_port",
        "set_output_pairing",
    ] {
        let all_of = served_tool(&tools, name)["inputSchema"]["allOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} has no dependent schema constraints"));
        assert!(
            all_of
                .iter()
                .any(|constraint| constraint["anyOf"].is_array()),
            "{name} must advertise its sparse non-null alternatives"
        );
    }

    let input = &served_tool(&tools, "set_input_port")["inputSchema"];
    assert!(input.to_string().contains("input_port_id"));
    assert!(input.to_string().contains("\"type\":\"null\""));
    let output = &served_tool(&tools, "set_output_port")["inputSchema"];
    assert!(output.to_string().contains("output_port_id"));
    assert!(output.to_string().contains("ground_lift"));
    assert!(output.to_string().contains("mute"));
}

fn normalize_schema(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(normalize_schema).collect()),
        Value::Object(object) => {
            let mut result: Map<String, Value> = object
                .iter()
                .map(|(key, value)| (key.clone(), normalize_schema(value)))
                .collect();
            if let Some(branches) = result.get("anyOf").and_then(Value::as_array).cloned()
                && branches.len() == 2
                && branches[1] == json!({"type":"null"})
                && branches[0]["type"].is_string()
            {
                result.remove("anyOf");
                result.extend(branches[0].as_object().unwrap().clone());
                result.insert("type".into(), json!([branches[0]["type"], "null"]));
                if let Some(values) = result.get_mut("enum").and_then(Value::as_array_mut) {
                    values.push(Value::Null);
                }
            }
            Value::Object(result)
        }
        Value::Number(number)
            if number.is_f64()
                && number
                    .as_f64()
                    .is_some_and(|n| n.fract() == 0.0 && n.abs() < i64::MAX as f64) =>
        {
            json!(number.as_f64().unwrap() as i64)
        }
        _ => value.clone(),
    }
}

#[tokio::test]
async fn every_served_schema_preserves_all_canonical_constraints() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/typescript/qc-core/src/generated-actions.ts");
    let source = std::fs::read_to_string(path).unwrap();
    let data = source
        .split_once("export const SHARED_QC_ACTIONS = ")
        .unwrap()
        .1;
    let actions = serde_json::Deserializer::from_str(data)
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap();
    let tools = served_tools().await;
    let mut mismatches = Vec::new();
    for action in actions.as_array().unwrap() {
        let name = action["name"].as_str().unwrap();
        let actual = &served_tool(&tools, name)["inputSchema"];
        if normalize_schema(actual) != normalize_schema(&action["inputSchema"]) {
            mismatches.push(name);
        }
    }
    assert_eq!(
        actions.as_array().unwrap().len(),
        contract()["actions"].as_array().unwrap().len()
    );
    assert!(
        mismatches.is_empty(),
        "{} served action schemas lose canonical constraints: {}",
        mismatches.len(),
        mismatches.join(", ")
    );
}
