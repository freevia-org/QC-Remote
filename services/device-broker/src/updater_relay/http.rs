//! The real [`HttpFetcher`]: a reqwest client on its own current-thread
//! runtime, built the first time the Quad Cortex asks for something.
//!
//! The client follows no redirects, uses no system proxy (a proxy would
//! resolve the destination itself and bypass the address check below) and does
//! not decompress, so the bytes the server sent are the bytes the unit
//! receives: the four `no_*` decompression calls in `build_engine` keep that
//! true even if another crate in the build turns a decompression feature on,
//! and no `Accept-Encoding` is ever sent. TLS is the platform verifier's; no
//! certificate or host-name check is relaxed.
//!
//! It sends the unit's headers as they are. What it adds is only what HTTP
//! itself needs (`Host`, the framing of the connection) and one default it
//! cannot be told to drop: reqwest puts `Accept: */*` on a request whose
//! header set has no `Accept` (read in reqwest 0.13.5, `ClientBuilder::new`;
//! the builder only ever extends its defaults). The capture's request set has
//! no `Accept`, so the server is sent one the unit did not build. Whether that
//! matters to the server (if its signature covers the header set) is UNKNOWN
//! until the first live run.

use super::fetch::{FetchFailure, FetchRequest, FetchedHop, HttpBody, HttpFetcher};
use super::policy::is_public_address;
use quadcortex::sync::RecoverPoison;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type BoxError = Box<dyn Error + Send + Sync>;

/// Resolves a name and hands the connection only addresses that are public.
/// One refused address refuses the whole answer, so a name that mixes a public
/// and a private address cannot be used to reach the private one. The check
/// runs on the addresses the client then connects to, so a name that changes
/// its answer between two lookups cannot slip past it.
struct GuardedResolver {
    refused: Arc<AtomicBool>,
}

async fn resolve_public(
    host: String,
    refused: Arc<AtomicBool>,
) -> Result<reqwest::dns::Addrs, BoxError> {
    let lookup = tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        (host.as_str(), 0_u16)
            .to_socket_addrs()
            .map(|found| found.collect::<Vec<_>>())
    })
    .await;
    let addresses = match lookup {
        Ok(Ok(addresses)) => addresses,
        _ => return Err(BoxError::from("name did not resolve")),
    };
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_public_address(address.ip()))
    {
        refused.store(true, Ordering::Release);
        return Err(BoxError::from("address not allowed"));
    }
    let addresses: reqwest::dns::Addrs = Box::new(addresses.into_iter());
    Ok(addresses)
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(resolve_public(
            name.as_str().to_owned(),
            Arc::clone(&self.refused),
        ))
    }
}

struct Engine {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    /// Set by the resolver when it refused an address during the last request.
    refused: Arc<AtomicBool>,
}

/// How long after a failed build of the engine the next request may try again.
const ENGINE_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// A value built on first use. A failed build is not remembered for the life of
/// the process: the next request after the backoff builds it again, so a
/// transient failure (the platform's certificate verifier or the runtime could
/// not start for a moment) does not leave every later request failing until the
/// broker restarts.
struct Lazy<T> {
    slot: Mutex<Slot<T>>,
}

enum Slot<T> {
    Empty,
    Built(Arc<T>),
    Failed { retry_at: Instant },
}

impl<T> Lazy<T> {
    fn new() -> Self {
        Self {
            slot: Mutex::new(Slot::Empty),
        }
    }

    /// The value, building it if it is not built and no failed build is still
    /// inside its backoff. `None` is a failure to build now.
    fn get_or_build(
        &self,
        now: Instant,
        backoff: Duration,
        build: impl FnOnce() -> Option<T>,
    ) -> Option<Arc<T>> {
        let mut slot = self.slot.lock_recover();
        match &*slot {
            Slot::Built(value) => return Some(Arc::clone(value)),
            Slot::Failed { retry_at } if now < *retry_at => return None,
            _ => {}
        }
        match build() {
            Some(value) => {
                let value = Arc::new(value);
                *slot = Slot::Built(Arc::clone(&value));
                Some(value)
            }
            None => {
                *slot = Slot::Failed {
                    retry_at: now + backoff,
                };
                None
            }
        }
    }
}

/// Builds its engine on first use, so a broker that is never asked for an
/// update never loads a TLS stack.
pub struct ReqwestFetcher {
    engine: Lazy<Engine>,
}

impl Default for ReqwestFetcher {
    fn default() -> Self {
        Self {
            engine: Lazy::new(),
        }
    }
}

impl ReqwestFetcher {
    fn engine(&self) -> Result<Arc<Engine>, FetchFailure> {
        self.engine
            .get_or_build(Instant::now(), ENGINE_RETRY_BACKOFF, || build_engine().ok())
            .ok_or(FetchFailure::Io)
    }
}

