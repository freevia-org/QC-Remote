//! Retained workflow identity and native evidence boundary for both hosts.
//! This owner never accepts a host's assertion of delivery.
mod catalog_guard;
mod preparation;
use catalog_guard::CatalogGuard;
mod result;
use super::{DispatchContext, ObservationTicket, OperationTicket, SessionRuntime};
use crate::device::catalog::CatalogVerificationAction;
use crate::device::preset_catalog::{PresetCatalogResolution, PresetMutationCatalog};
use crate::device::preset_workflow::{
    NativeStageOutcome, PresetWorkflowAction, PresetWorkflowCompletion, PresetWorkflowRuntime,
    WorkflowActionTicket,
};
use crate::device::request::{
    gateway_verification_policy, GatewayVerificationAction, GatewayVerificationRuntime,
    PlannedWrite, PresetMutationPlan,
};
use crate::sync::next_identity;
pub use preparation::OwnedPresetPreparationAction;
pub(super) use preparation::PresetPreparationEntry;
use prost::Message as _;
pub use result::{PersistentDelivery, PresetWorkflowResult};

/// Next step of a preset workflow, together with the owner-issued evidence the host needs to carry it out.
///
/// Returned by `advance_preset_workflow`.
pub struct OwnedPresetWorkflowAction {
    /// The workflow action to perform.
    pub action: PresetWorkflowAction,
    /// Present only on the first issuance of a native action. Repeated polls
    /// return AwaitReceipt, never a second operation or persistent write.
    pub operation: Option<OperationTicket>,
    /// For a pending readback, the read the host must run.
    pub readback: Option<OwnedPresetWorkflowReadback>,
    /// For a pending catalog verification, the catalog read the host must run.
    pub catalog: Option<OwnedPresetWorkflowCatalog>,
}

/// Readback the host must perform for a pending workflow verification.
pub struct OwnedPresetWorkflowReadback {
    /// Receipt of the workflow action this readback belongs to.
    pub ticket: WorkflowActionTicket,
    /// Step the verification asks for, such as waiting or refreshing.
    pub action: GatewayVerificationAction,
    /// Session context the read must run in.
    pub context: DispatchContext,
    /// Milliseconds left before the readback's deadline.
    pub remaining_ms: u64,
}

/// Catalog read the host must perform for a pending workflow catalog verification.
pub struct OwnedPresetWorkflowCatalog {
    /// Receipt of the workflow action this verification belongs to.
    pub ticket: WorkflowActionTicket,
    /// Step the catalog verification asks for, such as requesting the catalog or waiting.
    pub action: CatalogVerificationAction,
    /// Session context the read must run in.
    pub context: DispatchContext,
    /// Milliseconds left before the catalog verification's deadline.
    pub remaining_ms: u64,
}

struct CatalogAction {
    ticket: WorkflowActionTicket,
    runtime: PresetMutationCatalog,
    context: DispatchContext,
    expires_at_ms: u64,
}

struct ReadbackEvidence {
    observation_floor: u64,
    native_started_ms: Option<u64>,
}

struct ReadbackAction {
    ticket: WorkflowActionTicket,
    runtime: GatewayVerificationRuntime,
    context: DispatchContext,
    expires_at_ms: u64,
}

#[derive(Clone)]
struct SourceCheckpoint {
    source: Option<crate::state::PresetSourceSnapshot>,
    setlist_key: String,
    position: u32,
    scene: u32,
    dirty: bool,
}

impl SourceCheckpoint {
    fn capture(state: &crate::device::state_runtime::DeviceStateRuntime) -> Self {
        let snapshot = state.snapshot();
        Self {
            source: state.preset_source_snapshot(),
            setlist_key: snapshot.setlist_key.clone(),
            position: snapshot.preset_position,
            scene: snapshot.active_scene,
            dirty: snapshot.dirty,
        }
    }
    fn matches(&self, state: &crate::device::state_runtime::DeviceStateRuntime) -> bool {
        let snapshot = state.snapshot();
        self.source
            .as_ref()
            .is_some_and(|source| state.preset_source_matches(source))
            && snapshot.has_preset
            && self
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(snapshot.setlist_key.trim_end_matches('/'))
            && self.position == snapshot.preset_position
            && self.scene == snapshot.active_scene
            && self.dirty == snapshot.dirty
    }
}

struct NativeAction {
    ticket: WorkflowActionTicket,
    operation: OperationTicket,
    source: Option<SourceCheckpoint>,
    source_catalog_name: Option<String>,
    cleanup: bool,
    navigation_target: Option<(String, u32)>,
    save_target: bool,
    persistent: bool,
    message_types: Vec<u16>,
    observation_floor: u64,
    expires_at_ms: u64,
}

struct SaveTransition {
    source: SourceCheckpoint,
    source_catalog_name: Option<String>,
    target: (String, u32),
    started_ms: u64,
    observation_floor: u64,
}

pub(super) struct PresetWorkflowEntry {
    id: u64,
    context: DispatchContext,
    runtime: PresetWorkflowRuntime,
    native: Option<NativeAction>,
    readback: Option<ReadbackAction>,
    catalog: Option<CatalogAction>,
    catalog_result: Option<PresetCatalogResolution>,
    readback_evidence: Option<ReadbackEvidence>,
    observed: Option<(
        ObservationTicket,
        crate::device::GatewaySnapshot,
        SourceCheckpoint,
    )>,
    source_checkpoint: SourceCheckpoint,
    position_observed: Option<(ObservationTicket, SourceCheckpoint)>,
    expected_navigation: Option<(String, u32)>,
    save_transition: Option<SaveTransition>,
    navigation_lost: bool,
    persistent_delivery: PersistentDelivery,
    verified_snapshot: Option<(crate::device::GatewaySnapshot, SourceCheckpoint)>,
    result: Option<PresetWorkflowResult>,
    catalog_guard: Option<CatalogGuard>,
    /// File publications the unit pushed after the native attempt began but before the
    /// catalog stage existed (a save settles and reads back first). The stage replays them
    /// when it starts, so a push that arrived early still counts as evidence: verification
    /// is passive-first. Bounded; overflowing it only drops the buffer, which leaves the
    /// bounded directory READs to confirm.
    early_file_observation_bytes: usize,
    early_file_observations: Vec<(u64, Option<crate::device::transport::IngressTime>, Vec<u8>)>,
}

/// Most File publications buffered for the catalog stage before it exists.
const EARLY_FILE_OBSERVATION_LIMIT: usize = 4096;
// Operational serialized-envelope budget, independent of the per-frame wire cap.
// Retain bytes rather than decoded messages: repeated empty protobuf records can
// have much larger heap footprints than their encoded representation.
const EARLY_FILE_OBSERVATION_BYTES: usize = 32 * 1024 * 1024;

impl SessionRuntime {
    /// Install a prepared shared plan with its original context and clock.
    /// Public host requests use begin_preset_preparation to acquire source and
    /// catalog evidence before this workflow can be installed.
    pub fn begin_preset_workflow(
        &mut self,
        context: &DispatchContext,
        plan: PresetMutationPlan,
        started_ms: u64,
    ) -> Result<u64, String> {
        self.install_preset_workflow(context, plan, started_ms, 0)
    }

    fn install_preset_workflow(
        &mut self,
        context: &DispatchContext,
        plan: PresetMutationPlan,
        started_ms: u64,
        preparation_budget_ms: u64,
    ) -> Result<u64, String> {
        if self
            .preset_preparation
            .as_ref()
            .is_some_and(|entry| entry.is_active())
        {
            return Err("A QC preset preparation is already active".into());
        }
        if !self.accepts_fence(context.session_fence()) {
            return Err("QC preset workflow session changed during preparation".into());
        }
        if self.preset_workflow.as_ref().is_some_and(|entry| {
            !entry.runtime.is_terminal()
                || entry
                    .native
                    .as_ref()
                    .is_some_and(|native| self.deliveries.operation_in_flight(&native.operation))
        }) {
            return Err("A QC preset workflow is already active".into());
        }
        let original = self.state.snapshot();
        for stage in &plan.cleanup_stages {
            if !matches!(&stage.write,
                PlannedWrite::HidCommand(crate::commands::DeviceCommand::SetlistPosition { setlist_key, position, is_factory })
                if setlist_key.trim_end_matches('/').eq_ignore_ascii_case(original.setlist_key.trim_end_matches('/'))
                    && *position == original.preset_position && *is_factory == original.setlist_key.starts_with("/opt/")
            ) {
                return Err(
                    "Preset workflow cleanup must recall its original navigation address".into(),
                );
            }
        }
        let id = self.next_workflow;
        let next = next_identity(id, u64::MAX, "QC preset workflow")?;
        let mut runtime = PresetWorkflowRuntime::new_with_preparation(
            id,
            plan,
            started_ms,
            preparation_budget_ms,
        )?;
        if let Some(deadline) = context.deadline {
            runtime.restrict_deadline(deadline);
        }
        self.preset_workflow = Some(PresetWorkflowEntry {
            id,
            context: context.clone(),
            runtime,
            native: None,
            readback: None,
            catalog: None,
            catalog_result: None,
            readback_evidence: None,
            observed: None,
            source_checkpoint: SourceCheckpoint::capture(&self.state),
            position_observed: None,
            expected_navigation: None,
            save_transition: None,
            navigation_lost: false,
            persistent_delivery: PersistentDelivery::NotAttempted,
            verified_snapshot: None,
            result: None,
            catalog_guard: None,
            early_file_observation_bytes: 0,
            early_file_observations: Vec::new(),
        });
        self.next_workflow = next;
        Ok(id)
    }

    pub(super) fn observe_preset_workflow_snapshot(&mut self, ticket: &ObservationTicket) {
        if let Some(entry) = self
            .preset_workflow
            .as_mut()
            .filter(|entry| !entry.runtime.is_terminal())
        {
            entry.observed = Some((
                ticket.clone(),
                self.state.snapshot().clone(),
                SourceCheckpoint::capture(&self.state),
            ));
        }
    }

    pub(super) fn observe_preset_workflow_position(&mut self, ticket: &ObservationTicket) {
        if let Some(entry) = self
            .preset_workflow
            .as_mut()
            .filter(|entry| !entry.runtime.is_terminal())
        {
            let checkpoint = SourceCheckpoint::capture(&self.state);
            let current = (&checkpoint.setlist_key, checkpoint.position);
            let matches = |target: &(String, u32)| {
                target
                    .0
                    .trim_end_matches('/')
                    .eq_ignore_ascii_case(current.0.trim_end_matches('/'))
                    && target.1 == current.1
            };
            let pending_target = entry
                .native
                .as_ref()
                .filter(|native| self.deliveries.operation_started(&native.operation))
                .and_then(|native| native.navigation_target.as_ref());
            if !matches(&(
                entry.source_checkpoint.setlist_key.clone(),
                entry.source_checkpoint.position,
            )) && !pending_target.is_some_and(matches)
                && !entry.expected_navigation.as_ref().is_some_and(matches)
            {
                entry.navigation_lost = true;
            }
            entry.position_observed = Some((ticket.clone(), checkpoint));
        }
    }

    /// Returns the catalog resolution the workflow has verified so far, or `None` if there is none.
    ///
    /// Fails if workflow `id` is no longer the active workflow.
    pub fn preset_workflow_catalog_result(
        &self,
        id: u64,
    ) -> Result<Option<PresetCatalogResolution>, String> {
        let entry = self
            .preset_workflow
            .as_ref()
            .filter(|entry| entry.id == id)
            .ok_or("QC preset workflow identity is no longer active")?;
        Ok(entry.catalog_result.clone())
    }

