use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;

struct NoDevice;
#[async_trait::async_trait]
impl qc_remote_mcp::QcBackend for NoDevice {
    async fn request(
        &self,
        _: &qc_remote_mcp::PrincipalRoute,
        _: &'static str,
        _: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, qc_remote_mcp::BackendError> {
        panic!("Envelope/initialize fixture must never reach a device");
    }
}

/// A DEMO-only adapter behind the production authorization backend. It cannot
/// discover/open hardware and records effects only after the target fence.
#[cfg(target_os = "linux")]
struct DemoAdapter {
    generation: std::sync::atomic::AtomicU64,
    effects: std::sync::atomic::AtomicUsize,
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl qc_relay_client::DeviceAdapter for DemoAdapter {
    async fn ready(&self) -> bool {
        true
    }
    async fn invoke(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, qc_relay_client::DeviceError> {
        panic!("DEMO test must use the target-aware ingress")
    }
    async fn invoke_targeted(
        &self,
        method: &str,
        _: serde_json::Value,
        expected: Option<qc_remote_mcp::BrokerTarget>,
        _: bool,
    ) -> Result<serde_json::Value, qc_relay_client::DeviceError> {
        use std::sync::atomic::Ordering::SeqCst;
        let target = serde_json::json!({"source":"demo","sessionId":"linux-http-demo","generation":self.generation.load(SeqCst)});
        if method == "system.status" {
            return Ok(serde_json::json!({"targetReady":true,"brokerTarget":target}));
        }
        if expected.map(|t| serde_json::to_value(t).unwrap()) != Some(target) {
            return Err(qc_relay_client::DeviceError::new(
                "BROKER_TARGET_CHANGED",
                "Target changed",
                false,
            ));
        }
        assert_eq!(method, "device.setTempo");
        self.effects.fetch_add(1, SeqCst);
        Ok(
            serde_json::json!({"accepted":true,"verified":false,"verification":"accepted_unverified","detail":"DEMO fixture effect"}),
        )
    }
}

#[cfg(target_os = "linux")]
async fn demo_rpc(
    router: &axum::Router,
    session: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, Option<String>, serde_json::Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "127.0.0.1:41012")
        .header("authorization", "Bearer demo-http-secret")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let id = response
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 1 << 20),
    )
    .await
    .unwrap()
    .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let value = serde_json::from_str(text).unwrap_or_else(|_| {
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .find_map(|line| serde_json::from_str(line.trim()).ok())
            .unwrap_or(serde_json::Value::Null)
    });
    if status == StatusCode::OK && body.get("id").is_some() {
        assert_eq!(
            value.get("id"),
            body.get("id"),
            "a successful RPC must contain its actual response"
        );
    }
    (status, id, value)
}