fn build_engine() -> Result<Engine, BoxError> {
    let refused = Arc::new(AtomicBool::new(false));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        // These exist without their features, and turn the feature off if some
        // other crate in the build enables it: a client that decompressed
        // would send `Accept-Encoding` and hand the unit decoded bytes.
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .https_only(true)
        .dns_resolver(GuardedResolver {
            refused: Arc::clone(&refused),
        })
        .build()?;
    Ok(Engine {
        runtime,
        client,
        refused,
    })
}

/// Reduce a client error to a kind. The error's own text is never used: it
/// prints the request URL.
fn classify(error: &reqwest::Error, refused: bool) -> FetchFailure {
    if refused {
        return FetchFailure::BlockedAddress;
    }
    if error.is_timeout() {
        return FetchFailure::Timeout;
    }
    if error.is_builder() {
        return FetchFailure::Protocol;
    }
    // Walk the causes below the client's own error (whose `Debug` shows the
    // URL) for a TLS or resolver cause. The text is only matched against fixed
    // names here, never stored or shown.
    let mut source: Option<&(dyn Error + 'static)> = error.source();
    while let Some(current) = source {
        let name = format!("{current:?}");
        if name.contains("InvalidCertificate")
            || name.contains("Certificate")
            || name.contains("HandshakeFailure")
            || name.contains("AlertReceived")
        {
            return FetchFailure::Tls;
        }
        if name.contains("did not resolve") {
            return FetchFailure::Dns;
        }
        source = current.source();
    }
    if error.is_connect() {
        FetchFailure::Connect
    } else if error.is_request() || error.is_body() || error.is_decode() {
        FetchFailure::Io
    } else {
        FetchFailure::Protocol
    }
}

struct ReqwestBody {
    response: reqwest::Response,
    engine: Arc<Engine>,
}

impl HttpBody for ReqwestBody {
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure> {
        let engine = Arc::clone(&self.engine);
        let outcome = engine
            .runtime
            .block_on(async { tokio::time::timeout(timeout, self.response.chunk()).await });
        match outcome {
            Err(_elapsed) => Err(FetchFailure::Timeout),
            Ok(Ok(Some(bytes))) => Ok(Some(bytes.to_vec())),
            Ok(Ok(None)) => Ok(None),
            Ok(Err(error)) => Err(classify(&error, false)),
        }
    }
}

impl HttpFetcher for ReqwestFetcher {
    fn get(&self, request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
        let engine = self.engine()?;
        engine.refused.store(false, Ordering::Release);
        let mut builder = engine.client.get(request.url.clone());
        for header in request.headers {
            builder = builder.header(header.name.as_str(), header.value.as_str());
        }
        let head_timeout = request.head_timeout;
        let sent = engine
            .runtime
            .block_on(async { tokio::time::timeout(head_timeout, builder.send()).await });
        let response = match sent {
            Err(_elapsed) => return Err(FetchFailure::Timeout),
            Ok(Err(error)) => {
                return Err(classify(&error, engine.refused.load(Ordering::Acquire)));
            }
            Ok(Ok(response)) => response,
        };
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_length = response.content_length();
        Ok(FetchedHop {
            status,
            location,
            content_length,
            body: Box::new(ReqwestBody { response, engine }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_failed_build_is_retried_after_the_backoff_and_a_good_one_is_kept() {
        let lazy: Lazy<u32> = Lazy::new();
        let backoff = Duration::from_secs(5);
        let base = Instant::now();
        let builds = Cell::new(0_u32);
        let failing = || {
            builds.set(builds.get() + 1);
            None
        };
        // The first request fails to build, and a request inside the backoff
        // does not build again.
        assert_eq!(lazy.get_or_build(base, backoff, failing), None);
        assert_eq!(
            lazy.get_or_build(base + Duration::from_secs(4), backoff, || {
                builds.set(builds.get() + 1);
                Some(7)
            }),
            None
        );
        assert_eq!(builds.get(), 1);
        // The first request after the backoff builds again, and this time it works.
        let built = lazy
            .get_or_build(base + Duration::from_secs(5), backoff, || {
                builds.set(builds.get() + 1);
                Some(7)
            })
            .expect("built after the backoff");
        assert_eq!(*built, 7);
        assert_eq!(builds.get(), 2);
        // A good value is kept: later requests never build.
        let again = lazy
            .get_or_build(base + Duration::from_secs(60), backoff, || {
                builds.set(builds.get() + 1);
                Some(9)
            })
            .expect("kept");
        assert!(Arc::ptr_eq(&built, &again));
        assert_eq!(builds.get(), 2);
    }

    #[test]
    fn a_build_that_keeps_failing_keeps_being_retried_once_per_backoff() {
        let lazy: Lazy<u32> = Lazy::new();
        let backoff = Duration::from_secs(5);
        let base = Instant::now();
        let builds = Cell::new(0_u32);
        for second in 0..16_u64 {
            let _ = lazy.get_or_build(base + Duration::from_secs(second), backoff, || {
                builds.set(builds.get() + 1);
                None
            });
        }
        // Seconds 0, 5, 10 and 15.
        assert_eq!(builds.get(), 4);
    }
}
