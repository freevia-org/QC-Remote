//! Session ownership around the existing single-request LocalBackup assembler.
use super::{ObservationTicket, OperationTicket, SessionFence, SessionRuntime};
use crate::device::backup::{BackupAction, BackupProgress, BackupRuntime, BackupTerminalState};

pub(super) struct BackupEntry {
    operation: OperationTicket,
    runtime: BackupRuntime,
    last_observation: u64,
}

impl SessionRuntime {
    /// Starts a device backup for `operation` and marks the ticket as a backup operation.
    ///
    /// Fails if the operation is not admissible, if a non-terminal backup is already in progress, or if the operation's sequence is not newer than the last backup operation.
    pub fn begin_backup(
        &mut self,
        operation: &mut OperationTicket,
        now_ms: u64,
    ) -> Result<(), String> {
        self.validate_operation(operation, now_ms)?;
        if self
            .backup
            .as_ref()
            .is_some_and(|entry| !entry.runtime.is_terminal())
        {
            return Err("A device backup is already in progress".into());
        }
        if operation.sequence <= self.last_backup_operation {
            return Err("QC backup operation cannot be replayed".into());
        }
        operation.backup = true;
        self.last_backup_operation = operation.sequence;
        self.backup = Some(BackupEntry {
            operation: operation.clone(),
            runtime: BackupRuntime::start(now_ms, operation.remaining_ms(now_ms)),
            last_observation: 0,
        });
        Ok(())
    }

    fn backup_entry(&self, operation: &OperationTicket) -> Result<&BackupEntry, String> {
        let entry = self
            .backup
            .as_ref()
            .ok_or("No native QC backup transfer is active")?;
        if entry.operation != *operation {
            return Err("QC backup attempt changed".into());
        }
        let fence = SessionFence {
            connection: operation.connection,
            epoch: operation.epoch.clone(),
        };
        if !self.accepts_fence(&fence) {
            return Err("QC backup session changed".into());
        }
        Ok(entry)
    }

    pub(super) fn validate_backup_dispatch(
        &self,
        operation: &OperationTicket,
    ) -> Result<(), String> {
        if self.backup_entry(operation)?.runtime.is_terminal() {
            return Err("QC backup transfer is already terminal".into());
        }
        Ok(())
    }

    /// Validate provenance and mutate the assembler in the same owner transition.
    /// Pre-export or old-session frames are ignored, never spliced into the stream.
    pub fn consume_backup(
        &mut self,
        operation: &OperationTicket,
        observation: &ObservationTicket,
        payload: &[u8],
        now_ms: u64,
    ) -> Result<Option<BackupProgress>, String> {
        self.backup_entry(operation)?;
        let fence = SessionFence {
            connection: operation.connection,
            epoch: operation.epoch.clone(),
        };
        if !self.accepts_observation_for(&fence, observation) {
            self.backup
                .as_mut()
                .expect("validated backup")
                .runtime
                .record_owner_rejection("observation-fence");
            return Ok(None);
        }
        let Some(started_at) = self
            .deliveries
            .operation_started_at(operation, crate::profile::MESSAGE_TYPE_LOCAL_BACKUP)
        else {
            self.backup
                .as_mut()
                .expect("validated backup")
                .runtime
                .record_owner_rejection("request-not-dispatched");
            return Ok(None);
        };
        let Some(ingress) = observation.ingress() else {
            self.backup
                .as_mut()
                .expect("validated backup")
                .runtime
                .record_owner_rejection("ingress-missing");
            return Ok(None);
        };
        if ingress.first_report_at_ms < started_at {
            self.backup
                .as_mut()
                .expect("validated backup")
                .runtime
                .record_owner_rejection("ingress-before-dispatch");
            return Ok(None);
        }
        let entry = self.backup.as_mut().expect("validated backup");
        if observation.sequence <= entry.last_observation {
            entry.runtime.record_owner_rejection("observation-replayed");
            return Ok(None);
        }
        entry.last_observation = observation.sequence;
        // Millisecond equality cannot prove relative order; the native timing
        // boundary excludes older ingress but is not a firmware acknowledgement.
        entry.runtime.absorb(now_ms, payload).map(Some)
    }

