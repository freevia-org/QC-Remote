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
