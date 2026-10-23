//! On-demand, NO-HARDWARE transport fixture. Never linked into the app.
//! Uses the production QcMcp/session manager and target fence, not a JS model.
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use qc_remote_mcp::{BackendError, BrokerTarget, PrincipalRoute, QcBackend, QcMcp};
use serde_json::{Map, Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{RwLock, watch};
use tower::ServiceExt;

#[derive(Default)]
struct MockBackend {
    effects: AtomicUsize,
    observations: AtomicUsize,
}
fn target() -> BrokerTarget {
    serde_json::from_value(
        json!({"source":"demo","sessionId":"no-hardware-fixture:demo","generation":1}),
    )
    .expect("fixed fixture target")
}
#[async_trait]
impl QcBackend for MockBackend {
    async fn request(
        &self,
        _: &PrincipalRoute,
        rpc: &'static str,
        _: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        if rpc != "system.status" {
            return Err(BackendError::unavailable(
                "No hardware exists in this fixture",
            ));
        }
        self.observations.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"targetReady":true,"brokerTarget":target()}))
    }
    async fn request_targeted(
        &self,
        _: &PrincipalRoute,
        rpc: &'static str,
        _: Map<String, Value>,
        expected: Option<BrokerTarget>,
    ) -> Result<Value, BackendError> {
        if rpc != "device.setTempo" || expected != Some(target()) {
            return Err(BackendError::unavailable(
                "Mock target changed; call get_qc_status",
            ));
        }
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(
            json!({"accepted":true,"verified":false,"verification":"accepted_unverified","detail":"Memory-only fixture effect"}),
        )
    }
}
#[derive(Clone)]
struct Fixture {
    backend: Arc<MockBackend>,
    router: Arc<RwLock<Router>>,
    shutdown: watch::Sender<bool>,
    host: String,
}
fn session_router(backend: Arc<MockBackend>, host: String) -> Router {
    qc_remote_mcp::mcp_router_with_allowed_hosts(
        move || {
            Ok(QcMcp::for_route(
                backend.clone(),
                PrincipalRoute::new("mock-only", "no-device"),
            )
            .with_session_target_guard())
        },
        vec![host],
    )
}
async fn dispatch(State(state): State<Fixture>, request: Request<Body>) -> Response {
    let router = state.router.read().await.clone();
    router.oneshot(request).await.expect("router infallible")
}
async fn rotate(State(state): State<Fixture>) -> impl IntoResponse {
    *state.router.write().await = session_router(state.backend.clone(), state.host.clone());
    Json(json!({"rotated":true}))
}
async fn counts(State(state): State<Fixture>) -> impl IntoResponse {
    Json(
        json!({"effects":state.backend.effects.load(Ordering::SeqCst),"observations":state.backend.observations.load(Ordering::SeqCst)}),
    )
}
async fn shutdown(State(state): State<Fixture>) -> impl IntoResponse {
    let _ = state.shutdown.send(true);
    Json(json!({"stopping":true}))
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--local-mcp-http-headers") {
        // Test-only mock credential. Exercises exact Codex cmd/PowerShell
        // invocation with a native executable under metacharacter paths.
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(std::env::current_exe()?.with_extension("header-count.txt"))?
            .write_all(b"x")?;
        println!("{{\"Authorization\":\"Bearer fixture-only-not-a-real-key\"}}");
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let host = listener.local_addr()?.to_string();
    let backend = Arc::new(MockBackend::default());
    let (stop, mut stopped) = watch::channel(false);
    let state = Fixture {
        router: Arc::new(RwLock::new(session_router(backend.clone(), host.clone()))),
        backend,
        shutdown: stop,
        host: host.clone(),
    };
    let router = Router::new()
        .route("/fixture/counts", get(counts))
        .route("/fixture/rotate", post(rotate))
        .route("/fixture/shutdown", post(shutdown))
        .fallback(dispatch)
        .with_state(state);
    println!("{}", json!({"fixtureUrl":format!("http://{host}")}));
    axum::serve(listener,router).with_graceful_shutdown(async move {
        // An owned outer supervisor remains mandatory; this is a backstop.
        tokio::select! { _ = stopped.changed() => {}, _ = tokio::time::sleep(std::time::Duration::from_secs(600)) => {} }
    }).await?;
    Ok(())
}
