//! What arrived on the inbound link and was not acted on.
//!
//! An inbound frame can stop in six distinguishable ways, and until this
//! module existed none of them left a counter, a log line or a metric behind.
//! The envelope's encrypted flag drops the frame before its body is ever
//! read; a set compressed flag can fail to inflate; the trailer's message
//! type can be one the protocol does not declare; a declared type's protobuf
//! body can fail to decode; and a declared type that decodes cleanly can
//! reach a dispatch table with no handler for it, where it turns into an
//! empty update list indistinguishable from a handled message that had
//! nothing to report.
//!
//! Each of those is counted here, separately and keyed by message type, so
//! that "this host has never seen X" becomes a statement something can
//! contradict.
//!
//! The dispatch-table case is reported apart from the other four rather than
//! summed with them, because it is the only one that does not prove the frame
//! went unused: the settings, session, catalog and backup lanes all read
//! types the state decoder ignores. A type counted there that no other lane
//! claims either is a type the host receives and entirely ignores, and that
//! is now a question the counters can answer instead of an assumption.
//!
//! The eighth and ninth trailer bytes get the same treatment for a different
//! reason. The host writes zero in both and the framing layer parses them, so
//! they are the one field on every inbound message with no interpretation at
//! all. A per-`(message type, trailer bytes)` tally is kept so their meaning
//! can be read off real traffic rather than guessed at. Nothing in this
//! module or below it acts on their value.
//!
//! Three more counters cover what is lost before a frame is assembled at all,
//! and so is never one of `frames`: a partial message abandoned when a new
//! FIRST report or a native ingress epoch change cut it off, a continuation
//! report that arrived with no message open, and a report the framing layer
//! rejected. A frame cut in half by a host-side read problem looks like "the
//! device never answered", and these are its only evidence.

use crate::message_registry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Distinct `(message type, trailer bytes)` pairs the tally admits. The key
/// space is 2^16 per type and the tally exists to expose a small set of
/// recurring values, not to record a stream, so it stops admitting new keys
/// here and counts the rest in `deviceTrailerKeysDropped`.
pub const MAX_TRAILER_KEYS: usize = 256;

/// Distinct undeclared type numbers the tally admits. A corrupted trailer can
/// produce an unbounded set of these, so it is bounded the same way.
pub const MAX_UNDECLARED_TYPE_KEYS: usize = 64;

/// One observed pair of device trailer bytes for one message type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTrailerTally {
    /// Message type the trailer bytes were seen on.
    pub message_type: u16,
    /// The two device trailer bytes, in wire order.
    pub device_bytes: [u8; 2],
    /// Number of frames observed with this message type and trailer-byte pair.
    pub count: u64,
}

