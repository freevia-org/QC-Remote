//! Type 29 adapter for the shared direct HTTP transport. No request policy.
//!
//! The unit gives each Type 29 a deadline (30-40 s in every capture). When it
//! passes unanswered, the unit sends CloudLogin{DELETE} about 30 s later and
//! drops its account link (captured three times). So every decodable Type 29
//! is answered before its deadline: with the forwarded HTTP result, or with
//! `BROKER_ANSWER_STATUS` when the exchange is still running at its watchdog
//! time or the request is not forwarded at all. The only remaining silence
//! is a request whose answer cannot be written (no usable session key, or no
//! link); it is named in `take_events` rather than lost.
use super::http::DirectHttpFetcher;
use crate::ProductAnswer;
use prost::Message;
use quadcortex::device::transport::CloudAnswerKey;
use quadcortex::{commands::OutboundMessage, proto::cortex_protobuf_v2 as pa};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

/// The HTTP status of every answer the host makes up itself: relay off,
/// malformed request, an exchange retired by an epoch change or teardown, an
/// exchange still running at its watchdog time, or one whose HTTP request
/// timed out or failed. The flight record keeps the reason (`reason=timeout`,
/// `reason=network`, ...). Never 401, which tells the unit its credentials
/// are invalid, and no other made-up status (502 and 504 are not used).
///
/// Evidence (2026-10-14/15 capture): the unit's only DELETEs followed an
/// unanswered Authorization-bearing user-route POST. 500 is the one server
/// error answered there with no DELETE after it (seven times, other route).
/// 503 and 504 were never observed. Until a hardware run proves otherwise
/// this stays 500. Planned check: answer that POST with each candidate
/// status (500, 503, 504) on a unit with a linked account and watch the
/// flight record for `cloud-login action=DELETE` within 90 s.
pub const BROKER_ANSWER_STATUS: i32 = 500;
/// The deadline assumed for a request that names none (`timeout` 0): the
/// 30 s the unit uses for its account requests.
pub const DEFAULT_TIMEOUT_MS: i32 = 30_000;
/// Flight-record lines retained between drains.
const MAX_EVENTS: usize = 64;

/// Only the action of a CloudLogin (Type 18). The username and tokens it can
/// carry are skipped by the decoder rather than copied and zeroized.
#[derive(Clone, PartialEq, prost::Message)]
struct CloudLoginAction {
    #[prost(int32, tag = "1")]
    action: i32,
}

/// Where and when an account frame arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestOrigin {
    /// The session epoch it arrived in.
    pub epoch: u64,
    /// The transport key generation it was decrypted with; an answer is
    /// written only under the same one.
    pub key_generation: u64,
    /// When its first HID report arrived: the unit's clock starts there.
    pub received_at: Instant,
}

pub struct CloudResponse {
    pub deadline: Option<std::time::Instant>,
    pub cancel: Arc<AtomicBool>,
    pub messages: Vec<OutboundMessage>,
}
impl std::ops::Deref for CloudResponse {
    type Target = [OutboundMessage];
    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}
impl Drop for CloudResponse {
    fn drop(&mut self) {
        for message in &mut self.messages {
            message.payload.zeroize();
        }
    }
}
struct SecretResponse {
    cloud_request_id: u64,
    deadline: Instant,
    key_generation: u64,
    answer: Option<ProductAnswer>,
    cancel: Arc<AtomicBool>,
    messages: Vec<OutboundMessage>,
}
impl Drop for SecretResponse {
    fn drop(&mut self) {
        for m in &mut self.messages {
            m.payload.zeroize();
        }
    }
}
struct Task {
    cancel: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}
