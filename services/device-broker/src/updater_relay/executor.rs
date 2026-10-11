//! Running one validated request: the redirect loop, the size and time caps
//! and the chunking. It talks to the network only through [`HttpFetcher`] and
//! to the device only through [`ChunkSink`].

use super::fetch::{FetchFailure, FetchRequest, HttpFetcher};
use super::policy::{self, DeviceHeader, Refusal, RelayPolicy};
use super::request::{RelayRequest, RequestKind};
use super::response::error_code;
use reqwest::Url;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The caps one request runs under.
#[derive(Clone, Debug)]
pub struct ExecutionLimits {
    /// The most body bytes accepted for a firmware image.
    pub max_image_bytes: u64,
    /// The most body bytes accepted for any other request type.
    pub max_small_bytes: u64,
    /// The size of each reply chunk handed to the device.
    pub chunk_bytes: usize,
    pub max_redirects: u8,
    /// The longest one request may take in total, whatever the unit asked for.
    pub image_overall: Duration,
    pub small_overall: Duration,
    /// The unit's timeout is clamped into this range.
    pub min_timeout: Duration,
    pub max_timeout: Duration,
    /// How much of an HTTP error's body is passed on.
    pub error_body_bytes: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_image_bytes: crate::tray_settings::UPDATER_RELAY_DEFAULT_MAX_DOWNLOAD_BYTES,
            max_small_bytes: 4 * 1024 * 1024,
            // Each chunk is one USB write. At the ceiling of about 200 KB/s
            // this is a fraction of a second, which keeps the one-second
            // keepalive clock inside its budget between writes.
            chunk_bytes: 32 * 1024,
            max_redirects: policy::MAX_REDIRECTS,
            image_overall: Duration::from_secs(45 * 60),
            small_overall: Duration::from_secs(5 * 60),
            min_timeout: Duration::from_secs(1),
            max_timeout: Duration::from_secs(120),
            error_body_bytes: 64 * 1024,
        }
    }
}

/// The two time bounds of a request: how long any single wait may last (the
/// connect, the response head, each piece of the body), and how long the whole
/// request may last. The unit's own timeout bounds each wait. It also bounds
/// the whole request for everything except the image, whose total is the
/// limit's own: a firmware image takes far longer than a unit's per-request
/// timeout to cross USB. UNKNOWN: whether the unit means its timeout as a
/// per-wait bound or a total; the image rule is the safe reading of both.
pub fn effective_timeouts(
    kind: RequestKind,
    unit_timeout_ms: i32,
    limits: &ExecutionLimits,
) -> (Duration, Duration) {
    let asked = Duration::from_millis(u64::try_from(unit_timeout_ms).unwrap_or(0));
    let wait = asked.clamp(limits.min_timeout, limits.max_timeout);
    let overall = if kind.is_image_download() {
        limits.image_overall
    } else {
        wait.min(limits.small_overall)
    };
    (wait, overall)
}

/// Where reply chunks go. `error_code` is `Some` only on a final frame.
pub trait ChunkSink {
    /// `Err` means the device side is gone and the request should stop.
    fn emit(
        &mut self,
        payload: Vec<u8>,
        last: bool,
        error_code: Option<i32>,
    ) -> Result<(), SinkClosed>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkClosed;

/// How a request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The request was not sent: the named rule refused it.
    Refused(&'static str),
    /// The device's request could not be turned into an HTTP request.
    BadRequest(&'static str),
    TooLarge,
    Timeout,
    TooManyRedirects,
    Network(FetchFailure),
    /// The server answered with a status other than success; it was passed on.
    HttpStatus(u16),
    Cancelled,
    DeviceGone,
}

/// What the audit log records about a finished request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub failure: Option<Failure>,
    /// The final status the server returned, if one was.
    pub status: Option<u16>,
    pub bytes_from_server: u64,
    pub bytes_to_device: u64,
    /// Requests made, counting the first.
    pub hops: u8,
    /// The host of the last URL examined, and its path as a shape: query
    /// values removed, long or token-like segments replaced, short segments
    /// as they are.
    pub host: String,
    pub path_shape: String,
}

impl Outcome {
    fn new() -> Self {
        Self {
            failure: None,
            status: None,
            bytes_from_server: 0,
            bytes_to_device: 0,
            hops: 0,
            host: String::new(),
            path_shape: String::new(),
        }
    }

