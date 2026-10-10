//! Quad Cortex HID report framing.

use crate::profile;
use thiserror::Error;

/// Size in bytes of an HID report body, excluding the leading report-ID byte.
pub const REPORT_BODY_SIZE: usize = 128;
/// Size in bytes of a complete HID report: the report-ID byte followed by [`REPORT_BODY_SIZE`] bytes.
pub const REPORT_SIZE: usize = REPORT_BODY_SIZE + 1;
/// Report ID that [`encode_frame`] writes into every host-to-device report.
pub const OUT_REPORT_ID: u8 = 0x02;
/// Report ID used by device-to-host reports.
pub const IN_REPORT_ID: u8 = 0x01;
/// Flag set in the high header byte of the first report of a logical message.
pub const FLAG_FIRST: u8 = 0x40;
/// Flag set in the high header byte of the last report of a logical message.
pub const FLAG_LAST: u8 = 0x80;
/// Maximum number of message bytes carried by one report: the report body less its two header bytes.
pub const CHUNK_SIZE: usize = REPORT_BODY_SIZE - 2;
/// Size in bytes of the envelope trailer appended to every logical message payload.
pub const TRAILER_SIZE: usize = 8;
const MAX_REPORTS: usize = (profile::MAX_FRAME_BYTES + TRAILER_SIZE).div_ceil(CHUNK_SIZE);

/// Reasons a set of HID reports could not be assembled into, or an envelope
/// converted to or from, a logical QC frame.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    /// No reports were supplied.
    #[error("at least one HID report is required")]
    Empty,
    /// A report is shorter than its three-byte prefix (report ID and two-byte header).
    #[error("HID report is too short")]
    ShortReport,
    /// The first report of the batch does not carry [`FLAG_FIRST`].
    #[error("first HID report has no FIRST flag")]
    MissingFirst,
    /// The final report of the batch does not carry [`FLAG_LAST`].
    #[error("final HID report has no LAST flag")]
    MissingLast,
    /// A report is longer than [`REPORT_SIZE`], declares more than [`CHUNK_SIZE`] bytes, or declares more bytes than it contains.
    #[error("report declares more data than it contains")]
    InvalidLength,
    /// A report other than the first carries [`FLAG_FIRST`], or a report other than the last carries [`FLAG_LAST`].
    #[error("report batch contains more than one logical frame")]
    InvalidSequence,
    /// The assembled message is shorter than [`TRAILER_SIZE`] bytes, so it has no envelope trailer.
    #[error("logical message is shorter than the QC trailer")]
    MissingTrailer,
    /// The message spans more reports, or more bytes, than the shared frame-size limit allows.
    #[error("logical message exceeds the shared QC frame-size limit")]
    TooLarge,
    /// The 32-bit message type does not fit in a `u16`; carries the raw value.
    #[error("message type {0} is outside the native host's u16 dispatch range")]
    MessageTypeOutOfRange(u32),
    /// The message type belongs to a family that requires encryption, which this build cannot perform.
    #[error("message type belongs to Neural DSP's cloud-account/license family, which this build cannot encrypt or decrypt")]
    UnsupportedEncryptedMessageType,
    /// The frame's compressed flag was set but its payload could not be inflated; carries the decoder's message.
    #[error("compressed QC frame could not be decoded: {0}")]
    Compression(String),
}

/// The complete eight-byte QC envelope used by the transport.
///
/// Known protocol message IDs currently fit in one byte, but the wire reserves
/// four bytes. Keeping the full value and the otherwise opaque trailer fields
/// makes this codec preserve every envelope field for diagnostics and future
/// firmware messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Message type from the trailer. The wire reserves four bytes (little-endian).
    pub message_type: u32,
    /// Message body, without the trailer. Still compressed when `compressed` is set.
    pub payload: Vec<u8>,
    /// Trailer encrypted flag (`true` when the byte is non-zero).
    pub encrypted: bool,
    /// Trailer compressed flag (`true` when the byte is non-zero); the payload is then a gzip stream.
    pub compressed: bool,
    /// The two trailing envelope bytes. They are preserved as received and are not interpreted by this codec; their meaning is unknown.
    ///
    /// This crate writes zero in both. The device has been observed to write
    /// varying values, often zero, which do not appear to be a protocol field.
    /// In about 3,300 frames captured passively on CorOS 4.1.0, just over half
    /// carried `[0, 0]`. The rest showed 77 other pairs: the same pair turned
    /// up on unrelated message types, and the frames of a single type carried
    /// dozens of different pairs, which is what unused or padding bytes would
    /// look like. Host software is believed to ignore both bytes; that has not
    /// been verified.
    pub device_bytes: [u8; 2],
}