    pub(super) fn observe_preset_workflow_catalog(
        &mut self,
        ticket: &ObservationTicket,
        observation: &crate::device::state_runtime::StateObservation,
    ) {
        let Some(message) = &observation.file_message else {
            return;
        };
        if let Some(entry) = self
            .preset_workflow
            .as_mut()
            .filter(|entry| !entry.runtime.is_terminal())
        {
            if ticket.connection != entry.context.session.connection
                || ticket.epoch.as_ref() != Some(&entry.context.session.epoch)
            {
                return;
            }
            if entry.persistent_delivery == PersistentDelivery::NotAttempted {
                if let Some(guard) = &mut entry.catalog_guard {
                    if (!observation.native_catalog_change.updated.is_empty()
                        || !observation.native_catalog_change.removed.is_empty())
                        && ticket
                            .ingress
                            .is_none_or(|time| time.first_report_at_ms > time.last_report_at_ms)
                    {
                        guard.invalidate();
                    }
                    for key in &observation.native_catalog_change.removed {
                        guard.observe_removal(key);
                    }
                    for key in &observation.native_catalog_change.updated {
                        if let Some(listing) = self.catalog_observations.retained_listing(key) {
                            guard.observe(&listing);
                        }
                    }
                }
            }
            if let Some(catalog) = &mut entry.catalog {
                catalog
                    .runtime
                    .observe(ticket.sequence, ticket.ingress, message);
            } else if entry.persistent_delivery != PersistentDelivery::NotAttempted {
                // The native attempt began and the catalog stage has not: keep the
                // publication for the stage to replay.
                let bytes = message.encoded_len();
                let retained_bytes = entry.early_file_observation_bytes.saturating_add(bytes);
                if entry.early_file_observations.len() >= EARLY_FILE_OBSERVATION_LIMIT
                    || retained_bytes > EARLY_FILE_OBSERVATION_BYTES
                {
                    // This is only a passive optimization. Refuse retention and let the
                    // existing bounded catalog reads establish evidence instead.
                    entry.early_file_observations.clear();
                    entry.early_file_observation_bytes = 0;
                } else {
                    entry.early_file_observation_bytes = retained_bytes;
                    entry.early_file_observations.push((
                        ticket.sequence,
                        ticket.ingress,
                        message.encode_to_vec(),
                    ));
                }
            }
        }
    }

    pub(super) fn invalidate_preset_workflow(&mut self, reason: String) {
        if let Some(entry) = &mut self.preset_preparation {
            entry.invalidate(reason.clone());
        }
        if let Some(entry) = &mut self.preset_workflow {
            entry.runtime.invalidate(reason);
            entry.early_file_observations.clear();
            entry.early_file_observation_bytes = 0;
        }
    }

    pub(super) fn validate_preset_workflow_dispatch(
        &self,
        operation: &OperationTicket,
    ) -> Result<(), String> {
        let valid = self.preset_workflow.as_ref().is_some_and(|entry| {
            !entry.runtime.is_terminal()
                && entry.native.as_ref().is_some_and(|native| {
                    native.operation == *operation
                        && Some(native.ticket) == operation.workflow
                        && (self.deliveries.operation_started(operation)
                            || (!entry.navigation_lost
                                && (native.cleanup || catalog_matches(entry))
                                && deletion_target_inactive(entry, self)
                                && native
                                    .source
                                    .as_ref()
                                    .is_none_or(|source| source.matches(&self.state))))
                })
        });
        if !valid {
            return Err("QC preset workflow native action is no longer active".into());
        }
        Ok(())
    }

    pub(super) fn preset_workflow_source_changed(&self, operation: &OperationTicket) -> bool {
        self.preset_workflow.as_ref().is_some_and(|entry| {
            entry
                .native
                .as_ref()
                .filter(|native| native.operation == *operation)
                .is_some_and(|native| {
                    !self.deliveries.operation_started(operation)
                        && (entry.navigation_lost
                            || native
                                .source
                                .as_ref()
                                .is_some_and(|source| !source.matches(&self.state)))
                })
        })
    }

    pub(super) fn revoke_preset_workflow_source(&mut self) {
        if let Some(entry) = &mut self.preset_workflow {
            revoke_source(entry);
        }
    }

    /// Advances preset workflow `id` using the current time in milliseconds and returns its next action.
    ///
    /// When the action is a native write, the result carries a freshly admitted operation ticket, issued once per action; later calls return [`PresetWorkflowAction::AwaitReceipt`]. Fails if `id` is no longer the active workflow or the next action cannot be planned. The retained workflow is kept even when an error is returned.
    pub fn advance_preset_workflow(
        &mut self,
        id: u64,
        now_ms: u64,
    ) -> Result<OwnedPresetWorkflowAction, String> {
        if self
            .preset_workflow
            .as_ref()
            .is_none_or(|entry| entry.id != id)
        {
            return Err("QC preset workflow identity is no longer active".into());
        }
        // Hosts hold the owner mutex across this transition. Always restore the
        // retained entry even when planning fails; an error cannot erase it.
        let mut entry = self.preset_workflow.take().expect("validated workflow");
        let result = self.advance_owned_preset_workflow(&mut entry, now_ms);
        if let Ok(OwnedPresetWorkflowAction {
            action: PresetWorkflowAction::Finished(outcome),
            ..
        }) = &result
        {
            entry.early_file_observations.clear();
            entry.early_file_observation_bytes = 0;
            if entry.result.is_none() {
                entry.result = Some(PresetWorkflowResult::capture(&entry, outcome.clone(), self));
            }
        }
        self.preset_workflow = Some(entry);
        result
    }

    pub(super) fn validate_preset_workflow_message(
        &self,
        operation: &OperationTicket,
        message_type: u16,
    ) -> Result<(), String> {
        self.validate_preset_workflow_dispatch(operation)?;
        let native = self
            .preset_workflow
            .as_ref()
            .and_then(|entry| entry.native.as_ref())
            .expect("validated native action");
        let submitted = self.deliveries.operation_message_types(operation);
        if !native.message_types.starts_with(&submitted)
            || native.message_types.get(submitted.len()) != Some(&message_type)
        {
            return Err("Native frame does not match the retained preset workflow action".into());
        }
        Ok(())
    }

