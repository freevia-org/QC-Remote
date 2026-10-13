//! Atomic native session ownership. Hosts lock this owner once per transition;
//! expensive metadata parsing happens outside it and returns an ingress ticket.

mod backup;
mod catalog_observations;
mod library_catalog;
mod preset_workflow;
mod session_runtime;
mod suspension;
pub use catalog_observations::CatalogInvalidation;
pub use preset_workflow::{
    OwnedPresetPreparationAction, OwnedPresetWorkflowAction, OwnedPresetWorkflowCatalog,
    OwnedPresetWorkflowReadback, PersistentDelivery, PresetWorkflowResult,
};
pub use suspension::{
    ActiveSuspension, EndedSuspension, SuspensionDiagnostics, SuspensionEnd,
    DEVICE_SUSPENDED_MARKER,
};

use crate::device::delivery::{
    DeliveryJournal, DeliveryRecord, DeliveryStop, DeliveryTicket, ReportOutcome,
};
use crate::device::request::{GatewayVerificationAction, GatewayVerificationRuntime};
use crate::device::session::{SessionDriver, SessionEpoch};
use crate::device::state_runtime::DeviceStateRuntime;
use crate::device::transport::{HandshakeWrite, IngressTime, ReportLayout, TransportRuntime};
use crate::inbound_telemetry::InboundTelemetrySnapshot;
use crate::sync::{allocate_next_id, next_identity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How `clear_lifecycle_scoped_state` should stop outstanding deliveries.
/// `DeliveryJournal` exposes `reset()` (always reason `SessionChanged`, no
/// finish timestamp) and `stop_all(reason, now_ms)` as two distinct public
/// entry points, so callers pick between them rather than the helper
/// pretending they are interchangeable. Every `StopAll` caller stops for the
/// same reason (a session change), so the reason is hardcoded in
/// `clear_lifecycle_scoped_state`'s match arm rather than carried here.
enum DeliveryWipe {
    Reset,
    StopAll(u64),
}

struct GatewayVerificationEntry {
    context: DispatchContext,
    runtime: GatewayVerificationRuntime,
    preset_scope: Option<crate::device::operands::PresetVerificationScope>,
}

/// Why a retained preset-catalog read found nothing to project. Each value
/// names the first gate in [`SessionRuntime::retained_preset_catalog`] that
/// kept it empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresetCatalogAbsenceReason {
    /// No File frame produced a native catalog change on this connection.
    NoCatalogFrame,
    /// Retained evidence was invalidated; `invalidation` says why.
    Invalidated,
    /// The session epoch moved since the catalog was stored: either the
    /// retained origin belongs to another epoch, or an in-session rebuild
    /// discarded the File frames this connection had received.
    EpochChanged,
    /// The newest contributing File frame finished arriving after the read.
    IngressPending,
    /// The catalog is retained but holds no listing for the requested setlist.
    SetlistKeyMissing,
    /// The catalog is retained but holds no preset folder at all.
    NoFolders,
    /// Nothing is missing: the projection is available now.
    Available,
}

/// Machine-readable explanation of an empty retained preset catalog.
/// Diagnostics only: building it changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetCatalogAbsence {
    /// The first gate that kept the projection empty.
    pub reason: PresetCatalogAbsenceReason,
    /// Set once retained evidence was invalidated (the first cause).
    pub invalidation: Option<CatalogInvalidation>,
    /// File frames reduced into the retained catalog since it was last cleared.
    pub file_frames_observed: u64,
    /// Of those, frames that changed the native catalog.
    pub catalog_changes_observed: u64,
    /// File frames that in-session rebuilds discarded on this connection.
    pub file_frames_discarded_by_rebuild: u64,
    /// Folders retained, usable or not.
    pub retained_folders: usize,
    /// The owner's native connection identity.
    pub connection: u64,
    /// The current epoch revision: in-session rebuilds of this session id.
    pub session_revision: Option<u64>,
    /// The epoch revision of the frame that started the retained catalog.
    pub origin_revision: Option<u64>,
    /// Rebuilds of the current startup attempt (reset once connected).
    pub startup_rebuilds: Option<u32>,
    /// Whether staged startup reached `Connected`. The QC publishes its whole
    /// directory once, inside the Booting burst, and every File frame of that
    /// burst is reduced in ingress order before the Updater frame that ends
    /// it, so a connected session's retained catalog is the complete
    /// directory, not a partial one that is still filling.
    pub startup_complete: bool,
    /// Preset setlist folders in the retained catalog, or `None` while the
    /// catalog cannot be read at all. A count only: folder names are user data.
    pub known_setlists: Option<usize>,
    /// For [`PresetCatalogAbsenceReason::SetlistKeyMissing`] only: whether the
    /// retained catalog holds any listing under the requested key. `false`
    /// means the key itself is unknown; `true` means its listing is present
    /// but does not cover what the read needs yet.
    pub setlist_key_listed: Option<bool>,
}

impl PresetCatalogAbsence {
    /// The requested setlist key names no folder of a complete, retained,
    /// non-empty directory. Waiting cannot change that answer: the directory
    /// is published once per USB session, so the read should fail now rather
    /// than wait out its budget.
    pub fn setlist_key_unknown(&self) -> bool {
        self.reason == PresetCatalogAbsenceReason::SetlistKeyMissing
            && self.startup_complete
            && self.known_setlists.is_some_and(|count| count > 0)
            && self.setlist_key_listed == Some(false)
    }
}

/// Provenance token for one observation read from the device.
///
/// It records the native connection, lifecycle epoch and a per-connection sequence number, so the owner can later tell whether the observation still belongs to the current session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationTicket {
    connection: u64,
    epoch: Option<SessionEpoch>,
    sequence: u64,
    ingress: Option<IngressTime>,
}

impl ObservationTicket {
    /// Returns the native read timing recorded when the observation was captured, if any.
    pub fn ingress(&self) -> Option<IngressTime> {
        self.ingress
    }

    /// Provenance compatibility for cumulative host caches. This does not
    /// establish current-session validity or freshness of individual fields.
    pub fn same_session_as(&self, other: &Self) -> bool {
        self.connection == other.connection && self.epoch.is_some() && self.epoch == other.epoch
    }
}

/// A ModelRepo payload queued for off-lock parsing, with the observation it came from.
pub struct ModelRepoWork {
    /// Observation the payload was read under.
    pub ticket: ObservationTicket,
    /// Raw ModelRepo payload bytes.
    pub payload: Vec<u8>,
}

/// Identifies one device session: the native connection and its lifecycle epoch.
///
/// A fence stays valid only while the owner is still in that session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFence {
    connection: u64,
    epoch: SessionEpoch,
}

/// Session scope retained for one logical operation.
///
/// It carries the [`SessionFence`], the operands captured from the planned write (if any) and an optional overall deadline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchContext {
    session: SessionFence,
    operands: Option<crate::device::operands::DispatchOperands>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deadline: Option<crate::device::correlation::RequestDeadline>,
}

impl DispatchContext {
    /// Returns the session this context is bound to.
    pub fn session_fence(&self) -> &SessionFence {
        &self.session
    }

    fn phase_deadline(
        &self,
        started_ms: u64,
        timeout_ms: u64,
    ) -> crate::device::correlation::RequestDeadline {
        let phase = crate::device::correlation::RequestDeadline::new(started_ms, timeout_ms);
        self.deadline
            .map_or(phase, |overall| phase.intersect(overall))
    }
}

/// One logical read may have several protocol requests, but only one session
/// and deadline. Retries and composite follow-ups consume this same scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayReadScope {
    context: DispatchContext,
    deadline: crate::device::correlation::RequestDeadline,
}

impl GatewayReadScope {
    /// Returns the dispatch context this read scope belongs to.
    pub fn context(&self) -> &DispatchContext {
        &self.context
    }
    /// Returns the milliseconds left in the scope's deadline at `now_ms`, or zero once it has expired.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.deadline.remaining_ms(now_ms)
    }
}

/// Payload-free identity retained by delivery records. Admission operands stay
/// private to the command path, not in exported delivery evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationIdentity {
    connection: u64,
    epoch: SessionEpoch,
    sequence: u64,
}

/// One caller budget and identity for a logical write, including every message
/// in a paced batch. Admission does not establish native delivery or readback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationTicket {
    connection: u64,
    epoch: SessionEpoch,
    sequence: u64,
    deadline: crate::device::correlation::RequestDeadline,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operands: Option<crate::device::operands::DispatchOperands>,
    #[serde(default)]
    backup: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow: Option<crate::device::preset_workflow::WorkflowActionTicket>,
}

impl OperationTicket {
    /// Returns the payload-free identity of this operation.
    pub fn identity(&self) -> OperationIdentity {
        OperationIdentity {
            connection: self.connection,
            epoch: self.epoch.clone(),
            sequence: self.sequence,
        }
    }
    /// Returns the milliseconds left in the operation's deadline at `now_ms`, or zero once it has expired.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.deadline.remaining_ms(now_ms)
    }
}

/// Authorization to dispatch native reports in a session.
///
/// It carries the session fence, the optional [`OperationTicket`] being sent and, for capture retargeting, the model revision it was prepared against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeDispatchTicket {
    fence: SessionFence,
    operation: Option<OperationTicket>,
    capture_revision: Option<u64>,
}

impl NativeDispatchTicket {
    pub(crate) fn operation(&self) -> Option<&OperationTicket> {
        self.operation.as_ref()
    }
    /// Returns the milliseconds left in the associated operation's deadline at `now_ms`, or `None` if the dispatch has no operation.
    pub fn remaining_ms(&self, now_ms: u64) -> Option<u64> {
        self.operation
            .as_ref()
            .map(|operation| operation.remaining_ms(now_ms))
    }
}

/// These components share a single host mutex. In particular, checking an
/// observation's provenance and applying it must happen under that same lock.
/// The public component fields support migration of existing native operations;
/// they do not confer ownership of a second connection or independent state.
pub struct SessionRuntime {
    /// Decoded device state and its published snapshot.
    pub state: DeviceStateRuntime,
    /// Native transport state: reports, framing and the connection policy.
    pub transport: TransportRuntime,
    /// Session lifecycle driver; `None` until a handshake attempt has been selected.
    pub lifecycle: Option<SessionDriver>,
    connection: u64,
    open: bool,
    next_observation: u64,
    next_state_revision: u64,
    publications: crate::device::publication::StatePublications,
    next_operation: u64,
    next_verification: u64,
    report_epoch: u64,
    report_fence: Option<SessionFence>,
    preset_observation: Option<ObservationTicket>,
    metadata_pending: Option<ModelRepoWork>,
    metadata_active: Option<ObservationTicket>,
    metadata_latest: Option<ObservationTicket>,
    deliveries: DeliveryJournal,
    backup: Option<backup::BackupEntry>,
    last_backup_operation: u64,
    next_workflow: u64,
    preset_preparation: Option<preset_workflow::PresetPreparationEntry>,
    preset_workflow: Option<preset_workflow::PresetWorkflowEntry>,
    gateway_verifications: HashMap<u64, GatewayVerificationEntry>,
    catalog_observations: catalog_observations::CatalogObservations,
    library_catalog: library_catalog::LibraryCatalog,
    suspension: suspension::SuspensionTracker,
    /// The CorOS version the unit stated in its latest update request this session.
    cor_os_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::cortex_protobuf_v2 as pa;
    use prost::Message;

    fn live_owner() -> (SessionRuntime, u64) {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        (owner, connection)
    }

    /// Installs a ModelRepo that lists `ids` as selectable models, as the startup gate does in a
    /// real session. Pinning a model is admitted only against such a catalog.
    fn install_listed_models(owner: &mut SessionRuntime, ids: &[u32]) {
        let models: String = ids
            .iter()
            .map(|id| format!(r#"<Model id="{id}" name="Model {id}"/>"#))
            .collect();
        let xml = format!(r#"<Models><Category name="Delay">{models}</Category></Models>"#);
        let payload = pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                xml.into_bytes(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        owner
            .state
            .install_model_catalog(crate::state::parse_model_repo(&payload).unwrap());
    }

    fn update_request_frame(version: &str) -> Vec<u8> {
        pa::UpdaterForwardMessage {
            action: pa::message_action::Enum::Update as i32,
            updater_request_id: 3,
            request: Some(pa::updater_forward_message::Request::Request(
                pa::UpdaterForwardRequest {
                    r#type: pa::updater_forward_request_type::Enum::Check as i32,
                    url: "https://cloud-api.example.test/firmware/v1/latest".into(),
                    timeout: 60_000,
                    headers: vec![pa::ProductForwardHeaderPair {
                        name: "X-Cloud-Api-Hardware-Source-Version".into(),
                        value: version.into(),
                    }],
                    payload: None,
                },
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn the_session_keeps_the_cor_os_version_the_unit_stated_until_the_session_ends() {
        let (mut owner, connection) = live_owner();
        assert_eq!(owner.observed_cor_os_version(), None);

        let ticket = owner.capture_observation(connection).unwrap();
        owner
            .observe_state(
                &ticket,
                crate::profile::MESSAGE_TYPE_UPDATER_FORWARD,
                &update_request_frame("4.1.0"),
            )
            .ok();
        assert_eq!(owner.observed_cor_os_version(), Some("4.1.0"));

        // A frame that states nothing usable does not erase what the unit said.
        owner
            .observe_state(
                &ticket,
                crate::profile::MESSAGE_TYPE_UPDATER_FORWARD,
                &update_request_frame("not-a-version"),
            )
            .ok();
        assert_eq!(owner.observed_cor_os_version(), Some("4.1.0"));

        // The next session may be a different unit running different software.
        owner.reset_state();
        assert_eq!(owner.observed_cor_os_version(), None);
    }

    #[test]
    fn a_stale_observation_does_not_set_the_cor_os_version() {
        let (mut owner, connection) = live_owner();
        let stale = owner.capture_observation(connection).unwrap();
        owner.reset_state();
        let connection = owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(connection, "next".into(), 1));
        owner
            .observe_state(
                &stale,
                crate::profile::MESSAGE_TYPE_UPDATER_FORWARD,
                &update_request_frame("4.1.0"),
            )
            .ok();
        assert_eq!(owner.observed_cor_os_version(), None);
    }

    #[test]
    fn clear_lifecycle_scoped_state_wipes_shared_fields_but_never_lifecycle_or_fence() {
        // Regression for the shared clear_lifecycle_scoped_state extraction:
        // install_model_catalog and observe_lifecycle deliberately keep the
        // lifecycle driver and its report fence alive on failure (both call
        // sites immediately do `self.lifecycle.as_mut().expect(...)` again),
        // while clear_session_state is the only caller that also gives those
        // two up. The shared helper must only ever touch the fields that are
        // identical across all three call sites.
        let (mut owner, connection) = live_owner();
        owner.report_fence = Some(owner.session_fence().unwrap());
        owner.preset_observation = Some(ObservationTicket {
            connection,
            epoch: None,
            sequence: 1,
            ingress: None,
        });
        let fence_before = owner.report_fence.clone();
        let lifecycle_epoch_before = owner.lifecycle.as_ref().map(SessionDriver::epoch);

        owner.clear_lifecycle_scoped_state(DeliveryWipe::StopAll(42));

        assert!(owner.preset_observation.is_none());
        assert!(
            owner.lifecycle.is_some(),
            "the narrower wipe must not drop the lifecycle driver"
        );
        assert_eq!(
            owner.lifecycle.as_ref().map(SessionDriver::epoch),
            lifecycle_epoch_before,
            "the narrower wipe must not touch the lifecycle at all"
        );
        assert_eq!(
            owner.report_fence, fence_before,
            "the narrower wipe must not touch the report fence"
        );

        // clear_session_state (used by reset_state/opened) is the one path
        // that also gives up the lifecycle and its fence.
        owner.clear_session_state();
        assert!(owner.lifecycle.is_none());
        assert!(owner.report_fence.is_none());
    }

    fn preflight_context(
        owner: &SessionRuntime,
        method: &str,
        params: &serde_json::Value,
    ) -> DispatchContext {
        let plan = crate::device::request::plan_gateway_write(method, params, None).unwrap();
        owner
            .context_for_write(method, params, &plan.write)
            .unwrap()
    }

    fn observe_preflight(owner: &mut SessionRuntime, connection: u64, kind: u16, payload: Vec<u8>) {
        let ticket = owner
            .capture_observation_with_ingress(connection, None)
            .unwrap();
        owner
            .observe_state(&ticket, kind, &payload)
            .unwrap()
            .unwrap();
    }

    fn tempo_publication(values: &[(Option<u32>, f32)]) -> Vec<u8> {
        use crate::proto::{cortex_protobuf_v2 as pa, param, param_value, Param, ParamValue};
        use prost::Message;
        pa::GlobalTempoMessage {
            action: pa::message_action::Enum::Update as i32,
            params: values
                .iter()
                .map(|(index, value)| Param {
                    index: index.map(param::Index::Index),
                    param_values: vec![ParamValue {
                        value: Some(param_value::Value::FloatValue(*value)),
                    }],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[test]
    fn external_preflight_survives_queueing_and_rejects_a_changed_tempo_at_native_attempt() {
        use crate::profile::MESSAGE_TYPE_GLOBAL_TEMPO as TEMPO;
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let method = "device.setGlobalTempo";
        let params = json!({"bpm":121,"expectedMode":"GLOBAL","expectedGlobalBpm":120});
        let context = preflight_context(&owner, method, &params);
        assert!(owner
            .plan_operation_in_context(&context, 0, 100)
            .unwrap_err()
            .contains("not bound"));
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(None, 0.4), (Some(1), 1.0)]),
        );
        let (bound, decision) = owner
            .bind_write_preflight(
                &context,
                method,
                &params,
                &json!({"mode":"GLOBAL","globalBpm":120}),
                0,
            )
            .unwrap();
        assert_eq!(
            decision,
            crate::device::request::GatewayWritePreflightDecision::Proceed
        );
        let bound = serde_json::from_str(&serde_json::to_string(&bound).unwrap()).unwrap();
        let operation = owner.plan_operation_in_context(&bound, 0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, TEMPO, 1, 0).unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        // Metronome publications do not change the BPM/mode this write used.
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(2), 1.0)]),
        );
        assert!(owner.validate_operation(&operation, 1).is_ok());
        // A later sparse mode change reaches the owner before native transmission.
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(1), 0.0)]),
        );
        assert!(owner
            .begin_native_report(&delivery, 0, 2)
            .unwrap_err()
            .contains("target changed"));
        let record = owner.delivery_record(&delivery).unwrap();
        assert_eq!(record.started_at_ms, None);
        assert_eq!(record.delivered_reports, 0);
    }

