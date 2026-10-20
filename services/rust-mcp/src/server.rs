use crate::generated_result_kinds;
use crate::{
    ACTIONS, ASSISTANT_GUIDE, ActionSpec, BackendError, Classification, MCP_INSTRUCTIONS,
    PrincipalRoute, QcBackend,
};
use axum::Router;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
        ResourceContents, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::{NotificationContext, RequestContext},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde_json::{Map, Value, json};
use std::sync::{Arc, OnceLock};

const DEVICE_KNOWLEDGE_GUIDE: &str = include_str!("../../../skills/qc-device-knowledge/SKILL.md");
const DEVICE_KNOWLEDGE_CATALOG: &str =
    include_str!("../../../skills/qc-device-knowledge/references/data/qc-model-lineage.json");
const DEVICE_KNOWLEDGE_SEARCH_TOOL: &str = "search_qc_device_knowledge";

/// The name this server reports at `initialize`. It is the product name the
/// cloud connector reports too; the old "NDSP Quad Cortex" presented a vendor
/// abbreviation, at odds with TRADEMARKS.md's "not an official Neural DSP
/// product".
const SERVER_NAME: &str = "QC Remote";

/// Every MCP resource that is answered by a backend request. `read_resource`
/// resolves each through [`resource_request`].
const BACKEND_RESOURCES: &[&str] = &["qc://status", "qc://current-preset", "qc://models"];

const TARGET_GUARD: &str = "expectedBrokerTarget";

fn target_guard_schema() -> Value {
    json!({
        "type":"object", "additionalProperties":false,
        "required":["source","sessionId","generation"],
        "properties":{
            "source":{"type":"string","enum":["physical","demo"]},
            "sessionId":{"type":"string","minLength":1,"maxLength":256},
            "generation":{"type":"integer","minimum":1,"maximum":9007199254740991_u64}
        },
        "description":"Copy brokerTarget from the accepted status to keep this request on that target across queues and hot-plug."
    })
}

fn target_guarded_tool(spec: &ActionSpec) -> Tool {
    let mut tool = spec.tool();
    let schema = Arc::make_mut(&mut tool.input_schema);
    schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("generated properties")
        .insert(TARGET_GUARD.into(), target_guard_schema());
    tool
}

fn take_target_guard(args: &mut Map<String, Value>) -> Result<Option<crate::BrokerTarget>, String> {
    let Some(value) = args.remove(TARGET_GUARD) else {
        return Ok(None);
    };
    let valid = value.as_object().is_some_and(|object| object.len() == 3)
        && crate::schema::matches(&value, &target_guard_schema())
        && value["sessionId"]
            .as_str()
            .is_some_and(|id| !id.trim().is_empty());
    if !valid {
        return Err("invalid expectedBrokerTarget".into());
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|_| "invalid expectedBrokerTarget".into())
}

fn stamp_target_guard_capability(rpc: &str, mut value: Value) -> Value {
    if rpc == "system.status"
        && let Some(object) = value.as_object_mut()
    {
        let capabilities = object.entry("capabilities").or_insert_with(|| json!([]));
        if !capabilities.is_array() {
            *capabilities = json!([]);
        }
        let values = capabilities.as_array_mut().expect("capability array");
        if !values.iter().any(|value| value == "mcpBrokerTargetGuard") {
            values.push(json!("mcpBrokerTargetGuard"));
        }
    }
    value
}

/// Every backend-answered resource as `(uri, rpc)`, resolved exactly as
/// `read_resource` resolves it, so a host or bridge that must tell resource
/// reads apart derives the set from here rather than keeping its own list.
pub fn resource_rpcs() -> Vec<(&'static str, &'static str)> {
    BACKEND_RESOURCES
        .iter()
        .filter_map(|uri| resource_request(uri).map(|(rpc, _)| (*uri, rpc)))
        .collect()
}

/// The backend request a resource read makes, or `None` for a URI that is not
/// a backend-answered resource.
fn resource_request(uri: &str) -> Option<(&'static str, Map<String, Value>)> {
    match uri {
        "qc://status" => Some(("system.status", Map::new())),
        uri => resource_action(uri).map(|spec| (spec.rpc, gateway_params(spec, Map::new()))),
    }
}

tokio::task_local! {
    static SERVING_RESOURCE_READ: ();
}

/// True while the current task's backend request is serving an MCP resource
/// read rather than a tool call. A backend may use it to treat the two
/// differently by origin; an rpc name alone cannot tell them apart, because
/// `qc://models` and the `list_models` tool share `device.listModels`.
pub fn serving_resource_read() -> bool {
    SERVING_RESOURCE_READ.try_with(|_| ()).is_ok()
}

/// Runs `request` as a resource read for [`serving_resource_read`].
pub async fn as_resource_read<F: std::future::Future>(request: F) -> F::Output {
    SERVING_RESOURCE_READ.scope((), request).await
}

/// Attribution carried on every device-knowledge search result.
const DEVICE_KNOWLEDGE_CATALOG_SOURCE: &str = "Neural DSP Quad Cortex manual, device list, plugin pages and CorOS release notes \
     (neuraldsp.com), structured by QC Remote. Not an official Neural DSP product.";

#[derive(Clone)]
pub struct QcMcp {
    backend: Arc<dyn QcBackend>,
    fixed_route: Option<PrincipalRoute>,
    presence: Option<Arc<dyn ClientPresence>>,
}

/// A transport-owned observation of a successfully initialized MCP session.
/// Implementations may release their registration in Drop; the shared handle
/// then lives exactly as long as this session's handler clones.
pub trait ClientPresence: Send + Sync {
    fn initialized(&self, client: &Implementation);
    fn active(&self) {}
}

struct ActivityPulse(Option<tokio::task::JoinHandle<()>>);
impl Drop for ActivityPulse {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

impl QcMcp {
    fn activity(&self) -> ActivityPulse {
        ActivityPulse(self.presence.clone().map(|presence| {
            presence.active();
            tokio::spawn(async move {
                let mut pulse = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    pulse.tick().await;
                    presence.active();
                }
            })
        }))
    }
    pub fn new(backend: Arc<dyn QcBackend>) -> Self {
        Self {
            backend,
            fixed_route: None,
            presence: None,
        }
    }

    /// Creates a server for a transport that already established its route.
    ///
    /// HTTP callers still receive their authenticated route through Axum's
    /// request extensions. A local stdio launcher has no HTTP request context,
    /// so it uses this constructor after authenticating to the running desktop
    /// app with the per-process loopback credential.
    pub fn for_route(backend: Arc<dyn QcBackend>, route: PrincipalRoute) -> Self {
        Self {
            backend,
            fixed_route: Some(route),
            presence: None,
        }
    }

    pub fn with_client_presence(mut self, presence: Arc<dyn ClientPresence>) -> Self {
        self.presence = Some(presence);
        self
    }

