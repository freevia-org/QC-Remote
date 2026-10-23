//! Bounded reverse calls into one authenticated UI-owned demo session.
//!
//! This owns no simulation and never falls back to USB. The RPC layer supplies
//! the authenticated connection id and an already-captured target lease.
use crate::target::{TargetIdentity, TargetLease};
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const NEXT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PENDING: usize = 8;
const MAX_PARAMS_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Call {
    pub target: TargetIdentity,
    pub sequence: u64,
    pub method: String,
    pub params: Value,
}

struct Pending {
    call: Call,
    lease: TargetLease,
    deadline: Instant,
    delivered: bool,
    admitted: bool,
    response: mpsc::SyncSender<Result<Value, String>>,
}

#[derive(Default)]
struct State {
    owner: Option<(u64, String)>,
    explicit_selection: bool,
    selection_token: Option<String>,
    selection_revision: u64,
    sequence: u64,
    pending: VecDeque<Pending>,
}

#[derive(Default)]
pub(crate) struct Hub {
    state: Mutex<State>,
    changed: Condvar,
    #[cfg(test)]
    recheck_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Hub {
    #[cfg(test)]
    fn run_recheck_hook(&self) {
        let hook = self.recheck_hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn register(&self, connection: u64, session: String) -> Result<(), String> {
        self.register_selection(connection, session, false)
    }

    pub fn register_selection(
        &self,
        connection: u64,
        session: String,
        explicit_selection: bool,
    ) -> Result<(), String> {
        if session.trim().is_empty() || session.len() > 512 {
            return Err("Demo session id must contain 1..512 UTF-8 bytes".into());
        }
        let mut state = self.lock();
        if state.owner.is_some() || state.explicit_selection {
            return Err("A live UI already owns the demo provider".into());
        }
        if explicit_selection {
            state.selection_revision = state
                .selection_revision
                .checked_add(1)
                .ok_or("Demo selection revision exhausted")?;
        }
        state.owner = Some((connection, session));
        state.explicit_selection = explicit_selection;
        self.changed.notify_all();
        Ok(())
    }

    pub fn session(&self) -> Option<String> {
        self.lock().owner.as_ref().map(|(_, id)| id.clone())
    }

    pub fn explicit_session(&self) -> Option<String> {
        let state = self.lock();
        state
            .owner
            .as_ref()
            .filter(|_| state.explicit_selection)
            .map(|(_, id)| id.clone())
    }

    /// Public revision is not a capability. The bearer remains the trust boundary.
    pub fn selection_revision(&self) -> u64 {
        self.lock().selection_revision
    }

    pub fn selection_held(&self) -> bool {
        self.lock().explicit_selection
    }

    pub fn select_explicit(
        &self,
        connection: u64,
        session: String,
        token: String,
        previous_token: Option<&str>,
        recover_revision: Option<u64>,
    ) -> Result<(), String> {
        let mut state = self.lock();
        if state.owner.is_some() {
            return Err("A live UI already owns the demo provider".into());
        }
        // A fresh explicit recovery first reads the revision, then compares it
        // here. A delayed old action cannot overwrite a newer selection.
        let recovery = recover_revision == Some(state.selection_revision);
        let retained =
            previous_token.is_some() && previous_token == state.selection_token.as_deref();
        if !recovery && !retained {
            return Err("Demo selection changed; select Demo again".into());
        }
        let revision = state
            .selection_revision
            .checked_add(1)
            .ok_or("Demo selection revision exhausted")?;
        state.selection_revision = revision;
        state.selection_token = Some(token);
        state.explicit_selection = true;
        state.owner = Some((connection, session));
        self.changed.notify_all();
        Ok(())
    }

    pub fn clear_selection(&self, token: &str) -> bool {
        let retired = {
            let mut state = self.lock();
            if state.selection_token.as_deref() != Some(token) {
                return false;
            }
            if state.explicit_selection {
                let Some(revision) = state.selection_revision.checked_add(1) else {
                    return false;
                };
                state.selection_revision = revision;
            }
            state.owner = None;
            state.explicit_selection = false;
            state.selection_token = None;
            std::mem::take(&mut state.pending)
        };
        drop(retired);
        self.changed.notify_all();
        true
    }

    /// Explicit local-bearer action for an acknowledgement/token lost in transit.
    /// Increment even an empty intent so already-issued old registrations lose CAS.
    pub fn clear_observed_selection(&self, expected_revision: u64) -> bool {
        let retired = {
            let mut state = self.lock();
            if state.selection_revision != expected_revision {
                return false;
            }
            let Some(next) = state.selection_revision.checked_add(1) else {
                return false;
            };
            state.selection_revision = next;
            state.owner = None;
            state.explicit_selection = false;
            state.selection_token = None;
            std::mem::take(&mut state.pending)
        };
        drop(retired);
        self.changed.notify_all();
        true
    }

    /// Transport loss removes availability, NOT the person's selected target.
    pub fn disconnected(&self, connection: u64) -> bool {
        let retired = {
            let mut state = self.lock();
            if !Self::owner_matches(&state, connection) {
                return false;
            }
            state.owner = None;
            std::mem::take(&mut state.pending)
        };
        drop(retired);
        self.changed.notify_all();
        true
    }

    pub fn target_changed(&self) {
        Self::prune(&mut self.lock());
        self.changed.notify_all();
    }

    pub fn withdraw(&self, connection: u64) -> bool {
        let retired = {
            let mut state = self.lock();
            if !state
                .owner
                .as_ref()
                .is_some_and(|(id, _)| *id == connection)
            {
                return false;
            }
            if state.explicit_selection {
                let Some(revision) = state.selection_revision.checked_add(1) else {
                    return false;
                };
                state.selection_revision = revision;
            }
            state.owner = None;
            state.explicit_selection = false;
            state.selection_token = None;
            std::mem::take(&mut state.pending)
        };
        // Dropping senders wakes all callers, outside the hub lock.
        drop(retired);
        self.changed.notify_all();
        true
    }

    fn prune(state: &mut State) {
        let now = Instant::now();
        state.pending.retain(|pending| {
            let current = now < pending.deadline && pending.lease.assert_current().is_ok();
            if !current {
                let _ = pending
                    .response
                    .try_send(Err("Demo target changed or request expired".into()));
            }
            current
        });
    }

    fn owner_matches(state: &State, connection: u64) -> bool {
        state
            .owner
            .as_ref()
            .is_some_and(|(id, _)| *id == connection)
    }

    /// Queue once and wait against one original deadline. A late response is
    /// discarded and this call is never retried against a different target.
    pub fn invoke(&self, lease: TargetLease, method: &str, params: Value) -> Result<Value, String> {
        self.invoke_with_timeout(lease, method, params, REQUEST_TIMEOUT)
    }

    fn invoke_with_timeout(
        &self,
        lease: TargetLease,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        if serde_json::to_vec(&params)
            .map_err(|e| e.to_string())?
            .len()
            > MAX_PARAMS_BYTES
        {
            return Err("Demo request exceeds 1 MiB".into());
        }
        lease.assert_current().map_err(|_| "Demo target changed")?;
        let (response, receiver) = mpsc::sync_channel(1);
        let sequence = {
            let mut state = self.lock();
            Self::prune(&mut state);
            if !state.owner.as_ref().is_some_and(|(_, session)| {
                lease.identity().source == crate::target::TargetSource::Demo
                    && *session == lease.identity().session_id
            }) {
                return Err("Demo provider is unavailable for this target".into());
            }
            if state.pending.len() >= MAX_PENDING {
                return Err("Demo provider request queue is full; request was not sent".into());
            }
            state.sequence = state
                .sequence
                .checked_add(1)
                .ok_or("Demo sequence exhausted")?;
            let sequence = state.sequence;
            state.pending.push_back(Pending {
                call: Call {
                    target: lease.identity().clone(),
                    sequence,
                    method: method.into(),
                    params,
                },
                lease: lease.clone(),
                deadline,
                delivered: false,
                admitted: false,
                response,
            });
            sequence
        };
        self.changed.notify_all();
        let result = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        self.lock()
            .pending
            .retain(|pending| pending.call.sequence != sequence);
        self.changed.notify_all();
        lease
            .assert_current()
            .map_err(|_| "Demo target changed; result discarded")?;
        result.map_err(|_| "Demo provider stopped or request deadline expired".to_string())?
    }

    pub fn next(&self, connection: u64, wait: Duration) -> Result<Option<Call>, String> {
        let deadline = Instant::now() + wait.min(NEXT_TIMEOUT);
        let mut state = self.lock();
        loop {
            if !Self::owner_matches(&state, connection) {
                return Err("This connection does not own the demo provider".into());
            }
            Self::prune(&mut state);
            // One outstanding delivered call: a second Next must not cause
            // the UI to execute requests concurrently or execute one twice.
            if state.pending.iter().any(|pending| pending.delivered) {
                return Err("Reply to the delivered demo request before requesting another".into());
            }
            if let Some(pending) = state.pending.front_mut() {
                pending
                    .lease
                    .assert_current()
                    .map_err(|_| "Demo target changed")?;
                pending.delivered = true;
                return Ok(Some(pending.call.clone()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    pub fn admit(
        &self,
        connection: u64,
        target: &TargetIdentity,
        sequence: u64,
    ) -> Result<bool, String> {
        let mut state = self.lock();
        Self::prune(&mut state);
        #[cfg(test)]
        self.run_recheck_hook();
        if !Self::owner_matches(&state, connection) {
            return Err("This connection does not own the demo provider".into());
        }
        let Some(index) = state
            .pending
            .iter()
            .position(|p| p.call.sequence == sequence)
        else {
            return Ok(false);
        };
        let pending = &mut state.pending[index];
        if !pending.delivered || pending.call.target != *target {
            return Err("Demo request cannot be admitted with this identity/state".into());
        }
        if pending.lease.assert_current().is_err() || Instant::now() >= pending.deadline {
            let retired = state
                .pending
                .remove(index)
                .expect("located pending request");
            drop(state);
            let _ = retired
                .response
                .try_send(Err("Demo target changed or request expired".into()));
            self.changed.notify_all();
            return Ok(false);
        }
        pending.admitted = true;
        Ok(true)
    }

    pub fn reply(
        &self,
        connection: u64,
        target: &TargetIdentity,
        sequence: u64,
        result: Result<Value, String>,
    ) -> Result<bool, String> {
        let bytes = match &result {
            Ok(value) => serde_json::to_vec(value).map_err(|e| e.to_string())?.len(),
            Err(message) => message.len(),
        };
        if bytes > MAX_REPLY_BYTES {
            return Err("Demo reply exceeds 16 MiB".into());
        }
        let pending = {
            let mut state = self.lock();
            Self::prune(&mut state);
            #[cfg(test)]
            self.run_recheck_hook();
            if !Self::owner_matches(&state, connection) {
                return Err("This connection does not own the demo provider".into());
            }
            let Some(index) = state
                .pending
                .iter()
                .position(|p| p.call.sequence == sequence)
            else {
                return Ok(false);
            };
            let pending = &state.pending[index];
            if !pending.admitted || pending.call.target != *target {
                return Err("Demo reply does not match an admitted request".into());
            }
            if pending.lease.assert_current().is_err() || Instant::now() >= pending.deadline {
                let retired = state
                    .pending
                    .remove(index)
                    .expect("located pending request");
                drop(state);
                let _ = retired
                    .response
                    .try_send(Err("Demo target changed or request expired".into()));
                self.changed.notify_all();
                return Ok(false);
            }
            state
                .pending
                .remove(index)
                .expect("located pending request")
        };
        let accepted = pending.response.try_send(result).is_ok();
        self.changed.notify_all();
        Ok(accepted)
    }

    pub fn reject(
        &self,
        connection: u64,
        target: &TargetIdentity,
        sequence: u64,
        message: &str,
    ) -> Result<bool, String> {
        let pending = {
            let mut state = self.lock();
            if !Self::owner_matches(&state, connection) {
                return Err("This connection does not own the demo provider".into());
            }
            let Some(index) = state
                .pending
                .iter()
                .position(|pending| pending.call.sequence == sequence)
            else {
                return Ok(false);
            };
            if state.pending[index].call.target != *target {
                return Err("Demo request identity mismatch".into());
            }
            state
                .pending
                .remove(index)
                .expect("located pending request")
        };
        let retired = pending.response.try_send(Err(message.into())).is_ok();
        self.changed.notify_all();
        Ok(retired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::TargetRegistry;
    use std::sync::Arc;

    #[test]
    fn explicit_demo_loss_holds_intent_and_recovery_fences_stale_clear_and_disconnect() {
        let hub = Hub::default();
        hub.select_explicit(1, "demo-a".into(), "token-a".into(), None, Some(0))
            .unwrap();
        assert!(hub.disconnected(1));
        assert!(hub.selection_held());
        assert!(hub.explicit_session().is_none());
        assert!(hub.register(2, "legacy".into()).is_err());
        assert!(hub
            .select_explicit(2, "stale".into(), "old".into(), None, Some(0))
            .is_err());
        hub.select_explicit(2, "demo-b".into(), "token-b".into(), Some("token-a"), None)
            .unwrap();
        assert!(!hub.disconnected(1));
        assert!(!hub.clear_selection("token-a"));
        assert!(!hub.withdraw(1));
        assert_eq!(hub.explicit_session().as_deref(), Some("demo-b"));
        assert!(hub.clear_selection("token-b"));
        assert!(!hub.selection_held());
        assert!(!hub.disconnected(2));
        assert!(hub
            .select_explicit(3, "late".into(), "late".into(), None, Some(2))
            .is_err());
    }

    #[test]
    fn explicit_lost_token_recovery_is_revision_checked_without_restarting_the_broker() {
        let hub = Hub::default();
        hub.select_explicit(1, "demo-a".into(), "token-a".into(), None, Some(0))
            .unwrap();
        hub.disconnected(1);
        let observed = hub.selection_revision();
        hub.select_explicit(2, "demo-b".into(), "token-b".into(), None, Some(observed))
            .unwrap();
        hub.disconnected(2);
        assert!(hub
            .select_explicit(3, "late".into(), "late".into(), None, Some(observed))
            .is_err());
        assert!(!hub.clear_selection("token-a"));
        assert!(hub.selection_held());
    }

    #[test]
    fn explicit_clear_after_lost_registration_ack_fences_delayed_register_and_old_tokens() {
        let hub = Hub::default();
        let stale_revision = hub.selection_revision();
        hub.select_explicit(
            1,
            "unacknowledged".into(),
            "unseen-token".into(),
            None,
            Some(stale_revision),
        )
        .unwrap();
        hub.disconnected(1);
        let revision = hub.selection_revision();
        assert!(hub.clear_observed_selection(revision));
        assert!(!hub.selection_held());
        assert!(hub
            .select_explicit(2, "late".into(), "late".into(), None, Some(stale_revision))
            .is_err());
        assert!(hub
            .select_explicit(2, "late".into(), "late".into(), Some("unseen-token"), None)
            .is_err());
        hub.select_explicit(
            3,
            "newer".into(),
            "newer-token".into(),
            None,
            Some(hub.selection_revision()),
        )
        .unwrap();
        assert!(!hub.clear_observed_selection(revision));
        assert_eq!(hub.explicit_session().as_deref(), Some("newer"));
    }

    fn fixture() -> (Arc<Hub>, TargetRegistry) {
        let hub = Arc::new(Hub::default());
        hub.register(1, "window-session".into()).unwrap();
        let targets = TargetRegistry::default();
        targets.update(None, hub.session()).unwrap();
        (hub, targets)
    }

    fn call(
        hub: Arc<Hub>,
        targets: &TargetRegistry,
    ) -> std::thread::JoinHandle<Result<Value, String>> {
        let lease = targets.admit(None).unwrap();
        std::thread::spawn(move || {
            hub.invoke_with_timeout(
                lease,
                "device.snapshot",
                Value::Null,
                Duration::from_secs(2),
            )
        })
    }

    #[test]
    fn registration_is_exclusive_and_withdrawal_is_connection_owned() {
        let (hub, _) = fixture();
        assert!(hub.register(2, "other".into()).is_err());
        assert!(!hub.withdraw(2));
        assert_eq!(hub.session().as_deref(), Some("window-session"));
        assert!(hub.withdraw(1));
        hub.register(2, "replacement".into()).unwrap();
        assert!(!hub.withdraw(1));
        assert_eq!(hub.session().as_deref(), Some("replacement"));
        assert!(hub.next(1, Duration::ZERO).is_err());
    }

    #[test]
    fn a_request_is_delivered_admitted_and_answered_exactly_once() {
        let (hub, targets) = fixture();
        let caller = call(hub.clone(), &targets);
        let next = hub.next(1, Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(next.method, "device.snapshot");
        assert_eq!(next.target, targets.identity().unwrap());
        assert!(hub.next(1, Duration::ZERO).is_err());
        assert!(hub.admit(2, &next.target, next.sequence).is_err());
        assert!(hub
            .reply(1, &next.target, next.sequence, Ok(Value::Null))
            .is_err());
        hub.admit(1, &next.target, next.sequence).unwrap();
        assert!(hub.admit(1, &next.target, next.sequence).unwrap());
        let value = serde_json::json!({"sessionId":"window-session", "name":"shared UI preset"});
        hub.reply(1, &next.target, next.sequence, Ok(value.clone()))
            .unwrap();
        assert_eq!(caller.join().unwrap().unwrap(), value);
        assert!(!hub
            .reply(1, &next.target, next.sequence, Ok(Value::Null))
            .unwrap());
        assert!(hub.next(1, Duration::ZERO).unwrap().is_none());
    }

    #[test]
    fn hotplug_rejects_delayed_demo_admission_and_never_retargets_the_call() {
        let (hub, targets) = fixture();
        let caller = call(hub.clone(), &targets);
        let next = hub.next(1, Duration::from_secs(1)).unwrap().unwrap();
        targets
            .update(Some("physical-session".into()), hub.session())
            .unwrap();
        assert!(!hub.admit(1, &next.target, next.sequence).unwrap());
        assert!(caller.join().unwrap().is_err());
        assert!(hub.next(1, Duration::ZERO).unwrap().is_none());
        targets.update(None, hub.session()).unwrap();
        assert_ne!(targets.identity().unwrap(), next.target);
        assert!(!hub
            .reply(1, &next.target, next.sequence, Ok(Value::Null))
            .unwrap());
    }

    #[test]
    fn disconnect_wakes_admitted_callers_and_late_replies_cannot_enter_a_new_session() {
        let (hub, targets) = fixture();
        let caller = call(hub.clone(), &targets);
        let next = hub.next(1, Duration::from_secs(1)).unwrap().unwrap();
        hub.admit(1, &next.target, next.sequence).unwrap();
        assert!(hub.withdraw(1));
        assert!(caller.join().unwrap().is_err());
        hub.register(2, "new-window".into()).unwrap();
        targets.update(None, hub.session()).unwrap();
        assert!(hub
            .reply(1, &next.target, next.sequence, Ok(Value::Null))
            .is_err());
        assert!(hub.next(2, Duration::ZERO).unwrap().is_none());
    }

    #[test]
    fn timeout_retires_the_request_instead_of_accepting_a_late_reply() {
        let (hub, targets) = fixture();
        assert!(hub
            .invoke_with_timeout(
                targets.admit(None).unwrap(),
                "device.snapshot",
                Value::Null,
                Duration::ZERO
            )
            .is_err());
        assert!(hub.next(1, Duration::ZERO).unwrap().is_none());
    }

    #[test]
    fn saturation_refuses_the_ninth_call_and_withdrawal_retires_every_waiter() {
        let (hub, targets) = fixture();
        std::thread::scope(|scope| {
            let callers: Vec<_> = (0..MAX_PENDING)
                .map(|_| {
                    let hub = hub.clone();
                    let lease = targets.admit(None).unwrap();
                    scope.spawn(move || {
                        hub.invoke_with_timeout(
                            lease,
                            "device.snapshot",
                            Value::Null,
                            Duration::from_secs(3),
                        )
                    })
                })
                .collect();
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut state = hub.lock();
            while state.pending.len() != MAX_PENDING {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(
                    !remaining.is_zero(),
                    "callers did not reach the bounded queue"
                );
                state = hub.changed.wait_timeout(state, remaining).unwrap().0;
            }
            drop(state);
            let refused = hub.invoke_with_timeout(
                targets.admit(None).unwrap(),
                "device.snapshot",
                Value::Null,
                Duration::from_secs(1),
            );
            assert!(refused.unwrap_err().contains("queue is full"));
            assert!(hub.withdraw(1));
            for caller in callers {
                assert!(caller.join().unwrap().is_err());
            }
            assert!(hub.lock().pending.is_empty());
        });
    }

    #[test]
    fn target_changes_after_prune_return_false_without_retiring_the_provider() {
        for replying in [false, true] {
            let (hub, targets) = fixture();
            let caller = call(hub.clone(), &targets);
            let next = hub.next(1, Duration::from_secs(1)).unwrap().unwrap();
            if replying {
                assert!(hub.admit(1, &next.target, next.sequence).unwrap());
            }
            let changing = targets.clone();
            let session = hub.session();
            *hub.recheck_hook.lock().unwrap() = Some(Box::new(move || {
                changing
                    .update(Some("new-physical".into()), session)
                    .unwrap();
            }));
            let accepted = if replying {
                hub.reply(1, &next.target, next.sequence, Ok(Value::Null))
            } else {
                hub.admit(1, &next.target, next.sequence)
            };
            assert!(!accepted.unwrap());
            assert!(caller.join().unwrap().is_err());
            assert_eq!(hub.session().as_deref(), Some("window-session"));
        }
    }
}
