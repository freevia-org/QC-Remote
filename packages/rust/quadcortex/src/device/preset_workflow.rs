//! Deterministic sequencing for a retained preset mutation plan.
//!
//! This engine owns timing and one-use action receipts. The session-owner
//! adapter must supply native journal outcomes and validated readback outcomes;
//! issuing an action is not evidence of USB delivery or device acknowledgement.
use crate::commands::{DeviceCommand, DeviceOperation};
use crate::device::request::{
    planned_write_dispatch_budget_ms, GatewayVerification, PlannedWrite, PresetMutationPlan,
    PresetMutationStage,
};
use crate::sync::allocate_next_id;

/// Time allowed for a preset mutation plan, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetWorkflowBudget {
    /// Budget for the plan's mutation stages, including any directory catalog verification the plan needs.
    pub execution_ms: u64,
    /// Budget for the plan's cleanup stages, which restore the original preset navigation.
    pub restoration_ms: u64,
}

impl PresetWorkflowBudget {
    /// Computes the budget for `plan` by summing each stage's dispatch, verification and settle times plus its catalog verification timeouts.
    ///
    /// Fails if a cleanup stage is anything other than a setlist-position recall, or if a stage's dispatch budget cannot be computed.
    pub fn for_plan(plan: &PresetMutationPlan) -> Result<Self, String> {
        let mut execution_ms = 0_u64;
        let mut restoration_ms = 0_u64;
        for stage in &plan.stages {
            execution_ms = execution_ms.saturating_add(stage_budget(stage)?);
        }
        for stage in &plan.cleanup_stages {
            // A cleanup action must never replay a persistent write.
            if !matches!(
                stage.write,
                PlannedWrite::HidCommand(DeviceCommand::SetlistPosition { .. })
            ) {
                return Err(
                    "Preset workflow cleanup must be an idempotent navigation recall".into(),
                );
            }
            restoration_ms = restoration_ms.saturating_add(stage_budget(stage)?);
        }
        if let Some(folder) = &plan.folder_copy {
            execution_ms = execution_ms.saturating_add(folder.catalog_policy.timeout_ms);
        }
        if plan.deletion.is_some() || plan.deletes_setlist() {
            execution_ms = execution_ms.saturating_add(plan.catalog_policy.timeout_ms);
        }
        if !plan.saved_presets.is_empty() {
            execution_ms = execution_ms.saturating_add(plan.catalog_policy.timeout_ms);
        }
        Ok(Self {
            execution_ms,
            restoration_ms,
        })
    }
}

fn stage_budget(stage: &PresetMutationStage) -> Result<u64, String> {
    let verification = if matches!(stage.verification, GatewayVerification::None) {
        0
    } else {
        stage.timeout_ms
    };
    Ok(planned_write_dispatch_budget_ms(&stage.write)?
        .saturating_add(verification)
        .saturating_add(stage.settle_ms))
}

/// One-use receipt for an action issued by a [`PresetWorkflowRuntime`].
///
/// It names the workflow and the action's sequence number, and must be presented to `complete` to report the action's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowActionTicket {
    workflow: u64,
    sequence: u64,
}

/// Result of restoring the original preset navigation after a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorationOutcome {
    /// The plan has no cleanup stages.
    NotRequired,
    /// Every cleanup stage ran without failure.
    Completed,
    /// A cleanup stage failed; carries the failure message.
    Failed(String),
    /// Restoration was abandoned, for example because the workflow no longer owned the current preset; carries the reason.
    Skipped(String),
}

/// Terminal result of a [`PresetWorkflowRuntime`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetWorkflowOutcome {
    /// The first error that stopped the mutation, or `None` if none occurred.
    pub primary_error: Option<String>,
    /// Whether catalog verification confirmed the mutation.
    pub catalog_verified: bool,
    /// Set when the catalog verification window ended without a confirming listing and without
    /// contradicting evidence. The frames were delivered, so this is not a failure: the result
    /// is `accepted_unverified` and carries this text. Mutually exclusive with `primary_error`.
    pub catalog_unconfirmed: Option<String>,
    /// How restoration of the original navigation ended.
    pub restoration: RestorationOutcome,
}