    fn principal_route(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<PrincipalRoute, String> {
        self.fixed_route
            .clone()
            .or_else(|| principal_route(context).ok())
            .ok_or_else(|| "authenticated principal/device route is required".into())
    }

    pub fn tools(&self) -> Vec<Tool> {
        ACTIONS
            .iter()
            .map(target_guarded_tool)
            .chain(std::iter::once(device_knowledge_search_tool()))
            .collect()
    }

    pub async fn execute(
        &self,
        route: &PrincipalRoute,
        name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, String> {
        let spec = ACTIONS
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("unknown tool: {name}"))?;
        let mut args = arguments.unwrap_or_default();
        let expected_broker_target = take_target_guard(&mut args)?;
        restore_stringified_nulls(spec, &mut args);
        validate(spec, &args)?;
        let model_query =
            (spec.name == "list_models").then(|| normalized_model_query(args.get("query")));
        apply_confirmation_gate(spec, &mut args)?;
        let params = gateway_params(spec, args);
        let result = self
            .backend
            .request_targeted(route, spec.rpc, params, expected_broker_target)
            .await
            .map_err(|error| {
                let mut details = backend_error_details(&error);
                details["message"] = json!(error.message);
                details.to_string()
            })?;
        validate_backend_result(spec.rpc, &result)?;
        let result = sanitize_model_result(stamp_target_guard_capability(spec.rpc, result));
        match model_query {
            Some(query) => filter_models(result, &query),
            None => Ok(result),
        }
    }
}

/// Tools whose result is a PNG. Their JSON result (`pngBase64`, `width`,
/// `height`) stays exactly as the gateway produced it, because it is the
/// documented, structured form that existing clients read. A client that only
/// renders MCP content blocks cannot show that base64 text, so the same bytes
/// are also returned as a standard image block.
const SCREEN_IMAGE_TOOLS: [&str; 2] = ["capture_screen", "get_preset_screenshot"];

/// First characters of any base64 string that encodes the eight-byte PNG
/// signature (89 50 4E 47 0D 0A 1A 0A). A result that does not start with it
/// is not labelled `image/png`.
///
/// Limitations, stated rather than hidden: this is a signature check only. It
/// does not decode the base64, check that the bytes form a complete or valid
/// PNG, or compare them with `width` and `height`; the gateway that produced
/// the result is what validates the image. The block exists only on this
/// embedded server (direct and stdio). The cloud connector builds its own
/// text-only results and never carries it.
const PNG_BASE64_PREFIX: &str = "iVBORw0KGgo";

/// The base64 PNG of a screen-image result, or `None` when the result does not
/// carry one in the gateway's shape.
fn screen_image_data(value: &Value) -> Option<&str> {
    value
        .get("pngBase64")?
        .as_str()
        .filter(|data| data.starts_with(PNG_BASE64_PREFIX))
}

/// Build the MCP result for a successful tool call: the structured result
/// every tool returns, plus - for the two screen-image tools only - one image
/// content block carrying the same PNG bytes after the text block.
fn tool_result(name: &str, value: Value) -> CallToolResult {
    let image = SCREEN_IMAGE_TOOLS
        .contains(&name)
        .then(|| screen_image_data(&value).map(str::to_owned))
        .flatten();
    let mut result = CallToolResult::structured(value);
    if let Some(data) = image {
        result.content.push(ContentBlock::image(data, "image/png"));
    }
    result
}

fn validate_backend_result(method: &str, result: &Value) -> Result<(), String> {
    const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
    if serde_json::to_vec(result)
        .map_err(|_| format!("{method} returned a non-JSON result"))?
        .len()
        > MAX_RESULT_BYTES
    {
        return Err(format!(
            "{method} returned a result larger than the gateway frame limit"
        ));
    }
    generated_result_kinds::validate_result(method, result)
}

/// Remove persistent hardware identifiers before a result reaches a model.
///
/// The field list is the contract's `removedIdentifierFields`, the same one
/// the cloud Worker and the native relay hosts read, so a field added there is
/// stripped here too instead of leaking through the local server only.
fn sanitize_model_result(mut value: Value) -> Value {
    fn visit(value: &mut Value) {
        match value {
            Value::Object(object) => {
                object.retain(|name, _| {
                    !qc_relay_protocol::REMOVED_IDENTIFIER_FIELDS
                        .contains(&name.to_ascii_lowercase().as_str())
                });
                for child in object.values_mut() {
                    visit(child);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(visit),
            _ => {}
        }
    }
    visit(&mut value);
    value
}

/// `list_models`' query as the filter compares it: absent, null or blank is
/// empty (no filtering), otherwise trimmed and lowercased.
fn normalized_model_query(query: Option<&Value>) -> String {
    query
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

fn filter_models(mut result: Value, query: &str) -> Result<Value, String> {
    if query.is_empty() {
        return Ok(result);
    }
    let models = result
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "device.listModels returned a malformed response".to_string())?;
    models.retain(|model| {
        let field = |name: &str| {
            model
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase()
        };
        ["name", "basedOn"]
            .iter()
            .any(|name| field(name).contains(query))
            || category_matches(&field("category"), query)
    });
    Ok(result)
}

/// Whether a lowercase query selects a category.
///
/// Categories use the catalog's standard abbreviations - "Guitar Amp",
/// "Guitar Cab", "EQ" - while a caller naturally types the whole word. Plain
/// substring matching answered "Amplifier" with nothing at all across a
/// 598-model catalog, which reads as an empty device rather than as a near
/// miss. A query word and a category word therefore match when either is a
/// prefix of the other, so "Amplifier" finds "Guitar Amp" and "Cabinet" finds
/// "Guitar Cab". This only ever widens the substring match; it never rejects
/// something that already matched.
///
/// Note this cannot help a caller searching for the real-world gear a model
/// emulates ("AC30"). The catalog deliberately ships renamed models and leaves
/// `basedOn` empty for them, so no such mapping exists here to search.
fn category_matches(category: &str, query: &str) -> bool {
    if category.contains(query) {
        return true;
    }
    let words: Vec<&str> = category.split_whitespace().collect();
    let mut matched_any = false;
    for word in query.split_whitespace() {
        // One-character fragments would select nearly every category.
        if word.len() < 2 {
            return false;
        }
        if !words
            .iter()
            .any(|entry| entry.len() >= 2 && (word.starts_with(entry) || entry.starts_with(word)))
        {
            return false;
        }
        matched_any = true;
    }
    matched_any
}

impl ServerHandler for QcMcp {
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        if let (Some(presence), Some(info)) = (&self.presence, context.peer.peer_info()) {
            presence.initialized(&info.client_info);
        }
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
        .with_instructions(MCP_INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let _activity = self.activity();
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if name == DEVICE_KNOWLEDGE_SEARCH_TOOL {
            return Some(device_knowledge_search_tool());
        }
        ACTIONS
            .iter()
            .find(|a| a.name == name)
            .map(target_guarded_tool)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let _activity = self.activity();
        if request.name.as_ref() == DEVICE_KNOWLEDGE_SEARCH_TOOL {
            return Ok(match search_device_knowledge(request.arguments) {
                Ok(value) => CallToolResult::structured(value),
                Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
            }
            .into());
        }
        let route = match self.principal_route(&context) {
            Ok(route) => route,
            Err(message) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into());
            }
        };
        match self
            .execute(&route, request.name.as_ref(), request.arguments)
            .await
        {
            Ok(value) => Ok(tool_result(request.name.as_ref(), value).into()),
            Err(message) if message.starts_with("unknown tool:") => {
                Err(McpError::invalid_params(message, None))
            }
            Err(message) => Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into()),
        }
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let _activity = self.activity();
        Ok(ListResourcesResult::with_all_items(vec![
            resource(
                "qc://status",
                "status",
                "Connection and gateway capability status.",
            ),
            resource(
                "qc://current-preset",
                "current-preset",
                "Authoritative preset, scene, blocks, tempo, mode and dirty state.",
            ),
            resource(
                "qc://models",
                "models",
                "Models in the catalog the paired Quad Cortex published to this session; plugin ownership is not checked.",
            ),
            resource(
                "qc://assistant-guide",
                "assistant-guide",
                "How to decide when to use QC Remote and operate the connected Quad Cortex safely.",
            )
            .with_mime_type("text/markdown"),
            resource(
                "qc://device-knowledge/guide",
                "qc-device-knowledge",
                "Quad Cortex manual, device list, plugin and MIDI knowledge, tagged by source.",
            )
            .with_mime_type("text/markdown"),
            resource(
                "qc://device-knowledge/catalog",
                "qc-device-knowledge-catalog",
                &format!(
                    "Bundled {}-model device/plugin lineage catalog (based-on hardware, required plugin, CorOS history).",
                    device_knowledge_model_count()
                ),
            ),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let _activity = self.activity();
        if request.uri.as_str() == "qc://assistant-guide" {
            return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                ASSISTANT_GUIDE,
                request.uri,
            )])
            .into());
        }
        let static_reference = match request.uri.as_str() {
            "qc://device-knowledge/guide" => {
                Some((DEVICE_KNOWLEDGE_GUIDE.to_string(), "text/markdown"))
            }
            "qc://device-knowledge/catalog" => {
                Some((DEVICE_KNOWLEDGE_CATALOG.to_string(), "application/json"))
            }
            _ => None,
        };
        if let Some((contents, mime_type)) = static_reference {
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(contents, request.uri).with_mime_type(mime_type),
            ])
            .into());
        }
        let Some((rpc, params)) = BACKEND_RESOURCES
            .contains(&request.uri.as_str())
            .then(|| resource_request(request.uri.as_str()))
            .flatten()
        else {
            return Err(McpError::invalid_params("unknown QC resource", None));
        };
        let route = self
            .principal_route(&context)
            .map_err(|message| McpError::invalid_params(message, None))?;
        match as_resource_read(self.backend.request(&route, rpc, params)).await {
            Ok(value) => {
                validate_backend_result(rpc, &value).map_err(|message| {
                    McpError::internal_error(
                        message,
                        Some(json!({"code":"malformed_backend_result","retryable":false})),
                    )
                })?;
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    sanitize_model_result(stamp_target_guard_capability(rpc, value)).to_string(),
                    request.uri,
                )])
                .into())
            }
            Err(error) => Err(backend_protocol_error(error)),
        }
    }
}