/// A forwarded request the unit is still waiting for.
struct Pending {
    cloud_request_id: u64,
    request_id: Option<u64>,
    epoch: u64,
    key_generation: u64,
    deadline: Instant,
    watchdog_at: Instant,
    cancel: Arc<AtomicBool>,
}
/// An answer owed to the unit for a request that is not being forwarded.
struct Refusal {
    cloud_request_id: u64,
    request_id: Option<u64>,
    deadline: Instant,
    key_generation: u64,
}
pub struct CloudForwardRelay {
    enabled: bool,
    next_response_request_id: u64,
    sender: Sender<SecretResponse>,
    replies: Receiver<SecretResponse>,
    tasks: Vec<Task>,
    response_cancellations: Vec<std::sync::Weak<AtomicBool>>,
    fetcher: std::sync::Arc<DirectHttpFetcher>,
    pending: Vec<Pending>,
    held: VecDeque<SecretResponse>,
    refusals: VecDeque<Refusal>,
    events: Vec<String>,
    events_dropped: usize,
    undecodable: u32,
}
impl Default for CloudForwardRelay {
    fn default() -> Self {
        Self::with_fetcher(DirectHttpFetcher::default())
    }
}
impl CloudForwardRelay {
    pub fn with_fetcher(fetcher: DirectHttpFetcher) -> Self {
        let (sender, replies) = mpsc::channel();
        Self {
            enabled: false,
            next_response_request_id: 8,
            sender,
            replies,
            tasks: Vec::new(),
            response_cancellations: Vec::new(),
            fetcher: std::sync::Arc::new(fetcher),
            pending: Vec::new(),
            held: VecDeque::new(),
            refusals: VecDeque::new(),
            events: Vec::new(),
            events_dropped: 0,
            undecodable: 0,
        }
    }
}
impl CloudForwardRelay {
    /// Switching forwarding off answers every exchange still running with
    /// `BROKER_ANSWER_STATUS`; an answer already fetched is still delivered,
    /// including one already handed to USB. While off, each new request is
    /// answered at once. Nothing is sent to the network while off.
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled && self.enabled {
            self.refuse_outstanding("relay-off");
        }
        self.enabled = enabled;
    }
    /// Retire all requests immediately without joining on the USB owner thread.
    /// A cancelled response can never become current again after re-enabling.
    pub fn cancel_all(&mut self) {
        self.abandon_all("cancelled");
    }
    /// For a USB session that has gone and so can carry no more answers:
    /// everything is cancelled, including answers already handed to USB, and
    /// each request the unit was still owed is named.
    pub fn abandon_all(&mut self, reason: &str) {
        self.collect_replies();
        for cancel in self
            .response_cancellations
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            cancel.store(true, Ordering::Release);
        }
        self.response_cancellations.clear();
        for task in &self.tasks {
            task.cancel.store(true, Ordering::Release);
        }
        self.retire_owed(reason);
    }
    /// For a live USB session this host is about to close in order: the
    /// exchanges still running are cancelled and each request still owed is
    /// named, but answers already handed to USB keep their admission, so the
    /// orderly close still writes them. Use after the answers were flushed.
    pub fn retire_for_teardown(&mut self, reason: &str) {
        self.collect_replies();
        // A task's cancel flag is also its answer's: cancel only exchanges
        // still running, never a finished task whose answer is in flight.
        for pending in &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        self.response_cancellations.clear();
        self.retire_owed(reason);
    }
    /// Answers fetched and not yet written, after collecting finished ones.
    pub fn fetched_answers(&mut self) -> usize {
        self.collect_replies();
        self.held.len()
    }
    fn retire_owed(&mut self, reason: &str) {
        let now = Instant::now();
        let mut owed: Vec<u64> = Vec::new();
        let ids = self
            .pending
            .drain(..)
            .map(|pending| (pending.cloud_request_id, pending.deadline))
            .chain(
                self.held
                    .drain(..)
                    .map(|reply| (reply.cloud_request_id, reply.deadline)),
            )
            .chain(
                self.refusals
                    .drain(..)
                    .map(|refusal| (refusal.cloud_request_id, refusal.deadline)),
            );
        for (id, deadline) in ids {
            if now < deadline && !owed.contains(&id) {
                owed.push(id);
            }
        }
        for id in owed {
            self.event(format!("cloud-request-unanswered id={id} reason={reason}"));
        }
    }
    /// For a USB session that stays up: deliver every answer already
    /// fetched, and answer each exchange still running with
    /// `BROKER_ANSWER_STATUS` instead of leaving it to the unit's deadline.
    pub fn refuse_outstanding(&mut self, reason: &str) {
        self.collect_replies();
        for pending in std::mem::take(&mut self.pending) {
            if self.has_reply(pending.cloud_request_id) {
                self.pending.push(pending);
                continue;
            }
            pending.cancel.store(true, Ordering::Release);
            self.refuse_pending(&pending, reason);
        }
    }
    /// Redacted flight-record lines: request ids, methods, timeouts, statuses,
    /// remaining time and reasons only, never a URL, header, body, token or
    /// device identity. Lines past the buffer are counted in a final marker.
    pub fn take_events(&mut self) -> Vec<String> {
        let mut events = std::mem::take(&mut self.events);
        let dropped = std::mem::take(&mut self.events_dropped);
        if dropped > 0 {
            events.push(format!("cloud-events-dropped n={dropped}"));
        }
        events
    }
    /// Account frames that decrypted to something undecodable since the
    /// last call: with a key installed, the sign of a key the unit did not use.
    pub fn take_undecodable(&mut self) -> u32 {
        std::mem::take(&mut self.undecodable)
    }
    fn event(&mut self, line: String) {
        if self.events.len() < MAX_EVENTS {
            self.events.push(line);
        } else {
            self.events_dropped = self.events_dropped.saturating_add(1);
        }
    }
    fn has_reply(&self, cloud_request_id: u64) -> bool {
        self.held.iter().any(|reply| {
            reply.cloud_request_id == cloud_request_id && !reply.cancel.load(Ordering::Acquire)
        })
    }
    fn refuse_pending(&mut self, pending: &Pending, reason: &str) {
        self.refuse(
            Refusal {
                cloud_request_id: pending.cloud_request_id,
                request_id: pending.request_id,
                deadline: pending.deadline,
                key_generation: pending.key_generation,
            },
            reason,
        );
    }
    fn refuse(&mut self, refusal: Refusal, reason: &str) {
        let id = refusal.cloud_request_id;
        if Instant::now() >= refusal.deadline {
            self.event(format!("cloud-request-unanswered id={id} reason={reason}"));
            return;
        }
        self.event(format!(
            "cloud-request-refused id={id} status={BROKER_ANSWER_STATUS} reason={reason} \
             remaining_ms={}",
            remaining_ms(refusal.deadline)
        ));
        self.refusals.push_back(refusal);
    }
    /// Move finished exchanges into `held`; one that ended without an
    /// answer frame is refused, since the unit still waits.
    fn collect_replies(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            if reply.cancel.load(Ordering::Acquire) {
                continue;
            }
            if reply.messages.is_empty() {
                let index = self
                    .pending
                    .iter()
                    .position(|pending| pending.cloud_request_id == reply.cloud_request_id);
                match index.map(|index| self.pending.swap_remove(index)) {
                    Some(pending) => self.refuse_pending(&pending, "forward-failed"),
                    None => self.event(format!(
                        "cloud-request-unanswered id={} reason=forward-failed",
                        reply.cloud_request_id
                    )),
                }
                continue;
            }
            self.held.push_back(reply);
        }
    }
    fn response_request_id(&mut self, incoming: Option<u64>) -> u64 {
        incoming.unwrap_or_else(|| {
            let id = self.next_response_request_id;
            self.next_response_request_id = self.next_response_request_id.wrapping_add(1);
            id
        })
    }
    pub fn on_cloud_login(&mut self, payload: &mut Vec<u8>) {
        // Type 18 is a device notification with no HTTP request to forward.
        // Its action is recorded: DELETE is the unit dropping its account link.
        let action = CloudLoginAction::decode(payload.as_slice()).map(|login| login.action);
        payload.zeroize();
        let action = match action.map(pa::message_action::Enum::try_from) {
            Ok(Ok(pa::message_action::Enum::Create)) => "CREATE".to_string(),
            Ok(Ok(pa::message_action::Enum::Update)) => "UPDATE".to_string(),
            Ok(Ok(pa::message_action::Enum::Delete)) => "DELETE".to_string(),
            Ok(Ok(other)) => (other as i32).to_string(),
            Ok(Err(unknown)) => unknown.0.to_string(),
            Err(_) => {
                self.undecodable = self.undecodable.saturating_add(1);
                "undecodable".to_string()
            }
        };
        self.event(format!("cloud-login action={action}"));
    }
    /// A Type 29 that arrived in a lifecycle epoch the session has left. It is
    /// not forwarded, but the unit is still waiting for it.
    pub fn refuse_product_forward(
        &mut self,
        payload: &mut Vec<u8>,
        reason: &str,
        origin: RequestOrigin,
    ) {
        let Some((cloud_id, request_id, request)) = self.decode_request(payload) else {
            return;
        };
        let timeout = request.as_ref().map_or(0, |request| request.timeout);
        if let Some(mut request) = request {
            zeroize_request(&mut request);
        }
        self.event(format!("cloud-request id={cloud_id} op=retired"));
        self.refuse(
            Refusal {
                cloud_request_id: cloud_id,
                request_id,
                deadline: unit_deadline(timeout, origin.received_at),
                key_generation: origin.key_generation,
            },
            reason,
        );
    }
    /// Decode a Type 29 and zeroize its wire bytes. `None` when nothing is
    /// owed: undecodable (no id to echo) or the unit's own answer frame.
    #[allow(clippy::type_complexity)]
    fn decode_request(
        &mut self,
        payload: &mut Vec<u8>,
    ) -> Option<(u64, Option<u64>, Option<pa::ProductForwardRequest>)> {
        let decoded = pa::ProductForwardMessage::decode(payload.as_slice());
        payload.zeroize();
        let Ok(mut message) = decoded else {
            self.undecodable = self.undecodable.saturating_add(1);
            self.event("cloud-request-dropped reason=undecodable".into());
            return None;
        };
        let incoming = message
            .request_id
            .take()
            .map(|pa::product_forward_message::RequestId::RequestId(id)| id);
        let request_id = Some(self.response_request_id(incoming));
        let cloud_id = message.cloud_request_id;
        let request = message
            .request
            .take()
            .map(|pa::product_forward_message::Request::Request(request)| request);
        if request.is_none() && zeroize_response(&mut message) {
            // The unit's own answer frame, not a request: nothing is owed.
            self.event(format!("cloud-request id={cloud_id} op=response-only"));
            return None;
        }
        Some((cloud_id, request_id, request))
    }
    pub fn on_product_forward(&mut self, payload: &mut Vec<u8>, origin: RequestOrigin) {
        let Some((cloud_id, request_id, request)) = self.decode_request(payload) else {
            return;
        };
        let started = origin.received_at;
        let Some(mut request) = request else {
            self.event(format!("cloud-request id={cloud_id} op=none"));
            self.refuse(
                Refusal {
                    cloud_request_id: cloud_id,
                    request_id,
                    deadline: unit_deadline(0, started),
                    key_generation: origin.key_generation,
                },
                "malformed",
            );
            return;
        };
        let deadline = unit_deadline(request.timeout, started);
        self.event(format!(
            "cloud-request id={cloud_id} op={} timeout_ms={}",
            operation_name(request.operation),
            request.timeout
        ));
        if !self.enabled {
            zeroize_request(&mut request);
            self.refuse(
                Refusal {
                    cloud_request_id: cloud_id,
                    request_id,
                    deadline,
                    key_generation: origin.key_generation,
                },
                "relay-off",
            );
            return;
        }
        let watchdog_at = crate::watchdog_deadline(effective_timeout(request.timeout), started)
            .unwrap_or(deadline);
        let sender = self.sender.clone();
        let fetcher = std::sync::Arc::clone(&self.fetcher);
        let cancel = Arc::new(AtomicBool::new(false));
        self.response_cancellations
            .retain(|cancel| cancel.strong_count() > 0);
        self.response_cancellations.push(Arc::downgrade(&cancel));
        let task_cancel = Arc::clone(&cancel);
        let key_generation = origin.key_generation;
        let spawned = thread::Builder::new()
            .name("qc-http-product".into())
            .spawn(move || {
                let mut message = pa::ProductForwardMessage {
                    cloud_request_id: cloud_id,
                    request_id: request_id.map(pa::product_forward_message::RequestId::RequestId),
                    request: Some(pa::product_forward_message::Request::Request(request)),
                    ..Default::default()
                };
                let wire = Zeroizing::new(message.encode_to_vec());
                if let Some(pa::product_forward_message::Request::Request(request)) =
                    &mut message.request
                {
                    zeroize_request(request);
                }
                let mut messages = Vec::new();
                let mut limit = None;
                let answer = crate::forward_product_with_fetcher_at(
                    &fetcher,
                    &wire,
                    Arc::clone(&task_cancel),
                    &mut |message, admission| {
                        limit = admission;
                        messages.push(message);
                        Ok(())
                    },
                    started,
                )
                .ok();
                if !task_cancel.load(Ordering::Acquire) {
                    let _ = sender.send(SecretResponse {
                        cloud_request_id: cloud_id,
                        deadline: limit.unwrap_or(deadline),
                        key_generation,
                        answer,
                        cancel: Arc::clone(&task_cancel),
                        messages,
                    });
                }
            });
        match spawned {
            Ok(thread) => {
                self.tasks.push(Task {
                    cancel: Arc::clone(&cancel),
                    thread,
                });
                self.pending.push(Pending {
                    cloud_request_id: cloud_id,
                    request_id,
                    epoch: origin.epoch,
                    key_generation,
                    deadline,
                    watchdog_at,
                    cancel,
                });
            }
            Err(_) => self.refuse(
                Refusal {
                    cloud_request_id: cloud_id,
                    request_id,
                    deadline,
                    key_generation,
                },
                "spawn-failed",
            ),
        }
    }
    /// The next answer to write. `answer_key` says whether one may be
    /// encrypted now (`TransportRuntime::cloud_answer_key`): while not,
    /// answers wait, and any whose deadline passes meanwhile is named with
    /// `reason=not-ready` or `reason=no-session-key`. An answer prepared under
    /// another key generation is named and never written.
    pub fn take_response_when(
        &mut self,
        epoch: u64,
        answer_key: CloudAnswerKey,
    ) -> Option<CloudResponse> {
        let mut i = 0;
        while i < self.tasks.len() {
            if self.tasks[i].thread.is_finished() {
                let task = self.tasks.swap_remove(i);
                let _ = task.thread.join();
            } else {
                i += 1;
            }
        }
        self.collect_replies();
        // An exchange the session has moved past, or still running at its
        // watchdog time, is answered now; a fetched answer is never replaced.
        let now = Instant::now();
        for pending in std::mem::take(&mut self.pending) {
            let reason = if self.has_reply(pending.cloud_request_id) {
                None
            } else if pending.epoch != epoch {
                Some("epoch-changed")
            } else if now >= pending.watchdog_at {
                Some("answer-deadline")
            } else {
                None
            };
            match reason {
                Some(reason) => {
                    pending.cancel.store(true, Ordering::Release);
                    self.refuse_pending(&pending, reason);
                }
                None => self.pending.push(pending),
            }
        }
        let mut lapsed = Vec::new();
        let (key, waiting) = match answer_key {
            CloudAnswerKey::Ready(generation) => (Some(generation), "deadline-passed"),
            CloudAnswerKey::NotReady => (None, "not-ready"),
            CloudAnswerKey::Missing => (None, "no-session-key"),
        };
        let usable = |deadline: Instant, generation: u64| -> Result<(), &'static str> {
            if now >= deadline {
                Err(waiting)
            } else if key.is_some_and(|key| key != generation) {
                Err("key-changed")
            } else {
                Ok(())
            }
        };
        self.refusals.retain(
            |refusal| match usable(refusal.deadline, refusal.key_generation) {
                Ok(()) => true,
                Err(reason) => {
                    lapsed.push((refusal.cloud_request_id, reason));
                    false
                }
            },
        );
        self.held.retain(|reply| {
            if reply.cancel.load(Ordering::Acquire) {
                return false;
            }
            match usable(reply.deadline, reply.key_generation) {
                Ok(()) => true,
                Err(reason) => {
                    lapsed.push((reply.cloud_request_id, reason));
                    false
                }
            }
        });
        for (id, reason) in lapsed {
            self.retire_pending(id);
            self.event(format!("cloud-request-unanswered id={id} reason={reason}"));
        }
        key?;
        // A real answer goes before any made-up one.
        if let Some(mut reply) = self.held.pop_front() {
            self.retire_pending(reply.cloud_request_id);
            let status = reply.messages.first().and_then(response_status);
            let reason = match reply.answer {
                Some(ProductAnswer::TimedOut) => " reason=timeout",
                Some(ProductAnswer::NetworkFailed) => " reason=network",
                Some(ProductAnswer::Server(_)) | None => "",
            };
            self.event(format!(
                "cloud-request-answered id={} status={}{reason} remaining_ms={}",
                reply.cloud_request_id,
                status.map_or_else(|| "unknown".to_string(), |status| status.to_string()),
                remaining_ms(reply.deadline)
            ));
            return Some(CloudResponse {
                deadline: Some(reply.deadline),
                cancel: Arc::clone(&reply.cancel),
                messages: std::mem::take(&mut reply.messages),
            });
        }
        let refusal = self.refusals.pop_front()?;
        Some(CloudResponse {
            deadline: Some(refusal.deadline),
            cancel: Arc::new(AtomicBool::new(false)),
            messages: vec![product_forward_response(
                refusal.request_id,
                refusal.cloud_request_id,
                Vec::new(),
                BROKER_ANSWER_STATUS,
            )],
        })
    }
    fn retire_pending(&mut self, cloud_request_id: u64) {
        self.pending
            .retain(|pending| pending.cloud_request_id != cloud_request_id);
    }
}
impl Drop for CloudForwardRelay {
    fn drop(&mut self) {
        self.cancel_all();
        for task in self.tasks.drain(..) {
            let _ = task.thread.join();
        }
    }
}

