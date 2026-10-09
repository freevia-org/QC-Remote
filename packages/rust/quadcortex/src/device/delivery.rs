//! Bounded, payload-free evidence of individual native frame transfers.
//! Completion describes the host transport policy, not device acknowledgement.

use crate::device::owner::{
    NativeDispatchTicket, ObservationTicket, OperationIdentity, OperationTicket, SessionFence,
};
use crate::sync::allocate_next_id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

const MAX_DELIVERIES: usize = 256;

/// Opaque handle to one native frame transfer registered in the delivery
/// journal. It is bound to the session it was issued for, and is rejected once
/// that session has changed or its record has been evicted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryTicket {
    sequence: u64,
    fence: SessionFence,
}

/// Lifecycle state of one native frame transfer.
///
/// `Completed`, `Cancelled` and `Indeterminate` are terminal. Completion
/// describes the host transport policy, not device acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryStatus {
    /// Registered but not yet admitted to the writer queue.
    Prepared,
    /// Admitted to the writer queue; no report has started yet.
    Queued,
    /// At least one report has started and the transfer has not finished.
    InFlight,
    /// Every report of the frame was delivered.
    Completed,
    /// Stopped before any report started, so nothing reached the device.
    Cancelled,
    /// Stopped after a report had started; whether the device received any of
    /// the frame is unknown.
    Indeterminate,
}

impl DeliveryStatus {
    /// Returns `true` for `Completed`, `Cancelled` and `Indeterminate`, the
    /// states a transfer never leaves.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Indeterminate
        )
    }
}

/// Why a transfer ended without delivering every report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryStop {
    /// The host's writer queue refused the frame.
    QueueRejected,
    /// The session the frame was prepared for was replaced, reset or
    /// disconnected.
    SessionChanged,
    /// The operation's admission was revoked before the frame finished.
    AdmissionRevoked,
    /// A report write failed with a genuine transport error.
    TransportFailure,
    /// The host transport was stopped while the frame was pending.
    Stopped,
}

/// Result a host reports for one native report write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportOutcome {
    /// The report was written in full.
    Delivered,
    /// The write did not complete but was not a terminal failure: the stall
    /// the QC's USB status is expected to produce. It counts as delivered and
    /// is also tallied in [`DeliveryRecord::expected_stalls`].
    ExpectedStatusStall,
    /// The write failed; the transfer stops with [`DeliveryStop::TransportFailure`].
    Failed,
}

/// Payload-free evidence of one native frame transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRecord {
    /// Handle identifying this transfer.
    pub ticket: DeliveryTicket,
    /// The operation the frame belongs to, if any.
    pub operation: Option<OperationIdentity>,
    /// Protocol message type of the frame.
    pub message_type: u16,
    /// Number of native reports the frame is split into.
    pub report_count: usize,
    /// Number of reports delivered so far, including expected stalls.
    pub delivered_reports: usize,
    /// How many delivered reports ended in an expected status stall.
    pub expected_stalls: usize,
    /// Current lifecycle state.
    pub status: DeliveryStatus,
    /// Why the transfer stopped early; `None` unless it was stopped.
    pub stop: Option<DeliveryStop>,
    /// Host monotonic time, in milliseconds, when the transfer was prepared.
    pub prepared_at_ms: u64,
    /// Host monotonic time, in milliseconds, when the first report started.
    pub started_at_ms: Option<u64>,
    /// Host monotonic time, in milliseconds, when the transfer reached a
    /// terminal state. A reset records no finish time.
    pub finished_at_ms: Option<u64>,
}

struct Entry {
    record: DeliveryRecord,
    dispatch: NativeDispatchTicket,
    active_report: Option<usize>,
    observation_at_start: Option<ObservationTicket>,
}

pub(crate) struct DeliveryJournal {
    next_sequence: u64,
    entries: BTreeMap<u64, Entry>,
    watchers: Vec<SyncSender<()>>,
}

impl Default for DeliveryJournal {
    fn default() -> Self {
        Self {
            next_sequence: 1,
            entries: BTreeMap::new(),
            watchers: Vec::new(),
        }
    }
}

impl DeliveryJournal {
    pub fn operation_observation_start(
        &self,
        operation: &OperationTicket,
    ) -> Option<(&ObservationTicket, u64)> {
        let identity = operation.identity();
        self.entries
            .values()
            .filter(|entry| entry.record.operation.as_ref() == Some(&identity))
            .filter_map(|entry| {
                Some((
                    entry.observation_at_start.as_ref()?,
                    entry.record.started_at_ms?,
                ))
            })
            .max_by_key(|(_, started)| *started)
    }

