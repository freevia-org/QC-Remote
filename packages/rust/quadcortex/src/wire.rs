//! Minimal protobuf wire inspection used for request correlation before a
//! concrete message type is decoded.

use thiserror::Error;

/// Malformed protobuf wire data found while scanning a payload.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    /// A varint ran past the end of the payload or exceeded 64 bits.
    #[error("truncated protobuf varint")]
    TruncatedVarint,
    /// A field used a wire type this scanner cannot skip (anything other than 0, 1, 2 or 5); carries the wire type.
    #[error("unsupported protobuf wire type {0}")]
    UnsupportedWire(u8),
    /// A fixed-width or length-delimited field extends past the end of the payload.
    #[error("truncated protobuf field")]
    TruncatedField,
    /// A tag has field number zero or exceeds protobuf's 29-bit field-number domain.
    #[error("invalid protobuf field number {0}")]
    InvalidFieldNumber(u64),
    /// The requested scalar field was encoded with a non-varint wire type.
    #[error("protobuf field {field} requires varint wire type, received {wire}")]
    InvalidScalarWire {
        /// The requested protobuf scalar field number.
        field: u32,
        /// The incompatible wire type carried by that field's tag.
        wire: u8,
    },
}

/// Message types whose `request_id` is protobuf field 1 rather than field 2.
///
/// Every other correlated message declares `request_id = 2`. Reading field 2 on
/// one of these instead returns an unrelated varint — a `bool enable`, a
/// `bool connected`, a `confirmation_id`, or an `error_code` — which can never
/// equal the caller's id, so the reply is silently never delivered and the
/// request times out with no diagnostic.
pub const REQUEST_ID_FIELD_ONE_TYPES: [u16; 8] = [11, 12, 16, 47, 49, 52, 53, 56];

/// The correlation id a message carries, read from the field its schema declares.
///
/// `None` means this message carries no correlation id, which is normal: several
/// replies (Version among them) legitimately echo none, and callers fall back to
/// message-type correlation for those.
pub fn request_id(message_type: u16, payload: &[u8]) -> Option<u64> {
    let field = if REQUEST_ID_FIELD_ONE_TYPES.contains(&message_type) {
        1
    } else {
        2
    };
    varint_field(payload, field).ok().flatten()
}

/// Returns the last varint-encoded (wire type 0) field numbered `wanted`, scanning the top level of `payload`.
///
/// Singular protobuf scalars and these request-id oneofs use last-value-wins semantics.
/// Returns `Ok(None)` if there is no such field. Other fields are skipped; the wanted scalar must use wire type 0. An error is returned if
/// any part of the scan is malformed, including a tail after the wanted field.
pub fn varint_field(payload: &[u8], wanted: u32) -> Result<Option<u64>, WireError> {
    let mut offset = 0;
    let mut result = None;
    while offset < payload.len() {
        let tag = read_varint(payload, &mut offset)?;
        let field = tag >> 3;
        if field == 0 || field > 0x1fff_ffff {
            return Err(WireError::InvalidFieldNumber(field));
        }
        let wire = (tag & 7) as u8;
        if field == u64::from(wanted) {
            if wire != 0 {
                return Err(WireError::InvalidScalarWire {
                    field: wanted,
                    wire,
                });
            }
            result = Some(read_varint(payload, &mut offset)?);
        } else {
            skip(payload, &mut offset, wire)?;
        }
    }
    Ok(result)
}

fn read_varint(payload: &[u8], offset: &mut usize) -> Result<u64, WireError> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let Some(byte) = payload.get(*offset).copied() else {
            return Err(WireError::TruncatedVarint);
        };
        *offset += 1;
        // The tenth byte owns only bit 63. Rust's left shift otherwise
        // silently discards its excess bits, aliasing a malformed ID to a
        // valid one before the concrete protobuf decoder gets to inspect it.
        if shift == 63 && byte & 0x7e != 0 {
            return Err(WireError::TruncatedVarint);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(WireError::TruncatedVarint)
}

