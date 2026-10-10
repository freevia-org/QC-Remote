use crate::profile;
use flate2::read::GzDecoder;
use std::io::Read;
use thiserror::Error;

/// Failure to inflate a gzip-wrapped QC payload.
#[derive(Debug, Error)]
pub enum InflateError {
    /// The payload would inflate past the permitted size ceiling.
    #[error("compressed QC payload exceeds the inflated-size limit")]
    Limit,
    /// The payload is not a valid gzip stream (truncated, corrupt or bad CRC); carries the decoder's message.
    #[error("compressed QC payload could not be decoded: {0}")]
    Decode(String),
}

/// Inflate a gzip-wrapped QC payload while enforcing the shared expanded-size
/// limit. Uncompressed payloads are returned unchanged.
///
/// This is a separate layer from the envelope inflation below, not a way
/// round it. The transport inflates an envelope whose compressed flag is set
/// under that type's own [`max_inflated_bytes_for`] ceiling, and hands the
/// decoders a payload already bounded by it. These decoders then sniff that
/// payload (or a field inside it, such as the model repository's nested
/// catalogue) for a gzip member, and hold that second expansion to the shared
/// [`profile::MAX_INFLATED_BYTES`] ceiling. Each layer is bounded on its own.
pub fn maybe_gunzip(payload: &[u8]) -> Result<Vec<u8>, InflateError> {
    if !payload.starts_with(&[0x1f, 0x8b]) {
        return Ok(payload.to_vec());
    }
    gunzip(payload)
}

// Per-message-type inflation limits
// =================================
//
// These are MEASURED ENVELOPES, NOT PROTOCOL MAXIMA. Neither CorOS nor Cortex
// Control declares a maximum size for any message type; each limit below is
// what one unit was seen to send, plus stated headroom. A legitimate frame
// past its limit is possible on a larger library, a later firmware, or a
// screen or workflow the captures did not exercise, and is rejected loudly
// (counted per type, named in the error), never truncated. Raise a limit
// from a new capture, not by guessing.
//
// They bound only the inflation of an envelope whose compressed flag is set.
// An uncompressed envelope is bounded by the raw frame ceiling,
// `profile::MAX_FRAME_BYTES` (1 MiB), whatever its type, and the compressed
// bytes of a compressed one by the same ceiling; neither changes here. The
// decoders downstream of the transport therefore bound what they receive by
// [`max_delivered_payload_bytes_for`], never by the raw ceiling alone. A gzip
// member nested inside an inflated payload is a third, separate layer: see
// [`maybe_gunzip`].
//
// Every limit below marked `Captured` is derived from real inbound device
// traffic, not chosen: four consecutive lossless HID taps of a CorOS 4.1.0
// unit on 2026-09-28 (idle and reads, session reset and boot, a full device
// backup, scene copy and MIDI; 3,335 logical frames, zero framing errors).
// Each frame was reassembled with the same framing this crate decodes, and
// every frame whose envelope compressed flag was set was inflated, so the
// observed size is the inflated size, or the plain size for a type the unit
// sent uncompressed. A compressed frame of a type can carry no more content
// than its plain form, so both bound the type.
//
//   limit = max(DERIVED_INFLATION_FLOOR_BYTES,
//               next_power_of_two(observed_max_bytes * margin))
//
// The margin is the stated headroom over what the unit was seen to send:
//   - FIXED_SHAPE_MARGIN (8x) for settings, telemetry, control echoes and
//     protocol-chunked transfers, whose size does not grow with the library;
//   - USER_CONTENT_MARGIN (16x) for types that grow with the owner's library
//     or the model catalogue (directory listings, presets, favourites, model
//     presets, cloud products).
// The floor keeps a tiny fixed-shape type from being held to a few hundred
// bytes on the strength of one firmware's encoding.
//
// The captured library is a large one, which is what makes the user-content
// margins meaningful: the biggest type-4 directory listing held 2,127
// entries (283,951 bytes inflated, 133 bytes per entry), and the active
// preset filled 6 of its 32 grid slots (13,534 bytes); all 32 slots at the
// largest observed block (2,067 bytes) would be about 80 KB, a third of the
// derived 256 KiB. The type-40 backup chunks carry a fixed 150,000-byte data
// field. The type-71 model-preset catalogue held 2,753 entries.
//
// Types marked `Uncaptured` are the ones the taps never saw in their
// content-bearing form (the model repository, screenshots, logs,
// diagnostics, neural captures, graphics trees of other screens, and the
// grid, pinned/new model, default-parameter and bulk-operation replies, of
// which only empty acknowledgements or one small update were seen). They keep
// the previous shared ceiling, `profile::MAX_INFLATED_BYTES`, until a capture
// covers them. Any type in neither list keeps `DEFAULT_MAX_INFLATED_BYTES`,
// which is likewise not derived from a capture.
//
// Coverage the captures do NOT provide, and so what these limits do not
// claim:
//   - Uncaptured types (keep 16 MiB): Grid (1), Diagnostics (7),
//     DefaultParameters (19), Screenshot (25), NeuralCapture (36), Logs (44),
//     NewModels (50), ModelRepo (51), PinnedModels (54), BulkOperation (57),
//     NeuralCapture2 (63), RemoteControl (72). Only one RemoteControl
//     graphics tree (one screen, 21,391 bytes) and no screenshot at all were
//     captured, so screen-capture traffic is entirely unmeasured.
//   - Never-seen types (keep the 256 KiB default), including
//     CompilerInhibitedModules (42), seen only empty.
//   - Full-library scale: one library was captured. Its largest folder
//     listing held 2,127 entries; at 133 bytes each the 8 MiB File limit
//     admits about 63,000 entries in one listing, but a library with longer
//     names, or a listing type not exercised, was not measured. The preset
//     limit's headroom over a fully populated 32-slot grid (about 80 KB at
//     the largest captured block) is an estimate, not a measurement.
//   - One firmware (CorOS 4.1.0) and one unit.
//
// The captures themselves stay private (the unit's own traffic names the
// owner's presets and account); only these per-type maxima travel. The unit
// test `captured_limits_follow_the_stated_derivation` recomputes every
// captured limit from its recorded `observed_max_bytes`.