    fn advance_owned_preset_workflow(
        &mut self,
        entry: &mut PresetWorkflowEntry,
        now_ms: u64,
    ) -> Result<OwnedPresetWorkflowAction, String> {
        if !self.accepts_fence(entry.context.session_fence()) {
            entry
                .runtime
                .invalidate("QC preset workflow session changed".into());
        }
        // The delivery journal cannot change mid-tick (nothing below mutates
        // `self.deliveries`, and `revoke_source` never touches `entry.native`),
        // so its answer for this operation is computed once and reused at
        // every site below that previously re-scanned it for the same query.
        let native_operation_started = entry
            .native
            .as_ref()
            .is_some_and(|native| self.deliveries.operation_started(&native.operation));
        if entry.native.as_ref().is_some_and(|native| {
            !native_operation_started
                && (entry.navigation_lost
                    || native
                        .source
                        .as_ref()
                        .is_some_and(|source| !source.matches(&self.state)))
        }) {
            revoke_source(entry);
        }
        if !entry.runtime.is_terminal() && !deletion_target_inactive(entry, self) {
            if let Some(native) = entry.native.as_ref().filter(|_| !native_operation_started) {
                entry.runtime.complete(native.ticket, PresetWorkflowCompletion::Native(
                    NativeStageOutcome::NotAttempted("The setlist deletion target became active or active navigation is unavailable".into())), now_ms)?;
                entry.native = None;
            }
        }
        if !entry.runtime.is_terminal() && !catalog_matches(entry) {
            if let Some(native) = entry
                .native
                .as_ref()
                .filter(|native| !native.cleanup && !native_operation_started)
            {
                entry.runtime.complete(
                    native.ticket,
                    PresetWorkflowCompletion::Native(NativeStageOutcome::NotAttempted(
                        "QC catalog operands changed before the native attempt".into(),
                    )),
                    now_ms,
                )?;
                entry.native = None;
            }
        }
        if !entry.runtime.is_terminal() {
            if let Some(native) = &entry.native {
                // Expiry cannot permit cleanup while a native frame is still
                // completing. Its last callback starts the settlement clock.
                if self.deliveries.operation_in_flight(&native.operation) {
                    return Ok(OwnedPresetWorkflowAction {
                        action: PresetWorkflowAction::AwaitReceipt {
                            ticket: native.ticket,
                            expires_at_ms: native.expires_at_ms,
                        },
                        operation: None,
                        readback: None,
                        catalog: None,
                    });
                }
                let outcome = match self
                    .deliveries
                    .operation_complete(&native.operation, native.message_types.len())
                {
                    Ok(true) => Some(NativeStageOutcome::Completed),
                    Err(error) => Some(NativeStageOutcome::Indeterminate(error)),
                    Ok(false) if now_ms >= native.expires_at_ms => {
                        Some(NativeStageOutcome::Indeterminate(
                            "Native preset action expired without complete delivery evidence"
                                .into(),
                        ))
                    }
                    Ok(false) => None,
                };
                if let Some(outcome) = outcome {
                    if native.persistent && outcome == NativeStageOutcome::Completed {
                        entry.persistent_delivery = PersistentDelivery::Delivered;
                    }
                    let started = self
                        .deliveries
                        .operation_started_at(&native.operation, native.message_types[0]);
                    if let (Some(started_ms), Some(target)) = (started, &native.navigation_target) {
                        entry.expected_navigation = Some(target.clone());
                        if native.save_target {
                            if let Some(source) = &native.source {
                                entry.save_transition = Some(SaveTransition {
                                    source: source.clone(),
                                    source_catalog_name: native.source_catalog_name.clone(),
                                    target: target.clone(),
                                    started_ms,
                                    observation_floor: native.observation_floor,
                                });
                            }
                        }
                    }
                    entry.readback_evidence = Some(ReadbackEvidence {
                        observation_floor: native.observation_floor,
                        // Reuse `started`, computed just above for the same
                        // (operation, message_types[0]) pair: nothing mutates
                        // the delivery journal between the two reads.
                        native_started_ms: started,
                    });
                    entry.runtime.complete(
                        native.ticket,
                        PresetWorkflowCompletion::Native(outcome),
                        now_ms,
                    )?;
                    // Revoke queued frames before issuing any following action.
                    entry.native = None;
                }
            }
        }
        if !entry.runtime.is_terminal() {
            if let Some(readback) = &mut entry.readback {
                let evidence = entry
                    .readback_evidence
                    .as_ref()
                    .ok_or("Preset readback lost native evidence")?;
                let observed = entry
                    .observed
                    .as_ref()
                    .filter(|(ticket, _, _)| self.accepts_observation(ticket))
                    .filter(|(ticket, _, _)| ticket.sequence > evidence.observation_floor)
                    .filter(|(ticket, _, _)| {
                        ticket.ingress.is_some_and(|ingress| {
                            evidence.native_started_ms.is_some_and(|started| {
                                // Millisecond equality cannot order a queued ingress
                                // against the native attempt. Wait for stronger data.
                                ingress.first_report_at_ms > started
                                    && ingress.first_report_at_ms <= ingress.last_report_at_ms
                                    && ingress.last_report_at_ms <= now_ms
                            })
                        })
                    });
                let observation_sequence =
                    observed.map_or(evidence.observation_floor, |(ticket, _, _)| ticket.sequence);
                let action = readback.runtime.advance(
                    observed.map(|(_, snapshot, _)| snapshot),
                    None,
                    u128::from(observation_sequence),
                    now_ms,
                );
                match action {
                    GatewayVerificationAction::Verified | GatewayVerificationAction::TimedOut => {
                        let result = if action == GatewayVerificationAction::Verified {
                            if let Some((_, snapshot, checkpoint)) = observed {
                                entry.verified_snapshot =
                                    Some((snapshot.clone(), checkpoint.clone()));
                                entry.source_checkpoint = checkpoint.clone();
                            }
                            Ok(())
                        } else {
                            Err("Preset workflow readback deadline expired".into())
                        };
                        entry.runtime.complete(
                            readback.ticket,
                            PresetWorkflowCompletion::Readback(result),
                            now_ms,
                        )?;
                        entry.readback = None;
                    }
                    _ => {
                        return Ok(OwnedPresetWorkflowAction {
                            action: PresetWorkflowAction::AwaitReceipt {
                                ticket: readback.ticket,
                                expires_at_ms: readback.expires_at_ms,
                            },
                            operation: None,
                            readback: Some(OwnedPresetWorkflowReadback {
                                ticket: readback.ticket,
                                action,
                                context: readback.context.clone(),
                                remaining_ms: readback.runtime.remaining_ms(now_ms),
                            }),
                            catalog: None,
                        });
                    }
                }
            }
        }
        if !entry.runtime.is_terminal() {
            let deletion_unavailable = !deletion_target_inactive(entry, self);
            if let Some(catalog) = &mut entry.catalog {
                let action = catalog.runtime.take_action(now_ms);
                match action {
                    CatalogVerificationAction::Complete => {
                        entry.catalog_result = catalog.runtime.resolution().cloned();
                        if deletion_unavailable {
                            entry.catalog_result = None;
                        }
                        let result = if deletion_unavailable {
                            Err("Setlist deletion cannot be confirmed while the target is active or active navigation is unavailable".to_string())
                        } else {
                            entry.catalog_result.as_ref().map(|_| ()).ok_or_else(|| {
                                "Preset catalog completion has no retained resolution".to_string()
                            })
                        };
                        entry.runtime.complete(
                            catalog.ticket,
                            PresetWorkflowCompletion::Catalog(result),
                            now_ms,
                        )?;
                        // Native ingress already updated the catalog. Retain
                        // the verification result without installing an overlay
                        // that could mask subsequent device state.
                        entry.catalog = None;
                    }
                    CatalogVerificationAction::Failed { .. } => {
                        let message = catalog.runtime.failure_message();
                        // The window ended. Unless the retained capacity overflowed, no
                        // evidence contradicted the write (a stale listing is not
                        // contradiction: the first listing after a mutation can lag), so the
                        // frames-delivered write is reported unverified, not failed.
                        let completion = if catalog.runtime.overflowed() {
                            PresetWorkflowCompletion::Catalog(Err(message))
                        } else {
                            PresetWorkflowCompletion::CatalogUnconfirmed(message)
                        };
                        entry.runtime.complete(catalog.ticket, completion, now_ms)?;
                        entry.catalog = None;
                    }
                    _ => {
                        return Ok(OwnedPresetWorkflowAction {
                            action: PresetWorkflowAction::AwaitReceipt {
                                ticket: catalog.ticket,
                                expires_at_ms: catalog.expires_at_ms,
                            },
                            operation: None,
                            readback: None,
                            catalog: Some(OwnedPresetWorkflowCatalog {
                                ticket: catalog.ticket,
                                action,
                                context: catalog.context.clone(),
                                remaining_ms: catalog.runtime.remaining_ms(now_ms),
                            }),
                        });
                    }
                }
            }
        }
        let action = entry.runtime.advance(now_ms)?;
        let mut operation = None;
        if let PresetWorkflowAction::Dispatch {
            ticket,
            write,
            expires_at_ms,
            cleanup,
        } = &action
        {
            if *cleanup {
                refresh_save_navigation(entry, self, now_ms);
                if entry.navigation_lost
                    || self.state.snapshot().dirty
                    || !entry.source_checkpoint.matches(&self.state)
                {
                    let reason = if entry.navigation_lost {
                        "unrelated native navigation observed"
                    } else if self.state.snapshot().dirty {
                        "current preset has unsaved edits"
                    } else if self.state.preset_source_snapshot().is_none() {
                        "complete current preset contents unavailable"
                    } else {
                        "current preset differs from the retained source checkpoint"
                    };
                    entry.runtime.skip_restoration(format!("Original navigation restoration was skipped because the workflow no longer owns the current preset: {reason}"))?;
                    return Ok(OwnedPresetWorkflowAction {
                        action: entry.runtime.advance(now_ms)?,
                        operation: None,
                        readback: None,
                        catalog: None,
                    });
                }
            }
            if requires_source_checkpoint(write)
                && (entry.navigation_lost || !entry.source_checkpoint.matches(&self.state))
            {
                entry.runtime.invalidate("The complete source preset changed or became unavailable before preset stage admission".into());
                return Ok(OwnedPresetWorkflowAction {
                    action: entry.runtime.advance(now_ms)?,
                    operation: None,
                    readback: None,
                    catalog: None,
                });
            }
            if !*cleanup && (!catalog_matches(entry) || !deletion_target_inactive(entry, self)) {
                entry.runtime.complete(
                    *ticket,
                    PresetWorkflowCompletion::Native(NativeStageOutcome::NotAttempted(
                        "QC catalog operands changed after preparation".into(),
                    )),
                    now_ms,
                )?;
                return Ok(OwnedPresetWorkflowAction {
                    action: PresetWorkflowAction::Wait { until_ms: now_ms },
                    operation: None,
                    readback: None,
                    catalog: None,
                });
            }
            let admission = (|| {
                let source = if requires_source_checkpoint(write) {
                    Some(entry.source_checkpoint.clone())
                } else {
                    None
                };
                let message_types = match write {
                    PlannedWrite::HidCommand(command) => {
                        vec![
                            command
                                .clone()
                                .try_encode()
                                .map_err(|error| error.to_string())?
                                .message_type,
                        ]
                    }
                    PlannedWrite::HidOperation(operation) => operation
                        .clone()
                        .try_encode()
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .map(|message| message.message_type)
                        .collect(),
                    PlannedWrite::MidiControlChange { .. } => {
                        return Err("Preset workflow requires journalled HID delivery".into());
                    }
                };
                if message_types.is_empty() {
                    return Err("Preset workflow native action has no encoded frames".into());
                }
                let mut admitted = self.plan_operation_in_context(
                    &entry.context,
                    now_ms,
                    expires_at_ms.saturating_sub(now_ms),
                )?;
                admitted.workflow = Some(*ticket);
                Ok((admitted, message_types, source))
            })();
            match admission {
                Ok((admitted, message_types, source)) => {
                    entry.native = Some(NativeAction {
                        ticket: *ticket,
                        operation: admitted.clone(),
                        source_catalog_name: source.as_ref().and_then(|checkpoint| {
                            entry
                                .catalog_guard
                                .as_ref()?
                                .source_name(&checkpoint.setlist_key, checkpoint.position)
                        }),
                        source,
                        cleanup: *cleanup,
                        persistent: matches!(write, PlannedWrite::HidOperation(_)),
                        navigation_target: match write {
                            PlannedWrite::HidCommand(
                                crate::commands::DeviceCommand::SetlistPosition {
                                    setlist_key,
                                    position,
                                    ..
                                },
                            )
                            | PlannedWrite::HidOperation(
                                crate::commands::DeviceOperation::SavePreset {
                                    setlist_key,
                                    position,
                                    ..
                                },
                            ) => Some((setlist_key.clone(), *position)),
                            _ => None,
                        },
                        save_target: matches!(
                            write,
                            PlannedWrite::HidOperation(
                                crate::commands::DeviceOperation::SavePreset { .. }
                            )
                        ),
                        message_types,
                        observation_floor: self.next_observation.saturating_sub(1),
                        expires_at_ms: *expires_at_ms,
                    });
                    operation = Some(admitted);
                }
                Err(error) => {
                    // No operation was exposed to a host, hence no native
                    // attempt of this action is possible.
                    entry.runtime.complete(
                        *ticket,
                        PresetWorkflowCompletion::Native(NativeStageOutcome::NotAttempted(error)),
                        now_ms,
                    )?;
                    return Ok(OwnedPresetWorkflowAction {
                        action: PresetWorkflowAction::Wait { until_ms: now_ms },
                        operation: None,
                        readback: None,
                        catalog: None,
                    });
                }
            }
        }
        if let PresetWorkflowAction::Verify {
            ticket,
            verification,
            expires_at_ms,
        } = &action
        {
            let evidence = entry
                .readback_evidence
                .as_ref()
                .ok_or("Preset readback has no retained native evidence")?;
            let deadline = crate::device::correlation::RequestDeadline::new(
                now_ms,
                expires_at_ms.saturating_sub(now_ms),
            );
            let mut context = entry.context.clone();
            context.operands = None;
            context.deadline = Some(
                context
                    .deadline
                    .map_or(deadline, |outer| outer.intersect(deadline)),
            );
            entry.readback = Some(ReadbackAction {
                ticket: *ticket,
                runtime: GatewayVerificationRuntime::new(
                    verification.clone(),
                    u128::from(evidence.observation_floor),
                    now_ms,
                    &gateway_verification_policy(expires_at_ms.saturating_sub(now_ms)),
                ),
                context,
                expires_at_ms: *expires_at_ms,
            });
        }
        if let PresetWorkflowAction::VerifyCatalog {
            ticket,
            expires_at_ms,
        } = &action
        {
            let deadline = crate::device::correlation::RequestDeadline::new(
                now_ms,
                expires_at_ms.saturating_sub(now_ms),
            );
            let mut context = entry.context.clone();
            context.operands = None;
            context.deadline = Some(
                context
                    .deadline
                    .map_or(deadline, |outer| outer.intersect(deadline)),
            );
            // Evidence counts from the native attempt, not from the start of this stage:
            // the unit's own File publication usually arrives while the host is still
            // settling (a save waits out its readback first), and verification is
            // passive-first. Anything published before the attempt stays excluded.
            let (observation_floor, evidence_after_ms) = entry.readback_evidence.as_ref().map_or(
                (self.next_observation.saturating_sub(1), now_ms),
                |evidence| {
                    (
                        evidence.observation_floor,
                        evidence.native_started_ms.unwrap_or(now_ms),
                    )
                },
            );
            let mut runtime = PresetMutationCatalog::new(
                entry.runtime.plan(),
                now_ms,
                observation_floor,
                evidence_after_ms,
                context.deadline.expect("catalog deadline"),
            );
            entry.early_file_observation_bytes = 0;
            for (sequence, ingress, message) in std::mem::take(&mut entry.early_file_observations) {
                // These bytes were encoded from an accepted message above. Decode
                // one envelope at a time so retained decoded vectors cannot amplify.
                if let Ok(message) =
                    crate::proto::cortex_protobuf_v2::FileMessage::decode(message.as_slice())
                {
                    runtime.observe(sequence, ingress, &message);
                }
            }
            let phase_deadline = crate::device::correlation::RequestDeadline::new(
                now_ms,
                runtime.remaining_ms(now_ms),
            );
            context.deadline = Some(phase_deadline);
            entry.catalog = Some(CatalogAction {
                ticket: *ticket,
                runtime,
                context,
                expires_at_ms: phase_deadline.expires_at_ms(),
            });
        }
        Ok(OwnedPresetWorkflowAction {
            action,
            operation,
            readback: None,
            catalog: None,
        })
    }
}

