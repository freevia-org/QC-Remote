//! Byte budgets for the broker's subscriber queues, and the counters that make
//! every retention drop visible in the broker status.
//!
//! A subscriber queue used to be bounded only by entry count
//! (`STATE_EVENT_DEFAULT_LIMIT`, 256). With raw frames of up to the 16 MiB
//! inflation ceiling that is up to 4 GiB held for one stalled subscriber, and
//! a slow TCP client whose push thread blocks on its socket is exactly such a
//! subscriber. Each queue now also carries a byte budget. Like the inflation
//! limits these are measured envelopes with headroom, not protocol maxima:
//!
//! - [`RAW_SUBSCRIBER_QUEUE_MAX_BYTES`] (32 MiB) is twice the largest payload
//!   the transport can deliver (`profile::MAX_INFLATED_BYTES`), so an empty
//!   queue always admits one maximal frame and a waiter may fall a full
//!   maximal frame behind before anything is dropped. The 2026-09-28 HID taps
//!   put the busiest whole session (a device backup) at 2,060,784 bytes of
//!   inflated payload and the boot's burst of 586 directory listings at
//!   1,380,101 bytes: a subscriber that read none of either would still sit
//!   under a sixteenth of the budget.
//! - [`STATE_SUBSCRIBER_QUEUE_MAX_BYTES`] is the state event journal's own
//!   history budget (`STATE_EVENT_HISTORY_MAXIMUM_BYTES`, 8 MiB): a subscriber
//!   that falls further behind than the journal it would catch up from cannot
//!   be served by queueing more.
//!
//! Over budget the frame is dropped for that one subscriber only, exactly as a
//! full count-bounded queue already dropped it: subscribers are notification
//! aids and the bounded histories are the catch-up source, so readers already
//! treat a sequence gap as loss.
//!
//! How a client that lost frames gets back to correct state, so it is never
//! left silently stale: frames are deltas, and the TypeScript host consumer
//! (`consumeQcNativeStateFrame`) treats any sequence gap on the next frame as
//! a stream gap and pulls a full snapshot. If no later frame arrives, its
//! recovery poll (`useNativeStateRecovery`, every 2 s) reads the journal tail
//! and, finding it past the client's cursor, reads a full snapshot from the
//! native caches. The state journal itself keeps every frame a subscriber
//! queue dropped, under its own 8 MiB budget. The difference this module
//! makes is that a drop is no longer silent: it is logged (the first drop per queue and then at each power of
//! two, so a flood cannot flood the log) and counted in [`RetentionCounters`],
//! which the broker status reports as `retention`. The producer never blocks:
//! admission is a pair of atomic operations and a `try_send`, so a stalled
//! subscriber cannot stall the device read loop, its keepalives or its writes.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Byte budget of one raw-frame subscriber queue.
pub(crate) const RAW_SUBSCRIBER_QUEUE_MAX_BYTES: usize =
    2 * quadcortex::profile::MAX_INFLATED_BYTES;

/// Byte budget of one decoded-state subscriber queue.
pub(crate) const STATE_SUBSCRIBER_QUEUE_MAX_BYTES: usize =
    quadcortex::domain::STATE_EVENT_HISTORY_MAXIMUM_BYTES;

// An empty raw queue always admits one maximal frame, with room for a second.
const _: () =
    assert!(RAW_SUBSCRIBER_QUEUE_MAX_BYTES >= 2 * quadcortex::profile::MAX_INFLATED_BYTES);

/// Process-lifetime retention counters. Monotonic; never reset by a session.
#[derive(Debug, Default)]
pub(crate) struct RetentionCounters {
    raw_subscriber_dropped_frames: AtomicU64,
    raw_subscriber_dropped_bytes: AtomicU64,
    state_subscriber_dropped_frames: AtomicU64,
    state_subscriber_dropped_bytes: AtomicU64,
    latest_cache_rejected_frames: AtomicU64,
    latest_cache_rejected_bytes: AtomicU64,
}

/// The status payload's view of [`RetentionCounters`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionDiagnostics {
    /// Frames a raw-frame subscriber did not receive because its queue was
    /// over its entry count or byte budget.
    pub raw_subscriber_dropped_frames: u64,
    pub raw_subscriber_dropped_bytes: u64,
    /// The same for decoded-state subscribers (event streams to clients).
    pub state_subscriber_dropped_frames: u64,
    pub state_subscriber_dropped_bytes: u64,
    /// Messages the latest-message cache refused under its total bounds.
    pub latest_cache_rejected_frames: u64,
    pub latest_cache_rejected_bytes: u64,
}

/// Which kind of subscriber queue a drop happened in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueKind {
    Raw,
    State,
}

impl QueueKind {
    fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw-frame",
            Self::State => "state-event",
        }
    }
}

fn add(counter: &AtomicU64, amount: usize) {
    counter.fetch_add(amount as u64, Ordering::Relaxed);
}