fn effective_timeout(timeout: i32) -> i32 {
    if timeout > 0 {
        timeout
    } else {
        DEFAULT_TIMEOUT_MS
    }
}

/// The unit's own deadline for a request, counted from its first report.
fn unit_deadline(timeout: i32, received_at: Instant) -> Instant {
    received_at + Duration::from_millis(effective_timeout(timeout) as u64)
}

fn remaining_ms(deadline: Instant) -> u128 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
}

fn zeroize_request(request: &mut pa::ProductForwardRequest) {
    if let Some(pa::product_forward_request::Payload::Payload(bytes)) = &mut request.payload {
        bytes.zeroize();
    }
    for header in &mut request.headers {
        header.name.zeroize();
        header.value.zeroize();
    }
    request.url.zeroize();
}

/// Zeroize a response the message carries; whether it carried one.
fn zeroize_response(message: &mut pa::ProductForwardMessage) -> bool {
    match &mut message.response {
        Some(pa::product_forward_message::Response::Response(response)) => {
            response.payload.zeroize();
            true
        }
        None => false,
    }
}

fn operation_name(operation: i32) -> String {
    match pa::http_operation::Enum::try_from(operation) {
        Ok(pa::http_operation::Enum::Get) => "GET".into(),
        Ok(pa::http_operation::Enum::Put) => "PUT".into(),
        Ok(pa::http_operation::Enum::Post) => "POST".into(),
        Ok(pa::http_operation::Enum::Delete) => "DELETE".into(),
        Err(_) => format!("unknown-{operation}"),
    }
}

