//! The seam between the relay's rules and the network. The executor only sees
//! this trait, so every test of the rules runs against a fake and never opens
//! a socket.

use super::policy::DeviceHeader;
use std::time::{Duration, Instant};
use url::Url;

/// Why one hop did not produce a response or a body. Nothing here carries a
/// URL, a header value or a library error's text: the real client's errors
/// print the request URL, so they are mapped to these and dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchFailure {
    /// The deadline for connecting, for the response head, or for the next
    /// piece of the body passed.
    Timeout,
    /// The name did not resolve.
    Dns,
    /// The name resolved to an address the relay refuses to connect to.
    BlockedAddress,
    /// The connection was refused or dropped.
    Connect,
    /// The TLS handshake or certificate validation failed.
    Tls,
    /// The server's reply was not valid HTTP, or the request could not be built.
    Protocol,
    /// Retained for host-side operations that only need a status code.
    HttpStatus(u16),
    /// Anything else on the wire.
    Io,
}

/// One GET, to one URL, with no redirect handling.
pub struct FetchRequest<'a> {
    pub url: &'a Url,
    pub headers: &'a [DeviceHeader],
    /// The longest to wait for the response head.
    pub head_timeout: Duration,
    pub payload: &'a [u8],
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// The body of one response, read in pieces.
pub trait HttpBody: Send {
    /// The next piece, or `None` at the end of the body. Waits at most `timeout`.
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure>;
}

/// The head of one response and its body.
pub struct FetchedHop {
    pub status: u16,
    /// Every upstream response header, including duplicate fields.
    pub headers: Vec<(String, String)>,
    /// The `Location` header, for redirect statuses.
    pub location: Option<String>,
    /// The declared body length, when the server sent one.
    pub content_length: Option<u64>,
    pub body: Box<dyn HttpBody>,
}

/// Performs one GET without redirect handling, retries, or decompression.
/// Preserve device headers and destination; the transport supplies connection
/// framing and Neural API requests carry required desktop client metadata.
pub trait HttpFetcher: Send + Sync {
    fn get(&self, request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure>;
    fn get_until(
        &self,
        request: &FetchRequest<'_>,
        deadline: Option<Instant>,
    ) -> Result<FetchedHop, FetchFailure> {
        if deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(FetchFailure::Timeout);
        }
        self.get(request)
    }
}