/// Next step a [`PresetWorkflowRuntime`] asks its host to perform.
///
/// Times are absolute milliseconds on the same clock as the `now_ms` given to `advance`. Issuing an action is not evidence that it was delivered or acknowledged by the device.
#[derive(Debug, Clone, PartialEq)]
pub enum PresetWorkflowAction {
    /// Submit a planned write to the device.
    Dispatch {
        /// Receipt to pass to `complete` for this action.
        ticket: WorkflowActionTicket,
        /// The write to submit.
        write: PlannedWrite,
        /// `true` when the write is a cleanup (restoration) stage.
        cleanup: bool,
        /// Time by which the dispatch must complete.
        expires_at_ms: u64,
    },
    /// Read the device back to verify the previous write.
    Verify {
        /// Receipt to pass to `complete` for this action.
        ticket: WorkflowActionTicket,
        /// The verification to perform.
        verification: GatewayVerification,
        /// Time by which the readback must complete.
        expires_at_ms: u64,
    },
    /// Catalog reconciliation is a separate action, never an export replay.
    VerifyCatalog {
        /// Receipt to pass to `complete` for this action.
        ticket: WorkflowActionTicket,
        /// Time by which catalog verification must complete.
        expires_at_ms: u64,
    },
    /// A previously issued action is still pending; report its result with its receipt.
    AwaitReceipt {
        /// Receipt of the pending action.
        ticket: WorkflowActionTicket,
        /// Time at which the pending action expires.
        expires_at_ms: u64,
    },
    /// Nothing to do until the given time (for example while a stage settles).
    Wait {
        /// Time at which to call `advance` again.
        until_ms: u64,
    },
    /// The workflow is over; carries its outcome.
    Finished(PresetWorkflowOutcome),
}

/// Result of delivering one native stage, as established by the owner's delivery journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeStageOutcome {
    /// Every frame of the stage completed native delivery.
    Completed,
    /// The owner's delivery journal proves no native attempt began.
    NotAttempted(String),
    /// Receipt is incomplete; detailed native-attempt evidence stays in the
    /// owner journal. This outcome never establishes zero device effect.
    Indeterminate(String),
}