/// The readable form of [`InboundTelemetry`]. Every map is keyed by message
/// type; JSON renders those keys as decimal strings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundTelemetrySnapshot {
    /// Complete envelopes the framing layer assembled, whatever became of
    /// them afterwards.
    pub frames: u64,
    /// Frames no consumer anywhere in the host could have used: an
    /// undeclared type, a body that did not decode, a set encrypted flag, or
    /// a body that did not inflate. Zero here means every assembled frame
    /// was at least usable.
    ///
    /// `no_state_handler_frames` is deliberately *not* summed in. A type the
    /// state dispatch table has no handler for may still be consumed
    /// elsewhere in the host -- the settings publications and the session and
    /// backup lanes all read types this decoder ignores -- so counting those
    /// here would inflate this number with frames that were acted on.
    pub unacted_frames: u64,
    /// Frames of a declared type that decoded and reached the state dispatch
    /// table's default. See `no_state_handler_types`.
    pub no_state_handler_frames: u64,
    /// Trailer message types outside the declared set, keyed by the raw
    /// 32-bit trailer value. A firmware release that adds a type lands here.
    pub undeclared_types: BTreeMap<u32, u64>,
    /// Distinct undeclared type numbers past [`MAX_UNDECLARED_TYPE_KEYS`].
    pub undeclared_type_keys_dropped: u64,
    /// Declared types whose protobuf body did not decode.
    pub decode_failures: BTreeMap<u16, u64>,
    /// Declared types that decoded and reached the state dispatch table's
    /// default, keyed by type. Counted at that default itself, so the tally
    /// cannot drift from the table above it.
    ///
    /// This says the *state decoder* did nothing with the type, which is
    /// narrower than "nothing in the host did": several types are consumed by
    /// the settings, session, catalog and backup lanes instead and appear
    /// here all the same. A type here that no other lane claims either is a
    /// type the host receives and entirely ignores.
    pub no_state_handler_types: BTreeMap<u16, u64>,
    /// Frames dropped for a set encrypted flag. This build implements no
    /// inbound decryption, so this happens whatever the message type is.
    pub encrypted_rejections: BTreeMap<u16, u64>,
    /// Frames whose compressed flag was set, inflated or not.
    pub compressed: BTreeMap<u16, u64>,
    /// Frames whose compressed body failed to inflate.
    pub inflate_failures: BTreeMap<u16, u64>,
    /// Every distinct pair of device trailer bytes seen per message type.
    /// Ordered by message type, then by the bytes themselves.
    pub device_trailer_bytes: Vec<DeviceTrailerTally>,
    /// Distinct trailer keys past [`MAX_TRAILER_KEYS`].
    pub device_trailer_keys_dropped: u64,
    /// Messages lost part-way through assembly: a FIRST report arrived while
    /// another message was still open, or a native ingress epoch change cut
    /// one off. Never assembled, so neither in `frames` nor keyed by type.
    #[serde(default)]
    pub abandoned_partial_frames: u64,
    /// Continuation reports that arrived with no message open (after an
    /// attach, a reset, an epoch change or a framing error) and were ignored.
    #[serde(default)]
    pub orphaned_continuation_reports: u64,
    /// Reports the framing layer rejected (a malformed report, a message past
    /// the frame-size limit, a body shorter than its trailer). Each also
    /// discards whatever message was open.
    #[serde(default)]
    pub framing_errors: u64,
    /// The most recent frame that stopped, described in one line, so a
    /// reader gets the reason without correlating counters.
    pub last_unacted: Option<String>,
}

/// Per-connection counts of inbound frames the host could not act on.
///
/// Two of these exist per session: the transport layer owns the envelope
/// outcomes and the trailer tally, the state decoder owns the two outcomes
/// only a dispatch attempt can establish, and [`InboundTelemetry::merge`]
/// combines them for a reader.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboundTelemetry {
    frames: u64,
    // One unknown envelope can also be encrypted or fail inflation. Retain
    // both diagnostic categories without counting that same frame twice.
    undeclared_rejection_overlaps: u64,
    undeclared_types: BTreeMap<u32, u64>,
    undeclared_type_keys_dropped: u64,
    decode_failures: BTreeMap<u16, u64>,
    no_state_handler_types: BTreeMap<u16, u64>,
    encrypted_rejections: BTreeMap<u16, u64>,
    compressed: BTreeMap<u16, u64>,
    inflate_failures: BTreeMap<u16, u64>,
    device_trailer_bytes: BTreeMap<(u16, [u8; 2]), u64>,
    device_trailer_keys_dropped: u64,
    abandoned_partial_frames: u64,
    orphaned_continuation_reports: u64,
    framing_errors: u64,
    last_unacted: Option<String>,
    revision: u64,
}

impl InboundTelemetry {
    /// Bumped by every recording call. A host can compare this against the
    /// value it last published and skip copying the maps when nothing moved,
    /// which keeps the ordinary case off the allocation path.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Running count of inbound losses that could have cost a message of
    /// `message_type`: every report or frame dropped before its type was
    /// known (abandoned partial messages, orphaned continuations and framing
    /// errors), plus that type's own inflate failures and encrypted
    /// rejections. It only grows within a connection, so a reader can detect
    /// loss by comparing two readings.
    pub fn loss_for_type(&self, message_type: u16) -> u64 {
        self.abandoned_partial_frames
            .saturating_add(self.orphaned_continuation_reports)
            .saturating_add(self.framing_errors)
            .saturating_add(
                self.inflate_failures
                    .get(&message_type)
                    .copied()
                    .unwrap_or(0),
            )
            .saturating_add(
                self.encrypted_rejections
                    .get(&message_type)
                    .copied()
                    .unwrap_or(0),
            )
    }

    /// Count a complete envelope whose raw type cannot be keyed by the
    /// host's 16-bit trailer/outcome tables. No truncated type is invented.
    pub(crate) fn record_unkeyed_envelope(&mut self) {
        self.frames = self.frames.saturating_add(1);
        self.revision = self.revision.saturating_add(1);
    }