    /// Advances the active backup transfer.
    ///
    /// Returns the next action (for example a keepalive to send) and whether the transfer has started receiving data. Fails if there is no active backup for `operation` or its session changed.
    pub fn advance_backup(
        &mut self,
        operation: &OperationTicket,
        now_ms: u64,
    ) -> Result<(BackupAction, bool), String> {
        self.backup_entry(operation)?;
        let runtime = &mut self.backup.as_mut().expect("validated backup").runtime;
        let action = runtime.advance(now_ms);
        Ok((action, runtime.started()))
    }

    /// Claim an intentional host recovery exactly once, after a valid document.
    /// The host must claim at its serialized transport teardown boundary, not
    /// when file persistence is merely scheduled. This does not start USB I/O.
    pub fn claim_backup_recovery(&mut self, operation: &OperationTicket) -> Result<(), String> {
        if self.backup_entry(operation)?.runtime.terminal_state()
            != Some(BackupTerminalState::Complete)
        {
            return Err("QC backup recovery requires a completed document".into());
        }
        self.backup = None;
        Ok(())
    }

    /// A delayed cancellation cannot clear a newer attempt, even in one session.
    pub fn cancel_backup(&mut self, operation: &OperationTicket) -> bool {
        if self
            .backup
            .as_ref()
            .is_some_and(|entry| entry.operation == *operation)
        {
            self.backup = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::delivery::ReportOutcome;
    use crate::device::session::SessionDriver;
    use crate::device::transport::IngressTime;

    fn owner() -> (SessionRuntime, u64, OperationTicket) {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "backup".into(), 0));
        let mut operation = owner.plan_operation(0, 180_000).unwrap();
        owner.begin_backup(&mut operation, 0).unwrap();
        (owner, connection, operation)
    }

