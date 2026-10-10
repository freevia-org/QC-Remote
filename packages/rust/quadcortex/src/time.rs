//! Shared wall-clock conversion helpers for host adapters.

use std::time::{SystemTime, UNIX_EPOCH};

fn duration_to_unix_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

/// Returns the current Unix wall-clock time in milliseconds, saturating at the
/// largest representable value if the system clock is beyond the `u64` range.
///
/// A clock before the Unix epoch is represented as zero.
pub fn unix_time_ms() -> u64 {
    duration_to_unix_ms(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn duration_conversion_saturates_at_u64() {
        assert_eq!(super::duration_to_unix_ms(std::time::Duration::ZERO), 0);
        assert_eq!(
            super::duration_to_unix_ms(std::time::Duration::new(u64::MAX, 0)),
            u64::MAX
        );
    }
}
