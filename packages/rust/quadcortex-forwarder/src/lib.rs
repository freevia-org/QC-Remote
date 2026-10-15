//! Shared production HTTP forwarding for desktop and embedded hosts.
//! Network operations and emitted frames share the device's absolute deadline.
pub mod cloud_forward;
pub mod executor;
pub mod fetch;
pub mod http;
pub mod policy;
pub mod request;
pub mod response;
use prost::Message;
use quadcortex::{commands::OutboundMessage, proto::cortex_protobuf_v2 as pa};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// These envelopes may contain account credentials, tokens or private data.
/// Hosts must never publish them through generic caches, state or event lanes.
pub fn is_sensitive_message_type(message_type: u16) -> bool {
    matches!(message_type, 18 | 29 | 30 | 31 | 41 | 58 | 61)
}

/// A queued response retains the original admission and cancellation owner.
pub struct ForwardedMessage {
    pub message: OutboundMessage,
    pub deadline: Option<Instant>,
    pub cancel: Arc<AtomicBool>,
}
impl ForwardedMessage {
    pub fn current(&self) -> bool {
        check(&self.cancel, self.deadline).is_ok()
    }
    pub fn into_parts(mut self) -> (OutboundMessage, Option<Instant>, Arc<AtomicBool>) {
        let message = OutboundMessage {
            message_type: self.message.message_type,
            payload: std::mem::take(&mut self.message.payload),
        };
        (message, self.deadline, Arc::clone(&self.cancel))
    }
}
impl Drop for ForwardedMessage {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.message.payload.zeroize();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardError {
    Cancelled,
    Timeout,
    Network,
    Protocol,
    SinkClosed,
}
impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ForwardError {}
fn check(cancel: &AtomicBool, deadline: Option<Instant>) -> Result<(), ForwardError> {
    if cancel.load(Ordering::Acquire) {
        return Err(ForwardError::Cancelled);
    }
    if deadline.is_some_and(|end| Instant::now() >= end) {
        return Err(ForwardError::Timeout);
    }
    Ok(())
}
fn deadline(timeout: i32, started: Instant) -> Option<Instant> {
    (timeout > 0).then(|| started + Duration::from_millis(timeout as u64))
}
/// Forward one decrypted protocol frame. Encryption/framing remains in the
/// transport. The sink must honor cancellation and its supplied absolute deadline
/// while waiting for USB admission. Type 18 is a notification, never HTTP.
pub fn forward(
    message_type: u16,
    payload: &[u8],
    cancel: Arc<AtomicBool>,
    sink: &mut dyn FnMut(OutboundMessage, Option<Instant>) -> Result<(), ForwardError>,
) -> Result<(), ForwardError> {
    forward_at(message_type, payload, cancel, sink, Instant::now())
}
/// As `forward`, with a deadline anchored at native request admission rather
/// than worker scheduling. This includes time spent queued before execution.
pub fn forward_at(
    message_type: u16,
    payload: &[u8],
    cancel: Arc<AtomicBool>,
    sink: &mut dyn FnMut(OutboundMessage, Option<Instant>) -> Result<(), ForwardError>,
    started: Instant,
) -> Result<(), ForwardError> {
    forward_with_fetcher_at(
        &http::DirectHttpFetcher::default(),
        message_type,
        payload,
        cancel,
        sink,
        started,
    )
}
pub fn forward_with_fetcher(
    fetcher: &http::DirectHttpFetcher,
    message_type: u16,
    payload: &[u8],
    cancel: Arc<AtomicBool>,
    sink: &mut dyn FnMut(OutboundMessage, Option<Instant>) -> Result<(), ForwardError>,
) -> Result<(), ForwardError> {
    forward_with_fetcher_at(fetcher, message_type, payload, cancel, sink, Instant::now())
}
pub fn forward_with_fetcher_at(
    fetcher: &http::DirectHttpFetcher,
    message_type: u16,
    payload: &[u8],
    cancel: Arc<AtomicBool>,
    sink: &mut dyn FnMut(OutboundMessage, Option<Instant>) -> Result<(), ForwardError>,
    started: Instant,
) -> Result<(), ForwardError> {
    check(&cancel, None)?;
    match message_type {
        18 => Ok(()),
        61 => {
            let request = request::decode_request(payload).map_err(|_| ForwardError::Protocol)?;
            let deadline = deadline(request.timeout_ms, started);
            struct Sink<'a> {
                callback:
                    &'a mut dyn FnMut(OutboundMessage, Option<Instant>) -> Result<(), ForwardError>,
                request_id: Option<u64>,
                updater_id: u64,
                deadline: Option<Instant>,
                cancel: Arc<AtomicBool>,
                error: Option<ForwardError>,
            }
            impl executor::ChunkSink for Sink<'_> {
                fn emit(
                    &mut self,
                    payload: Vec<u8>,
                    last: bool,
                    status: Option<i32>,
                ) -> Result<(), executor::SinkClosed> {
                    let result = check(&self.cancel, self.deadline).and_then(|()| {
                        (self.callback)(
                            response::forward_response(
                                self.request_id,
                                self.updater_id,
                                payload,
                                last,
                                status,
                            ),
                            self.deadline,
                        )
                    });
                    if let Err(error) = result {
                        self.error = Some(error);
                        return Err(executor::SinkClosed);
                    }
                    check(&self.cancel, self.deadline).map_err(|error| {
                        self.error = Some(error);
                        executor::SinkClosed
                    })
                }
            }
            let mut adapter = Sink {
                callback: sink,
                request_id: request.request_id,
                updater_id: request.updater_request_id,
                deadline,
                cancel: Arc::clone(&cancel),
                error: None,
            };
            let outcome = executor::execute_at(fetcher, &request, &cancel, &mut adapter, started);
            if let Some(error) = adapter.error {
                return Err(error);
            }
            check(&cancel, deadline)?;
            match outcome.failure {
                Some(executor::Failure::Cancelled) => Err(ForwardError::Cancelled),
                Some(executor::Failure::DeviceGone) => Err(ForwardError::SinkClosed),
                Some(executor::Failure::Network(fetch::FetchFailure::Timeout)) => {
                    Err(ForwardError::Timeout)
                }
                Some(executor::Failure::Network(_)) => Err(ForwardError::Network),
                Some(executor::Failure::BadRequest) => Err(ForwardError::Protocol),
                _ => Ok(()), // An HTTP error is an intact response, not a transport error.
            }
        }
        29 => {
            let mut message =
                pa::ProductForwardMessage::decode(payload).map_err(|_| ForwardError::Protocol)?;
            let incoming = message
                .request_id
                .take()
                .map(|pa::product_forward_message::RequestId::RequestId(id)| id);
            static NEXT_RESPONSE_REQUEST_ID: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(8);
            let request_id = Some(
                incoming
                    .unwrap_or_else(|| NEXT_RESPONSE_REQUEST_ID.fetch_add(1, Ordering::Relaxed)),
            );
            let Some(pa::product_forward_message::Request::Request(request)) =
                message.request.take()
            else {
                return Err(ForwardError::Protocol);
            };
            let deadline = deadline(request.timeout, started);
            check(&cancel, deadline)?;
            let timeout = deadline
                .map(|end| end.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::ZERO);
            let body = Zeroizing::new(match request.payload {
                Some(pa::product_forward_request::Payload::Payload(bytes)) => bytes,
                None => Vec::new(),
            });
            let headers = request
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect::<Vec<_>>();
            let result = (|| {
                let url =
                    url::Url::parse(&request.url).map_err(|_| fetch::FetchFailure::Protocol)?;
                let method = match pa::http_operation::Enum::try_from(request.operation).ok() {
                    Some(pa::http_operation::Enum::Get) => ::http::Method::GET,
                    Some(pa::http_operation::Enum::Put) => ::http::Method::PUT,
                    Some(pa::http_operation::Enum::Post) => ::http::Method::POST,
                    Some(pa::http_operation::Enum::Delete) => ::http::Method::DELETE,
                    None => return Err(fetch::FetchFailure::Protocol),
                };
                let mut response = fetcher.send_until(
                    method,
                    &url,
                    &headers,
                    &body,
                    deadline,
                    Arc::clone(&cancel),
                )?;
                let mut bytes = Zeroizing::new(Vec::new());
                while let Some(piece) = response.body.next_chunk(timeout)? {
                    bytes.extend_from_slice(&piece);
                }
                Ok((response.status, std::mem::take(&mut *bytes)))
            })();
            check(&cancel, deadline)?;
            let (status, bytes) = match result {
                Ok((status, bytes)) => (i32::from(status), bytes),
                Err(fetch::FetchFailure::Timeout) => (504, Vec::new()),
                Err(_) => (502, Vec::new()),
            };
            let response = cloud_forward::product_forward_response(
                request_id,
                message.cloud_request_id,
                bytes,
                status,
            );
            sink(response, deadline)?;
            check(&cancel, deadline)
        }
        _ => Err(ForwardError::Protocol),
    }
}

#[cfg(test)]
mod tests;
