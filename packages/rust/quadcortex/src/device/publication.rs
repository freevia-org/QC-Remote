//! Bounded handoff from atomic native reductions to a host's one publisher.
//!
//! Hosts wake their delivery worker after ingress or metadata completion. The
//! wakeup carries no state: draining this queue preserves the order in which the
//! owner committed those reductions, regardless of which host thread wakes first.

use crate::device::owner::ObservationTicket;
use crate::responses::TempoClock;
use crate::state::StateUpdate;
use serde::Serialize;
use std::collections::VecDeque;

// Native host resource policy: bounds chosen for this host's own memory
// budget, independent of anything the wire protocol requires. A slow
// UI cannot retain an unbounded series of complete preset projections.
const MAX_PUBLICATIONS: usize = crate::domain::STATE_EVENT_MAXIMUM_LIMIT;
const MAX_PUBLICATION_BYTES: usize = 8 * 1024 * 1024;

/// One atomic reduction of native state, queued for a host to publish to its clients in the order
/// the owner committed it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePublication {
    /// Ticket of the observation that produced this publication.
    pub observation: ObservationTicket,
    /// Wall-clock time the publication was staged, in milliseconds since the Unix epoch.
    pub observed_at: u64,
    /// Normalized state updates produced by the observation.
    pub states: Vec<StateUpdate>,
    /// Tempo clock reading carried by the observation, if any.
    pub tempo_clock: Option<TempoClock>,
}

#[derive(Default)]
pub(crate) struct StatePublications {
    pending: VecDeque<StatePublication>,
    bytes: usize,
    failure: Option<String>,
}

impl StatePublications {
    pub fn ready(&self) -> bool {
        self.failure.is_some() || !self.pending.is_empty()
    }

    pub fn push(&mut self, publication: StatePublication) -> Result<(), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if publication.states.is_empty() && publication.tempo_clock.is_none() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&publication)
            .map_err(|error| format!("Could not encode native state publication: {error}"))?
            .len();
        if self.pending.len() >= MAX_PUBLICATIONS
            || bytes > MAX_PUBLICATION_BYTES.saturating_sub(self.bytes)
        {
            let error =
                "Native state publication backlog exceeded its bounded capacity".to_string();
            self.pending.clear();
            self.bytes = 0;
            self.failure = Some(error.clone());
            return Err(error);
        }
        self.bytes += bytes;
        self.pending.push_back(publication);
        Ok(())
    }

    pub fn take(&mut self) -> Result<Vec<StatePublication>, String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.bytes = 0;
        Ok(self.pending.drain(..).collect())
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.bytes = 0;
        self.failure = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publication(sequence: u64, label: String) -> StatePublication {
        let observation = serde_json::from_value(serde_json::json!({
            "connection": 1, "epoch": {"sessionId": "test", "revision": 0},
            "sequence": sequence, "ingress": null
        }))
        .unwrap();
        let mut state = StateUpdate::empty("sceneLabel");
        state.index = Some(0);
        state.label = Some(label);
        StatePublication {
            observation,
            observed_at: sequence,
            states: vec![state],
            tempo_clock: None,
        }
    }

    #[test]
    fn drains_committed_order_without_sorting_older_metadata_tickets() {
        let mut queue = StatePublications::default();
        queue.push(publication(2, "wire".into())).unwrap();
        queue
            .push(publication(1, "metadata installed later".into()))
            .unwrap();
        let publications = queue.take().unwrap();
        assert_eq!(publications.len(), 2);
        assert_eq!(publications[0].states[0].label.as_deref(), Some("wire"));
        assert_eq!(
            publications[1].states[0].label.as_deref(),
            Some("metadata installed later")
        );
        assert!(!queue.ready());
        assert!(queue.take().unwrap().is_empty());
    }

    #[test]
    fn overflow_is_sticky_and_never_silently_delivers_a_truncated_stream() {
        let mut queue = StatePublications::default();
        for sequence in 0..MAX_PUBLICATIONS {
            queue
                .push(publication(sequence as u64, "value".into()))
                .unwrap();
        }
        assert!(queue.push(publication(999, "excess".into())).is_err());
        assert!(queue.ready());
        assert!(queue.take().is_err());
        assert!(queue.push(publication(1000, "later".into())).is_err());
        queue.clear();
        assert!(!queue.ready());
        queue.push(publication(1001, "new session".into())).unwrap();
        assert_eq!(
            queue.take().unwrap()[0].states[0].label.as_deref(),
            Some("new session")
        );
    }

    #[test]
    fn byte_budget_also_bounds_a_single_large_projection() {
        let mut queue = StatePublications::default();
        assert!(queue
            .push(publication(1, "x".repeat(MAX_PUBLICATION_BYTES)))
            .is_err());
        assert!(queue.take().is_err());
    }
}