impl RetentionCounters {
    pub(crate) fn snapshot(&self) -> RetentionDiagnostics {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        RetentionDiagnostics {
            raw_subscriber_dropped_frames: read(&self.raw_subscriber_dropped_frames),
            raw_subscriber_dropped_bytes: read(&self.raw_subscriber_dropped_bytes),
            state_subscriber_dropped_frames: read(&self.state_subscriber_dropped_frames),
            state_subscriber_dropped_bytes: read(&self.state_subscriber_dropped_bytes),
            latest_cache_rejected_frames: read(&self.latest_cache_rejected_frames),
            latest_cache_rejected_bytes: read(&self.latest_cache_rejected_bytes),
        }
    }

    fn record_subscriber_drop(&self, kind: QueueKind, bytes: usize) {
        let (frames, total) = match kind {
            QueueKind::Raw => (
                &self.raw_subscriber_dropped_frames,
                &self.raw_subscriber_dropped_bytes,
            ),
            QueueKind::State => (
                &self.state_subscriber_dropped_frames,
                &self.state_subscriber_dropped_bytes,
            ),
        };
        add(frames, 1);
        add(total, bytes);
    }

    pub(crate) fn record_latest_cache_rejection(&self, bytes: usize) {
        add(&self.latest_cache_rejected_frames, 1);
        add(&self.latest_cache_rejected_bytes, bytes);
    }
}

/// Budget state one queue's sender and receiver share.
#[derive(Debug)]
struct QueueBudget {
    kind: QueueKind,
    max_bytes: usize,
    queued_bytes: AtomicUsize,
    dropped_frames: AtomicU64,
    counters: Arc<RetentionCounters>,
}

/// Why [`BudgetedSender::offer`] did not queue an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfferOutcome {
    Queued,
    /// Dropped for this subscriber: its queue was over its byte budget or
    /// entry count. Logged and counted; the subscription stays open.
    Dropped,
    /// The receiver is gone; the caller should forget this sender.
    Disconnected,
}

/// The producer side of a subscriber queue bounded by entry count and bytes.
#[derive(Debug)]
pub(crate) struct BudgetedSender<T> {
    sender: mpsc::SyncSender<(T, usize)>,
    budget: Arc<QueueBudget>,
}

/// The consumer side. Mirrors the `mpsc::Receiver` calls subscribers use and
/// returns each item's bytes to the budget as it is taken.
#[derive(Debug)]
pub struct BudgetedReceiver<T> {
    receiver: mpsc::Receiver<(T, usize)>,
    budget: Arc<QueueBudget>,
}

/// A queue holding at most `capacity` items and `max_bytes` bytes.
pub(crate) fn budgeted_queue<T>(
    kind: QueueKind,
    capacity: usize,
    max_bytes: usize,
    counters: Arc<RetentionCounters>,
) -> (BudgetedSender<T>, BudgetedReceiver<T>) {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    let budget = Arc::new(QueueBudget {
        kind,
        max_bytes,
        queued_bytes: AtomicUsize::new(0),
        dropped_frames: AtomicU64::new(0),
        counters,
    });
    (
        BudgetedSender {
            sender,
            budget: Arc::clone(&budget),
        },
        BudgetedReceiver { receiver, budget },
    )
}

impl QueueBudget {
    /// Reserves `bytes`, or refuses when that would exceed the budget.
    fn reserve(&self, bytes: usize) -> bool {
        self.queued_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                queued
                    .checked_add(bytes)
                    .filter(|total| *total <= self.max_bytes)
            })
            .is_ok()
    }

    fn release(&self, bytes: usize) {
        self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    fn record_drop(&self, bytes: usize, why: &str) {
        self.counters.record_subscriber_drop(self.kind, bytes);
        let dropped = self.dropped_frames.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped.is_power_of_two() {
            eprintln!(
                "Dropped a {}-byte frame for a slow {} subscriber ({why}; {} bytes queued of {}); {dropped} dropped for this subscriber so far",
                bytes,
                self.kind.label(),
                self.queued_bytes.load(Ordering::Acquire),
                self.max_bytes
            );
        }
    }
}

impl<T> BudgetedSender<T> {
    /// Queues `item`, accounted as `bytes`, without ever blocking.
    pub(crate) fn offer(&self, item: T, bytes: usize) -> OfferOutcome {
        if !self.budget.reserve(bytes) {
            // A disconnected receiver is reported as such even when the
            // budget is also exhausted, so the bus forgets it.
            if Arc::strong_count(&self.budget) == 1 {
                return OfferOutcome::Disconnected;
            }
            self.budget.record_drop(bytes, "over its byte budget");
            return OfferOutcome::Dropped;
        }
        match self.sender.try_send((item, bytes)) {
            Ok(()) => OfferOutcome::Queued,
            Err(mpsc::TrySendError::Full(_)) => {
                self.budget.release(bytes);
                self.budget.record_drop(bytes, "over its entry count");
                OfferOutcome::Dropped
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.budget.release(bytes);
                OfferOutcome::Disconnected
            }
        }
    }
}