fn device_knowledge_search_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": 200,
                "description": "Device/model name, real-world hardware name (e.g. an amp or pedal), category, or plugin to find in the bundled Quad Cortex catalog."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 10,
                "default": 5,
                "description": "Maximum catalog entries to return."
            }
        },
        "required": ["query"]
    });
    Tool::new(
        DEVICE_KNOWLEDGE_SEARCH_TOOL,
        format!(
            "Search QC Remote's bundled Quad Cortex device/plugin lineage catalog -- {} models joined against Neural DSP's own manual, device list, plugin pages and CorOS release notes, including which real amp/pedal (if any) a model is based on and which plugin (if any) it requires. This lookup works without a connected Quad Cortex.",
            device_knowledge_model_count()
        ),
        Arc::<JsonObject>::new(schema.as_object().expect("device knowledge search schema").clone()),
    )
    .with_annotations(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

fn search_device_knowledge(arguments: Option<Map<String, Value>>) -> Result<Value, String> {
    let arguments = arguments.unwrap_or_default();
    for name in arguments.keys() {
        if !matches!(name.as_str(), "query" | "limit") {
            return Err(format!("unexpected argument: {name}"));
        }
    }
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "query is required".to_string())?;
    if query.chars().count() > 200 || query.chars().any(char::is_control) {
        return Err("invalid query".into());
    }
    let limit = arguments
        .get("limit")
        .map(|value| value.as_u64().ok_or_else(|| "invalid limit".to_string()))
        .transpose()?
        .unwrap_or(5);
    if !(1..=10).contains(&limit) {
        return Err("invalid limit".into());
    }

    let normalized_query = normalize_search_text(query);
    if normalized_query.is_empty() {
        return Err("query must contain a letter or number".into());
    }
    let query_tokens: Vec<&str> = normalized_query.split_whitespace().collect();

    let entries = device_knowledge_entries()?;

    let matches = ranked_matches(
        entries.iter(),
        &normalized_query,
        &query_tokens,
        &[
            "device_name",
            "based_on",
            "device_category",
            "manual_category",
        ],
        limit as usize,
    )
    .into_iter()
    .map(|mut record| {
        let object = record
            .as_object_mut()
            .expect("catalog entries are objects");
        object.insert(
            "reference_source".into(),
            json!(DEVICE_KNOWLEDGE_CATALOG_SOURCE),
        );
        object.insert(
            "limitations".into(),
            json!("based_on null means no cited public product source states a hardware basis for this model."),
        );
        record
    })
    .collect::<Vec<_>>();

    Ok(json!({
        "query": query,
        "matches": matches,
        "guidance": "Each match is one catalog entry from Neural DSP's own published manual/device-list/plugin-page/CorOS-release-note content, joined by this repository's tooling. Report based_on, required_plugin and added_in_coros as stated; do not infer a hardware basis for an entry whose based_on is null."
    }))
}

/// The catalog's joined entries, parsed once per process.
fn device_knowledge_entries() -> Result<&'static [Value], String> {
    static ENTRIES: OnceLock<Option<Vec<Value>>> = OnceLock::new();
    ENTRIES
        .get_or_init(|| {
            let mut catalog = serde_json::from_str::<Value>(DEVICE_KNOWLEDGE_CATALOG).ok()?;
            match catalog.get_mut("join")?.get_mut("entries")?.take() {
                Value::Array(entries) => Some(entries),
                _ => None,
            }
        })
        .as_deref()
        .ok_or_else(|| "bundled device knowledge catalog is invalid".to_string())
}

/// How many models the bundled catalog describes, for the descriptions that
/// quote it. Counted from the catalog rather than written into the text, so a
/// catalog update cannot leave the advertised number behind.
fn device_knowledge_model_count() -> usize {
    device_knowledge_entries().map_or(0, <[Value]>::len)
}

