//! The total bound on a latest-message cache: the per-type store that keeps
//! the most recent inbound message of each message type.
//!
//! Each entry was already individually bounded (by the inflation ceiling, and
//! in the broker by the largest payload one IPC frame can return), but the
//! store as a whole was not: the key is the envelope's 16-bit type, which the
//! device chooses, so a unit or an impostor with the QC's USB IDs could fill
//! up to 65,536 keys with maximal payloads. This module bounds both the
//! number of keys and the total payload bytes.
//!
//! These bounds are measured envelopes with headroom, not protocol maxima: no
//! device or Cortex Control limit exists for them.
//!
//! How the bounds were chosen, from the 2026-09-28 lossless inbound HID taps
//! of a CorOS 4.1.0 unit (four sessions, 3,335 frames, 36 distinct types):
//! the largest latest-message set any session accumulated was 27 types
//! holding 194,672 payload bytes (the session reset and boot). The unit
//! declares 72 message types (`quadcortex::profile`).
//!
//! - [`LATEST_MESSAGE_CACHE_MAX_ENTRIES`] = 128: every declared type plus 56
//!   for types a later firmware adds, 3.5x the 36 types ever observed.
//! - [`LATEST_MESSAGE_CACHE_MAX_BYTES`] = 32 MiB: 172x the observed 194,672
//!   bytes, and room for two of the largest payloads any entry can legitimately
//!   hold (the 16 MiB shared inflation ceiling, which the model repository,
//!   never captured in full, may approach). It matches the broker's raw event
//!   history byte budget.
//!
//! Over either bound the incoming message is not cached, any older entry of
//! its type is removed (it is no longer the latest publication, and leaving it
//! would answer later reads with superseded state), the caller is told why,
//! and nothing else in the cache is evicted: one hostile type cannot push out
//! the settings or preset another reader depends on.

use crate::usb::IncomingMessage;
use std::collections::HashMap;

/// Most distinct message types a latest-message cache holds.
pub const LATEST_MESSAGE_CACHE_MAX_ENTRIES: usize = 128;

/// Most payload bytes a latest-message cache holds across every entry.
pub const LATEST_MESSAGE_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Cloud envelopes can carry credentials, signed URLs, and account responses.
/// They belong to the forwarding exchange, never to a reusable state cache.
pub const fn is_private_cloud_message(message_type: u16) -> bool {
    matches!(message_type, 18 | 29 | 30 | 31 | 41 | 58 | 61)
}
// The bounds cover every declared type and the observed traffic.
const _: () = assert!(LATEST_MESSAGE_CACHE_MAX_ENTRIES > 72);
const _: () =
    assert!(LATEST_MESSAGE_CACHE_MAX_BYTES >= 2 * quadcortex::profile::MAX_INFLATED_BYTES);
const _: () = assert!(LATEST_MESSAGE_CACHE_MAX_BYTES > 100 * 194_672);

/// Why a message was not cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatestCacheRejection {
    /// Caching a new type would exceed [`LATEST_MESSAGE_CACHE_MAX_ENTRIES`].
    Entries {
        /// Entries already held.
        held: usize,
    },
    /// Caching the payload would exceed [`LATEST_MESSAGE_CACHE_MAX_BYTES`].
    Bytes {
        /// Bytes the cache would have held with this message, net of the
        /// entry of the same type it would have replaced.
        would_hold: usize,
    },
}

impl std::fmt::Display for LatestCacheRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entries { held } => write!(
                formatter,
                "latest-message cache already holds {held} types (limit {LATEST_MESSAGE_CACHE_MAX_ENTRIES})"
            ),
            Self::Bytes { would_hold } => write!(
                formatter,
                "latest-message cache would hold {would_hold} bytes (limit {LATEST_MESSAGE_CACHE_MAX_BYTES})"
            ),
        }
    }
}

/// Total payload bytes held by `latest`.
pub fn latest_cache_bytes(latest: &HashMap<u16, IncomingMessage>) -> usize {
    latest.values().map(|message| message.payload.len()).sum()
}

/// Whether `incoming` fits `latest` under both bounds, replacing any entry of
/// its own type. Does not modify the cache.
pub fn check_latest_message(
    latest: &HashMap<u16, IncomingMessage>,
    incoming: &IncomingMessage,
) -> Result<(), LatestCacheRejection> {
    check_latest_message_within(
        latest,
        incoming,
        LATEST_MESSAGE_CACHE_MAX_ENTRIES,
        LATEST_MESSAGE_CACHE_MAX_BYTES,
    )
}

fn check_latest_message_within(
    latest: &HashMap<u16, IncomingMessage>,
    incoming: &IncomingMessage,
    max_entries: usize,
    max_bytes: usize,
) -> Result<(), LatestCacheRejection> {
    let replaced = latest.get(&incoming.message_type);
    if replaced.is_none() && latest.len() >= max_entries {
        return Err(LatestCacheRejection::Entries { held: latest.len() });
    }
    let would_hold = latest_cache_bytes(latest)
        - replaced.map_or(0, |previous| previous.payload.len())
        + incoming.payload.len();
    if would_hold > max_bytes {
        return Err(LatestCacheRejection::Bytes { would_hold });
    }
    Ok(())
}

