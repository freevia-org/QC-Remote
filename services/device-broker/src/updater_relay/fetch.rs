//! The seam between the relay's rules and the network. The executor only sees
//! this trait, so every test of the rules runs against a fake and never opens
//! a socket.

use super::policy::DeviceHeader;
use reqwest::Url;
use std::time::Duration;

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
    /// Anything else on the wire.
    Io,
}

/// One GET, to one URL, with no redirect handling.
pub struct FetchRequest<'a> {
    pub url: &'a Url,
    pub headers: &'a [DeviceHeader],
    /// The longest to wait for the response head.
    pub head_timeout: Duration,
}

/// The body of one response, read in pieces.
pub trait HttpBody: Send {
    /// The next piece, or `None` at the end of the body. Waits at most `timeout`.
    fn next_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, FetchFailure>;
}

/// The head of one response and its body.
pub struct FetchedHop {
    pub status: u16,
    /// The `Location` header, for redirect statuses.
    pub location: Option<String>,
    /// The declared body length, when the server sent one.
    pub content_length: Option<u64>,
    pub body: Box<dyn HttpBody>,
}

/// Performs one GET. Implementations must not follow redirects, must not add
/// request headers beyond the ones passed in (the connection's own framing
/// excepted, and the real client's default `Accept: */*` when none was passed,
/// which it cannot drop), must not decompress, and must refuse to connect to a
/// non-public address.
pub trait HttpFetcher: Send + Sync {
    fn get(&self, request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure>;
}