    pub fn operation_message_types(&self, operation: &OperationTicket) -> Vec<u16> {
        let identity = operation.identity();
        self.entries
            .values()
            .filter(|entry| entry.record.operation.as_ref() == Some(&identity))
            .map(|entry| entry.record.message_type)
            .collect()
    }

    /// An already-started frame may outlive admission expiry. Workflow cleanup
    /// must wait for its final native callback before starting storage settlement.
    pub fn operation_in_flight(&self, operation: &OperationTicket) -> bool {
        let identity = operation.identity();
        self.entries.values().any(|entry| {
            entry.record.operation.as_ref() == Some(&identity)
                && (entry.record.status == DeliveryStatus::InFlight
                    || entry.active_report.is_some())
        })
    }

    pub fn operation_started(&self, operation: &OperationTicket) -> bool {
        let identity = operation.identity();
        self.entries.values().any(|entry| {
            entry.record.operation.as_ref() == Some(&identity)
                && entry.record.started_at_ms.is_some()
        })
    }
    pub fn operation_started_at(
        &self,
        operation: &OperationTicket,
        message_type: u16,
    ) -> Option<u64> {
        let identity = operation.identity();
        self.entries
            .values()
            .filter(|entry| {
                entry.record.operation.as_ref() == Some(&identity)
                    && entry.record.message_type == message_type
            })
            .filter_map(|entry| entry.record.started_at_ms)
            .min()
    }

    fn notify(&mut self) {
        self.watchers
            .retain(|sender| !matches!(sender.try_send(()), Err(TrySendError::Disconnected(()))));
    }

