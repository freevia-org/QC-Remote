//! Shared matching and timeout policy for replies awaited by native hosts.
//!
//! Channels, futures, and wakeups remain platform concerns. The device
//! protocol has one definition of a matching response: the expected message
//! type must match, and a requested correlation id must be present and equal.

use crate::wire;

/// A monotonic request budget, sampled when the caller creates the operation.
/// Queueing and response handling consume this same budget; host timer delivery
/// is only a wakeup and cannot change whether the deadline has expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestDeadline {
    expires_at_ms: u64,
}

impl RequestDeadline {
    /// Creates a budget of `timeout_ms` milliseconds starting at
    /// `started_at_ms` on the host's monotonic clock.
    pub fn new(started_at_ms: u64, timeout_ms: u64) -> Self {
        Self {
            expires_at_ms: started_at_ms.saturating_add(timeout_ms),
        }
    }

    /// Whether `now_ms` is at or past the expiry time.
    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }

    /// Milliseconds left at `now_ms`, or zero once expired.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.expires_at_ms.saturating_sub(now_ms)
    }

    /// The monotonic time, in milliseconds, at which the budget runs out.
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    /// Nested phases may shorten a caller's budget, never renew it.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            expires_at_ms: self.expires_at_ms.min(other.expires_at_ms),
        }
    }
}

/// How an inbound message relates to a [`ResponseExpectation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseDecision {
    /// Not the awaited reply; keep waiting.
    Unrelated,
    /// The awaited reply, received before the deadline.
    Matched,
    /// The deadline had already passed when the message was observed.
    Expired,
}

/// The reply a host is waiting for: a message type, an optional correlation id
/// to match, and a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseExpectation {
    message_type: u16,
    request_id: Option<u64>,
    deadline: RequestDeadline,
    started_at_ms: u64,
}

impl ResponseExpectation {
    /// Expects a reply of `message_type` within `timeout_ms` of `started_at_ms`
    /// (monotonic milliseconds). With `request_id` set, the reply must carry
    /// that id; with `None`, any message of that type matches.
    pub fn new(
        message_type: u16,
        request_id: Option<u64>,
        started_at_ms: u64,
        timeout_ms: u64,
    ) -> Self {
        Self {
            message_type,
            request_id,
            deadline: RequestDeadline::new(started_at_ms, timeout_ms),
            started_at_ms,
        }
    }

    /// The expected message type.
    pub fn message_type(&self) -> u16 {
        self.message_type
    }

    /// The correlation id the reply must carry, if one was requested.
    pub fn request_id(&self) -> Option<u64> {
        self.request_id
    }

    /// Whether a message of `actual_type` with `payload` is the awaited reply,
    /// ignoring the deadline. The type must equal the expected one and, if an
    /// id was requested, the payload must carry exactly that request id.
    pub fn matches(&self, actual_type: u16, payload: &[u8]) -> bool {
        actual_type == self.message_type
            && self
                .request_id
                .is_none_or(|expected| wire::request_id(actual_type, payload) == Some(expected))
    }