/// Incrementally assembles logical QC frames from the live HID report stream.
/// This is wire-format state only; connection/session policy lives in
/// `quadcortex::device`.
#[derive(Debug, Default)]
pub struct FrameAssembler {
    body: Vec<u8>,
    report_count: usize,
    discard: Option<FrameDiscard>,
}

/// Something [`FrameAssembler::push_frame`] threw away without returning an
/// error. Neither is a protocol violation the assembler can reject, but each
/// loses inbound data, so it is reported for the caller to count rather than
/// vanishing; read it with [`FrameAssembler::take_discard`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDiscard {
    /// A FIRST report arrived while another message was still open, and the
    /// open message was dropped. Carries the number of reports it had.
    AbandonedPartial {
        /// Reports the dropped message had received.
        reports: usize,
    },
    /// A continuation report arrived with no message open, and was ignored.
    OrphanedContinuation,
}

impl FrameAssembler {
    /// Creates an assembler with no partially received message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards any partially assembled message so the next message starts cleanly.
    pub fn reset(&mut self) {
        self.body.clear();
        self.report_count = 0;
    }

    /// Reports received for the message currently being assembled; zero when
    /// no message is open.
    pub fn pending_reports(&self) -> usize {
        self.report_count
    }

    /// What the most recent [`FrameAssembler::push_frame`] discarded without an
    /// error, if anything. Cleared by the next push and by reading it.
    pub fn take_discard(&mut self) -> Option<FrameDiscard> {
        self.discard.take()
    }

    /// Feeds one report and returns `(message_type, payload)` once the last report of a message arrives.
    ///
    /// Returns `Ok(None)` while a message is incomplete, and also for a continuation report that arrives before any
    /// report with [`FLAG_FIRST`], which is ignored. Any error resets the assembler. Fails with
    /// [`FrameError::MessageTypeOutOfRange`] when the type does not fit in a `u16`; use [`FrameAssembler::push_frame`]
    /// to keep the full envelope.
    pub fn push(&mut self, report: Vec<u8>) -> Result<Option<(u16, Vec<u8>)>, FrameError> {
        self.push_frame(report).and_then(|frame| {
            frame
                .map(|frame| {
                    let message_type = u16::try_from(frame.message_type)
                        .map_err(|_| FrameError::MessageTypeOutOfRange(frame.message_type))?;
                    Ok((message_type, frame.payload))
                })
                .transpose()
        })
    }

    /// Assemble the complete QC envelope without discarding any trailer flags.
    pub fn push_frame(&mut self, report: impl AsRef<[u8]>) -> Result<Option<Frame>, FrameError> {
        self.discard = None;
        let chunk = match ReportChunk::parse(report.as_ref()) {
            Ok(chunk) => chunk,
            Err(error) => {
                self.reset();
                return Err(error);
            }
        };
        if chunk.first {
            if self.report_count > 0 {
                self.discard = Some(FrameDiscard::AbandonedPartial {
                    reports: self.report_count,
                });
            }
            self.reset();
        } else if self.report_count == 0 {
            // A host can attach between chunks, or the device can leave one
            // continuation queued after a reset. Ignore it until FIRST.
            self.discard = Some(FrameDiscard::OrphanedContinuation);
            return Ok(None);
        }
        if self.report_count >= MAX_REPORTS
            || self.body.len() + chunk.payload.len() > profile::MAX_FRAME_BYTES + TRAILER_SIZE
        {
            self.reset();
            return Err(FrameError::TooLarge);
        }
        self.body.extend_from_slice(chunk.payload);
        self.report_count += 1;
        if !chunk.last {
            return Ok(None);
        }
        self.report_count = 0;
        decode_body(std::mem::take(&mut self.body)).map(Some)
    }
}

/// A validated slice of one native report. Both batch and streaming decoding
/// use the packed u16 header (length & 0x3fff, FIRST, LAST). Report IDs are
/// checked by the native adapter; either direction may appear. Padding is
/// never part of the logical payload.
struct ReportChunk<'a> {
    payload: &'a [u8],
    first: bool,
    last: bool,
}

impl<'a> ReportChunk<'a> {
    fn parse(report: &'a [u8]) -> Result<Self, FrameError> {
        if report.len() < 3 {
            return Err(FrameError::ShortReport);
        }
        let header = u16::from_le_bytes([report[1], report[2]]);
        let length = usize::from(header & 0x3fff);
        if report.len() > REPORT_SIZE || length > CHUNK_SIZE || report.len() < 3 + length {
            return Err(FrameError::InvalidLength);
        }
        Ok(Self {
            payload: &report[3..3 + length],
            first: header & 0x4000 != 0,
            last: header & 0x8000 != 0,
        })
    }
}

