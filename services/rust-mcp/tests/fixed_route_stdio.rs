use async_trait::async_trait;
use qc_remote_mcp::{BackendError, PrincipalRoute, QcBackend, QcMcp};
use rmcp::ServiceExt;
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Default)]
struct RecordingBackend {
    requests: Mutex<Vec<(PrincipalRoute, &'static str)>>,
}

#[async_trait]
impl QcBackend for RecordingBackend {
    async fn request(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        _params: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        self.requests.lock().unwrap().push((route.clone(), rpc));
        Ok(json!({ "connected": true }))
    }
}

#[tokio::test]
async fn fixed_route_authorizes_a_non_http_stdio_transport() {
    let backend = Arc::new(RecordingBackend::default());
    let route = PrincipalRoute::new("local-user", "local-device");
    let server = QcMcp::for_route(backend.clone(), route.clone());
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .expect("serve stdio")
            .waiting()
            .await
            .expect("stdio completion");
    });
    let (read, mut write) = tokio::io::split(client_io);
    let mut lines = BufReader::new(read).lines();

    write
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}
"#,
        )
        .await
        .expect("initialize request");
    let initialized: Value = serde_json::from_str(
        &lines
            .next_line()
            .await
            .expect("initialize read")
            .expect("initialize response"),
    )
    .expect("initialize JSON");
    assert_eq!(initialized["id"], 1);

    write
        .write_all(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"qc://status"}}
"#,
        )
        .await
        .expect("resource request");
    let response: Value = loop {
        let value: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("resource read")
                .expect("resource response"),
        )
        .expect("resource JSON");
        if value["id"] == 2 {
            break value;
        }
    };
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        backend.requests.lock().unwrap().as_slice(),
        &[(route, "system.status")]
    );

    drop(lines);
    drop(write);
    tokio::time::timeout(std::time::Duration::from_secs(2), server_task)
        .await
        .expect("stdio server stopped")
        .expect("stdio task joined");
}

#[derive(Default)]
struct OriginBackend {
    requests: Mutex<Vec<(&'static str, bool)>>,
}

#[async_trait]
impl QcBackend for OriginBackend {
    async fn request(
        &self,
        _route: &PrincipalRoute,
        rpc: &'static str,
        _params: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        self.requests
            .lock()
            .unwrap()
            .push((rpc, qc_remote_mcp::serving_resource_read()));
        Ok(json!({ "connected": true }))
    }
}

/// A backend can tell a resource read from a tool call by origin, for every
/// resource-backed method, including `device.listModels`, which the
/// `list_models` tool shares with `qc://models`.
#[tokio::test]
async fn every_resource_backed_read_is_marked_and_a_tool_call_is_not() {
    let backend = Arc::new(OriginBackend::default());
    let server = QcMcp::for_route(
        backend.clone(),
        PrincipalRoute::new("local-user", "local-device"),
    );
    let (server_io, client_io) = tokio::io::duplex(256 * 1024);
    let server_task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .expect("serve stdio")
            .waiting()
            .await
            .expect("stdio completion");
    });
    let (read, mut write) = tokio::io::split(client_io);
    let mut lines = BufReader::new(read).lines();
    let mut send = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
    );
    let resources = qc_remote_mcp::resource_rpcs();
    assert_eq!(resources.len(), 3, "{resources:?}");
    for (index, (uri, _)) in resources.iter().enumerate() {
        send.push_str(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"resources/read\",\"params\":{{\"uri\":\"{uri}\"}}}}\n",
            index + 2
        ));
    }
    let tool_id = resources.len() + 2;
    send.push_str(&format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{tool_id},\"method\":\"tools/call\",\"params\":{{\"name\":\"list_models\",\"arguments\":{{\"query\":null}}}}}}\n"
    ));
    write.write_all(send.as_bytes()).await.expect("requests");
    let mut answered = 0;
    while answered < resources.len() + 1 {
        let value: Value =
            serde_json::from_str(&lines.next_line().await.expect("read").expect("response"))
                .expect("JSON");
        if value["id"].as_u64().is_some_and(|id| id >= 2) {
            answered += 1;
        }
    }
    let requests = backend.requests.lock().unwrap().clone();
    let mut expected: Vec<_> = resources.iter().map(|&(_, rpc)| (rpc, true)).collect();
    expected.push(("device.listModels", false));
    let mut seen = requests.clone();
    seen.sort();
    expected.sort();
    assert_eq!(seen, expected, "{requests:?}");
    assert!(!qc_remote_mcp::serving_resource_read());

    drop(lines);
    drop(write);
    tokio::time::timeout(std::time::Duration::from_secs(2), server_task)
        .await
        .expect("stdio server stopped")
        .expect("stdio task joined");
}