    /// Whether the deadline has passed at `now_ms`.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.deadline.expired(now_ms)
    }

    /// Milliseconds left before the deadline, or zero once it has passed.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.deadline.remaining_ms(now_ms)
    }

    /// Evaluate deadline and correlation atomically with respect to the event's
    /// clock sample. A late matching reply is a timeout, even when the host's
    /// maintenance callback has not run yet.
    pub fn observe(&self, actual_type: u16, payload: &[u8], now_ms: u64) -> ResponseDecision {
        if self.expired(now_ms) {
            ResponseDecision::Expired
        } else if self.matches(actual_type, payload) {
            ResponseDecision::Matched
        } else {
            ResponseDecision::Unrelated
        }
    }

    /// Whether the frame's read timestamps prove it arrived after this
    /// expectation was created. Returns `false` when `ingress` is `None`, or
    /// when its first report predates the expectation or the times are out of
    /// order or in the future relative to `now_ms`.
    pub fn accepts_ingress(
        &self,
        ingress: Option<crate::device::transport::IngressTime>,
        now_ms: u64,
    ) -> bool {
        ingress.is_some_and(|ingress| {
            ingress.first_report_at_ms >= self.started_at_ms
                && ingress.first_report_at_ms <= ingress.last_report_at_ms
                && ingress.last_report_at_ms <= now_ms
        })
    }

    /// Like [`ResponseExpectation::observe`], but a frame whose `ingress`
    /// timestamps are missing or predate the expectation is `Unrelated`, so a
    /// buffered or partially old frame cannot confirm a new request. An expired
    /// deadline still wins.
    pub fn observe_received(
        &self,
        actual_type: u16,
        payload: &[u8],
        ingress: Option<crate::device::transport::IngressTime>,
        now_ms: u64,
    ) -> ResponseDecision {
        if self.expired(now_ms) {
            ResponseDecision::Expired
        } else if !self.accepts_ingress(ingress, now_ms) {
            ResponseDecision::Unrelated
        } else {
            self.observe(actual_type, payload, now_ms)
        }
    }

    /// Human-readable text for a timeout of this expectation, naming the message
    /// type and, if set, the request id.
    pub fn timeout_message(&self) -> String {
        match self.request_id {
            Some(request_id) => format!(
                "No correlated QC message type {} response for request {} before timeout",
                self.message_type, request_id
            ),
            None => format!(
                "No QC message type {} response before timeout",
                self.message_type
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncorrelated_expectation_matches_only_its_message_type() {
        let expected = ResponseExpectation::new(10, None, 100, 50);
        assert!(expected.matches(10, &[]));
        assert!(!expected.matches(11, &[]));
        assert!(!expected.expired(149));
        assert!(expected.expired(150));
    }

    #[test]
    fn buffered_or_partially_old_frames_cannot_confirm_a_new_read() {
        use crate::device::transport::IngressTime;
        let expected = ResponseExpectation::new(13, Some(42), 100, 50);
        let payload = [8, 1, 16, 42];
        let old = Some(IngressTime {
            first_report_at_ms: 99,
            last_report_at_ms: 120,
        });
        assert_eq!(
            expected.observe_received(13, &payload, old, 130),
            ResponseDecision::Unrelated
        );
        assert_eq!(
            expected.observe_received(13, &payload, None, 130),
            ResponseDecision::Unrelated
        );
        let current = Some(IngressTime {
            first_report_at_ms: 100,
            last_report_at_ms: 120,
        });
        assert_eq!(
            expected.observe_received(13, &payload, current, 130),
            ResponseDecision::Matched
        );
        assert_eq!(
            expected.observe_received(13, &payload, current, 119),
            ResponseDecision::Unrelated
        );
        assert_eq!(
            expected.observe_received(13, &payload, current, 150),
            ResponseDecision::Expired
        );
        let inverted = Some(IngressTime {
            first_report_at_ms: 120,
            last_report_at_ms: 110,
        });
        assert_eq!(
            expected.observe_received(13, &payload, inverted, 130),
            ResponseDecision::Unrelated
        );
    }

    #[test]
    fn correlated_expectation_requires_the_id_to_be_present_and_equal() {
        let expected = ResponseExpectation::new(13, Some(42), 0, 1_000);
        assert!(expected.matches(13, &[0x08, 0x01, 0x10, 0x2a]));
        assert!(!expected.matches(13, &[0x08, 0x01]));
        assert!(!expected.matches(13, &[0x08, 0x01, 0x10, 0x29]));
    }

    #[test]
    fn correlated_expectation_honors_field_one_schema_exceptions() {
        let expected = ResponseExpectation::new(49, Some(42), 0, 1_000);
        assert!(expected.matches(49, &[0x08, 0x2a, 0x10, 0x01]));
    }

    #[test]
    fn timeout_arithmetic_saturates() {
        let expected = ResponseExpectation::new(1, None, u64::MAX - 2, 10);
        assert_eq!(expected.remaining_ms(0), u64::MAX);
        assert!(expected.expired(u64::MAX));
    }

    #[test]
    fn matching_reply_cannot_beat_a_delayed_timeout_callback() {
        let expected = ResponseExpectation::new(13, Some(42), 100, 50);
        let reply = [0x08, 0x01, 0x10, 0x2a];
        assert_eq!(expected.observe(13, &reply, 149), ResponseDecision::Matched);
        assert_eq!(expected.observe(13, &reply, 150), ResponseDecision::Expired);
        assert_eq!(expected.observe(13, &reply, 151), ResponseDecision::Expired);
        assert_eq!(
            expected.observe(10, &reply, 149),
            ResponseDecision::Unrelated
        );
        assert_eq!(expected.observe(10, &reply, 150), ResponseDecision::Expired);
    }

    #[test]
    fn queue_delay_consumes_the_original_budget() {
        let deadline = RequestDeadline::new(100, 50);
        assert_eq!(deadline.remaining_ms(140), 10);
        assert!(!deadline.expired(149));
        assert!(deadline.expired(150));
        assert_eq!(deadline.remaining_ms(151), 0);
        assert!(RequestDeadline::new(100, 0).expired(100));
    }
}
