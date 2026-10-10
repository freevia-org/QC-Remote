//! Reaps a `--stdio` broker whose spawning parent has died.
//!
//! The Windows GUI app no longer reaches `--stdio` in practice - its own
//! `GatewayProcess::start_owned_child` is kept unreachable by an infallible
//! descriptor-directory fallback in `apps/windows/src-tauri/src/lib.rs`'s
//! Tauri `.setup()` (see that function's own doc comment). This module's
//! sole current production caller spawns `qc-device-broker.exe --stdio`
//! directly (searching for that binary specifically - not `qc-remote.exe`,
//! the Tauri GUI app) and depends on this watchdog for the same reaping
//! guarantee - do not remove this module on the assumption that `--stdio`
//! is now dead code; it is dead only for the Windows GUI's own hosting
//! path.
//!
//! `serve_stdio` blocks synchronously in `read_request` waiting on stdin. If
//! the parent that piped that stdin dies while any other process still holds
//! a handle to the write end (a common shape on Windows, where a shell or a
//! debugger can keep a pipe open past the process that created it), the pipe
//! never closes and the broker waits forever, holding the exclusive USB
//! handle.
//!
//! The reaping predicate is the parent's liveness, never the image name:
//! every paired host's STDIO bridge runs as the same `qc-remote.exe
//! --local-mcp-bridge`, so an image-name sweep would break every pairing on
//! the machine. A watchdog thread polls the parent's liveness independently
//! of the blocked read, and only exits once the parent is gone AND no
//! request has arrived for `ORPHAN_IDLE_GRACE` - a brief gap alone is not
//! proof of anything, since this process is asked for its own parent id only
//! once, at startup, and a parent recorded as gone stays gone - by creation
//! time, not by pid alone: Windows reuses pids, and the watchdog also
//! records the parent's creation timestamp at startup, so a pid later
//! reassigned to an unrelated process reads as gone rather than as the same
//! parent still running.
//!
//! `ORPHAN_IDLE_GRACE` alone is not enough: `create_backup` and
//! `device.reconnect` legitimately hold a single request for well past 30s
//! (up to ~215s and ~35s respectively), and a clock that only records when a
//! request *starts* would let the watchdog `std::process::exit` mid-request
//! once its start time ages past the grace - abandoning a write whose
//! outcome is genuinely unknown, with no chance for `rpc.rs`'s own ambiguous-
//! write quarantine to run before the process is gone. `RequestClock`
//! therefore also tracks whether a request is currently in flight, and the
//! watchdog treats "in flight" as never idle, no matter how long the request
//! has been running.

#[cfg(windows)]
use qc_windows_process::os_parent_process_id;
use qc_windows_process::{os_process_created_at, os_process_is_alive};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// How often the watchdog checks the parent's liveness.
const ORPHAN_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// How long this broker must have gone unrequested, on top of the parent
/// being gone, before it reaps itself. Long enough that a request already in
/// flight when the parent exits still gets answered; short enough that a
/// genuine orphan is not left holding the device for hours.
const ORPHAN_IDLE_GRACE: Duration = Duration::from_secs(30);

fn now_unix_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

/// Tracks when the last request started or finished, and whether one is
/// currently in flight. The watchdog will not reap this process while a
/// request is in flight, however long it runs, and otherwise measures
/// idleness from whichever of "started" or "finished" happened last.
pub struct RequestClock {
    at: AtomicU64,
    in_flight: AtomicBool,
}

impl RequestClock {
    pub fn started_now() -> Self {
        Self {
            at: AtomicU64::new(now_unix_ms()),
            in_flight: AtomicBool::new(false),
        }
    }

    /// Call when a request is read off the transport, before it is handled.
    pub fn start_request(&self) {
        self.at.store(now_unix_ms(), Ordering::Release);
        self.in_flight.store(true, Ordering::Release);
    }

    /// Call once that request's handler has returned, successfully or not.
    pub fn finish_request(&self) {
        self.at.store(now_unix_ms(), Ordering::Release);
        self.in_flight.store(false, Ordering::Release);
    }

    fn idle_for(&self) -> Duration {
        Duration::from_millis(now_unix_ms().saturating_sub(self.at.load(Ordering::Acquire)))
    }

    fn is_idle(&self) -> bool {
        !self.in_flight.load(Ordering::Acquire) && self.idle_for() >= ORPHAN_IDLE_GRACE
    }
}

/// Spawn the watchdog. `parent_pid` is `None` when the parent could not be
/// determined at startup (an unsupported platform, or the lookup itself
/// failed) - the watchdog then has no liveness signal to act on and never
/// exits the process, matching the conservative "err toward alive" stance
/// used everywhere else a leftover process is judged.
///
/// `before_exit` runs once, on the watchdog thread, right before
/// `process::exit`: `process::exit` runs no destructors, so this is where the
/// device gets released (the caller stops its USB worker, bounded).
pub fn watch(
    parent_pid: Option<u32>,
    clock: std::sync::Arc<RequestClock>,
    before_exit: impl FnOnce() + Send + 'static,
) {
    let Some(parent_pid) = parent_pid else {
        return;
    };
    // Recorded once, alongside the pid: if a later poll finds a process at
    // this same pid with a *different* creation time, the OS has reassigned
    // the pid to an unrelated process and the real parent is gone, even
    // though `os_process_is_alive` alone would read it as running.
    let parent_created_at = os_process_created_at(parent_pid);
    std::thread::Builder::new()
        .name("qc-native-orphan-guard".into())
        .spawn(move || {
            let mut before_exit = Some(before_exit);
            loop {
                std::thread::sleep(ORPHAN_CHECK_INTERVAL);
                if os_process_is_alive(parent_pid)
                    && !parent_pid_was_reused(parent_pid, parent_created_at)
                {
                    continue;
                }
                if !clock.is_idle() {
                    continue;
                }
                eprintln!(
                    "QC device broker's parent process {parent_pid} is gone and no request has \
                 arrived in {:?}; exiting rather than holding the device as a leftover.",
                    clock.idle_for()
                );
                if let Some(before_exit) = before_exit.take() {
                    before_exit();
                }
                std::process::exit(0);
            }
        })
        .expect("orphan guard thread spawns");
}

