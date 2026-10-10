//! Small, dependency-free mechanics shared across otherwise unrelated parts
//! of this crate (and, for `RecoverPoison`, other crates in the workspace).
//! Nothing here carries QC domain knowledge; it exists to give those bits of
//! plumbing one home instead of letting each caller reinvent them.

use std::sync::{Mutex, MutexGuard};

/// Shared by `quadcortex-windows` and `device-broker` (both hold long-lived
/// `Mutex`-protected caches on background threads), so it lives here rather
/// than in either one, to avoid either depending on the other for it.
pub trait RecoverPoison<T> {
    /// Locks the mutex, returning the guard even if a previous holder panicked and poisoned it.
    ///
    /// The value is returned as left by the panicking holder rather than propagating the poison.
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> RecoverPoison<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        // A panic while updating one cache must not permanently crash both
        // broker workers and every future request. The protected structures
        // are replaced or updated atomically enough for their next event to
        // reconcile them, so retaining the inner value is safer than cascading.
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Compute the next value of a saturating `u64` identity counter, failing
/// with `"{label} identity exhausted"` once incrementing would overflow --
/// or, when `max` is less than `u64::MAX`, once the incremented value would
/// exceed `max`. Pass `u64::MAX` for an uncapped counter.
///
/// This only computes and validates the next value; it never touches a
/// counter field itself. A couple of `device::owner` call sites must defer
/// committing the new value until a later fallible step succeeds (so a
/// failed operation does not burn an identity), so the commit is left to
/// the caller rather than baked in here. Callers that always commit
/// immediately can use [`allocate_next_id`] instead.
pub(crate) fn next_identity(current: u64, max: u64, label: &str) -> Result<u64, String> {
    current
        .checked_add(1)
        .filter(|next| *next <= max)
        .ok_or_else(|| format!("{label} identity exhausted"))
}

/// Allocate the next id from an uncapped `u64` counter: read the current
/// value, commit `current + 1` back into `counter` immediately, and return
/// the id that was just consumed. This is the shape of most of this
/// crate's identity counters, which hand out `counter`'s old value as the
/// new id and never need to defer the commit.
pub(crate) fn allocate_next_id(counter: &mut u64, label: &str) -> Result<u64, String> {
    let id = *counter;
    *counter = next_identity(id, u64::MAX, label)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_identity_increments_and_rejects_overflow() {
        assert_eq!(next_identity(0, u64::MAX, "thing"), Ok(1));
        assert_eq!(next_identity(41, u64::MAX, "thing"), Ok(42));
        assert_eq!(
            next_identity(u64::MAX, u64::MAX, "thing"),
            Err("thing identity exhausted".to_string())
        );
    }

    #[test]
    fn next_identity_enforces_an_explicit_cap_below_u64_max() {
        let cap = i64::MAX as u64;
        assert_eq!(next_identity(cap - 1, cap, "capped"), Ok(cap));
        assert_eq!(
            next_identity(cap, cap, "capped"),
            Err("capped identity exhausted".to_string())
        );
    }

    #[test]
    fn allocate_next_id_returns_the_old_value_and_commits_the_increment() {
        let mut counter = 5;
        assert_eq!(allocate_next_id(&mut counter, "thing"), Ok(5));
        assert_eq!(counter, 6);
        assert_eq!(allocate_next_id(&mut counter, "thing"), Ok(6));
        assert_eq!(counter, 7);
    }

    #[test]
    fn allocate_next_id_rejects_overflow_without_advancing_the_counter() {
        let mut counter = u64::MAX;
        assert_eq!(
            allocate_next_id(&mut counter, "thing"),
            Err("thing identity exhausted".to_string())
        );
        assert_eq!(counter, u64::MAX);
    }
}
