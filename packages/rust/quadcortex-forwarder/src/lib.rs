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
/// How long before the unit's own deadline a Type 29 is answered when its
/// HTTP exchange has not finished. An unanswered request is followed ~30 s
/// later by the unit's CloudLogin DELETE (captured three times), which drops
/// its account link, so a late answer must never become no answer.
pub const ANSWER_MARGIN: Duration = Duration::from_secs(2);
/// The instant by which a Type 29 with `timeout` ms must be answered:
/// `ANSWER_MARGIN` before the unit's deadline, or halfway for a short one.
pub fn answer_deadline(timeout: i32, started: Instant) -> Option<Instant> {
    (timeout > 0).then(|| {
        let total = Duration::from_millis(timeout as u64);
        started + total.saturating_sub(ANSWER_MARGIN.min(total / 2))
    })
}
/// How long after `answer_deadline` an owner gives a still-running exchange
/// before it answers in its place, so a real response finishing just at the
/// answer deadline is not replaced by an error.
pub const WATCHDOG_GRACE: Duration = Duration::from_millis(500);
/// `WATCHDOG_GRACE` after `answer_deadline`, and at most a quarter of the
/// margin: 28.5 s for the unit's 30 s, 1.5 s before its deadline.
pub fn watchdog_deadline(timeout: i32, started: Instant) -> Option<Instant> {
    (timeout > 0).then(|| {
        let total = Duration::from_millis(timeout as u64);
        let margin = ANSWER_MARGIN.min(total / 2);
        started + total.saturating_sub(margin) + WATCHDOG_GRACE.min(margin / 4)
    })
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
        29 => forward_product_with_fetcher_at(fetcher, payload, cancel, sink, started).map(|_| ()),
        _ => Err(ForwardError::Protocol),
    }
}

/// How a forwarded Type 29 was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductAnswer {
    /// With the HTTP status the server sent.
    Server(i32),
    /// The exchange did not finish by its answer deadline.
    TimedOut,
    /// The exchange failed (connection, TLS, protocol).
    NetworkFailed,
}

/// Forward one decrypted Type 29 and answer it, as `forward_with_fetcher_at`
/// does, reporting how it was answered.
pub fn forward_product_with_fetcher_at(
    fetcher: &http::DirectHttpFetcher,
    payload: &[u8],
    cancel: Arc<AtomicBool>,
    sink: &mut dyn FnMut(OutboundMessage, Option<Instant>) -> Result<(), ForwardError>,
    started: Instant,
) -> Result<ProductAnswer, ForwardError> {
    check(&cancel, None)?;
    let mut message =
        pa::ProductForwardMessage::decode(payload).map_err(|_| ForwardError::Protocol)?;
    let incoming = message
        .request_id
        .take()
        .map(|pa::product_forward_message::RequestId::RequestId(id)| id);
    static NEXT_RESPONSE_REQUEST_ID: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(8);
    let request_id =
        Some(incoming.unwrap_or_else(|| NEXT_RESPONSE_REQUEST_ID.fetch_add(1, Ordering::Relaxed)));
    let Some(pa::product_forward_message::Request::Request(request)) = message.request.take()
    else {
        return Err(ForwardError::Protocol);
    };
    // The unit's deadline bounds USB admission of the answer; the HTTP
    // exchange (head and streamed body share `answer_by`) stops
    // `ANSWER_MARGIN` earlier, so a slow or silent server still
    // produces an answer the unit receives in time.
    let deadline = deadline(request.timeout, started);
    let answer_by = answer_deadline(request.timeout, started);
    check(&cancel, deadline)?;
    let timeout = answer_by
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
        let url = url::Url::parse(&request.url).map_err(|_| fetch::FetchFailure::Protocol)?;
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
            answer_by,
            Arc::clone(&cancel),
        )?;
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(piece) = response.body.next_chunk(timeout)? {
            bytes.extend_from_slice(&piece);
        }
        Ok((response.status, std::mem::take(&mut *bytes)))
    })();
    // Only cancellation (session retired, relay owner gone) or a
    // missed device deadline suppresses the answer; an HTTP timeout
    // is answered while the unit still waits.
    check(&cancel, deadline)?;
    // A failed exchange is answered too, with the one made-up status
    // (`cloud_forward::BROKER_ANSWER_STATUS`); the caller learns which.
    let (status, bytes, answer) = match result {
        Ok((status, bytes)) => {
            let status = i32::from(status);
            (status, bytes, ProductAnswer::Server(status))
        }
        Err(fetch::FetchFailure::Timeout) => (
            cloud_forward::BROKER_ANSWER_STATUS,
            Vec::new(),
            ProductAnswer::TimedOut,
        ),
        Err(_) => (
            cloud_forward::BROKER_ANSWER_STATUS,
            Vec::new(),
            ProductAnswer::NetworkFailed,
        ),
    };
    let response = cloud_forward::product_forward_response(
        request_id,
        message.cloud_request_id,
        bytes,
        status,
    );
    sink(response, deadline)?;
    check(&cancel, deadline)?;
    Ok(answer)
}

#[cfg(test)]
mod tests;
