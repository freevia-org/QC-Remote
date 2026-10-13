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
    /// A field key is zero or exceeds the protobuf 29-bit field-number range.
    #[error("invalid protobuf field tag")]
    InvalidTag,
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

/// Returns the value of the first varint-encoded (wire type 0) field numbered `wanted`, scanning the top level of `payload`.
///
/// Returns `Ok(None)` if there is no such field. Fields of other wire types are skipped; an error is returned if
/// the scan meets malformed data before finding the field.
pub fn varint_field(payload: &[u8], wanted: u32) -> Result<Option<u64>, WireError> {
    let mut offset = 0;
    while offset < payload.len() {
        let tag = read_varint(payload, &mut offset)?;
        let field = tag >> 3;
        if field == 0 || field > (1 << 29) - 1 {
            return Err(WireError::InvalidTag);
        }
        let wire = (tag & 7) as u8;
        if field == u64::from(wanted) && wire == 0 {
            return Ok(Some(read_varint(payload, &mut offset)?));
        }
        skip(payload, &mut offset, wire)?;
    }
    Ok(None)
}

fn read_varint(payload: &[u8], offset: &mut usize) -> Result<u64, WireError> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let Some(byte) = payload.get(*offset).copied() else {
            return Err(WireError::TruncatedVarint);
        };
        *offset += 1;
        // Only bit 63 remains in the tenth octet. Reject overflow before
        // shifting, otherwise a malformed value can alias a valid request id.
        if shift == 63 && byte > 1 {
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
    fn rejects_overflowing_varints_without_truncating_their_high_bits() {
        for last in [2, 0x7f, 0x80, 0xff] {
            let mut payload = vec![0x10, 0xaa];
            payload.extend_from_slice(&[0x80; 8]);
            payload.push(last);
            assert_eq!(varint_field(&payload, 2), Err(WireError::TruncatedVarint));
            assert_eq!(request_id(13, &payload), None);
        }
        let mut maximum = vec![0x10];
        maximum.extend_from_slice(&[0xff; 9]);
        maximum.push(1);
        assert_eq!(varint_field(&maximum, 2), Ok(Some(u64::MAX)));
    }

    #[test]
    fn rejects_invalid_field_keys_before_narrowing() {
        // Field 2^32 + 2 previously narrowed to field 2.
        let oversized = [0x90, 0x80, 0x80, 0x80, 0x80, 0x01, 42];
        assert_eq!(varint_field(&oversized, 2), Err(WireError::InvalidTag));
        assert_eq!(
            varint_field(&[0, 0, 0x10, 42], 2),
            Err(WireError::InvalidTag)
        );
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