fn ranked_matches<'a>(
    records: impl Iterator<Item = &'a Value>,
    normalized_query: &str,
    query_tokens: &[&str],
    primary_fields: &[&str],
    limit: usize,
) -> Vec<Value> {
    let mut matches = records
        .filter_map(|record| {
            let searchable = normalize_search_text(&searchable_value(record));
            let primary = record
                .as_object()
                .map(|object| {
                    primary_fields
                        .iter()
                        .filter_map(|name| object.get(*name))
                        .map(searchable_value)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .map(|value| normalize_search_text(&value))
                .unwrap_or_default();
            let matched_tokens = query_tokens
                .iter()
                .filter(|token| searchable.contains(**token))
                .count();
            if matched_tokens == 0 {
                return None;
            }
            let primary_matched_tokens = query_tokens
                .iter()
                .filter(|token| primary.contains(**token))
                .count();
            let score = matched_tokens * 10
                + usize::from(matched_tokens == query_tokens.len()) * 50
                + usize::from(searchable.contains(normalized_query)) * 100
                + primary_matched_tokens * 100
                + usize::from(primary_matched_tokens == query_tokens.len()) * 500
                + usize::from(primary.contains(normalized_query)) * 1_000;
            Some((score, searchable, record))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    matches
        .into_iter()
        .take(limit)
        .map(|(_, _, record)| record.clone())
        .collect()
}

fn searchable_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(searchable_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(values) => values
            .values()
            .map(searchable_value)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn normalize_search_text(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The read tool whose action a resource serves, for resources that mirror one.
///
/// The shared instructions tell a model to read `qc://current-preset` before
/// preset-scoped work and to copy `expected_preset_identity` and the other
/// guards from it, so the resource has to serve exactly what
/// `get_current_preset` serves. It used to read `device.snapshot`, the
/// broker's cached view, while the tool and the cloud connector read
/// `device.currentPreset`, which refreshes only when the cached Grid topology
/// is incomplete; a model following the instructions
/// then sourced its guards from older state than the write was checked
/// against. Resolving the RPC through the action makes that divergence
/// impossible to reintroduce here: the URI names a tool, not an RPC.
fn resource_action(uri: &str) -> Option<&'static ActionSpec> {
    let action = match uri {
        "qc://current-preset" => "get_current_preset",
        "qc://models" => "list_models",
        _ => return None,
    };
    ACTIONS.iter().find(|spec| spec.name == action)
}

fn resource(uri: &str, name: &str, description: &str) -> Resource {
    Resource::new(uri, name)
        .with_description(description)
        .with_mime_type("application/json")
}

/// Additive error facts shared by tool text errors and resource protocol errors.
fn backend_error_details(error: &BackendError) -> Value {
    let (retryable, user_action_required) = error.reporting_policy();
    let mut details = json!({"code":error.code,"retryable":retryable});
    if let Some(required) = user_action_required {
        details["userActionRequired"] = json!(required);
    }
    if let Some(device_code) = &error.detail_code {
        details["deviceCode"] = json!(device_code);
    }
    if let Some(device_details) = &error.details {
        details["deviceDetails"] = device_details.as_ref().clone();
    }
    details
}

fn backend_protocol_error(error: BackendError) -> McpError {
    let details = backend_error_details(&error);
    McpError::internal_error(error.message, Some(details))
}

fn principal_route(context: &RequestContext<RoleServer>) -> Result<PrincipalRoute, String> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<PrincipalRoute>())
        .cloned()
        .ok_or_else(|| "authenticated principal/device route is required".into())
}

/// Mount this behind the authentication/authorization middleware. The service is
/// intentionally not a runnable unauthenticated binary and does not terminate TLS.
pub fn mcp_router<F>(make_handler: F) -> Router
where
    F: Fn() -> Result<QcMcp, std::io::Error> + Clone + Send + Sync + 'static,
{
    mcp_router_with_config(make_handler, StreamableHttpServerConfig::default())
}

/// Mount the MCP service for an HTTPS reverse-proxy deployment while retaining
/// rmcp's DNS-rebinding protection for the configured public host names.
pub fn mcp_router_with_allowed_hosts<F>(make_handler: F, allowed_hosts: Vec<String>) -> Router
where
    F: Fn() -> Result<QcMcp, std::io::Error> + Clone + Send + Sync + 'static,
{
    mcp_router_with_config(
        make_handler,
        StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts),
    )
}

fn mcp_router_with_config<F>(make_handler: F, config: StreamableHttpServerConfig) -> Router
where
    F: Fn() -> Result<QcMcp, std::io::Error> + Clone + Send + Sync + 'static,
{
    let service = StreamableHttpService::new(
        make_handler,
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new().route_service("/mcp", service)
}

/// Some MCP clients cannot emit a JSON `null` for a union-typed parameter: they
/// keep the literal text `null` and send the four-character string. Some MCP
/// sessions have sent `expected_model_id: "null"` to
/// `add_block` (and were refused with `invalid expected_model_id`) although the
/// served schema is a correct `anyOf` with a null branch. Restore the null the
/// caller meant, but only where it is unambiguous: the property must accept
/// `null` and must reject the string `"null"`. A nullable string property
/// (`label`, `query`, `folder`...) legitimately accepts that text and is left
/// alone, and every guard keeps its meaning - the value is still checked by
/// `validate` against the canonical schema afterwards.
fn restore_stringified_nulls(spec: &ActionSpec, args: &mut Map<String, Value>) {
    let schema = spec.parsed_input_schema();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    let text = Value::String("null".into());
    for (name, value) in args.iter_mut() {
        if *value != text {
            continue;
        }
        if let Some(property) = properties.get(name)
            && crate::schema::matches(&Value::Null, property)
            && !crate::schema::matches(&text, property)
        {
            *value = Value::Null;
        }
    }
}

fn validate(spec: &ActionSpec, args: &Map<String, Value>) -> Result<(), String> {
    let schema = spec.parsed_input_schema();
    let properties = schema["properties"]
        .as_object()
        .expect("generated properties");
    for key in args.keys() {
        if !properties.contains_key(key) {
            return Err(format!("unexpected argument: {key}"));
        }
    }
    for name in schema["required"].as_array().expect("generated required") {
        let name = name.as_str().expect("required field name");
        if !args.contains_key(name) {
            return Err(format!("{name} is required"));
        }
    }
    for (name, value) in args {
        if !crate::schema::matches(value, &properties[name]) {
            return Err(format!("invalid {name}"));
        }
    }
    let constraints = Value::Object(schema.clone());
    if !crate::schema::matches(&Value::Object(args.clone()), &constraints) {
        return Err(format!("invalid dependent arguments for {}", spec.name));
    }
    Ok(())
}

fn apply_confirmation_gate(spec: &ActionSpec, args: &mut Map<String, Value>) -> Result<(), String> {
    let required = match spec.classification {
        Classification::PersistentWrite => Some("confirm_persistent_write"),
        Classification::RiskyWrite => Some("confirm_risky_operation"),
        _ => None,
    };
    if let Some(flag) = required
        && args.remove(flag) != Some(Value::Bool(true))
    {
        return Err(format!(
            "{flag}=true is required after explicit user confirmation"
        ));
    }
    Ok(())
}

fn gateway_params(spec: &ActionSpec, args: Map<String, Value>) -> Map<String, Value> {
    spec.gateway_arguments
        .iter()
        .filter_map(|(source, target)| {
            args.get(*source)
                .cloned()
                .map(|value| ((*target).into(), value))
        })
        .chain(
            spec.gateway_true_arguments
                .iter()
                .map(|name| ((*name).into(), Value::Bool(true))),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_guard_is_optional_tool_metadata_without_mutating_generated_actions() {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.name == "get_current_preset")
            .unwrap();
        let tool = target_guarded_tool(spec);
        assert!(tool.input_schema["properties"].get(TARGET_GUARD).is_some());
        assert!(
            !tool.input_schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(TARGET_GUARD))
        );
        assert!(
            spec.parsed_input_schema()["properties"]
                .get(TARGET_GUARD)
                .is_none()
        );
        let status = stamp_target_guard_capability(
            "system.status",
            json!({"capabilities":["nativeBroker"]}),
        );
        let repeated = stamp_target_guard_capability("system.status", status.clone());
        assert_eq!(status, repeated);
        assert_eq!(
            status["capabilities"],
            json!(["nativeBroker", "mcpBrokerTargetGuard"])
        );
    }

    type TargetCall = (Map<String, Value>, Option<crate::BrokerTarget>);
    struct TargetBackend(std::sync::Mutex<Vec<TargetCall>>);

    #[async_trait::async_trait]
    impl QcBackend for TargetBackend {
        async fn request(
            &self,
            _: &PrincipalRoute,
            _: &'static str,
            _: Map<String, Value>,
        ) -> Result<Value, BackendError> {
            panic!("target-aware calls must use request_targeted")
        }
        async fn request_targeted(
            &self,
            _: &PrincipalRoute,
            _: &'static str,
            params: Map<String, Value>,
            target: Option<crate::BrokerTarget>,
        ) -> Result<Value, BackendError> {
            self.0.lock().unwrap().push((params, target));
            Ok(json!({"presetName":"Demo", "blocks":[]}))
        }
    }

    #[tokio::test]
    async fn accepted_target_reaches_backend_separately_and_malformed_guards_run_nothing() {
        let backend = Arc::new(TargetBackend(std::sync::Mutex::new(Vec::new())));
        let server = QcMcp::new(backend.clone());
        let route = PrincipalRoute::new("fixture", "device");
        let target = json!({"source":"demo", "sessionId":"broker-1:demo-1", "generation":1});
        let args = json!({"expectedBrokerTarget":target});
        server
            .execute(
                &route,
                "get_current_preset",
                Some(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        {
            let calls = backend.0.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert!(calls[0].0.is_empty());
            assert_eq!(serde_json::to_value(&calls[0].1).unwrap(), target);
        }
        for malformed in [
            Value::Null,
            json!({}),
            json!({"source":"demo", "sessionId":" ", "generation":1}),
            json!({"source":"demo", "sessionId":"x", "generation":0}),
            json!({"source":"unknown", "sessionId":"x", "generation":1}),
            json!({"source":"demo", "sessionId":"x", "generation":1, "extra":true}),
        ] {
            let args = json!({"expectedBrokerTarget":malformed});
            assert!(
                server
                    .execute(
                        &route,
                        "get_current_preset",
                        Some(args.as_object().unwrap().clone())
                    )
                    .await
                    .unwrap_err()
                    .contains("expectedBrokerTarget")
            );
        }
        assert_eq!(backend.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn legacy_backend_accepts_old_calls_but_never_silently_drops_a_target_guard() {
        struct Legacy(std::sync::atomic::AtomicUsize);
        #[async_trait::async_trait]
        impl QcBackend for Legacy {
            async fn request(
                &self,
                _: &PrincipalRoute,
                _: &'static str,
                _: Map<String, Value>,
            ) -> Result<Value, BackendError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({"presetName":"Legacy physical", "blocks":[]}))
            }
        }
        let backend = Arc::new(Legacy(std::sync::atomic::AtomicUsize::new(0)));
        let server = QcMcp::new(backend.clone());
        let route = PrincipalRoute::new("fixture", "device");
        let args =
            json!({"expectedBrokerTarget":{"source":"demo","sessionId":"demo-1","generation":1}});
        assert!(
            server
                .execute(
                    &route,
                    "get_current_preset",
                    Some(args.as_object().unwrap().clone())
                )
                .await
                .is_err()
        );
        assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        server
            .execute(&route, "get_current_preset", None)
            .await
            .unwrap();
        assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    struct RefusalBackend {
        code: Option<&'static str>,
        supplied_retry: bool,
        raw_details: Value,
    }

    impl RefusalBackend {
        fn failure(&self) -> BackendError {
            BackendError {
                code: "device_error",
                message: "UNSAVED_STATE_UNCONFIRMED message must not drive policy".into(),
                retryable: self.supplied_retry,
                detail_code: self.code.map(str::to_owned),
                details: Some(Box::new(self.raw_details.clone())),
            }
        }
    }

    #[async_trait::async_trait]
    impl QcBackend for RefusalBackend {
        async fn request(
            &self,
            _: &PrincipalRoute,
            _: &'static str,
            _: Map<String, Value>,
        ) -> Result<Value, BackendError> {
            Err(self.failure())
        }
    }

    #[tokio::test]
    async fn actual_tool_and_resource_errors_share_exact_code_action_policy() {
        let route = PrincipalRoute::new("fixture-principal", "fixture-device");
        for (code, expected) in [
            (Some("UNSAVED_STATE_UNCONFIRMED"), Some(true)),
            (Some("UNSAVED_STATE_NOT_YET_OBSERVED"), Some(false)),
            (Some("DEVICE_TRANSPORT"), None),
            (Some("UNSAVED_STATE_UNCONFIRMED_EXTRA"), None),
            (Some("unsaved_state_unconfirmed"), None),
            (None, None),
        ] {
            for supplied in [json!(true), json!(false), json!("true"), Value::Null] {
                let backend = Arc::new(RefusalBackend {
                    code,
                    supplied_retry: expected.unwrap_or(false),
                    raw_details: json!({"userActionRequired":supplied,"code":"UNSAVED_STATE_UNCONFIRMED"}),
                });
                let resource =
                    serde_json::to_value(backend_protocol_error(backend.failure())).unwrap();
                let server = QcMcp::new(backend);
                let text = server
                    .execute(
                        &route,
                        "list_models",
                        Some(Map::from_iter([("query".into(), Value::Null)])),
                    )
                    .await
                    .unwrap_err();
                let tool: Value = serde_json::from_str(&text).unwrap();
                for value in [&tool, &resource["data"]] {
                    assert_eq!(
                        value.get("userActionRequired").and_then(Value::as_bool),
                        expected
                    );
                    if expected.is_none() {
                        assert!(value.get("userActionRequired").is_none());
                    }
                    assert_eq!(
                        value["retryable"],
                        expected.map(|required| !required).unwrap_or(false)
                    );
                    assert_eq!(value["code"], "device_error");
                }
                assert_eq!(
                    tool["message"],
                    "UNSAVED_STATE_UNCONFIRMED message must not drive policy"
                );
            }
        }
        let legacy = backend_error_details(&BackendError::unavailable("offline"));
        assert!(legacy.get("userActionRequired").is_none());
        assert_eq!(legacy["retryable"], true);
    }

    #[test]
    fn backend_protocol_error_keeps_device_code_additive_to_retry_class() {
        let error = backend_protocol_error(BackendError {
            code: "device_error",
            message: "The device is busy".into(),
            retryable: false,
            detail_code: Some("DEVICE_SUSPENDED".into()),
            details: Some(Box::new(json!({"requestSent": false}))),
        });
        let encoded = serde_json::to_value(error).expect("MCP error serialization");
        assert_eq!(encoded["data"]["code"], "device_error");
        assert_eq!(encoded["data"]["retryable"], false);
        assert_eq!(encoded["data"]["deviceCode"], "DEVICE_SUSPENDED");
        assert_eq!(encoded["data"]["deviceDetails"]["requestSent"], false);
    }

    #[test]
    fn scene_color_mcp_requires_exact_rgb_readback_before_dispatch() {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.name == "set_scene_color")
            .unwrap();
        let valid = json!({"scene": 2, "color": 0xffff02c2_u32,
            "expected_color": "#AbCdEf", "expected_preset_name": "Test",
            "expected_preset_identity": "observed-native-A"});
        assert!(validate(spec, valid.as_object().unwrap()).is_ok());
        for invalid in [
            Value::Null,
            json!(1),
            json!(""),
            json!("abcdef"),
            json!("#abcdeg"),
            json!("#abcdef0"),
        ] {
            let mut args = valid.clone();
            args["expected_color"] = invalid;
            assert!(validate(spec, args.as_object().unwrap()).is_err());
        }
        let mut missing = valid.as_object().unwrap().clone();
        missing.remove("expected_color");
        assert!(validate(spec, &missing).is_err());
        assert_eq!(
            spec.tool().input_schema["properties"]["expected_color"],
            json!({"type":"string", "pattern":"^#[0-9a-fA-F]{6}$"})
        );
    }

    fn action(name: &str) -> &'static ActionSpec {
        ACTIONS.iter().find(|spec| spec.name == name).unwrap()
    }

    #[test]
    fn stringified_null_is_restored_for_every_nullable_non_string_property() {
        // The wire value a client sends when it cannot write a JSON null.
        let text = json!("null");
        let mut restored = 0usize;
        let mut kept_as_text = 0usize;
        for spec in ACTIONS {
            let schema = spec.parsed_input_schema();
            for (name, property) in schema["properties"].as_object().unwrap() {
                if !crate::schema::matches(&Value::Null, property) {
                    continue;
                }
                let mut args = Map::new();
                args.insert(name.clone(), text.clone());
                restore_stringified_nulls(spec, &mut args);
                if crate::schema::matches(&text, property) {
                    assert_eq!(args[name], text, "{}.{name} is a string kind", spec.name);
                    kept_as_text += 1;
                } else {
                    assert_eq!(args[name], Value::Null, "{}.{name}", spec.name);
                    restored += 1;
                }
            }
        }
        assert!(restored > 20, "swept only {restored} nullable properties");
        assert!(kept_as_text > 0, "no nullable string property was checked");
    }

    #[test]
    fn add_block_accepts_a_stringified_null_guard_and_still_rejects_other_text() {
        let spec = action("add_block");
        let args = |guard: Value| {
            let mut args = json!({"row": 1, "column": 3, "model_id": 6005,
                "expected_preset_name": "Test", "expected_preset_identity": "native-A"})
            .as_object()
            .unwrap()
            .clone();
            args.insert("expected_model_id".into(), guard);
            args
        };
        for guard in [json!(null), json!("null")] {
            let mut sent = args(guard);
            restore_stringified_nulls(spec, &mut sent);
            assert_eq!(sent["expected_model_id"], Value::Null);
            assert!(validate(spec, &sent).is_ok());
        }
        // Only the exact text `null` is restored; the guard is not loosened.
        for guard in [
            json!("None"),
            json!("NULL"),
            json!(" null"),
            json!(0),
            json!("6005"),
        ] {
            let mut sent = args(guard.clone());
            restore_stringified_nulls(spec, &mut sent);
            assert_eq!(sent["expected_model_id"], guard);
            assert!(validate(spec, &sent).is_err(), "{guard} must stay invalid");
        }
        // A non-nullable guard is never turned into null.
        let spec = action("remove_block");
        let mut sent = json!({"row": 1, "column": 3, "expected_model_id": "null",
            "expected_preset_name": "Test", "expected_preset_identity": "native-A"})
        .as_object()
        .unwrap()
        .clone();
        restore_stringified_nulls(spec, &mut sent);
        assert_eq!(sent["expected_model_id"], json!("null"));
        assert!(validate(spec, &sent).is_err());
    }

    #[test]
    fn a_nullable_string_property_keeps_the_literal_text_null() {
        let spec = action("set_scene_label");
        let mut sent = json!({"scene": 0, "label": "null", "expected_label": "",
            "expected_preset_name": "Test", "expected_preset_identity": "native-A"})
        .as_object()
        .unwrap()
        .clone();
        restore_stringified_nulls(spec, &mut sent);
        assert_eq!(sent["label"], json!("null"));
    }

    #[test]
    fn canonical_empty_label_operands_are_valid_but_long_identity_is_not() {
        for (name, extra) in [
            ("set_scene_label", json!({"scene":2})),
            ("set_stomp_label", json!({"footswitch":2})),
        ] {
            let spec = ACTIONS.iter().find(|spec| spec.name == name).unwrap();
            let mut args = json!({"label":"Lead", "expected_label":"", "expected_preset_name":"Test", "expected_preset_identity":"native-A"});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(
                validate(spec, args.as_object().unwrap()).is_ok(),
                "an empty observed label is a declared operand for {name}"
            );
            args["expected_label"] = json!("Previous");
            args["expected_preset_identity"] = json!("x".repeat(1025));
            assert!(validate(spec, args.as_object().unwrap()).is_err());
        }
    }

    #[test]
    fn favorite_key_patterns_reject_invalid_segments_before_backend_dispatch() {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.name == "set_favorite")
            .unwrap();
        let valid = json!({"name":"Favorite", "folder_key":"/media/p4/Presets/My Presets", "product_key":"Preset A", "is_factory":false,
            "favorite":true,"expected_favorite":false,"confirm_persistent_write":true,"is_plugin":false});
        assert!(validate(spec, valid.as_object().unwrap()).is_ok());
        for (field, invalid) in [
            ("name", "a/b"),
            ("name", ".."),
            ("name", " leading"),
            ("folder_key", "/media/../Presets"),
            ("folder_key", "/media//Presets"),
            ("folder_key", "bad\\key"),
            ("product_key", "trailing "),
        ] {
            let mut args = valid.clone();
            args[field] = json!(invalid);
            assert!(
                validate(spec, args.as_object().unwrap()).is_err(),
                "{field} accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn device_knowledge_resources_bundle_the_skill_and_catalog() {
        assert!(DEVICE_KNOWLEDGE_GUIDE.contains("name: qc-device-knowledge"));
        assert!(DEVICE_KNOWLEDGE_CATALOG.contains("\"device_name\""));
        assert!(DEVICE_KNOWLEDGE_CATALOG.contains("\"based_on\""));
    }

    #[test]
    fn device_knowledge_search_finds_a_known_device_and_its_hardware_basis() {
        let result = search_device_knowledge(Some(
            serde_json::from_value(json!({
                "query": "Myth Drive",
                "limit": 1
            }))
            .unwrap(),
        ))
        .unwrap();
        assert_eq!(result["matches"][0]["device_name"], "Myth Drive");
        assert_eq!(result["matches"][0]["based_on"], "Klon\u{ae} Centaur\u{ae}");
    }

    #[test]
    fn every_device_knowledge_result_names_its_public_source() {
        // The attribution identifies the public product documentation behind
        // the structured compatibility record.
        let result = search_device_knowledge(Some(
            serde_json::from_value(json!({"query": "Myth Drive", "limit": 1})).unwrap(),
        ))
        .unwrap();
        let source = result["matches"][0]["reference_source"].as_str().unwrap();
        assert!(
            source.contains("Neural DSP") && source.contains("neuraldsp.com"),
            "device knowledge attribution names no source: {source}"
        );
        assert!(
            !source.to_ascii_lowercase().contains("creativecommons")
                && !source.to_ascii_lowercase().contains("cc by"),
            "device knowledge attribution must not assert an unverified license: {source}"
        );
    }

    /// The cloud Worker's filterModelResult runs the same file
    /// (tests/remote-model-query.test.ts in the maintainers' suite).
    #[test]
    fn shared_model_query_cases_keep_the_stated_models() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/model-query-cases.json")).unwrap();
        let catalog = json!({"models": fixture["models"].clone()});
        let cases = fixture["cases"].as_array().unwrap();
        assert!(cases.len() >= 10, "only {} shared query cases", cases.len());
        for case in cases {
            let query = normalized_model_query(Some(&case["query"]));
            let kept = filter_models(catalog.clone(), &query).unwrap();
            let ids: Vec<_> = kept["models"]
                .as_array()
                .unwrap()
                .iter()
                .map(|model| model["id"].clone())
                .collect();
            assert_eq!(Value::Array(ids), case["ids"], "query {}", case["query"]);
        }
    }

    #[test]
    fn advertised_catalog_size_is_counted_from_the_catalog() {
        let count = device_knowledge_model_count();
        assert!(count > 0, "the bundled catalog parsed to nothing");
        let raw: Value = serde_json::from_str(DEVICE_KNOWLEDGE_CATALOG).unwrap();
        assert_eq!(count, raw["join"]["entries"].as_array().unwrap().len());
        let description = device_knowledge_search_tool().description.unwrap();
        assert!(
            description.contains(&format!("-- {count} models")),
            "{description}"
        );
    }

    #[test]
    fn identifier_stripping_follows_the_relay_profile() {
        let mut value = json!({"presetName": "Clean", "nested": [{}]});
        for field in qc_relay_protocol::REMOVED_IDENTIFIER_FIELDS {
            value[*field] = json!("secret");
            value["nested"][0][field.to_ascii_uppercase()] = json!("secret");
        }
        let sanitized = sanitize_model_result(value);
        assert_eq!(sanitized, json!({"presetName": "Clean", "nested": [{}]}));
    }

    #[test]
    fn device_knowledge_search_rejects_invalid_arguments() {
        for arguments in [
            json!({}),
            json!({"query": ""}),
            json!({"query": "!!!"}),
            json!({"query": "cortex", "limit": 11}),
            json!({"query": "cortex", "surprise": true}),
        ] {
            let arguments = serde_json::from_value(arguments).unwrap();
            assert!(search_device_knowledge(Some(arguments)).is_err());
        }
    }

    #[test]
    fn generated_io_constraints_reject_empty_and_unsupported_sparse_writes() {
        let validate_action = |name: &str, value: Value| {
            let spec = ACTIONS.iter().find(|item| item.name == name).unwrap();
            let args = serde_json::from_value::<Map<String, Value>>(value).unwrap();
            validate(spec, &args)
        };
        for (name, value) in [
            (
                "set_input_port",
                json!({"input_port_id":4,"level_db":null,"impedance":0.0,"input_type":null,"ground_lift":null,"confirm_persistent_write":true}),
            ),
            (
                "set_output_port",
                json!({"output_port_id":2,"level":null,"ground_lift":0.0,"mute":null,"confirm_persistent_write":true}),
            ),
            (
                "set_output_port",
                json!({"output_port_id":8,"level":null,"ground_lift":null,"mute":false,"confirm_persistent_write":true}),
            ),
            (
                "set_input_port",
                json!({"input_port_id":1,"level_db":null,"impedance":null,"input_type":null,"ground_lift":null,"confirm_persistent_write":true}),
            ),
            (
                "set_output_port",
                json!({"output_port_id":1,"level":null,"ground_lift":null,"mute":null,"confirm_persistent_write":true}),
            ),
            (
                "set_usb_port",
                json!({"level":null,"headphones_source":null,"dry_wet":null,"confirm_persistent_write":true}),
            ),
            (
                "set_output_pairing",
                json!({"xlr12_linked":null,"out34_linked":null,"confirm_persistent_write":true}),
            ),
        ] {
            assert!(
                validate_action(name, value).is_err(),
                "{name} accepted invalid dependent arguments"
            );
        }
        validate_action("set_input_port", json!({"input_port_id":4,"level_db":0.0,"impedance":null,"input_type":null,"ground_lift":null,"confirm_persistent_write":true})).unwrap();
        validate_action("set_output_port", json!({"output_port_id":8,"level":0.0,"ground_lift":null,"mute":null,"confirm_persistent_write":true})).unwrap();
    }

    #[test]
    fn set_device_name_requires_and_consumes_persistent_confirmation() {
        let spec = ACTIONS
            .iter()
            .find(|item| item.name == "set_device_name")
            .unwrap();
        let mut missing = serde_json::from_value::<Map<String, Value>>(serde_json::json!({
            "name": "Stage QC"
        }))
        .unwrap();
        assert!(apply_confirmation_gate(spec, &mut missing).is_err());

        let mut confirmed = serde_json::from_value::<Map<String, Value>>(serde_json::json!({
            "name": "Stage QC",
            "confirm_persistent_write": true
        }))
        .unwrap();
        apply_confirmation_gate(spec, &mut confirmed).unwrap();
        assert_eq!(
            confirmed.get("name"),
            Some(&Value::String("Stage QC".into()))
        );
        assert!(!confirmed.contains_key("confirm_persistent_write"));
        assert_eq!(
            gateway_params(spec, confirmed).get("name"),
            Some(&Value::String("Stage QC".into()))
        );
    }

    /// A real 1x1 PNG, base64 encoded.
    const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    #[test]
    fn screen_image_tools_add_exactly_one_png_image_block_and_keep_the_json() {
        for name in SCREEN_IMAGE_TOOLS {
            let value = json!({"pngBase64": ONE_PIXEL_PNG, "width": 1, "height": 1});
            let result = tool_result(name, value.clone());
            // Structured content is untouched, byte for byte.
            assert_eq!(result.structured_content, Some(value.clone()));
            assert_eq!(result.is_error, Some(false));
            // The text block is still first and still the same JSON.
            assert_eq!(result.content.len(), 2, "{name}");
            let text = result.content[0].as_text().expect("text block first");
            assert_eq!(serde_json::from_str::<Value>(&text.text).unwrap(), value);
            // Exactly one image block with the same bytes and the PNG type.
            let images: Vec<_> = result
                .content
                .iter()
                .filter_map(ContentBlock::as_image)
                .collect();
            assert_eq!(images.len(), 1, "{name}");
            assert_eq!(images[0].data, ONE_PIXEL_PNG);
            assert_eq!(images[0].mime_type, "image/png");
            let wire = serde_json::to_value(&result.content[1]).unwrap();
            assert_eq!(
                wire,
                json!({"type": "image", "data": ONE_PIXEL_PNG, "mimeType": "image/png"})
            );
        }
    }

    #[test]
    fn other_tools_and_malformed_screen_results_return_no_image_block() {
        let png = json!({"pngBase64": ONE_PIXEL_PNG, "width": 1, "height": 1});
        // A non-capture tool is unchanged even if its result happens to carry the field.
        let other = tool_result("get_current_preset", png.clone());
        assert_eq!(other.content.len(), 1);
        assert!(other.content[0].as_image().is_none());
        assert_eq!(other.structured_content, Some(png));
        // A capture result that is not a PNG in the gateway's shape stays text only.
        for malformed in [
            json!({"width": 1, "height": 1}),
            json!({"pngBase64": 7}),
            json!({"pngBase64": ""}),
            json!({"pngBase64": "/9j/4AAQSkZJRgABAQ=="}),
            json!([ONE_PIXEL_PNG]),
        ] {
            let result = tool_result("capture_screen", malformed.clone());
            assert_eq!(result.content.len(), 1, "{malformed}");
            assert_eq!(result.structured_content, Some(malformed));
        }
    }

    #[test]
    fn drag_screen_requires_confirmation_bounds_its_options_and_omits_defaults() {
        let spec = ACTIONS
            .iter()
            .find(|item| item.name == "drag_screen")
            .unwrap();
        assert_eq!(spec.rpc, "device.dragScreen");
        assert_eq!(spec.classification, Classification::RiskyWrite);
        let base = json!({"x": 400, "y": 100, "to_x": 400, "to_y": 300});
        let with = |extra: Value| {
            let mut args = base.as_object().unwrap().clone();
            args.extend(extra.as_object().unwrap().clone());
            args
        };
        // Unconfirmed calls never pass the gate.
        let mut missing = with(json!({}));
        assert!(apply_confirmation_gate(spec, &mut missing).is_err());
        let mut denied = with(json!({"confirm_risky_operation": false}));
        assert!(apply_confirmation_gate(spec, &mut denied).is_err());
        // Optional operands may be omitted: validation passes and the gateway
        // receives only what the caller named (the planner owns the defaults).
        let mut confirmed = with(json!({"confirm_risky_operation": true}));
        validate(spec, &confirmed).unwrap();
        apply_confirmation_gate(spec, &mut confirmed).unwrap();
        assert_eq!(
            Value::Object(gateway_params(spec, confirmed)),
            json!({"x": 400, "y": 100, "toX": 400, "toY": 300})
        );
        let mut explicit = with(json!({
            "confirm_risky_operation": true, "steps": 40, "hold_ms": 1500
        }));
        validate(spec, &explicit).unwrap();
        apply_confirmation_gate(spec, &mut explicit).unwrap();
        assert_eq!(
            Value::Object(gateway_params(spec, explicit)),
            json!({"x": 400, "y": 100, "toX": 400, "toY": 300, "steps": 40, "holdMs": 1500})
        );
        // Out-of-range or malformed options are refused before dispatch.
        for invalid in [
            json!({"steps": 1}),
            json!({"steps": 41}),
            json!({"steps": 18.5}),
            json!({"steps": "18"}),
            json!({"steps": null}),
            json!({"hold_ms": -1}),
            json!({"hold_ms": 1501}),
            json!({"hold_ms": null}),
            json!({"x": 800}),
            json!({"to_y": 480}),
            json!({"unknown": 1}),
        ] {
            let mut args = with(json!({"confirm_risky_operation": true}));
            args.extend(invalid.as_object().unwrap().clone());
            assert!(validate(spec, &args).is_err(), "{invalid} must be refused");
        }
        assert_eq!(
            spec.tool().input_schema["properties"]["steps"]["minimum"],
            json!(2)
        );
        assert_eq!(
            spec.tool().input_schema["properties"]["hold_ms"]["maximum"],
            json!(1500)
        );
    }

    #[test]
    fn mouse_frame_requires_confirmation_and_takes_an_end_point_for_drag_only() {
        let spec = ACTIONS
            .iter()
            .find(|item| item.name == "mouse_frame")
            .unwrap();
        assert_eq!(spec.rpc, "device.mouseFrame");
        assert_eq!(spec.classification, Classification::RiskyWrite);
        let with = |extra: Value| {
            let mut args = json!({"x": 400, "y": 100}).as_object().unwrap().clone();
            args.extend(extra.as_object().unwrap().clone());
            args
        };
        // Unconfirmed calls never pass the gate.
        let mut missing = with(json!({"type": "tap"}));
        assert!(apply_confirmation_gate(spec, &mut missing).is_err());
        let mut denied = with(json!({"type": "tap", "confirm_risky_operation": false}));
        assert!(apply_confirmation_gate(spec, &mut denied).is_err());
        // Every non-drag type is valid without an end point, and the gateway
        // receives only what the caller named.
        for kind in ["press", "release", "move", "tap"] {
            let mut confirmed = with(json!({"type": kind, "confirm_risky_operation": true}));
            validate(spec, &confirmed).unwrap();
            apply_confirmation_gate(spec, &mut confirmed).unwrap();
            assert_eq!(
                Value::Object(gateway_params(spec, confirmed)),
                json!({"type": kind, "x": 400, "y": 100})
            );
        }
        // A null end point means the same as an absent one for a non-drag type.
        let nulls = with(json!({
            "type": "press", "confirm_risky_operation": true, "to_x": null, "to_y": null
        }));
        validate(spec, &nulls).unwrap();
        // A drag carries both end coordinates through under their gateway names.
        let mut drag = with(json!({
            "type": "drag", "confirm_risky_operation": true, "to_x": 400, "to_y": 300
        }));
        validate(spec, &drag).unwrap();
        apply_confirmation_gate(spec, &mut drag).unwrap();
        assert_eq!(
            Value::Object(gateway_params(spec, drag)),
            json!({"type": "drag", "x": 400, "y": 100, "toX": 400, "toY": 300})
        );
        // Everything else is refused before dispatch.
        for invalid in [
            json!({"type": "drag"}),
            json!({"type": "drag", "to_x": 400}),
            json!({"type": "drag", "to_y": 300}),
            json!({"type": "drag", "to_x": null, "to_y": 300}),
            json!({"type": "drag", "to_x": 800, "to_y": 300}),
            json!({"type": "drag", "to_x": 400, "to_y": 480}),
            json!({"type": "drag", "to_x": 1.5, "to_y": 300}),
            json!({"type": "tap", "to_x": 400}),
            json!({"type": "tap", "to_x": 400, "to_y": 300}),
            json!({"type": "release", "to_y": 300}),
            json!({"type": "TAP"}),
            json!({"type": "click"}),
            json!({"type": "mouse"}),
            json!({"type": 3}),
            json!({"type": null}),
            json!({"type": "tap", "x": 800}),
            json!({"type": "tap", "y": 480}),
            json!({"type": "tap", "x": -1}),
            json!({"type": "tap", "x": 1.5}),
            json!({"type": "tap", "unknown": 1}),
        ] {
            let mut args = with(json!({"confirm_risky_operation": true}));
            args.extend(invalid.as_object().unwrap().clone());
            assert!(validate(spec, &args).is_err(), "{invalid} must be refused");
        }
        // The type is required.
        let untyped = with(json!({"confirm_risky_operation": true}));
        assert!(validate(spec, &untyped).is_err());
        assert_eq!(
            spec.tool().input_schema["properties"]["type"]["enum"],
            json!(["press", "release", "move", "tap", "drag"])
        );
    }

    #[test]
    fn swipe_screen_requires_and_consumes_risky_confirmation() {
        let spec = ACTIONS
            .iter()
            .find(|item| item.name == "swipe_screen")
            .unwrap();
        let coordinates = serde_json::json!({"x": 0, "y": 0, "to_x": 799, "to_y": 479});
        let mut missing =
            serde_json::from_value::<Map<String, Value>>(coordinates.clone()).unwrap();
        assert!(apply_confirmation_gate(spec, &mut missing).is_err());

        let mut confirmed = serde_json::from_value::<Map<String, Value>>(serde_json::json!({
            "x": 0, "y": 0, "to_x": 799, "to_y": 479,
            "confirm_risky_operation": true
        }))
        .unwrap();
        apply_confirmation_gate(spec, &mut confirmed).unwrap();
        assert!(!confirmed.contains_key("confirm_risky_operation"));
        assert_eq!(
            Value::Object(gateway_params(spec, confirmed)),
            serde_json::json!({"x": 0, "y": 0, "toX": 799, "toY": 479})
        );
    }
}

#[cfg(test)]
mod navigation_coordinate_mapping_regression {
    use super::*;

    #[test]
    fn failed_navigation_coordinates_keep_exact_generated_mcp_mapping() {
        let coordinates = [(100, 440), (0, 479)];
        assert_eq!(coordinates.len(), 2);
        let actions = [
            ("swipe_screen", "device.swipeScreen", false),
            ("drag_screen", "device.dragScreen", false),
            ("drag_screen", "device.dragScreen", true),
        ];
        assert_eq!(actions.len(), 3);
        for (y, to_y) in coordinates {
            for (name, rpc, options) in actions {
                let spec = ACTIONS.iter().find(|item| item.name == name).unwrap();
                assert_eq!(spec.rpc, rpc);
                let mut input =
                    json!({"x":400,"y":y,"to_x":400,"to_y":to_y,"confirm_risky_operation":true});
                let mut expected = json!({"x":400,"y":y,"toX":400,"toY":to_y});
                if options {
                    input["steps"] = json!(30);
                    input["hold_ms"] = json!(1450);
                    expected["steps"] = json!(30);
                    expected["holdMs"] = json!(1450);
                }
                let mut args = input.as_object().unwrap().clone();
                validate(spec, &args).unwrap();
                apply_confirmation_gate(spec, &mut args).unwrap();
                assert_eq!(Value::Object(gateway_params(spec, args)), expected);
            }
        }
    }
}
