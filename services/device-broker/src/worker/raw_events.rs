use super::*;

const SUBSCRIBER_QUEUE_CAPACITY: usize = quadcortex::domain::STATE_EVENT_DEFAULT_LIMIT;

#[derive(Default)]
pub(super) struct RawEventBus {
    subscribers: Mutex<Vec<RawSubscriber>>,
    /// Shared with every subscriber queue (raw and decoded-state) and read
    /// by `DeviceController::status`.
    pub(super) retention: Arc<RetentionCounters>,
}

impl RawEventBus {
    #[cfg(test)]
    pub(super) fn subscriber_count(&self) -> usize {
        self.subscribers.lock_recover().len()
    }
}

struct RawSubscriber {
    identity: Arc<()>,
    sender: BudgetedSender<IncomingMessage>,
}

/// The waiter owns its membership, including on timeout, cancellation, and
/// queue-admission failure. Cleanup does not depend on future device traffic.
pub struct RawSubscription {
    receiver: BudgetedReceiver<IncomingMessage>,
    identity: Arc<()>,
    bus: std::sync::Weak<RawEventBus>,
}

impl std::ops::Deref for RawSubscription {
    type Target = BudgetedReceiver<IncomingMessage>;
    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl Drop for RawSubscription {
    fn drop(&mut self) {
        if let Some(bus) = self.bus.upgrade() {
            bus.subscribers
                .lock_recover()
                .retain(|subscriber| !Arc::ptr_eq(&subscriber.identity, &self.identity));
        }
    }
}

pub(super) fn subscribe_raw(bus: &Arc<RawEventBus>) -> RawSubscription {
    let (sender, receiver) = budgeted_queue(
        QueueKind::Raw,
        SUBSCRIBER_QUEUE_CAPACITY,
        RAW_SUBSCRIBER_QUEUE_MAX_BYTES,
        Arc::clone(&bus.retention),
    );
    let identity = Arc::new(());
    bus.subscribers.lock_recover().push(RawSubscriber {
        identity: Arc::clone(&identity),
        sender,
    });
    RawSubscription {
        receiver,
        identity,
        bus: Arc::downgrade(bus),
    }
}

/// Offers `message` to every raw-frame subscriber without blocking the device
/// read loop. An over-budget subscriber loses this frame and keeps its
/// subscription; a disconnected subscriber is removed.
pub(super) fn fan_out_raw_event(raw_events: &RawEventBus, message: &IncomingMessage) {
    let payload_bytes = message.payload.len();
    raw_events.subscribers.lock_recover().retain(|subscriber| {
        subscriber.sender.offer(message.clone(), payload_bytes) != OfferOutcome::Disconnected
    });
}
