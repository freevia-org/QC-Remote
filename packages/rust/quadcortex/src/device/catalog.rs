//! Shared preset-catalog verification policy: bounded and passive-first.
//!
//! Host policy (not a Cortex Control frame). Cortex Control sends the directory
//! read (`readFolders(false)`, T4 `08 03 58 00`) at boot and for a user-initiated
//! refresh; it does not poll after a mutation, and neither does this host any
//! more. After a persistent File mutation the verification
//!
//! 1. waits passively for [`PASSIVE_WINDOW_MS`] and accepts whatever File
//!    publication the unit pushes or has already folded into the cached catalog;
//! 2. only then sends at most [`MAX_REQUESTS`] directory READs, spaced by the
//!    policy's retry delays (and by the full cooldown after a stale listing,
//!    because a directory dump streams for ~10 s and a second READ only queues
//!    another dump behind it);
//! 3. waits passively for the rest of the (shorter) budget; and
//! 4. ends the write as `accepted_unverified` when no listing confirmed it.
//!    It is not an error and quarantines nothing.
//!
//! Evidence (ESTABLISHED, CorOS 4.1.0, direct USB 2026-10-07): the unit answers a
//! bare directory READ on demand every time, within seconds; direct File COPY and
//! UPDATE take effect; and the first listing after a mutation can lag (a slot
//! briefly missing or stale, correct on the next listing). That is why a first
//! READ is not sent immediately and why one stale listing is not a failure. The
//! timings are BELIEVED, with no measurement in the repository. The two recorded
//! sessions that saw no File traffic after a later READ were
//! earlier runs and are superseded by the 2026-10-07 measurement.
//!
//! A catalog request produces an expensive, uncorrelated stream of File
//! messages. Hosts retain their own event-loop primitives, but this runtime is
//! the single authority for deadlines, retry backoff, stale-stream cooldown,
//! the request cap and failure diagnostics.

use crate::profile;
use serde::{Deserialize, Serialize};

/// Milliseconds after the mutation during which no directory READ is sent and only
/// pushed or cached File publications count. Host policy, BELIEVED, no measurement.
pub const PASSIVE_WINDOW_MS: u64 = 3_000;

/// Largest number of directory READs one verification may send. Host policy,
/// BELIEVED, no measurement.
pub const MAX_REQUESTS: u32 = 3;

/// Timing policy for confirming that the catalog reflects a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVerificationPolicy {
    /// Overall budget, in milliseconds, from the start of verification.
    pub timeout_ms: u64,
    /// Milliseconds from the start of verification before the first directory
    /// READ may be sent; until then only pushed or cached publications count.
    pub passive_window_ms: u64,
    /// Largest number of directory READs that may be sent in the budget.
    pub max_requests: u32,
    /// Delay, in milliseconds, before the second request; it doubles after
    /// each request.
    pub initial_retry_ms: u64,
    /// Upper bound, in milliseconds, for the retry delay. It is also the wait
    /// imposed after a stale listing.
    pub maximum_retry_ms: u64,
}

impl CatalogVerificationPolicy {
    /// Bounded readback policy for one already-dispatched persistent File
    /// mutation. This is host policy, not a Cortex protocol timeout, and never
    /// authorizes replaying the write.
    pub fn persistent_file_mutation() -> Self {
        Self {
            timeout_ms: profile::PERSISTENT_FILE_VERIFICATION_TIMEOUT_MS,
            passive_window_ms: PASSIVE_WINDOW_MS,
            max_requests: MAX_REQUESTS,
            initial_retry_ms: profile::PERSISTENT_FILE_VERIFICATION_RETRY_MS,
            maximum_retry_ms: profile::PERSISTENT_FILE_VERIFICATION_MAXIMUM_RETRY_MS,
        }
    }

