//! Semantic read completion shared by native hosts. A queued timer is a wakeup,
//! not evidence that a reply is timely, correlated, or a valid projection.

use crate::device::correlation::{ResponseDecision, ResponseExpectation};
use crate::device::request::GatewayResponseProjection;
use serde::Serialize;
use serde_json::Value;

/// The result of offering one received message to a pending read.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ReadObservation {
    /// The message is not the awaited response: its type or request id does not match. The read
    /// stays pending.
    Unrelated,
    /// The read's deadline had passed when the message arrived, or passed before decoding finished.
    Expired,
    /// The message matched the read but could not be projected. The read stays pending, so a later
    /// valid reply can still complete it within the original budget.
    Rejected {
        /// Description of why the reply could not be projected.
        error: String,
    },
    /// The correlated reply arrived in time and was projected.
    Matched {
        /// The projected reply as JSON.
        value: Value,
    },
}

/// Tracks one pending gateway read: the expected response message type, its request id when the
/// projection carries one, the deadline, and how to project a matching reply.
#[derive(Debug, Clone)]
pub struct GatewayReadRuntime {
    expectation: ResponseExpectation,
    projection: GatewayResponseProjection,
}

impl GatewayReadRuntime {
    /// Starts a read at `started_at_ms` that expects a message of `response_type` and expires
    /// `timeout_ms` later.
    ///
    /// When `projection` carries a request id, only a reply with that id matches.
    pub fn new(
        response_type: u16,
        projection: GatewayResponseProjection,
        started_at_ms: u64,
        timeout_ms: u64,
    ) -> Self {
        Self {
            expectation: ResponseExpectation::new(
                response_type,
                projection.expected_request_id(),
                started_at_ms,
                timeout_ms,
            ),
            projection,
        }
    }