/// Encodes a message as the sequence of host-to-device reports that carries it, with the encrypted and
/// compressed flags and the device bytes all zero.
pub fn encode(message_type: u16, payload: &[u8]) -> Vec<[u8; REPORT_SIZE]> {
    encode_message(u32::from(message_type), payload)
}

/// Encode the complete 32-bit message-type field used by the QC trailer.
pub fn encode_message(message_type: u32, payload: &[u8]) -> Vec<[u8; REPORT_SIZE]> {
    encode_frame(&Frame {
        message_type,
        payload: payload.to_vec(),
        encrypted: false,
        compressed: false,
        device_bytes: [0, 0],
    })
}

/// Encode a logical message while preserving the complete QC trailer.
pub fn encode_frame(frame: &Frame) -> Vec<[u8; REPORT_SIZE]> {
    let payload = &frame.payload;
    let mut body = Vec::with_capacity(payload.len() + TRAILER_SIZE);
    body.extend_from_slice(payload);
    body.extend_from_slice(&frame.message_type.to_le_bytes());
    body.push(u8::from(frame.encrypted));
    body.push(u8::from(frame.compressed));
    body.extend_from_slice(&frame.device_bytes);

    body.chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(index, chunk)| {
            let mut report = [0_u8; REPORT_SIZE];
            report[0] = OUT_REPORT_ID;
            report[1] = chunk.len() as u8;
            report[2] = (if index == 0 { FLAG_FIRST } else { 0 })
                | (if (index + 1) * CHUNK_SIZE >= body.len() {
                    FLAG_LAST
                } else {
                    0
                });
            report[3..3 + chunk.len()].copy_from_slice(chunk);
            report
        })
        .collect()
}

/// Returns whether the last report in `reports` carries [`FLAG_LAST`]; `false` for an empty slice.
///
/// Fails if that report is malformed.
pub fn is_complete(reports: &[Vec<u8>]) -> Result<bool, FrameError> {
    let Some(last) = reports.last() else {
        return Ok(false);
    };
    Ok(ReportChunk::parse(last)?.last)
}

/// Decodes a complete batch of reports into `(message_type, payload)`.
///
/// Fails as [`decode_reports`] does, and with [`FrameError::MessageTypeOutOfRange`] when the message type does
/// not fit in a `u16`.
pub fn decode(reports: &[Vec<u8>]) -> Result<(u16, Vec<u8>), FrameError> {
    let frame = decode_reports(reports)?;
    let message_type = u16::try_from(frame.message_type)
        .map_err(|_| FrameError::MessageTypeOutOfRange(frame.message_type))?;
    Ok((message_type, frame.payload))
}

/// Decode a logical message without discarding any QC trailer metadata.
pub fn decode_reports(reports: &[Vec<u8>]) -> Result<Frame, FrameError> {
    if reports.len() > MAX_REPORTS {
        return Err(FrameError::TooLarge);
    }
    let Some(first) = reports.first() else {
        return Err(FrameError::Empty);
    };
    let Some(last) = reports.last() else {
        return Err(FrameError::Empty);
    };
    if !ReportChunk::parse(first)?.first {
        return Err(FrameError::MissingFirst);
    }
    if !ReportChunk::parse(last)?.last {
        return Err(FrameError::MissingLast);
    }

    let mut body = Vec::new();
    for (index, report) in reports.iter().enumerate() {
        let chunk = ReportChunk::parse(report)?;
        if (chunk.first && index != 0) || (chunk.last && index + 1 != reports.len()) {
            return Err(FrameError::InvalidSequence);
        }
        if body.len() + chunk.payload.len() > profile::MAX_FRAME_BYTES + TRAILER_SIZE {
            return Err(FrameError::TooLarge);
        }
        body.extend_from_slice(chunk.payload);
    }
    decode_body(body)
}