#[cfg(target_os = "linux")]
async fn demo_session(router: &axum::Router, name: &str) -> String {
    let (status, session, _) = demo_rpc(router, None, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":name,"version":"1"}}})).await;
    assert_eq!(status, StatusCode::OK);
    let session = session.unwrap();
    let (status, _, _) = demo_rpc(
        router,
        Some(&session),
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert!(status.is_success());
    session
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_http_demo_sessions_do_not_share_binding_permissions_or_reconnect_replay() {
    use qc_relay_client::AccessMode;
    use serde_json::json;
    use std::sync::atomic::Ordering::SeqCst;
    let adapter = Arc::new(DemoAdapter {
        generation: 1.into(),
        effects: 0.into(),
    });
    let (policy, access) = tokio::sync::watch::channel(AccessMode::Full);
    let backend = Arc::new(crate::local_access::LocalBackend {
        adapter: adapter.clone(),
        access,
    });
    let router = crate::local_access::direct_http_router(
        backend,
        Arc::from("demo-http-secret"),
        Arc::from("demo-http-instance"),
        41012,
    );
    let (first, second) = tokio::join!(
        demo_session(&router, "linux-demo-one"),
        demo_session(&router, "linux-demo-two")
    );
    assert_ne!(first, second);
    let write = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"set_tempo","arguments":{"bpm":120,"expected_tempo":110,"expected_preset_name":"Fixture","expected_preset_identity":"fixture:1"}}});
    let status = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_qc_status","arguments":{}}});
    assert_eq!(
        demo_rpc(&router, Some(&first), write.clone()).await.2["result"]["isError"],
        true
    );
    assert_eq!(adapter.effects.load(SeqCst), 0);
    assert_ne!(
        demo_rpc(&router, Some(&first), status.clone()).await.2["result"]["isError"],
        true
    );
    assert_ne!(
        demo_rpc(&router, Some(&first), write.clone()).await.2["result"]["isError"],
        true
    );
    assert_eq!(adapter.effects.load(SeqCst), 1);
    assert_eq!(
        demo_rpc(&router, Some(&second), write.clone()).await.2["result"]["isError"],
        true,
        "another session does not inherit status binding"
    );
    policy.send(AccessMode::ReadOnly).unwrap();
    assert_eq!(
        demo_rpc(&router, Some(&first), write.clone()).await.2["result"]["isError"],
        true
    );
    assert_eq!(
        adapter.effects.load(SeqCst),
        1,
        "revocation applies to an already-bound session"
    );
    policy.send(AccessMode::Full).unwrap();
    adapter.generation.store(2, SeqCst);
    assert_eq!(
        demo_rpc(&router, Some(&first), write.clone()).await.2["result"]["isError"],
        true,
        "old target cannot receive effects"
    );
    demo_rpc(&router, Some(&second), status).await;
    assert_ne!(
        demo_rpc(&router, Some(&second), write.clone()).await.2["result"]["isError"],
        true
    );
    assert_eq!(adapter.effects.load(SeqCst), 2);
    let delete = Request::builder()
        .method("DELETE")
        .uri("/mcp")
        .header("host", "127.0.0.1:41012")
        .header("authorization", "Bearer demo-http-secret")
        .header("mcp-session-id", &first)
        .body(Body::empty())
        .unwrap();
    assert!(router
        .clone()
        .oneshot(delete)
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        demo_rpc(&router, Some(&first), write.clone()).await.0,
        StatusCode::NOT_FOUND
    );
    let replacement = demo_session(&router, "linux-demo-reconnected").await;
    assert_eq!(
        demo_rpc(&router, Some(&replacement), write).await.2["result"]["isError"],
        true
    );
    assert_eq!(
        adapter.effects.load(SeqCst),
        2,
        "uncertain/reinitialized retry is unbound, not replayed"
    );
    for session in [&second, &replacement] {
        let delete = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header("host", "127.0.0.1:41012")
            .header("authorization", "Bearer demo-http-secret")
            .header("mcp-session-id", session.as_str())
            .body(Body::empty())
            .unwrap();
        assert!(router
            .clone()
            .oneshot(delete)
            .await
            .unwrap()
            .status()
            .is_success());
    }
}