fn skip(payload: &[u8], offset: &mut usize, wire: u8) -> Result<(), WireError> {
    let length = match wire {
        0 => {
            read_varint(payload, offset)?;
            return Ok(());
        }
        1 => 8,
        2 => {
            usize::try_from(read_varint(payload, offset)?).map_err(|_| WireError::TruncatedField)?
        }
        5 => 4,
        other => return Err(WireError::UnsupportedWire(other)),
    };
    if payload.len().saturating_sub(*offset) < length {
        return Err(WireError::TruncatedField);
    }
    *offset += length;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    const SCHEMA: &str = include_str!("../proto/ProductionAutomation.proto");

    /// The declared `request_id` field number inside one top-level message.
    fn schema_request_id_field(message: &str) -> Option<u32> {
        let header = format!("\nmessage {message} {{");
        let start = SCHEMA.find(&header)? + header.len();
        let body = &SCHEMA[start..start + SCHEMA[start..].find("\n}")?];
        let declaration = body.find("uint64 request_id = ")? + "uint64 request_id = ".len();
        body[declaration..]
            .split(';')
            .next()?
            .trim()
            .parse::<u32>()
            .ok()
    }

    #[test]
    fn locates_request_id_around_other_wire_types() {
        assert_eq!(
            varint_field(&[0x08, 0x01, 0x12, 0x02, 0xaa, 0xbb, 0x18, 0x2a], 3),
            Ok(Some(42))
        );
        assert_eq!(varint_field(&[0x08, 0x01], 2), Ok(None));
    }

    #[test]
    fn correlation_uses_the_same_duplicate_id_as_the_concrete_protobuf_decoder() {
        use crate::device::correlation::ResponseExpectation;
        use crate::proto::cortex_protobuf_v2 as pa;

        let connection = [0x08, 42, 0x10, 1, 0x08, 43];
        let decoded = pa::ConnectionMessage::decode(connection.as_slice()).unwrap();
        assert_eq!(
            decoded.request_id,
            Some(pa::connection_message::RequestId::RequestId(43))
        );
        let scene = [0x08, 1, 0x10, 42, 0x10, 43];
        let decoded = pa::SceneMessage::decode(scene.as_slice()).unwrap();
        assert_eq!(
            decoded.request_id,
            Some(pa::scene_message::RequestId::RequestId(43))
        );
        for (kind, payload) in [(49, connection.as_slice()), (13, scene.as_slice())] {
            assert_eq!(request_id(kind, payload), Some(43));
            assert!(!ResponseExpectation::new(kind, Some(42), 0, 100).matches(kind, payload));
            assert!(ResponseExpectation::new(kind, Some(43), 0, 100).matches(kind, payload));
        }
    }

    #[test]
    fn a_malformed_tail_cannot_turn_a_partial_scan_into_correlation_evidence() {
        use crate::proto::cortex_protobuf_v2 as pa;
        for tail in [vec![0x80], vec![0], vec![0x1a, 3, 1]] {
            let mut payload = vec![0x08, 1, 0x10, 42];
            payload.extend(tail);
            assert!(pa::SceneMessage::decode(payload.as_slice()).is_err());
            assert!(varint_field(&payload, 2).is_err());
            assert_eq!(request_id(13, &payload), None);
        }
        assert_eq!(
            request_id(13, &[0x10, 42, 0x9a, 0x06, 2, 0xaa, 0xbb]),
            Some(42),
            "well-formed unknown tails remain forward compatible"
        );
    }

    #[test]
    fn a_request_id_with_wrong_wire_type_is_not_skipped_as_an_unknown_field() {
        use crate::proto::cortex_protobuf_v2 as pa;
        for wrong_field in [
            vec![0x11, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![0x12, 1, 0],
            vec![0x15, 0, 0, 0, 0],
        ] {
            let mut payload = vec![0x08, 1, 0x10, 42];
            let wire = wrong_field[0] & 7;
            payload.extend(wrong_field);
            assert!(pa::SceneMessage::decode(payload.as_slice()).is_err());
            assert_eq!(
                varint_field(&payload, 2),
                Err(WireError::InvalidScalarWire { field: 2, wire })
            );
            assert_eq!(request_id(13, &payload), None);
        }
    }

    #[test]
    fn correlation_refuses_varints_that_overflow_u64_instead_of_aliasing_ids() {
        let mut maximum = vec![0x10];
        maximum.extend([0xff; 9]);
        maximum.push(0x01);
        assert_eq!(varint_field(&maximum, 2), Ok(Some(u64::MAX)));
        assert_eq!(request_id(13, &maximum), Some(u64::MAX));
        for last in [0x02, 0x7f, 0x81, 0xff] {
            let mut overflow = maximum.clone();
            *overflow.last_mut().unwrap() = last;
            assert_eq!(varint_field(&overflow, 2), Err(WireError::TruncatedVarint));
            assert_eq!(
                request_id(13, &overflow),
                None,
                "overflow cannot correlate a request"
            );
        }
        let mut overlong = vec![0x10];
        overlong.extend([0x80; 10]);
        overlong.push(0x00);
        assert_eq!(varint_field(&overlong, 2), Err(WireError::TruncatedVarint));
    }

    #[test]
    fn malformed_tags_cannot_alias_valid_request_id_fields() {
        let encode = |mut value: u64| {
            let mut bytes = Vec::new();
            loop {
                let mut byte = (value & 0x7f) as u8;
                value >>= 7;
                if value != 0 {
                    byte |= 0x80;
                }
                bytes.push(byte);
                if value == 0 {
                    return bytes;
                }
            }
        };
        for field in [0, 0x2000_0000, (1_u64 << 32) + 2] {
            let mut payload = encode(field << 3);
            payload.push(42);
            payload.extend([0x10, 7]);
            assert_eq!(
                varint_field(&payload, 2),
                Err(WireError::InvalidFieldNumber(field))
            );
            assert_eq!(request_id(13, &payload), None);
        }
        // Unknown legal fields still skip safely, including the maximum tag;
        // refusing malformed data must not reject forward-compatible messages.
        let mut payload = encode((0x1fff_ffff_u64 << 3) | 2);
        payload.extend([2, 0xaa, 0xbb, 0x10, 42]);
        assert_eq!(varint_field(&payload, 2), Ok(Some(42)));
        let mut truncated = encode((3_u64 << 3) | 2);
        truncated.extend(encode(u64::MAX));
        assert_eq!(varint_field(&truncated, 2), Err(WireError::TruncatedField));
    }

    #[test]
    fn correlation_reads_request_id_from_the_field_the_schema_declares() {
        // ConnectionMessage (49) declares `request_id = 1` and `bool connected = 2`.
        // Reading field 2 would correlate on the connected flag instead.
        let connection = [0x08, 0x2a, 0x10, 0x01];
        assert_eq!(request_id(49, &connection), Some(42));

        // SceneMessage-style replies declare `request_id = 2`.
        let scene = [0x08, 0x01, 0x10, 0x2a];
        assert_eq!(request_id(13, &scene), Some(42));
    }

    #[test]
    fn every_field_one_message_is_covered_by_the_correlation_table() {
        for (message, message_type) in [
            ("ProductionAutomationModeMessage", 11_u16),
            ("GridMoveMessage", 12),
            ("EnableCaptureOutMessage", 16),
            ("ConfirmationMessage", 47),
            ("ConnectionMessage", 49),
            ("ResetCommsBuffersMessage", 52),
            ("SuspendConnectionMessage", 53),
            ("GenericErrorMessage", 56),
        ] {
            assert_eq!(
                schema_request_id_field(message),
                Some(1),
                "{message} no longer declares request_id = 1"
            );
            assert!(
                REQUEST_ID_FIELD_ONE_TYPES.contains(&message_type),
                "{message} ({message_type}) is missing from the correlation table"
            );
        }
    }
}