    #[test]
    fn tempo_preflight_cannot_bind_a_stale_response_or_adopt_a_replacement_session() {
        use crate::profile::MESSAGE_TYPE_GLOBAL_TEMPO as TEMPO;
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let method = "device.setGlobalTempo";
        let params = json!({"bpm":121,"expectedMode":"GLOBAL","expectedGlobalBpm":120});
        let context = preflight_context(&owner, method, &params);
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(0), 0.5), (Some(1), 1.0)]),
        );
        assert!(owner
            .bind_write_preflight(
                &context,
                method,
                &params,
                &json!({"mode":"GLOBAL","globalBpm":120}),
                0
            )
            .unwrap_err()
            .contains("changed before binding"));
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(0), 0.4)]),
        );
        let response = json!({"mode":"GLOBAL","globalBpm":120});
        let (bound, _) = owner
            .bind_write_preflight(&context, method, &params, &response, 0)
            .unwrap();
        // A malformed value for the relevant field invalidates its prior value.
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(0), f32::NAN)]),
        );
        assert!(owner.plan_operation_in_context(&bound, 1, 100).is_err());
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 2));
        assert!(owner
            .bind_write_preflight(&context, method, &params, &response, 0)
            .unwrap_err()
            .contains("session changed"));
    }

    #[test]
    fn scene_preflight_rejects_a_stale_noop_and_retains_the_planned_scene_until_dispatch() {
        use crate::{commands::DeviceCommand, profile::MESSAGE_TYPE_SCENE as SCENE};
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let snapshot = owner.state.snapshot_mut();
        snapshot.has_preset = true;
        snapshot.preset_name = "Preset".into();
        snapshot.setlist_key = "My Presets".into();
        snapshot.preset_position = 3;
        snapshot.active_scene = 2;
        let method = "device.selectScene";
        let params = json!({"scene":2,"expectedPresetName":"Preset"});
        let context = owner
            .context_for_gateway_write(
                method,
                &params,
                &crate::device::request::PlannedWrite::HidCommand(DeviceCommand::SelectScene(2)),
                0,
            )
            .unwrap();
        assert!(owner
            .plan_operation_in_context(&context, 0, 100)
            .unwrap_err()
            .contains("not bound"));
        let old_snapshot = json!({"presetName":"Preset","activeScene":2});
        // Valid Scene UPDATE to scene 1 reaches the owner after the host cloned
        // the preflight snapshot but before it decides whether to suppress a write.
        observe_preflight(&mut owner, connection, SCENE, vec![8, 1, 24, 1]);
        assert!(owner
            .bind_write_preflight(&context, method, &params, &old_snapshot, 1)
            .unwrap_err()
            .contains("changed before binding"));
        // Caller arguments changed after planning cannot turn this into a no-op
        // for a different scene: the retained desired scene is still 2.
        let (bound, decision) = owner
            .bind_write_preflight(
                &context,
                method,
                &json!({"scene":1,"expectedPresetName":"Preset"}),
                &json!({"presetName":"Preset","activeScene":1}),
                1,
            )
            .unwrap();
        assert_eq!(
            decision,
            crate::device::request::GatewayWritePreflightDecision::Proceed
        );
        let bound = serde_json::from_str(&serde_json::to_string(&bound).unwrap()).unwrap();
        let operation = owner.plan_operation_in_context(&bound, 1, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 1).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, SCENE, 1, 1).unwrap();
        observe_preflight(&mut owner, connection, SCENE, vec![8, 1, 24, 0]);
        assert!(owner
            .begin_native_report(&delivery, 0, 2)
            .unwrap_err()
            .contains("target changed"));
        assert_eq!(
            owner.delivery_record(&delivery).unwrap().delivered_reports,
            0
        );
        observe_preflight(&mut owner, connection, SCENE, vec![8, 1, 24, 2]);
        let (_, decision) = owner
            .bind_write_preflight(&context, method, &params, &old_snapshot, 3)
            .unwrap();
        assert_eq!(
            decision,
            crate::device::request::GatewayWritePreflightDecision::AlreadyMatched
        );
    }

    #[test]
    fn a_started_scene_write_can_verify_its_own_scene_update() {
        use crate::device::request::{
            gateway_write_verification_policy, GatewayVerification, PlannedWrite,
        };
        use crate::{commands::DeviceCommand, profile::MESSAGE_TYPE_SCENE as SCENE};
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        owner.state.snapshot_mut().has_preset = true;
        owner.state.snapshot_mut().preset_name = "Preset".into();
        let method = "device.selectScene";
        let params = json!({"scene":2,"expectedPresetName":"Preset"});
        let context = owner
            .context_for_gateway_write(
                method,
                &params,
                &PlannedWrite::HidCommand(DeviceCommand::SelectScene(2)),
                0,
            )
            .unwrap();
        let (context, _) = owner
            .bind_write_preflight(
                &context,
                method,
                &params,
                &json!({"presetName":"Preset","activeScene":0}),
                1,
            )
            .unwrap();
        let operation = owner.plan_operation_in_context(&context, 1, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 1).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, SCENE, 1, 1).unwrap();
        owner.begin_native_report(&delivery, 0, 2).unwrap();
        observe_preflight(&mut owner, connection, SCENE, vec![8, 1, 24, 2]);
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 3)
            .unwrap();
        assert!(owner.validate_operation(&operation, 3).is_ok());
        let wait = owner
            .begin_gateway_verification(
                &context,
                GatewayVerificationRuntime::new(
                    GatewayVerification::Scene { scene: 2 },
                    0,
                    3,
                    &gateway_write_verification_policy(method),
                ),
            )
            .unwrap();
        let (action, _, _) = owner.advance_gateway_verification(wait, 1, 4).unwrap();
        assert_eq!(action, GatewayVerificationAction::Verified);
    }

    #[test]
    fn verification_rejects_a_matching_value_on_another_preset_or_after_aba() {
        use crate::commands::DeviceCommand;
        use crate::device::request::{
            gateway_write_verification_policy, GatewayVerification, PlannedWrite,
        };
        use serde_json::json;
        for transition in ["setlist", "slot", "aba", "repeated"] {
            for before_start in [false, true] {
                let (mut owner, _) = live_owner();
                let snapshot = owner.state.snapshot_mut();
                snapshot.has_preset = true;
                snapshot.preset_name = "Same name".into();
                snapshot.setlist_key = "/user/A".into();
                snapshot.preset_position = 19;
                let context = owner
                    .context_for_gateway_write(
                        "device.setTempo",
                        &json!({"bpm":130,"expectedTempo":120,"expectedPresetName":"Same name"}),
                        &PlannedWrite::HidCommand(DeviceCommand::SetTempo(130)),
                        0,
                    )
                    .unwrap();
                let runtime = GatewayVerificationRuntime::new(
                    GatewayVerification::Tempo { bpm: 130 },
                    0,
                    1,
                    &gateway_write_verification_policy("device.setTempo"),
                );
                let wait = if before_start {
                    None
                } else {
                    Some(
                        owner
                            .begin_gateway_verification(&context, runtime.clone())
                            .unwrap(),
                    )
                };
                let key = if transition == "setlist" || transition == "aba" {
                    "/user/B"
                } else {
                    "/user/A/"
                };
                let position = if transition == "slot" { 20 } else { 19 };
                let update = serde_json::from_value(
                    json!({"kind":"position","setlistKey":key,"position":position}),
                )
                .unwrap();
                owner.state.snapshot_mut().apply(&update);
                if transition == "aba" {
                    owner.state.snapshot_mut().apply(
                        &serde_json::from_value(
                            json!({"kind":"position","setlistKey":"/user/A","position":19}),
                        )
                        .unwrap(),
                    );
                }
                owner.state.snapshot_mut().tempo = 130;
                if before_start && transition != "repeated" {
                    assert!(owner
                        .begin_gateway_verification(&context, runtime)
                        .unwrap_err()
                        .contains("preset changed"));
                    continue;
                }
                let wait = wait.unwrap_or_else(|| {
                    owner.begin_gateway_verification(&context, runtime).unwrap()
                });
                let result = owner.advance_gateway_verification(wait, 1, 2);
                if transition == "repeated" {
                    assert_eq!(result.unwrap().0, GatewayVerificationAction::Verified);
                } else {
                    assert!(result.unwrap_err().contains("preset changed"));
                    assert!(owner
                        .advance_gateway_verification(wait, 2, 3)
                        .unwrap_err()
                        .contains("no longer active"));
                }
            }
        }
    }

    #[test]
    fn a_preset_change_does_not_invalidate_device_global_volume_verification() {
        use crate::commands::DeviceCommand;
        use crate::device::request::{
            gateway_write_verification_policy, GatewayVerification, PlannedWrite,
        };
        use serde_json::json;
        let (mut owner, _) = live_owner();
        owner.state.snapshot_mut().has_preset = true;
        let context = owner
            .context_for_gateway_write(
                "device.setMasterVolume",
                &json!({"value":51,"expectedValue":0}),
                &PlannedWrite::HidCommand(DeviceCommand::SetMasterVolume(0.51)),
                0,
            )
            .unwrap();
        let wait = owner
            .begin_gateway_verification(
                &context,
                GatewayVerificationRuntime::new(
                    GatewayVerification::MasterVolume { value: 51 },
                    0,
                    1,
                    &gateway_write_verification_policy("device.setMasterVolume"),
                ),
            )
            .unwrap();
        owner.state.snapshot_mut().apply(
            &serde_json::from_value(
                json!({"kind":"position","setlistKey":"/user/B","position":19}),
            )
            .unwrap(),
        );
        owner.state.snapshot_mut().master_volume = 51;
        assert_eq!(
            owner.advance_gateway_verification(wait, 1, 2).unwrap().0,
            GatewayVerificationAction::Verified
        );
    }

    #[test]
    fn navigation_verifies_its_authorized_destination_instead_of_the_source() {
        use crate::device::request::{
            gateway_write_verification_policy, plan_preset_recall, PlannedWrite,
        };
        use serde_json::json;
        let (mut owner, _) = live_owner();
        owner.state.snapshot_mut().has_preset = true;
        owner.state.snapshot_mut().preset_name = "Same name".into();
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/A".into();
        // A recall is admitted only on an observed dirty flag.
        owner
            .state
            .snapshot_mut()
            .apply(&serde_json::from_value(json!({"kind": "dirty", "dirty": false})).unwrap());
        let method = "device.recallPreset";
        let params = json!({"setlistKey":"/media/p4/Presets/B", "position":19,
            "expectedPresetName":"Same name", "expectedPosition":0,
            "expectedSetlistKey":"/media/p4/Presets/A"});
        let plan = plan_preset_recall(method, &params, Some(owner.state.snapshot())).unwrap();
        let context = owner
            .context_for_gateway_write(
                method,
                &params,
                &PlannedWrite::HidCommand(plan.command.clone()),
                0,
            )
            .unwrap();
        let wait = owner
            .begin_gateway_verification(
                &context,
                GatewayVerificationRuntime::new(
                    plan.verification(),
                    0,
                    1,
                    &gateway_write_verification_policy(method),
                ),
            )
            .unwrap();
        owner.state.snapshot_mut().apply(
            &serde_json::from_value(json!({"kind":"position",
            "setlistKey":"/media/p4/Presets/B", "position":19}))
            .unwrap(),
        );
        owner.state.snapshot_mut().full_preset_revision += 1;
        assert_eq!(
            owner.advance_gateway_verification(wait, 1, 2).unwrap().0,
            GatewayVerificationAction::Verified
        );
    }

    #[test]
    fn one_write_deadline_caps_preflight_queueing_readback_and_snapshot_verification() {
        use crate::device::request::{
            gateway_verification_policy, GatewayVerification, GatewayVerificationRuntime,
        };
        use crate::profile::MESSAGE_TYPE_GLOBAL_TEMPO as TEMPO;
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let method = "device.setGlobalTempo";
        let params = json!({"bpm":121,"expectedMode":"GLOBAL","expectedGlobalBpm":120});
        let plan = crate::device::request::plan_gateway_write(method, &params, None).unwrap();
        let original = owner
            .context_for_gateway_write(method, &params, &plan.write, 100)
            .unwrap();
        let end = original.deadline.unwrap().expires_at_ms();
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(0), 0.4), (Some(1), 1.0)]),
        );
        let response = json!({"mode":"GLOBAL","globalBpm":120});
        let (bound, _) = owner
            .bind_write_preflight(&original, method, &params, &response, end - 20)
            .unwrap();
        let bound: DispatchContext =
            serde_json::from_str(&serde_json::to_string(&bound).unwrap()).unwrap();
        // Starting a new host phase does not reset time spent planning or waiting.
        let queued = owner
            .plan_operation_in_context(&bound, end - 20, 3_000)
            .unwrap();
        assert_eq!(queued.remaining_ms(end - 1), 1);
        assert!(owner.validate_operation(&queued, end).is_err());
        assert!(owner
            .plan_operation_in_context(&bound, end, 60_000)
            .is_err());
        assert!(owner
            .bind_write_preflight(&original, method, &params, &response, end)
            .unwrap_err()
            .contains("budget expired"));
        // Readback strips pre-write values but retains the overall deadline.
        let readback = owner
            .begin_gateway_read_scope_for(&bound, end - 10, 3_000)
            .unwrap();
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(0), 0.405)]),
        );
        assert!(owner
            .plan_operation_in_context(readback.context(), end - 5, 3_000)
            .is_ok());
        assert_eq!(
            owner
                .gateway_read_scope_remaining(&readback, end - 1)
                .unwrap(),
            1
        );
        assert!(owner.gateway_read_scope_remaining(&readback, end).is_err());
        let refresh = owner
            .begin_gateway_read_scope_for(readback.context(), end - 5, 60_000)
            .unwrap();
        assert_eq!(refresh.remaining_ms(end - 1), 1);
        // Even an exact desired snapshot arriving at expiry cannot verify.
        owner.state.snapshot_mut().has_preset = true;
        owner.state.snapshot_mut().active_scene = 1;
        let verification = owner
            .begin_gateway_verification(
                &bound,
                GatewayVerificationRuntime::new(
                    GatewayVerification::Scene { scene: 1 },
                    0,
                    end - 10,
                    &gateway_verification_policy(3_000),
                ),
            )
            .unwrap();
        let (action, _, remaining) = owner
            .advance_gateway_verification(verification, 1, end)
            .unwrap();
        assert_eq!(action, GatewayVerificationAction::TimedOut);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn late_frame_completion_is_recorded_without_renewing_the_write_deadline() {
        use crate::commands::DeviceCommand;
        use serde_json::json;
        let (mut owner, _) = live_owner();
        let context = owner
            .context_for_gateway_write(
                "device.setMasterVolume",
                &json!({}),
                &crate::device::request::PlannedWrite::HidCommand(DeviceCommand::SetMasterVolume(
                    0.5,
                )),
                100,
            )
            .unwrap();
        let end = context.deadline.unwrap().expires_at_ms();
        let operation = owner
            .plan_operation_in_context(&context, end - 5, 3_000)
            .unwrap();
        let dispatch = owner
            .prepare_dispatch(Some(&operation), None, end - 5)
            .unwrap();
        let frame = owner.prepare_delivery(&dispatch, 4, 2, end - 5).unwrap();
        let next = owner.prepare_delivery(&dispatch, 4, 1, end - 5).unwrap();
        owner.begin_native_report(&frame, 0, end - 1).unwrap();
        owner
            .finish_native_report(&frame, 0, ReportOutcome::Delivered, end)
            .unwrap();
        // Preserve framing: finish an already-started envelope, then stop.
        owner.begin_native_report(&frame, 1, end + 1).unwrap();
        owner
            .finish_native_report(&frame, 1, ReportOutcome::Delivered, end + 2)
            .unwrap();
        assert_eq!(owner.delivery_record(&frame).unwrap().delivered_reports, 2);
        assert!(owner.begin_native_report(&next, 0, end + 3).is_err());
        // Both hosts check the operation before accepting native completion.
        assert!(owner.validate_operation(&operation, end + 3).is_err());
        assert!(owner
            .plan_operation_in_context(&context, end + 3, 3_000)
            .is_err());
    }

    #[test]
    fn tempo_mode_noop_is_bound_to_observed_mode_without_depending_on_bpm() {
        use crate::profile::MESSAGE_TYPE_GLOBAL_TEMPO as TEMPO;
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let method = "device.setTempoMode";
        let params = json!({"mode":"GLOBAL"});
        let context = preflight_context(&owner, method, &params);
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(1), 1.0)]),
        );
        let (bound, decision) = owner
            .bind_write_preflight(&context, method, &params, &json!({"mode":"GLOBAL"}), 0)
            .unwrap();
        assert_eq!(
            decision,
            crate::device::request::GatewayWritePreflightDecision::AlreadyMatched
        );
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(0), 0.7)]),
        );
        assert!(owner.plan_operation_in_context(&bound, 0, 100).is_ok());
        observe_preflight(
            &mut owner,
            connection,
            TEMPO,
            tempo_publication(&[(Some(1), 0.0)]),
        );
        assert!(owner
            .bind_write_preflight(&context, method, &params, &json!({"mode":"GLOBAL"}), 0)
            .unwrap_err()
            .contains("changed before binding"));
    }

    #[test]
    fn pinned_preflight_guards_only_the_planned_membership_and_retains_original_arguments() {
        use crate::{
            profile::MESSAGE_TYPE_PINNED_MODELS as PINNED, proto::cortex_protobuf_v2 as pa,
        };
        use prost::Message;
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        install_listed_models(&mut owner, &[7, 8, 42]);
        let method = "device.setModelPinned";
        let params = json!({"modelId":42,"expectedPinned":false,"pinned":true});
        let context = preflight_context(&owner, method, &params);
        let publication = |models| {
            pa::PinnedModelsMessage {
                action: pa::message_action::Enum::Update as i32,
                models,
                ..Default::default()
            }
            .encode_to_vec()
        };
        observe_preflight(&mut owner, connection, PINNED, publication(vec![7]));
        // Replacing the caller's arguments after planning cannot change its target.
        let (bound, decision) = owner
            .bind_write_preflight(
                &context,
                method,
                &json!({"modelId":7,"expectedPinned":true,"pinned":true}),
                &json!({"models":[7]}),
                0,
            )
            .unwrap();
        assert_eq!(
            decision,
            crate::device::request::GatewayWritePreflightDecision::Proceed
        );
        let operation = owner.plan_operation_in_context(&bound, 0, 100).unwrap();
        observe_preflight(&mut owner, connection, PINNED, publication(vec![7, 8]));
        assert!(owner.validate_operation(&operation, 1).is_ok());
        observe_preflight(&mut owner, connection, PINNED, publication(vec![42, 8]));
        assert!(owner
            .validate_operation(&operation, 2)
            .unwrap_err()
            .contains("target changed"));
        owner.reset_state();
        assert!(owner.state.external_preflight_observation(method).is_none());
    }

    #[test]
    fn favorite_preflight_distinguishes_identity_and_rejects_partial_list_mutations() {
        use crate::{
            profile::MESSAGE_TYPE_RECENTS_FAVORITES as FAVORITES, proto::cortex_protobuf_v2 as pa,
        };
        use prost::Message;
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let method = "device.setFavorite";
        let params = json!({"name":"Preset","folderKey":"My Presets","productKey":"target",
            "isFactory":false,"isPlugin":false,"expectedFavorite":false,"favorite":true});
        let context = preflight_context(&owner, method, &params);
        let publication = |product: &str, is_favorites, action| {
            pa::RecentsFavoritesMessage {
                action,
                is_favorites,
                request_id: Some(pa::recents_favorites_message::RequestId::RequestId(9)),
                items: vec![pa::RecentsFavoritesItem {
                    name: "Preset".into(),
                    folder_key: "My Presets".into(),
                    product_key: product.into(),
                    ..Default::default()
                }],
                ..Default::default()
            }
            .encode_to_vec()
        };
        let update = pa::message_action::Enum::Update as i32;
        let other = publication("other", true, update);
        observe_preflight(&mut owner, connection, FAVORITES, other.clone());
        let response =
            serde_json::to_value(crate::responses::decode_favorites(&other, 9).unwrap()).unwrap();
        let (bound, _) = owner
            .bind_write_preflight(&context, method, &params, &response, 0)
            .unwrap();
        let operation = owner.plan_operation_in_context(&bound, 0, 100).unwrap();
        // The same target in recents is not evidence of favorite membership.
        observe_preflight(
            &mut owner,
            connection,
            FAVORITES,
            publication("target", false, update),
        );
        assert!(owner.validate_operation(&operation, 1).is_ok());
        observe_preflight(
            &mut owner,
            connection,
            FAVORITES,
            publication("target", true, update),
        );
        assert!(owner.validate_operation(&operation, 2).is_err());
        observe_preflight(&mut owner, connection, FAVORITES, other);
        assert!(owner.validate_operation(&operation, 3).is_ok());
        observe_preflight(
            &mut owner,
            connection,
            FAVORITES,
            publication("other", true, pa::message_action::Enum::Delete as i32),
        );
        assert!(owner.validate_operation(&operation, 4).is_err());
    }

    #[test]
    fn an_accepted_favorite_or_pin_leaves_the_retained_list_unconfirmed_until_a_complete_list_arrives(
    ) {
        // The unit answers a favorite or pin with a partial acknowledgement (the changed entry
        // alone, measured on CorOS 4.1.0, 2026-10-07) and that echo may never arrive. A second
        // request whose expected state is the pre-mutation list must not be sent on the strength
        // of a list the accepted write has already made obsolete.
        use crate::{
            profile::{
                MESSAGE_TYPE_PINNED_MODELS as PINNED, MESSAGE_TYPE_RECENTS_FAVORITES as FAVORITES,
            },
            proto::cortex_protobuf_v2::{self as pa, message_action::Enum as Action},
        };
        use prost::Message;
        use serde_json::{json, Value};

        let (mut owner, connection) = live_owner();
        install_listed_models(&mut owner, &[7, 8, 42]);
        let send = |owner: &mut SessionRuntime, method: &str, params: &Value| {
            let context = preflight_context(owner, method, params);
            let response = owner
                .decoded_preflight_observation(method)
                .expect("the list is confirmed before the first request");
            let (bound, _) = owner
                .bind_write_preflight(&context, method, params, &response, 0)
                .unwrap();
            let operation = owner.plan_operation_in_context(&bound, 0, 100).unwrap();
            let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
            let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
            owner.begin_native_report(&delivery, 0, 1).unwrap();
        };
        let unconfirmed = |owner: &SessionRuntime, method: &str| {
            assert!(
                owner.decoded_preflight_observation(method).is_none(),
                "{method}: the retained list must be unconfirmed"
            );
            let message = crate::device::request::gateway_preflight_unavailable_message(method);
            assert!(
                message.contains("unconfirmed") && message.contains("nothing was sent"),
                "{message}"
            );
        };

        // Pinned models: populate, accept a pin, hear nothing, ask again with the stale expectation.
        let pinned = |action: Action, models: Vec<u32>| {
            pa::PinnedModelsMessage {
                action: action as i32,
                models,
                ..Default::default()
            }
            .encode_to_vec()
        };
        let pin = "device.setModelPinned";
        observe_preflight(
            &mut owner,
            connection,
            PINNED,
            pinned(Action::Update, vec![7, 8]),
        );
        assert_eq!(
            owner.decoded_preflight_observation(pin),
            Some(json!({"models": [7, 8], "captures": []}))
        );
        send(
            &mut owner,
            pin,
            &json!({"modelId": 42, "expectedPinned": false, "pinned": true}),
        );
        unconfirmed(&owner, pin);
        // The partial echo is not a list: it neither confirms nor replaces what was retained.
        observe_preflight(
            &mut owner,
            connection,
            PINNED,
            pinned(Action::Create, vec![42]),
        );
        unconfirmed(&owner, pin);
        // A complete publication (an explicit list read) confirms it again, with every model.
        observe_preflight(
            &mut owner,
            connection,
            PINNED,
            pinned(Action::Update, vec![7, 8, 42]),
        );
        assert_eq!(
            owner.decoded_preflight_observation(pin),
            Some(json!({"models": [7, 8, 42], "captures": []}))
        );
        // Even with no write, a partial update that arrives alone never stands in for the list.
        observe_preflight(
            &mut owner,
            connection,
            PINNED,
            pinned(Action::Delete, vec![42]),
        );
        unconfirmed(&owner, pin);

        // Favorites: the same path.
        let favorites = |action: Action, name: &str| {
            pa::RecentsFavoritesMessage {
                action: action as i32,
                is_favorites: true,
                request_id: Some(pa::recents_favorites_message::RequestId::RequestId(9)),
                items: vec![pa::RecentsFavoritesItem {
                    name: name.into(),
                    folder_key: "My Presets".into(),
                    product_key: "key".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }
            .encode_to_vec()
        };
        let favorite = "device.setFavorite";
        observe_preflight(
            &mut owner,
            connection,
            FAVORITES,
            favorites(Action::Update, "Other"),
        );
        assert!(owner.decoded_preflight_observation(favorite).is_some());
        send(
            &mut owner,
            favorite,
            &json!({"name":"Preset","folderKey":"My Presets","productKey":"key",
                "isFactory":false,"isPlugin":false,"expectedFavorite":false,"favorite":true}),
        );
        unconfirmed(&owner, favorite);
        observe_preflight(
            &mut owner,
            connection,
            FAVORITES,
            favorites(Action::Create, "Preset"),
        );
        unconfirmed(&owner, favorite);
        observe_preflight(
            &mut owner,
            connection,
            FAVORITES,
            favorites(Action::Update, "Preset"),
        );
        assert!(owner.decoded_preflight_observation(favorite).is_some());
    }

    #[test]
    fn delayed_readback_cannot_start_on_a_replacement_write_session() {
        let (mut owner, _) = live_owner();
        let original_write = owner.session_context().unwrap();
        let scope = owner
            .begin_gateway_read_scope_for(&original_write, 10, 30)
            .unwrap();
        assert_eq!(
            scope.context().session_fence(),
            original_write.session_fence()
        );
        assert_eq!(owner.gateway_read_scope_remaining(&scope, 39).unwrap(), 1);
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 20));
        assert!(owner
            .begin_gateway_read_scope_for(&original_write, 20, 30)
            .is_err());
        assert!(owner.gateway_read_scope_remaining(&scope, 21).is_err());
    }

    #[test]
    fn logical_read_scope_keeps_one_budget_for_retry_followup_and_completion() {
        let (owner, _) = live_owner();
        let scope = owner.begin_gateway_read_scope(100, 30).unwrap();
        assert_eq!(owner.gateway_read_scope_remaining(&scope, 105).unwrap(), 25);
        // JNI round-tripping and a second component cannot renew the deadline.
        let retained: GatewayReadScope =
            serde_json::from_str(&serde_json::to_string(&scope).unwrap()).unwrap();
        assert_eq!(
            owner.gateway_read_scope_remaining(&retained, 120).unwrap(),
            10
        );
        assert_eq!(owner.gateway_read_scope_remaining(&scope, 129).unwrap(), 1);
        assert!(owner
            .gateway_read_scope_remaining(&retained, 130)
            .unwrap_err()
            .contains("budget expired"));
        assert!(owner.gateway_read_scope_remaining(&scope, 140).is_err());
    }

    #[test]
    fn logical_read_scope_cannot_be_adopted_by_a_replacement_session() {
        let (mut owner, _) = live_owner();
        let original = owner.begin_gateway_read_scope(100, 30).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 110));
        assert!(owner
            .gateway_read_scope_remaining(&original, 111)
            .unwrap_err()
            .contains("session changed"));
        let replacement = owner.begin_gateway_read_scope(111, 30).unwrap();
        assert_eq!(
            owner
                .gateway_read_scope_remaining(&replacement, 112)
                .unwrap(),
            29
        );
        owner.reset_state();
        assert!(owner
            .gateway_read_scope_remaining(&replacement, 113)
            .is_err());
    }

    #[test]
    fn gateway_verification_ids_survive_teardown_without_reuse_or_overflow() {
        use crate::device::request::{gateway_verification_policy, GatewayVerification};
        let (mut owner, _) = live_owner();
        let runtime = || {
            GatewayVerificationRuntime::new(
                GatewayVerification::Scene { scene: 1 },
                0,
                0,
                &gateway_verification_policy(50),
            )
        };
        let context = owner.session_context().unwrap();
        let old = owner
            .begin_gateway_verification(&context, runtime())
            .unwrap();
        owner.reset_state();
        owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 1));
        let context = owner.session_context().unwrap();
        let current = owner
            .begin_gateway_verification(&context, runtime())
            .unwrap();
        assert!(current > old);
        owner.cancel_gateway_verification(old);
        assert!(owner.advance_gateway_verification(current, 0, 1).is_ok());
        owner.next_verification = i64::MAX as u64;
        assert!(owner
            .begin_gateway_verification(&context, runtime())
            .unwrap_err()
            .contains("identity exhausted"));
        assert_eq!(owner.next_verification, i64::MAX as u64);
    }

    #[test]
    fn gateway_verification_does_not_follow_a_replacement_session() {
        use crate::device::request::{gateway_verification_policy, GatewayVerification};
        let (mut owner, _) = live_owner();
        let context = owner.session_context().unwrap();
        let runtime = GatewayVerificationRuntime::new(
            GatewayVerification::Scene { scene: 1 },
            5,
            10,
            &gateway_verification_policy(50),
        );
        owner
            .start_gateway_verification(1, &context, runtime)
            .unwrap();
        owner.state.snapshot_mut().active_scene = 1;
        assert!(!matches!(
            owner.advance_gateway_verification(1, 5, 20).unwrap().0,
            GatewayVerificationAction::Verified
        ));
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 21));
        assert!(owner
            .advance_gateway_verification(1, 6, 22)
            .unwrap_err()
            .contains("session changed"));
        assert!(owner.gateway_verifications.is_empty());
        assert!(owner
            .start_gateway_verification(
                2,
                &context,
                GatewayVerificationRuntime::new(
                    GatewayVerification::Scene { scene: 1 },
                    5,
                    10,
                    &gateway_verification_policy(50)
                )
            )
            .unwrap_err()
            .contains("session changed"));
    }

    #[test]
    fn gateway_verification_keeps_deadline_and_discards_pre_write_operands_for_refresh() {
        use crate::device::request::{gateway_verification_policy, GatewayVerification};
        let (mut owner, _) = live_owner();
        let context = bypass_context(&mut owner);
        let runtime = || {
            GatewayVerificationRuntime::new(
                GatewayVerification::Scene { scene: 1 },
                5,
                10,
                &gateway_verification_policy(50),
            )
        };
        owner
            .start_gateway_verification(1, &context, runtime())
            .unwrap();
        let (_, refresh, remaining) = owner.advance_gateway_verification(1, 5, 20).unwrap();
        assert_eq!(refresh.session_fence(), context.session_fence());
        assert!(refresh.operands.is_none());
        assert_eq!(remaining, 40);
        assert!(owner
            .start_gateway_verification(1, &context, runtime())
            .is_err());
        owner.state.snapshot_mut().active_scene = 1;
        assert_eq!(
            owner.advance_gateway_verification(1, 6, 60).unwrap().0,
            GatewayVerificationAction::TimedOut
        );
        assert!(owner.gateway_verifications.is_empty());
        owner
            .start_gateway_verification(2, &context, runtime())
            .unwrap();
        assert_eq!(
            owner.advance_gateway_verification(2, 6, 59).unwrap().0,
            GatewayVerificationAction::Verified
        );
        assert!(owner.gateway_verifications.is_empty());
    }

    #[test]
    fn gateway_verification_capacity_is_bounded_and_reset_reclaims_it() {
        use crate::device::request::{gateway_verification_policy, GatewayVerification};
        let (mut owner, _) = live_owner();
        let context = owner.session_context().unwrap();
        let runtime = || {
            GatewayVerificationRuntime::new(
                GatewayVerification::Scene { scene: 1 },
                0,
                0,
                &gateway_verification_policy(50),
            )
        };
        for id in 1..=128 {
            owner
                .start_gateway_verification(id, &context, runtime())
                .unwrap();
        }
        assert!(owner
            .start_gateway_verification(129, &context, runtime())
            .unwrap_err()
            .contains("capacity"));
        owner.cancel_gateway_verification(1);
        owner
            .start_gateway_verification(129, &context, runtime())
            .unwrap();
        owner.reset_state();
        assert!(owner.gateway_verifications.is_empty());
        assert!(owner.advance_gateway_verification(129, 1, 1).is_err());
    }

    fn observe_folder(
        owner: &mut SessionRuntime,
        connection: u64,
        key: &str,
        name: Option<&str>,
        first_ms: u64,
        last_ms: u64,
    ) {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                name: Some(pa::folder_info::Name::Name("Test folder".into())),
                // This helper publishes all 256 explicit slot records. It is
                // used by tests of the complete-slot read API, not as a claim
                // that every firmware File message contains a whole folder.
                files: (0..256)
                    .map(|index| pa::ProductData {
                        key: (index == 3)
                            .then_some(name)
                            .flatten()
                            .map(|name| pa::product_data::Key::Key(format!("{key}/{name}.pb"))),
                        name: (index == 3)
                            .then_some(name)
                            .flatten()
                            .map(|name| pa::product_data::Name::Name(name.into())),
                        index: Some(pa::product_data::Index::Index(index)),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: first_ms,
                    last_report_at_ms: last_ms,
                }),
            )
            .unwrap();
        assert!(owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap()
            .is_some());
    }

    #[test]
    fn retained_partial_catalog_updates_preserve_inventory_and_ignore_cache_edits() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let (mut owner, connection) = live_owner();
        let key = "/media/p4/Presets/Test";
        let fence = owner.session_context().unwrap().session_fence().clone();
        observe_folder(&mut owner, connection, key, Some("Old"), 1, 2);
        let message = pa::FileMessage {
            action: pa::message_action::Enum::Update as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(3)),
                    key: Some(pa::product_data::Key::Key("new-key".into())),
                    name: Some(pa::product_data::Name::Name("New".into())),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        };
        let ticket = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 11,
                    last_report_at_ms: 12,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, 4, &message.encode_to_vec())
            .unwrap();
        let retained = owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 13)
            .unwrap()
            .unwrap();
        assert_eq!(retained["presets"][3]["name"], "New");
        assert_eq!(retained["presets"][4]["position"], 4);
        assert!(retained["presets"][4]["productKey"].is_null());
        let mut local = owner
            .state
            .native_preset_catalog()
            .folder(key)
            .unwrap()
            .clone();
        local.name = "Invented folder".into();
        local.files[3].name = "Invented product".into();
        owner.state.ingest_preset_listing(local);
        let retained = owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 13)
            .unwrap()
            .unwrap();
        assert_eq!(retained["presets"][3]["name"], "New");
        let folders = owner
            .retained_preset_catalog(&fence, "device.listPresetFolders", key, 13)
            .unwrap()
            .unwrap();
        assert_eq!(folders["folders"][0]["name"], "Test folder");
        // An explicit native folder deletion retires every old slot proof.
        let deletion = pa::FileMessage {
            action: pa::message_action::Enum::Delete as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                ..Default::default()
            })),
            ..Default::default()
        };
        let ticket = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 14,
                    last_report_at_ms: 14,
                }),
            )
            .unwrap();
        owner
            .observe_state(&ticket, 4, &deletion.encode_to_vec())
            .unwrap();
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 15)
            .unwrap()
            .is_none());
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetFolders", key, 15)
            .unwrap()
            .is_none());
    }

    #[test]
    fn retained_paired_catalog_updates_preserve_inherited_instrument_on_both_sides() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let (mut owner, connection) = live_owner();
        let source = "/media/p4/Presets/Source";
        let destination = "/media/p4/Presets/Destination";
        let fence = owner.session_context().unwrap().session_fence().clone();
        let folder = |key: &str, index: i32, instrument: Option<i32>| pa::FolderInfo {
            key: Some(pa::folder_info::Key::Key(key.into())),
            files: vec![pa::ProductData {
                index: Some(pa::product_data::Index::Index(index)),
                key: Some(pa::product_data::Key::Key(format!("{key}/saved.pb"))),
                name: Some(pa::product_data::Name::Name("Saved".into())),
                instrument: instrument.map(pa::product_data::Instrument::Instrument),
                ..Default::default()
            }],
            ..Default::default()
        };
        for (time, instrument) in [(1, Some(2)), (11, None)] {
            let message = pa::FileMessage {
                action: pa::message_action::Enum::Update as i32,
                folder: Some(pa::file_message::Folder::Folder(folder(
                    source, 3, instrument,
                ))),
                to_folder: Some(pa::file_message::ToFolder::ToFolder(folder(
                    destination,
                    7,
                    instrument,
                ))),
                ..Default::default()
            };
            let ticket = owner
                .capture_observation_with_ingress(
                    connection,
                    Some(IngressTime {
                        first_report_at_ms: time,
                        last_report_at_ms: time,
                    }),
                )
                .unwrap();
            owner
                .observe_state(&ticket, 4, &message.encode_to_vec())
                .unwrap();
        }
        for (key, index) in [(source, 3), (destination, 7)] {
            let value = owner
                .retained_preset_catalog(&fence, "device.listPresets", key, 12)
                .unwrap()
                .unwrap();
            let observed = &value["presets"][index];
            assert_eq!(observed["name"], "Saved");
            assert_eq!(observed["instrument"], 2);
            assert_eq!(
                owner
                    .state
                    .native_preset_catalog()
                    .folder(key)
                    .unwrap()
                    .files[0]
                    .instrument,
                2
            );
        }
        let folders = owner
            .retained_preset_catalog(&fence, "device.listPresetFolders", source, 12)
            .unwrap()
            .unwrap();
        assert_eq!(folders["folders"].as_array().unwrap().len(), 2);
    }

    /// The reason a listPresets read of `key` reports, at `now_ms`.
    fn absence_of(owner: &SessionRuntime, key: &str, now_ms: u64) -> PresetCatalogAbsence {
        let fence = owner.session_fence().unwrap();
        owner
            .preset_catalog_absence(&fence, "device.listPresets", key, now_ms)
            .unwrap()
    }

    #[test]
    fn catalog_absence_reports_no_catalog_frame_before_any_file_frame() {
        let (owner, _) = live_owner();
        let absence = absence_of(&owner, "/media/p4/Presets/Test", 10);
        assert_eq!(absence.reason, PresetCatalogAbsenceReason::NoCatalogFrame);
        assert_eq!(absence.file_frames_observed, 0);
        assert_eq!(absence.file_frames_discarded_by_rebuild, 0);
        assert_eq!(absence.session_revision, Some(0));
        assert_eq!(absence.origin_revision, None);
        let json = serde_json::to_value(&absence).unwrap();
        assert_eq!(json["reason"], "noCatalogFrame");
        assert!(json["invalidation"].is_null());
    }

    #[test]
    fn catalog_absence_reports_invalidation_and_its_cause() {
        let key = "/media/p4/Presets/Test";
        // Inverted ingress: the first report arrived after the last one.
        let (mut owner, connection) = live_owner();
        observe_folder(&mut owner, connection, key, Some("Saved"), 9, 4);
        let absence = absence_of(&owner, key, 100);
        assert_eq!(absence.reason, PresetCatalogAbsenceReason::Invalidated);
        assert_eq!(
            absence.invalidation,
            Some(CatalogInvalidation::IngressTimingInverted)
        );
        assert_eq!(absence.file_frames_observed, 1);
        assert_eq!(absence.catalog_changes_observed, 1);
        let json = serde_json::to_value(&absence).unwrap();
        assert_eq!(json["reason"], "invalidated");
        assert_eq!(json["invalidation"], "ingressTimingInverted");
        // A later well-timed frame cannot repair it; the first cause is kept.
        observe_folder(&mut owner, connection, key, Some("Later"), 20, 21);
        let absence = absence_of(&owner, key, 100);
        assert_eq!(absence.reason, PresetCatalogAbsenceReason::Invalidated);
        assert_eq!(
            absence.invalidation,
            Some(CatalogInvalidation::IngressTimingInverted)
        );
        assert_eq!(absence.file_frames_observed, 2);
    }

    #[test]
    fn catalog_absence_reports_an_epoch_change_that_discarded_the_burst() {
        use crate::device::session::DispatchOutcome;
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let key = "/media/p4/Presets/Test";
        let (mut owner, connection) = live_owner();
        // Walk the lifecycle to Building, where a Connection(false) is an
        // in-session rebuild rather than the predecessor's stale session.
        let submit = |owner: &mut SessionRuntime, now_ms: u64| {
            let lifecycle = owner.lifecycle.as_mut().unwrap();
            let batch = lifecycle.next_batch(now_ms, 1_000_000).unwrap();
            assert!(lifecycle.dispatched(&batch.ticket, DispatchOutcome::Accepted));
        };
        let observe = |owner: &mut SessionRuntime, kind: u16, payload: &[u8], now_ms: u64| {
            let ticket = owner.capture_observation(connection).unwrap();
            assert!(owner.observe_lifecycle(&ticket, kind, payload, now_ms));
        };
        submit(&mut owner, 0);
        let reset = pa::ResetCommsBuffersMessage {
            session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                "session".into(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        observe(&mut owner, 52, &reset, 10);
        submit(&mut owner, 10);
        let version = pa::VersionMessage {
            action: pa::message_action::Enum::Update as i32,
            device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                pa::version_message::DeviceType::Qc as i32,
            )),
            cortex_control_version_valid: Some(
                pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        observe(&mut owner, 10, &version, 20);
        submit(&mut owner, 20);
        submit(&mut owner, 20);
        assert_eq!(
            owner.lifecycle.as_ref().unwrap().phase(),
            crate::device::initialization::DeviceStartupPhase::Building
        );
        observe_folder(&mut owner, connection, key, Some("Saved"), 21, 22);
        assert_eq!(
            absence_of(&owner, key, 30).reason,
            PresetCatalogAbsenceReason::Available
        );
        // The device announces Connection(false): the epoch revision moves and
        // the retained catalog is wiped, which only a new burst could refill.
        observe(&mut owner, 49, &[16, 0], 40);
        let absence = absence_of(&owner, key, 50);
        assert_eq!(absence.reason, PresetCatalogAbsenceReason::EpochChanged);
        assert_eq!(absence.session_revision, Some(1));
        assert_eq!(absence.file_frames_observed, 0);
        assert_eq!(absence.file_frames_discarded_by_rebuild, 1);
        assert_eq!(absence.retained_folders, 0);
        assert_eq!(
            serde_json::to_value(&absence).unwrap()["reason"],
            "epochChanged"
        );
        // A new connection starts the tally again.
        owner.opened(60).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "next".into(), 60));
        assert_eq!(
            absence_of(&owner, key, 70).reason,
            PresetCatalogAbsenceReason::NoCatalogFrame
        );
    }

    #[test]
    fn catalog_absence_reports_a_setlist_key_missing_from_the_catalog() {
        let (mut owner, connection) = live_owner();
        observe_folder(
            &mut owner,
            connection,
            "/media/p4/Presets/Other",
            Some("Saved"),
            1,
            2,
        );
        let absence = absence_of(&owner, "/media/p4/Presets/My Presets", 10);
        assert_eq!(
            absence.reason,
            PresetCatalogAbsenceReason::SetlistKeyMissing
        );
        assert_eq!(absence.retained_folders, 1);
        assert_eq!(absence.invalidation, None);
        assert_eq!(
            serde_json::to_value(&absence).unwrap()["reason"],
            "setlistKeyMissing"
        );
        assert_eq!(
            absence_of(&owner, "/media/p4/Presets/Other", 10).reason,
            PresetCatalogAbsenceReason::Available
        );
    }

    /// A live owner whose staged startup reached `Connected`, so its Booting
    /// burst (the device's one directory publication) is over.
    fn connected_owner() -> (SessionRuntime, u64) {
        let (mut owner, connection) = live_owner();
        super::library_catalog::tests::boot(&mut owner, "session", 0, true);
        assert!(owner
            .lifecycle
            .as_ref()
            .is_some_and(SessionDriver::is_connected));
        (owner, connection)
    }

    #[test]
    fn an_unknown_setlist_key_in_a_complete_directory_is_settled() {
        let (mut owner, connection) = connected_owner();
        observe_folder(
            &mut owner,
            connection,
            "/media/p4/Presets/Other",
            Some("Saved"),
            1,
            2,
        );
        let fence = owner.session_fence().unwrap();
        let mangled = "C:/Program Files/Git/media/p4/Presets/My Presets";
        let settled = owner
            .settled_preset_catalog_absence(&fence, "device.listPresets", mangled, 10)
            .unwrap()
            .expect("an unknown key in a complete directory cannot appear later");
        assert_eq!(
            settled.reason,
            PresetCatalogAbsenceReason::SetlistKeyMissing
        );
        assert!(settled.startup_complete);
        assert_eq!(settled.known_setlists, Some(1));
        assert_eq!(settled.setlist_key_listed, Some(false));
        assert!(settled.setlist_key_unknown());
        let json = serde_json::to_value(&settled).unwrap();
        assert_eq!(json["reason"], "setlistKeyMissing");
        assert_eq!(json["knownSetlists"], 1);
        assert_eq!(json["setlistKeyListed"], false);
        assert_eq!(json["startupComplete"], true);
        // Counts only: no folder name or key reaches the diagnostics.
        assert!(!json.to_string().contains("Other"), "{json}");
        // A known key is served, and nothing about it is settled.
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresets", "/media/p4/Presets/Other", 10)
            .unwrap()
            .is_some());
        assert!(owner
            .settled_preset_catalog_absence(
                &fence,
                "device.listPresets",
                "/media/p4/Presets/Other",
                10
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_missing_setlist_key_is_not_settled_while_the_directory_can_still_grow() {
        let missing = "/media/p4/Presets/My Presets";
        // Mid-Booting: the directory burst is still arriving.
        let (mut owner, connection) = live_owner();
        observe_folder(
            &mut owner,
            connection,
            "/media/p4/Presets/Other",
            Some("Saved"),
            1,
            2,
        );
        let fence = owner.session_fence().unwrap();
        let absence = absence_of(&owner, missing, 10);
        assert_eq!(
            absence.reason,
            PresetCatalogAbsenceReason::SetlistKeyMissing
        );
        assert!(!absence.startup_complete);
        assert_eq!(absence.setlist_key_listed, Some(false));
        assert!(!absence.setlist_key_unknown());
        assert!(owner
            .settled_preset_catalog_absence(&fence, "device.listPresets", missing, 10)
            .unwrap()
            .is_none());

        // Connected, but the last frame has not finished arriving.
        let (mut owner, connection) = connected_owner();
        observe_folder(
            &mut owner,
            connection,
            "/media/p4/Presets/Other",
            Some("Saved"),
            1,
            50,
        );
        let fence = owner.session_fence().unwrap();
        let absence = absence_of(&owner, missing, 10);
        assert_eq!(absence.reason, PresetCatalogAbsenceReason::IngressPending);
        assert_eq!(absence.known_setlists, None);
        assert!(owner
            .settled_preset_catalog_absence(&fence, "device.listPresets", missing, 10)
            .unwrap()
            .is_none());

        // Connected with no retained folder at all: nothing proves the key wrong.
        let (owner, _) = connected_owner();
        let fence = owner.session_fence().unwrap();
        assert!(owner
            .settled_preset_catalog_absence(&fence, "device.listPresets", missing, 10)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_listed_setlist_with_partial_slot_coverage_is_not_settled() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let (mut owner, connection) = connected_owner();
        let key = "/media/p4/Presets/Test";
        owner.state.snapshot_mut().setlist_key = key.into();
        owner.state.snapshot_mut().has_preset = true;
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(0)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 1,
                    last_report_at_ms: 2,
                }),
            )
            .unwrap();
        owner.observe_state(&ticket, 4, &payload).unwrap();
        let fence = owner.session_fence().unwrap();
        let absence = owner
            .preset_catalog_absence(&fence, "device.listPresetSlots", key, 10)
            .unwrap();
        assert_eq!(
            absence.reason,
            PresetCatalogAbsenceReason::SetlistKeyMissing
        );
        assert_eq!(absence.setlist_key_listed, Some(true));
        assert!(!absence.setlist_key_unknown());
        assert!(owner
            .settled_preset_catalog_absence(&fence, "device.listPresetSlots", key, 10)
            .unwrap()
            .is_none());
    }

    #[test]
    fn catalog_absence_reports_ingress_still_pending_and_agrees_with_the_read() {
        let key = "/media/p4/Presets/Test";
        let (mut owner, connection) = live_owner();
        observe_folder(&mut owner, connection, key, Some("Saved"), 5, 50);
        let fence = owner.session_fence().unwrap();
        for now_ms in [10, 49, 50, 51] {
            let absence = absence_of(&owner, key, now_ms);
            let read = owner
                .retained_preset_catalog(&fence, "device.listPresets", key, now_ms)
                .unwrap();
            assert_eq!(
                read.is_some(),
                absence.reason == PresetCatalogAbsenceReason::Available,
                "the read and its explanation must agree at {now_ms}"
            );
            if now_ms < 50 {
                assert_eq!(absence.reason, PresetCatalogAbsenceReason::IngressPending);
            }
        }
    }

    #[test]
    fn retained_public_catalog_uses_original_session_and_explicit_slot_coverage() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let (mut owner, connection) = live_owner();
        let fence = owner.session_context().unwrap().session_fence().clone();
        let key = "/media/p4/Presets/Test";
        owner.state.snapshot_mut().setlist_key = key.into();
        owner.state.snapshot_mut().has_preset = true;
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetSlots", key, 0)
            .unwrap()
            .is_none());
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(0)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 1,
                    last_report_at_ms: 2,
                }),
            )
            .unwrap();
        owner.observe_state(&ticket, 4, &payload).unwrap();
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetFolders", key, 1)
            .unwrap()
            .is_none());
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetFolders", key, 2)
            .unwrap()
            .is_some());
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetSlots", key, 3)
            .unwrap()
            .is_none());
        observe_folder(&mut owner, connection, key, Some("Native"), 4, 5);
        owner
            .state
            .preset_library_mut()
            .edit_cached_preset(key, 3, "Presentation", 0);
        // Reading later reuses earlier records without declaring them fresh replies.
        let value = owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 100)
            .unwrap()
            .unwrap();
        assert_eq!(value["presets"][3]["name"], "Native");
        let slots = owner
            .retained_preset_catalog(&fence, "device.listPresetSlots", key, 100)
            .unwrap()
            .unwrap();
        assert_eq!(slots["slots"].as_array().unwrap().len(), 256);
        assert_eq!(slots["slots"][0]["occupied"], false);
        assert_eq!(slots["slots"][3]["occupied"], true);
        // Regression: this comparison used a case-sensitive `==`. A caller
        // whose requested key differs from the active setlist only by case
        // (CorOS setlist keys are not guaranteed to round-trip with stable
        // case) must not be refused as a changed active setlist.
        assert!(owner
            .retained_preset_catalog(
                &fence,
                "device.listPresetSlots",
                "/MEDIA/p4/Presets/TEST",
                100
            )
            .unwrap()
            .is_some());
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Other".into();
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetSlots", key, 100)
            .unwrap_err()
            .contains("active setlist changed"));
        owner.opened(101).unwrap();
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 102)
            .unwrap_err()
            .contains("session changed"));
    }

    #[test]
    fn retained_catalog_keeps_distinct_keys_and_waits_for_complete_ingress() {
        let (mut owner, connection) = live_owner();
        let fence = owner.session_context().unwrap().session_fence().clone();
        let requested = "/media/p4/Presets/Requested";
        let unrelated = "/media/p4/Presets/Unrelated";
        observe_folder(&mut owner, connection, requested, Some("Old"), 10, 11);
        observe_folder(&mut owner, connection, unrelated, Some("Other"), 21, 22);
        let value = owner
            .retained_preset_catalog(&fence, "device.listPresets", requested, 30)
            .unwrap()
            .unwrap();
        assert_eq!(value["presets"][3]["name"], "Old");
        observe_folder(&mut owner, connection, requested, Some("Delayed"), 19, 31);
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresets", requested, 30)
            .unwrap()
            .is_none());
        let value = owner
            .retained_preset_catalog(&fence, "device.listPresets", &format!("{requested}/"), 32)
            .unwrap()
            .unwrap();
        assert_eq!(value["presets"][3]["name"], "Delayed");
        let folders = owner
            .retained_preset_catalog(&fence, "device.listPresetFolders", requested, 32)
            .unwrap()
            .unwrap();
        assert_eq!(folders["folders"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn retained_catalog_projection_ignores_subsequent_presentation_edits() {
        let (mut owner, connection) = live_owner();
        let key = "/media/p4/Presets/Test";
        let fence = owner.session_context().unwrap().session_fence().clone();
        owner.state.snapshot_mut().setlist_key = key.into();
        owner.state.snapshot_mut().has_preset = true;
        observe_folder(&mut owner, connection, key, Some("Saved"), 1, 2);
        observe_folder(&mut owner, connection, key, Some("Device row"), 11, 12);
        owner
            .state
            .preset_library_mut()
            .edit_cached_preset(key, 3, "Saved", 0);
        assert_eq!(
            owner.state.preset_list(key).unwrap().presets[3].name,
            "Saved"
        );
        let observed = owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 13)
            .unwrap()
            .unwrap();
        assert_eq!(observed["presets"][3]["name"], "Device row");
        assert_eq!(
            observed["presets"][3]["productKey"].as_str(),
            Some(format!("{key}/Device row.pb").as_str())
        );
        // Only an explicit empty slot, not an empty folder header, proves absence.
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Update as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(3)),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 14,
                    last_report_at_ms: 15,
                }),
            )
            .unwrap();
        owner.observe_state(&ticket, 4, &payload).unwrap();
        let observed = owner
            .retained_preset_catalog(&fence, "device.listPresets", key, 16)
            .unwrap()
            .unwrap();
        assert!(observed["presets"][3]["productKey"].is_null());
        let slots = owner
            .retained_preset_catalog(&fence, "device.listPresetSlots", key, 16)
            .unwrap()
            .unwrap();
        assert_eq!(slots["slots"][3]["occupied"], false);
        assert!(!owner.state.preset_slots().unwrap().unwrap().slots[3].occupied);
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Other".into();
        assert!(owner
            .retained_preset_catalog(&fence, "device.listPresetSlots", key, 16)
            .unwrap_err()
            .contains("active setlist changed"));
    }

    #[test]
    fn setlist_deletion_rejects_an_active_target_and_tracks_navigation() {
        let (mut owner, _) = live_owner();
        bypass_context(&mut owner);
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Test".into();
        let write = crate::device::request::PlannedWrite::HidOperation(
            crate::commands::DeviceOperation::DeleteSetlist {
                name: "test".into(),
            },
        );
        assert!(owner
            .context_for_write(
                "device.deleteSetlist",
                &serde_json::json!({"name":"test"}),
                &write
            )
            .unwrap_err()
            .contains("active setlist"));
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Other".into();
        let context = owner
            .context_for_write(
                "device.deleteSetlist",
                &serde_json::json!({"name":"test"}),
                &write,
            )
            .unwrap();
        let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
        owner.state.snapshot_mut().setlist_key = "/media/p4/Presets/Test".into();
        assert!(owner
            .prepare_dispatch(Some(&operation), None, 1)
            .unwrap_err()
            .contains("target changed"));
    }

    fn preset_payload() -> Vec<u8> {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(
                crate::proto::BinaryPreset::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn bypass_context(owner: &mut SessionRuntime) -> DispatchContext {
        use serde_json::json;
        let snapshot = owner.state.snapshot_mut();
        snapshot.has_preset = true;
        snapshot.preset_name = "Private preset label".into();
        snapshot.setlist_key = "My Presets".into();
        snapshot.preset_position = 0;
        snapshot.active_scene = 0;
        snapshot.blocks = vec![serde_json::from_value(json!({
            "id": "0:0", "name": "Amp", "kind": "amp", "row": 0,
            "column": 0, "modelId": 7, "bypassed": false
        }))
        .unwrap()];
        owner
            .context_for_write(
                "device.setBypass",
                &json!({"row": 0, "column": 0, "expectedBypassed": false,
                "expectedPresetName": "Private preset label", "expectedScene": 0}),
                &crate::device::request::PlannedWrite::HidCommand(
                    crate::commands::DeviceCommand::SetBypass {
                        row: 0,
                        column: 0,
                        bypassed: true,
                    },
                ),
            )
            .unwrap()
    }

    #[test]
    fn queued_write_rejects_a_replaced_target_in_the_same_session() {
        let (mut owner, _) = live_owner();
        let context = bypass_context(&mut owner);
        // Native hosts retain this opaque context through their planning boundary.
        let context = serde_json::from_str(&serde_json::to_string(&context).unwrap()).unwrap();
        let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        owner.state.snapshot_mut().blocks[0].model_id = Some(8);
        assert!(owner
            .begin_native_report(&delivery, 0, 1)
            .unwrap_err()
            .contains("target changed"));
        let record = owner.delivery_record(&delivery).unwrap();
        assert_eq!(
            record.status,
            crate::device::delivery::DeliveryStatus::Cancelled
        );
        assert_eq!(record.started_at_ms, None);
        assert_eq!(record.delivered_reports, 0);
    }

    #[test]
    fn write_context_ignores_unrelated_telemetry_but_detects_same_named_preset_recall() {
        let (mut owner, _) = live_owner();
        let context = bypass_context(&mut owner);
        owner.state.snapshot_mut().master_volume = 37;
        owner.state.snapshot_mut().tempo = 127;
        assert!(owner.plan_operation_in_context(&context, 1, 100).is_ok());
        owner.state.snapshot_mut().preset_position = 1;
        assert!(owner
            .plan_operation_in_context(&context, 2, 100)
            .unwrap_err()
            .contains("target changed"));
    }

    #[test]
    fn started_operation_does_not_reject_its_own_echo_or_expose_private_guards() {
        let (mut owner, _) = live_owner();
        let context = bypass_context(&mut owner);
        let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let first = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        owner.begin_native_report(&first, 0, 1).unwrap();
        owner
            .finish_native_report(&first, 0, ReportOutcome::Delivered, 2)
            .unwrap();
        owner.state.snapshot_mut().blocks[0].bypassed = Some(true);
        let next = owner.prepare_delivery(&dispatch, 4, 1, 3).unwrap();
        owner.begin_native_report(&next, 0, 4).unwrap();
        owner
            .finish_native_report(&next, 0, ReportOutcome::Delivered, 5)
            .unwrap();
        assert!(owner.deliveries.operation_complete(&operation, 2).unwrap());
        assert!(owner.plan_operation_in_context(&context, 6, 100).is_err());
        let record = serde_json::to_value(owner.require_delivery_complete(&next).unwrap()).unwrap();
        assert_eq!(
            record["operation"],
            serde_json::to_value(operation.identity()).unwrap()
        );
        let serialized = record.to_string();
        assert!(!serialized.contains("Private preset label"));
        assert!(!serialized.contains("expectedBypassed"));
        assert!(!serialized.contains("operands"));
    }

    #[test]
    fn receipt_pressure_retains_live_operation_start_evidence() {
        let (mut owner, _) = live_owner();
        let context = bypass_context(&mut owner);
        let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let first = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        owner.begin_native_report(&first, 0, 1).unwrap();
        owner
            .finish_native_report(&first, 0, ReportOutcome::Delivered, 2)
            .unwrap();
        let background = owner.prepare_dispatch(None, None, 3).unwrap();
        for _ in 0..512 {
            let delivery = owner.prepare_delivery(&background, 26, 1, 3).unwrap();
            owner.begin_native_report(&delivery, 0, 3).unwrap();
            owner
                .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 4)
                .unwrap();
        }
        assert!(owner.require_delivery_complete(&first).is_ok());
        owner.state.snapshot_mut().blocks[0].bypassed = Some(true);
        let next = owner.prepare_delivery(&dispatch, 4, 1, 5).unwrap();
        assert!(owner.begin_native_report(&next, 0, 6).is_ok());
        // Once its budget expires the record may be evicted, but no later frame
        // can acquire a fresh budget from this ticket.
        let later = owner.prepare_delivery(&background, 26, 1, 100).unwrap();
        assert!(owner.delivery_record(&later).is_ok());
        assert!(owner.delivery_record(&first).is_err());
        assert!(owner.prepare_delivery(&dispatch, 4, 1, 100).is_err());
    }

    #[test]
    fn assignment_transition_retains_the_previous_footswitch_operand() {
        use crate::commands::{DeviceOperation, StompAssignmentType};
        let (mut owner, _) = live_owner();
        bypass_context(&mut owner);
        owner.state.snapshot_mut().blocks[0].footswitch = Some(1);
        let context = owner
            .context_for_write(
                "device.setFootswitch",
                &serde_json::json!({"row": 0, "column": 0}),
                &crate::device::request::PlannedWrite::HidOperation(
                    DeviceOperation::SetFootswitch {
                        row: 0,
                        column: 0,
                        footswitch: Some(2),
                        assignment_type: StompAssignmentType::Primary,
                        previous_footswitch: Some(1),
                        previous_assignment_type: None,
                    },
                ),
            )
            .unwrap();
        let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        owner.state.snapshot_mut().blocks[0].footswitch = Some(3);
        assert!(owner
            .begin_native_report(&delivery, 0, 1)
            .unwrap_err()
            .contains("target changed"));
        assert_eq!(
            owner.delivery_record(&delivery).unwrap().started_at_ms,
            None
        );
    }

    fn inbound_reports(payload: &[u8]) -> Vec<Vec<u8>> {
        crate::framing::encode(13, payload)
            .into_iter()
            .map(|mut report| {
                report[0] = crate::framing::IN_REPORT_ID;
                report.to_vec()
            })
            .collect()
    }

    /// One inbound report carrying an arbitrary envelope, including the
    /// trailer fields the host fixes for its own writes.
    fn inbound_envelope(message_type: u32, payload: &[u8], device_bytes: [u8; 2]) -> Vec<u8> {
        let mut report = crate::framing::encode_frame(&crate::framing::Frame {
            message_type,
            payload: payload.to_vec(),
            encrypted: false,
            compressed: false,
            device_bytes,
        })[0]
            .to_vec();
        report[0] = crate::framing::IN_REPORT_ID;
        report
    }

    #[test]
    fn the_inbound_tally_reports_both_halves_of_the_production_path() {
        use crate::inbound_telemetry::DeviceTrailerTally;

        let (mut owner, connection) = live_owner();
        assert_eq!(owner.inbound_telemetry_revision(), 0);
        let epoch = owner.capture_report_epoch(connection).unwrap();

        // A declared vendor-cloud type the local-control state dispatch table
        // intentionally has no handler for.
        let (frame, ticket) = owner
            .push_native_report(
                connection,
                epoch,
                &inbound_envelope(
                    u32::from(crate::profile::MESSAGE_TYPE_CLOUD_PRODUCT),
                    &[],
                    [0x11, 0x22],
                ),
                1,
            )
            .unwrap()
            .unwrap();
        assert_eq!(frame.device_bytes, [0x11, 0x22]);
        owner
            .observe_state(&ticket, frame.message_type, &frame.payload)
            .unwrap();

        // A type number the protocol does not declare at all.
        let (frame, _) = owner
            .push_native_report(
                connection,
                epoch,
                &inbound_envelope(73, &[8, 1], [0x33, 0x44]),
                2,
            )
            .unwrap()
            .unwrap();
        assert_eq!(frame.message_type, 73);

        // One reader sees the transport's half and the decoder's half merged.
        let snapshot = owner.inbound_telemetry();
        assert_eq!(snapshot.frames, 2);
        assert_eq!(snapshot.unacted_frames, 1);
        assert_eq!(snapshot.no_state_handler_frames, 1);
        assert_eq!(snapshot.undeclared_types[&73], 1);
        assert_eq!(
            snapshot.no_state_handler_types[&crate::profile::MESSAGE_TYPE_CLOUD_PRODUCT],
            1
        );
        assert!(snapshot.decode_failures.is_empty());
        assert_eq!(
            snapshot.device_trailer_bytes,
            vec![
                DeviceTrailerTally {
                    message_type: crate::profile::MESSAGE_TYPE_CLOUD_PRODUCT,
                    device_bytes: [0x11, 0x22],
                    count: 1,
                },
                DeviceTrailerTally {
                    message_type: 73,
                    device_bytes: [0x33, 0x44],
                    count: 1,
                },
            ]
        );
        assert!(owner.inbound_telemetry_revision() > 0);

        // And it ends with the connection, like every other receive total.
        owner.disconnect(10, false);
        assert_eq!(
            owner.inbound_telemetry(),
            crate::inbound_telemetry::InboundTelemetrySnapshot::default()
        );
    }

    #[test]
    fn native_reports_cannot_acquire_a_new_epoch_while_buffered() {
        let (mut owner, connection) = live_owner();
        let old = owner.capture_report_epoch(connection).unwrap();
        let reports = inbound_reports(&[8, 1, 16, 42]);
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 10));
        // Reject even before another report has refreshed the compact identity.
        assert!(owner
            .push_native_report(connection, old, &reports[0], 1)
            .unwrap()
            .is_none());
        let current = owner.capture_report_epoch(connection).unwrap();
        assert_ne!(current, old);
        assert!(owner
            .push_native_report(connection, old, &reports[0], 1)
            .unwrap()
            .is_none());
        let (frame, observation) = owner
            .push_native_report(connection, current, &reports[0], 11)
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload, [8, 1, 16, 42]);
        assert!(owner.accepts_observation(&observation));
        assert_eq!(observation.ingress().unwrap().first_report_at_ms, 11);
    }

    /// The native read path a Windows or Android host drives for every
    /// handshake, seed and connected-state read: an over-limit compressed
    /// frame fails `push_native_report` (named by type and limit, counted per
    /// type), and the host then applies the shared read-error policy to it
    /// exactly as to any other rejected frame. One is tolerated; a following
    /// good frame resets the streak through `read_succeeded`.
    #[test]
    fn an_over_limit_compressed_frame_goes_through_the_shared_read_error_policy() {
        let (mut owner, connection) = live_owner();
        let epoch = owner.capture_report_epoch(connection).unwrap();
        let scene = crate::profile::MESSAGE_TYPE_SCENE;
        let limit = crate::compression::max_inflated_bytes_for(scene);
        let compressed_envelope = |inflated: usize| {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            std::io::Write::write_all(&mut encoder, &vec![0; inflated]).unwrap();
            let reports = crate::framing::encode_frame(&crate::framing::Frame {
                message_type: u32::from(scene),
                payload: encoder.finish().unwrap(),
                encrypted: false,
                compressed: true,
                device_bytes: [0, 0],
            });
            assert_eq!(reports.len(), 1);
            let mut report = reports[0].to_vec();
            report[0] = crate::framing::IN_REPORT_ID;
            report
        };

        let error = owner
            .push_native_report(connection, epoch, &compressed_envelope(limit + 1), 1)
            .unwrap_err();
        assert!(
            error.contains(&format!("type {scene} (limit {limit} bytes)")),
            "{error}"
        );
        assert_eq!(
            owner
                .transport
                .inbound_telemetry()
                .snapshot()
                .inflate_failures[&scene],
            1
        );
        // The host's policy call for that rejected read: tolerated once.
        assert!(!owner.transport.read_failed());
        // At the limit the frame is delivered, and the host's success call
        // clears the streak, so the next rejection is again tolerated.
        let (frame, _) = owner
            .push_native_report(connection, epoch, &compressed_envelope(limit), 2)
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload.len(), limit);
        owner.transport.read_succeeded();
        assert!(owner
            .push_native_report(connection, epoch, &compressed_envelope(limit + 1), 3)
            .is_err());
        assert!(!owner.transport.read_failed());
        // Two rejections in a row are not tolerated: the session ends loudly.
        assert!(owner
            .push_native_report(connection, epoch, &compressed_envelope(limit + 1), 4)
            .is_err());
        assert!(owner.transport.read_failed());
    }

    #[test]
    fn a_partial_frame_cannot_span_native_ingress_epochs() {
        let (mut owner, connection) = live_owner();
        let reports = inbound_reports(&vec![0x5a; crate::framing::CHUNK_SIZE * 2]);
        assert_eq!(reports.len(), 3);
        let old = owner.capture_report_epoch(connection).unwrap();
        assert!(owner
            .push_native_report(connection, old, &reports[0], 1)
            .unwrap()
            .is_none());
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 10));
        let current = owner.capture_report_epoch(connection).unwrap();
        // Reports completing an old frame after the transition carry a new
        // ingress identity, but do not create a frame in the new byte stream.
        assert!(owner
            .push_native_report(connection, current, &reports[1], 11)
            .unwrap()
            .is_none());
        assert!(owner
            .push_native_report(connection, current, &reports[2], 12)
            .unwrap()
            .is_none());
        assert!(owner
            .push_native_report(connection, current, &reports[0], 13)
            .unwrap()
            .is_none());
        // A delayed old-epoch report must not corrupt a new partial frame.
        assert!(owner
            .push_native_report(connection, old, &reports[1], 2)
            .unwrap()
            .is_none());
        assert!(owner
            .push_native_report(connection, current, &reports[1][1..], 14)
            .unwrap()
            .is_none());
        let (frame, observation) = owner
            .push_native_report(connection, current, &reports[2][1..], 15)
            .unwrap()
            .unwrap();
        assert_eq!(frame.payload, vec![0x5a; crate::framing::CHUNK_SIZE * 2]);
        assert_eq!(frame.report_count, 3);
        assert_eq!(
            observation.ingress(),
            Some(IngressTime {
                first_report_at_ms: 13,
                last_report_at_ms: 15
            })
        );
    }

    #[test]
    fn compact_ingress_identity_is_stable_but_never_reused_after_reopen() {
        let (mut owner, first) = live_owner();
        let old = owner.capture_report_epoch(first).unwrap();
        assert!(old > 0);
        assert_eq!(old, owner.capture_report_epoch(first).unwrap());
        let second = owner.opened(1).unwrap();
        assert_eq!(owner.capture_report_epoch(second).unwrap(), 0);
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 1));
        assert_eq!(owner.capture_report_epoch(first).unwrap(), 0);
        let current = owner.capture_report_epoch(second).unwrap();
        assert!(current > old);
        let reports = inbound_reports(&[8, 1]);
        assert!(owner
            .push_native_report(second, old, &reports[0], 2)
            .unwrap()
            .is_none());
        assert!(owner
            .push_native_report(second, current, &reports[0], 2)
            .unwrap()
            .is_some());
        owner.lifecycle.as_mut().unwrap().transport_failed();
        assert_eq!(owner.capture_report_epoch(second).unwrap(), 0);
        assert!(owner
            .push_native_report(second, current, &reports[0], 3)
            .unwrap()
            .is_none());
    }

    #[test]
    fn settings_readback_rejects_matching_observation_accepted_before_native_start() {
        let (mut owner, connection) = live_owner();
        let operation = owner.plan_operation(0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let delivery = owner.prepare_delivery(&dispatch, 9, 1, 0).unwrap();
        // RPC opens its subscription, then pauses before the queued write starts.
        let early_cursor = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 1,
                    last_report_at_ms: 1,
                }),
            )
            .unwrap();
        let prewrite_match = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 2,
                    last_report_at_ms: 2,
                }),
            )
            .unwrap();
        assert!(
            prewrite_match.sequence > early_cursor.sequence,
            "the old RPC cursor would accept this matching frame"
        );
        assert!(!owner.operation_observation_is_fresh(&operation, &prewrite_match));
        owner.begin_native_report(&delivery, 0, 3).unwrap();
        owner
            .finish_native_report(&delivery, 0, ReportOutcome::Delivered, 4)
            .unwrap();
        assert!(!owner.operation_observation_is_fresh(&operation, &prewrite_match));
        // A partial frame started before dispatch is not repaired by late decoding.
        let partial = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 2,
                    last_report_at_ms: 5,
                }),
            )
            .unwrap();
        assert!(!owner.operation_observation_is_fresh(&operation, &partial));
        let same_tick = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 3,
                    last_report_at_ms: 5,
                }),
            )
            .unwrap();
        assert!(!owner.operation_observation_is_fresh(&operation, &same_tick));
        let no_timing = owner.capture_observation(connection).unwrap();
        assert!(!owner.operation_observation_is_fresh(&operation, &no_timing));
        let later = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 5,
                    last_report_at_ms: 5,
                }),
            )
            .unwrap();
        assert!(owner.operation_observation_is_fresh(&operation, &later));
        owner.disconnect(6, false);
        assert!(!owner.operation_observation_is_fresh(&operation, &later));
    }

    #[test]
    fn settings_observation_boundary_advances_for_each_started_batch_frame() {
        let (mut owner, connection) = live_owner();
        let operation = owner.plan_operation(0, 100).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let first = owner.prepare_delivery(&dispatch, 3, 1, 0).unwrap();
        let second = owner.prepare_delivery(&dispatch, 3, 1, 0).unwrap();
        owner.begin_native_report(&first, 0, 1).unwrap();
        owner
            .finish_native_report(&first, 0, ReportOutcome::Delivered, 2)
            .unwrap();
        let first_echo = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 3,
                    last_report_at_ms: 3,
                }),
            )
            .unwrap();
        assert!(owner.operation_observation_is_fresh(&operation, &first_echo));
        owner.begin_native_report(&second, 0, 4).unwrap();
        owner
            .finish_native_report(&second, 0, ReportOutcome::Delivered, 5)
            .unwrap();
        assert!(!owner.operation_observation_is_fresh(&operation, &first_echo));
        let final_echo = owner
            .capture_observation_with_ingress(
                connection,
                Some(IngressTime {
                    first_report_at_ms: 6,
                    last_report_at_ms: 6,
                }),
            )
            .unwrap();
        assert!(owner.operation_observation_is_fresh(&operation, &final_echo));
    }

    #[test]
    fn native_receipts_distinguish_admission_completion_and_expected_stalls() {
        use crate::device::delivery::DeliveryStatus;
        let (mut owner, _) = live_owner();
        let operation = owner.plan_operation(0, 10).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let first = owner.prepare_delivery(&dispatch, 4, 2, 0).unwrap();
        let second = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        assert_ne!(
            first, second,
            "a reused batch template must issue distinct frame receipts"
        );
        let first: DeliveryTicket =
            serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
        owner.delivery_admitted(&first).unwrap();
        assert_eq!(
            owner.delivery_record(&first).unwrap().status,
            DeliveryStatus::Queued
        );
        assert!(owner.require_delivery_complete(&first).is_err());
        owner.begin_native_report(&first, 0, 5).unwrap();
        assert!(owner.begin_native_report(&first, 0, 5).is_err());
        owner
            .finish_native_report(&first, 0, ReportOutcome::ExpectedStatusStall, 6)
            .unwrap();
        assert!(owner.require_delivery_complete(&first).is_err());
        // Expiry may prevent another frame from starting, but cannot truncate
        // this already-started logical frame.
        assert!(owner.begin_native_report(&second, 0, 10).is_err());
        assert_eq!(
            owner.delivery_record(&second).unwrap().status,
            DeliveryStatus::Cancelled
        );
        owner.begin_native_report(&first, 1, 10).unwrap();
        owner
            .finish_native_report(&first, 1, ReportOutcome::Delivered, 11)
            .unwrap();
        owner.delivery_admitted(&first).unwrap(); // Late producer callback cannot regress completion.
        let record = owner.require_delivery_complete(&first).unwrap();
        assert_eq!(record.operation, Some(operation.identity()));
        assert_eq!(record.delivered_reports, 2);
        assert_eq!(record.expected_stalls, 1);
        assert_eq!(record.started_at_ms, Some(5));
        assert_eq!(record.finished_at_ms, Some(11));
    }

    #[test]
    fn native_failure_and_reset_never_promote_partial_delivery_to_success() {
        use crate::device::delivery::DeliveryStatus;
        let (mut owner, _) = live_owner();
        let dispatch = owner.prepare_dispatch(None, None, 0).unwrap();
        let failed = owner.prepare_delivery(&dispatch, 4, 2, 0).unwrap();
        owner.begin_native_report(&failed, 0, 1).unwrap();
        owner
            .finish_native_report(&failed, 0, ReportOutcome::Failed, 2)
            .unwrap();
        assert_eq!(
            owner.delivery_record(&failed).unwrap().status,
            DeliveryStatus::Indeterminate
        );
        assert!(owner.begin_native_report(&failed, 1, 3).is_err());
        let queued = owner.prepare_delivery(&dispatch, 4, 1, 3).unwrap();
        let in_flight = owner.prepare_delivery(&dispatch, 4, 1, 3).unwrap();
        owner.begin_native_report(&in_flight, 0, 4).unwrap();
        owner.opened(5).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 5));
        assert_eq!(
            owner.delivery_record(&queued).unwrap().status,
            DeliveryStatus::Cancelled
        );
        owner
            .finish_native_report(&in_flight, 0, ReportOutcome::Delivered, 6)
            .unwrap();
        let record = owner.delivery_record(&in_flight).unwrap();
        assert_eq!(record.status, DeliveryStatus::Indeterminate);
        assert_eq!(record.delivered_reports, 0);
        assert_eq!(record.finished_at_ms, Some(5));
        assert!(owner.require_delivery_complete(&in_flight).is_err());
        let current_dispatch = owner.prepare_dispatch(None, None, 6).unwrap();
        let current = owner.prepare_delivery(&current_dispatch, 4, 1, 6).unwrap();
        assert_ne!(current, queued);
        owner.begin_native_report(&current, 0, 7).unwrap();
        // A lifecycle replacement may occur without a physical close/reset.
        owner.lifecycle = Some(SessionDriver::start(8, "next-epoch".into(), 8));
        owner
            .finish_native_report(&current, 0, ReportOutcome::Delivered, 9)
            .unwrap();
        assert_eq!(
            owner.delivery_record(&current).unwrap().status,
            DeliveryStatus::Indeterminate
        );
        assert!(owner.require_delivery_complete(&current).is_err());
    }

    #[test]
    fn delivery_history_is_bounded_without_evicting_unfinished_transfers() {
        let (mut owner, _) = live_owner();
        let dispatch = owner.prepare_dispatch(None, None, 0).unwrap();
        let pending: Vec<_> = (0..256)
            .map(|_| owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap())
            .collect();
        assert_eq!(pending.len(), 256);
        assert!(owner.prepare_delivery(&dispatch, 4, 1, 0).is_err());
        owner
            .stop_delivery(&pending[0], DeliveryStop::QueueRejected, 1)
            .unwrap();
        let replacement = owner.prepare_delivery(&dispatch, 4, 1, 1).unwrap();
        assert_ne!(replacement, pending[0]);
        assert!(owner.delivery_record(&pending[0]).is_err());
        assert!(owner.delivery_record(&pending[1]).is_ok());
        assert!(owner.prepare_delivery(&dispatch, 4, 0, 1).is_err());
    }

    #[test]
    fn delivery_wakes_are_coalesced_and_dropped_observers_release_capacity() {
        let (mut owner, _) = live_owner();
        let watchers: Vec<_> = (0..256)
            .map(|_| owner.watch_deliveries().unwrap())
            .collect();
        assert_eq!(watchers.len(), 256);
        assert!(owner.watch_deliveries().is_err());
        drop(watchers);
        let events = owner.watch_deliveries().unwrap();
        let operation = owner.plan_operation(0, 10).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let ticket = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        owner.delivery_admitted(&ticket).unwrap();
        events.try_recv().unwrap();
        assert!(events.try_recv().is_err());
        assert!(!owner.operation_delivery_complete(&operation, 1).unwrap());
        owner.begin_native_report(&ticket, 0, 1).unwrap();
        owner
            .finish_native_report(&ticket, 0, ReportOutcome::Delivered, 2)
            .unwrap();
        events.try_recv().unwrap();
        assert!(owner.operation_delivery_complete(&operation, 1).unwrap());
        assert!(!owner.operation_delivery_complete(&operation, 2).unwrap());
    }

    #[test]
    fn a_write_batch_keeps_one_budget_through_queue_delay_and_serialization() {
        let (mut owner, _) = live_owner();
        let admission = owner.plan_operation(10, 100).unwrap();
        let queued: OperationTicket =
            serde_json::from_str(&serde_json::to_string(&admission).unwrap()).unwrap();
        assert_eq!(queued, admission);
        assert!(owner.validate_operation(&queued, 10).is_ok());
        assert!(owner.validate_operation(&queued, 109).is_ok());
        assert!(owner
            .validate_operation(&queued, 110)
            .unwrap_err()
            .contains("budget expired"));
        assert!(owner.validate_operation(&queued, 1_000).is_err());
        let next = owner.plan_operation(110, 100).unwrap();
        assert_ne!(next.sequence, queued.sequence);
        assert!(owner.validate_operation(&next, 110).is_ok());
    }

    #[test]
    fn native_dispatch_survives_admission_expiry_after_start_but_never_a_new_epoch() {
        let (mut owner, _) = live_owner();
        let operation = owner.plan_operation(0, 10).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let serialized = serde_json::to_string(&dispatch).unwrap();
        let native: NativeDispatchTicket = serde_json::from_str(&serialized).unwrap();
        assert_eq!(native, dispatch);
        assert!(owner.validate_dispatch(&native, false, 9).is_ok());
        assert!(owner.validate_dispatch(&native, false, 10).is_err());
        assert!(owner.validate_dispatch(&native, true, 100).is_ok());
        assert!(owner.prepare_dispatch(Some(&operation), None, 10).is_err());
        owner.lifecycle = Some(SessionDriver::start(8, "next".into(), 101));
        assert!(owner.validate_dispatch(&native, false, 101).is_err());
        assert!(owner.validate_dispatch(&native, true, 101).is_err());
    }

    #[test]
    fn asynchronous_preflight_cannot_rebind_a_plan_to_a_replacement_session() {
        let (mut owner, _) = live_owner();
        let serialized = serde_json::to_string(&owner.session_fence().unwrap()).unwrap();
        let fence: SessionFence = serde_json::from_str(&serialized).unwrap();
        let admission = owner.plan_operation_for(&fence, 100, 10).unwrap();
        assert_eq!(admission.remaining_ms(109), 1);
        owner.opened(111).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "same".into(), 111));
        assert!(owner.plan_operation_for(&fence, 111, 1000).is_err());
        assert!(owner.validate_operation(&admission, 105).is_err());
        let replacement = owner.session_fence().unwrap();
        assert!(owner.plan_operation_for(&replacement, 111, 1000).is_ok());
    }

    #[test]
    fn queued_writes_cannot_cross_physical_replacement_even_if_the_wire_id_repeats() {
        let (mut owner, _) = live_owner();
        let queued = owner.plan_operation(0, 100).unwrap();
        owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 1));
        assert_eq!(owner.lifecycle.as_ref().unwrap().epoch(), queued.epoch);
        assert!(owner
            .validate_operation(&queued, 1)
            .unwrap_err()
            .contains("session changed"));
        let current = owner.plan_operation(1, 100).unwrap();
        assert!(owner.validate_operation(&current, 1).is_ok());
        owner.lifecycle = Some(SessionDriver::start(8, "rebuild".into(), 2));
        assert!(owner.validate_operation(&current, 2).is_err());
        let failed = owner.plan_operation(2, 100).unwrap();
        owner.lifecycle.as_mut().unwrap().transport_failed();
        assert!(owner.validate_operation(&failed, 2).is_err());
        assert!(owner.plan_operation(2, 100).is_err());
        owner.disconnect(3, false);
        assert!(owner.plan_operation(3, 100).is_err());
    }

    #[test]
    fn native_state_versions_order_reductions_within_one_session() {
        let (mut owner, connection) = live_owner();
        assert!(owner.state.snapshot().state_version.is_none());
        let first_ticket = owner.capture_observation(connection).unwrap();
        let first = owner
            .observe_state(&first_ticket, 15, &preset_payload())
            .unwrap()
            .unwrap();
        let original = owner.current_preset().unwrap().clone();
        let version = original.state_version.clone().unwrap();
        assert!(!first.states.is_empty());
        assert!(first
            .states
            .iter()
            .all(|state| state.state_version.as_ref() == Some(&version)));

        let next_ticket = owner.capture_observation(connection).unwrap();
        owner
            .observe_state(&next_ticket, 15, &preset_payload())
            .unwrap();
        let next = owner.current_preset().unwrap();
        assert_eq!(next.preset_identity, original.preset_identity);
        let next_version = next.state_version.as_ref().unwrap();
        assert_eq!(next_version.session, version.session);
        assert!(next_version.revision > version.revision);
        assert_eq!(
            original.state_version.as_ref(),
            Some(&version),
            "retained results keep their original revision"
        );

        let replacement = owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 1));
        assert!(owner
            .observe_state(&first_ticket, 15, &preset_payload())
            .unwrap()
            .is_none());
        observe_preflight(&mut owner, replacement, 15, preset_payload());
        let replaced = owner
            .current_preset()
            .unwrap()
            .state_version
            .as_ref()
            .unwrap();
        assert_ne!(replaced.session, version.session);
        assert_eq!(replaced.revision, 1);
    }

    #[test]
    fn deferred_metadata_versions_the_reprojection_after_newer_wire_state() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let (mut owner, connection) = live_owner();
        let metadata_ticket = owner.capture_observation(connection).unwrap();
        observe_preflight(&mut owner, connection, 15, preset_payload());
        let before = owner
            .current_preset()
            .unwrap()
            .state_version
            .clone()
            .unwrap();
        let message = pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                br#"<Models><Category name="Delay"><Model id="7" name="Echo" /></Category></Models>"#.to_vec()
            )),
            ..Default::default()
        };
        let catalog = crate::state::parse_model_repo(&message.encode_to_vec()).unwrap();
        let states = owner
            .install_model_catalog(&metadata_ticket, Ok(catalog), 1)
            .unwrap()
            .unwrap();
        let after = owner
            .current_preset()
            .unwrap()
            .state_version
            .as_ref()
            .unwrap();
        assert_eq!(after.session, before.session);
        assert!(after.revision > before.revision);
        assert!(!states.is_empty());
        assert!(states
            .iter()
            .all(|state| state.state_version.as_ref() == Some(after)));
        let after = after.clone();
        let publications = owner.take_state_publications().unwrap();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications[0].states[0].state_version.as_ref(),
            Some(&before)
        );
        assert_eq!(
            publications[1].states[0].state_version.as_ref(),
            Some(&after)
        );
        assert_eq!(publications[1].observation, metadata_ticket);
        assert!(!owner.state_publications_ready());
    }

    #[test]
    fn replacement_discards_pending_publications_and_revokes_already_drained_batches() {
        let (mut owner, connection) = live_owner();
        observe_preflight(&mut owner, connection, 15, preset_payload());
        let drained = owner.take_state_publications().unwrap();
        observe_preflight(&mut owner, connection, 15, preset_payload());
        assert!(owner.state_publications_ready());
        let replacement = owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 1));
        assert!(!owner.state_publications_ready());
        assert!(!owner.accepts_observation(&drained[0].observation));
        observe_preflight(&mut owner, replacement, 15, preset_payload());
        let fresh = owner.take_state_publications().unwrap();
        assert_eq!(fresh.len(), 1);
        assert!(owner.accepts_observation(&fresh[0].observation));
    }

    #[test]
    fn a_publication_overflow_invalidates_native_admission_and_cached_preset_state() {
        let (mut owner, connection) = live_owner();
        for _ in 0..crate::domain::STATE_EVENT_MAXIMUM_LIMIT {
            observe_preflight(&mut owner, connection, 15, preset_payload());
        }
        let ticket = owner.capture_observation(connection).unwrap();
        let error = owner
            .observe_state(&ticket, 15, &preset_payload())
            .unwrap_err();
        assert!(error.contains("publication backlog"), "{error}");
        assert!(!owner.accepts_observation(&ticket));
        assert!(owner.current_preset().is_none());
        assert!(owner.session_context().is_err());
        assert!(owner.take_state_publications().is_err());
    }

    #[test]
    fn native_snapshot_read_keeps_dirty_until_the_device_publishes_clean() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;

        let (mut owner, connection) = live_owner();
        observe_preflight(&mut owner, connection, 15, preset_payload());
        for is_dirty in [true, false, true] {
            let payload = pa::PresetDirtyMessage {
                action: pa::message_action::Enum::Update as i32,
                is_dirty,
                ..Default::default()
            }
            .encode_to_vec();
            observe_preflight(&mut owner, connection, 34, payload);
            observe_preflight(&mut owner, connection, 15, preset_payload());
            let snapshot = serde_json::to_value(owner.current_preset().unwrap()).unwrap();
            assert_eq!(snapshot["dirty"], is_dirty);
            assert!(snapshot["presetIdentity"].as_str().is_some());
        }
    }

    #[test]
    fn a_snapshot_requires_an_original_preset_observation_in_this_epoch() {
        let (mut owner, connection) = live_owner();
        owner.state.snapshot_mut().has_preset = true;
        assert!(owner.current_preset().is_none());
        let ticket = owner.capture_observation(connection).unwrap();
        assert!(owner
            .observe_state(&ticket, 15, &preset_payload())
            .unwrap()
            .is_some());
        assert!(owner.current_preset().is_some());
        // A catalog/save-name reconciliation remains visible, but does not
        // become a new wire observation or certify a command's completion.
        owner.state.snapshot_mut().preset_name = "Saved name".into();
        assert_eq!(owner.current_preset().unwrap().preset_name, "Saved name");
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 1));
        assert!(owner.state.snapshot().has_preset);
        assert!(owner.current_preset().is_none());
        assert!(owner
            .observe_state(&ticket, 15, &preset_payload())
            .unwrap()
            .is_none());
        let current = owner.capture_observation(connection).unwrap();
        owner
            .observe_state(&current, 15, &preset_payload())
            .unwrap();
        assert!(owner.current_preset().is_some());
        owner.disconnect(2, false);
        assert!(owner.current_preset().is_none());
    }

    #[test]
    fn parameter_retention_overflow_revokes_snapshot_write_operands_and_publishes_incompleteness() {
        use crate::proto::{
            chain, model, param, param_value, BinaryPreset, Chain, Model, Param, ParamValue,
        };
        use serde_json::json;
        let (mut owner, connection) = live_owner();
        let preset = BinaryPreset {
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![Model {
                    column: Some(model::Column::Column(0)),
                    hash: Some(model::Hash::Hash(7)),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let recall = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(preset)),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner.capture_observation(connection).unwrap();
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_RECALL_PRESET, &recall)
            .unwrap();
        assert!(owner.current_preset().is_some());
        assert!(owner.state.snapshot().grid_layout_complete);
        assert_eq!(owner.state.snapshot().blocks.len(), 1);
        let identity = owner.caller_preset_identity().unwrap();
        owner.take_state_publications().unwrap();

        // Ordinary malformed messages do not revoke a successfully retained preset.
        assert!(owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_GRID, &[0xff])
            .is_err());
        assert!(owner.current_preset().is_some());
        assert!(owner.state.snapshot().grid_layout_complete);
        assert!(owner.take_state_publications().unwrap().is_empty());

        let overflow = pa::GridMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::grid_message::Preset::Preset(BinaryPreset {
                chains: vec![Chain {
                    row: Some(chain::Row::Row(0)),
                    models: vec![Model {
                        column: Some(model::Column::Column(0)),
                        params: (0..65_537)
                            .map(|index| Param {
                                index: Some(param::Index::Index(index)),
                                param_values: vec![ParamValue {
                                    value: Some(param_value::Value::FloatValue(0.5)),
                                }],
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        assert!(owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_GRID, &overflow)
            .is_err());
        assert!(owner.current_preset().is_none());
        assert!(owner.assert_caller_preset_identity(&identity).is_err());
        let retained = owner.state.snapshot();
        assert!(!retained.has_preset);
        assert!(!retained.grid_layout_complete);
        assert!(retained.blocks.is_empty());
        assert!(retained.routes.is_empty());
        assert!(retained.preset_identity.is_none());
        let refused = crate::device::request::plan_gateway_write_with_state(
            "device.removeBlock",
            &json!({"row": 0, "column": 0}),
            &owner.state,
        )
        .unwrap_err();
        assert!(refused.starts_with("STATE_UNAVAILABLE:"), "{refused}");
        let published = owner.take_state_publications().unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].states.len(), 1);
        let update = &published[0].states[0];
        assert_eq!(update.kind, "gridTopology");
        assert_eq!(update.grid_layout_complete, Some(false));
        assert!(update.state_version.is_some());
        assert!(owner.take_state_publications().unwrap().is_empty());

        // Only a new complete device preset can restore this authority.
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_RECALL_PRESET, &recall)
            .unwrap();
        assert!(owner.current_preset().is_some());
        assert!(owner.state.snapshot().grid_layout_complete);
        assert_eq!(owner.state.snapshot().blocks.len(), 1);
    }

    #[test]
    fn caller_identity_survives_reads_but_not_address_aba_or_session_replacement() {
        use crate::state::StateUpdate;
        let (mut owner, connection) = live_owner();
        assert!(owner.caller_preset_identity().is_err());
        let mut position = StateUpdate::empty("position");
        position.position = Some(19);
        position.setlist_key = Some("/user/A".into());
        owner.state.snapshot_mut().apply(&position);
        let ticket = owner.capture_observation(connection).unwrap();
        owner.observe_state(&ticket, 15, &preset_payload()).unwrap();
        let initial = owner.caller_preset_identity().unwrap();
        assert!(owner.assert_caller_preset_identity("").is_err());
        assert!(owner.assert_caller_preset_identity(&initial).is_ok());
        let read = owner.capture_observation(connection).unwrap();
        owner.observe_state(&read, 15, &preset_payload()).unwrap();
        owner.state.snapshot_mut().tempo = 130;
        position.setlist_key = Some("/user/A/".into());
        owner.state.snapshot_mut().apply(&position);
        assert_eq!(owner.caller_preset_identity().unwrap(), initial);
        position.setlist_key = Some("/user/B".into());
        owner.state.snapshot_mut().apply(&position);
        assert!(owner.assert_caller_preset_identity(&initial).is_err());
        position.setlist_key = Some("/user/A".into());
        owner.state.snapshot_mut().apply(&position);
        assert!(owner.assert_caller_preset_identity(&initial).is_err());
        let returned = owner.caller_preset_identity().unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "replacement".into(), 1));
        assert!(owner.caller_preset_identity().is_err());
        let replacement = owner.capture_observation(connection).unwrap();
        owner
            .observe_state(&replacement, 15, &preset_payload())
            .unwrap();
        assert!(owner.assert_caller_preset_identity(&returned).is_err());
        let current = owner.caller_preset_identity().unwrap();
        assert!(owner.assert_caller_preset_identity(&current).is_ok());
        owner.disconnect(2, false);
        assert!(owner.assert_caller_preset_identity(&current).is_err());
    }

    #[test]
    fn public_preset_guard_rejects_same_name_at_another_address_and_aba() {
        use crate::device::request::assert_expected_preset;
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        let (mut owner, connection) = live_owner();
        let seed = owner.capture_observation(connection).unwrap();
        let observed = owner
            .observe_state(&seed, 15, &preset_payload())
            .unwrap()
            .unwrap();
        let initial = owner
            .current_preset()
            .unwrap()
            .preset_identity
            .clone()
            .unwrap();
        assert_eq!(
            observed.states[0].preset_identity.as_deref(),
            Some(initial.as_str())
        );
        let params = serde_json::json!({"expectedPresetIdentity": initial});
        assert!(assert_expected_preset(owner.current_preset(), &params).is_ok());
        for position in [1, 0] {
            let payload = pa::SetlistPositionMessage {
                action: pa::message_action::Enum::Update as i32,
                position: Some(pa::setlist_position_message::Position::Position(position)),
                ..Default::default()
            }
            .encode_to_vec();
            let ticket = owner.capture_observation(connection).unwrap();
            let update = owner
                .observe_state(
                    &ticket,
                    crate::profile::MESSAGE_TYPE_SETLIST_POSITION,
                    &payload,
                )
                .unwrap()
                .unwrap();
            assert_ne!(
                update.states[0].preset_identity.as_deref(),
                Some(initial.as_str())
            );
            assert!(assert_expected_preset(owner.current_preset(), &params).is_err());
        }
        owner.disconnect(1, false);
        assert!(assert_expected_preset(owner.current_preset(), &params).is_err());
    }

    #[test]
    fn a_failed_lifecycle_revokes_cached_preset_and_readiness_before_host_drain() {
        let (mut owner, connection) = live_owner();
        let ticket = owner.capture_observation(connection).unwrap();
        owner.observe_state(&ticket, 15, &preset_payload()).unwrap();
        owner.transport.handshake_completed(0, true);
        assert!(owner.is_connected());
        // A leftover transport sync flag cannot override startup state.
        assert!(!owner.is_synchronized());
        assert!(owner.current_preset().is_some());
        owner.lifecycle.as_mut().unwrap().transport_failed();
        assert!(owner.transport.is_connected());
        assert!(owner.transport.synchronized());
        assert!(owner.state.snapshot().has_preset);
        assert!(!owner.is_connected());
        assert!(!owner.is_synchronized());
        assert!(owner.current_preset().is_none());
    }

    #[test]
    fn metadata_keeps_one_active_parse_and_the_newest_pending_observation() {
        let (mut owner, connection) = live_owner();
        let first = owner.capture_observation(connection).unwrap();
        assert!(owner.queue_model_repo(first.clone(), vec![1]));
        assert_eq!(owner.take_model_repo().unwrap().payload, vec![1]);
        let second = owner.capture_observation(connection).unwrap();
        let newest = owner.capture_observation(connection).unwrap();
        assert!(owner.queue_model_repo(second.clone(), vec![2]));
        assert!(owner.queue_model_repo(newest.clone(), vec![3]));
        assert!(!owner.queue_model_repo(second, vec![99]));
        assert!(!owner.queue_model_repo(newest.clone(), vec![99]));
        assert!(!owner.model_repo_ready());
        assert!(owner.take_model_repo().is_none());
        // A parse failure outside Building is an observation error, not a
        // lifecycle failure. It still releases this exact active slot.
        assert!(owner
            .complete_model_repo(&first, Err("parse failed".into()), 0)
            .is_err());
        assert!(owner.model_repo_ready());
        let work = owner.take_model_repo().unwrap();
        assert_eq!(work.ticket, newest);
        assert_eq!(work.payload, vec![3]);
        assert!(owner
            .complete_model_repo(&first, Err("duplicate result".into()), 0)
            .unwrap()
            .is_none());
        assert!(owner.take_model_repo().is_none());
        assert!(owner
            .complete_model_repo(&work.ticket, Err("last failure".into()), 0)
            .is_err());
        assert!(!owner.model_repo_ready());
    }

    #[test]
    fn metadata_reset_discards_pending_work_and_rejects_the_old_active_receipt() {
        let (mut owner, connection) = live_owner();
        let old = owner.capture_observation(connection).unwrap();
        owner.queue_model_repo(old.clone(), vec![1]);
        let _parsing = owner.take_model_repo().unwrap();
        let pending = owner.capture_observation(connection).unwrap();
        owner.queue_model_repo(pending, vec![2]);
        let replacement = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
        assert!(owner.take_model_repo().is_none());
        let current = owner.capture_observation(replacement).unwrap();
        assert!(owner.queue_model_repo(current.clone(), vec![3]));
        assert_eq!(owner.take_model_repo().unwrap().ticket, current);
        assert!(owner
            .complete_model_repo(&old, Err("late error".into()), 0)
            .unwrap()
            .is_none());
        assert!(owner.take_model_repo().is_none());
        assert!(owner.lifecycle.as_ref().unwrap().failure().is_none());
        assert_eq!(owner.metadata_active, Some(current));
    }

    #[test]
    fn oversized_metadata_cannot_replace_an_admitted_snapshot() {
        let (mut owner, connection) = live_owner();
        let valid = owner.capture_observation(connection).unwrap();
        assert!(owner.queue_model_repo(valid.clone(), vec![1]));
        let oversized = owner.capture_observation(connection).unwrap();
        let limit = crate::compression::max_delivered_payload_bytes_for(
            crate::profile::MESSAGE_TYPE_MODEL_REPO,
        );
        assert!(!owner.queue_model_repo(oversized, vec![0; limit + 1]));
        assert_eq!(owner.take_model_repo().unwrap().ticket, valid);
        // A ModelRepo payload the transport inflated past the raw
        // ceiling but within its own limit is queued, not silently dropped.
        let inflated = owner.capture_observation(connection).unwrap();
        assert!(owner.queue_model_repo(
            inflated.clone(),
            vec![0; crate::profile::MAX_FRAME_BYTES + 1]
        ));
        assert_eq!(owner.metadata_pending.as_ref().unwrap().ticket, inflated);
    }

    #[test]
    fn a_parser_receipt_cannot_cross_reset_even_when_the_wire_identity_repeats() {
        let mut owner = SessionRuntime::new(0);
        let first = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(1, "wire-session".into(), 0));
        let ticket = owner.capture_observation(first).unwrap();
        assert!(owner.accepts_observation(&ticket));
        owner.reset_state();
        assert!(!owner.accepts_observation(&ticket));
        let second = owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(1, "wire-session".into(), 1));
        assert_ne!(first, second);
        assert!(!owner.accepts_observation(&ticket));
        assert!(owner.capture_observation(first).is_err());
        let current = owner.capture_observation(second).unwrap();
        assert!(owner.accepts_observation(&current));
        assert!(owner
            .install_model_catalog(&ticket, Err("old parser failed".into()), 1)
            .unwrap()
            .is_none());
        assert!(owner.lifecycle.as_ref().unwrap().failure().is_none());
    }

    #[test]
    fn parsing_does_not_own_the_session_and_installation_rechecks_its_ticket() {
        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(1, "first".into(), 0));
        let parsing = owner.capture_observation(connection).unwrap();
        owner.lifecycle = Some(SessionDriver::start(2, "replacement".into(), 1));
        assert!(!owner.accepts_observation(&parsing));
        let current = owner.capture_observation(connection).unwrap();
        assert!(owner.accepts_observation(&current));
    }

    #[test]
    fn old_metadata_cannot_replace_a_new_sessions_catalog() {
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;
        fn catalog(name: &str) -> crate::state::ModelCatalog {
            let xml = format!(
                "<Models><Category name=\"Delay\"><Model id=\"7\" name=\"{name}\" /></Category></Models>"
            );
            let message = pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(xml.into_bytes()),
                ),
                ..Default::default()
            };
            crate::state::parse_model_repo(&message.encode_to_vec()).unwrap()
        }
        let mut owner = SessionRuntime::new(0);
        let first = owner.opened(0).unwrap();
        owner.lifecycle = Some(SessionDriver::start(1, "old".into(), 0));
        let old_input = owner.capture_observation(first).unwrap();
        let old_parse = catalog("Old catalog");
        let second = owner.opened(1).unwrap();
        owner.lifecycle = Some(SessionDriver::start(1, "new".into(), 1));
        let new_input = owner.capture_observation(second).unwrap();
        assert!(owner
            .install_model_catalog(&new_input, Ok(catalog("Current catalog")), 1)
            .unwrap()
            .is_some());
        assert!(owner
            .install_model_catalog(&old_input, Ok(old_parse), 1)
            .unwrap()
            .is_none());
        let models = owner.state.model_list().unwrap();
        assert_eq!(models.audit.model_count, 1);
        assert_eq!(models.models[0].name, "Current catalog");
    }

    #[test]
    fn install_model_catalog_reports_a_fatally_empty_catalog_as_an_error_not_ok_none() {
        // Regression: observe_model_catalog fails closed the same way for a
        // genuine parse error and a parsed-but-fatally-empty catalog (see
        // parsed_catalog_failure_matches_the_wire_failure_only_during_building
        // in initialization.rs), but `catalog` here is still `Ok(_)` in the
        // empty case, so install_model_catalog used to forward it through
        // `catalog.map(|_| None)` -- the same "not ready yet" signal used for
        // a merely-stale ticket -- instead of reporting the failure as `Err`.
        use crate::device::initialization::DeviceStartupPhase;
        use crate::device::session::DispatchOutcome;
        use crate::proto::cortex_protobuf_v2 as pa;
        use prost::Message;

        let mut owner = SessionRuntime::new(0);
        let connection = owner.opened(0).unwrap();
        let mut driver = SessionDriver::start(7, "session".into(), 0);

        // Drive the shared startup lifecycle through to `Building`, the same
        // sequence proven by
        // session::tests::rejected_metadata_revokes_queued_startup_work_without_a_timer_tick.
        let reset = driver.next_batch(0, 1_000_000).unwrap();
        assert!(driver.dispatched(&reset.ticket, DispatchOutcome::Accepted));
        driver.observe(
            crate::profile::MESSAGE_TYPE_RESET_COMMS_BUFFERS,
            &pa::ResetCommsBuffersMessage {
                session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                    "session".into(),
                )),
                ..Default::default()
            }
            .encode_to_vec(),
            10,
        );
        let read_version = driver.next_batch(10, 1_000_000).unwrap();
        assert!(driver.dispatched(&read_version.ticket, DispatchOutcome::Accepted));
        driver.observe(
            crate::profile::MESSAGE_TYPE_VERSION,
            &pa::VersionMessage {
                action: pa::message_action::Enum::Update as i32,
                device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                    pa::version_message::DeviceType::Qc as i32,
                )),
                cortex_control_version_valid: Some(
                    pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
            20,
        );
        let disconnect = driver.next_batch(20, 1_000_000).unwrap();
        assert!(driver.dispatched(&disconnect.ticket, DispatchOutcome::Accepted));
        driver.next_batch(21, 0).unwrap();
        assert_eq!(driver.phase(), DeviceStartupPhase::Building);

        owner.lifecycle = Some(driver);
        let ticket = owner.capture_observation(connection).unwrap();

        let empty_catalog = crate::state::parse_model_repo(
            &pa::ModelRepoMessage {
                action: pa::message_action::Enum::Update as i32,
                model_repo_payload: Some(
                    pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                        b"<Models />".to_vec(),
                    ),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
        )
        .unwrap();
        assert_eq!(empty_catalog.model_list().audit.model_count, 0);

        let error = owner
            .install_model_catalog(&ticket, Ok(empty_catalog), 22)
            .expect_err("a fatally empty catalog must fail, not silently report Ok(None)");
        assert!(!error.is_empty());
        assert!(owner.lifecycle.as_ref().unwrap().failure().is_some());
    }

    fn model_preset_payload(action: pa::message_action::Enum, values: &[&str]) -> Vec<u8> {
        pa::ModelPresetMessage {
            action: action as i32,
            presets: values
                .iter()
                .map(|value| pa::ModelPreset {
                    id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                        value: Some(pa::model_preset_id::Value::Value((*value).into())),
                        is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(false)),
                        hash: Some(pa::model_preset_id::Hash::Hash(
                            crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
                        )),
                    })),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
        .encode_to_vec()
    }

    /// A live owner whose reader has an epoch, as a host has after attach.
    fn ingesting_owner() -> (SessionRuntime, u64, u64) {
        let (mut owner, connection) = live_owner();
        let epoch = owner.capture_report_epoch(connection).unwrap();
        (owner, connection, epoch)
    }

    fn inbound_message(message_type: u16, payload: &[u8]) -> Vec<Vec<u8>> {
        crate::framing::encode(message_type, payload)
            .into_iter()
            .map(|mut report| {
                report[0] = crate::framing::IN_REPORT_ID;
                report.to_vec()
            })
            .collect()
    }

    /// What both hosts do with one inbound message: push its reports into the
    /// owner, then reduce the completed frame.
    fn deliver(owner: &mut SessionRuntime, connection: u64, epoch: u64, payload: &[u8]) {
        let mut completed = None;
        for report in inbound_message(crate::profile::MESSAGE_TYPE_MODEL_PRESET, payload) {
            completed = owner
                .push_native_report(connection, epoch, &report, 1)
                .unwrap();
        }
        let (frame, ticket) = completed.expect("the message completes");
        owner
            .observe_state(&ticket, frame.message_type, &frame.payload)
            .unwrap();
    }

    /// A complete frame of another type, so a test can lose a fragment after the first frame.
    fn complete_any_frame(owner: &mut SessionRuntime, connection: u64, epoch: u64) {
        for report in inbound_message(13, &[8, 1, 16, 42]) {
            owner
                .push_native_report(connection, epoch, &report, 1)
                .unwrap();
        }
    }

    /// The planner step of a Global EQ preset load, as both hosts take it:
    /// the owner's own device-state runtime, nothing else.
    fn plan_global_eq_load(owner: &SessionRuntime, value: &str) -> Result<(), String> {
        crate::device::request::plan_gateway_write_with_state(
            "device.loadGlobalEqModelPreset",
            &serde_json::json!({
                "value": value,
                "isFactory": false,
                "hash": crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
            }),
            &owner.state,
        )
        .map(|_| ())
    }

    /// A delete listing enough entries to need several reports, and the
    /// reports after the first: the part that arrives when the first was lost.
    fn lost_delete_tail(values: &[&str]) -> Vec<Vec<u8>> {
        let payload = model_preset_payload(pa::message_action::Enum::Delete, values);
        let reports = inbound_message(crate::profile::MESSAGE_TYPE_MODEL_PRESET, &payload);
        assert!(reports.len() >= 2, "the delete must span several reports");
        reports[1..].to_vec()
    }

    fn filler() -> Vec<String> {
        (0..16)
            .map(|index| format!("filler-with-a-long-name-{index}"))
            .collect()
    }

    #[test]
    fn the_model_preset_fold_carries_its_session_and_dies_with_it() {
        use crate::state::GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE;
        let (mut owner, connection) = live_owner();
        let ticket = owner.capture_observation(connection).unwrap();
        let payload = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        let observation = owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_MODEL_PRESET, &payload)
            .unwrap()
            .unwrap();
        let catalog = observation.states[0].model_preset_catalog.as_ref().unwrap();
        assert_eq!(catalog.state, "folding");
        // The event, the snapshot copy and the stamp all name one session.
        let version = observation.states[0].state_version.clone().unwrap();
        assert_eq!(catalog.session, version.session);
        let retained = owner
            .state
            .snapshot()
            .model_preset_catalog
            .as_ref()
            .unwrap();
        assert_eq!(retained.session, version.session);
        assert_eq!(retained.entries, catalog.entries);
        // Once a preset is observed, a snapshot read carries the list, and a write guard's
        // preflight observation does not.
        owner.preset_observation = Some(ticket.clone());
        owner.state.snapshot_mut().has_preset = true;
        let snapshot = serde_json::to_value(owner.current_preset().unwrap()).unwrap();
        assert_eq!(snapshot["modelPresetCatalog"]["session"], version.session);
        assert!(owner
            .decoded_preflight_observation("device.selectScene")
            .unwrap()
            .get("modelPresetCatalog")
            .is_none());
        assert!(owner
            .state
            .model_preset_catalog()
            .admits("one", false, SCOPE)
            .is_ok());

        // A stale tuple is refused once the device deletes it.
        let delete = model_preset_payload(pa::message_action::Enum::Delete, &["one"]);
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_MODEL_PRESET, &delete)
            .unwrap();
        assert!(owner
            .state
            .model_preset_catalog()
            .admits("one", false, SCOPE)
            .is_err());

        // A new connection is a new session: nothing carries over.
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_MODEL_PRESET, &payload)
            .unwrap();
        owner.opened(5).unwrap();
        assert!(owner.state.snapshot().model_preset_catalog.is_none());
        assert!(owner
            .state
            .model_preset_catalog()
            .admits("one", false, SCOPE)
            .unwrap_err()
            .message()
            .starts_with("STATE_UNAVAILABLE:"));
    }

    #[test]
    fn a_lost_delete_invalidates_at_ingress_so_the_next_load_is_refused_with_no_later_traffic() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one", "two"]);
        deliver(&mut owner, connection, epoch, &create);
        owner.take_state_publications().unwrap();
        assert!(plan_global_eq_load(&owner, "one").is_ok());

        // The device deletes "one", but the first report of that delete never arrives. Its
        // later reports are orphans. Nothing completes, and nothing follows.
        let values = filler();
        let mut listed: Vec<&str> = vec!["one"];
        listed.extend(values.iter().map(String::as_str));
        for report in lost_delete_tail(&listed) {
            let _ = owner.push_native_report(connection, epoch, &report, 2);
        }
        let lost = owner.inbound_telemetry();
        assert!(lost.orphaned_continuation_reports + lost.framing_errors > 0);

        // The very next load, with no frame in between, is refused once the transport has counted
        // the loss: the stale admission of "one" does not survive it, and neither does the
        // still-listed "two".
        for value in ["one", "two"] {
            let refused = plan_global_eq_load(&owner, value).unwrap_err();
            assert!(refused.starts_with("STATE_UNAVAILABLE:"), "{refused}");
            assert!(refused.contains("inbound-loss"), "{refused}");
        }
        let retained = owner.state.snapshot().model_preset_catalog.clone().unwrap();
        assert_eq!(retained.state, "invalidated");
        assert_eq!(retained.entries, None);
        // The invalidation was staged for subscribers at ingress, stamped with the session.
        let published = owner.take_state_publications().unwrap();
        assert_eq!(published.len(), 1);
        let catalog = published[0].states[0]
            .model_preset_catalog
            .as_ref()
            .unwrap();
        assert_eq!(catalog.invalid_reason.as_deref(), Some("inbound-loss"));
        assert_eq!(Some(catalog.session.as_str()), retained_session(&owner));
    }

    fn retained_session(owner: &SessionRuntime) -> Option<&str> {
        owner
            .state
            .snapshot()
            .state_version
            .as_ref()
            .map(|version| version.session.as_str())
    }

    #[test]
    fn a_fragment_lost_before_the_first_complete_frame_leaves_a_later_create_unusable() {
        // The first thing the session sees is the tail of a message it never saw the start of:
        // the shape of a partial boot publication whose first report was lost. There is no
        // grace period for the attach, and a later append cannot prove the list whole.
        let (mut owner, connection, epoch) = ingesting_owner();
        let values = filler();
        let listed: Vec<&str> = values.iter().map(String::as_str).collect();
        for report in lost_delete_tail(&listed) {
            let _ = owner.push_native_report(connection, epoch, &report, 1);
        }
        let lost = owner.inbound_telemetry();
        assert!(lost.orphaned_continuation_reports + lost.framing_errors > 0);
        let retained = owner.state.snapshot().model_preset_catalog.clone().unwrap();
        assert_eq!(retained.state, "invalidated");
        assert_eq!(retained.invalid_reason.as_deref(), Some("inbound-loss"));

        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        let refused = plan_global_eq_load(&owner, "one").unwrap_err();
        assert!(refused.starts_with("STATE_UNAVAILABLE:"), "{refused}");
        assert!(refused.contains("inbound-loss"), "{refused}");
        assert_eq!(
            owner
                .state
                .snapshot()
                .model_preset_catalog
                .clone()
                .unwrap()
                .entries,
            None
        );
    }

    #[test]
    fn a_loss_after_a_complete_frame_and_before_any_list_message_leaves_a_later_list_unusable() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        // The boot update may be the frame that was lost.
        let values = filler();
        let listed: Vec<&str> = values.iter().map(String::as_str).collect();
        for report in lost_delete_tail(&listed) {
            let _ = owner.push_native_report(connection, epoch, &report, 2);
        }
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        let update = model_preset_payload(pa::message_action::Enum::Update, &["two", "three"]);
        deliver(&mut owner, connection, epoch, &update);
        for value in ["one", "two", "three"] {
            assert!(plan_global_eq_load(&owner, value)
                .unwrap_err()
                .contains("inbound-loss"));
        }
    }

    #[test]
    fn a_loss_counted_before_the_session_was_created_is_not_this_sessions() {
        let (mut owner, connection, epoch) = ingesting_owner();
        let values = filler();
        let listed: Vec<&str> = values.iter().map(String::as_str).collect();
        for report in lost_delete_tail(&listed) {
            let _ = owner.push_native_report(connection, epoch, &report, 1);
        }
        assert_eq!(
            owner
                .state
                .snapshot()
                .model_preset_catalog
                .clone()
                .unwrap()
                .state,
            "invalidated"
        );
        // A new connection on the same owner: the transport tally still holds that loss, but
        // the new fold was seeded with it when the decoder was reset, before any ingress.
        let connection = owner.opened(5).unwrap();
        owner.lifecycle = Some(SessionDriver::start(8, "second".into(), 5));
        let epoch = owner.capture_report_epoch(connection).unwrap();
        assert!(owner.state.snapshot().model_preset_catalog.is_none());
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        assert!(plan_global_eq_load(&owner, "one").is_ok());
        // And a loss in the new session still invalidates it.
        for report in lost_delete_tail(&listed) {
            let _ = owner.push_native_report(connection, epoch, &report, 6);
        }
        assert!(plan_global_eq_load(&owner, "one")
            .unwrap_err()
            .contains("inbound-loss"));
    }

    #[test]
    fn a_failure_typed_as_another_message_does_not_invalidate_the_fold() {
        let (mut owner, connection, epoch) = ingesting_owner();
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        // A complete frame of another type that this build refuses (encrypted). Its type is
        // known from the envelope and the fold has no dependency on it.
        let mut report = crate::framing::encode_frame(&crate::framing::Frame {
            message_type: 13,
            payload: vec![8, 1],
            encrypted: true,
            compressed: false,
            device_bytes: [0, 0],
        })[0]
            .to_vec();
        report[0] = crate::framing::IN_REPORT_ID;
        assert!(owner
            .push_native_report(connection, epoch, &report, 3)
            .is_err());
        assert_eq!(owner.inbound_telemetry().encrypted_rejections[&13], 1);
        assert!(plan_global_eq_load(&owner, "one").is_ok());
        assert_eq!(
            owner
                .state
                .snapshot()
                .model_preset_catalog
                .clone()
                .unwrap()
                .state,
            "folding"
        );
    }

    /// The write of a Global EQ preset recall, planned and queued up to the point just before its
    /// first frame starts: the shape both hosts reach before `begin_native_report`.
    fn queued_global_eq_load(
        owner: &mut SessionRuntime,
        value: &str,
    ) -> crate::device::delivery::DeliveryTicket {
        let method = "device.loadGlobalEqModelPreset";
        let params = serde_json::json!({
            "value": value,
            "isFactory": false,
            "hash": crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
        });
        let plan =
            crate::device::request::plan_gateway_write_with_state(method, &params, &owner.state)
                .unwrap();
        let context = owner
            .context_for_write(method, &params, &plan.write)
            .unwrap();
        let operation = owner.plan_operation_in_context(&context, 1, 3_000).unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 1).unwrap();
        let delivery = owner
            .prepare_delivery(&dispatch, crate::profile::MESSAGE_TYPE_GLOBAL_EQ, 1, 1)
            .unwrap();
        owner.delivery_admitted(&delivery).unwrap();
        delivery
    }

    /// The refusal must come from the real admission gate and must leave nothing on the wire:
    /// the delivery is stopped as revoked before any report started.
    fn assert_first_frame_refused(
        owner: &mut SessionRuntime,
        delivery: &crate::device::delivery::DeliveryTicket,
        reason: &str,
    ) {
        use crate::device::delivery::{DeliveryStatus, DeliveryStop};
        let refused = owner.begin_native_report(delivery, 0, 3).unwrap_err();
        assert!(refused.contains(reason), "{refused}");
        let record = owner.delivery_record(delivery).unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled, "{record:?}");
        assert_eq!(record.stop, Some(DeliveryStop::AdmissionRevoked));
        assert_eq!(record.started_at_ms, None, "no report may have started");
    }

    #[test]
    fn an_unchanged_fold_admits_the_queued_load_at_the_first_report() {
        use crate::device::delivery::DeliveryStatus;
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one", "two"]);
        deliver(&mut owner, connection, epoch, &create);
        let delivery = queued_global_eq_load(&mut owner, "one");
        owner.begin_native_report(&delivery, 0, 3).unwrap();
        assert_eq!(
            owner.delivery_record(&delivery).unwrap().status,
            DeliveryStatus::InFlight
        );
    }

    #[test]
    fn a_delete_the_owner_observes_before_the_first_report_refuses_the_queued_load() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one", "two"]);
        deliver(&mut owner, connection, epoch, &create);
        let delivery = queued_global_eq_load(&mut owner, "one");

        let delete = model_preset_payload(pa::message_action::Enum::Delete, &["one"]);
        deliver(&mut owner, connection, epoch, &delete);

        assert_first_frame_refused(&mut owner, &delivery, "not announced");
        // The entry that was not deleted is still admitted by a fresh plan.
        assert!(plan_global_eq_load(&owner, "two").is_ok());
    }

    #[test]
    fn a_duplicate_the_owner_observes_refuses_the_queued_load_as_ambiguous() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        let delivery = queued_global_eq_load(&mut owner, "one");

        // The same identity is announced again: both copies are kept and neither is admitted.
        deliver(&mut owner, connection, epoch, &create);

        assert_first_frame_refused(&mut owner, &delivery, "announced more than once");
        assert!(
            !plan_global_eq_load(&owner, "one")
                .unwrap_err()
                .starts_with("STATE_UNAVAILABLE"),
            "the duplicate refusal is not the unavailable class"
        );
    }

    #[test]
    fn a_fold_replaced_under_an_unchanged_fence_refuses_the_queued_load() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        let delivery = queued_global_eq_load(&mut owner, "one");

        // A decoder reset under an unchanged fence. The owner does not take this path on its own
        // (a new session installs a new lifecycle and epoch, so the fence changes with it); it is
        // done here directly so that the only thing left to refuse the write is the operand
        // re-check. This is the test that isolates the operand guard.
        let fence = owner.session_fence().unwrap();
        owner.state.reset();
        assert!(
            owner.session_fence().unwrap() == fence,
            "the fence must be untouched for this test to isolate the operand re-check"
        );

        assert_first_frame_refused(&mut owner, &delivery, "STATE_UNAVAILABLE");
    }

    #[test]
    fn a_new_session_between_planning_and_the_first_report_refuses_the_queued_load() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        let delivery = queued_global_eq_load(&mut owner, "one");

        // A new connection replaces the session; the owner stops the old delivery.
        //
        // This test guards the SESSION FENCE, not the operand re-check: the connection and epoch
        // check refuses first, so it would still pass if the operand guard were removed.
        // `a_fold_replaced_under_an_unchanged_fence_refuses_the_queued_load` is the test that
        // isolates the operand guard.
        owner.opened(5).unwrap();

        let refused = owner.begin_native_report(&delivery, 0, 6).unwrap_err();
        assert!(refused.contains("session changed"), "{refused}");
        let record = owner.delivery_record(&delivery).unwrap();
        assert_eq!(
            record.status,
            crate::device::delivery::DeliveryStatus::Cancelled,
            "{record:?}"
        );
        assert_eq!(
            record.stop,
            Some(crate::device::delivery::DeliveryStop::SessionChanged)
        );
        assert_eq!(record.started_at_ms, None, "no report may have started");
        // Nothing carries over into the new session's fold either.
        assert!(owner.state.snapshot().model_preset_catalog.is_none());
        assert!(plan_global_eq_load(&owner, "one")
            .unwrap_err()
            .starts_with("STATE_UNAVAILABLE:"));
    }

    #[test]
    fn a_counted_loss_before_the_first_report_refuses_the_queued_load() {
        let (mut owner, connection, epoch) = ingesting_owner();
        complete_any_frame(&mut owner, connection, epoch);
        let create = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        deliver(&mut owner, connection, epoch, &create);
        let method = "device.loadGlobalEqModelPreset";
        let params = serde_json::json!({
            "value": "one",
            "isFactory": false,
            "hash": crate::state::GLOBAL_EQ_MODEL_PRESET_HASH,
        });
        let plan =
            crate::device::request::plan_gateway_write_with_state(method, &params, &owner.state)
                .unwrap();
        let context = owner
            .context_for_write(method, &params, &plan.write)
            .unwrap();
        let operation = owner.plan_operation_in_context(&context, 1, 3_000).unwrap();
        assert!(owner.validate_operation(&operation, 1).is_ok());

        // The write is queued. The transport counts a lost fragment before the first report starts.
        let values = filler();
        let listed: Vec<&str> = values.iter().map(String::as_str).collect();
        for report in lost_delete_tail(&listed) {
            let _ = owner.push_native_report(connection, epoch, &report, 2);
        }
        let refused = owner.validate_operation(&operation, 2).unwrap_err();
        assert!(refused.contains("STATE_UNAVAILABLE"), "{refused}");
        assert!(refused.contains("inbound-loss"), "{refused}");
    }

    #[test]
    fn observe_state_stages_an_invalidated_publication_when_a_model_preset_payload_fails() {
        let (mut owner, connection) = live_owner();
        let ticket = owner.capture_observation(connection).unwrap();
        let payload = model_preset_payload(pa::message_action::Enum::Create, &["one"]);
        owner
            .observe_state(&ticket, crate::profile::MESSAGE_TYPE_MODEL_PRESET, &payload)
            .unwrap();
        owner.take_state_publications().unwrap();

        // A truncated length-delimited field is not a protobuf message.
        let failure = owner.observe_state(
            &ticket,
            crate::profile::MESSAGE_TYPE_MODEL_PRESET,
            &[0x1a, 0x7f, 0x01],
        );
        assert!(failure.is_err());
        let published = owner.take_state_publications().unwrap();
        assert_eq!(published.len(), 1);
        let catalog = published[0].states[0]
            .model_preset_catalog
            .as_ref()
            .unwrap();
        assert_eq!(catalog.state, "invalidated");
        assert_eq!(catalog.invalid_reason.as_deref(), Some("decode-failure"));
        assert!(!catalog.session.is_empty());
    }
}