/// Caches `incoming` as its type's latest message when it fits both bounds.
/// On rejection the older entry of that type is removed and nothing else is
/// touched; the caller logs and counts the returned reason.
pub fn insert_latest_message(
    latest: &mut HashMap<u16, IncomingMessage>,
    mut incoming: IncomingMessage,
) -> Result<(), LatestCacheRejection> {
    if is_private_cloud_message(incoming.message_type) {
        if let Some(mut previous) = latest.remove(&incoming.message_type) {
            previous.payload.fill(0);
        }
        incoming.payload.fill(0);
        return Ok(());
    }
    insert_latest_message_within(
        latest,
        incoming,
        LATEST_MESSAGE_CACHE_MAX_ENTRIES,
        LATEST_MESSAGE_CACHE_MAX_BYTES,
    )
}

fn insert_latest_message_within(
    latest: &mut HashMap<u16, IncomingMessage>,
    incoming: IncomingMessage,
    max_entries: usize,
    max_bytes: usize,
) -> Result<(), LatestCacheRejection> {
    match check_latest_message_within(latest, &incoming, max_entries, max_bytes) {
        Ok(()) => {
            latest.insert(incoming.message_type, incoming);
            Ok(())
        }
        Err(rejection) => {
            latest.remove(&incoming.message_type);
            Err(rejection)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_credentials_and_signed_urls_never_enter_the_latest_cache() {
        let mut latest = HashMap::new();
        for kind in [18, 29, 30, 31, 41, 58, 61] {
            latest.insert(kind, message(kind, 32));
            assert_eq!(
                insert_latest_message(&mut latest, message(kind, 64)),
                Ok(())
            );
            assert!(!latest.contains_key(&kind));
        }
        assert_eq!(insert_latest_message(&mut latest, message(60, 8)), Ok(()));
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[&60].payload.len(), 8);
    }
    fn message(message_type: u16, bytes: usize) -> IncomingMessage {
        IncomingMessage {
            observation: None,
            sequence: 0,
            message_type,
            payload: vec![0; bytes],
            received_at_unix_ms: 0,
        }
    }

    fn filled(types: u16, bytes_each: usize) -> HashMap<u16, IncomingMessage> {
        let mut latest = HashMap::new();
        for message_type in 0..types {
            latest.insert(message_type, message(message_type, bytes_each));
        }
        latest
    }

    #[test]
    fn entry_count_admits_up_to_the_limit_and_rejects_one_more_type() {
        let limit = LATEST_MESSAGE_CACHE_MAX_ENTRIES;
        // Just below: one slot left, a new type fits.
        let mut latest = filled(limit as u16 - 1, 1);
        assert_eq!(insert_latest_message(&mut latest, message(1000, 1)), Ok(()));
        // At the limit: a new type is refused, the cache is unchanged.
        assert_eq!(latest.len(), limit);
        assert_eq!(
            insert_latest_message(&mut latest, message(1001, 1)),
            Err(LatestCacheRejection::Entries { held: limit })
        );
        assert_eq!(latest.len(), limit);
        assert!(!latest.contains_key(&1001));
        // At the limit a type already held may still be replaced.
        assert_eq!(insert_latest_message(&mut latest, message(1000, 2)), Ok(()));
        assert_eq!(latest[&1000].payload.len(), 2);
    }

    #[test]
    fn total_bytes_admit_up_to_the_limit_and_reject_one_byte_more() {
        let max = 1_000;
        let mut latest = filled(3, 300);
        // Just below the limit after insert: 900 + 99.
        assert_eq!(
            insert_latest_message_within(&mut latest, message(10, 99), 8, max),
            Ok(())
        );
        // Exactly at the limit: replacing type 10 with 100 bytes holds 1,000.
        assert_eq!(
            insert_latest_message_within(&mut latest, message(10, 100), 8, max),
            Ok(())
        );
        assert_eq!(latest_cache_bytes(&latest), max);
        // One byte over: refused, and the superseded type-10 entry is gone.
        assert_eq!(
            insert_latest_message_within(&mut latest, message(10, 101), 8, max),
            Err(LatestCacheRejection::Bytes { would_hold: 1_001 })
        );
        assert!(!latest.contains_key(&10));
        // Nothing else was evicted.
        assert_eq!(latest.len(), 3);
        assert_eq!(latest_cache_bytes(&latest), 900);
    }

    #[test]
    fn the_production_byte_limit_holds_at_and_rejects_past_its_boundary() {
        let half = LATEST_MESSAGE_CACHE_MAX_BYTES / 2;
        let mut latest = HashMap::new();
        assert_eq!(insert_latest_message(&mut latest, message(1, half)), Ok(()));
        assert_eq!(
            insert_latest_message(&mut latest, message(2, half - 1)),
            Ok(())
        );
        assert_eq!(insert_latest_message(&mut latest, message(2, half)), Ok(()));
        assert_eq!(latest_cache_bytes(&latest), LATEST_MESSAGE_CACHE_MAX_BYTES);
        assert_eq!(
            insert_latest_message(&mut latest, message(3, 1)),
            Err(LatestCacheRejection::Bytes {
                would_hold: LATEST_MESSAGE_CACHE_MAX_BYTES + 1
            })
        );
        assert_eq!(latest.len(), 2);
    }

    #[test]
    fn rejection_text_names_the_bound() {
        assert!(LatestCacheRejection::Entries { held: 128 }
            .to_string()
            .contains("limit 128"));
        assert!(LatestCacheRejection::Bytes { would_hold: 5 }
            .to_string()
            .contains(&LATEST_MESSAGE_CACHE_MAX_BYTES.to_string()));
    }
}
