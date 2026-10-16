//! Android request-owned HTTP tasks. Payloads stay on this native/private lane.
use quadcortex::commands::OutboundMessage;
use quadcortex::device::owner::ObservationTicket;
use quadcortex::sync::RecoverPoison;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

pub(super) fn is_forwarding_type(kind: u16) -> bool {
    matches!(kind, 18 | 29 | 61)
}

struct SecretMessage(OutboundMessage, Option<Instant>);
impl Drop for SecretMessage {
    fn drop(&mut self) {
        self.0.payload.zeroize();
    }
}

struct Task {
    observation: ObservationTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Mutex<mpsc::Receiver<SecretMessage>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    delivered_deadline: Mutex<Option<Instant>>,
}
impl Task {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn close(&self) {
        self.cancel();
        // Joining never holds the decoder's session or task-registry lock.
        if let Some(worker) = self.worker.lock_recover().take() {
            let _ = worker.join();
        }
        while self.receiver.lock_recover().try_recv().is_ok() {}
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Default)]
pub(super) struct ForwardingTasks {
    enabled: AtomicBool,
    tasks: Mutex<HashMap<i64, Arc<Task>>>,
}
impl ForwardingTasks {
    pub(super) fn set_enabled(&self, enabled: bool) {
        // Serialize disabling against task publication, including a task whose
        // thread has started but has not yet entered the registry.
        let tasks = self.tasks.lock_recover();
        self.enabled.store(enabled, Ordering::Release);
        if !enabled {
            for task in tasks.values() {
                task.cancel();
            }
        }
    }
    pub(super) fn cancel_all(&self) {
        self.set_enabled(false);
    }
    pub(super) fn retire_tasks(&self) {
        for task in self.tasks.lock_recover().values() {
            task.cancel();
        }
    }
    pub(super) fn start(
        &self,
        kind: u16,
        payload: Vec<u8>,
        observation: ObservationTicket,
    ) -> Result<i64, String> {
        let started = Instant::now();
        let payload = Zeroizing::new(payload);
        if !is_forwarding_type(kind) {
            return Err("unsupported forwarding family".into());
        }
        if kind == 18 {
            return Ok(0);
        } // Notification, no HTTP request.
        let mut tasks = self.tasks.lock_recover();
        if !self.enabled.load(Ordering::Acquire) {
            return Ok(0);
        }
        static NEXT_TASK: AtomicI64 = AtomicI64::new(1);
        let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
        if id <= 0 {
            return Err("forwarding task identity exhausted".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        // At most one response frame can wait for USB admission. Firmware
        // download size therefore does not grow the native response queue.
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("qc-android-forward".into())
            .spawn(move || {
                let mut sink = |message: OutboundMessage,
                                deadline: Option<Instant>|
                 -> Result<(), quadcortex_forwarder::ForwardError> {
                    let mut pending = SecretMessage(message, deadline);
                    loop {
                        if worker_cancelled.load(Ordering::Acquire) {
                            return Err(quadcortex_forwarder::ForwardError::Cancelled);
                        }
                        if deadline.is_some_and(|limit| Instant::now() >= limit) {
                            return Err(quadcortex_forwarder::ForwardError::Timeout);
                        }
                        match sender.try_send(pending) {
                            Ok(()) => return Ok(()),
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                return Err(quadcortex_forwarder::ForwardError::SinkClosed)
                            }
                            Err(mpsc::TrySendError::Full(message)) => pending = message,
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                };
                let _ = quadcortex_forwarder::forward_at(
                    kind,
                    &payload,
                    Arc::clone(&worker_cancelled),
                    &mut sink,
                    started,
                );
            })
            .map_err(|_| "could not start forwarding task".to_string())?;
        tasks.insert(
            id,
            Arc::new(Task {
                observation,
                cancelled,
                receiver: Mutex::new(receiver),
                worker: Mutex::new(Some(worker)),
                delivered_deadline: Mutex::new(None),
            }),
        );
        Ok(id)
    }
    pub(super) fn observation(&self, id: i64) -> Option<ObservationTicket> {
        let tasks = self.tasks.lock_recover();
        let task = tasks.get(&id)?;
        if !self.enabled.load(Ordering::Acquire) || task.cancelled.load(Ordering::Acquire) {
            return None;
        }
        if task
            .delivered_deadline
            .lock_recover()
            .is_some_and(|limit| Instant::now() >= limit)
        {
            task.cancel();
            return None;
        }
        Some(task.observation.clone())
    }
    pub(super) fn next(&self, id: i64) -> Result<Option<OutboundMessage>, String> {
        let task = self
            .tasks
            .lock_recover()
            .get(&id)
            .cloned()
            .ok_or("unknown forwarding task")?;
        if task.cancelled.load(Ordering::Acquire) {
            return Ok(None);
        }
        let received = task
            .receiver
            .lock_recover()
            .recv_timeout(Duration::from_millis(100));
        match received {
            Ok(mut message) => {
                if task.cancelled.load(Ordering::Acquire) {
                    return Ok(None);
                }
                if message.1.is_some_and(|limit| Instant::now() >= limit) {
                    task.cancel();
                    return Ok(None);
                }
                *task.delivered_deadline.lock_recover() = message.1;
                Ok(Some(OutboundMessage {
                    message_type: message.0.message_type,
                    payload: std::mem::take(&mut message.0.payload),
                }))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                task.cancel();
                Ok(None)
            }
        }
    }
    pub(super) fn write_budget_ms(&self, id: i64) -> i32 {
        let tasks = self.tasks.lock_recover();
        let Some(task) = tasks.get(&id) else {
            return 0;
        };
        if !self.enabled.load(Ordering::Acquire) || task.cancelled.load(Ordering::Acquire) {
            return 0;
        }
        let deadline = *task.delivered_deadline.lock_recover();
        match deadline {
            None => 250,
            Some(end) => {
                let remaining = end.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    task.cancel();
                    return 0;
                }
                // Round up sub-millisecond positive budgets. Android's USB
                // timeout 0 means unlimited and is never returned as admission.
                remaining.as_micros().div_ceil(1000).min(250) as i32
            }
        }
    }
    pub(super) fn finished(&self, id: i64) -> bool {
        self.tasks
            .lock_recover()
            .get(&id)
            .is_none_or(|task| task.cancelled.load(Ordering::Acquire))
    }
    pub(super) fn cancel(&self, id: i64) {
        if let Some(task) = self.tasks.lock_recover().get(&id) {
            task.cancel();
        }
    }
    pub(super) fn close(&self, id: i64) {
        let task = self.tasks.lock_recover().remove(&id);
        if let Some(task) = task {
            task.close();
        }
    }
}
impl Drop for ForwardingTasks {
    fn drop(&mut self) {
        self.enabled.store(false, Ordering::Release);
        let tasks = std::mem::take(&mut *self.tasks.lock_recover());
        for task in tasks.values() {
            task.cancel();
        }
        for task in tasks.values() {
            task.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued_task(
        deadline: Option<Instant>,
    ) -> (ForwardingTasks, mpsc::SyncSender<SecretMessage>) {
        let registry = ForwardingTasks::default();
        registry.set_enabled(true);
        let (sender, receiver) = mpsc::sync_channel(1);
        let observation =
            serde_json::from_str(r#"{"connection":1,"epoch":null,"sequence":1,"ingress":null}"#)
                .unwrap();
        registry.tasks.lock_recover().insert(
            1,
            Arc::new(Task {
                observation,
                cancelled: Arc::new(AtomicBool::new(false)),
                receiver: Mutex::new(receiver),
                worker: Mutex::new(None),
                delivered_deadline: Mutex::new(deadline),
            }),
        );
        (registry, sender)
    }

    fn message(byte: u8, deadline: Option<Instant>) -> SecretMessage {
        SecretMessage(
            OutboundMessage {
                message_type: 61,
                payload: vec![byte; 32 * 1024],
            },
            deadline,
        )
    }

    #[test]
    fn final_queued_chunk_is_delivered_before_completion() {
        let (registry, sender) = queued_task(None);
        assert!(sender.try_send(message(7, None)).is_ok());
        assert!(matches!(
            sender.try_send(message(8, None)),
            Err(mpsc::TrySendError::Full(_))
        ));
        drop(sender);
        assert!(!registry.finished(1));
        assert_eq!(
            registry.next(1).unwrap().unwrap().payload,
            vec![7; 32 * 1024]
        );
        assert!(!registry.finished(1));
        assert!(registry.next(1).unwrap().is_none());
        assert!(registry.finished(1));
        registry.close(1);
    }

    #[test]
    fn disable_retires_queued_payload_and_cannot_be_reenabled() {
        let (registry, sender) = queued_task(None);
        assert!(sender.try_send(message(7, None)).is_ok());
        registry.set_enabled(false);
        registry.set_enabled(true);
        assert!(registry.observation(1).is_none());
        assert_eq!(registry.write_budget_ms(1), 0);
        assert!(registry.next(1).unwrap().is_none());
        registry.close(1);
        assert!(matches!(
            sender.try_send(message(8, None)),
            Err(mpsc::TrySendError::Disconnected(_))
        ));
    }

    #[test]
    fn expired_response_never_crosses_to_java_or_usb_admission() {
        let expired = Some(Instant::now() - Duration::from_millis(1));
        let (registry, sender) = queued_task(None);
        assert!(sender.try_send(message(7, expired)).is_ok());
        assert!(registry.next(1).unwrap().is_none());
        assert_eq!(registry.write_budget_ms(1), 0);
        assert!(registry.finished(1));
        registry.close(1);
    }

    #[test]
    fn usb_timeout_is_capped_and_never_unlimited_after_expiry() {
        let (registry, _sender) = queued_task(None);
        assert_eq!(registry.write_budget_ms(1), 250);
        let task = registry.tasks.lock_recover().get(&1).cloned().unwrap();
        *task.delivered_deadline.lock_recover() = Some(Instant::now() + Duration::from_millis(200));
        assert!((1..=200).contains(&registry.write_budget_ms(1)));
        *task.delivered_deadline.lock_recover() = Some(Instant::now() - Duration::from_millis(1));
        assert_eq!(registry.write_budget_ms(1), 0);
        registry.close(1);
    }
}