fn deletion_target_inactive(entry: &PresetWorkflowEntry, owner: &SessionRuntime) -> bool {
    !entry.runtime.plan().deletes_setlist()
        || (owner.state.snapshot().has_preset
            && !owner.state.snapshot().setlist_key.is_empty()
            && !owner
                .state
                .snapshot()
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(entry.runtime.plan().setlist_key.trim_end_matches('/')))
}

fn catalog_matches(entry: &PresetWorkflowEntry) -> bool {
    entry
        .catalog_guard
        .as_ref()
        .is_none_or(CatalogGuard::matches)
}

fn requires_source_checkpoint(write: &PlannedWrite) -> bool {
    matches!(
        write,
        PlannedWrite::HidOperation(crate::commands::DeviceOperation::SavePreset { .. })
            | PlannedWrite::HidCommand(crate::commands::DeviceCommand::SetlistPosition { .. })
    )
}

fn revoke_source(entry: &mut PresetWorkflowEntry) {
    if entry.runtime.is_terminal() {
        return;
    }
    let reason = "QC preset workflow source changed before its native attempt".to_string();
    if entry.native.as_ref().is_some_and(|native| native.cleanup) {
        if let Err(error) = entry.runtime.skip_restoration(reason) {
            entry.runtime.invalidate(error);
        }
    } else {
        entry.runtime.invalidate(reason);
    }
}