/// Inflated-size ceiling for an envelope-compressed frame of any message type
/// not listed in [`INFLATION_LIMITS`]. Not derived from a capture: it is the
/// project's earlier ceiling for small control types, kept for types the
/// captures never saw.
pub const DEFAULT_MAX_INFLATED_BYTES: usize = 256 * 1024;

/// The smallest limit a captured type is given; see the derivation above.
pub const DERIVED_INFLATION_FLOOR_BYTES: usize = 64 * 1024;

/// Headroom over the observed maximum for types whose size does not grow
/// with the owner's library.
pub const FIXED_SHAPE_MARGIN: usize = 8;

/// Headroom over the observed maximum for types that grow with the owner's
/// library or the model catalogue.
pub const USER_CONTENT_MARGIN: usize = 16;

/// Where one type's inflation limit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflationBasis {
    /// Derived from captured device traffic: the largest observed inflated
    /// (or plain) payload, times `margin`.
    Captured {
        /// Largest payload of this type in the 2026-09-28 taps, in bytes.
        observed_max_bytes: usize,
        /// [`FIXED_SHAPE_MARGIN`] or [`USER_CONTENT_MARGIN`].
        margin: usize,
    },
    /// Never captured in its content-bearing form; keeps the previous shared
    /// ceiling ([`profile::MAX_INFLATED_BYTES`]).
    Uncaptured,
}

/// One message type's inflated-size ceiling and its evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InflationLimit {
    /// The envelope message type this limit applies to.
    pub message_type: u16,
    /// The most a compressed frame of this type may expand to.
    pub max_inflated_bytes: usize,
    /// Where `max_inflated_bytes` comes from.
    pub basis: InflationBasis,
}

const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;

const fn captured(
    message_type: u16,
    max_inflated_bytes: usize,
    observed_max_bytes: usize,
    margin: usize,
) -> InflationLimit {
    InflationLimit {
        message_type,
        max_inflated_bytes,
        basis: InflationBasis::Captured {
            observed_max_bytes,
            margin,
        },
    }
}

const fn uncaptured(message_type: u16) -> InflationLimit {
    InflationLimit {
        message_type,
        max_inflated_bytes: profile::MAX_INFLATED_BYTES,
        basis: InflationBasis::Uncaptured,
    }
}

