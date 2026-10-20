//! Broker target admission, independent of USB and the UI transport.
//!
//! A demo provider is the live UI session, not a second simulation. Provider
//! adapters must withdraw it on disconnect and publish a new session id after
//! reopening. Physical ids likewise identify a usable *session*, not a USB port.
//! This module performs no discovery, polling, replay, or transport fallback.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetSource {
    Physical,
    Demo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetIdentity {
    pub source: TargetSource,
    pub session_id: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Unavailable,
    Superseded,
    InvalidSession,
}

#[derive(Default)]
struct State {
    generation: u64,
    selected: Option<TargetIdentity>,
}

/// Shared by provider lifecycle events, MCP admission and broker presentation.
#[derive(Clone, Default)]
pub struct TargetRegistry(Arc<RwLock<State>>);

impl TargetRegistry {
    /// Publish availability, not a user's window selection. A usable physical
    /// session always wins, even when a window deliberately displays its demo.
    /// Identical events leave admitted work valid and generate no new traffic.
    pub fn update(
        &self,
        physical: Option<String>,
        demo: Option<String>,
    ) -> Result<bool, AdmissionError> {
        if physical
            .iter()
            .chain(demo.iter())
            .any(|id| id.trim().is_empty())
        {
            return Err(AdmissionError::InvalidSession);
        }
        let mut state = self.0.write().unwrap_or_else(|p| p.into_inner());
        let selected = physical
            .as_ref()
            .map(|id| (TargetSource::Physical, id))
            .or_else(|| demo.as_ref().map(|id| (TargetSource::Demo, id)));
        let changed = state
            .selected
            .as_ref()
            .map(|id| (id.source, &id.session_id))
            != selected;
        if changed {
            // Refuse future admission rather than wrapping into an old identity.
            state.generation = state
                .generation
                .checked_add(1)
                .expect("target generation exhausted");
            state.selected = selected.map(|(source, session_id)| TargetIdentity {
                source,
                session_id: session_id.clone(),
                generation: state.generation,
            });
        }
        Ok(changed)
    }

    pub fn identity(&self) -> Option<TargetIdentity> {
        self.0
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .selected
            .clone()
    }

    /// Capture exactly once before queuing work. A provider adapter must bind
    /// its callback/transport to this lease; it must never resolve a new target
    /// when a queued callback runs or retry a failed demo request on hardware.
    pub fn admit(&self, expected: Option<&TargetIdentity>) -> Result<TargetLease, AdmissionError> {
        let identity = self.identity().ok_or(AdmissionError::Unavailable)?;
        if expected.is_some_and(|expected| expected != &identity) {
            return Err(AdmissionError::Superseded);
        }
        Ok(TargetLease {
            registry: self.clone(),
            identity,
        })
    }
}

#[derive(Clone)]
pub struct TargetLease {
    registry: TargetRegistry,
    identity: TargetIdentity,
}

impl TargetLease {
    pub fn identity(&self) -> &TargetIdentity {
        &self.identity
    }

    /// Call before every effect and before publishing a result. A target
    /// change invalidates even a result from a successfully completed read.
    pub fn assert_current(&self) -> Result<(), AdmissionError> {
        if self.registry.identity().as_ref() == Some(&self.identity) {
            Ok(())
        } else {
            Err(AdmissionError::Superseded)
        }
    }

    /// Invoke a callback bound to the captured provider, once, and discard its
    /// result if selection changed while it ran. Multi-step providers also
    /// check `assert_current` before each step; this is not a USB write lock.
    pub fn with_current<T>(
        &self,
        effect: impl FnOnce(&TargetIdentity) -> T,
    ) -> Result<T, AdmissionError> {
        self.assert_current()?;
        let result = effect(&self.identity);
        self.assert_current()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_wins_over_demo_and_disconnect_returns_to_same_demo_session() {
        let registry = TargetRegistry::default();
        registry.update(None, Some("ui-demo-1".into())).unwrap();
        let demo = registry.admit(None).unwrap();
        registry
            .update(Some("qc-1-session-1".into()), Some("ui-demo-1".into()))
            .unwrap();
        let physical = registry.admit(None).unwrap();
        assert_eq!(physical.identity().source, TargetSource::Physical);
        assert_eq!(demo.assert_current(), Err(AdmissionError::Superseded));
        registry.update(None, Some("ui-demo-1".into())).unwrap();
        let resumed = registry.admit(None).unwrap();
        assert_eq!(resumed.identity().source, TargetSource::Demo);
        assert_eq!(resumed.identity().session_id, "ui-demo-1");
        assert_ne!(resumed.identity().generation, demo.identity().generation);
        assert!(matches!(
            registry.admit(Some(demo.identity())),
            Err(AdmissionError::Superseded)
        ));
        assert_eq!(physical.assert_current(), Err(AdmissionError::Superseded));
    }

    #[test]
    fn duplicate_events_and_background_demo_changes_do_not_invalidate_physical_work() {
        let registry = TargetRegistry::default();
        registry
            .update(Some("qc-1".into()), Some("demo-1".into()))
            .unwrap();
        let lease = registry.admit(None).unwrap();
        assert!(!registry
            .update(Some("qc-1".into()), Some("demo-2".into()))
            .unwrap());
        assert_eq!(lease.assert_current(), Ok(()));
        assert!(!registry.update(Some("qc-1".into()), None).unwrap());
        assert_eq!(lease.assert_current(), Ok(()));
    }

    #[test]
    fn provider_shutdown_and_replacement_invalidate_queued_work() {
        let registry = TargetRegistry::default();
        registry.update(None, Some("demo-1".into())).unwrap();
        let queued = registry.admit(None).unwrap();
        registry.update(None, None).unwrap();
        assert!(matches!(
            registry.admit(None),
            Err(AdmissionError::Unavailable)
        ));
        registry.update(None, Some("demo-2".into())).unwrap();
        assert_eq!(queued.assert_current(), Err(AdmissionError::Superseded));
    }

    #[test]
    fn invalid_events_leave_current_target_unchanged() {
        let registry = TargetRegistry::default();
        registry.update(None, Some("demo-1".into())).unwrap();
        let lease = registry.admit(None).unwrap();
        assert_eq!(
            registry.update(Some(" ".into()), None),
            Err(AdmissionError::InvalidSession)
        );
        assert_eq!(lease.assert_current(), Ok(()));
    }

    #[test]
    fn queued_callbacks_do_not_run_after_hotplug_and_inflight_results_are_discarded() {
        let registry = TargetRegistry::default();
        registry.update(None, Some("demo-1".into())).unwrap();
        let queued = registry.admit(None).unwrap();
        registry
            .update(Some("qc-1".into()), Some("demo-1".into()))
            .unwrap();
        assert_eq!(
            queued.with_current(|_| panic!("stale write must not run")),
            Err(AdmissionError::Superseded)
        );
        registry.update(None, Some("demo-1".into())).unwrap();
        let executing = registry.admit(None).unwrap();
        let mut calls = 0;
        let result = executing.with_current(|identity| {
            calls += 1;
            assert_eq!(identity.source, TargetSource::Demo);
            registry
                .update(Some("qc-1".into()), Some("demo-1".into()))
                .unwrap();
            "old demo result"
        });
        assert_eq!(result, Err(AdmissionError::Superseded));
        assert_eq!(
            calls, 1,
            "a target change must not retry the callback on hardware"
        );
    }
}
