//! Type 61 adapter: stream one HTTP response into correlated USB chunks.
use super::{
    fetch::{FetchFailure, FetchRequest, HttpFetcher},
    request::{RelayRequest, RequestKind},
    response::error_code,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// USB frame size, not a limit on the HTTP response.
pub const USB_CHUNK_BYTES: usize = 32 * 1024;
pub trait ChunkSink {
    fn set_deadline(&mut self, _deadline: Option<Instant>) {}
    fn emit(&mut self, payload: Vec<u8>, last: bool, status: Option<i32>)
        -> Result<(), SinkClosed>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkClosed;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    BadRequest,
    Network(FetchFailure),
    HttpStatus(u16),
    Cancelled,
    DeviceGone,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub failure: Option<Failure>,
    pub status: Option<u16>,
    pub bytes_from_server: u64,
    pub bytes_to_device: u64,
}
pub fn error_code_for(failure: &Failure) -> i32 {
    match failure {
        Failure::HttpStatus(status) => i32::from(*status),
        Failure::Network(FetchFailure::Timeout) => error_code::TIMEOUT,
        Failure::BadRequest | Failure::Network(FetchFailure::Protocol) => error_code::PROTOCOL,
        _ => error_code::NETWORK,
    }
}
pub fn execute(
    fetcher: &dyn HttpFetcher,
    request: &RelayRequest,
    cancel: &Arc<AtomicBool>,
    sink: &mut dyn ChunkSink,
) -> Outcome {
    execute_at(fetcher, request, cancel, sink, Instant::now())
}
pub fn execute_at(
    fetcher: &dyn HttpFetcher,
    request: &RelayRequest,
    cancel: &Arc<AtomicBool>,
    sink: &mut dyn ChunkSink,
    started: Instant,
) -> Outcome {
    let timeout = Duration::from_millis(u64::try_from(request.timeout_ms).unwrap_or(0));
    let deadline = (!timeout.is_zero()).then(|| started + timeout);
    sink.set_deadline(deadline);
    let mut outcome = Outcome {
        failure: None,
        status: None,
        bytes_from_server: 0,
        bytes_to_device: 0,
    };
    let result = (|| {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        if deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(Failure::Network(FetchFailure::Timeout));
        }
        let timeout = deadline
            .map(|end| end.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::ZERO);
        let url = url::Url::parse(&request.url).map_err(|_| Failure::BadRequest)?;

        let mut response = fetcher
            .get_until(
                &FetchRequest {
                    url: &url,
                    headers: &request.headers,
                    head_timeout: timeout,
                    payload: &request.payload,
                    cancel: Arc::clone(cancel),
                },
                deadline,
            )
            .map_err(Failure::Network)?;
        outcome.status = Some(response.status);
        // DOWNLOAD_UPDATE_HEADER asks for response metadata, not image bytes.
        // The JSON envelope carries every upstream header; the GET body is unread.
        if request.kind == Some(RequestKind::DownloadUpdateHeader) {
            let payload = response_headers_json(&response.headers)?;
            let length = payload.len() as u64;
            sink.emit(payload, true, Some(i32::from(response.status)))
                .map_err(|_| Failure::DeviceGone)?;
            outcome.bytes_to_device = length;
            return if (200..300).contains(&response.status) {
                Ok(())
            } else {
                Err(Failure::HttpStatus(response.status))
            };
        }
        let mut pending = Vec::new();
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(Failure::Cancelled);
            }
            if deadline.is_some_and(|end| Instant::now() >= end) {
                return Err(Failure::Network(FetchFailure::Timeout));
            }
            match response
                .body
                .next_chunk(timeout)
                .map_err(Failure::Network)?
            {
                Some(piece) => {
                    outcome.bytes_from_server += piece.len() as u64;
                    pending.extend_from_slice(&piece);
                    while pending.len() > USB_CHUNK_BYTES {
                        let rest = pending.split_off(USB_CHUNK_BYTES);
                        let chunk = std::mem::replace(&mut pending, rest);
                        let size = chunk.len() as u64;
                        sink.emit(chunk, false, None)
                            .map_err(|_| Failure::DeviceGone)?;
                        outcome.bytes_to_device += size;
                    }
                }
                None => {
                    let size = pending.len() as u64;
                    sink.emit(pending, true, Some(i32::from(response.status)))
                        .map_err(|_| Failure::DeviceGone)?;
                    outcome.bytes_to_device += size;
                    return if (200..300).contains(&response.status) {
                        Ok(())
                    } else {
                        Err(Failure::HttpStatus(response.status))
                    };
                }
            }
        }
    })();
    if let Err(failure) = result {
        if !matches!(
            failure,
            Failure::HttpStatus(_) | Failure::DeviceGone | Failure::Cancelled
        ) && !cancel.load(Ordering::Acquire)
        {
            let _ = sink.emit(Vec::new(), true, Some(error_code_for(&failure)));
        }
        outcome.failure = Some(failure);
    }
    outcome
}

/// QC metadata uses conventional HTTP field names. Serialize all entries,
/// including duplicate fields, without an allowlist or value conversion.
fn response_headers_json(headers: &[(String, String)]) -> Result<Vec<u8>, Failure> {
    use serde::ser::{SerializeMap, Serializer};
    let mut bytes = Vec::new();
    {
        let mut serializer = serde_json::Serializer::new(&mut bytes);
        let mut map = serializer
            .serialize_map(Some(headers.len()))
            .map_err(|_| Failure::BadRequest)?;
        for (name, value) in headers {
            let name = name
                .split('-')
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join("-");
            map.serialize_entry(&name, value)
                .map_err(|_| Failure::BadRequest)?;
        }
        map.end().map_err(|_| Failure::BadRequest)?;
    }
    Ok(bytes)
}
