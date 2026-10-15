//! `qc://current-preset` and `get_current_preset` must serve the same state.
//!
//! The shared MCP instructions tell a model to read the resource before any
//! preset-scoped action and to copy `expected_preset_identity` from it. The
//! resource used to read `device.snapshot` (the broker's cached view) while the
//! tool read `device.currentPreset` (which refreshes), so after the device moved
//! the resource kept handing out the old identity and guarded writes built from
//! it were refused, or aimed at state that no longer existed.
mod common;
use async_trait::async_trait;
use common::OwnedTask;
use qc_remote_mcp::{ACTIONS, BackendError, PrincipalRoute, QcBackend, QcMcp};
use rmcp::ServiceExt;
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::DuplexStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines, ReadHalf, WriteHalf};

fn preset(name: &str, identity: &str, tempo: u32) -> Value {
    json!({
        "presetName": name,
        // Session-scoped, like the broker's: per-broker UUID, connection
        // counter, then the preset position.
        "presetIdentity": identity,
        "blocks": [],
        "scene": 0,
        "tempo": tempo,
        "dirty": false,
        "deviceName": "Owner's QC",
    })
}

/// A device whose cached snapshot lags its live state, as the broker's does
/// until something refreshes it.
struct LaggingDevice {
    live: Mutex<Value>,
    cached: Value,
    requests: Mutex<Vec<&'static str>>,
}

impl LaggingDevice {
    fn new() -> Self {
        let initial = preset("Clean", "7f3c2a10-uuid:1:A1", 120);
        Self {
            live: Mutex::new(initial.clone()),
            cached: initial,
            requests: Mutex::default(),
        }
    }

    /// The person at the unit recalls another preset. Only a refreshing read
    /// sees it.
    fn recall(&self, next: Value) {
        *self.live.lock().unwrap() = next;
    }
}

#[async_trait]
impl QcBackend for LaggingDevice {
    async fn request(
        &self,
        _route: &PrincipalRoute,
        rpc: &'static str,
        _params: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        self.requests.lock().unwrap().push(rpc);
        match rpc {
            "device.currentPreset" => Ok(self.live.lock().unwrap().clone()),
            "device.snapshot" => Ok(self.cached.clone()),
            "device.listModels" => Ok(json!({"models": []})),
            "system.status" => Ok(json!({"connected": true})),
            other => Err(BackendError::unavailable(format!("unexpected rpc {other}"))),
        }
    }
}

struct Client {
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    write: WriteHalf<DuplexStream>,
    next_id: u64,
    server_task: OwnedTask<()>,
}

impl Client {
    async fn start(device: Arc<LaggingDevice>) -> Self {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let server =
                QcMcp::for_route(device, PrincipalRoute::new("local-user", "local-device"));
            let (server_io, client_io) = tokio::io::duplex(256 * 1024);
            let server_task = OwnedTask(tokio::spawn(async move {
                if let Ok(running) = server.serve(server_io).await {
                    let _ = running.waiting().await;
                }
            }));
            let (read, write) = tokio::io::split(client_io);
            let mut client = Self {
                lines: BufReader::new(read).lines(),
                write,
                next_id: 1,
                server_task,
            };
            client
                .call(
                    "initialize",
                    json!({"protocolVersion":"2025-06-18","capabilities":{},
                       "clientInfo":{"name":"resource-parity","version":"1"}}),
                )
                .await;
            client
                .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .await;
            client
        })
        .await
        .expect("whole MCP fixture observation timed out")
    }

    async fn send(&mut self, message: Value) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut line = message.to_string();
            line.push('\n');
            self.write.write_all(line.as_bytes()).await.unwrap();
        })
        .await
        .expect("whole MCP fixture observation timed out")
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let id = self.next_id;
            self.next_id += 1;
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await;
            loop {
                let line = self.lines.next_line().await.unwrap().expect("server reply");
                let value: Value = serde_json::from_str(&line).unwrap();
                if value["id"] == id {
                    assert!(value.get("error").is_none(), "{method}: {value}");
                    return value["result"].clone();
                }
            }
        })
        .await
        .expect("whole MCP fixture observation timed out")
    }

    async fn close(self) {
        let Self {
            lines,
            write,
            server_task,
            ..
        } = self;
        drop(lines);
        drop(write);
        server_task.join().await;
    }

    async fn resource(&mut self, uri: &str) -> Value {
        let result = self.call("resources/read", json!({"uri": uri})).await;
        serde_json::from_str(result["contents"][0]["text"].as_str().unwrap()).unwrap()
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self
            .call("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        assert_ne!(result["isError"], true, "{name}: {result}");
        result["structuredContent"].clone()
    }
}