    /// A complete envelope, before anything decides what to do with it. The
    /// trailer bytes are tallied here so that a frame rejected further down
    /// still contributes the one datum nothing interprets.
    pub fn record_envelope(&mut self, message_type: u16, device_bytes: [u8; 2]) {
        self.record_unkeyed_envelope();
        bump_bounded(
            &mut self.device_trailer_bytes,
            (message_type, device_bytes),
            MAX_TRAILER_KEYS,
            &mut self.device_trailer_keys_dropped,
        );
    }

    /// A trailer message type the protocol does not declare. `message_type`
    /// is the raw 32-bit trailer value, which can exceed the host's 16-bit
    /// dispatch range.
    pub fn record_undeclared_type(&mut self, message_type: u32) {
        bump_bounded(
            &mut self.undeclared_types,
            message_type,
            MAX_UNDECLARED_TYPE_KEYS,
            &mut self.undeclared_type_keys_dropped,
        );
        self.note(format!("undeclared message type {message_type}"));
    }

    /// A declared type whose protobuf body did not decode.
    pub fn record_decode_failure(&mut self, message_type: u16) {
        bump(&mut self.decode_failures, message_type);
        self.note(format!("{} did not decode", describe_type(message_type)));
    }

    /// A declared type that decoded and that no state handler claims.
    pub fn record_no_state_handler(&mut self, message_type: u16) {
        bump(&mut self.no_state_handler_types, message_type);
        self.note(format!(
            "{} decoded with no state handler",
            describe_type(message_type)
        ));
    }

    /// A frame dropped because its envelope declared an encrypted body.
    pub fn record_encrypted_rejection(&mut self, message_type: u16) {
        bump(&mut self.encrypted_rejections, message_type);
        self.note(format!(
            "{} arrived encrypted and was dropped",
            describe_type(message_type)
        ));
    }

    pub(crate) fn record_undeclared_rejection_overlap(&mut self) {
        self.undeclared_rejection_overlaps = self.undeclared_rejection_overlaps.saturating_add(1);
        self.revision = self.revision.saturating_add(1);
    }

    /// A frame whose envelope declared a compressed body. Recorded whether
    /// or not the body then inflated.
    pub fn record_compressed(&mut self, message_type: u16) {
        bump(&mut self.compressed, message_type);
        self.revision = self.revision.saturating_add(1);
    }

    /// A compressed frame whose body did not inflate.
    pub fn record_inflate_failure(&mut self, message_type: u16) {
        bump(&mut self.inflate_failures, message_type);
        self.note(format!("{} did not inflate", describe_type(message_type)));
    }

    /// A message dropped part-way through assembly, after `reports` of its
    /// reports had arrived. `cause` says what cut it off.
    pub fn record_abandoned_partial(&mut self, reports: usize, cause: &str) {
        self.abandoned_partial_frames = self.abandoned_partial_frames.saturating_add(1);
        self.note(format!(
            "a partial message of {reports} report(s) was abandoned: {cause}"
        ));
    }

    /// A continuation report that arrived with no message open.
    pub fn record_orphaned_continuation(&mut self) {
        self.orphaned_continuation_reports = self.orphaned_continuation_reports.saturating_add(1);
        self.note("a continuation report arrived with no message open".into());
    }

    /// A report the framing layer rejected; `error` describes why.
    pub fn record_framing_error(&mut self, error: &str) {
        self.framing_errors = self.framing_errors.saturating_add(1);
        self.note(format!("framing error: {error}"));
    }

    /// Classify a declared-or-not type against the protocol's own decoders,
    /// without a handler having to exist for it. Used for the types a
    /// dispatch table falls through on, which are never decoded otherwise:
    /// an undeclared number is already counted by the transport, so only the
    /// two outcomes a decode attempt can establish are recorded here.
    pub fn record_dispatch_fallthrough(&mut self, message_type: u16, payload: &[u8]) {
        match message_registry::decode_any(message_type, payload) {
            Some(Ok(_)) => self.record_no_state_handler(message_type),
            Some(Err(_)) => self.record_decode_failure(message_type),
            None => {}
        }
    }