/// Result reported to [`PresetWorkflowRuntime::complete`] for a pending action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetWorkflowCompletion {
    /// Outcome of a dispatched native write.
    Native(NativeStageOutcome),
    /// Outcome of a readback verification.
    Readback(Result<(), String>),
    /// Outcome of catalog verification.
    Catalog(Result<(), String>),
    /// The catalog verification window ended with no confirming listing and no contradicting
    /// evidence (the message says what was observed). The write is reported unverified.
    CatalogUnconfirmed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    cleanup: bool,
    index: usize,
}
impl Cursor {
    fn next(self) -> Self {
        Self {
            index: self.index + 1,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Pending {
    Native(Cursor),
    Readback(Cursor),
    Catalog,
}
#[derive(Debug, Clone)]
enum Phase {
    Ready(Cursor),
    Verify(Cursor),
    Catalog,
    Pending {
        ticket: WorkflowActionTicket,
        action: Pending,
        expires_at_ms: u64,
    },
    Settling {
        until_ms: u64,
        next: Cursor,
    },
    Finished,
}

/// Deterministic sequencer for one retained preset mutation plan.
///
/// Call `advance` to obtain the next [`PresetWorkflowAction`], perform it, and report its result with `complete`. After a failed mutation stage the runtime proceeds to the plan's cleanup stages.
pub struct PresetWorkflowRuntime {
    id: u64,
    next_sequence: u64,
    plan: PresetMutationPlan,
    phase: Phase,
    execution_deadline_ms: u64,
    restoration_deadline_ms: u64,
    outcome: PresetWorkflowOutcome,
}

impl PresetWorkflowRuntime {
    /// The owner supplies a non-reused workflow ID and the original clock.
    /// Preparation must eventually be included in that owner's outer scope;
    /// this core only derives the concrete retained plan's phase ceilings.
    pub fn new(id: u64, plan: PresetMutationPlan, started_ms: u64) -> Result<Self, String> {
        Self::new_with_preparation(id, plan, started_ms, 0)
    }

    pub(crate) fn new_with_preparation(
        id: u64,
        plan: PresetMutationPlan,
        started_ms: u64,
        preparation_budget_ms: u64,
    ) -> Result<Self, String> {
        if id == 0 {
            return Err("Invalid preset workflow identity".into());
        }
        let budget = PresetWorkflowBudget::for_plan(&plan)?;
        let execution_deadline_ms = started_ms
            .saturating_add(preparation_budget_ms)
            .saturating_add(budget.execution_ms);
        let restoration_deadline_ms = execution_deadline_ms.saturating_add(budget.restoration_ms);
        let restoration = if plan.cleanup_stages.is_empty() {
            RestorationOutcome::NotRequired
        } else {
            RestorationOutcome::Completed
        };
        Ok(Self {
            id,
            next_sequence: 1,
            plan,
            phase: Phase::Ready(Cursor {
                cleanup: false,
                index: 0,
            }),
            execution_deadline_ms,
            restoration_deadline_ms,
            outcome: PresetWorkflowOutcome {
                primary_error: None,
                catalog_verified: false,
                catalog_unconfirmed: None,
                restoration,
            },
        })
    }

    /// Returns `true` once the workflow has finished.
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, Phase::Finished)
    }

    /// Intersect fixed phase ceilings with the original preparation scope.
    pub fn restrict_deadline(&mut self, deadline: crate::device::correlation::RequestDeadline) {
        self.execution_deadline_ms = self.execution_deadline_ms.min(deadline.expires_at_ms());
        self.restoration_deadline_ms = self.restoration_deadline_ms.min(deadline.expires_at_ms());
    }

    /// Returns the retained mutation plan.
    pub fn plan(&self) -> &PresetMutationPlan {
        &self.plan
    }

    fn stage(&self, cursor: Cursor) -> &PresetMutationStage {
        if cursor.cleanup {
            &self.plan.cleanup_stages[cursor.index]
        } else {
            &self.plan.stages[cursor.index]
        }
    }
    fn deadline(&self, cursor: Cursor) -> u64 {
        if cursor.cleanup {
            self.restoration_deadline_ms
        } else {
            self.execution_deadline_ms
        }
    }
    fn cleanup(&self) -> Cursor {
        Cursor {
            cleanup: true,
            index: 0,
        }
    }
    fn needs_catalog(&self) -> bool {
        self.plan.needs_directory_catalog()
            || self.plan.deletion.is_some()
            || !self.plan.saved_presets.is_empty()
    }
    fn issue(
        &mut self,
        action: Pending,
        expires_at_ms: u64,
    ) -> Result<WorkflowActionTicket, String> {
        let sequence = allocate_next_id(&mut self.next_sequence, "Preset workflow action")?;
        let ticket = WorkflowActionTicket {
            workflow: self.id,
            sequence,
        };
        self.phase = Phase::Pending {
            ticket,
            action,
            expires_at_ms,
        };
        Ok(ticket)
    }
    fn failure(&mut self, cursor: Cursor, message: String) -> Cursor {
        if cursor.cleanup {
            if !matches!(self.outcome.restoration, RestorationOutcome::Failed(_)) {
                self.outcome.restoration = RestorationOutcome::Failed(message);
            }
            cursor.next()
        } else {
            if self.outcome.primary_error.is_none() {
                self.outcome.primary_error = Some(message);
            }
            self.cleanup()
        }
    }
    fn settled(&mut self, cursor: Cursor, next: Cursor, now_ms: u64, attempted: bool) {
        let settle_ms = if attempted {
            self.stage(cursor).settle_ms
        } else {
            0
        };
        self.phase = Phase::Settling {
            until_ms: now_ms.saturating_add(settle_ms),
            next,
        };
    }
    fn finish_readback(&mut self, cursor: Cursor, result: Result<(), String>, now_ms: u64) {
        let compatible_save = !cursor.cleanup
            && self.needs_catalog()
            && matches!(
                self.stage(cursor).write,
                PlannedWrite::HidOperation(DeviceOperation::SavePreset { .. })
            );
        let next = match result {
            Ok(()) => cursor.next(),
            Err(_) if compatible_save => cursor.next(),
            Err(message) => self.failure(cursor, message),
        };
        self.settled(cursor, next, now_ms, true);
    }

    /// Returns the next action given the current time in milliseconds.
    ///
    /// Repeated calls while an action is pending return [`PresetWorkflowAction::AwaitReceipt`]; a pending action whose deadline has passed is recorded as failed or indeterminate. Fails if a stage's dispatch budget cannot be computed or the action sequence is exhausted.
    pub fn advance(&mut self, now_ms: u64) -> Result<PresetWorkflowAction, String> {
        loop {
            match self.phase.clone() {
                Phase::Pending {
                    ticket,
                    action,
                    expires_at_ms,
                } => {
                    if now_ms < expires_at_ms {
                        return Ok(PresetWorkflowAction::AwaitReceipt {
                            ticket,
                            expires_at_ms,
                        });
                    }
                    let error = "Preset workflow action deadline expired".to_string();
                    let completion = match action {
                        Pending::Native(_) => PresetWorkflowCompletion::Native(
                            NativeStageOutcome::Indeterminate(error),
                        ),
                        Pending::Readback(_) => PresetWorkflowCompletion::Readback(Err(error)),
                        Pending::Catalog => PresetWorkflowCompletion::CatalogUnconfirmed(error),
                    };
                    self.complete(ticket, completion, now_ms)?;
                }
                Phase::Finished => return Ok(PresetWorkflowAction::Finished(self.outcome.clone())),
                Phase::Settling { until_ms, next } => {
                    if now_ms < until_ms {
                        return Ok(PresetWorkflowAction::Wait { until_ms });
                    }
                    self.phase = Phase::Ready(next);
                }
                Phase::Ready(cursor) => {
                    let length = if cursor.cleanup {
                        self.plan.cleanup_stages.len()
                    } else {
                        self.plan.stages.len()
                    };
                    if cursor.index >= length {
                        self.phase = if cursor.cleanup {
                            Phase::Finished
                        } else {
                            Phase::Catalog
                        };
                        continue;
                    }
                    if now_ms >= self.deadline(cursor) {
                        let next = self.failure(
                            cursor,
                            "Preset workflow budget expired before native dispatch".into(),
                        );
                        self.phase = Phase::Ready(next);
                        continue;
                    }
                    let write = self.stage(cursor).write.clone();
                    let expires_at_ms = now_ms
                        .saturating_add(planned_write_dispatch_budget_ms(&write)?)
                        .min(self.deadline(cursor));
                    let ticket = self.issue(Pending::Native(cursor), expires_at_ms)?;
                    return Ok(PresetWorkflowAction::Dispatch {
                        ticket,
                        write,
                        cleanup: cursor.cleanup,
                        expires_at_ms,
                    });
                }
                Phase::Verify(cursor) => {
                    if now_ms >= self.deadline(cursor) {
                        self.finish_readback(
                            cursor,
                            Err("Preset workflow budget expired before readback".into()),
                            now_ms,
                        );
                        continue;
                    }
                    let verification = self.stage(cursor).verification.clone();
                    let expires_at_ms = now_ms
                        .saturating_add(self.stage(cursor).timeout_ms)
                        .min(self.deadline(cursor));
                    let ticket = self.issue(Pending::Readback(cursor), expires_at_ms)?;
                    return Ok(PresetWorkflowAction::Verify {
                        ticket,
                        verification,
                        expires_at_ms,
                    });
                }
                Phase::Catalog => {
                    if !self.needs_catalog() {
                        self.phase = Phase::Ready(self.cleanup());
                        continue;
                    }
                    if now_ms >= self.execution_deadline_ms {
                        self.outcome.primary_error = Some(
                            "Preset workflow budget expired before catalog verification".into(),
                        );
                        self.phase = Phase::Ready(self.cleanup());
                        continue;
                    }
                    let expires_at_ms = self.execution_deadline_ms;
                    let ticket = self.issue(Pending::Catalog, expires_at_ms)?;
                    return Ok(PresetWorkflowAction::VerifyCatalog {
                        ticket,
                        expires_at_ms,
                    });
                }
            }
        }
    }

    /// Reports the result of the pending action identified by `ticket`.
    ///
    /// Fails if no action is pending, if the ticket is stale or belongs to another action, or if the completion kind does not match the pending action. A result arriving after the action's deadline is recorded as a failure.
    pub fn complete(
        &mut self,
        ticket: WorkflowActionTicket,
        completion: PresetWorkflowCompletion,
        now_ms: u64,
    ) -> Result<(), String> {
        let Phase::Pending {
            ticket: expected,
            action,
            expires_at_ms,
        } = self.phase.clone()
        else {
            return Err("Preset workflow receipt has no pending action".into());
        };
        if ticket != expected {
            return Err("Preset workflow receipt is stale or belongs to another action".into());
        }
        match (action, completion) {
            (Pending::Native(cursor), PresetWorkflowCompletion::Native(result)) => match result {
                NativeStageOutcome::Completed if now_ms < expires_at_ms => {
                    if matches!(self.stage(cursor).verification, GatewayVerification::None) {
                        self.settled(cursor, cursor.next(), now_ms, true);
                    } else {
                        self.phase = Phase::Verify(cursor);
                    }
                }
                NativeStageOutcome::NotAttempted(error) => {
                    let next = self.failure(cursor, error);
                    self.settled(cursor, next, now_ms, false);
                }
                result => {
                    let error = match result {
                        NativeStageOutcome::Indeterminate(error) => error,
                        _ => "Native preset stage completion arrived after its deadline".into(),
                    };
                    let next = self.failure(cursor, error);
                    self.settled(cursor, next, now_ms, true);
                }
            },
            (Pending::Readback(cursor), PresetWorkflowCompletion::Readback(result)) => {
                let result = if now_ms >= expires_at_ms {
                    Err("Preset stage readback arrived after its deadline".into())
                } else {
                    result
                };
                self.finish_readback(cursor, result, now_ms);
            }
            (Pending::Catalog, PresetWorkflowCompletion::Catalog(result)) => {
                if now_ms >= expires_at_ms {
                    self.outcome.catalog_unconfirmed =
                        Some("Preset catalog verification arrived after its deadline".into());
                } else {
                    match result {
                        Ok(()) => self.outcome.catalog_verified = true,
                        Err(message) => self.outcome.primary_error = Some(message),
                    }
                }
                self.phase = Phase::Ready(self.cleanup());
            }
            (Pending::Catalog, PresetWorkflowCompletion::CatalogUnconfirmed(message)) => {
                self.outcome.catalog_unconfirmed = Some(message);
                self.phase = Phase::Ready(self.cleanup());
            }
            _ => {
                return Err(
                    "Preset workflow receipt kind does not match the pending action".into(),
                );
            }
        }
        Ok(())
    }

    /// The owner calls this when session/checkpoint ownership is lost. A stale
    /// workflow never restores navigation over a replacement owner.
    pub fn invalidate(&mut self, reason: String) {
        if matches!(self.phase, Phase::Finished) {
            return;
        }
        let restoring = match &self.phase {
            Phase::Ready(cursor) | Phase::Verify(cursor) => cursor.cleanup,
            Phase::Pending {
                action: Pending::Native(cursor) | Pending::Readback(cursor),
                ..
            } => cursor.cleanup,
            Phase::Settling { next, .. } => next.cleanup,
            _ => false,
        };
        if !restoring && self.outcome.primary_error.is_none() {
            self.outcome.primary_error = Some(reason.clone());
        }
        if !self.plan.cleanup_stages.is_empty() {
            self.outcome.restoration = RestorationOutcome::Skipped(reason);
        }
        self.phase = Phase::Finished;
    }

    /// Restoration is separate from mutation. Losing navigation ownership
    /// during cleanup must not turn a verified mutation into a primary failure.
    pub fn skip_restoration(&mut self, reason: String) -> Result<(), String> {
        let cleanup = match &self.phase {
            Phase::Ready(cursor) => cursor.cleanup,
            Phase::Pending {
                action: Pending::Native(cursor),
                ..
            } => cursor.cleanup,
            _ => false,
        };
        if !cleanup {
            return Err("Preset workflow is not awaiting restoration".into());
        }
        self.outcome.restoration = RestorationOutcome::Skipped(reason);
        self.phase = Phase::Finished;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::catalog::CatalogVerificationPolicy;
    use crate::device::request::PresetMutationRecord;

    fn plan() -> PresetMutationPlan {
        PresetMutationPlan {
            stages: vec![PresetMutationStage {
                write: PlannedWrite::HidOperation(DeviceOperation::SavePreset {
                    setlist_key: "/media/p4/Presets/Test".into(),
                    position: 2,
                    name: "Saved".into(),
                    instrument: 0,
                }),
                verification: GatewayVerification::Preset {
                    setlist_key: "/media/p4/Presets/Test".into(),
                    position: 2,
                    name: Some("Saved".into()),
                    require_clean: false,
                    after_position_revision: None,
                    after_full_preset_revision: None,
                },
                timeout_ms: 100,
                settle_ms: 30,
            }],
            cleanup_stages: vec![PresetMutationStage {
                write: PlannedWrite::HidCommand(DeviceCommand::SetlistPosition {
                    is_factory: false,
                    setlist_key: "/media/p4/Presets/Test".into(),
                    position: 1,
                }),
                verification: GatewayVerification::Preset {
                    setlist_key: "/media/p4/Presets/Test".into(),
                    position: 1,
                    name: None,
                    require_clean: true,
                    after_position_revision: None,
                    after_full_preset_revision: None,
                },
                timeout_ms: 50,
                settle_ms: 0,
            }],
            detail: "save and restore".into(),
            saved_name: "Saved".into(),
            setlist_key: "/media/p4/Presets/Test".into(),
            position: 2,
            instrument: 0,
            catalog_policy: CatalogVerificationPolicy {
                timeout_ms: 200,
                passive_window_ms: 0,
                max_requests: 100,
                initial_retry_ms: 10,
                maximum_retry_ms: 20,
            },
            saved_presets: vec![PresetMutationRecord {
                setlist_key: "/media/p4/Presets/Test".into(),
                position: 2,
                name: "Saved".into(),
                instrument: 0,
                allow_collision_suffix: true,
            }],
            folder_copy: None,
            setlist_deletion: None,
            deletion: None,
        }
    }
    fn dispatch(
        runtime: &mut PresetWorkflowRuntime,
        now: u64,
        cleanup_expected: bool,
    ) -> WorkflowActionTicket {
        let PresetWorkflowAction::Dispatch {
            ticket, cleanup, ..
        } = runtime.advance(now).unwrap()
        else {
            panic!("expected native stage");
        };
        assert_eq!(cleanup, cleanup_expected);
        ticket
    }
    fn verify(runtime: &mut PresetWorkflowRuntime, now: u64) -> WorkflowActionTicket {
        let PresetWorkflowAction::Verify { ticket, .. } = runtime.advance(now).unwrap() else {
            panic!("expected readback");
        };
        ticket
    }

    #[test]
    fn save_catalog_and_restoration_are_ordered_without_replaying_create() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let native = dispatch(&mut runtime, 0, false);
        assert!(
            matches!(runtime.advance(1).unwrap(), PresetWorkflowAction::AwaitReceipt { ticket, .. } if ticket == native)
        );
        runtime
            .complete(
                native,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                2,
            )
            .unwrap();
        let readback = verify(&mut runtime, 3);
        runtime
            .complete(readback, PresetWorkflowCompletion::Readback(Ok(())), 4)
            .unwrap();
        assert_eq!(
            runtime.advance(33).unwrap(),
            PresetWorkflowAction::Wait { until_ms: 34 }
        );
        let PresetWorkflowAction::VerifyCatalog {
            ticket: catalog, ..
        } = runtime.advance(34).unwrap()
        else {
            panic!("catalog must precede restoration");
        };
        runtime
            .complete(catalog, PresetWorkflowCompletion::Catalog(Ok(())), 35)
            .unwrap();
        let restore = dispatch(&mut runtime, 36, true);
        runtime
            .complete(
                restore,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                37,
            )
            .unwrap();
        let restored = verify(&mut runtime, 38);
        runtime
            .complete(restored, PresetWorkflowCompletion::Readback(Ok(())), 39)
            .unwrap();
        let PresetWorkflowAction::Finished(outcome) = runtime.advance(39).unwrap() else {
            panic!("expected completion");
        };
        assert!(outcome.primary_error.is_none());
        assert!(outcome.catalog_verified);
        assert_eq!(outcome.restoration, RestorationOutcome::Completed);
    }

    #[test]
    fn duplicate_wrong_kind_and_foreign_receipts_do_not_advance_pending_action() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let mut other = PresetWorkflowRuntime::new(2, plan(), 0).unwrap();
        let first = dispatch(&mut runtime, 0, false);
        let foreign = dispatch(&mut other, 0, false);
        assert!(runtime
            .complete(
                foreign,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                1
            )
            .is_err());
        assert!(runtime
            .complete(first, PresetWorkflowCompletion::Catalog(Ok(())), 1)
            .is_err());
        runtime
            .complete(
                first,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                2,
            )
            .unwrap();
        let second = verify(&mut runtime, 3);
        assert!(runtime
            .complete(
                first,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                4
            )
            .is_err());
        assert!(
            matches!(runtime.advance(4).unwrap(), PresetWorkflowAction::AwaitReceipt { ticket, .. } if ticket == second)
        );
    }