    fn note_url(&mut self, url: &Url) {
        self.host = url.host_str().unwrap_or("").to_owned();
        self.path_shape = policy::path_shape(url);
    }
}

/// The status-shaped number a failure is reported to the unit as.
pub fn error_code_for(failure: &Failure) -> i32 {
    match failure {
        Failure::Refused(_) => error_code::REFUSED,
        Failure::BadRequest(_) => error_code::BAD_REQUEST,
        Failure::TooLarge => error_code::TOO_LARGE,
        Failure::Timeout => error_code::TIMEOUT,
        Failure::TooManyRedirects => error_code::TOO_MANY_REDIRECTS,
        Failure::Network(FetchFailure::Timeout) => error_code::TIMEOUT,
        Failure::Network(FetchFailure::Protocol) => error_code::PROTOCOL,
        Failure::Network(_) => error_code::NETWORK,
        Failure::HttpStatus(status) => i32::from(*status),
        Failure::Cancelled | Failure::DeviceGone => error_code::NETWORK,
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn same_host(left: &Url, right: &Url) -> bool {
    left.host_str().map(str::to_ascii_lowercase) == right.host_str().map(str::to_ascii_lowercase)
}

fn map_fetch(failure: FetchFailure) -> Failure {
    match failure {
        FetchFailure::Timeout => Failure::Timeout,
        other => Failure::Network(other),
    }
}

/// Run one request to its end, sending every reply frame through `sink`.
///
/// The request is validated before anything is sent, every redirect target is
/// validated the same way, and a failure at any point ends with one final
/// frame carrying an `error_code`, so the unit fails fast instead of waiting
/// out its own timeout. Nothing is retried.
pub fn execute(
    fetcher: &dyn HttpFetcher,
    policy: &RelayPolicy,
    request: &RelayRequest,
    limits: &ExecutionLimits,
    cancel: &AtomicBool,
    sink: &mut dyn ChunkSink,
) -> Outcome {
    let mut outcome = Outcome::new();
    let result = run(fetcher, policy, request, limits, cancel, sink, &mut outcome);
    if let Err(failure) = result {
        outcome.failure = Some(failure);
        // A device that is gone cannot be told; a cancelled request is ended
        // by whoever cancelled it.
        if !matches!(failure, Failure::DeviceGone | Failure::Cancelled) {
            // The final frame of an HTTP error already went out with its body.
            if !matches!(failure, Failure::HttpStatus(_)) {
                let _ = sink.emit(Vec::new(), true, Some(error_code_for(&failure)));
            }
        }
    }
    outcome
}

fn run(
    fetcher: &dyn HttpFetcher,
    policy: &RelayPolicy,
    request: &RelayRequest,
    limits: &ExecutionLimits,
    cancel: &AtomicBool,
    sink: &mut dyn ChunkSink,
    outcome: &mut Outcome,
) -> Result<(), Failure> {
    let Some(kind) = request.kind else {
        return Err(Failure::BadRequest("request-type-unknown"));
    };
    // The method follows from the request type alone; a body is never sent.
    let _method = kind.http_method();
    if request.has_payload {
        return Err(Failure::BadRequest("request-has-body"));
    }
    let (wait, overall) = effective_timeouts(kind, request.timeout_ms, limits);
    let deadline = Instant::now() + overall;
    let max_body = if kind.is_image_download() {
        limits.max_image_bytes
    } else {
        limits.max_small_bytes
    };
    let first_url = Url::parse(&request.url).map_err(|_| Failure::Refused("url-malformed"))?;
    let first_headers = policy::forwardable_headers(&request.headers)
        .map_err(|_| Failure::BadRequest("headers-refused"))?;

    let mut url = first_url.clone();
    let mut hop = 0_u8;
    let fetched = loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        outcome.note_url(&url);
        if let Err(refusal) = policy.check_url(&url) {
            // A request for a domain that is not allowed is recorded by host
            // name only, never by path or query.
            if matches!(refusal, Refusal::Host(_)) {
                outcome.path_shape.clear();
            }
            return Err(Failure::Refused(refusal.reason()));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Failure::Timeout);
        }
        let headers: Vec<DeviceHeader> = if same_host(&url, &first_url) {
            first_headers.clone()
        } else {
            policy::cross_host_headers(&first_headers)
        };
        let hop_result = fetcher.get(&FetchRequest {
            url: &url,
            headers: &headers,
            head_timeout: wait.min(remaining),
        });
        hop += 1;
        outcome.hops = hop;
        let fetched = hop_result.map_err(map_fetch)?;
        outcome.status = Some(fetched.status);
        if !is_redirect(fetched.status) {
            break fetched;
        }
        let location = fetched
            .location
            .ok_or(Failure::Network(FetchFailure::Protocol))?;
        if hop > limits.max_redirects {
            return Err(Failure::TooManyRedirects);
        }
        let mut next = url
            .join(&location)
            .map_err(|_| Failure::Refused("redirect-target-malformed"))?;
        next.set_fragment(None);
        url = next;
    };

    let mut body = fetched.body;
    let success = (200..300).contains(&fetched.status);
    if !success {
        // Pass the server's answer on, bounded, and let the unit read the
        // status from `error_code`.
        let mut collected = Vec::new();
        while collected.len() < limits.error_body_bytes {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match body.next_chunk(wait.min(remaining)) {
                Ok(Some(piece)) => collected.extend_from_slice(&piece),
                _ => break,
            }
        }
        collected.truncate(limits.error_body_bytes);
        outcome.bytes_from_server = collected.len() as u64;
        let length = collected.len() as u64;
        sink.emit(collected, true, Some(i32::from(fetched.status)))
            .map_err(|_| Failure::DeviceGone)?;
        outcome.bytes_to_device = length;
        return Err(Failure::HttpStatus(fetched.status));
    }
    if fetched
        .content_length
        .is_some_and(|length| length > max_body)
    {
        return Err(Failure::TooLarge);
    }

    // Hold one chunk back so the final frame can be marked without knowing
    // the length in advance.
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Failure::Timeout);
        }
        match body.next_chunk(wait.min(remaining)).map_err(map_fetch)? {
            Some(piece) => {
                outcome.bytes_from_server += piece.len() as u64;
                if outcome.bytes_from_server > max_body {
                    return Err(Failure::TooLarge);
                }
                buffer.extend_from_slice(&piece);
                while buffer.len() > limits.chunk_bytes {
                    let rest = buffer.split_off(limits.chunk_bytes);
                    let chunk = std::mem::replace(&mut buffer, rest);
                    let length = chunk.len() as u64;
                    sink.emit(chunk, false, None)
                        .map_err(|_| Failure::DeviceGone)?;
                    outcome.bytes_to_device += length;
                }
            }
            None => {
                let length = buffer.len() as u64;
                sink.emit(buffer, true, None)
                    .map_err(|_| Failure::DeviceGone)?;
                outcome.bytes_to_device += length;
                return Ok(());
            }
        }
    }
}