/// The HTTP status an encoded answer carries, for the flight record only.
fn response_status(message: &OutboundMessage) -> Option<i32> {
    let mut decoded = pa::ProductForwardMessage::decode(message.payload.as_slice()).ok()?;
    match &mut decoded.response {
        Some(pa::product_forward_message::Response::Response(response)) => {
            response.payload.zeroize();
            Some(response.error_code)
        }
        None => None,
    }
}

pub fn product_forward_response(
    request_id: Option<u64>,
    cloud_request_id: u64,
    response_payload: Vec<u8>,
    error_code: i32,
) -> OutboundMessage {
    let mut response_payload = Zeroizing::new(response_payload);
    let mut response = pa::ProductForwardMessage {
        action: pa::message_action::Enum::Update as i32,
        request_id: request_id.map(pa::product_forward_message::RequestId::RequestId),
        cloud_request_id,
        response: Some(pa::product_forward_message::Response::Response(
            pa::ProductForwardResponse {
                payload: std::mem::take(&mut *response_payload),
                error_code,
            },
        )),
        ..Default::default()
    };
    let mut wire_payload = response.encode_to_vec();
    if let Some(pa::product_forward_message::Response::Response(response)) = &mut response.response
    {
        response.payload.zeroize();
    }
    response_payload.zeroize();
    let message = OutboundMessage {
        message_type: 29,
        payload: std::mem::take(&mut wire_payload),
    };
    wire_payload.zeroize();
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const KEY: CloudAnswerKey = CloudAnswerKey::Ready(0);

    fn origin(epoch: u64) -> RequestOrigin {
        RequestOrigin {
            epoch,
            key_generation: 0,
            received_at: Instant::now(),
        }
    }

    fn account_request(cloud_request_id: u64, url: &str, timeout: i32) -> Vec<u8> {
        pa::ProductForwardMessage {
            action: 1,
            cloud_request_id,
            request: Some(pa::product_forward_message::Request::Request(
                pa::ProductForwardRequest {
                    operation: pa::http_operation::Enum::Post as i32,
                    url: url.into(),
                    timeout,
                    headers: vec![pa::ProductForwardHeaderPair {
                        name: "Authorization".into(),
                        value: "Bearer secret-token".into(),
                    }],
                    payload: Some(pa::product_forward_request::Payload::Payload(
                        b"secret-body".to_vec(),
                    )),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    /// (cloud_request_id, request_id, HTTP status) of a single-frame answer.
    fn answer(response: &CloudResponse) -> (u64, Option<u64>, i32) {
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].message_type, 29);
        let reply = pa::ProductForwardMessage::decode(response[0].payload.as_slice()).unwrap();
        let Some(pa::product_forward_message::Response::Response(actual)) = reply.response else {
            panic!("missing response")
        };
        let request_id = reply
            .request_id
            .map(|pa::product_forward_message::RequestId::RequestId(id)| id);
        (reply.cloud_request_id, request_id, actual.error_code)
    }

    fn assert_redacted(events: &[String]) {
        for line in events {
            for secret in ["127.0.0.1", "secret", "Bearer", "Authorization", "user"] {
                assert!(!line.contains(secret), "{line}");
            }
        }
    }

    fn has_event(events: &[String], prefix: &str) -> bool {
        events.iter().any(|line| line.starts_with(prefix))
    }

    fn pending(relay: &mut CloudForwardRelay, id: u64, epoch: u64, watchdog_in: i64) {
        let now = Instant::now();
        let watchdog_at = if watchdog_in >= 0 {
            now + Duration::from_millis(watchdog_in as u64)
        } else {
            now - Duration::from_millis(watchdog_in.unsigned_abs())
        };
        relay.pending.push(Pending {
            cloud_request_id: id,
            request_id: Some(id + 1000),
            epoch,
            key_generation: 0,
            deadline: now + Duration::from_secs(5),
            watchdog_at,
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }

    fn fetched(relay: &mut CloudForwardRelay, id: u64, status: i32) -> Arc<AtomicBool> {
        let cancel = relay
            .pending
            .iter()
            .find(|pending| pending.cloud_request_id == id)
            .map_or_else(
                || Arc::new(AtomicBool::new(false)),
                |p| Arc::clone(&p.cancel),
            );
        relay
            .sender
            .send(SecretResponse {
                cloud_request_id: id,
                deadline: Instant::now() + Duration::from_secs(5),
                key_generation: 0,
                answer: None,
                cancel: Arc::clone(&cancel),
                messages: vec![product_forward_response(
                    Some(id + 1000),
                    id,
                    vec![7],
                    status,
                )],
            })
            .ok()
            .unwrap();
        cancel
    }

    #[test]
    fn product_forward_translates_only_the_envelope_and_preserves_large_error_body() {
        let response = vec![0xff; 600 * 1024];
        let (url, received, server) = super::super::http::wire_tests::serve(401, response.clone());
        let body = vec![0xab; 100 * 1024];
        let mut message = pa::ProductForwardMessage {
            action: 1,
            cloud_request_id: 55,
            request: Some(pa::product_forward_message::Request::Request(
                pa::ProductForwardRequest {
                    operation: 2,
                    url: url.as_str().into(),
                    timeout: 0,
                    is_multipart: true,
                    payload: Some(pa::product_forward_request::Payload::Payload(body.clone())),
                    headers: (0..50)
                        .map(|i| pa::ProductForwardHeaderPair {
                            name: format!("X-Device-{i}"),
                            value: "exact".into(),
                        })
                        .collect(),
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.on_product_forward(&mut message, origin(7));
        assert!(message.is_empty(), "zeroized");
        let deadline = Instant::now() + Duration::from_secs(5);
        let replies = loop {
            if let Some(replies) = relay.take_response_when(7, KEY) {
                break replies;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(replies.len(), 1);
        let reply = pa::ProductForwardMessage::decode(replies[0].payload.as_slice()).unwrap();
        assert_eq!(reply.cloud_request_id, 55);
        assert!(matches!(
            reply.request_id,
            Some(pa::product_forward_message::RequestId::RequestId(8))
        ));
        let Some(pa::product_forward_message::Response::Response(actual)) = reply.response else {
            panic!("missing response")
        };
        // A status the server sent is forwarded as it is, 401 included.
        assert_eq!(actual.error_code, 401);
        assert_eq!(actual.payload, response);
        let events = relay.take_events();
        assert!(events.contains(&"cloud-request id=55 op=POST timeout_ms=0".to_string()));
        assert!(has_event(
            &events,
            "cloud-request-answered id=55 status=401 remaining_ms="
        ));
        assert!(!events.iter().any(|line| line.contains("127.0.0.1")));
        let request = received.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap().unwrap();
        let end = request.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
        assert_eq!(&request[end + 4..], body);
        assert!(String::from_utf8_lossy(&request[..end])
            .to_ascii_lowercase()
            .contains("x-device-49: exact"));
    }

    #[test]
    fn dropping_account_adapter_cancels_a_zero_timeout_exchange() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut block = [0; 4096];
            let _ = socket.read(&mut block);
            tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(300));
        });
        let mut payload = account_request(56, &format!("http://{address}/"), 0);
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.on_product_forward(&mut payload, origin(7));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        drop(relay);
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }

    #[test]
    fn session_retirement_cancels_zero_timeout_socket_and_rejects_old_reply() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                assert_eq!(socket.read(&mut byte).unwrap(), 1);
                head.push(byte[0]);
            }
            ready_tx.send(()).unwrap();
            socket.read(&mut byte)
        });
        let mut payload = pa::ProductForwardMessage {
            cloud_request_id: 9,
            request: Some(pa::product_forward_message::Request::Request(
                pa::ProductForwardRequest {
                    operation: 0,
                    url: format!("http://{address}/"),
                    timeout: 0,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec();
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.on_product_forward(&mut payload, origin(3));
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        relay.cancel_all();
        assert!(started.elapsed() < Duration::from_millis(100));
        let closed = server.join().unwrap();
        assert!(
            matches!(closed, Ok(0))
                || matches!(closed, Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset)
        );
        relay.set_enabled(false);
        relay.set_enabled(true);
        assert!(relay.take_response_when(3, KEY).is_none());
        assert!(relay
            .take_events()
            .contains(&"cloud-request-unanswered id=9 reason=cancelled".to_string()));
        drop(relay);
    }

    #[test]
    fn delayed_drain_drops_a_completed_reply_after_its_absolute_deadline() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        let expired = Instant::now() - Duration::from_millis(1);
        relay
            .sender
            .send(SecretResponse {
                cloud_request_id: 9,
                deadline: expired,
                key_generation: 0,
                answer: None,
                cancel: Arc::new(AtomicBool::new(false)),
                messages: vec![product_forward_response(Some(8), 9, vec![1, 2, 3], 200)],
            })
            .ok()
            .unwrap();
        assert!(relay.take_response_when(3, KEY).is_none());
        assert!(relay
            .take_events()
            .contains(&"cloud-request-unanswered id=9 reason=deadline-passed".to_string()));
        let deadline = Instant::now() + Duration::from_secs(1);
        relay
            .sender
            .send(SecretResponse {
                cloud_request_id: 9,
                deadline,
                key_generation: 0,
                answer: None,
                cancel: Arc::new(AtomicBool::new(false)),
                messages: vec![product_forward_response(Some(8), 9, vec![1, 2, 3], 200)],
            })
            .ok()
            .unwrap();
        let response = relay.take_response_when(3, KEY).unwrap();
        assert_eq!(response.deadline, Some(deadline));
        assert_eq!(response.len(), 1);
    }

    #[test]
    fn disabling_keeps_a_reply_already_transferred_to_usb_ownership() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        let cancel = Arc::new(AtomicBool::new(false));
        relay.response_cancellations.push(Arc::downgrade(&cancel));
        relay
            .sender
            .send(SecretResponse {
                cloud_request_id: 9,
                deadline: Instant::now() + Duration::from_secs(5),
                key_generation: 0,
                answer: None,
                cancel: Arc::clone(&cancel),
                messages: vec![product_forward_response(Some(8), 9, vec![1], 200)],
            })
            .ok()
            .unwrap();
        let response = relay.take_response_when(3, KEY).unwrap();
        assert!(relay.tasks.is_empty());
        // The link stays up, so the answer in flight to the unit is kept.
        relay.set_enabled(false);
        assert!(!response.cancel.load(Ordering::Acquire));
        relay.set_enabled(true);
        assert!(!response.cancel.load(Ordering::Acquire));
        // A session that ends takes it down.
        relay.abandon_all("device-session-closed");
        assert!(response.cancel.load(Ordering::Acquire));
    }

    #[test]
    fn relay_off_answers_each_request_at_once_and_never_forwards_it() {
        let mut relay = CloudForwardRelay::default();
        let mut payload = account_request(202, "http://127.0.0.1:1/must-not-connect", 30_000);
        // The unit's clock started at the frame's first report, 10 s ago.
        let Some(received_at) = Instant::now().checked_sub(Duration::from_secs(10)) else {
            return; // A host up for under 10 s cannot express that instant.
        };
        relay.on_product_forward(
            &mut payload,
            RequestOrigin {
                epoch: 3,
                key_generation: 0,
                received_at,
            },
        );
        assert!(payload.is_empty(), "zeroized");
        assert!(relay.tasks.is_empty() && relay.pending.is_empty());
        let response = relay.take_response_when(3, KEY).unwrap();
        assert_eq!(answer(&response), (202, Some(8), BROKER_ANSWER_STATUS));
        assert_eq!(
            response.deadline,
            Some(received_at + Duration::from_millis(30_000))
        );
        let events = relay.take_events();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0], "cloud-request id=202 op=POST timeout_ms=30000");
        assert!(events[1]
            .starts_with("cloud-request-refused id=202 status=500 reason=relay-off remaining_ms="));
        assert_redacted(&events);
        assert!(relay.take_response_when(3, KEY).is_none());
    }

    #[test]
    fn a_request_without_a_deadline_gets_the_units_default_one() {
        let mut relay = CloudForwardRelay::default();
        let received_at = Instant::now();
        relay.on_product_forward(
            &mut account_request(203, "http://127.0.0.1:1/", 0),
            RequestOrigin {
                epoch: 3,
                key_generation: 0,
                received_at,
            },
        );
        let response = relay.take_response_when(3, KEY).unwrap();
        let default = Duration::from_millis(DEFAULT_TIMEOUT_MS as u64);
        assert_eq!(response.deadline, Some(received_at + default));
    }

    #[test]
    fn a_request_without_a_request_body_is_refused_but_a_response_frame_is_not() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        let mut malformed = pa::ProductForwardMessage {
            action: 1,
            cloud_request_id: 77,
            request_id: Some(pa::product_forward_message::RequestId::RequestId(40)),
            ..Default::default()
        }
        .encode_to_vec();
        relay.on_product_forward(&mut malformed, origin(3));
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()),
            (77, Some(40), BROKER_ANSWER_STATUS)
        );
        let mut echoed = product_forward_response(Some(41), 34, vec![1, 2], 0)
            .payload
            .clone();
        relay.on_product_forward(&mut echoed, origin(3));
        assert!(relay.take_response_when(3, KEY).is_none());
        let events = relay.take_events();
        assert!(has_event(
            &events,
            "cloud-request-refused id=77 status=500 reason=malformed"
        ));
        assert!(events.contains(&"cloud-request id=34 op=response-only".into()));
        assert_eq!(relay.take_undecodable(), 0);
    }

    #[test]
    fn an_undecodable_request_is_dropped_named_and_counted() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        let mut payload = vec![0x0a, 0x05, 0x01];
        relay.on_product_forward(&mut payload, origin(3));
        assert!(payload.is_empty(), "zeroized");
        assert!(relay.take_response_when(3, KEY).is_none());
        assert_eq!(
            relay.take_events(),
            ["cloud-request-dropped reason=undecodable"]
        );
        assert_eq!(relay.take_undecodable(), 1);
        assert_eq!(relay.take_undecodable(), 0);
    }

    #[test]
    fn switching_off_delivers_a_fetched_answer_first_and_refuses_the_rest() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        pending(&mut relay, 210, 3, 5_000);
        pending(&mut relay, 211, 3, 5_000);
        let fetched_cancel = fetched(&mut relay, 210, 200);
        let running_cancel = Arc::clone(&relay.pending[1].cancel);
        relay.set_enabled(false);
        assert!(
            !fetched_cancel.load(Ordering::Acquire),
            "fetched answer kept"
        );
        assert!(
            running_cancel.load(Ordering::Acquire),
            "running exchange retired"
        );
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()),
            (210, Some(1210), 200)
        );
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()),
            (211, Some(1211), BROKER_ANSWER_STATUS)
        );
        assert!(relay.take_response_when(3, KEY).is_none());
        let events = relay.take_events();
        assert!(has_event(
            &events,
            "cloud-request-refused id=211 status=500 reason=relay-off"
        ));
        assert!(has_event(
            &events,
            "cloud-request-answered id=210 status=200"
        ));
        assert!(!has_event(&events, "cloud-request-refused id=210"));
    }

    #[test]
    fn a_teardown_on_a_live_link_also_keeps_fetched_answers() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        pending(&mut relay, 212, 3, 5_000);
        fetched(&mut relay, 212, 200);
        pending(&mut relay, 213, 3, 5_000);
        relay.refuse_outstanding("broker-stopping");
        assert_eq!(answer(&relay.take_response_when(3, KEY).unwrap()).2, 200);
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()).2,
            BROKER_ANSWER_STATUS
        );
    }

    #[test]
    fn an_epoch_change_answers_the_exchange_it_retires() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        pending(&mut relay, 214, 3, 5_000);
        let cancel = Arc::clone(&relay.pending[0].cancel);
        assert_eq!(
            answer(&relay.take_response_when(4, KEY).unwrap()),
            (214, Some(1214), BROKER_ANSWER_STATUS)
        );
        assert!(cancel.load(Ordering::Acquire));
        assert!(has_event(
            &relay.take_events(),
            "cloud-request-refused id=214 status=500 reason=epoch-changed"
        ));
    }

    #[test]
    fn the_watchdog_answers_a_hung_exchange_only_after_its_grace() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        // Past the answer deadline but inside the grace: a real answer may
        // still arrive, so nothing is made up yet. The unit's clock started
        // 28.25 s ago: answer deadline 0.25 s past, watchdog 0.25 s ahead.
        let Some(received_at) = Instant::now().checked_sub(Duration::from_millis(28_250)) else {
            return; // A host up for under 28.25 s cannot express that instant.
        };
        let answer_by = crate::answer_deadline(30_000, received_at).unwrap();
        let watchdog_at = crate::watchdog_deadline(30_000, received_at).unwrap();
        assert!(answer_by < Instant::now() && Instant::now() < watchdog_at);
        relay.pending.push(Pending {
            cloud_request_id: 215,
            request_id: Some(1215),
            epoch: 3,
            key_generation: 0,
            deadline: received_at + Duration::from_millis(30_000),
            watchdog_at,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert!(relay.take_response_when(3, KEY).is_none());
        assert_eq!(relay.pending.len(), 1);
        pending(&mut relay, 216, 3, -1);
        let cancel = Arc::clone(&relay.pending[1].cancel);
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()),
            (216, Some(1216), BROKER_ANSWER_STATUS)
        );
        assert!(cancel.load(Ordering::Acquire), "the exchange is retired");
        assert!(has_event(
            &relay.take_events(),
            "cloud-request-refused id=216 status=500 reason=answer-deadline remaining_ms="
        ));
        // A real answer at its watchdog time wins over the watchdog.
        pending(&mut relay, 217, 3, -1);
        fetched(&mut relay, 217, 200);
        assert_eq!(answer(&relay.take_response_when(3, KEY).unwrap()).2, 200);
    }

    #[test]
    fn a_silent_server_is_answered_before_the_units_deadline() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut block = [0; 4096];
            let _ = socket.read(&mut block);
            let _ = socket.read(&mut block);
        });
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        // Arrived 27.5 s ago: answer deadline 0.5 s away, the unit's 2.5 s.
        let Some(received_at) = Instant::now().checked_sub(Duration::from_millis(27_500)) else {
            return; // A host up for under 27.5 s cannot express that instant.
        };
        let device_deadline = received_at + Duration::from_millis(30_000);
        relay.on_product_forward(
            &mut account_request(218, &format!("http://{address}/"), 30_000),
            RequestOrigin {
                epoch: 3,
                key_generation: 0,
                received_at,
            },
        );
        let response = loop {
            if let Some(response) = relay.take_response_when(3, KEY) {
                break response;
            }
            assert!(Instant::now() < device_deadline, "no answer in time");
            thread::sleep(Duration::from_millis(2));
        };
        assert!(Instant::now() < device_deadline);
        assert_eq!(answer(&response), (218, Some(8), BROKER_ANSWER_STATUS));
        assert_eq!(response.deadline, Some(device_deadline));
        drop(relay);
        server.join().unwrap();
    }

    #[test]
    fn an_exchange_that_ends_without_an_answer_frame_is_refused() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        pending(&mut relay, 219, 3, 5_000);
        let cancel = Arc::clone(&relay.pending[0].cancel);
        relay
            .sender
            .send(SecretResponse {
                cloud_request_id: 219,
                deadline: Instant::now() + Duration::from_secs(5),
                key_generation: 0,
                answer: None,
                cancel,
                messages: Vec::new(),
            })
            .ok()
            .unwrap();
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()),
            (219, Some(1219), BROKER_ANSWER_STATUS)
        );
        assert!(relay.pending.is_empty());
    }

    #[test]
    fn without_a_usable_key_answers_wait_and_a_lapsed_one_is_named() {
        let mut relay = CloudForwardRelay::default();
        relay.on_product_forward(
            &mut account_request(220, "http://127.0.0.1:1/", 30_000),
            origin(3),
        );
        relay.refusals.push_back(Refusal {
            cloud_request_id: 221,
            request_id: Some(9),
            deadline: Instant::now() - Duration::from_millis(1),
            key_generation: 0,
        });
        assert!(relay
            .take_response_when(3, CloudAnswerKey::Missing)
            .is_none());
        relay.refusals.push_back(Refusal {
            cloud_request_id: 226,
            request_id: Some(10),
            deadline: Instant::now() - Duration::from_millis(1),
            key_generation: 0,
        });
        assert!(relay
            .take_response_when(3, CloudAnswerKey::NotReady)
            .is_none());
        let events = relay.take_events();
        assert!(events.contains(&"cloud-request-unanswered id=221 reason=no-session-key".into()));
        // A key still being built is a different wait from a missing one.
        assert!(events.contains(&"cloud-request-unanswered id=226 reason=not-ready".into()));
        assert_eq!(
            answer(&relay.take_response_when(3, KEY).unwrap()).0,
            220,
            "written once the key is usable"
        );
    }

    #[test]
    fn an_answer_prepared_under_an_older_key_is_named_not_written() {
        let mut relay = CloudForwardRelay::default();
        relay.on_product_forward(
            &mut account_request(222, "http://127.0.0.1:1/", 30_000),
            RequestOrigin {
                epoch: 3,
                key_generation: 1,
                received_at: Instant::now(),
            },
        );
        assert!(relay
            .take_response_when(3, CloudAnswerKey::Ready(2))
            .is_none());
        assert!(relay
            .take_events()
            .contains(&"cloud-request-unanswered id=222 reason=key-changed".into()));
        assert!(relay
            .take_response_when(3, CloudAnswerKey::Ready(1))
            .is_none());
    }

    #[test]
    fn abandoning_a_session_names_every_request_still_owed_once() {
        let mut relay = CloudForwardRelay::default();
        relay.on_product_forward(
            &mut account_request(223, "http://127.0.0.1:1/", 30_000),
            origin(3),
        );
        relay.set_enabled(true);
        pending(&mut relay, 224, 3, 5_000);
        fetched(&mut relay, 224, 200);
        relay.take_events();
        relay.abandon_all("device-session-closed");
        assert!(relay.take_response_when(3, KEY).is_none());
        assert_eq!(
            relay.take_events(),
            [
                "cloud-request-unanswered id=224 reason=device-session-closed",
                "cloud-request-unanswered id=223 reason=device-session-closed",
            ]
        );
    }

    #[test]
    fn a_retired_epoch_request_is_still_answered() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        relay.refuse_product_forward(
            &mut account_request(225, "http://127.0.0.1:1/", 30_000),
            "stale-epoch",
            origin(3),
        );
        assert_eq!(
            answer(&relay.take_response_when(9, KEY).unwrap()),
            (225, Some(8), BROKER_ANSWER_STATUS)
        );
        let events = relay.take_events();
        assert!(has_event(
            &events,
            "cloud-request-refused id=225 status=500 reason=stale-epoch"
        ));
        assert_redacted(&events);
    }

    #[test]
    fn cloud_login_actions_are_recorded_without_their_credentials() {
        let mut relay = CloudForwardRelay::default();
        // The action, then username (field 3) "user", which is never decoded.
        for (action, name) in [(0_u8, "CREATE"), (1, "UPDATE"), (2, "DELETE")] {
            let mut payload = vec![0x08, action, 0x1a, 0x04, b'u', b's', b'e', b'r'];
            relay.on_cloud_login(&mut payload);
            assert!(payload.is_empty(), "zeroized");
            assert_eq!(relay.take_events(), [format!("cloud-login action={name}")]);
        }
        assert_eq!(relay.take_undecodable(), 0);
        relay.on_cloud_login(&mut vec![0x08]);
        assert_eq!(relay.take_events(), ["cloud-login action=undecodable"]);
        assert_eq!(relay.take_undecodable(), 1);
    }

    #[test]
    fn recorded_events_are_bounded_and_the_overflow_is_counted() {
        let mut relay = CloudForwardRelay::default();
        for _ in 0..(MAX_EVENTS + 5) {
            relay.on_cloud_login(&mut vec![0x08, 0x01]);
        }
        let events = relay.take_events();
        assert_eq!(events.len(), MAX_EVENTS + 1);
        assert_eq!(events[MAX_EVENTS], "cloud-events-dropped n=5");
        assert!(relay.take_events().is_empty());
    }

    #[test]
    fn a_failed_exchange_is_named_by_its_reason() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        for (id, answer) in [
            (227, ProductAnswer::NetworkFailed),
            (228, ProductAnswer::TimedOut),
        ] {
            relay
                .sender
                .send(SecretResponse {
                    cloud_request_id: id,
                    deadline: Instant::now() + Duration::from_secs(5),
                    key_generation: 0,
                    answer: Some(answer),
                    cancel: Arc::new(AtomicBool::new(false)),
                    messages: vec![product_forward_response(
                        Some(8),
                        id,
                        Vec::new(),
                        BROKER_ANSWER_STATUS,
                    )],
                })
                .ok()
                .unwrap();
        }
        assert!(relay.take_response_when(3, KEY).is_some());
        assert!(relay.take_response_when(3, KEY).is_some());
        let events = relay.take_events();
        assert!(has_event(
            &events,
            "cloud-request-answered id=227 status=500 reason=network"
        ));
        assert!(has_event(
            &events,
            "cloud-request-answered id=228 status=500 reason=timeout"
        ));
    }

    #[test]
    fn a_teardown_retirement_keeps_answers_already_handed_to_usb() {
        let mut relay = CloudForwardRelay::default();
        relay.set_enabled(true);
        pending(&mut relay, 229, 3, 5_000);
        let transferred = fetched(&mut relay, 229, 200);
        relay
            .response_cancellations
            .push(Arc::downgrade(&transferred));
        assert_eq!(relay.fetched_answers(), 1);
        let response = relay.take_response_when(3, KEY).unwrap();
        pending(&mut relay, 230, 3, 5_000);
        let running = Arc::clone(&relay.pending[0].cancel);
        relay.retire_for_teardown("broker-stopping");
        assert!(!response.cancel.load(Ordering::Acquire), "in flight, kept");
        assert!(running.load(Ordering::Acquire), "still running, cancelled");
        assert!(relay
            .take_events()
            .contains(&"cloud-request-unanswered id=230 reason=broker-stopping".to_string()));
    }
}