    /// Returns the milliseconds left in the read's budget at `now_ms`, or zero once it has expired.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.expectation.remaining_ms(now_ms)
    }

    /// Returns whether `ingress`, the first and last report times of a received message, is present
    /// and consistent with this read.
    ///
    /// That requires the first report no earlier than the read's start, not after the last report,
    /// and the last report not after `now_ms`.
    pub fn accepts_ingress(
        &self,
        ingress: Option<crate::device::transport::IngressTime>,
        now_ms: u64,
    ) -> bool {
        self.expectation.accepts_ingress(ingress, now_ms)
    }

    /// Adds what the session itself has observed to a projected reply the device could not
    /// state in that reply. Today that is the CorOS version in an identity reply: the unit
    /// states it in its update request, not in the version reply. Any other read, a reply
    /// that is not a match, or a session that has seen no version passes through unchanged.
    pub fn with_session_facts(
        &self,
        observation: ReadObservation,
        cor_os_version: Option<&str>,
    ) -> ReadObservation {
        match (observation, &self.projection, cor_os_version) {
            (
                ReadObservation::Matched { mut value },
                GatewayResponseProjection::DeviceIdentity,
                Some(version),
            ) => {
                if let Some(identity) = value.as_object_mut() {
                    identity.insert("corOsVersion".into(), Value::String(version.to_owned()));
                }
                ReadObservation::Matched { value }
            }
            (observation, _, _) => observation,
        }
    }

    /// Decoding may be expensive (for example a screen image). Hosts call this
    /// on an operation/decoder lane, never as part of the native report reader.
    /// Rejected input does not complete the read; a later valid reply may do so
    /// within the original budget. The caller supplies the current clock sample,
    /// so queuing a decoder job cannot extend that budget either.
    pub fn observe(&self, message_type: u16, payload: &[u8], now_ms: u64) -> ReadObservation {
        self.observe_with_clock(message_type, payload, || now_ms)
    }

    /// Like [`GatewayReadRuntime::observe`], but samples the clock through `now_ms` both before
    /// correlating and after decoding.
    ///
    /// A reply whose decoding finishes after the deadline is reported as
    /// [`ReadObservation::Expired`].
    pub fn observe_with_clock(
        &self,
        message_type: u16,
        payload: &[u8],
        mut now_ms: impl FnMut() -> u64,
    ) -> ReadObservation {
        match self.expectation.observe(message_type, payload, now_ms()) {
            ResponseDecision::Unrelated => ReadObservation::Unrelated,
            ResponseDecision::Expired => ReadObservation::Expired,
            ResponseDecision::Matched => {
                let decoded = self.projection.decode(payload);
                if self.expectation.expired(now_ms()) {
                    return ReadObservation::Expired;
                }
                match decoded {
                    Ok(value) => ReadObservation::Matched { value },
                    Err(error) => ReadObservation::Rejected { error },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_or_malformed_reply_does_not_consume_a_read() {
        let runtime =
            GatewayReadRuntime::new(9, GatewayResponseProjection::GeneralSettings, 100, 50);
        assert_eq!(
            runtime.observe(10, &[0xff], 110),
            ReadObservation::Unrelated
        );
        assert!(matches!(
            runtime.observe(9, &[0xff], 110),
            ReadObservation::Rejected { .. }
        ));
        assert!(matches!(
            runtime.observe(9, &[8, 1], 149),
            ReadObservation::Matched { .. }
        ));
        assert_eq!(runtime.observe(9, &[8, 1], 150), ReadObservation::Expired);
    }

    fn identity_reply() -> Vec<u8> {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message as _;
        pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            device_serial_number: Some(
                pa::version_message::DeviceSerialNumber::DeviceSerialNumber("SYN-0001".into()),
            ),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn an_identity_read_carries_the_cor_os_version_the_session_observed() {
        let runtime = GatewayReadRuntime::new(10, GatewayResponseProjection::DeviceIdentity, 0, 50);
        let matched = runtime.observe(10, &identity_reply(), 1);
        let ReadObservation::Matched { value } = runtime.with_session_facts(matched, Some("4.1.0"))
        else {
            panic!("the identity reply no longer matches");
        };
        assert_eq!(value["corOsVersion"], "4.1.0");
        assert_eq!(value["serial"], "SYN-0001");

        // A session that has seen no update request leaves the field out, so a
        // host shows its documented reference value instead of a guess.
        let matched = runtime.observe(10, &identity_reply(), 1);
        let ReadObservation::Matched { value } = runtime.with_session_facts(matched, None) else {
            panic!("the identity reply no longer matches");
        };
        assert!(value.get("corOsVersion").is_none());

        // Only an identity read takes it; and a reply that is not a match is untouched.
        let settings =
            GatewayReadRuntime::new(9, GatewayResponseProjection::GeneralSettings, 0, 50);
        let matched = settings.observe(9, &[8, 1], 1);
        let ReadObservation::Matched { value } =
            settings.with_session_facts(matched, Some("4.1.0"))
        else {
            panic!("the settings reply no longer matches");
        };
        assert!(value.get("corOsVersion").is_none());
        assert_eq!(
            runtime.with_session_facts(ReadObservation::Unrelated, Some("4.1.0")),
            ReadObservation::Unrelated
        );
    }

    #[test]
    fn a_bare_current_preset_read_is_matched_by_type_not_by_request_id() {
        let runtime = GatewayReadRuntime::new(15, GatewayResponseProjection::CurrentPreset, 0, 50);
        assert_eq!(runtime.observe(9, &[8, 1], 1), ReadObservation::Unrelated);
        assert!(matches!(
            runtime.observe(15, &[8, 1, 16, 42, 0xff], 1),
            ReadObservation::Rejected { .. }
        ));
        assert_eq!(runtime.observe(15, &[8, 1], 50), ReadObservation::Expired);
    }

    #[test]
    fn backlogged_decoder_cannot_validate_an_expired_response() {
        let runtime =
            GatewayReadRuntime::new(9, GatewayResponseProjection::GeneralSettings, 100, 50);
        assert_eq!(runtime.remaining_ms(140), 10);
        assert_eq!(runtime.observe(9, &[8, 1], 151), ReadObservation::Expired);
    }

    #[test]
    fn decoding_does_not_extend_the_completion_budget() {
        let runtime =
            GatewayReadRuntime::new(9, GatewayResponseProjection::GeneralSettings, 100, 50);
        let mut samples = [149, 150].into_iter();
        assert_eq!(
            runtime.observe_with_clock(9, &[8, 1], || samples.next().unwrap()),
            ReadObservation::Expired
        );
    }
}