fn decode_body(mut body: Vec<u8>) -> Result<Frame, FrameError> {
    if body.len() < TRAILER_SIZE {
        return Err(FrameError::MissingTrailer);
    }
    let trailer = body.split_off(body.len() - TRAILER_SIZE);
    Ok(Frame {
        message_type: u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]),
        payload: body,
        encrypted: trailer[4] != 0,
        compressed: trailer[5] != 0,
        device_bytes: [trailer[6], trailer[7]],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn matches_known_wire_capture_of_version_read() {
        let expected = hex("020ac008030a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(encode(10, &[0x08, 0x03])[0].as_slice(), expected.as_slice());
        assert_eq!(decode(&[expected]).unwrap(), (10, vec![0x08, 0x03]));
    }

    #[test]
    fn matches_known_wire_capture_of_scene_update() {
        let payload = hex("0801100b1801");
        let expected = hex("020ec00801100b18010d0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(encode(13, &payload)[0].as_slice(), expected.as_slice());
    }

    #[test]
    fn single_report_round_trip() {
        let reports = encode(13, &[8, 1, 24, 3]);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0][0], OUT_REPORT_ID);
        assert_eq!(reports[0][2], FLAG_FIRST | FLAG_LAST);
        let decoded =
            decode(&reports.iter().map(|item| item.to_vec()).collect::<Vec<_>>()).unwrap();
        assert_eq!(decoded, (13, vec![8, 1, 24, 3]));
    }

    #[test]
    fn multi_report_round_trip() {
        let payload = vec![0x5a; CHUNK_SIZE * 3 + 17];
        let reports = encode(51, &payload);
        assert_eq!(reports.len(), 4);
        assert_eq!(reports[0][2], FLAG_FIRST);
        assert_eq!(reports[1][2], 0);
        assert_eq!(reports[3][2], FLAG_LAST);
        let decoded =
            decode(&reports.iter().map(|item| item.to_vec()).collect::<Vec<_>>()).unwrap();
        assert_eq!(decoded, (51, payload));
    }

    #[test]
    fn lossless_codec_preserves_the_complete_upstream_trailer() {
        let mut report = encode_message(0x1234_5678, &[0xaa, 0xbb])[0].to_vec();
        let trailer = 3 + 2;
        report[trailer + 4] = 1;
        report[trailer + 5] = 1;
        report[trailer + 6] = 0x9a;
        report[trailer + 7] = 0xbc;

        assert_eq!(
            decode_reports(&[report]).unwrap(),
            Frame {
                message_type: 0x1234_5678,
                payload: vec![0xaa, 0xbb],
                encrypted: true,
                compressed: true,
                device_bytes: [0x9a, 0xbc],
            }
        );
    }

    #[test]
    fn lossless_encoder_places_payload_before_complete_trailer() {
        let frame = Frame {
            message_type: 58,
            payload: vec![0xaa, 0xbb, 0xcc],
            encrypted: true,
            compressed: true,
            device_bytes: [0x12, 0x34],
        };
        let report = encode_frame(&frame)[0].to_vec();
        assert_eq!(
            &report[3..3 + 11],
            &[0xaa, 0xbb, 0xcc, 58, 0, 0, 0, 1, 1, 0x12, 0x34]
        );
        assert_eq!(decode_reports(&[report]).unwrap(), frame);
    }

    #[test]
    fn native_dispatch_rejects_but_lossless_decode_retains_large_types() {
        let report = encode_message(0x1_0000, &[])[0].to_vec();
        assert_eq!(
            decode(&[report]),
            Err(FrameError::MessageTypeOutOfRange(0x1_0000))
        );
    }

    #[test]
    fn malformed_sequences_are_rejected() {
        assert_eq!(decode(&[]), Err(FrameError::Empty));
        let mut report = encode(1, &[])[0].to_vec();
        report[2] &= !FLAG_FIRST;
        assert_eq!(decode(&[report]), Err(FrameError::MissingFirst));
    }

    #[test]
    fn every_high_length_bit_is_validated_before_decoding() {
        // A u16 masked with 0x3fff is the valid length range. These
        // declarations exceed a report's capacity even though their low byte
        // looks valid.
        for high_length_bits in 1..=0x3f {
            let mut report = encode(10, &[8, 3])[0].to_vec();
            report[2] |= high_length_bits;
            assert_eq!(
                decode_reports(&[report.clone()]),
                Err(FrameError::InvalidLength)
            );
            assert_eq!(
                FrameAssembler::new().push_frame(report),
                Err(FrameError::InvalidLength)
            );
        }
    }

    #[test]
    fn invalid_nonfinal_fragment_is_rejected_immediately() {
        let mut report = encode(51, &[0; CHUNK_SIZE])[0].to_vec();
        report[1] = 127;
        assert_eq!(
            FrameAssembler::new().push_frame(report),
            Err(FrameError::InvalidLength)
        );
    }

    #[test]
    fn batch_decoder_rejects_multiple_logical_frames() {
        let first = encode(10, &[8, 3])[0].to_vec();
        let second = encode(13, &[8, 1, 24, 1])[0].to_vec();
        assert!(decode_reports(&[first, second]).is_err());
    }

    #[test]
    fn streaming_and_batch_decoders_enforce_the_same_payload_limit() {
        for length in [profile::MAX_FRAME_BYTES, profile::MAX_FRAME_BYTES + 1] {
            let payload = vec![0x5a; length];
            let reports = encode(51, &payload)
                .into_iter()
                .map(Vec::from)
                .collect::<Vec<_>>();
            let batch = decode_reports(&reports);
            let mut assembler = FrameAssembler::new();
            let mut streamed = Ok(None);
            for report in &reports {
                streamed = assembler.push_frame(report);
                if streamed.is_err() {
                    break;
                }
            }
            if length == profile::MAX_FRAME_BYTES {
                assert_eq!(batch.as_ref().unwrap().payload, payload);
                assert_eq!(streamed.unwrap(), Some(batch.unwrap()));
            } else {
                assert_eq!(batch, Err(FrameError::TooLarge));
                assert_eq!(streamed, Err(FrameError::TooLarge));
            }
        }
    }

    #[test]
    fn invalid_fragment_resets_stream_and_only_first_resynchronizes() {
        let reports = encode(51, &[0x33; CHUNK_SIZE]);
        let mut assembler = FrameAssembler::new();
        assert!(assembler.push_frame(reports[0]).unwrap().is_none());
        let mut invalid = reports[0];
        invalid[2] = 1; // 382 bytes, no FIRST/LAST.
        assert_eq!(
            assembler.push_frame(invalid),
            Err(FrameError::InvalidLength)
        );
        assert!(assembler.push_frame(reports[1]).unwrap().is_none());
        let replacement = encode(10, &[8, 3])[0];
        assert_eq!(
            assembler.push_frame(replacement).unwrap().unwrap().payload,
            [8, 3]
        );
    }

    #[test]
    fn the_assembler_reports_what_it_discards_without_an_error() {
        let mut assembler = FrameAssembler::new();
        let open = encode(51, &[0x33; CHUNK_SIZE * 2]);
        assert!(assembler.push_frame(open[0]).unwrap().is_none());
        assert_eq!(assembler.take_discard(), None);
        assert!(assembler.push_frame(open[1]).unwrap().is_none());
        assert_eq!(assembler.pending_reports(), 2);
        // A new FIRST report drops the open message.
        let replacement = encode(10, &[8, 3])[0];
        assert!(assembler.push_frame(replacement).unwrap().is_some());
        assert_eq!(
            assembler.take_discard(),
            Some(FrameDiscard::AbandonedPartial { reports: 2 })
        );
        assert_eq!(assembler.take_discard(), None);
        assert_eq!(assembler.pending_reports(), 0);
        // A continuation with nothing open is ignored and reported.
        assert!(assembler.push_frame(open[2]).unwrap().is_none());
        assert_eq!(
            assembler.take_discard(),
            Some(FrameDiscard::OrphanedContinuation)
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn arbitrary_bounded_payloads_round_trip(
            message_type in any::<u16>(),
            payload in prop::collection::vec(any::<u8>(), 0..8192)
        ) {
            let reports = encode(message_type, &payload);
            let owned = reports.iter().map(|report| report.to_vec()).collect::<Vec<_>>();
            prop_assert_eq!(decode(&owned), Ok((message_type, payload)));
        }

        #[test]
        fn arbitrary_report_sequences_never_panic_or_allocate_past_the_bound(
            reports in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..160), 0..96)
        ) {
            let result = std::panic::catch_unwind(|| decode(&reports));
            prop_assert!(result.is_ok());
            if let Ok(Ok((_message_type, payload))) = result {
                prop_assert!(payload.len() <= profile::MAX_FRAME_BYTES);
            }
        }

        #[test]
        fn assembler_recovers_after_an_arbitrary_malformed_prefix(
            prefix in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..160), 0..32),
            message_type in any::<u16>(),
            payload in prop::collection::vec(any::<u8>(), 0..2048)
        ) {
            let mut assembler = FrameAssembler::new();
            for report in prefix {
                let _ = assembler.push(report);
            }
            let mut observed = None;
            for report in encode(message_type, &payload) {
                if let Ok(Some(frame)) = assembler.push(report.to_vec()) {
                    observed = Some(frame);
                }
            }
            prop_assert_eq!(observed, Some((message_type, payload)));
        }
    }
}
