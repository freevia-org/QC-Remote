//! Bounded, advisory checksum inspection without interpreting or changing backups.
//!
//! The observed local layout omits decoded bytes 873 through 893 from SHA-256.
//! A supplied digest is not authentication of the archive, stamp or metadata,
//! and a match establishes neither archive validity nor restore compatibility.

use super::generated_integrity::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use sha2_010::{Digest, Sha256};

/// Inspect bounded JSON bytes without admitting, refusing, or modifying a save.
pub fn inspect_local_backup_checksum_json(utf8: &[u8]) -> BackupChecksumReportV1 {
    if utf8.len() > MAXIMUM_JSON_BYTES {
        return BackupChecksumReportV1::from_reason(BackupChecksumReason::InspectionLimit);
    }
    match serde_json::from_slice::<Value>(utf8) {
        Ok(document) => inspect_local_backup_checksum(&document),
        Err(_) => BackupChecksumReportV1::from_reason(BackupChecksumReason::UnsupportedEnvelope),
    }
}

/// Compare archive bytes with the untrusted supplied hash under one observed layout.
///
/// All reports are advisory. Existing finalizer and host save admission are unchanged.
/// Base64 is decoded in bounded chunks; no full decoded archive is allocated here.
pub fn inspect_local_backup_checksum(document: &Value) -> BackupChecksumReportV1 {
    use BackupChecksumReason as Reason;
    let report = BackupChecksumReportV1::from_reason;
    if document.get("type").and_then(Value::as_str) != Some("backup")
        || document.get("creator").and_then(Value::as_str) != Some("quad")
    {
        return report(Reason::UnsupportedEnvelope);
    }
    let (Some(payload), Some(digest)) = (
        document.get("payload").and_then(Value::as_str),
        document.get("payload_hash").and_then(Value::as_str),
    ) else {
        return report(Reason::MissingFields);
    };
    if payload.len() > MAXIMUM_ENCODED_PAYLOAD_BYTES {
        return report(Reason::InspectionLimit);
    }
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return report(Reason::InvalidHash);
    }
    let nibble = |byte: u8| {
        if byte <= b'9' {
            byte - b'0'
        } else {
            byte.to_ascii_lowercase() - b'a' + 10
        }
    };
    let mut expected = [0_u8; 32];
    for (index, pair) in digest.as_bytes().chunks_exact(2).enumerate() {
        expected[index] = (nibble(pair[0]) << 4) | nibble(pair[1]);
    }
    if !payload.len().is_multiple_of(4) {
        return report(Reason::InvalidBase64);
    }
    let Some(decoded_bound) = (payload.len() / 4).checked_mul(3) else {
        return report(Reason::InspectionLimit);
    };
    if decoded_bound > MAXIMUM_DECODED_PAYLOAD_BYTES {
        return report(Reason::InspectionLimit);
    }
    let mut local = Sha256::new();
    let mut whole = Sha256::new();
    let mut decoded = [0_u8; ENCODED_CHUNK_BYTES / 4 * 3];
    let boundary = PREFIX_BYTES + STAMP_BYTES;
    let mut position = 0;
    for (index, chunk) in payload.as_bytes().chunks(ENCODED_CHUNK_BYTES).enumerate() {
        let final_chunk = (index + 1) * ENCODED_CHUNK_BYTES >= payload.len();
        if !final_chunk && chunk.contains(&b'=') {
            return report(Reason::InvalidBase64);
        }
        let Ok(length) = STANDARD.decode_slice(chunk, &mut decoded) else {
            return report(Reason::InvalidBase64);
        };
        let bytes = &decoded[..length];
        whole.update(bytes);
        let prefix_end = PREFIX_BYTES.saturating_sub(position).min(length);
        local.update(&bytes[..prefix_end]);
        let suffix_start = boundary.saturating_sub(position).min(length);
        local.update(&bytes[suffix_start..]);
        position += length;
    }
    if position < boundary {
        return report(Reason::TooShort);
    }
    let local_digest: [u8; 32] = local.finalize().into();
    if local_digest == expected {
        return report(Reason::LocalMatch);
    }
    let whole_digest: [u8; 32] = whole.finalize().into();
    report(if whole_digest == expected {
        Reason::AlternateWholePayload
    } else {
        Reason::LocalMismatch
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn document(length: usize) -> Value {
        let archive: Vec<u8> = (0..length).map(|i| i.wrapping_mul(37) as u8).collect();
        let split = PREFIX_BYTES.min(archive.len());
        let payload = [&archive[..split], &[0x53; STAMP_BYTES], &archive[split..]].concat();
        json!({"type":"backup", "creator":"quad", "payload":STANDARD.encode(payload),
            "payload_hash":sha256_hex(archive), "future":{"keep":true}})
    }

    #[test]
    fn bounded_stream_matches_archive_at_boundaries_and_keeps_documents_unchanged() {
        for length in [872, 873, 874, 24_555, 24_556, 24_557, 50_000] {
            let document = document(length);
            let before = serde_json::to_vec(&document).unwrap();
            let report = inspect_local_backup_checksum(&document);
            assert_eq!(
                report.status,
                if length < PREFIX_BYTES {
                    BackupChecksumStatus::UnknownLayout
                } else {
                    BackupChecksumStatus::Verified
                }
            );
            assert_eq!(serde_json::to_vec(&document).unwrap(), before);
            let metadata = serde_json::to_value(report).unwrap();
            assert_eq!(metadata.as_object().unwrap().len(), 10);
            for field in [
                "authenticityVerified",
                "stampIntegrityVerified",
                "metadataIntegrityVerified",
                "restoreCompatibilityVerified",
            ] {
                assert_eq!(metadata[field], false);
            }
            assert!(metadata.get("payload").is_none());
            assert!(metadata.get("payload_hash").is_none());
        }
    }

    #[test]
    fn archive_mismatch_and_stamp_changes_are_distinct_from_authentication() {
        let original = document(1_000);
        for (offset, expected) in [
            (20, BackupChecksumStatus::Mismatch),
            (PREFIX_BYTES, BackupChecksumStatus::Verified),
        ] {
            let mut changed = original.clone();
            let mut bytes = STANDARD
                .decode(changed["payload"].as_str().unwrap())
                .unwrap();
            bytes[offset] ^= 1;
            changed["payload"] = json!(STANDARD.encode(bytes));
            changed["author"] = json!("Invented public author");
            assert_eq!(inspect_local_backup_checksum(&changed).status, expected);
        }
        let mut uppercase = original.clone();
        uppercase["payload_hash"] =
            json!(uppercase["payload_hash"].as_str().unwrap().to_uppercase());
        assert_eq!(
            inspect_local_backup_checksum(&uppercase).status,
            BackupChecksumStatus::Verified
        );
    }

    #[test]
    fn alternate_and_invalid_inputs_remain_unknown_with_content_free_reports() {
        let mut alternate = document(1_000);
        alternate["payload_hash"] = json!(sha256_hex(
            STANDARD
                .decode(alternate["payload"].as_str().unwrap())
                .unwrap()
        ));
        assert_eq!(
            inspect_local_backup_checksum(&alternate).reason,
            BackupChecksumReason::AlternateWholePayload
        );
        for (payload, reason) in [
            ("AA==", BackupChecksumReason::TooShort),
            ("!!!!", BackupChecksumReason::InvalidBase64),
            ("A===", BackupChecksumReason::InvalidBase64),
            ("AAA", BackupChecksumReason::InvalidBase64),
        ] {
            let input = json!({"type":"backup", "creator":"quad", "payload":payload, "payload_hash":"a".repeat(64)});
            let report = inspect_local_backup_checksum(&input);
            assert_eq!(report.reason, reason);
            assert_eq!(report.checked_layout, None);
        }
        assert_eq!(
            inspect_local_backup_checksum_json(b"not json").reason,
            BackupChecksumReason::UnsupportedEnvelope
        );
        assert_eq!(
            inspect_local_backup_checksum(&json!({"type":"backup","creator":"quad"})).reason,
            BackupChecksumReason::MissingFields
        );
        let invalid_hash = json!({"type":"backup","creator":"quad","payload":"AA==","payload_hash":"private-invalid"});
        let report = inspect_local_backup_checksum(&invalid_hash);
        assert_eq!(report.reason, BackupChecksumReason::InvalidHash);
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private-invalid"));
    }

    #[test]
    fn limits_and_nonterminal_padding_are_refused_before_unbounded_decode() {
        let payload = "A".repeat(MAXIMUM_ENCODED_PAYLOAD_BYTES + 1);
        let input = json!({"type":"backup","creator":"quad","payload":payload,"payload_hash":"a".repeat(64)});
        assert_eq!(
            inspect_local_backup_checksum(&input).reason,
            BackupChecksumReason::InspectionLimit
        );
        assert_eq!(
            inspect_local_backup_checksum_json(input["payload"].as_str().unwrap().as_bytes())
                .reason,
            BackupChecksumReason::InspectionLimit
        );
        let mut padded = "A".repeat(ENCODED_CHUNK_BYTES + 4);
        padded.replace_range(ENCODED_CHUNK_BYTES - 2..ENCODED_CHUNK_BYTES, "==");
        let input = json!({"type":"backup","creator":"quad","payload":padded,"payload_hash":"a".repeat(64)});
        assert_eq!(
            inspect_local_backup_checksum(&input).reason,
            BackupChecksumReason::InvalidBase64
        );
    }

    #[test]
    fn canonical_base64_and_envelope_types_are_required_without_leaking_inputs() {
        for payload in ["AB==", "AAB=", "AA==\n", "AA-_", "AA==AA==", "éé"] {
            let input = json!({"type":"backup", "creator":"quad", "payload":payload, "payload_hash":"a".repeat(64)});
            assert_eq!(
                inspect_local_backup_checksum(&input).reason,
                BackupChecksumReason::InvalidBase64,
                "{payload}"
            );
        }
        for input in [
            json!(null),
            json!([]),
            json!({"type":"backup","creator":"cloud"}),
            json!({"type":5,"creator":"quad"}),
        ] {
            assert_eq!(
                inspect_local_backup_checksum(&input).reason,
                BackupChecksumReason::UnsupportedEnvelope
            );
        }
        for input in [
            json!({"type":"backup","creator":"quad","payload":5,"payload_hash":"a".repeat(64)}),
            json!({"type":"backup","creator":"quad","payload":"AA==","payload_hash":5}),
        ] {
            assert_eq!(
                inspect_local_backup_checksum(&input).reason,
                BackupChecksumReason::MissingFields
            );
        }
    }

    #[test]
    fn every_generated_report_has_exact_content_free_shape_and_false_assurances() {
        for reason in [
            BackupChecksumReason::LocalMatch,
            BackupChecksumReason::LocalMismatch,
            BackupChecksumReason::UnsupportedEnvelope,
            BackupChecksumReason::MissingFields,
            BackupChecksumReason::InvalidHash,
            BackupChecksumReason::InvalidBase64,
            BackupChecksumReason::TooShort,
            BackupChecksumReason::InspectionLimit,
            BackupChecksumReason::AlternateWholePayload,
            BackupChecksumReason::InternalUnavailable,
        ] {
            let value = serde_json::to_value(BackupChecksumReportV1::from_reason(reason)).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 10);
            assert_eq!(value["schemaVersion"], 1);
            assert_eq!(value["algorithm"], "sha256");
            for key in [
                "authenticityVerified",
                "stampIntegrityVerified",
                "metadataIntegrityVerified",
                "restoreCompatibilityVerified",
            ] {
                assert_eq!(value[key], false);
            }
            assert!(value["message"].as_str().unwrap().len() <= 256);
            assert!(!value.as_object().unwrap().contains_key("payload"));
            assert!(!value.as_object().unwrap().contains_key("payload_hash"));
        }
    }
}