    /// Fold another accumulator's counts into this one.
    pub fn merge(&mut self, other: &InboundTelemetry) {
        self.frames = self.frames.saturating_add(other.frames);
        self.undeclared_rejection_overlaps = self
            .undeclared_rejection_overlaps
            .saturating_add(other.undeclared_rejection_overlaps);
        merge_bounded_into(
            &mut self.undeclared_types,
            &other.undeclared_types,
            MAX_UNDECLARED_TYPE_KEYS,
            &mut self.undeclared_type_keys_dropped,
        );
        self.undeclared_type_keys_dropped = self
            .undeclared_type_keys_dropped
            .saturating_add(other.undeclared_type_keys_dropped);
        merge_into(&mut self.decode_failures, &other.decode_failures);
        merge_into(
            &mut self.no_state_handler_types,
            &other.no_state_handler_types,
        );
        merge_into(&mut self.encrypted_rejections, &other.encrypted_rejections);
        merge_into(&mut self.compressed, &other.compressed);
        merge_into(&mut self.inflate_failures, &other.inflate_failures);
        merge_bounded_into(
            &mut self.device_trailer_bytes,
            &other.device_trailer_bytes,
            MAX_TRAILER_KEYS,
            &mut self.device_trailer_keys_dropped,
        );
        self.device_trailer_keys_dropped = self
            .device_trailer_keys_dropped
            .saturating_add(other.device_trailer_keys_dropped);
        self.abandoned_partial_frames = self
            .abandoned_partial_frames
            .saturating_add(other.abandoned_partial_frames);
        self.orphaned_continuation_reports = self
            .orphaned_continuation_reports
            .saturating_add(other.orphaned_continuation_reports);
        self.framing_errors = self.framing_errors.saturating_add(other.framing_errors);
        if other.last_unacted.is_some() {
            self.last_unacted = other.last_unacted.clone();
        }
        self.revision = self.revision.saturating_add(other.revision);
    }

    /// The readable form.
    pub fn snapshot(&self) -> InboundTelemetrySnapshot {
        InboundTelemetrySnapshot {
            frames: self.frames,
            unacted_frames: u64::try_from(
                wide_total(&self.undeclared_types)
                    .saturating_add(u128::from(self.undeclared_type_keys_dropped))
                    .saturating_add(wide_total(&self.decode_failures))
                    .saturating_add(wide_total(&self.encrypted_rejections))
                    .saturating_add(wide_total(&self.inflate_failures))
                    .saturating_sub(u128::from(self.undeclared_rejection_overlaps)),
            )
            .unwrap_or(u64::MAX),
            no_state_handler_frames: total(&self.no_state_handler_types),
            undeclared_types: self.undeclared_types.clone(),
            undeclared_type_keys_dropped: self.undeclared_type_keys_dropped,
            decode_failures: self.decode_failures.clone(),
            no_state_handler_types: self.no_state_handler_types.clone(),
            encrypted_rejections: self.encrypted_rejections.clone(),
            compressed: self.compressed.clone(),
            inflate_failures: self.inflate_failures.clone(),
            device_trailer_bytes: self
                .device_trailer_bytes
                .iter()
                .map(|((message_type, device_bytes), count)| DeviceTrailerTally {
                    message_type: *message_type,
                    device_bytes: *device_bytes,
                    count: *count,
                })
                .collect(),
            device_trailer_keys_dropped: self.device_trailer_keys_dropped,
            abandoned_partial_frames: self.abandoned_partial_frames,
            orphaned_continuation_reports: self.orphaned_continuation_reports,
            framing_errors: self.framing_errors,
            last_unacted: self.last_unacted.clone(),
        }
    }

    fn note(&mut self, reason: String) {
        self.last_unacted = Some(reason);
        self.revision = self.revision.saturating_add(1);
    }
}

fn describe_type(message_type: u16) -> String {
    match message_registry::message_name(message_type) {
        Some(name) => format!("type {message_type} ({name})"),
        None => format!("type {message_type}"),
    }
}

fn bump<K: Ord>(counts: &mut BTreeMap<K, u64>, key: K) {
    let count = counts.entry(key).or_default();
    *count = count.saturating_add(1);
}

/// Increment `key`, admitting a new key only while the map is under `limit`.
/// An observation that cannot be keyed is counted in `dropped` rather than
/// discarded, so a saturated tally still says that it saturated.
fn bump_bounded<K: Ord>(counts: &mut BTreeMap<K, u64>, key: K, limit: usize, dropped: &mut u64) {
    if let Some(count) = counts.get_mut(&key) {
        *count = count.saturating_add(1);
        return;
    }
    if counts.len() >= limit {
        *dropped = dropped.saturating_add(1);
        return;
    }
    counts.insert(key, 1);
}