#[tokio::test]
async fn current_preset_resource_and_tool_agree_after_the_device_moves() {
    let device = Arc::new(LaggingDevice::new());
    let mut client = Client::start(device.clone()).await;

    let before = client.resource("qc://current-preset").await;
    assert_eq!(before["presetIdentity"], "7f3c2a10-uuid:1:A1");

    device.recall(preset("Lead", "7f3c2a10-uuid:1:B3", 96));

    let resource = client.resource("qc://current-preset").await;
    let tool = client.tool("get_current_preset", json!({})).await;
    assert_eq!(
        resource["presetIdentity"], "7f3c2a10-uuid:1:B3",
        "the resource served a stale preset identity: {resource}"
    );
    assert_eq!(resource, tool, "resource and tool disagree");
    // Identifier stripping applies to the resource exactly as to the tool.
    assert!(resource.get("deviceName").is_none(), "{resource}");

    let requests = device.requests.lock().unwrap().clone();
    assert!(
        !requests.contains(&"device.snapshot"),
        "the resource read the cached snapshot: {requests:?}"
    );
    client.close().await;
}

#[test]
fn every_read_resource_uses_its_tools_rpc() {
    // The resource list and the tools are served from one table; check the
    // two tools the resources mirror still exist and read what the resource
    // test above expects.
    let rpc = |name: &str| ACTIONS.iter().find(|spec| spec.name == name).unwrap().rpc;
    assert_eq!(rpc("get_current_preset"), "device.currentPreset");
    assert_eq!(rpc("list_models"), "device.listModels");
}

#[tokio::test]
async fn search_tool_refuses_bad_arguments_as_a_tool_error_like_every_other_tool() {
    let device = Arc::new(LaggingDevice::new());
    let mut client = Client::start(device).await;
    for (name, arguments) in [
        ("search_qc_device_knowledge", json!({"query": ""})),
        ("get_current_preset", json!({"surprise": true})),
    ] {
        let result = client
            .call("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        assert_eq!(result["isError"], true, "{name}: {result}");
    }
    client.close().await;
}

#[tokio::test]
async fn server_reports_the_product_name() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
    let server = QcMcp::for_route(
        Arc::new(LaggingDevice::new()),
        PrincipalRoute::new("local-user", "local-device"),
    );
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server_task = OwnedTask(tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    }));
    let (read, mut write) = tokio::io::split(client_io);
    write
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"t\",\"version\":\"1\"}}}\n",
        )
        .await
        .unwrap();
    let line = BufReader::new(read)
        .lines()
        .next_line()
        .await
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["result"]["serverInfo"]["name"], "QC Remote");
    drop(write);
    server_task.join().await;
    }).await.expect("whole MCP fixture observation timed out")
}

#[tokio::test]
async fn models_resource_matches_the_unfiltered_tool() {
    let device = Arc::new(LaggingDevice::new());
    let mut client = Client::start(device.clone()).await;
    let resource = client.resource("qc://models").await;
    let tool = client.tool("list_models", json!({"query": null})).await;
    assert_eq!(resource, tool);
    client.close().await;
}

#[tokio::test]
async fn tool_and_resource_preserve_snapshot_and_catalog_session() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let device = Arc::new(LaggingDevice::new());
        let mut client = Client::start(device.clone()).await;
        for (session, revision) in [("fixture-session-a", 41), ("fixture-session-b", 1)] {
            let mut snapshot = preset("Fixture", "fixture-preset", 120);
            snapshot["stateVersion"] = json!({"session":session,"revision":revision});
            snapshot["gridLayoutComplete"] = json!(true);
            let counters: Map<String, Value> = [
                "messages",
                "creates",
                "updates",
                "renames",
                "renameMisses",
                "deletes",
                "deleteMisses",
                "wipes",
                "readsIgnored",
                "otherActionsIgnored",
                "markerUpdates",
                "ambiguousMarkers",
                "loadedIgnored",
                "malformedIdentities",
                "decodeFailures",
                "inboundLoss",
                "duplicateTuples",
                "ambiguousTuples",
                "retainedEntries",
                "retainedOtherHashEntries",
            ]
            .into_iter()
            .map(|key| (key.into(), json!(0)))
            .collect();
            snapshot["modelPresetCatalog"] = json!({
                "state":"folding", "session":session, "generation":1, "scopeHash":123,
                "entries":[], "counters":counters
            });
            device.recall(snapshot);
            let tool = client.tool("get_current_preset", json!({})).await;
            let resource = client.resource("qc://current-preset").await;
            for wire in [&tool, &resource] {
                assert_eq!(wire["stateVersion"]["session"], session);
                assert_eq!(wire["stateVersion"]["revision"], revision);
                assert_eq!(wire["modelPresetCatalog"]["session"], session);
                assert_eq!(wire["modelPresetCatalog"]["state"], "folding");
            }
            assert_eq!(tool, resource);
        }
        client.close().await;
    })
    .await
    .expect("actual MCP tool/resource serialization timed out");
}
