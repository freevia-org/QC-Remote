//! Direct HTTP transport shared by both QC forwarding protocols.
//! One device request becomes one HTTP request. No destination policy, retry,
//! redirect, cookie jar, proxy, content processing, or response size limit.
use super::fetch::{FetchFailure, FetchRequest, FetchedHop, HttpBody, HttpFetcher};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Request};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::{
    connect::{dns::Name, HttpConnector},
    Client,
};
use hyper_util::rt::TokioExecutor;
use std::{
    error::Error,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tower_service::Service;

type BoxError = Box<dyn Error + Send + Sync>;
#[derive(Clone)]
struct OsResolver(Resolver);
/// Platform DNS hook. Desktop can retain its isolated resolver helper; embedded
/// hosts use the OS resolver through Tokio without spawning an executable.
pub type ResolverFuture = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;
pub type Resolver = Arc<dyn Fn(String) -> ResolverFuture + Send + Sync>;
fn platform_resolver() -> Resolver {
    Arc::new(|host| {
        Box::pin(async move {
            tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map(|addresses| addresses.collect())
        })
    })
}
impl Service<Name> for OsResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;
    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn call(&mut self, name: Name) -> Self::Future {
        let host = name.as_str().to_owned();
        let resolve = Arc::clone(&self.0);
        Box::pin(async move { Ok(resolve(host).await?.into_iter()) })
    }
}
struct Engine {
    runtime: Option<tokio::runtime::Runtime>,
    client: Client<HttpsConnector<HttpConnector<OsResolver>>, Full<Bytes>>,
}
impl Engine {
    fn new(resolver: Resolver) -> Result<Self, BoxError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut http = HttpConnector::new_with_resolver(OsResolver(resolver));
        http.enforce_http(false);
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_platform_verifier(rustls::crypto::aws_lc_rs::default_provider())?
            .https_or_http()
            .enable_all_versions()
            .wrap_connector(http);
        let mut builder = Client::builder(TokioExecutor::new());
        builder.retry_canceled_requests(false);
        Ok(Self {
            runtime: Some(runtime),
            client: builder.build(connector),
        })
    }
}
impl Engine {
    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime
            .as_ref()
            .expect("runtime exists until engine drop")
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        // An OS DNS lookup may be uninterruptible. Do not let runtime teardown
        // extend the caller's deadline while Tokio's blocking resolver finishes.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}
/// Observe cancellation without restarting the network future. Only a positive
/// timeout supplied by the QC sets a deadline; zero means no deadline.
#[cfg(test)]
async fn wait<T>(
    future: impl Future<Output = Result<T, FetchFailure>>,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<T, FetchFailure> {
    wait_until(
        future,
        (!timeout.is_zero()).then(|| Instant::now() + timeout),
        cancel,
    )
    .await
}
async fn wait_until<T>(
    future: impl Future<Output = Result<T, FetchFailure>>,
    deadline: Option<Instant>,
    cancel: &AtomicBool,
) -> Result<T, FetchFailure> {
    let mut future = Box::pin(future);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(FetchFailure::Io);
        }
        let poll = remaining(deadline)?.min(Duration::from_millis(25));
        let poll = if deadline.is_none() {
            Duration::from_millis(25)
        } else {
            poll
        };
        if let Ok(result) = tokio::time::timeout(poll, &mut future).await {
            return result;
        }
    }
}
fn add_client_metadata(headers: &mut HeaderMap, url: &url::Url) {
    let Some(host) = url.host_str() else { return };
    if url.scheme() == "https" && (host == "neuraldsp.com" || host.ends_with(".neuraldsp.com")) {
        headers
            .entry("x-cloud-api-client-type")
            .or_insert(HeaderValue::from_static("cortex_control"));
        headers
            .entry("x-cloud-api-client-version")
            .or_insert(HeaderValue::from_static(
                quadcortex::profile::CORTEX_CONTROL_VERSION,
            ));
    }
}
fn classify(error: &hyper_util::client::legacy::Error) -> FetchFailure {
    let mut source: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(current) = source {
        let detail = format!("{current:?}");
        if detail.contains("Certificate")
            || detail.contains("HandshakeFailure")
            || detail.contains("AlertReceived")
        {
            return FetchFailure::Tls;
        }
        if detail.contains("did not resolve") {
            return FetchFailure::Dns;
        }
        source = current.source();
    }
    if error.is_connect() {
        FetchFailure::Connect
    } else {
        FetchFailure::Io
    }
}
/// A request owns its runtime through the response body. Keeping a current-thread
/// runtime in the fetcher leaves pooled sockets undriven between requests, so
/// server FINs and cancellation cannot retire until another request arrives.
/// Independent runtimes also keep concurrent requests from sharing one reactor.
pub struct DirectHttpFetcher {
    resolver: Resolver,
}
impl Default for DirectHttpFetcher {
    fn default() -> Self {
        Self {
            resolver: platform_resolver(),
        }
    }
}
impl DirectHttpFetcher {
    pub fn with_resolver(resolver: Resolver) -> Self {
        Self { resolver }
    }
    pub fn send(
        &self,
        method: http::Method,
        url: &url::Url,
        headers: &[(String, String)],
        body: &[u8],
        timeout: Duration,
        cancel: Arc<AtomicBool>,
    ) -> Result<FetchedHop, FetchFailure> {
        let deadline = (!timeout.is_zero()).then(|| Instant::now() + timeout);
        self.send_until(method, url, headers, body, deadline, cancel)
    }
    pub fn send_until(
        &self,
        method: http::Method,
        url: &url::Url,
        headers: &[(String, String)],
        body: &[u8],
        deadline: Option<Instant>,
        cancel: Arc<AtomicBool>,
    ) -> Result<FetchedHop, FetchFailure> {
        remaining(deadline)?;
        if cancel.load(Ordering::Acquire) {
            return Err(FetchFailure::Io);
        }
        let engine =
            Arc::new(Engine::new(Arc::clone(&self.resolver)).map_err(|_| FetchFailure::Io)?);
        let mut request = Request::builder()
            .method(method.clone())
            .uri(url.as_str())
            .body(Full::new(Bytes::copy_from_slice(body)))
            .map_err(|_| FetchFailure::Protocol)?;
        for (name, value) in headers {
            request.headers_mut().append(
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| FetchFailure::Protocol)?,
                HeaderValue::from_bytes(value.as_bytes()).map_err(|_| FetchFailure::Protocol)?,
            );
        }
        add_client_metadata(request.headers_mut(), url);