/// Every message type with its own inflation ceiling, in type order. See the
/// derivation comment above for the evidence and the margins.
#[rustfmt::skip]
pub const INFLATION_LIMITS: [InflationLimit; 40] = [
    uncaptured(profile::MESSAGE_TYPE_GRID),
    captured(profile::MESSAGE_TYPE_SETLIST_POSITION, 64 * KIB, 36, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_IO_SETTINGS, 64 * KIB, 272, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_FILE, 8 * MIB, 283_951, USER_CONTENT_MARGIN),
    captured(profile::MESSAGE_TYPE_TUNER, 64 * KIB, 11, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_DIAGNOSTICS),
    captured(profile::MESSAGE_TYPE_MIDI_SETTINGS, 64 * KIB, 40, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_GENERAL_SETTINGS, 64 * KIB, 162, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_VERSION, 64 * KIB, 287, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_SCENE, 64 * KIB, 4, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_MODE, 64 * KIB, 11, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_RECALL_PRESET, 256 * KIB, 13_534, USER_CONTENT_MARGIN),
    captured(profile::MESSAGE_TYPE_MASTER_VOLUME, 64 * KIB, 9, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_DEFAULT_PARAMETERS),
    captured(profile::MESSAGE_TYPE_RECENTS_FAVORITES, 128 * KIB, 6_892, USER_CONTENT_MARGIN),
    captured(profile::MESSAGE_TYPE_UNDO_REDO, 64 * KIB, 24, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_SCENE_COPY, 64 * KIB, 4, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_SHOW_GIG_VIEW, 64 * KIB, 2, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_SCREENSHOT),
    captured(profile::MESSAGE_TYPE_CPU_LOAD, 64 * KIB, 115, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_LOOPER, 64 * KIB, 12, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_GLOBAL_TEMPO, 64 * KIB, 277, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_PRESET_DIRTY, 64 * KIB, 4, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_MODULE_STATS, 256 * KIB, 28_350, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_NEURAL_CAPTURE),
    captured(profile::MESSAGE_TYPE_GLOBAL_EQ, 64 * KIB, 229, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_LOCAL_BACKUP, 2 * MIB, 150_008, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_LOGS),
    captured(profile::MESSAGE_TYPE_CLOUD_PRODUCT, 512 * KIB, 23_483, USER_CONTENT_MARGIN),
    captured(profile::MESSAGE_TYPE_CONNECTION, 64 * KIB, 2, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_NEW_MODELS),
    uncaptured(profile::MESSAGE_TYPE_MODEL_REPO),
    captured(profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS, 64 * KIB, 34, FIXED_SHAPE_MARGIN),
    captured(profile::MESSAGE_TYPE_SUSPEND_CONNECTION, 64 * KIB, 4, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_PINNED_MODELS),
    uncaptured(profile::MESSAGE_TYPE_BULK_OPERATION),
    captured(profile::MESSAGE_TYPE_UPDATER, 64 * KIB, 6, FIXED_SHAPE_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_NEURAL_CAPTURE2),
    captured(profile::MESSAGE_TYPE_MODEL_PRESET, 2 * MIB, 101_139, USER_CONTENT_MARGIN),
    uncaptured(profile::MESSAGE_TYPE_REMOTE_CONTROL),
];

/// The most a compressed frame of `message_type` may expand to.
pub fn max_inflated_bytes_for(message_type: u16) -> usize {
    INFLATION_LIMITS
        .iter()
        .find(|limit| limit.message_type == message_type)
        .map_or(DEFAULT_MAX_INFLATED_BYTES, |limit| limit.max_inflated_bytes)
}

/// The largest payload the transport can hand a decoder for `message_type`.
///
/// An uncompressed envelope is bounded by the raw frame ceiling,
/// [`profile::MAX_FRAME_BYTES`]; a compressed one by its type's
/// [`max_inflated_bytes_for`] ceiling. A consumer of an
/// [`InboundFrame`](crate::device::transport::InboundFrame) payload cannot
/// tell which of the two it holds, so the larger one is its bound. Checking an
/// inflated payload against the raw ceiling instead would make every per-type
/// limit above 1 MiB ineffective, and turn a large but legitimate directory
/// listing into a startup failure.
pub fn max_delivered_payload_bytes_for(message_type: u16) -> usize {
    max_inflated_bytes_for(message_type).max(profile::MAX_FRAME_BYTES)
}

/// Decode an envelope whose compressed flag is set. Unlike legacy/nested
/// payload sniffing, that flag requires a valid gzip stream, including its CRC.
pub fn gunzip(payload: &[u8]) -> Result<Vec<u8>, InflateError> {
    gunzip_with_limit(payload, profile::MAX_INFLATED_BYTES)
}