    pub fn watch(&mut self) -> Result<Receiver<()>, String> {
        self.notify();
        if self.watchers.len() >= MAX_DELIVERIES {
            return Err("QC native delivery observer capacity exhausted".into());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        self.watchers.push(sender);
        Ok(receiver)
    }

    pub fn operation_complete(
        &self,
        operation: &OperationTicket,
        frame_count: usize,
    ) -> Result<bool, String> {
        if frame_count == 0 {
            return Err("QC native delivery requires at least one frame".into());
        }
        let mut count = 0;
        let identity = operation.identity();
        let mut complete = true;
        for entry in self
            .entries
            .values()
            .filter(|entry| entry.record.operation.as_ref() == Some(&identity))
        {
            count += 1;
            match entry.record.status {
                DeliveryStatus::Cancelled | DeliveryStatus::Indeterminate => {
                    return Err(format!(
                        "QC native delivery is {:?}; device effect is not confirmed",
                        entry.record.status
                    ));
                }
                DeliveryStatus::Completed => {}
                _ => complete = false,
            }
        }
        if count > frame_count {
            return Err("QC operation has more native frame receipts than planned".into());
        }
        // Evicted history or an unsubmitted frame is never positive evidence.
        Ok(count == frame_count && complete)
    }

    pub fn prepare(
        &mut self,
        dispatch: NativeDispatchTicket,
        fence: SessionFence,
        operation: Option<OperationIdentity>,
        message_type: u16,
        report_count: usize,
        now_ms: u64,
    ) -> Result<DeliveryTicket, String> {
        let max_reports = crate::profile::MAX_FRAME_BYTES / crate::framing::CHUNK_SIZE + 2;
        if report_count == 0 || report_count > max_reports {
            return Err("invalid QC native delivery report count".into());
        }
        if self.entries.len() >= MAX_DELIVERIES {
            let expired = self.entries.iter().find_map(|(key, entry)| {
                (entry.record.status.is_terminal()
                    && entry.active_report.is_none()
                    && entry
                        .dispatch
                        .operation()
                        .is_none_or(|operation| operation.remaining_ms(now_ms) == 0))
                .then_some(*key)
            });
            let Some(expired) = expired else {
                return Err("QC native delivery capacity exhausted by retained transfers".into());
            };
            self.entries.remove(&expired);
        }
        let sequence = allocate_next_id(&mut self.next_sequence, "QC native delivery")?;
        let ticket = DeliveryTicket { sequence, fence };
        self.entries.insert(
            sequence,
            Entry {
                record: DeliveryRecord {
                    ticket: ticket.clone(),
                    operation,
                    message_type,
                    report_count,
                    delivered_reports: 0,
                    expected_stalls: 0,
                    status: DeliveryStatus::Prepared,
                    stop: None,
                    prepared_at_ms: now_ms,
                    started_at_ms: None,
                    finished_at_ms: None,
                },
                dispatch,
                active_report: None,
                observation_at_start: None,
            },
        );
        self.notify();
        Ok(ticket)
    }

    fn entry(&self, ticket: &DeliveryTicket) -> Result<&Entry, String> {
        self.entries
            .get(&ticket.sequence)
            .filter(|entry| entry.record.ticket == *ticket)
            .ok_or_else(|| "QC native delivery receipt is unavailable".into())
    }

    fn entry_mut(&mut self, ticket: &DeliveryTicket) -> Result<&mut Entry, String> {
        self.entries
            .get_mut(&ticket.sequence)
            .filter(|entry| entry.record.ticket == *ticket)
            .ok_or_else(|| "QC native delivery receipt is unavailable".into())
    }

    pub fn record(&self, ticket: &DeliveryTicket) -> Result<DeliveryRecord, String> {
        Ok(self.entry(ticket)?.record.clone())
    }

    pub fn dispatch(&self, ticket: &DeliveryTicket) -> Result<&NativeDispatchTicket, String> {
        Ok(&self.entry(ticket)?.dispatch)
    }

    pub fn admitted(&mut self, ticket: &DeliveryTicket) -> Result<(), String> {
        let entry = self.entry_mut(ticket)?;
        // The writer can begin before its producer returns from queue admission.
        if entry.record.status == DeliveryStatus::Prepared {
            entry.record.status = DeliveryStatus::Queued;
        }
        self.notify();
        Ok(())
    }

    pub fn begin_report(
        &mut self,
        ticket: &DeliveryTicket,
        index: usize,
        now_ms: u64,
        observation: ObservationTicket,
    ) -> Result<(), String> {
        let entry = self.entry_mut(ticket)?;
        if entry.record.status.is_terminal()
            || entry.active_report.is_some()
            || index != entry.record.delivered_reports
            || index >= entry.record.report_count
        {
            return Err("QC native report cannot be started twice or out of order".into());
        }
        entry.record.status = DeliveryStatus::InFlight;
        entry.record.started_at_ms.get_or_insert(now_ms);
        entry.observation_at_start.get_or_insert(observation);
        entry.active_report = Some(index);
        self.notify();
        Ok(())
    }

    pub fn report_finished(
        &mut self,
        ticket: &DeliveryTicket,
        index: usize,
        outcome: ReportOutcome,
        now_ms: u64,
    ) -> Result<(), String> {
        let entry = self.entry_mut(ticket)?;
        // Reset can revoke the transfer while a native call is still returning.
        // Its late result must not promote an indeterminate attempt to success.
        if entry.record.status.is_terminal() {
            // Revocation is a logical result, not proof that the native call
            // returned. Preserve that distinction until its actual callback.
            if entry.active_report == Some(index) {
                entry.active_report = None;
                self.notify();
            }
            return Ok(());
        }
        if entry.active_report != Some(index) {
            return Err("QC native report completion has no matching start".into());
        }
        entry.active_report = None;
        match outcome {
            ReportOutcome::Failed => {
                Self::stop_entry(entry, DeliveryStop::TransportFailure, Some(now_ms))
            }
            ReportOutcome::Delivered | ReportOutcome::ExpectedStatusStall => {
                entry.record.delivered_reports += 1;
                if outcome == ReportOutcome::ExpectedStatusStall {
                    entry.record.expected_stalls += 1;
                }
                if entry.record.delivered_reports == entry.record.report_count {
                    entry.record.status = DeliveryStatus::Completed;
                    entry.record.finished_at_ms = Some(now_ms);
                }
            }
        }
        self.notify();
        Ok(())
    }

    fn stop_entry(entry: &mut Entry, reason: DeliveryStop, now_ms: Option<u64>) {
        if entry.record.status.is_terminal() {
            return;
        }
        entry.record.status = if entry.record.started_at_ms.is_some() {
            DeliveryStatus::Indeterminate
        } else {
            DeliveryStatus::Cancelled
        };
        entry.record.stop = Some(reason);
        entry.record.finished_at_ms = now_ms;
        // An active native call may still return after this logical stop.
        // report_finished clears it without promoting the terminal outcome.
    }

    pub fn stop(
        &mut self,
        ticket: &DeliveryTicket,
        reason: DeliveryStop,
        now_ms: u64,
    ) -> Result<(), String> {
        Self::stop_entry(self.entry_mut(ticket)?, reason, Some(now_ms));
        self.notify();
        Ok(())
    }

    pub fn stop_all(&mut self, reason: DeliveryStop, now_ms: u64) {
        for entry in self.entries.values_mut() {
            Self::stop_entry(entry, reason, Some(now_ms));
        }
        self.notify();
    }

    pub fn reset(&mut self) {
        for entry in self.entries.values_mut() {
            Self::stop_entry(entry, DeliveryStop::SessionChanged, None);
        }
        self.notify();
    }
}