// Save can publish its new address without publishing a full preset. Accept
// that planned transition only with native position provenance and unchanged
// complete source contents. Catalog-proven save metadata is handled separately;
// every other field and all pre-write source checks remain exact.
fn refresh_save_navigation(entry: &mut PresetWorkflowEntry, owner: &SessionRuntime, now_ms: u64) {
    if entry.navigation_lost || entry.source_checkpoint.matches(&owner.state) {
        return;
    }
    let (Some(transition), Some((ticket, observed))) =
        (&entry.save_transition, &entry.position_observed)
    else {
        return;
    };
    if !owner.accepts_observation_for(entry.context.session_fence(), ticket)
        || ticket.sequence <= transition.observation_floor
        || !ticket.ingress.is_some_and(|time| {
            time.first_report_at_ms > transition.started_ms
                && time.first_report_at_ms <= time.last_report_at_ms
                && time.last_report_at_ms <= now_ms
        })
        || !observed
            .setlist_key
            .trim_end_matches('/')
            .eq_ignore_ascii_case(transition.target.0.trim_end_matches('/'))
        || observed.position != transition.target.1
        || observed.scene != transition.source.scene
        || observed.source.is_none()
    {
        return;
    }
    let mut candidate = observed.clone();
    candidate.dirty = false;
    if observed.source == transition.source.source && candidate.matches(&owner.state) {
        entry.source_checkpoint = candidate;
        return;
    }
    let Some(saved) = entry.catalog_result.as_ref().and_then(|catalog| {
        catalog.saved_presets.iter().find(|saved| {
            saved
                .setlist_key
                .trim_end_matches('/')
                .eq_ignore_ascii_case(transition.target.0.trim_end_matches('/'))
                && saved.position == transition.target.1
        })
    }) else {
        return;
    };
    let Some((snapshot_ticket, _, observed_current)) = &entry.observed else {
        return;
    };
    let current = SourceCheckpoint::capture(&owner.state);
    if !owner.accepts_observation_for(entry.context.session_fence(), snapshot_ticket)
        || snapshot_ticket.sequence <= transition.observation_floor
        || !snapshot_ticket.ingress.is_some_and(|time| {
            time.first_report_at_ms > transition.started_ms
                && time.first_report_at_ms <= time.last_report_at_ms
                && time.last_report_at_ms <= now_ms
        })
        || !current
            .setlist_key
            .trim_end_matches('/')
            .eq_ignore_ascii_case(transition.target.0.trim_end_matches('/'))
        || current.position != transition.target.1
        || current.scene != transition.source.scene
        || current.dirty
        || observed_current.source != current.source
        || !current.matches(&owner.state)
    {
        return;
    }
    if let (Some(original), Some(current_source), Some(position_source)) =
        (&transition.source.source, &current.source, &observed.source)
    {
        if current_source.matches_saved_contents(original, &saved.name)
            && (position_source == original
                || position_source.matches_save_progress(
                    original,
                    &saved.name,
                    transition.source_catalog_name.as_deref(),
                ))
        {
            entry.source_checkpoint = current.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{DeviceCommand, DeviceOperation};
    use crate::device::catalog::CatalogVerificationPolicy;
    use crate::device::delivery::ReportOutcome;
    use crate::device::preset_workflow::RestorationOutcome;
    use crate::device::request::{GatewayVerification, PresetMutationStage};
    use crate::device::session::SessionDriver;

    fn owner() -> SessionRuntime {
        let mut owner = SessionRuntime::new(0);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        publish_preset(&mut owner, Some(0), 0);
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Test".into();
        owner.state.snapshot_mut().preset_position = 1;
        owner
    }

    // Native-boundary fixture deliberately omits readback; the production plan
    // builder retains its verification predicates, tested in request.rs.
    fn plan() -> PresetMutationPlan {
        PresetMutationPlan {
            stages: vec![PresetMutationStage {
                write: PlannedWrite::HidOperation(DeviceOperation::SavePreset {
                    setlist_key: "/media/p4/Presets/Test".into(),
                    position: 2,
                    name: "Saved".into(),
                    instrument: 0,
                }),
                verification: GatewayVerification::None,
                timeout_ms: 100,
                settle_ms: 30,
            }],
            cleanup_stages: vec![PresetMutationStage {
                write: PlannedWrite::HidCommand(DeviceCommand::SetlistPosition {
                    is_factory: false,
                    setlist_key: "/media/p4/Presets/Test".into(),
                    position: 1,
                }),
                verification: GatewayVerification::None,
                timeout_ms: 100,
                settle_ms: 0,
            }],
            detail: "native boundary".into(),
            saved_name: "Saved".into(),
            setlist_key: "/media/p4/Presets/Test".into(),
            position: 2,
            instrument: 0,
            catalog_policy: CatalogVerificationPolicy {
                passive_window_ms: 0,
                ..CatalogVerificationPolicy::default()
            },
            saved_presets: vec![],
            folder_copy: None,
            setlist_deletion: None,
            deletion: None,
        }
    }

    fn start(owner: &mut SessionRuntime) -> (u64, OperationTicket, u64) {
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, plan(), 0).unwrap();
        let step = owner.advance_preset_workflow(id, 0).unwrap();
        let PresetWorkflowAction::Dispatch { expires_at_ms, .. } = step.action else {
            panic!("expected native dispatch");
        };
        (id, step.operation.unwrap(), expires_at_ms)
    }

    fn prepare(
        owner: &mut SessionRuntime,
        operation: &OperationTicket,
    ) -> crate::device::delivery::DeliveryTicket {
        let dispatch = owner.prepare_dispatch(Some(operation), None, 1).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, 4, 1, 1).unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        delivery
    }

    fn publish_preset(owner: &mut SessionRuntime, first: Option<u64>, last: u64) {
        publish_source(owner, crate::proto::BinaryPreset::default(), first, last);
    }

    fn publish_source(
        owner: &mut SessionRuntime,
        preset: crate::proto::BinaryPreset,
        first: Option<u64>,
        last: u64,
    ) {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                first.map(|first_report_at_ms| crate::device::transport::IngressTime {
                    first_report_at_ms,
                    last_report_at_ms: last,
                }),
            )
            .unwrap();
        owner
            .observe_state(
                &ticket,
                crate::profile::MESSAGE_TYPE_RECALL_PRESET,
                &payload,
            )
            .unwrap()
            .unwrap();
        assert!(owner.state.snapshot().has_preset);
    }

    fn changed_source() -> crate::proto::BinaryPreset {
        crate::proto::BinaryPreset {
            volume: Some(crate::proto::binary_preset::Volume::Volume(0.5)),
            ..Default::default()
        }
    }

    #[test]
    fn invalid_library_record_does_not_revoke_an_unrelated_preset_workflow() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        for file_type in [1, 2] {
            let mut owner = owner();
            let context = owner.session_context().unwrap();
            let id = owner.begin_preset_workflow(&context, plan(), 0).unwrap();
            let message = pa::FileMessage {
                action: pa::message_action::Enum::Update as i32,
                r#type: Some(pa::file_message::Type::Type(file_type)),
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key("library".into())),
                    files: vec![pa::ProductData {
                        key: Some(pa::product_data::Key::Key("bad-position".into())),
                        index: Some(pa::product_data::Index::Index(-1)),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            };
            let ticket = owner.capture_observation(owner.connection).unwrap();
            assert!(owner.observe_lifecycle(&ticket, 4, &message.encode_to_vec(), 0));
            assert!(owner
                .retained_library_catalog(
                    &owner.session_fence().unwrap(),
                    if file_type == 1 {
                        crate::device::native_library::LibraryKind::Ir
                    } else {
                        crate::device::native_library::LibraryKind::Capture
                    },
                    "library",
                    0
                )
                .is_err());
            owner
                .observe_state(&ticket, 4, &message.encode_to_vec())
                .unwrap();
            assert!(owner.state.native_preset_catalog().health().is_ok());
            let step = owner.advance_preset_workflow(id, 1).unwrap();
            assert!(matches!(step.action, PresetWorkflowAction::Dispatch { .. }));
            assert!(step.operation.is_some());
        }
    }

    #[test]
    fn source_change_after_planning_prevents_save_and_skips_navigation_cleanup() {
        let mut owner = owner();
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, plan(), 0).unwrap();
        publish_source(&mut owner, changed_source(), Some(1), 1);
        let step = owner.advance_preset_workflow(id, 2).unwrap();
        assert!(step.operation.is_none());
        let PresetWorkflowAction::Finished(outcome) = step.action else {
            panic!("changed source must terminate");
        };
        assert!(outcome
            .primary_error
            .unwrap()
            .contains("source preset changed"));
        assert!(matches!(
            outcome.restoration,
            RestorationOutcome::Skipped(_)
        ));
    }

    #[test]
    fn queued_save_rechecks_source_at_first_report_and_revocation_is_sticky() {
        let mut owner = owner();
        let (id, operation, _) = start(&mut owner);
        let delivery = prepare(&mut owner, &operation);
        publish_source(&mut owner, changed_source(), Some(2), 2);
        assert!(owner.begin_native_report(&delivery, 0, 3).is_err());
        assert!(owner
            .delivery_record(&delivery)
            .unwrap()
            .started_at_ms
            .is_none());
        // Returning to the old source cannot revive the rejected Save or its cleanup.
        publish_preset(&mut owner, Some(4), 4);
        let PresetWorkflowAction::Finished(outcome) =
            owner.advance_preset_workflow(id, 5).unwrap().action
        else {
            panic!("revoked workflow must stay terminal");
        };
        assert!(matches!(
            outcome.restoration,
            RestorationOutcome::Skipped(_)
        ));
        assert!(owner.prepare_dispatch(Some(&operation), None, 6).is_err());
    }

    #[test]
    fn unchanged_full_read_does_not_revoke_a_queued_save() {
        let mut owner = owner();
        let (_, operation, _) = start(&mut owner);
        let delivery = prepare(&mut owner, &operation);
        publish_preset(&mut owner, Some(2), 2);
        owner.begin_native_report(&delivery, 0, 3).unwrap();
        // Once started, a mutation's own echo does not revoke its current frame.
        publish_source(&mut owner, changed_source(), Some(4), 4);
        assert!(owner.validate_operation(&operation, 5).is_ok());
    }

    #[test]
    fn incomplete_source_cannot_issue_a_save_operation() {
        let mut owner = owner();
        owner.state.reset();
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Test".into();
        owner.state.snapshot_mut().preset_position = 1;
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, plan(), 0).unwrap();
        let step = owner.advance_preset_workflow(id, 1).unwrap();
        assert!(step.operation.is_none());
        assert!(matches!(step.action, PresetWorkflowAction::Finished(_)));
    }

    #[test]
    fn verified_recall_adopts_its_source_but_cannot_bless_later_navigation() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        for later_navigation in [false, true] {
            let mut owner = owner();
            let mut plan = plan();
            plan.stages.insert(
                0,
                PresetMutationStage {
                    write: PlannedWrite::HidCommand(DeviceCommand::SetlistPosition {
                        is_factory: false,
                        setlist_key: "/media/p4/Presets/Test".into(),
                        position: 2,
                    }),
                    verification: GatewayVerification::Preset {
                        setlist_key: "/media/p4/Presets/Test".into(),
                        position: 2,
                        name: None,
                        require_clean: false,
                        after_position_revision: None,
                        after_full_preset_revision: None,
                    },
                    timeout_ms: 100,
                    settle_ms: 0,
                },
            );
            let context = owner.session_context().unwrap();
            let id = owner.begin_preset_workflow(&context, plan, 0).unwrap();
            let recall = owner
                .advance_preset_workflow(id, 0)
                .unwrap()
                .operation
                .unwrap();
            let dispatch = owner.prepare_dispatch(Some(&recall), None, 1).unwrap();
            let delivery = owner
                .prepare_delivery(
                    &dispatch,
                    crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                    1,
                    1,
                )
                .unwrap();
            owner.delivery_admitted(&delivery).unwrap();
            owner.begin_native_report(&delivery, 0, 2).unwrap();
            owner
                .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 3)
                .unwrap();
            assert!(matches!(
                owner.advance_preset_workflow(id, 3).unwrap().action,
                PresetWorkflowAction::Verify { .. }
            ));
            let payload = pa::SetlistPositionMessage {
                action: pa::message_action::Enum::Update as i32,
                folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                    "/media/p4/Presets/Test".into(),
                )),
                position: Some(pa::setlist_position_message::Position::Position(2)),
                ..Default::default()
            }
            .encode_to_vec();
            let ticket = owner
                .capture_observation_with_ingress(
                    owner.connection,
                    Some(crate::device::transport::IngressTime {
                        first_report_at_ms: 4,
                        last_report_at_ms: 4,
                    }),
                )
                .unwrap();
            owner
                .observe_state(
                    &ticket,
                    crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                    &payload,
                )
                .unwrap();
            publish_source(&mut owner, changed_source(), Some(5), 5);
            if later_navigation {
                owner.state.snapshot_mut().preset_position = 9;
            }
            let step = owner.advance_preset_workflow(id, 6).unwrap();
            if later_navigation {
                assert!(step.operation.is_none());
                assert!(matches!(step.action, PresetWorkflowAction::Finished(_)));
            } else {
                assert!(matches!(
                    step.action,
                    PresetWorkflowAction::Dispatch {
                        write: PlannedWrite::HidOperation(DeviceOperation::SavePreset { .. }),
                        ..
                    }
                ));
                assert!(owner
                    .prepare_dispatch(step.operation.as_ref(), None, 7)
                    .is_ok());
            }
        }
    }

    fn start_verified(owner: &mut SessionRuntime) -> u64 {
        let mut plan = plan();
        owner.state.snapshot_mut().setlist_key = plan.setlist_key.clone();
        owner.state.snapshot_mut().preset_position = plan.position;
        plan.cleanup_stages[0].write = PlannedWrite::HidCommand(DeviceCommand::SetlistPosition {
            is_factory: false,
            setlist_key: plan.setlist_key.clone(),
            position: plan.position,
        });
        plan.stages[0].verification = GatewayVerification::Preset {
            setlist_key: plan.setlist_key.clone(),
            position: plan.position,
            name: None,
            require_clean: false,
            after_position_revision: None,
            after_full_preset_revision: None,
        };
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, plan, 0).unwrap();
        let operation = owner
            .advance_preset_workflow(id, 0)
            .unwrap()
            .operation
            .unwrap();
        let delivery = prepare(owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 3)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_workflow(id, 3).unwrap().action,
            PresetWorkflowAction::Verify { .. }
        ));
        id
    }

    fn start_catalog(owner: &mut SessionRuntime) -> u64 {
        start_catalog_with_source_name(owner, None)
    }

    fn start_catalog_with_source_name(
        owner: &mut SessionRuntime,
        source_name: Option<&str>,
    ) -> u64 {
        let mut plan = plan();
        plan.catalog_policy.timeout_ms = 100;
        plan.saved_presets
            .push(crate::device::request::PresetMutationRecord {
                setlist_key: plan.setlist_key.clone(),
                position: 2,
                name: "Saved".into(),
                instrument: 0,
                allow_collision_suffix: true,
            });
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, plan, 0).unwrap();
        if let Some(name) = source_name {
            let key = "/media/p4/Presets/Test".to_string();
            let folder = crate::state::PresetFolderListing {
                order: None,
                key: key.clone(),
                name: "Test".into(),
                file_type: Some(0),
                is_factory: false,
                is_plugin: false,
                files: vec![crate::state::PresetFileListing {
                    position: 1,
                    name: name.into(),
                    instrument: 0,
                    instrument_present: true,
                    product_key: Some("retained-source-key".into()),
                    ..Default::default()
                }],
            };
            owner.preset_workflow.as_mut().unwrap().catalog_guard = Some(CatalogGuard::new(
                &std::collections::BTreeMap::from([(key.clone(), folder)]),
                &[key],
                false,
            ));
        }
        let operation = owner
            .advance_preset_workflow(id, 0)
            .unwrap()
            .operation
            .unwrap();
        let delivery = prepare(owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 3)
            .unwrap();
        assert_eq!(
            owner.advance_preset_workflow(id, 3).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 33 }
        );
        assert!(matches!(
            owner.advance_preset_workflow(id, 33).unwrap().action,
            PresetWorkflowAction::VerifyCatalog { .. }
        ));
        id
    }

    fn publish_folder(owner: &mut SessionRuntime, name: &str, first_ms: u64, last_ms: u64) {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("/media/p4/Presets/Test".into())),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key("device-key".into())),
                    name: Some(pa::product_data::Name::Name(name.into())),
                    index: Some(pa::product_data::Index::Index(2)),
                    instrument: Some(pa::product_data::Instrument::Instrument(0)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: first_ms,
                    last_report_at_ms: last_ms,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap()
            .unwrap();
    }

    fn position_payload(position: u32) -> Vec<u8> {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        pa::SetlistPositionMessage {
            action: pa::message_action::Enum::Update as i32,
            folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                "/media/p4/Presets/Test".into(),
            )),
            position: Some(pa::setlist_position_message::Position::Position(position)),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn publish_position(owner: &mut SessionRuntime, position: u32, first: Option<u64>, last: u64) {
        let payload = position_payload(position);
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                first.map(|first_report_at_ms| crate::device::transport::IngressTime {
                    first_report_at_ms,
                    last_report_at_ms: last,
                }),
            )
            .unwrap();
        owner
            .observe_state(
                &ticket,
                crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                &payload,
            )
            .unwrap()
            .unwrap();
    }

    fn assert_saved_without_restoration(step: OwnedPresetWorkflowAction) {
        assert!(step.operation.is_none());
        let PresetWorkflowAction::Finished(outcome) = step.action else {
            panic!("must finish without a restoration operation");
        };
        assert!(outcome.primary_error.is_none());
        assert!(outcome.catalog_verified);
        assert!(matches!(
            outcome.restoration,
            RestorationOutcome::Skipped(_)
        ));
    }

    #[test]
    fn cleanup_must_target_the_original_navigation_address() {
        for wrong_field in 0..3 {
            let mut owner = owner();
            let mut plan = plan();
            let PlannedWrite::HidCommand(DeviceCommand::SetlistPosition {
                setlist_key,
                position,
                is_factory,
            }) = &mut plan.cleanup_stages[0].write
            else {
                unreachable!()
            };
            match wrong_field {
                0 => *setlist_key = "/media/p4/Presets/Other".into(),
                1 => *position = 9,
                _ => *is_factory = true,
            }
            let context = owner.session_context().unwrap();
            assert!(owner
                .begin_preset_workflow(&context, plan, 0)
                .unwrap_err()
                .contains("original navigation address"));
            assert!(owner.preset_workflow.is_none());
        }
    }

    #[test]
    fn unrelated_native_navigation_skips_cleanup_even_after_returning() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_position(&mut owner, 9, Some(34), 34);
        publish_position(&mut owner, 1, Some(35), 35);
        publish_folder(&mut owner, "Saved", 36, 36);
        assert_saved_without_restoration(owner.advance_preset_workflow(id, 37).unwrap());
    }

    #[test]
    fn changed_source_scene_or_dirty_state_skips_cleanup_but_preserves_catalog_success() {
        for change in 0..3 {
            let mut owner = owner();
            let id = start_catalog(&mut owner);
            match change {
                0 => publish_source(&mut owner, changed_source(), Some(34), 34),
                1 => owner.state.snapshot_mut().dirty = true,
                _ => owner.state.snapshot_mut().active_scene = 3,
            }
            publish_folder(&mut owner, "Saved", 35, 35);
            assert_saved_without_restoration(owner.advance_preset_workflow(id, 36).unwrap());
        }
    }

    #[test]
    fn queued_cleanup_rechecks_ownership_at_first_report_and_never_revives() {
        for change in 0..3 {
            let mut owner = owner();
            let id = start_catalog(&mut owner);
            publish_folder(&mut owner, "Saved", 34, 34);
            let step = owner.advance_preset_workflow(id, 35).unwrap();
            assert!(matches!(
                step.action,
                PresetWorkflowAction::Dispatch { cleanup: true, .. }
            ));
            let operation = step.operation.unwrap();
            let dispatch = owner.prepare_dispatch(Some(&operation), None, 36).unwrap();
            let delivery = owner
                .prepare_delivery(
                    &dispatch,
                    crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                    1,
                    36,
                )
                .unwrap();
            owner.delivery_admitted(&delivery).unwrap();
            match change {
                0 => publish_position(&mut owner, 9, Some(37), 37),
                1 => publish_source(&mut owner, changed_source(), Some(37), 37),
                _ => owner.state.snapshot_mut().dirty = true,
            }
            assert!(owner.begin_native_report(&delivery, 0, 38).is_err());
            assert!(owner
                .delivery_record(&delivery)
                .unwrap()
                .started_at_ms
                .is_none());
            publish_position(&mut owner, 1, Some(39), 39);
            publish_preset(&mut owner, Some(40), 40);
            owner.state.snapshot_mut().dirty = false;
            assert_saved_without_restoration(owner.advance_preset_workflow(id, 41).unwrap());
            assert!(owner.begin_native_report(&delivery, 0, 42).is_err());
            assert!(owner.prepare_dispatch(Some(&operation), None, 42).is_err());
        }
    }

    #[test]
    fn catalog_verified_save_metadata_allows_only_unchanged_contents_to_restore() {
        use crate::proto::{binary_preset, BinaryPreset};
        for case in 0..12 {
            let mut owner = owner();
            let id = start_catalog_with_source_name(
                &mut owner,
                (case >= 9).then_some(if case == 10 { "Different" } else { "Renamed" }),
            );
            let mut saved = BinaryPreset {
                name: Some(binary_preset::Name::Name(
                    if case == 2 { "Unrelated" } else { "Saved_3" }.into(),
                )),
                hash: Some(binary_preset::Hash::Hash(42)),
                date: Some(binary_preset::Date::Date("2026-09-14".into())),
                ..Default::default()
            };
            if case == 3 {
                saved.volume = Some(binary_preset::Volume::Volume(0.5));
            }
            if case >= 6 {
                let mut progress = saved.clone();
                progress.name = None;
                if case == 7 {
                    progress.name = Some(binary_preset::Name::Name("Unrelated".into()));
                }
                if case >= 9 {
                    progress.name = Some(binary_preset::Name::Name("Renamed".into()));
                }
                if case == 8 || case == 11 {
                    progress.volume = Some(binary_preset::Volume::Volume(0.5));
                }
                publish_source(&mut owner, progress, Some(33), 33);
                publish_position(&mut owner, 2, Some(34), 34);
                publish_source(&mut owner, saved, Some(35), 35);
            } else if case == 1 {
                publish_source(&mut owner, saved, Some(34), 34);
                publish_position(&mut owner, 2, Some(35), 35);
            } else {
                publish_position(&mut owner, 2, Some(34), 34);
                publish_source(&mut owner, saved, Some(35), 35);
            }
            if case == 4 {
                owner.state.snapshot_mut().active_scene = 3;
            }
            if case == 5 {
                owner.state.snapshot_mut().dirty = true;
            }
            publish_folder(&mut owner, "Saved_3", 36, 36);
            let step = owner.advance_preset_workflow(id, 37).unwrap();
            if case < 2 || case == 6 || case == 9 {
                assert!(
                    matches!(
                        step.action,
                        PresetWorkflowAction::Dispatch { cleanup: true, .. }
                    ),
                    "case {case}"
                );
                assert!(owner
                    .prepare_dispatch(step.operation.as_ref(), None, 38)
                    .is_ok());
            } else {
                assert_saved_without_restoration(step);
            }
        }
    }

    #[test]
    fn save_position_echo_with_unchanged_source_allows_cleanup_without_full_readback() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        let revision = owner.state.snapshot().full_preset_revision;
        publish_position(&mut owner, 2, Some(34), 34);
        publish_folder(&mut owner, "Saved", 35, 35);
        let step = owner.advance_preset_workflow(id, 36).unwrap();
        assert!(matches!(
            step.action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
        assert_eq!(owner.state.snapshot().full_preset_revision, revision);
        let operation = step.operation.unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 37).unwrap();
        let delivery = owner
            .prepare_delivery(
                &dispatch,
                crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                2,
                37,
            )
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, 38).unwrap();
        // Its own recall echo must not interrupt the remainder of this native frame.
        publish_position(&mut owner, 1, Some(39), 39);
        publish_source(&mut owner, changed_source(), Some(40), 40);
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 41)
            .unwrap();
        owner.begin_native_report(&delivery, 1, 42).unwrap();
        owner
            .finish_native_report(&delivery, 1, ReportOutcome::Delivered, 43)
            .unwrap();
        let PresetWorkflowAction::Finished(outcome) =
            owner.advance_preset_workflow(id, 44).unwrap().action
        else {
            panic!("cleanup fixture should finish");
        };
        assert!(outcome.primary_error.is_none());
        assert!(outcome.catalog_verified);
        assert_eq!(outcome.restoration, RestorationOutcome::Completed);
    }

    #[test]
    fn position_ticket_captured_before_save_cannot_supply_cleanup_proof() {
        let mut owner = owner();
        let old = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: 34,
                    last_report_at_ms: 34,
                }),
            )
            .unwrap();
        let id = start_catalog(&mut owner);
        owner
            .observe_state(
                &old,
                crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                &position_payload(2),
            )
            .unwrap()
            .unwrap();
        publish_folder(&mut owner, "Saved", 35, 35);
        assert_saved_without_restoration(owner.advance_preset_workflow(id, 36).unwrap());
    }

    #[test]
    fn session_replacement_during_cleanup_preserves_verified_mutation() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_folder(&mut owner, "Saved", 34, 34);
        let cleanup = owner
            .advance_preset_workflow(id, 35)
            .unwrap()
            .operation
            .unwrap();
        owner.opened(36).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 36));
        assert_saved_without_restoration(owner.advance_preset_workflow(id, 37).unwrap());
        assert!(owner.prepare_dispatch(Some(&cleanup), None, 38).is_err());
    }

    #[test]
    fn save_position_fallback_requires_ordered_native_proof_and_unchanged_contents() {
        for case in 0..7 {
            let mut owner = owner();
            let id = start_catalog(&mut owner);
            let (first, last) = match case {
                0 => (None, 34),
                1 => (Some(1), 34),
                2 => (Some(2), 34), // same millisecond as the Save attempt
                3 => (Some(35), 34),
                4 => (Some(34), 50), // future observation cannot authorize now
                _ => (Some(34), 34),
            };
            publish_position(&mut owner, 2, first, last);
            if case == 5 {
                publish_source(&mut owner, changed_source(), Some(35), 35);
            }
            if case == 6 {
                owner.state.snapshot_mut().dirty = true;
            }
            publish_folder(&mut owner, "Saved", 35, 35);
            assert_saved_without_restoration(owner.advance_preset_workflow(id, 36).unwrap());
        }
    }

    #[test]
    fn native_catalog_resolution_precedes_cleanup_and_retains_the_actual_saved_name() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        // A publication the unit pushed before the attempt began is not evidence
        // of it: nothing resolves (this fixture's policy has no passive window,
        // so the next step is the directory request).
        publish_folder(&mut owner, "Saved", 1, 1);
        let pending = owner.advance_preset_workflow(id, 34).unwrap();
        assert!(pending.operation.is_none());
        assert_eq!(
            pending.catalog.unwrap().action,
            CatalogVerificationAction::Request
        );
        assert!(owner.preset_workflow_catalog_result(id).unwrap().is_none());
        // The unit's own push after the attempt (even before this stage began
        // settling) resolves it, with the name the unit actually stored.
        publish_folder(&mut owner, "Saved_1", 35, 35);
        let cleanup = owner.advance_preset_workflow(id, 36).unwrap();
        assert!(matches!(
            cleanup.action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
        assert_eq!(
            owner
                .preset_workflow_catalog_result(id)
                .unwrap()
                .unwrap()
                .saved_name,
            "Saved_1"
        );
    }

    #[test]
    fn catalog_matching_ignores_later_cache_edits_to_a_retained_native_listing() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_folder(&mut owner, "Previous", 34, 34);
        let mut cached = owner
            .state
            .preset_library()
            .observed_listing("/media/p4/Presets/Test")
            .unwrap()
            .clone();
        cached.files[0].name = "Saved".into();
        owner.state.ingest_preset_listing(cached);
        assert!(owner
            .advance_preset_workflow(id, 35)
            .unwrap()
            .catalog
            .is_some());
        assert!(owner.preset_workflow_catalog_result(id).unwrap().is_none());
        publish_folder(&mut owner, "Saved", 36, 36);
        assert!(matches!(
            owner.advance_preset_workflow(id, 37).unwrap().action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
    }

    #[test]
    fn catalog_refresh_context_cannot_outlive_its_policy_deadline() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        let catalog = owner
            .advance_preset_workflow(id, 34)
            .unwrap()
            .catalog
            .unwrap();
        assert_eq!(catalog.remaining_ms, 99);
        let scope = owner
            .begin_gateway_read_scope_for(&catalog.context, 35, 10_000)
            .unwrap();
        assert_eq!(scope.remaining_ms(35), 98);
        publish_folder(&mut owner, "Saved", 132, 132);
        // The payload was timely, but processing it after expiry cannot revive the phase.
        assert!(matches!(
            owner.advance_preset_workflow(id, 134).unwrap().action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
        assert!(owner.preset_workflow_catalog_result(id).unwrap().is_none());
    }

    #[test]
    fn replacement_catalog_cannot_complete_or_refresh_the_original_workflow() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        let catalog = owner
            .advance_preset_workflow(id, 34)
            .unwrap()
            .catalog
            .unwrap();
        owner.opened(35).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 35));
        publish_folder(&mut owner, "Saved", 36, 36);
        let step = owner.advance_preset_workflow(id, 37).unwrap();
        assert!(step.catalog.is_none());
        assert!(step.operation.is_none());
        assert!(matches!(step.action, PresetWorkflowAction::Finished(_)));
        assert!(owner.preset_workflow_catalog_result(id).unwrap().is_none());
        assert!(owner
            .begin_gateway_read_scope_for(&catalog.context, 37, 50)
            .is_err());
    }

    #[test]
    fn readback_requires_a_provably_post_attempt_native_preset_publication() {
        let mut owner = owner();
        let id = start_verified(&mut owner);
        for (first, now) in [(None, 4), (Some(1), 5), (Some(2), 6)] {
            publish_preset(&mut owner, first, now);
            let step = owner.advance_preset_workflow(id, now).unwrap();
            assert!(step.operation.is_none());
            assert!(step.readback.is_some());
            assert!(matches!(
                step.action,
                PresetWorkflowAction::AwaitReceipt { .. }
            ));
        }
        publish_preset(&mut owner, Some(7), 7);
        assert_eq!(
            owner.advance_preset_workflow(id, 7).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 37 }
        );
    }

    #[test]
    fn readback_checks_the_native_publication_not_later_cache_only_edits() {
        let mut owner = owner();
        let id = start_verified(&mut owner);
        owner.state.snapshot_mut().preset_position = 3;
        publish_preset(&mut owner, Some(4), 4);
        owner.state.snapshot_mut().preset_position = 2;
        assert!(owner
            .advance_preset_workflow(id, 5)
            .unwrap()
            .readback
            .is_some());
        publish_preset(&mut owner, Some(6), 6);
        // A later presentation edit cannot erase an already observed match.
        owner.state.snapshot_mut().preset_position = 3;
        assert_eq!(
            owner.advance_preset_workflow(id, 7).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 37 }
        );
    }

    #[test]
    fn readback_refresh_keeps_original_session_and_fixed_stage_deadline() {
        let mut owner = owner();
        let id = start_verified(&mut owner);
        let readback = owner
            .advance_preset_workflow(id, 4)
            .unwrap()
            .readback
            .unwrap();
        assert_eq!(
            readback.context.session_fence(),
            &owner.session_fence().unwrap()
        );
        assert!(readback.context.operands.is_none());
        assert_eq!(readback.remaining_ms, 99);
        let scope = owner
            .begin_gateway_read_scope_for(&readback.context, 10, 500)
            .unwrap();
        assert_eq!(scope.remaining_ms(10), 93);
        // Even a matching complete publication cannot extend the original expiry.
        publish_preset(&mut owner, Some(104), 104);
        assert_eq!(
            owner.advance_preset_workflow(id, 104).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 134 }
        );
    }

    #[test]
    fn readback_cannot_accept_a_publication_from_a_replacement_session() {
        let mut owner = owner();
        let id = start_verified(&mut owner);
        let readback = owner
            .advance_preset_workflow(id, 4)
            .unwrap()
            .readback
            .unwrap();
        owner.opened(5).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 5));
        publish_preset(&mut owner, Some(6), 6);
        let step = owner.advance_preset_workflow(id, 7).unwrap();
        assert!(step.operation.is_none());
        assert!(step.readback.is_none());
        let PresetWorkflowAction::Finished(outcome) = step.action else {
            panic!("replacement must terminate original workflow");
        };
        assert!(matches!(
            outcome.restoration,
            RestorationOutcome::Skipped(_)
        ));
        assert!(owner
            .begin_gateway_read_scope_for(&readback.context, 7, 50)
            .is_err());
    }

    #[test]
    fn native_delivery_advances_once_and_revokes_the_old_operation() {
        let mut owner = owner();
        let (id, operation, _) = start(&mut owner);
        let pending = owner.advance_preset_workflow(id, 1).unwrap();
        assert!(pending.operation.is_none());
        assert!(matches!(
            pending.action,
            PresetWorkflowAction::AwaitReceipt { .. }
        ));
        let delivery = prepare(&mut owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 3)
            .unwrap();
        assert_eq!(
            owner.advance_preset_workflow(id, 3).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 33 }
        );
        assert!(owner.prepare_dispatch(Some(&operation), None, 4).is_err());
        let next = owner.advance_preset_workflow(id, 33).unwrap();
        assert!(matches!(
            next.action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
        assert_ne!(next.operation.unwrap().identity(), operation.identity());
    }

    #[test]
    fn unrelated_delivery_and_wrong_or_extra_frames_cannot_satisfy_the_action() {
        let mut owner = owner();
        let (id, operation, _) = start(&mut owner);
        let unrelated = owner.plan_operation(0, 100).unwrap();
        let foreign = prepare(&mut owner, &unrelated);
        owner.begin_native_report(&foreign, 0, 2).unwrap();
        owner
            .finish_native_report(&foreign, 0, ReportOutcome::Delivered, 3)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_workflow(id, 4).unwrap().action,
            PresetWorkflowAction::AwaitReceipt { .. }
        ));
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 4).unwrap();
        assert!(owner.prepare_delivery(&dispatch, 49, 1, 4).is_err());
        let delivery = owner.prepare_delivery(&dispatch, 4, 1, 4).unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        assert!(owner.prepare_delivery(&dispatch, 4, 1, 4).is_err());
        owner.begin_native_report(&delivery, 0, 5).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 6)
            .unwrap();
        assert_eq!(
            owner.advance_preset_workflow(id, 6).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 36 }
        );
    }

    #[test]
    fn expired_in_flight_save_waits_for_final_callback_then_full_settlement() {
        let mut owner = owner();
        let (id, operation, expires) = start(&mut owner);
        let delivery = prepare(&mut owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        assert!(matches!(
            owner
                .advance_preset_workflow(id, expires + 100)
                .unwrap()
                .action,
            PresetWorkflowAction::AwaitReceipt { .. }
        ));
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, expires + 101)
            .unwrap();
        assert_eq!(
            owner
                .advance_preset_workflow(id, expires + 101)
                .unwrap()
                .action,
            PresetWorkflowAction::Wait {
                until_ms: expires + 131
            }
        );
        assert!(matches!(
            owner
                .advance_preset_workflow(id, expires + 130)
                .unwrap()
                .action,
            PresetWorkflowAction::Wait { .. }
        ));
    }

    #[test]
    fn logical_stop_does_not_start_settlement_before_the_native_call_returns() {
        let mut owner = owner();
        let (id, operation, _) = start(&mut owner);
        let delivery = prepare(&mut owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        owner
            .stop_delivery(&delivery, crate::device::delivery::DeliveryStop::Stopped, 3)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_workflow(id, 4).unwrap().action,
            PresetWorkflowAction::AwaitReceipt { .. }
        ));
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 50)
            .unwrap();
        assert_eq!(
            owner.advance_preset_workflow(id, 50).unwrap().action,
            PresetWorkflowAction::Wait { until_ms: 80 }
        );
        assert_eq!(
            owner.delivery_record(&delivery).unwrap().status,
            crate::device::delivery::DeliveryStatus::Indeterminate
        );
        assert!(owner.prepare_dispatch(Some(&operation), None, 51).is_err());
    }

    #[test]
    fn expired_queued_save_cannot_start_after_cleanup_is_issued() {
        let mut owner = owner();
        let (id, operation, expires) = start(&mut owner);
        let delivery = prepare(&mut owner, &operation);
        assert_eq!(
            owner.advance_preset_workflow(id, expires).unwrap().action,
            PresetWorkflowAction::Wait {
                until_ms: expires + 30
            }
        );
        assert!(owner
            .begin_native_report(&delivery, 0, expires + 1)
            .is_err());
        assert!(matches!(
            owner
                .advance_preset_workflow(id, expires + 30)
                .unwrap()
                .action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
    }

    #[test]
    fn reset_retains_skipped_outcome_and_rejects_old_ids_after_replacement() {
        let mut owner = owner();
        let (id, operation, _) = start(&mut owner);
        let context = owner.session_context().unwrap();
        assert!(owner.begin_preset_workflow(&context, plan(), 1).is_err());
        owner.opened(2).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 2));
        let PresetWorkflowAction::Finished(outcome) =
            owner.advance_preset_workflow(id, 3).unwrap().action
        else {
            panic!("old workflow must be terminal");
        };
        assert!(matches!(
            outcome.restoration,
            RestorationOutcome::Skipped(_)
        ));
        assert!(owner.prepare_dispatch(Some(&operation), None, 3).is_err());
        assert!(owner.begin_preset_workflow(&context, plan(), 3).is_err());
        publish_preset(&mut owner, Some(3), 3);
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Test".into();
        owner.state.snapshot_mut().preset_position = 1;
        let current = owner.session_context().unwrap();
        let replacement = owner.begin_preset_workflow(&current, plan(), 3).unwrap();
        assert_ne!(id, replacement);
        assert!(owner.advance_preset_workflow(id, 4).is_err());
        assert!(matches!(
            owner
                .advance_preset_workflow(replacement, 4)
                .unwrap()
                .action,
            PresetWorkflowAction::Dispatch { cleanup: false, .. }
        ));
    }

    #[test]
    fn original_preparation_deadline_cannot_be_renewed_by_starting_the_engine_later() {
        let mut owner = owner();
        let mut context = owner.session_context().unwrap();
        context.deadline = Some(crate::device::correlation::RequestDeadline::new(0, 10));
        let id = owner.begin_preset_workflow(&context, plan(), 8).unwrap();
        let step = owner.advance_preset_workflow(id, 10).unwrap();
        assert!(step.operation.is_none());
        let PresetWorkflowAction::Finished(outcome) = step.action else {
            panic!("expired preparation cannot dispatch");
        };
        assert!(outcome.primary_error.is_some());
        assert!(matches!(outcome.restoration, RestorationOutcome::Failed(_)));
    }

    #[test]
    fn verified_save_is_published_before_cleanup_and_survives_cleanup_failure() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_folder(&mut owner, "Saved_1", 34, 34);
        let step = owner.advance_preset_workflow(id, 35).unwrap();
        assert!(owner.preset_workflow_result(id).unwrap().is_none());
        assert!(matches!(
            step.action,
            PresetWorkflowAction::Dispatch { cleanup: true, .. }
        ));
        // Keep the completed operation's evidence while following later native
        // state. There is no version marker proving a later frame is stale.
        publish_folder(&mut owner, "Previous", 36, 36);
        assert_eq!(
            owner
                .state
                .preset_entry("/media/p4/Presets/Test", 2)
                .unwrap()
                .name,
            "Previous"
        );
        let dispatch = owner
            .prepare_dispatch(step.operation.as_ref(), None, 37)
            .unwrap();
        let delivery = owner
            .prepare_delivery(
                &dispatch,
                crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                1,
                37,
            )
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, 38).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Failed, 39)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_workflow(id, 40).unwrap().action,
            PresetWorkflowAction::Finished(_)
        ));
        let result = owner.preset_workflow_result(id).unwrap().unwrap();
        assert_eq!(result.persistent_delivery, PersistentDelivery::Delivered);
        let value = result.gateway_result().unwrap();
        assert_eq!(value["savedName"], "Saved_1");
        assert_eq!(value["verified"], true);
        assert_eq!(value["restoration"]["status"], "failed");
        assert!(value.get("changed").is_none());
        assert!(value.get("snapshot").is_none());
        crate::device::generated_gateway::validate_result("device.copyPreset", &value).unwrap();
    }

    #[test]
    fn skipped_cleanup_returns_verified_result_and_never_republishes_into_a_replacement() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_folder(&mut owner, "Saved", 34, 34);
        owner.advance_preset_workflow(id, 35).unwrap();
        owner.opened(36).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 36));
        publish_folder(&mut owner, "Replacement", 37, 37);
        owner.advance_preset_workflow(id, 38).unwrap();
        let before = owner
            .preset_workflow_result(id)
            .unwrap()
            .unwrap()
            .gateway_result()
            .unwrap();
        assert_eq!(before["verified"], true);
        assert_eq!(before["restoration"]["status"], "skipped");
        assert!(before.get("snapshot").is_none());
        assert_eq!(
            owner
                .state
                .preset_entry("/media/p4/Presets/Test", 2)
                .unwrap()
                .name,
            "Replacement"
        );
        owner.advance_preset_workflow(id, 39).unwrap();
        assert_eq!(
            owner
                .preset_workflow_result(id)
                .unwrap()
                .unwrap()
                .gateway_result()
                .unwrap(),
            before
        );
        assert_eq!(
            owner
                .state
                .preset_entry("/media/p4/Presets/Test", 2)
                .unwrap()
                .name,
            "Replacement"
        );
    }

    #[test]
    fn reset_during_native_call_retains_attempt_evidence_before_the_journal_is_reset() {
        for attempted in [false, true] {
            let mut owner = owner();
            let (id, operation, _) = start(&mut owner);
            let delivery = prepare(&mut owner, &operation);
            if attempted {
                owner.begin_native_report(&delivery, 0, 2).unwrap();
            }
            owner.opened(3).unwrap();
            owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 3));
            owner.advance_preset_workflow(id, 4).unwrap();
            let result = owner.preset_workflow_result(id).unwrap().unwrap();
            assert_eq!(
                result.persistent_delivery,
                if attempted {
                    PersistentDelivery::Attempted
                } else {
                    PersistentDelivery::NotAttempted
                }
            );
            let error = result.gateway_result().unwrap_err();
            if attempted {
                assert!(error.contains("outcome remains unconfirmed"));
            } else {
                assert!(error.contains("No persistent USB write was attempted"));
            }
            assert!(result.catalog.is_none());
            assert!(owner.preset_workflow_result(id + 1).is_err());
        }
    }

    #[test]
    fn failed_catalog_verification_does_not_publish_a_save_overlay() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_folder(&mut owner, "Previous", 34, 34);
        owner.advance_preset_workflow(id, 35).unwrap();
        owner.state.snapshot_mut().dirty = true;
        assert!(matches!(
            owner.advance_preset_workflow(id, 134).unwrap().action,
            PresetWorkflowAction::Finished(_)
        ));
        let result = owner.preset_workflow_result(id).unwrap().unwrap();
        assert!(result.catalog.is_none());
        assert_eq!(result.persistent_delivery, PersistentDelivery::Delivered);
        // The window ended with only a stale listing: delivered, not confirmed.
        // Not an error and not a quarantine trigger; no overlay is published.
        let unverified = result.gateway_result().unwrap();
        assert_eq!(unverified["verified"], false);
        assert_eq!(unverified["verification"], "accepted_unverified");
        assert!(unverified["detail"]
            .as_str()
            .unwrap()
            .contains("not verified"));
        assert_eq!(
            owner
                .state
                .preset_entry("/media/p4/Presets/Test", 2)
                .unwrap()
                .name,
            "Previous"
        );
    }

    #[test]
    fn deletion_evidence_updates_state_and_later_recreation_remains_visible() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let mut owner = owner();
        let key = "/media/p4/Presets/Test";
        publish_folder(&mut owner, "Saved", 0, 0);
        let mut plan = plan();
        plan.cleanup_stages.clear();
        plan.stages[0].settle_ms = 0;
        plan.stages[0].write = PlannedWrite::HidOperation(DeviceOperation::DeletePreset {
            setlist_key: key.into(),
            product_key: "device-key".into(),
        });
        plan.deletion = Some(crate::device::request::PresetDeletionVerification {
            setlist_key: key.into(),
            product_key: "device-key".into(),
            expected_name: "Saved".into(),
            expected_position: 2,
        });
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, plan, 0).unwrap();
        let operation = owner
            .advance_preset_workflow(id, 0)
            .unwrap()
            .operation
            .unwrap();
        let delivery = prepare(&mut owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 3)
            .unwrap();
        assert!(matches!(
            owner.advance_preset_workflow(id, 3).unwrap().action,
            PresetWorkflowAction::VerifyCatalog { .. }
        ));
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Delete as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                files: vec![pa::ProductData {
                    key: Some(pa::product_data::Key::Key("device-key".into())),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: 4,
                    last_report_at_ms: 4,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap();
        assert!(owner.state.preset_entry(key, 2).is_none());
        assert!(matches!(
            owner.advance_preset_workflow(id, 5).unwrap().action,
            PresetWorkflowAction::Finished(_)
        ));
        assert!(owner.state.preset_entry(key, 2).is_none());
        let result = owner
            .preset_workflow_result(id)
            .unwrap()
            .unwrap()
            .gateway_result()
            .unwrap();
        assert_eq!(result["verified"], true);
        assert_eq!(result["restoration"]["status"], "not_required");
        crate::device::generated_gateway::validate_result("device.deletePreset", &result).unwrap();
        publish_folder(&mut owner, "Saved", 6, 6);
        assert!(owner.state.preset_entry(key, 2).is_some());
        assert_eq!(
            owner
                .preset_workflow_result(id)
                .unwrap()
                .unwrap()
                .gateway_result()
                .unwrap(),
            result
        );
    }

    #[test]
    fn result_snapshot_is_a_matching_verified_readback_and_freezes_at_completion() {
        let mut owner = owner();
        let id = start_verified(&mut owner);
        publish_preset(&mut owner, Some(4), 4);
        owner.advance_preset_workflow(id, 5).unwrap();
        let cleanup = owner
            .advance_preset_workflow(id, 35)
            .unwrap()
            .operation
            .unwrap();
        let dispatch = owner.prepare_dispatch(Some(&cleanup), None, 36).unwrap();
        let delivery = owner
            .prepare_delivery(
                &dispatch,
                crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                1,
                36,
            )
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.begin_native_report(&delivery, 0, 37).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 38)
            .unwrap();
        owner.advance_preset_workflow(id, 39).unwrap();
        let before = owner
            .preset_workflow_result(id)
            .unwrap()
            .unwrap()
            .gateway_result()
            .unwrap();
        assert_eq!(before["snapshot"]["presetPosition"], 2);
        assert_eq!(before["verified"], false); // This fixture deliberately has no catalog verification.
        owner.state.snapshot_mut().preset_position = 9;
        publish_source(&mut owner, changed_source(), Some(40), 40);
        assert_eq!(
            owner
                .preset_workflow_result(id)
                .unwrap()
                .unwrap()
                .gateway_result()
                .unwrap(),
            before
        );
    }

    #[test]
    fn a_pending_catalog_match_cannot_publish_after_session_replacement() {
        let mut owner = owner();
        let id = start_catalog(&mut owner);
        publish_folder(&mut owner, "Saved", 34, 34);
        owner.opened(35).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 35));
        publish_folder(&mut owner, "Replacement", 36, 36);
        owner.advance_preset_workflow(id, 37).unwrap();
        let result = owner.preset_workflow_result(id).unwrap().unwrap();
        assert!(result.catalog.is_none());
        assert!(result.gateway_result().is_err());
        assert_eq!(
            owner
                .state
                .preset_entry("/media/p4/Presets/Test", 2)
                .unwrap()
                .name,
            "Replacement"
        );
    }

    #[test]
    fn save_early_file_envelopes_have_an_aggregate_budget_and_recover_after_drop() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let mut owner = owner();
        let mut mutation = plan();
        mutation
            .saved_presets
            .push(crate::device::request::PresetMutationRecord {
                setlist_key: mutation.setlist_key.clone(),
                position: 2,
                name: "Saved".into(),
                instrument: 0,
                allow_collision_suffix: true,
            });
        let context = owner.session_context().unwrap();
        let id = owner.begin_preset_workflow(&context, mutation, 0).unwrap();
        let operation = owner
            .advance_preset_workflow(id, 0)
            .unwrap()
            .operation
            .unwrap();
        let delivery = prepare(&mut owner, &operation);
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        // Empty nested records are tiny on the wire but much larger as decoded
        // structs. The retained lane stores their exact compact encoding instead.
        let compact = pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                files: vec![pa::ProductData::default(); 100_000],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(compact.len() < crate::profile::MAX_FRAME_BYTES);
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: 3,
                    last_report_at_ms: 3,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &compact)
            .unwrap();
        let entry = owner.preset_workflow.as_mut().unwrap();
        assert_eq!(entry.early_file_observations[0].2, compact);
        assert_eq!(entry.early_file_observation_bytes, compact.len());
        entry.early_file_observations.clear();
        entry.early_file_observation_bytes = 0;
        // A legal READ envelope is ignored by the native catalog but is accepted by
        // the observation lane; repeated envelopes previously grew the passive buffer.
        let message = pa::FileMessage {
            action: pa::message_action::Enum::Read as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key("/media/p4/Presets/Test".into())),
                files: (0..60)
                    .map(|index| pa::ProductData {
                        description: Some(pa::product_data::Description::Description(
                            "x".repeat(16 * 1024),
                        )),
                        index: Some(pa::product_data::Index::Index(index)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let payload = message.encode_to_vec();
        assert!(payload.len() < crate::profile::MAX_FRAME_BYTES);
        let accepted = EARLY_FILE_OBSERVATION_BYTES / payload.len();
        assert!(accepted < EARLY_FILE_OBSERVATION_LIMIT);
        for _ in 0..accepted {
            let ticket = owner
                .capture_observation_with_ingress(
                    owner.connection,
                    Some(crate::device::transport::IngressTime {
                        first_report_at_ms: 3,
                        last_report_at_ms: 3,
                    }),
                )
                .unwrap();
            owner
                .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &payload)
                .unwrap();
        }
        let entry = owner.preset_workflow.as_ref().unwrap();
        assert_eq!(entry.early_file_observations.len(), accepted);
        assert_eq!(entry.early_file_observation_bytes, accepted * payload.len());
        let ticket = owner
            .capture_observation_with_ingress(
                owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: 4,
                    last_report_at_ms: 4,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap();
        let entry = owner.preset_workflow.as_ref().unwrap();
        assert!(entry.early_file_observations.is_empty());
        assert_eq!(entry.early_file_observation_bytes, 0);
        assert!(!entry.runtime.is_terminal());
        // A smaller fresh device confirmation can still be retained and replayed.
        publish_folder(&mut owner, "Saved", 5, 5);
        assert_eq!(
            owner
                .preset_workflow
                .as_ref()
                .unwrap()
                .early_file_observations
                .len(),
            1
        );
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 6)
            .unwrap();
        owner.advance_preset_workflow(id, 6).unwrap();
        owner.advance_preset_workflow(id, 36).unwrap();
        assert_eq!(
            owner
                .preset_workflow
                .as_ref()
                .unwrap()
                .early_file_observation_bytes,
            0
        );
        assert!(owner
            .preset_workflow
            .as_ref()
            .unwrap()
            .early_file_observations
            .is_empty());
        owner.advance_preset_workflow(id, 37).unwrap();
        assert!(owner.preset_workflow_catalog_result(id).unwrap().is_some());
        owner.opened(38).unwrap();
        assert_eq!(
            owner
                .preset_workflow
                .as_ref()
                .unwrap()
                .early_file_observation_bytes,
            0
        );
        let mut reset_owner = super::tests::owner();
        let (_, reset_operation, _) = start(&mut reset_owner);
        let reset_delivery = prepare(&mut reset_owner, &reset_operation);
        reset_owner
            .begin_native_report(&reset_delivery, 0, 2)
            .unwrap();
        let ticket = reset_owner
            .capture_observation_with_ingress(
                reset_owner.connection,
                Some(crate::device::transport::IngressTime {
                    first_report_at_ms: 3,
                    last_report_at_ms: 3,
                }),
            )
            .unwrap();
        reset_owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap();
        assert!(
            reset_owner
                .preset_workflow
                .as_ref()
                .unwrap()
                .early_file_observation_bytes
                > 0
        );
        reset_owner.opened(4).unwrap();
        assert!(reset_owner
            .preset_workflow
            .as_ref()
            .unwrap()
            .early_file_observations
            .is_empty());
        assert_eq!(
            reset_owner
                .preset_workflow
                .as_ref()
                .unwrap()
                .early_file_observation_bytes,
            0
        );
    }

    #[test]
    fn source_checkpoint_setlist_key_comparison_is_case_insensitive() {
        // Regression: SourceCheckpoint::matches compared setlist keys with a
        // case-sensitive `==`, unlike deletion_target_inactive's sibling
        // guard in this same file, which correctly uses
        // .eq_ignore_ascii_case(...). CorOS setlist keys are not guaranteed
        // to round-trip with stable case, so a case-differing but otherwise
        // identical setlist key must still count as the same source.
        let owner = owner();
        let checkpoint = SourceCheckpoint::capture(&owner.state);
        let mut recased = owner;
        recased.state.snapshot_mut().setlist_key = "/MEDIA/p4/Presets/TEST".into();
        assert!(
            checkpoint.matches(&recased.state),
            "a case-differing but otherwise identical setlist key must still match"
        );
    }

    #[test]
    fn observe_preset_workflow_position_setlist_key_comparison_is_case_insensitive() {
        // Regression: the `matches` closure in observe_preset_workflow_position
        // compared setlist keys with a case-sensitive `==`. A native position
        // echo whose setlist key differs from the workflow's source only by
        // case must not be treated as navigation loss.
        let mut owner = owner();
        let _id = start_catalog(&mut owner);
        owner.state.snapshot_mut().setlist_key = "/MEDIA/p4/Presets/TEST".into();
        let ticket = owner
            .capture_observation_with_ingress(owner.connection, None)
            .unwrap();
        owner.observe_preset_workflow_position(&ticket);
        let entry = owner.preset_workflow.as_ref().unwrap();
        assert!(
            !entry.navigation_lost,
            "a case-differing but otherwise identical setlist key must not be navigation loss"
        );
    }
}