    #[test]
    fn uncertain_native_failure_waits_for_storage_before_cleanup() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let native = dispatch(&mut runtime, 0, false);
        runtime
            .complete(
                native,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Indeterminate(
                    "link failed".into(),
                )),
                10,
            )
            .unwrap();
        assert_eq!(
            runtime.advance(39).unwrap(),
            PresetWorkflowAction::Wait { until_ms: 40 }
        );
        dispatch(&mut runtime, 40, true);
        assert!(!runtime.outcome.catalog_verified);
        assert_eq!(
            runtime.outcome.primary_error.as_deref(),
            Some("link failed")
        );
    }

    #[test]
    fn missing_native_receipt_expires_and_late_completion_cannot_revive_it() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let native = dispatch(&mut runtime, 0, false);
        let deadline = planned_write_dispatch_budget_ms(&runtime.plan.stages[0].write).unwrap();
        assert_eq!(
            runtime.advance(deadline).unwrap(),
            PresetWorkflowAction::Wait {
                until_ms: deadline + 30
            }
        );
        assert!(runtime
            .complete(
                native,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                deadline + 1
            )
            .is_err());
        dispatch(&mut runtime, deadline + 30, true);
    }

    #[test]
    fn save_readback_timeout_uses_catalog_without_replaying_the_write() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let native = dispatch(&mut runtime, 0, false);
        runtime
            .complete(
                native,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                1,
            )
            .unwrap();
        let readback = verify(&mut runtime, 2);
        assert_eq!(
            runtime.advance(102).unwrap(),
            PresetWorkflowAction::Wait { until_ms: 132 }
        );
        assert!(runtime
            .complete(readback, PresetWorkflowCompletion::Readback(Ok(())), 103)
            .is_err());
        assert!(matches!(
            runtime.advance(132).unwrap(),
            PresetWorkflowAction::VerifyCatalog { .. }
        ));
        assert!(runtime.outcome.primary_error.is_none());
    }

    #[test]
    fn fixed_deadlines_do_not_shorten_settlement_or_renew_restoration_time() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let end = runtime.restoration_deadline_ms;
        let native = dispatch(&mut runtime, 0, false);
        runtime
            .complete(
                native,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Indeterminate(
                    "late native result".into(),
                )),
                end - 10,
            )
            .unwrap();
        assert_eq!(
            runtime.advance(end).unwrap(),
            PresetWorkflowAction::Wait { until_ms: end + 20 }
        );
        let PresetWorkflowAction::Finished(outcome) = runtime.advance(end + 20).unwrap() else {
            panic!("expired cleanup must not dispatch");
        };
        assert!(matches!(outcome.restoration, RestorationOutcome::Failed(_)));
        assert_eq!(runtime.restoration_deadline_ms, end);
    }

    #[test]
    fn lost_session_skips_cleanup_and_rejects_old_receipts() {
        let mut runtime = PresetWorkflowRuntime::new(1, plan(), 0).unwrap();
        let native = dispatch(&mut runtime, 0, false);
        runtime.invalidate("session replaced".into());
        assert!(runtime
            .complete(
                native,
                PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                1
            )
            .is_err());
        let PresetWorkflowAction::Finished(outcome) = runtime.advance(2).unwrap() else {
            panic!("must be terminal");
        };
        assert_eq!(
            outcome.restoration,
            RestorationOutcome::Skipped("session replaced".into())
        );
    }

    #[test]
    fn skipping_cleanup_preserves_the_primary_result_and_rejects_late_native_receipts() {
        for failed in [false, true] {
            let mut plan = plan();
            plan.stages[0].verification = GatewayVerification::None;
            let mut runtime = PresetWorkflowRuntime::new(1, plan, 0).unwrap();
            assert!(runtime.skip_restoration("too early".into()).is_err());
            let native = dispatch(&mut runtime, 0, false);
            assert!(runtime.skip_restoration("primary pending".into()).is_err());
            runtime
                .complete(
                    native,
                    PresetWorkflowCompletion::Native(if failed {
                        NativeStageOutcome::Indeterminate("primary failure".into())
                    } else {
                        NativeStageOutcome::Completed
                    }),
                    2,
                )
                .unwrap();
            if !failed {
                let PresetWorkflowAction::VerifyCatalog { ticket, .. } =
                    runtime.advance(32).unwrap()
                else {
                    panic!("expected catalog");
                };
                runtime
                    .complete(ticket, PresetWorkflowCompletion::Catalog(Ok(())), 33)
                    .unwrap();
            }
            let cleanup = dispatch(&mut runtime, 34, true);
            runtime.skip_restoration("user navigated".into()).unwrap();
            assert!(runtime
                .complete(
                    cleanup,
                    PresetWorkflowCompletion::Native(NativeStageOutcome::Completed),
                    35
                )
                .is_err());
            let PresetWorkflowAction::Finished(outcome) = runtime.advance(36).unwrap() else {
                panic!("expected finished");
            };
            assert_eq!(
                outcome.primary_error.as_deref(),
                failed.then_some("primary failure")
            );
            assert_eq!(outcome.catalog_verified, !failed);
            assert_eq!(
                outcome.restoration,
                RestorationOutcome::Skipped("user navigated".into())
            );
        }
    }

    #[test]
    fn cleanup_cannot_contain_a_second_persistent_write() {
        let mut unsafe_plan = plan();
        unsafe_plan.cleanup_stages = unsafe_plan.stages.clone();
        assert!(PresetWorkflowRuntime::new(1, unsafe_plan, 0).is_err());
    }
}