/// The current process's parent id, or `None` if it cannot be determined.
#[cfg(windows)]
pub fn parent_process_id() -> Option<u32> {
    os_parent_process_id()
}

#[cfg(not(windows))]
pub fn parent_process_id() -> Option<u32> {
    None
}

// Process liveness/creation-time checks are `qc_windows_process::{
// os_process_is_alive, os_process_created_at}` - shared with
// `apps/windows/src-tauri`, which needs the identical logic for its own
// broker-liveness judgment. This module used to carry its own copy of both;
// the two had already drifted cosmetically despite identical logic, exactly
// the risk a real behavioral fix applied to only one copy would realize.

/// Whether `pid` now names a different process than the one whose creation
/// time was recorded at startup - i.e. the pid was reused.
///
/// Errs towards "not reused": if either timestamp could not be read, this
/// returns `false`, the same conservative stance `os_process_is_alive` takes
/// - an unreadable creation time must never manufacture a "gone" verdict.
fn parent_pid_was_reused(pid: u32, recorded_created_at: Option<u64>) -> bool {
    let (Some(recorded), Some(current)) = (recorded_created_at, os_process_created_at(pid)) else {
        return false;
    };
    recorded != current
}

#[cfg(test)]
mod tests {
    use super::*;

    // `os_process_is_alive`/`os_process_created_at` themselves are tested in
    // `qc-windows-process`, where they're now defined. This module only owns
    // `parent_process_id` and the orphan-reaping logic built on top of them.

    #[cfg(windows)]
    #[test]
    fn this_process_can_find_its_own_parent() {
        // Whatever launched `cargo test` is still running while the test
        // runs, so a `None` here would mean the lookup itself is broken.
        assert!(parent_process_id().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn a_still_running_process_is_never_read_as_reused() {
        let pid = std::process::id();
        let recorded = os_process_created_at(pid);
        assert!(!parent_pid_was_reused(pid, recorded));
    }

    #[test]
    fn an_unreadable_recorded_time_is_never_read_as_reused() {
        // Errs towards "not reused", the same conservative stance
        // `os_process_is_alive` takes: an unreadable timestamp must never
        // manufacture a "gone" verdict on its own.
        assert!(!parent_pid_was_reused(std::process::id(), None));
    }

    /// The positive case: both prior tests only ever assert `false`, so a
    /// `parent_pid_was_reused` hardcoded to always return `false` would pass
    /// them both, silently disabling the pid-reuse guard. A recorded time
    /// that provably isn't this process's real one (its real creation time
    /// is always a valid, non-zero FILETIME) must read as reused.
    #[cfg(windows)]
    #[test]
    fn a_recorded_time_that_does_not_match_the_current_one_is_read_as_reused() {
        let pid = std::process::id();
        let fabricated_recorded_at = Some(0);
        assert_ne!(
            os_process_created_at(pid),
            fabricated_recorded_at,
            "a real process's creation time is never the fabricated sentinel"
        );
        assert!(parent_pid_was_reused(pid, fabricated_recorded_at));
    }

    #[test]
    fn a_fresh_clock_is_not_idle() {
        let clock = RequestClock::started_now();
        assert!(clock.idle_for() < Duration::from_secs(1));
    }

    #[test]
    fn finishing_a_request_resets_idle_time() {
        let clock = RequestClock::started_now();
        std::thread::sleep(Duration::from_millis(50));
        clock.finish_request();
        assert!(clock.idle_for() < Duration::from_millis(50));
    }

    /// The bug this guards against: `create_backup` and `device.reconnect`
    /// legitimately run past `ORPHAN_IDLE_GRACE`. A clock that only tracked
    /// "when did the last request start" would read as idle - and so
    /// reapable - while one of those was still genuinely in progress.
    #[test]
    fn a_request_in_flight_is_never_idle_however_long_it_runs() {
        let clock = RequestClock::started_now();
        clock.start_request();
        // Simulate the clock having started long enough ago that a naive
        // "time since start" check would read as idle, without an actual
        // multi-second sleep.
        clock.at.store(
            now_unix_ms().saturating_sub(ORPHAN_IDLE_GRACE.as_millis() as u64 + 1000),
            Ordering::Release,
        );
        assert!(
            !clock.is_idle(),
            "a request still in flight must never read as idle"
        );
    }

    #[test]
    fn a_request_reads_idle_only_after_it_finishes_and_the_grace_elapses() {
        let clock = RequestClock::started_now();
        clock.start_request();
        clock.finish_request();
        assert!(
            !clock.is_idle(),
            "just-finished is not idle yet; the grace period has not elapsed"
        );
        clock.at.store(
            now_unix_ms().saturating_sub(ORPHAN_IDLE_GRACE.as_millis() as u64 + 1),
            Ordering::Release,
        );
        assert!(clock.is_idle());
    }
}