/// [`gunzip`] for one envelope's message type, under that type's own
/// [`max_inflated_bytes_for`] ceiling.
pub fn gunzip_for_message_type(payload: &[u8], message_type: u16) -> Result<Vec<u8>, InflateError> {
    gunzip_with_limit(payload, max_inflated_bytes_for(message_type))
}

/// [`gunzip`] with an explicit inflated-size ceiling. Inflation is streamed
/// and stops as soon as one byte past `limit` has been produced, so a
/// decompression bomb never expands (or allocates) beyond the ceiling.
pub fn gunzip_with_limit(payload: &[u8], limit: usize) -> Result<Vec<u8>, InflateError> {
    inflate_bounded(payload, limit)
}

fn inflate_bounded(compressed: impl Read, limit: usize) -> Result<Vec<u8>, InflateError> {
    let mut decoded = Vec::new();
    GzDecoder::new(compressed)
        .take(limit as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| InflateError::Decode(error.to_string()))?;
    if decoded.len() > limit {
        return Err(InflateError::Limit);
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    #[test]
    fn required_gzip_rejects_plain_truncated_and_corrupt_payloads() {
        assert!(matches!(gunzip(&[8, 1]), Err(InflateError::Decode(_))));
        assert!(matches!(gunzip(&[]), Err(InflateError::Decode(_))));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&[8, 1, 24, 2]).unwrap();
        let mut encoded = encoder.finish().unwrap();
        assert_eq!(gunzip(&encoded).unwrap(), [8, 1, 24, 2]);
        assert!(matches!(
            gunzip(&encoded[..encoded.len() - 1]),
            Err(InflateError::Decode(_))
        ));
        let crc_offset = encoded.len() - 8;
        encoded[crc_offset] ^= 1;
        assert!(matches!(gunzip(&encoded), Err(InflateError::Decode(_))));
    }

    #[test]
    fn required_gzip_enforces_the_expanded_size_limit() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&vec![0; profile::MAX_INFLATED_BYTES + 1])
            .unwrap();
        assert!(matches!(
            gunzip(&encoder.finish().unwrap()),
            Err(InflateError::Limit)
        ));
    }

    fn gzip(plain: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(plain).unwrap();
        encoder.finish().unwrap()
    }

    /// Counts how much of the compressed stream the decoder pulled.
    struct CountingReader<'a> {
        inner: &'a [u8],
        consumed: &'a std::cell::Cell<usize>,
    }

    impl Read for CountingReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let count = self.inner.read(buf)?;
            self.consumed.set(self.consumed.get() + count);
            Ok(count)
        }
    }

    #[test]
    fn a_gzip_bomb_stops_inflating_at_the_limit_instead_of_expanding_fully() {
        // 256 MiB of zeros compresses to a few hundred KiB.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        let chunk = vec![0; 1024 * 1024];
        for _ in 0..256 {
            encoder.write_all(&chunk).unwrap();
        }
        let bomb = encoder.finish().unwrap();
        let consumed = std::cell::Cell::new(0);
        let result = inflate_bounded(
            CountingReader {
                inner: &bomb,
                consumed: &consumed,
            },
            DEFAULT_MAX_INFLATED_BYTES,
        );
        assert!(matches!(result, Err(InflateError::Limit)));
        // A full inflate would have read the whole stream. Stopping at the
        // 256 KiB ceiling reads only the decoder's input buffer's worth.
        assert!(
            consumed.get() < bomb.len() / 4,
            "decoder read {} of {} compressed bytes",
            consumed.get(),
            bomb.len()
        );
    }

    #[test]
    fn per_type_limits_keep_small_types_small_and_let_catalogue_types_grow() {
        let one_mib = gzip(&vec![7; 1024 * 1024]);
        assert!(matches!(
            gunzip_for_message_type(&one_mib, profile::MESSAGE_TYPE_VERSION),
            Err(InflateError::Limit)
        ));
        assert!(matches!(
            gunzip_for_message_type(&one_mib, profile::MESSAGE_TYPE_GENERAL_SETTINGS),
            Err(InflateError::Limit)
        ));
        assert_eq!(
            gunzip_for_message_type(&one_mib, profile::MESSAGE_TYPE_MODEL_REPO)
                .unwrap()
                .len(),
            1024 * 1024
        );
        // Exactly at a small type's ceiling still decodes.
        let at_limit = gzip(&vec![3; DERIVED_INFLATION_FLOOR_BYTES]);
        assert_eq!(
            gunzip_for_message_type(&at_limit, profile::MESSAGE_TYPE_SCENE)
                .unwrap()
                .len(),
            DERIVED_INFLATION_FLOOR_BYTES
        );
        let over_large = gzip(&vec![0; profile::MAX_INFLATED_BYTES + 1]);
        assert!(matches!(
            gunzip_for_message_type(&over_large, profile::MESSAGE_TYPE_MODEL_REPO),
            Err(InflateError::Limit)
        ));
        assert_eq!(max_inflated_bytes_for(73), DEFAULT_MAX_INFLATED_BYTES);
    }

    #[test]
    fn captured_limits_follow_the_stated_derivation() {
        for limit in INFLATION_LIMITS {
            match limit.basis {
                InflationBasis::Captured {
                    observed_max_bytes,
                    margin,
                } => {
                    assert!(
                        margin == FIXED_SHAPE_MARGIN || margin == USER_CONTENT_MARGIN,
                        "type {} uses an undeclared margin {margin}",
                        limit.message_type
                    );
                    let derived = (observed_max_bytes * margin)
                        .next_power_of_two()
                        .max(DERIVED_INFLATION_FLOOR_BYTES);
                    assert_eq!(
                        limit.max_inflated_bytes, derived,
                        "type {}: observed {observed_max_bytes} x {margin}",
                        limit.message_type
                    );
                    // A captured limit always admits what the unit was seen to send.
                    assert!(limit.max_inflated_bytes >= observed_max_bytes);
                    // And never reaches the shared ceiling it replaces.
                    assert!(limit.max_inflated_bytes < profile::MAX_INFLATED_BYTES);
                }
                InflationBasis::Uncaptured => assert_eq!(
                    limit.max_inflated_bytes,
                    profile::MAX_INFLATED_BYTES,
                    "type {} has no capture and keeps the shared ceiling",
                    limit.message_type
                ),
            }
        }
    }

    #[test]
    fn the_limit_table_is_in_type_order_with_one_row_per_type() {
        assert!(INFLATION_LIMITS.len() >= 2);
        for pair in INFLATION_LIMITS.windows(2) {
            assert!(
                pair[0].message_type < pair[1].message_type,
                "types {} and {} are out of order or duplicated",
                pair[0].message_type,
                pair[1].message_type
            );
        }
        for limit in INFLATION_LIMITS {
            assert_eq!(
                max_inflated_bytes_for(limit.message_type),
                limit.max_inflated_bytes
            );
        }
    }

    /// One payload just below, at, and just above `limit`, inflated as `message_type`.
    fn assert_boundary(message_type: u16, limit: usize) {
        let below = gzip(&vec![0x5a; limit - 1]);
        let at = gzip(&vec![0x5a; limit]);
        let above = gzip(&vec![0x5a; limit + 1]);
        assert_eq!(
            gunzip_for_message_type(&below, message_type).unwrap().len(),
            limit - 1,
            "type {message_type} just below {limit}"
        );
        assert_eq!(
            gunzip_for_message_type(&at, message_type).unwrap().len(),
            limit,
            "type {message_type} at {limit}"
        );
        assert!(
            matches!(
                gunzip_for_message_type(&above, message_type),
                Err(InflateError::Limit)
            ),
            "type {message_type} just above {limit}"
        );
    }

    #[test]
    fn every_captured_type_admits_its_limit_and_rejects_one_byte_more() {
        for limit in INFLATION_LIMITS {
            if matches!(limit.basis, InflationBasis::Captured { .. }) {
                assert_boundary(limit.message_type, limit.max_inflated_bytes);
            }
        }
    }

    #[test]
    fn uncaptured_and_unlisted_types_keep_their_previous_ceilings_at_the_boundary() {
        assert_boundary(
            profile::MESSAGE_TYPE_MODEL_REPO,
            profile::MAX_INFLATED_BYTES,
        );
        // Not in the table: the default, which a capture did not set.
        assert_boundary(
            profile::MESSAGE_TYPE_COMPILER_INHIBITED_MODULES,
            DEFAULT_MAX_INFLATED_BYTES,
        );
        assert_boundary(73, DEFAULT_MAX_INFLATED_BYTES);
    }
}