fn merge_into<K: Ord + Clone>(into: &mut BTreeMap<K, u64>, from: &BTreeMap<K, u64>) {
    for (key, count) in from {
        let existing = into.entry(key.clone()).or_default();
        *existing = existing.saturating_add(*count);
    }
}

fn merge_bounded_into<K: Ord + Clone>(
    into: &mut BTreeMap<K, u64>,
    from: &BTreeMap<K, u64>,
    limit: usize,
    dropped: &mut u64,
) {
    for (key, count) in from {
        if let Some(existing) = into.get_mut(key) {
            *existing = existing.saturating_add(*count);
        } else if into.len() < limit {
            into.insert(key.clone(), *count);
        } else {
            *dropped = dropped.saturating_add(*count);
        }
    }
}

fn wide_total<K: Ord>(counts: &BTreeMap<K, u64>) -> u128 {
    counts
        .values()
        .fold(0u128, |sum, count| sum.saturating_add(u128::from(*count)))
}

fn total<K: Ord>(counts: &BTreeMap<K, u64>) -> u64 {
    counts
        .values()
        .fold(0u64, |sum, count| sum.saturating_add(*count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile;

    #[test]
    fn overlapping_outcomes_subtract_before_final_saturation() {
        let mut telemetry = InboundTelemetry::default();
        telemetry.undeclared_types.insert(73, u64::MAX);
        telemetry.encrypted_rejections.insert(73, u64::MAX);
        telemetry.undeclared_rejection_overlaps = u64::MAX;
        assert_eq!(telemetry.snapshot().unacted_frames, u64::MAX);
        assert_eq!(telemetry.snapshot().undeclared_types[&73], u64::MAX);
        assert_eq!(telemetry.snapshot().encrypted_rejections[&73], u64::MAX);

        // Map totals must also remain wide until overlap subtraction.
        telemetry.undeclared_types.insert(74, 7);
        assert_eq!(telemetry.snapshot().unacted_frames, u64::MAX);

        let mut left = InboundTelemetry::default();
        left.undeclared_types.insert(73, u64::MAX - 1);
        left.encrypted_rejections.insert(73, u64::MAX - 1);
        left.undeclared_rejection_overlaps = u64::MAX - 1;
        let mut right = InboundTelemetry::default();
        right.undeclared_types.insert(73, 2);
        right.encrypted_rejections.insert(73, 2);
        right.undeclared_rejection_overlaps = 2;
        left.merge(&right);
        assert_eq!(left.snapshot().unacted_frames, u64::MAX);
        assert_eq!(left.snapshot().undeclared_types[&73], u64::MAX);
        assert_eq!(left.snapshot().encrypted_rejections[&73], u64::MAX);
    }

    #[test]
    fn each_outcome_is_counted_in_its_own_bucket() {
        let mut telemetry = InboundTelemetry::default();
        telemetry.record_envelope(profile::MESSAGE_TYPE_SCENE, [0, 0]);
        telemetry.record_undeclared_type(73);
        telemetry.record_decode_failure(profile::MESSAGE_TYPE_GRID);
        telemetry.record_no_state_handler(profile::MESSAGE_TYPE_MIDI_SETTINGS);
        telemetry.record_encrypted_rejection(profile::MESSAGE_TYPE_LICENSE);
        telemetry.record_compressed(profile::MESSAGE_TYPE_MODEL_REPO);
        telemetry.record_inflate_failure(profile::MESSAGE_TYPE_MODEL_REPO);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.frames, 1);
        assert_eq!(snapshot.undeclared_types[&73], 1);
        assert_eq!(snapshot.decode_failures[&profile::MESSAGE_TYPE_GRID], 1);
        assert_eq!(
            snapshot.no_state_handler_types[&profile::MESSAGE_TYPE_MIDI_SETTINGS],
            1
        );
        assert_eq!(
            snapshot.encrypted_rejections[&profile::MESSAGE_TYPE_LICENSE],
            1
        );
        assert_eq!(snapshot.compressed[&profile::MESSAGE_TYPE_MODEL_REPO], 1);
        assert_eq!(
            snapshot.inflate_failures[&profile::MESSAGE_TYPE_MODEL_REPO],
            1
        );
        // The compressed tally is not an outcome, and a type the state
        // decoder ignored may still have been used elsewhere in the host.
        assert_eq!(snapshot.unacted_frames, 4);
        assert_eq!(snapshot.no_state_handler_frames, 1);
        assert!(snapshot.last_unacted.is_some());
    }

    #[test]
    fn loss_for_a_type_counts_untyped_drops_and_only_that_types_own_failures() {
        let mut telemetry = InboundTelemetry::default();
        assert_eq!(
            telemetry.loss_for_type(profile::MESSAGE_TYPE_MODEL_PRESET),
            0
        );
        telemetry.record_inflate_failure(profile::MESSAGE_TYPE_MODEL_REPO);
        telemetry.record_encrypted_rejection(profile::MESSAGE_TYPE_LICENSE);
        assert_eq!(
            telemetry.loss_for_type(profile::MESSAGE_TYPE_MODEL_PRESET),
            0
        );
        telemetry.record_abandoned_partial(2, "a new message began");
        telemetry.record_orphaned_continuation();
        telemetry.record_framing_error("short report");
        telemetry.record_inflate_failure(profile::MESSAGE_TYPE_MODEL_PRESET);
        telemetry.record_encrypted_rejection(profile::MESSAGE_TYPE_MODEL_PRESET);
        assert_eq!(
            telemetry.loss_for_type(profile::MESSAGE_TYPE_MODEL_PRESET),
            5
        );
        assert_eq!(telemetry.loss_for_type(profile::MESSAGE_TYPE_SCENE), 3);
    }

    #[test]
    fn a_dispatch_fallthrough_separates_an_unhandled_type_from_a_corrupt_one() {
        let mut telemetry = InboundTelemetry::default();
        // An empty body is a valid encoding of every declared type.
        telemetry.record_dispatch_fallthrough(profile::MESSAGE_TYPE_UNDO_REDO, &[]);
        // A reserved wire type is not decodable as any of them.
        telemetry.record_dispatch_fallthrough(profile::MESSAGE_TYPE_SCENE_COPY, &[0xff, 0xff]);
        // An undeclared number is the transport's to count, not this one's.
        telemetry.record_dispatch_fallthrough(73, &[]);

        let snapshot = telemetry.snapshot();
        assert_eq!(
            snapshot.no_state_handler_types[&profile::MESSAGE_TYPE_UNDO_REDO],
            1
        );
        assert_eq!(
            snapshot.decode_failures[&profile::MESSAGE_TYPE_SCENE_COPY],
            1
        );
        assert!(snapshot.undeclared_types.is_empty());
        assert!(!snapshot
            .no_state_handler_types
            .contains_key(&profile::MESSAGE_TYPE_SCENE_COPY));
        // Only the corrupt body proves the frame went unused.
        assert_eq!(snapshot.unacted_frames, 1);
        assert_eq!(snapshot.no_state_handler_frames, 1);
    }

    #[test]
    fn the_trailer_tally_keys_on_the_bytes_and_stays_bounded() {
        let mut telemetry = InboundTelemetry::default();
        telemetry.record_envelope(profile::MESSAGE_TYPE_SCENE, [0x12, 0x34]);
        telemetry.record_envelope(profile::MESSAGE_TYPE_SCENE, [0x12, 0x34]);
        telemetry.record_envelope(profile::MESSAGE_TYPE_SCENE, [0x00, 0x00]);
        let snapshot = telemetry.snapshot();
        assert_eq!(
            snapshot.device_trailer_bytes,
            vec![
                DeviceTrailerTally {
                    message_type: profile::MESSAGE_TYPE_SCENE,
                    device_bytes: [0x00, 0x00],
                    count: 1,
                },
                DeviceTrailerTally {
                    message_type: profile::MESSAGE_TYPE_SCENE,
                    device_bytes: [0x12, 0x34],
                    count: 2,
                },
            ]
        );

        for value in 0..=u16::MAX {
            telemetry.record_envelope(profile::MESSAGE_TYPE_GRID, value.to_le_bytes());
        }
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.device_trailer_bytes.len(), MAX_TRAILER_KEYS);
        assert!(snapshot.device_trailer_keys_dropped > 0);
        // A saturated tally keeps counting the keys it already admitted.
        assert_eq!(
            snapshot
                .device_trailer_bytes
                .iter()
                .filter(|entry| entry.message_type == profile::MESSAGE_TYPE_SCENE)
                .count(),
            2
        );
    }

    #[test]
    fn undeclared_type_keys_stay_bounded() {
        let mut telemetry = InboundTelemetry::default();
        for number in 1_000..1_000 + (MAX_UNDECLARED_TYPE_KEYS as u32) * 2 {
            telemetry.record_undeclared_type(number);
        }
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.undeclared_types.len(), MAX_UNDECLARED_TYPE_KEYS);
        assert_eq!(
            snapshot.undeclared_type_keys_dropped,
            MAX_UNDECLARED_TYPE_KEYS as u64
        );
    }

    #[test]
    fn undeclared_observations_remain_unacted_after_key_admission_saturates() {
        let mut telemetry = InboundTelemetry::default();
        for number in 1_000..1_000 + (MAX_UNDECLARED_TYPE_KEYS as u32) * 2 {
            telemetry.record_envelope(13, [0, 0]);
            telemetry.record_undeclared_type(number);
        }
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.frames, (MAX_UNDECLARED_TYPE_KEYS * 2) as u64);
        assert_eq!(snapshot.unacted_frames, snapshot.frames);
        assert_eq!(snapshot.undeclared_types.len(), MAX_UNDECLARED_TYPE_KEYS);
        assert_eq!(
            snapshot.undeclared_type_keys_dropped,
            MAX_UNDECLARED_TYPE_KEYS as u64
        );
        telemetry.record_undeclared_type(1_000);
        assert_eq!(
            telemetry.snapshot().unacted_frames,
            snapshot.unacted_frames + 1
        );
        // Trailer saturation is metadata loss, not proof of an unused frame.
        let mut usable = InboundTelemetry::default();
        for number in 0..=MAX_TRAILER_KEYS as u16 {
            usable.record_envelope(profile::MESSAGE_TYPE_SCENE, number.to_le_bytes());
        }
        assert_eq!(usable.snapshot().device_trailer_keys_dropped, 1);
        assert_eq!(usable.snapshot().unacted_frames, 0);
    }

    #[test]
    fn merge_sums_both_halves_and_keeps_the_later_reason() {
        let mut transport = InboundTelemetry::default();
        transport.record_envelope(profile::MESSAGE_TYPE_SCENE, [1, 2]);
        transport.record_undeclared_type(73);
        let mut state = InboundTelemetry::default();
        state.record_no_state_handler(profile::MESSAGE_TYPE_SHOW_GIG_VIEW);

        transport.merge(&state);
        let snapshot = transport.snapshot();
        assert_eq!(snapshot.frames, 1);
        assert_eq!(snapshot.undeclared_types[&73], 1);
        assert_eq!(
            snapshot.no_state_handler_types[&profile::MESSAGE_TYPE_SHOW_GIG_VIEW],
            1
        );
        assert_eq!(snapshot.unacted_frames, 1);
        assert_eq!(snapshot.no_state_handler_frames, 1);
        assert_eq!(
            snapshot.last_unacted.unwrap(),
            "type 24 (ShowGigView) decoded with no state handler"
        );
    }

    #[test]
    fn a_revision_moves_only_when_something_was_recorded() {
        let mut telemetry = InboundTelemetry::default();
        assert_eq!(telemetry.revision(), 0);
        telemetry.record_envelope(profile::MESSAGE_TYPE_SCENE, [0, 0]);
        let after_envelope = telemetry.revision();
        assert!(after_envelope > 0);
        telemetry.record_dispatch_fallthrough(73, &[]);
        assert_eq!(telemetry.revision(), after_envelope);
        telemetry.record_no_state_handler(profile::MESSAGE_TYPE_UNDO_REDO);
        assert!(telemetry.revision() > after_envelope);
    }

    #[test]
    fn merging_disjoint_tallies_preserves_software_caps_and_known_counts() {
        let mut first = InboundTelemetry::default();
        let mut second = InboundTelemetry::default();
        for value in 0..MAX_TRAILER_KEYS as u16 {
            first.record_envelope(profile::MESSAGE_TYPE_SCENE, value.to_le_bytes());
            let shifted = value + (MAX_TRAILER_KEYS / 2) as u16;
            second.record_envelope(profile::MESSAGE_TYPE_SCENE, shifted.to_le_bytes());
            second.record_envelope(profile::MESSAGE_TYPE_SCENE, shifted.to_le_bytes());
        }
        second.record_envelope(profile::MESSAGE_TYPE_SCENE, u16::MAX.to_le_bytes());
        for value in 0..MAX_UNDECLARED_TYPE_KEYS as u32 {
            first.record_undeclared_type(value + 1_000);
            second.record_undeclared_type(value + 1_000 + (MAX_UNDECLARED_TYPE_KEYS / 2) as u32);
            second.record_undeclared_type(value + 1_000 + (MAX_UNDECLARED_TYPE_KEYS / 2) as u32);
        }
        second.record_undeclared_type(u32::MAX);
        first.merge(&second);
        let snapshot = first.snapshot();
        assert_eq!(snapshot.device_trailer_bytes.len(), MAX_TRAILER_KEYS);
        assert_eq!(snapshot.undeclared_types.len(), MAX_UNDECLARED_TYPE_KEYS);
        assert_eq!(
            snapshot.undeclared_types[&(1_000 + (MAX_UNDECLARED_TYPE_KEYS / 2) as u32)],
            3
        );
        let overlapping = (MAX_TRAILER_KEYS / 2) as u16;
        assert_eq!(
            snapshot
                .device_trailer_bytes
                .iter()
                .find(|entry| entry.device_bytes == overlapping.to_le_bytes())
                .unwrap()
                .count,
            3
        );
        assert_eq!(
            snapshot.device_trailer_keys_dropped,
            MAX_TRAILER_KEYS as u64 + 1
        );
        assert_eq!(
            snapshot.undeclared_type_keys_dropped,
            MAX_UNDECLARED_TYPE_KEYS as u64 + 1
        );
        assert_eq!(snapshot.frames, (MAX_TRAILER_KEYS * 3 + 1) as u64);
        assert_eq!(
            snapshot.unacted_frames,
            (MAX_UNDECLARED_TYPE_KEYS * 3 + 1) as u64
        );
        // Existing admitted keys continue accumulating after saturation.
        first.record_envelope(profile::MESSAGE_TYPE_SCENE, overlapping.to_le_bytes());
        first.record_undeclared_type(1_000);
        assert_eq!(first.snapshot().undeclared_types[&1_000], 2);
        assert_eq!(
            first.snapshot().device_trailer_bytes.len(),
            MAX_TRAILER_KEYS
        );
    }

    #[test]
    fn the_snapshot_serializes_with_camel_case_keys() {
        let mut telemetry = InboundTelemetry::default();
        telemetry.record_envelope(profile::MESSAGE_TYPE_SCENE, [0xab, 0xcd]);
        telemetry.record_undeclared_type(73);
        let json = serde_json::to_value(telemetry.snapshot()).unwrap();
        assert_eq!(json["frames"], 1);
        assert_eq!(json["unactedFrames"], 1);
        assert_eq!(json["noStateHandlerFrames"], 0);
        assert_eq!(json["undeclaredTypes"]["73"], 1);
        assert_eq!(json["deviceTrailerBytes"][0]["messageType"], 13);
        assert_eq!(json["deviceTrailerBytes"][0]["deviceBytes"][0], 0xab);
        assert_eq!(json["deviceTrailerBytes"][0]["count"], 1);
    }

    #[test]
    fn assembly_losses_are_counted_merged_and_serialized() {
        let mut transport = InboundTelemetry::default();
        transport.record_abandoned_partial(3, "a new FIRST report arrived");
        transport.record_orphaned_continuation();
        transport.record_orphaned_continuation();
        transport.record_framing_error("report declares more data than it contains");
        let before = transport.revision();
        let mut merged = InboundTelemetry::default();
        merged.record_abandoned_partial(1, "the native ingress epoch changed");
        merged.merge(&transport);
        assert!(merged.revision() >= before);
        let snapshot = merged.snapshot();
        assert_eq!(snapshot.abandoned_partial_frames, 2);
        assert_eq!(snapshot.orphaned_continuation_reports, 2);
        assert_eq!(snapshot.framing_errors, 1);
        // Never assembled, so neither a frame nor an unacted frame.
        assert_eq!(snapshot.frames, 0);
        assert_eq!(snapshot.unacted_frames, 0);
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["abandonedPartialFrames"], 2);
        assert_eq!(json["orphanedContinuationReports"], 2);
        assert_eq!(json["framingErrors"], 1);
        // A snapshot serialized before these counters existed still reads.
        let mut legacy = json.clone();
        for key in [
            "abandonedPartialFrames",
            "orphanedContinuationReports",
            "framingErrors",
        ] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        let read: InboundTelemetrySnapshot = serde_json::from_value(legacy).unwrap();
        assert_eq!(read.framing_errors, 0);
    }
}