impl<T> BudgetedReceiver<T> {
    fn take(&self, (item, bytes): (T, usize)) -> T {
        self.budget.release(bytes);
        item
    }

    pub fn recv(&self) -> Result<T, mpsc::RecvError> {
        self.receiver.recv().map(|entry| self.take(entry))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, mpsc::RecvTimeoutError> {
        self.receiver
            .recv_timeout(timeout)
            .map(|entry| self.take(entry))
    }

    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver.try_recv().map(|entry| self.take(entry))
    }

    /// Frames this subscriber has lost to its budget or entry count.
    pub fn dropped_frames(&self) -> u64 {
        self.budget.dropped_frames.load(Ordering::Relaxed)
    }

    /// Bytes currently queued for this subscriber.
    pub fn queued_bytes(&self) -> usize {
        self.budget.queued_bytes.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(
        capacity: usize,
        max_bytes: usize,
    ) -> (
        BudgetedSender<u32>,
        BudgetedReceiver<u32>,
        Arc<RetentionCounters>,
    ) {
        let counters = Arc::new(RetentionCounters::default());
        let (sender, receiver) =
            budgeted_queue(QueueKind::Raw, capacity, max_bytes, Arc::clone(&counters));
        (sender, receiver, counters)
    }

    #[test]
    fn bytes_are_admitted_up_to_the_budget_and_refused_one_byte_past_it() {
        let (sender, receiver, counters) = queue(16, 100);
        // Just below the budget.
        assert_eq!(sender.offer(1, 99), OfferOutcome::Queued);
        assert_eq!(receiver.queued_bytes(), 99);
        // Exactly at it.
        assert_eq!(sender.offer(2, 1), OfferOutcome::Queued);
        assert_eq!(receiver.queued_bytes(), 100);
        // One byte over: dropped, counted, subscription intact.
        assert_eq!(sender.offer(3, 1), OfferOutcome::Dropped);
        assert_eq!(receiver.dropped_frames(), 1);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.raw_subscriber_dropped_frames, 1);
        assert_eq!(snapshot.raw_subscriber_dropped_bytes, 1);
        assert_eq!(snapshot.state_subscriber_dropped_frames, 0);
        // Receiving returns the bytes; the queue admits again.
        assert_eq!(receiver.try_recv(), Ok(1));
        assert_eq!(receiver.queued_bytes(), 1);
        assert_eq!(sender.offer(4, 99), OfferOutcome::Queued);
        assert_eq!(receiver.recv_timeout(Duration::from_millis(10)), Ok(2));
        assert_eq!(receiver.recv(), Ok(4));
        assert_eq!(receiver.queued_bytes(), 0);
    }

    #[test]
    fn a_single_frame_larger_than_the_budget_is_dropped_not_truncated() {
        let (sender, receiver, counters) = queue(16, 100);
        assert_eq!(sender.offer(1, 101), OfferOutcome::Dropped);
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(counters.snapshot().raw_subscriber_dropped_bytes, 101);
        // A frame exactly the budget's size still fits an empty queue.
        assert_eq!(sender.offer(2, 100), OfferOutcome::Queued);
        assert_eq!(receiver.try_recv(), Ok(2));
    }

    #[test]
    fn the_entry_count_still_applies_and_releases_its_reservation() {
        let (sender, receiver, counters) = queue(2, 1_000);
        assert_eq!(sender.offer(1, 10), OfferOutcome::Queued);
        assert_eq!(sender.offer(2, 10), OfferOutcome::Queued);
        assert_eq!(sender.offer(3, 10), OfferOutcome::Dropped);
        assert_eq!(receiver.queued_bytes(), 20);
        assert_eq!(counters.snapshot().raw_subscriber_dropped_frames, 1);
    }

    #[test]
    fn a_dropped_receiver_reads_as_disconnected_even_over_budget() {
        let (sender, receiver, _) = queue(4, 10);
        assert_eq!(sender.offer(1, 10), OfferOutcome::Queued);
        drop(receiver);
        assert_eq!(sender.offer(2, 10), OfferOutcome::Disconnected);
        assert_eq!(sender.offer(3, 1), OfferOutcome::Disconnected);
    }

    #[test]
    fn state_queue_drops_are_counted_separately() {
        let counters = Arc::new(RetentionCounters::default());
        let (sender, _receiver) =
            budgeted_queue::<u8>(QueueKind::State, 4, 8, Arc::clone(&counters));
        assert_eq!(sender.offer(1, 9), OfferOutcome::Dropped);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.state_subscriber_dropped_frames, 1);
        assert_eq!(snapshot.state_subscriber_dropped_bytes, 9);
        assert_eq!(snapshot.raw_subscriber_dropped_frames, 0);
    }
}