    fn attempted(owner: &mut SessionRuntime, operation: &OperationTicket, now: u64) {
        let dispatch = owner.prepare_dispatch(Some(operation), None, now).unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, crate::profile::MESSAGE_TYPE_LOCAL_BACKUP, 1, now)
            .unwrap();
        owner.begin_native_report(&delivery, 0, now).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, now)
            .unwrap();
    }

    fn observation(
        owner: &mut SessionRuntime,
        connection: u64,
        first: u64,
        last: u64,
    ) -> ObservationTicket {
        owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: first,
                    last_report_at_ms: last,
                }),
            )
            .unwrap()
    }

    fn chunk(text: &str, last: bool) -> Vec<u8> {
        assert!(text.len() < 128);
        let mut bytes = vec![8, 1, 26, text.len() as u8];
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(&[48, u8::from(last)]);
        bytes
    }

    #[test]
    fn backup_requires_native_attempt_and_rejects_frames_started_before_it() {
        let (mut owner, connection, operation) = owner();
        let bytes = chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true);
        let queued = observation(&mut owner, connection, 1, 2);
        assert!(owner
            .consume_backup(&operation, &queued, &bytes, 3)
            .unwrap()
            .is_none());
        attempted(&mut owner, &operation, 5);
        let spanning = observation(&mut owner, connection, 4, 6);
        assert!(owner
            .consume_backup(&operation, &spanning, &bytes, 7)
            .unwrap()
            .is_none());
        let current = observation(&mut owner, connection, 8, 9);
        let complete = owner
            .consume_backup(&operation, &current, &bytes, 10)
            .unwrap()
            .unwrap();
        assert_eq!(
            complete.document.as_deref(),
            Some("{\"type\":\"backup\",\"creator\":\"quad\"}")
        );
        assert!(owner
            .consume_backup(&operation, &current, &bytes, 11)
            .unwrap()
            .is_none());
    }

    #[test]
    fn timeout_reports_owner_rejection_separately_from_later_ignored_frame() {
        let (mut owner, connection, operation) = owner();
        let pre_dispatch = observation(&mut owner, connection, 1, 2);
        assert!(owner
            .consume_backup(
                &operation,
                &pre_dispatch,
                &chunk("private payload", false),
                3
            )
            .unwrap()
            .is_none());

        attempted(&mut owner, &operation, 5);
        let current = observation(&mut owner, connection, 6, 7);
        let create_echo = {
            let text = "private payload";
            let mut bytes = vec![8, 0, 26, text.len() as u8];
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(&[48, 0]);
            bytes
        };
        let progress = owner
            .consume_backup(&operation, &current, &create_echo, 8)
            .unwrap()
            .unwrap();
        assert!(progress.document.is_none());

        let (action, _started) = owner
            .advance_backup(
                &operation,
                5 + crate::profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS,
            )
            .unwrap();
        let BackupAction::Failed(message) = action else {
            panic!("expected first-chunk timeout");
        };
        assert!(message.contains("type40_observations=2"));
        assert!(message.contains("assembler_frames=1"));
        assert!(message.contains("owner_rejections=1"));
        assert!(message.contains("last_owner_rejection=request-not-dispatched"));
        assert!(message.contains("last_outcome=ignored-action"));
        assert!(!message.contains("private payload"));
    }

    #[test]
    fn timeout_reports_pre_dispatch_ingress_rejection_without_payload() {
        let (mut owner, connection, operation) = owner();
        attempted(&mut owner, &operation, 5);
        let spanning = observation(&mut owner, connection, 4, 6);
        assert!(owner
            .consume_backup(&operation, &spanning, &chunk("private payload", false), 7)
            .unwrap()
            .is_none());

        let (action, _started) = owner
            .advance_backup(
                &operation,
                5 + crate::profile::BACKUP_FIRST_CHUNK_TIMEOUT_MS,
            )
            .unwrap();
        let BackupAction::Failed(message) = action else {
            panic!("expected first-chunk timeout");
        };
        assert!(message.contains("type40_observations=1"));
        assert!(message.contains("assembler_frames=0"));
        assert!(message.contains("owner_rejections=1"));
        assert!(message.contains("last_owner_rejection=ingress-before-dispatch"));
        assert!(!message.contains("private payload"));
    }

    #[test]
    fn duplicate_callback_does_not_append_a_chunk_twice() {
        let (mut owner, connection, operation) = owner();
        attempted(&mut owner, &operation, 0);
        let start = observation(&mut owner, connection, 1, 2);
        let prefix = chunk("{\"type\":\"backup\",", false);
        assert!(owner
            .consume_backup(&operation, &start, &prefix, 3)
            .unwrap()
            .unwrap()
            .document
            .is_none());
        assert!(owner
            .consume_backup(&operation, &start, &prefix, 4)
            .unwrap()
            .is_none());
        assert_eq!(
            owner.advance_backup(&operation, 5).unwrap(),
            (BackupAction::Wait, true)
        );
        let end = observation(&mut owner, connection, 6, 7);
        let complete = owner
            .consume_backup(&operation, &end, &chunk("\"creator\":\"quad\"}", true), 8)
            .unwrap()
            .unwrap();
        assert_eq!(
            complete.document.as_deref(),
            Some("{\"type\":\"backup\",\"creator\":\"quad\"}")
        );
    }

    #[test]
    fn old_cancel_and_watchdog_cannot_mutate_a_new_attempt() {
        let (mut owner, connection, mut old) = owner();
        let mut next = owner.plan_operation(1, 180_000).unwrap();
        assert!(owner.begin_backup(&mut next, 1).is_err());
        let queued = owner.prepare_dispatch(Some(&old), None, 1).unwrap();
        let delivery = owner
            .prepare_delivery(&queued, crate::profile::MESSAGE_TYPE_LOCAL_BACKUP, 1, 1)
            .unwrap();
        assert!(owner.cancel_backup(&old));
        assert!(owner.begin_native_report(&delivery, 0, 2).is_err());
        let record = owner.delivery_record(&delivery).unwrap();
        assert_eq!(
            record.status,
            crate::device::delivery::DeliveryStatus::Cancelled
        );
        assert_eq!(record.delivered_reports, 0);
        assert!(owner.validate_dispatch(&queued, false, 2).is_err());
        assert!(owner.prepare_dispatch(Some(&old), None, 2).is_err());
        assert!(owner.begin_backup(&mut old, 2).is_err());
        owner.begin_backup(&mut next, 2).unwrap();
        assert!(!owner.cancel_backup(&old));
        assert!(owner.advance_backup(&old, 3).is_err());
        attempted(&mut owner, &next, 4);
        let current = observation(&mut owner, connection, 5, 6);
        assert!(owner
            .consume_backup(
                &next,
                &current,
                &chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true),
                7
            )
            .unwrap()
            .unwrap()
            .document
            .is_some());
    }

    #[test]
    fn lifecycle_replacement_discards_partial_stream_and_old_observations() {
        let (mut owner, connection, old) = owner();
        attempted(&mut owner, &old, 0);
        let prefix = observation(&mut owner, connection, 1, 2);
        owner
            .consume_backup(&old, &prefix, &chunk("{\"type\":\"backup\",", false), 3)
            .unwrap();
        let replacement = owner.opened(4).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 4));
        let mut next = owner.plan_operation(4, 180_000).unwrap();
        owner.begin_backup(&mut next, 4).unwrap();
        assert!(!owner.cancel_backup(&old));
        assert!(owner.advance_backup(&old, 5).is_err());
        attempted(&mut owner, &next, 5);
        assert!(owner
            .consume_backup(&next, &prefix, &chunk("{}", true), 6)
            .unwrap()
            .is_none());
        let current = observation(&mut owner, replacement, 7, 8);
        assert!(owner
            .consume_backup(
                &next,
                &current,
                &chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true),
                9
            )
            .unwrap()
            .unwrap()
            .document
            .is_some());
    }

    #[test]
    fn transfer_keeps_original_deadline_despite_late_start() {
        let (mut owner, _, old) = owner();
        owner.cancel_backup(&old);
        let mut operation = owner.plan_operation(10, 20).unwrap();
        owner.begin_backup(&mut operation, 25).unwrap();
        assert_eq!(
            owner.advance_backup(&operation, 29).unwrap().0,
            BackupAction::Wait
        );
        assert!(matches!(
            owner.advance_backup(&operation, 30).unwrap().0,
            BackupAction::Failed(_)
        ));
        assert!(owner.begin_backup(&mut operation, 30).is_err());
    }
    #[test]
    fn recovery_requires_completed_document_and_is_claimed_once() {
        let (mut owner, connection, operation) = owner();
        assert!(owner.claim_backup_recovery(&operation).is_err());
        attempted(&mut owner, &operation, 1);
        let terminal = observation(&mut owner, connection, 2, 3);
        owner
            .consume_backup(
                &operation,
                &terminal,
                &chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true),
                4,
            )
            .unwrap()
            .unwrap();
        owner.claim_backup_recovery(&operation).unwrap();
        assert!(owner.claim_backup_recovery(&operation).is_err());
        assert!(
            owner.accepts_connection(connection),
            "claiming recovery must not itself replace transport ownership"
        );
    }

    #[test]
    fn late_recovery_cannot_consume_a_replacement_backup() {
        let (mut owner, connection, operation) = owner();
        attempted(&mut owner, &operation, 1);
        let terminal = observation(&mut owner, connection, 2, 3);
        owner
            .consume_backup(
                &operation,
                &terminal,
                &chunk("{\"type\":\"backup\",\"creator\":\"quad\"}", true),
                4,
            )
            .unwrap();
        let replacement = owner.opened(5).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 5));
        let mut next = owner.plan_operation(5, 180_000).unwrap();
        owner.begin_backup(&mut next, 5).unwrap();
        assert!(owner.claim_backup_recovery(&operation).is_err());
        assert!(owner.accepts_connection(replacement));
        assert_eq!(
            owner.advance_backup(&next, 6).unwrap().0,
            BackupAction::Wait
        );
        assert!(owner.prepare_dispatch(Some(&next), None, 6).is_ok());
    }
    #[test]
    fn failed_backup_does_not_authorize_recovery() {
        let (mut owner, connection, operation) = owner();
        attempted(&mut owner, &operation, 1);
        let terminal = observation(&mut owner, connection, 2, 3);
        assert!(owner
            .consume_backup(&operation, &terminal, &chunk("{invalid}", true), 4)
            .is_err());
        assert!(owner.claim_backup_recovery(&operation).is_err());
        assert!(owner.accepts_connection(connection));
    }
}