#[tokio::test]
async fn direct_audience_routes_headers_and_identity_are_fail_closed() {
    let router = crate::local_access::direct_http_router(
        Arc::new(NoDevice),
        Arc::from("direct-secret"),
        Arc::from("direct-instance"),
        41012,
    );
    let request = |path: &str, token: &str, host: &str| {
        Request::builder().method("POST")
        .uri(path).header("host",host).header("authorization",format!("Bearer {token}"))
        .header("content-type","application/json").header("accept","application/json, text/event-stream")
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"isolated-fixture","version":"1"}}}"#)).unwrap()
    };
    for path in ["/rpc", "/health", "/other"] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(path, "direct-secret", "127.0.0.1:41012"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        router
            .clone()
            .oneshot(request("/mcp", "legacy-secret", "127.0.0.1:41012"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for host in [
        "localhost:41012",
        "127.0.0.1:41012.",
        "127.0.0.1:41013",
        "LOCALHOST:41012",
        "[::1]:41012",
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(request("/mcp", "direct-secret", host))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut origin = request("/mcp", "direct-secret", "127.0.0.1:41012");
    origin
        .headers_mut()
        .insert("origin", "null".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(origin).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut old = request("/mcp", "direct-secret", "127.0.0.1:41012");
    *old.version_mut() = axum::http::Version::HTTP_10;
    assert_eq!(
        router.clone().oneshot(old).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "http://127.0.0.1:41012/mcp",
                "direct-secret",
                "127.0.0.1:41012"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        router
            .oneshot(request("/mcp", "direct-secret", "127.0.0.1:41012"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn identity_proof_has_a_shared_bounded_rate_budget() {
    let router = crate::local_access::direct_http_router(
        Arc::new(NoDevice),
        Arc::from("direct-secret"),
        Arc::from("direct-instance"),
        41012,
    );
    for index in 0..33 {
        let request = Request::builder()
            .method("POST")
            .uri("/identity")
            .header("host", "127.0.0.1:41012")
            .body(Body::from("{}"))
            .unwrap();
        let status = router.clone().oneshot(request).await.unwrap().status();
        assert_eq!(
            status,
            if index == 32 {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::BAD_REQUEST
            }
        );
    }
}

#[tokio::test]
async fn explicit_pair_is_adopted_by_existing_host_and_cleanup_revokes_new_credentials() {
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    let directory = std::env::temp_dir().join(format!(
        "qc-http-owned-fixture-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    qc_owner_only_file::create_owner_only_directory(&directory).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let direct = super::DirectHost::prepare(&directory, Arc::new(NoDevice), &"I".repeat(22));
    let task = tokio::spawn(direct.run());
    // Own the async actor before assertions, including unwind/cancel cleanup.
    struct Task(tokio::task::JoinHandle<Result<(), String>>);
    impl Drop for Task {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let task = Task(task);
    assert!(!directory
        .join(crate::local_http_helper::DESCRIPTOR_FILE)
        .exists());
    let port_path = directory.join(crate::local_http_helper::PORT_FILE);
    let port = crate::local_http_binding::StablePort::select_for_pairing(&port_path).unwrap();
    let headers = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(headers) = crate::local_http_helper::verified_headers(&directory).await {
                break headers;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("existing host adopts explicit pairing");
    assert!(headers["Authorization"]
        .as_str()
        .unwrap()
        .starts_with("Bearer "));
    assert!(
        port.bind().is_err(),
        "live host owns the exact selected port"
    );
    let broker = crate::device_broker_descriptor::device_broker_descriptor_path(&directory);
    let quit = crate::device_broker_descriptor::user_quit_path(&broker);
    qc_owner_only_file::publish_atomically(
        &quit,
        br#"{"instanceId":"quit-fixture","source":"tray"}"#,
    )
    .unwrap();
    assert!(
        crate::local_http_helper::verified_headers(&directory)
            .await
            .is_err(),
        "Quit refuses NEW credentials even while listener is still reachable"
    );
    fs::remove_file(&quit).unwrap();
    assert!(crate::local_http_helper::verified_headers(&directory)
        .await
        .is_ok());
    task.0.abort();
    // Wait for cancellation/Published Drop, never call this proof before join.
    tokio::time::timeout(Duration::from_secs(2), async {
        while !task.0.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(crate::local_http_helper::verified_headers(&directory)
        .await
        .is_err());
    assert!(!directory
        .join(crate::local_http_helper::DESCRIPTOR_FILE)
        .exists());
    assert!(port.bind().is_ok(), "only our exact socket was released");
    assert_eq!(
        crate::local_http_binding::StablePort::read(&port_path)
            .unwrap()
            .port(),
        port.port()
    );
}

#[test]
fn failed_atomic_publication_does_not_rebase_last_success_and_grace_is_bounded() {
    let root = std::env::temp_dir().join(format!(
        "qc-http-publish-fault-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    qc_owner_only_file::create_owner_only_directory(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    crate::local_http_binding::StablePort::select_for_pairing(
        &root.join(crate::local_http_helper::PORT_FILE),
    )
    .unwrap();
    let mut published = super::Published::prepare(&root, &"I".repeat(22))
        .unwrap()
        .unwrap();
    published.descriptor.published_at_unix_ms = 1;
    assert!(published
        .publish_with(|_, _| Err("injected atomic replace failure".into()))
        .is_err());
    assert_eq!(
        published.descriptor.published_at_unix_ms, 1,
        "attempt must not become last successful publication"
    );
    assert!(super::within_publication_grace(1, 120_000));
    assert!(!super::within_publication_grace(1, 120_001));
    assert!(!super::within_publication_grace(100, 99));
    published.publish().unwrap();
    assert!(published.private_record_matches());
    std::fs::write(&published.path, b"not our descriptor").unwrap();
    assert!(
        !published.private_record_matches(),
        "unknown/changed record fails closed"
    );
}
