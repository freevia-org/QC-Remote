//! Feeding the relay's replies to the device from the broker's device loop,
//! and keeping the session alive while it does.
//!
//! A reply stream occupies the device lane for minutes. The unit drops a
//! session that stops hearing the dedicated KeepAlive, and the ordinary idle
//! keepalive is deferred by traffic and stands down during a transfer, so this
//! module owns an unconditional keepalive clock for exactly as long as the
//! relay is active, the same arrangement a running device backup has.

use super::UpdaterRelay;
use quadcortex::commands::{self, OutboundMessage};
use quadcortex::profile;
use std::time::{Duration, Instant};

/// A clock that fires on a fixed interval from when it was started. It does
/// not drift forward by the time a send took, and after a long stall it fires
/// once and restarts from there, never in a burst.
#[derive(Debug)]
pub struct FixedClock {
    interval: Duration,
    next: Option<Instant>,
}

impl FixedClock {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            next: None,
        }
    }

    /// Start the clock with the first firing due at once.
    pub fn start(&mut self, now: Instant) {
        if self.next.is_none() {
            self.next = Some(now);
        }
    }

    pub fn stop(&mut self) {
        self.next = None;
    }

    pub fn is_running(&self) -> bool {
        self.next.is_some()
    }

    pub fn due(&self, now: Instant) -> bool {
        self.next.is_some_and(|next| now >= next)
    }

    /// Record a firing at `now`.
    pub fn fired(&mut self, now: Instant) {
        let scheduled = self.next.unwrap_or(now);
        let following = scheduled + self.interval;
        self.next = Some(if now < following {
            following
        } else {
            now + self.interval
        });
    }
}

/// Where the pump's frames go: the connected unit, or a fake in tests.
pub trait RelaySink {
    fn send(&mut self, message: OutboundMessage) -> Result<(), String>;
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PumpReport {
    pub keepalives: u32,
    pub frames: u32,
    /// The first write that failed. The session cannot be trusted after one.
    pub write_error: Option<String>,
}

/// How many inbound reads the device loop makes between two reply frames while
/// inbound traffic is waiting.
///
/// The loop reads one HID event per pass, and a reply frame's write blocks the
/// pass for as long as the one-frame HID queue is full: about 156 ms for a
/// 32 KiB chunk (261 reports, 6 ms of pacing per 10). Writing a frame on every
/// pass therefore left the loop reading about six events a second for the
/// whole transfer, far fewer than the unit sends (idle markers, tempo state,
/// the unit's own type-60 progress and its FAILED), so inbound ran minutes
/// behind and the receive queue could overflow. With this many reads owed
/// after each frame, up to this many waiting events are read before the next
/// frame is written.
///
/// The quota is for a backlog only. A read that finds nothing waiting (a
/// timed-out poll, or the reader's idle marker, which it queues after about
/// 200 ms of silence and which reports that arrived after it can wait behind)
/// clears what is owed, so on a quiet link the next frame goes out at once and
/// the write rate, not the poll timer, sets the pace: a 5 ms poll can really take 15.6 ms on Windows,
/// and 24 of those would have been 375 ms per chunk. A read that returned a
/// report which only completed part of a frame is not empty: more of that
/// frame is arriving, so it counts against the quota like any other event.
/// Because an idle marker can clear the debt while a few reports still wait
/// behind it, a frame may follow fewer than this many reads; the bound is still
/// at most this many reads per frame, and idle markers come at most five a
/// second. The keepalive is not gated by any of this.
pub const READS_PER_FRAME: u32 = 24;

pub struct RelayPump {
    keepalive: FixedClock,
    /// Reads still owed before the next reply frame may be written.
    reads_owed: u32,
}

impl Default for RelayPump {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayPump {
    pub fn new() -> Self {
        Self {
            keepalive: FixedClock::new(Duration::from_millis(profile::KEEPALIVE_INTERVAL_MS)),
            reads_owed: 0,
        }
    }

    /// Whether this pump's clock is running, which is the device loop's cue to
    /// leave the idle keepalive alone.
    pub fn is_running(&self) -> bool {
        self.keepalive.is_running()
    }

    /// The device loop calls this once for every inbound read it makes.
    /// `queue_was_empty` is true when the read found nothing waiting (see
    /// `QcUsb::last_read_was_empty`): that ends the debt, since there is no
    /// backlog left to read. Otherwise the read counts against the quota.
    pub fn note_read(&mut self, queue_was_empty: bool) {
        if queue_was_empty {
            self.reads_owed = 0;
        } else {
            self.reads_owed = self.reads_owed.saturating_sub(1);
        }
    }

    /// Whether the next reply frame is being held back for inbound reads.
    pub fn awaiting_reads(&self) -> bool {
        self.reads_owed > 0
    }

    /// One pass: send the keepalive if it is due, then at most one reply
    /// frame, and only once [`READS_PER_FRAME`] reads have followed the last
    /// one. At most one frame per pass keeps a long write from starving the
    /// clock, and the read quota keeps the frames from starving the reads; the
    /// loop calls this every few milliseconds.
    pub fn tick(
        &mut self,
        relay: &UpdaterRelay,
        now: Instant,
        sink: &mut dyn RelaySink,
    ) -> PumpReport {
        let mut report = PumpReport::default();
        relay.maintain(now);
        if !relay.is_active() {
            self.keepalive.stop();
            self.reads_owed = 0;
            return report;
        }
        self.keepalive.start(now);
        if self.keepalive.due(now) {
            match sink.send(commands::keepalive()) {
                Ok(()) => {
                    report.keepalives += 1;
                    self.keepalive.fired(now);
                }
                Err(error) => {
                    report.write_error = Some(error);
                    return report;
                }
            }
        }
        if self.reads_owed > 0 {
            return report;
        }
        if let Some(frame) = relay.poll_outbound() {
            match sink.send(frame) {
                Ok(()) => {
                    report.frames += 1;
                    self.reads_owed = READS_PER_FRAME;
                }
                Err(error) => report.write_error = Some(error),
            }
        }
        report
    }
}