        let response = engine.runtime().block_on(wait_until(
            async {
                engine
                    .client
                    .request(request)
                    .await
                    .map_err(|e| classify(&e))
            },
            deadline,
            &cancel,
        ))?;
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let location = response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let content_length = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());

        Ok(FetchedHop {
            status,
            headers,
            location,
            content_length,
            body: Box::new(StreamingBody {
                body: response.into_body(),
                engine,
                cancel,
                deadline,
            }),
        })
    }
}
struct StreamingBody {
    body: Incoming,
    engine: Arc<Engine>,
    cancel: Arc<AtomicBool>,
    deadline: Option<Instant>,
}
fn remaining(deadline: Option<Instant>) -> Result<Duration, FetchFailure> {
    match deadline {
        None => Ok(Duration::ZERO),
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                Err(FetchFailure::Timeout)
            } else {
                Ok(remaining)
            }
        }
    }
}
impl HttpBody for StreamingBody {
    fn next_chunk(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
        loop {
            let frame = self.engine.runtime().block_on(wait_until(
                async {
                    self.body
                        .frame()
                        .await
                        .transpose()
                        .map_err(|_| FetchFailure::Io)
                },
                self.deadline,
                &self.cancel,
            ))?;
            let Some(frame) = frame else {
                return Ok(None);
            };
            if let Ok(data) = frame.into_data() {
                return Ok(Some(data.to_vec()));
            }
        }
    }
}
impl HttpFetcher for DirectHttpFetcher {
    fn get(&self, request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
        self.get_until(
            request,
            (!request.head_timeout.is_zero()).then(|| Instant::now() + request.head_timeout),
        )
    }
    fn get_until(
        &self,
        request: &FetchRequest<'_>,
        deadline: Option<Instant>,
    ) -> Result<FetchedHop, FetchFailure> {
        let headers = request
            .headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect::<Vec<_>>();
        self.send_until(
            http::Method::GET,
            request.url,
            &headers,
            request.payload,
            deadline,
            Arc::clone(&request.cancel),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neural_pairing_has_required_client_metadata_without_changing_signed_headers() {
        let url = url::Url::parse("https://cloud-api.neuraldsp.com/hardware/v1/pair").unwrap();
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("authorization", "Bearer device-token"),
            ("x-cloud-api-client-signature", "device-signature"),
            ("x-cloud-api-client-id", "device-client-id"),
            ("x-cloud-api-client-serial-number", "device-serial"),
            ("x-cloud-api-date", "1792057027"),
            ("content-type", "application/json"),
        ] {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
        }
        let original = headers.clone();
        add_client_metadata(&mut headers, &url);
        assert_eq!(headers["x-cloud-api-client-type"], "cortex_control");
        assert_eq!(
            headers["x-cloud-api-client-version"],
            quadcortex::profile::CORTEX_CONTROL_VERSION
        );
        assert_eq!(headers.len(), original.len() + 2);
        for (name, value) in &original {
            assert_eq!(headers.get(name), Some(value));
        }
        add_client_metadata(&mut headers, &url);
        assert_eq!(headers.len(), original.len() + 2);
    }

    #[test]
    fn client_metadata_does_not_change_external_download_or_device_supplied_metadata() {
        for url in [
            "https://downloads.example.com/firmware?signature=keep",
            "https://evilneuraldsp.com/firmware",
            "https://neuraldsp.com.example.com/firmware",
        ] {
            let mut headers = HeaderMap::new();
            add_client_metadata(&mut headers, &url::Url::parse(url).unwrap());
            assert!(headers.is_empty(), "{url}");
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cloud-api-client-type",
            HeaderValue::from_static("device-client"),
        );
        headers.insert(
            "x-cloud-api-client-version",
            HeaderValue::from_static("device-version"),
        );
        add_client_metadata(
            &mut headers,
            &url::Url::parse("https://neuraldsp.com/update").unwrap(),
        );
        assert_eq!(headers["x-cloud-api-client-type"], "device-client");
        assert_eq!(headers["x-cloud-api-client-version"], "device-version");
        assert_eq!(headers.len(), 2);
    }
}

#[cfg(test)]
pub(crate) mod wire_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };
    pub(crate) fn serve(
        status: u16,
        body: Vec<u8>,
    ) -> (
        url::Url,
        mpsc::Receiver<Vec<u8>>,
        thread::JoinHandle<io::Result<()>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = url::Url::parse(&format!(
            "http://{}/original?signature=A%2Bb%2Fc%3D&x=1&x=2",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let (mut socket, _) = listener.accept()?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut request = Vec::new();
            let mut block = [0; 4096];
            loop {
                let n = socket.read(&mut block)?;
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&block[..n]);
                if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let len = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            tx.send(request)
                .map_err(|_| io::Error::other("test receiver closed"))?;
            write!(socket,"HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nContent-Encoding: gzip\r\nLocation: http://127.0.0.1:1/must-not-follow\r\nConnection: close\r\n\r\n",body.len())?;
            socket.write_all(&body)?;
            Ok(())
        });
        (url, rx, thread)
    }
    #[test]
    fn actual_http_preserves_method_query_duplicate_headers_body_and_error_response() {
        for (method, status) in [
            (http::Method::GET, 200),
            (http::Method::POST, 401),
            (http::Method::PUT, 302),
            (http::Method::DELETE, 503),
        ] {
            let bytes = vec![0x1f, 0x8b, 0, 255, 13, 10, 42];
            let (url, rx, server) = serve(status, bytes.clone());
            let payload = vec![0, 255, 12, 13, 10];
            let headers = vec![
                ("X-Device".into(), "first".into()),
                ("X-Device".into(), "second".into()),
                ("Host".into(), "device-selected.example".into()),
                ("Content-Type".into(), "application/custom".into()),
            ];
            let mut response = DirectHttpFetcher::default()
                .send(
                    method.clone(),
                    &url,
                    &headers,
                    &payload,
                    Duration::from_secs(5),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap();
            assert_eq!(response.status, status);
            let mut body = Vec::new();
            while let Some(piece) = response.body.next_chunk(Duration::from_secs(5)).unwrap() {
                body.extend_from_slice(&piece)
            }
            assert_eq!(body, bytes);
            let request = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            server.join().unwrap().unwrap();
            let end = request.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
            let head = String::from_utf8_lossy(&request[..end]);
            let lower = head.to_ascii_lowercase();
            assert!(head.starts_with(&format!(
                "{} /original?signature=A%2Bb%2Fc%3D&x=1&x=2 HTTP/1.1",
                method.as_str()
            )));
            assert!(lower.contains("host: device-selected.example"));
            assert!(lower.contains("x-device: first"));
            assert!(lower.contains("x-device: second"));
            assert!(lower.contains("content-type: application/custom"));
            assert!(!lower.contains("range:"));
            assert!(!lower.contains("cookie:"));
            assert_eq!(&request[end + 4..], payload);
        }
    }
    #[test]
    fn completed_and_abandoned_responses_close_their_socket_while_fetcher_is_idle() {
        // Neither peer sends Connection: close. The fetcher stays alive after
        // dropping each response, with no subsequent request to drive cleanup.
        let fetcher = DirectHttpFetcher::default();
        for consume_body in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url =
                url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            let server = thread::spawn(move || -> io::Result<usize> {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => return Err(e),
                    }
                };
                socket.set_nonblocking(false)?;
                socket.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte)? == 0 {
                        return Err(io::Error::other("request ended early"));
                    }
                    request.push(byte[0]);
                }
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata")?;
                socket.read(&mut byte)
            });
            let result = (|| -> Result<(), FetchFailure> {
                let mut response = fetcher.send(
                    http::Method::GET,
                    &url,
                    &[],
                    &[],
                    Duration::from_secs(5),
                    Arc::new(AtomicBool::new(false)),
                )?;
                if consume_body {
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.body.next_chunk(Duration::from_secs(5))? {
                        bytes.extend(chunk);
                    }
                    assert_eq!(bytes, b"data");
                }
                drop(response);
                Ok(())
            })();
            let closed = server.join().unwrap();
            result.unwrap();
            // Dropping an unread response may reset instead of FIN on Windows.
            assert!(
                matches!(closed, Ok(0))
                    || matches!(&closed, Err(e) if e.kind() == io::ErrorKind::ConnectionReset),
                "consume={consume_body}: {closed:?}"
            );
        }
    }

    #[test]
    fn zero_timeout_wait_is_cancellable_without_restarting_the_future() {
        let engine = Engine::new(platform_resolver()).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancel);
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(75));
            trigger.store(true, Ordering::Release);
        });
        let calls = std::cell::Cell::new(0);
        let started = Instant::now();
        let result: Result<(), _> = engine.runtime().block_on(wait(
            async {
                calls.set(calls.get() + 1);
                std::future::pending().await
            },
            Duration::ZERO,
            &cancel,
        ));
        sender.join().unwrap();
        assert_eq!(result, Err(FetchFailure::Io));
        assert_eq!(calls.get(), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