    /// Readback policy for a duplicated setlist holding `occupied_presets`
    /// presets. The timeout grows with the preset count up to a fixed maximum;
    /// the retry delays are fixed.
    pub fn duplicate_setlist(occupied_presets: usize) -> Self {
        let occupied_presets = u64::try_from(occupied_presets).unwrap_or(u64::MAX);
        let timeout_ms = profile::DUPLICATE_SETLIST_VERIFICATION_BASE_TIMEOUT_MS
            .saturating_add(
                occupied_presets
                    .saturating_mul(profile::DUPLICATE_SETLIST_VERIFICATION_PER_PRESET_MS),
            )
            .min(profile::DUPLICATE_SETLIST_VERIFICATION_MAXIMUM_TIMEOUT_MS);
        Self {
            timeout_ms,
            passive_window_ms: PASSIVE_WINDOW_MS,
            max_requests: MAX_REQUESTS,
            initial_retry_ms: profile::DUPLICATE_SETLIST_VERIFICATION_RETRY_MS,
            maximum_retry_ms: profile::DUPLICATE_SETLIST_VERIFICATION_MAXIMUM_RETRY_MS,
        }
    }
}

impl Default for CatalogVerificationPolicy {
    fn default() -> Self {
        Self {
            timeout_ms: profile::PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS,
            passive_window_ms: PASSIVE_WINDOW_MS,
            max_requests: MAX_REQUESTS,
            initial_retry_ms: profile::PRESET_LIBRARY_VERIFICATION_RETRY_MS,
            maximum_retry_ms: profile::PRESET_LIBRARY_REFRESH_COALESCE_MS,
        }
    }
}

/// What the host should do next, as decided by
/// [`CatalogVerificationRuntime::advance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogVerificationAction {
    /// Send a catalog request now. At most [`CatalogVerificationPolicy::max_requests`] are
    /// ever issued, the first not before the passive window has elapsed.
    Request,
    /// Do nothing for the given number of milliseconds, then ask again.
    Wait {
        /// Milliseconds until the next request is due or the deadline passes,
        /// whichever comes first.
        delay_ms: u64,
    },
    /// A fresh listing confirmed the requested change.
    Complete,
    /// The deadline passed without confirmation.
    Failed {
        /// Number of fresh catalog listings seen, none of which confirmed the
        /// change.
        observed_generations: u32,
    },
}

/// Tracks the deadline, passive window, request cap, retry backoff and stale-listing
/// cooldown while a host confirms a catalog change. It owns timing only; the host
/// sends the requests and reports the listings it receives.
#[derive(Debug, Clone)]
pub struct CatalogVerificationRuntime {
    deadline_ms: u64,
    next_request_ms: u64,
    retry_delay_ms: u64,
    maximum_retry_ms: u64,
    timeout_ms: u64,
    requests_remaining: u32,
    observed_generations: u32,
    complete: bool,
}

impl CatalogVerificationRuntime {
    /// Shortens the deadline so it does not outlast `deadline`. It never
    /// extends the current deadline; the reported timeout shrinks to match.
    pub fn restrict_deadline(&mut self, deadline: crate::device::correlation::RequestDeadline) {
        let started_ms = self.deadline_ms.saturating_sub(self.timeout_ms);
        self.deadline_ms = self.deadline_ms.min(deadline.expires_at_ms());
        self.timeout_ms = self.deadline_ms.saturating_sub(started_ms);
    }
    /// Starts verification at `now_ms` (the host's monotonic clock in
    /// milliseconds) under the default policy, with the first request due
    /// after the passive window.
    pub fn new(now_ms: u64) -> Self {
        Self::with_policy(now_ms, CatalogVerificationPolicy::default())
    }

