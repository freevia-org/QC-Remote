//! The MCP result a client receives for the two screen-image tools, asserted
//! over the real stdio transport with a mock backend and no device.
//!
//! `capture_screen` and `get_preset_screenshot` return `{pngBase64, width,
//! height}`. That JSON is unchanged, and the same PNG is also returned as a
//! standard MCP image content block so a client that renders content blocks can
//! show the screen. Every other tool must be unaffected.

use async_trait::async_trait;
use qc_remote_mcp::{BackendError, PrincipalRoute, QcBackend, QcMcp};
use rmcp::ServiceExt;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// A real 1x1 PNG, base64 encoded.
const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

struct ScreenBackend;

#[async_trait]
impl QcBackend for ScreenBackend {
    async fn request(
        &self,
        _: &PrincipalRoute,
        rpc: &'static str,
        _: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        Ok(match rpc {
            "device.captureScreen" | "device.presetScreenshot" => {
                json!({"pngBase64": ONE_PIXEL_PNG, "width": 1, "height": 1})
            }
            "device.graphicsTree" => json!({"tree": "zenUI::RootGraphicsItem"}),
            other => json!({"method": other}),
        })
    }
}

async fn call(name: &str, arguments: Value) -> Value {
    let server = QcMcp::for_route(
        Arc::new(ScreenBackend),
        PrincipalRoute::new("local-user", "local-device"),
    );
    let (server_io, client_io) = tokio::io::duplex(256 * 1024);
    let task = tokio::spawn(async move {
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
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "screen-image-test", "version": "1"}}});
    write
        .write_all(format!("{init}\n").as_bytes())
        .await
        .expect("initialize");
    lines.next_line().await.expect("read").expect("init reply");
    let request = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}});
    write
        .write_all(
            format!(
                "{}\n{request}\n",
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
            )
            .as_bytes(),
        )
        .await
        .expect("call");
    let response = loop {
        let value: Value =
            serde_json::from_str(&lines.next_line().await.expect("read").expect("response"))
                .expect("JSON");
        if value["id"] == 2 {
            break value;
        }
    };
    drop(lines);
    drop(write);
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .expect("stdio server stopped")
        .expect("task joined");
    response["result"].clone()
}

fn images(result: &Value) -> Vec<&Value> {
    result["content"]
        .as_array()
        .expect("content array")
        .iter()
        .filter(|block| block["type"] == "image")
        .collect()
}

#[tokio::test]
async fn capture_screen_returns_the_png_as_an_image_block_next_to_the_json() {
    let result = call("capture_screen", json!({})).await;
    assert_eq!(
        result["structuredContent"],
        json!({"pngBase64": ONE_PIXEL_PNG, "width": 1, "height": 1})
    );
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    let images = images(&result);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["mimeType"], "image/png");
    assert_eq!(images[0]["data"], ONE_PIXEL_PNG);
    assert_ne!(result["isError"], json!(true));
}

#[tokio::test]
async fn get_preset_screenshot_returns_the_same_image_block() {
    let result = call(
        "get_preset_screenshot",
        json!({"folder_name": "Folder", "position": 0, "is_factory": false}),
    )
    .await;
    assert_eq!(result["structuredContent"]["width"], 1);
    assert_eq!(result["structuredContent"]["height"], 1);
    let images = images(&result);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["data"], ONE_PIXEL_PNG);
    assert_eq!(images[0]["mimeType"], "image/png");
}

#[tokio::test]
async fn other_tools_return_no_image_block() {
    let result = call("get_graphics_tree", json!({})).await;
    assert_eq!(
        result["structuredContent"],
        json!({"tree": "zenUI::RootGraphicsItem"})
    );
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert!(images(&result).is_empty());
}