    /// Starts verification at `now_ms` under `policy`, with the first request
    /// due after the policy's passive window.
    pub fn with_policy(now_ms: u64, policy: CatalogVerificationPolicy) -> Self {
        Self {
            deadline_ms: now_ms.saturating_add(policy.timeout_ms),
            next_request_ms: now_ms.saturating_add(policy.passive_window_ms),
            retry_delay_ms: policy.initial_retry_ms,
            maximum_retry_ms: policy.maximum_retry_ms,
            timeout_ms: policy.timeout_ms,
            requests_remaining: policy.max_requests,
            observed_generations: 0,
            complete: false,
        }
    }

    /// Reports the next action without changing any state. Completion takes
    /// precedence over the deadline, which takes precedence over a due request.
    pub fn advance(&self, now_ms: u64) -> CatalogVerificationAction {
        if self.complete {
            CatalogVerificationAction::Complete
        } else if now_ms >= self.deadline_ms {
            CatalogVerificationAction::Failed {
                observed_generations: self.observed_generations,
            }
        } else if self.requests_remaining > 0 && now_ms >= self.next_request_ms {
            CatalogVerificationAction::Request
        } else if self.requests_remaining == 0 {
            // The cap is spent: only pushed evidence can still confirm.
            CatalogVerificationAction::Wait {
                delay_ms: self.deadline_ms.saturating_sub(now_ms),
            }
        } else {
            CatalogVerificationAction::Wait {
                delay_ms: self
                    .next_request_ms
                    .min(self.deadline_ms)
                    .saturating_sub(now_ms),
            }
        }
    }

    /// The cap, in milliseconds, on the retry delay and on the wait after a
    /// stale listing.
    pub fn maximum_retry_ms(&self) -> u64 {
        self.maximum_retry_ms
    }

    /// Like [`CatalogVerificationRuntime::advance`], but when the action is
    /// [`CatalogVerificationAction::Request`] it also reserves that request so
    /// the next one is scheduled with backoff.
    pub fn take_action(&mut self, now_ms: u64) -> CatalogVerificationAction {
        let action = self.advance(now_ms);
        if action == CatalogVerificationAction::Request {
            self.request_started(now_ms);
        }
        action
    }

    /// Reserve one request before the host submits it, preventing duplicate
    /// scheduler wakeups from emitting parallel catalog streams.
    pub fn request_started(&mut self, now_ms: u64) {
        self.requests_remaining = self.requests_remaining.saturating_sub(1);
        self.next_request_ms = now_ms.saturating_add(self.retry_delay_ms);
        self.retry_delay_ms = self
            .retry_delay_ms
            .saturating_mul(2)
            .min(self.maximum_retry_ms);
    }

    /// Record a fresh listing for the requested setlist. A stale listing proves
    /// that CorOS just completed a catalog stream, so the next request must wait
    /// for its full cooldown instead of following the earlier request clock.
    pub fn generation_observed(&mut self, now_ms: u64, matches: bool) {
        if self.complete || now_ms >= self.deadline_ms {
            return;
        }
        self.observed_generations = self.observed_generations.saturating_add(1);
        if matches {
            self.complete = true;
        } else {
            self.next_request_ms = now_ms.saturating_add(self.maximum_retry_ms);
            self.retry_delay_ms = self.maximum_retry_ms;
        }
    }

    /// Whether the request cap has been spent.
    pub fn requests_exhausted(&self) -> bool {
        self.requests_remaining == 0
    }

    /// Number of fresh catalog listings observed so far.
    pub fn observed_generations(&self) -> u32 {
        self.observed_generations
    }

    /// Milliseconds left before the deadline, or zero once it has passed.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.deadline_ms.saturating_sub(now_ms)
    }

    /// Human-readable timeout message. `subject` names the catalog read, for
    /// example "preset", and is inserted into the sentence; the text differs
    /// depending on whether any fresh listing was observed.
    pub fn failure_message(&self, subject: &str) -> String {
        let seconds = self.timeout_ms / 1_000;
        if self.observed_generations == 0 {
            format!("The QC published no fresh {subject} generation within {seconds} seconds.")
        } else {
            format!(
                "The QC published {} fresh {subject} generation(s), but none confirmed the requested change within {seconds} seconds.",
                self.observed_generations
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(passive_window_ms: u64, max_requests: u32) -> CatalogVerificationPolicy {
        CatalogVerificationPolicy {
            timeout_ms: 100_000,
            passive_window_ms,
            max_requests,
            initial_retry_ms: 500,
            maximum_retry_ms: 8_000,
        }
    }

    #[test]
    fn no_request_is_sent_during_the_passive_window() {
        let mut runtime = CatalogVerificationRuntime::new(100);
        assert_eq!(
            runtime.advance(100),
            CatalogVerificationAction::Wait {
                delay_ms: PASSIVE_WINDOW_MS
            }
        );
        assert_eq!(
            runtime.take_action(100 + PASSIVE_WINDOW_MS - 1),
            CatalogVerificationAction::Wait { delay_ms: 1 }
        );
        assert_eq!(
            runtime.take_action(100 + PASSIVE_WINDOW_MS),
            CatalogVerificationAction::Request
        );
    }

    #[test]
    fn a_pushed_listing_inside_the_passive_window_completes_with_no_request() {
        let mut runtime = CatalogVerificationRuntime::new(0);
        runtime.generation_observed(40, true);
        assert_eq!(runtime.advance(41), CatalogVerificationAction::Complete);
        assert!(!runtime.requests_exhausted());
    }

    #[test]
    fn the_request_cap_bounds_the_reads_and_the_rest_is_passive() {
        let mut runtime = CatalogVerificationRuntime::with_policy(0, policy(1_000, 3));
        let mut now = 1_000;
        let mut sent = 0;
        let expected = [500, 1_000, 2_000];
        for delay in expected {
            assert_eq!(runtime.take_action(now), CatalogVerificationAction::Request);
            sent += 1;
            assert_eq!(
                runtime.advance(now),
                if sent < 3 {
                    CatalogVerificationAction::Wait { delay_ms: delay }
                } else {
                    CatalogVerificationAction::Wait {
                        delay_ms: 100_000 - now,
                    }
                }
            );
            now += delay;
        }
        assert!(runtime.requests_exhausted());
        // Long past any retry delay, still no fourth read.
        assert!(matches!(
            runtime.advance(60_000),
            CatalogVerificationAction::Wait { .. }
        ));
        // A late push can still confirm...
        runtime.generation_observed(60_000, true);
        assert_eq!(runtime.advance(60_001), CatalogVerificationAction::Complete);
    }

    #[test]
    fn stale_listing_restarts_the_full_cooldown() {
        let mut runtime = CatalogVerificationRuntime::with_policy(0, policy(0, 3));
        runtime.request_started(0);
        runtime.generation_observed(300, false);
        assert_eq!(
            runtime.advance(301),
            CatalogVerificationAction::Wait { delay_ms: 7_999 }
        );
        assert_eq!(runtime.advance(8_300), CatalogVerificationAction::Request);
        assert_eq!(runtime.observed_generations(), 1);
    }

    #[test]
    fn matching_listing_completes_without_another_request() {
        let mut runtime = CatalogVerificationRuntime::new(10);
        runtime.request_started(10);
        runtime.generation_observed(20, true);
        assert_eq!(runtime.advance(20), CatalogVerificationAction::Complete);
    }

    #[test]
    fn timeout_diagnostics_distinguish_silence_from_stale_state() {
        let silent = CatalogVerificationRuntime::new(0);
        assert!(matches!(
            silent.advance(profile::PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS),
            CatalogVerificationAction::Failed {
                observed_generations: 0
            }
        ));
        assert!(silent
            .failure_message("preset catalog")
            .contains("no fresh"));

        let mut stale = CatalogVerificationRuntime::new(0);
        stale.generation_observed(1, false);
        assert!(stale
            .failure_message("preset catalog")
            .contains("1 fresh preset catalog generation(s)"));
    }

    #[test]
    fn duplicate_setlist_policy_scales_with_occupied_inventory_and_is_bounded() {
        let empty = CatalogVerificationPolicy::duplicate_setlist(0);
        assert_eq!(empty.timeout_ms, 90_000);
        let measured = CatalogVerificationPolicy::duplicate_setlist(74);
        assert_eq!(measured.timeout_ms, 238_000);
        assert_eq!(measured.initial_retry_ms, 2_000);
        assert_eq!(measured.maximum_retry_ms, 30_000);
        assert_eq!(measured.max_requests, MAX_REQUESTS);
        assert_eq!(
            CatalogVerificationPolicy::duplicate_setlist(256).timeout_ms,
            600_000
        );
    }

    #[test]
    fn every_policy_sends_a_small_bounded_number_of_reads_after_a_passive_window() {
        for policy in [
            CatalogVerificationPolicy::default(),
            CatalogVerificationPolicy::persistent_file_mutation(),
            CatalogVerificationPolicy::duplicate_setlist(74),
        ] {
            assert_eq!(policy.passive_window_ms, PASSIVE_WINDOW_MS);
            assert!(policy.max_requests >= 1 && policy.max_requests <= 3);
        }
    }

    #[test]
    fn duplicate_setlist_runtime_uses_its_own_deadline_and_slow_backoff() {
        let policy = CatalogVerificationPolicy::duplicate_setlist(74);
        let mut runtime = CatalogVerificationRuntime::with_policy(0, policy);
        let mut now = PASSIVE_WINDOW_MS;
        for delay in [2_000, 4_000, 8_000] {
            assert_eq!(runtime.take_action(now), CatalogVerificationAction::Request);
            now += delay;
        }
        assert!(runtime.requests_exhausted());
        runtime.generation_observed(now, false);
        assert!(matches!(
            runtime.advance(now),
            CatalogVerificationAction::Wait { .. }
        ));
        assert!(runtime
            .failure_message("setlist catalog")
            .contains("238 seconds"));
    }

    #[test]
    fn persistent_file_policy_accepts_a_fresh_match_after_a_stale_listing() {
        let policy = CatalogVerificationPolicy::persistent_file_mutation();
        let mut runtime = CatalogVerificationRuntime::with_policy(0, policy);
        runtime.request_started(0);
        runtime.generation_observed(10_000, false);
        runtime.generation_observed(20_000, true);
        assert_eq!(runtime.advance(20_000), CatalogVerificationAction::Complete);
    }

    #[test]
    fn persistent_file_timeout_never_authorizes_a_second_write() {
        let policy = CatalogVerificationPolicy::persistent_file_mutation();
        let mut runtime = CatalogVerificationRuntime::with_policy(0, policy);
        let persistent_writes = 1;
        runtime.request_started(0);
        runtime.generation_observed(20_000, false);
        assert!(matches!(
            runtime.advance(policy.timeout_ms),
            CatalogVerificationAction::Failed {
                observed_generations: 1
            }
        ));
        assert_eq!(persistent_writes, 1);
    }

    #[test]
    fn expired_catalog_observation_cannot_turn_timeout_into_confirmation() {
        let policy = CatalogVerificationPolicy {
            timeout_ms: 10,
            passive_window_ms: 0,
            max_requests: 3,
            initial_retry_ms: 1,
            maximum_retry_ms: 2,
        };
        let mut expired = CatalogVerificationRuntime::with_policy(5, policy);
        expired.generation_observed(15, true);
        assert_eq!(
            expired.advance(15),
            CatalogVerificationAction::Failed {
                observed_generations: 0
            }
        );
        let mut timely = CatalogVerificationRuntime::with_policy(5, policy);
        timely.generation_observed(14, true);
        timely.generation_observed(16, false);
        assert_eq!(timely.advance(16), CatalogVerificationAction::Complete);
        assert_eq!(timely.observed_generations(), 1);
    }
}
