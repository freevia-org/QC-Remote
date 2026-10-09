#![cfg(test)]

//! Catalog admission of a replacing block and of a pin, and the typed refusal of a session that
//! has no model catalog.
//!
//! The proof is the one `device.addBlock` already uses (`Source::ModelAvailability`): the model is
//! listed by the ModelRepo this session installed and is not hidden. It is captured when the write
//! is planned for the host and read again before the first USB report.

use super::*;
use crate::device::owner::SessionRuntime;
use crate::device::request::plan_gateway_write;
use crate::device::session::SessionDriver;
use crate::device::GatewaySnapshot;
use crate::proto::cortex_protobuf_v2 as pa;
use crate::state::GridBlock;
use prost::Message;

const ADD: &str = "device.addBlock";
const REPLACE: &str = "device.replaceBlock";
const PIN: &str = "device.setModelPinned";

/// What every model write sees when the session holds no model catalog.
const UNAVAILABLE: &str =
    "STATE_UNAVAILABLE: The Quad Cortex model catalog has not been synchronized in this session";

/// 7 and 42 are ordinary models, 9022 is a plugin model, 12000 is hidden, 99999 is not listed.
const LISTED: &[u8] = br#"<Models><Category name="Delay"><Model id="7" name="Echo"/><Model id="42" name="Other"/><Model id="9022" name="Plugin" plugin_id="13"/></Category><Category name="Utility"><Model id="12000" name="Default Cabsim" hidden="true"/></Category></Models>"#;

const ABSENT_99999: &str = "Model 99999 is absent from the synchronized QC catalog";
const HIDDEN_12000: &str = "Model 12000 is not a selectable QC model";

fn model_repo(xml: &[u8]) -> crate::state::ModelCatalog {
    let payload = pa::ModelRepoMessage {
        action: pa::message_action::Enum::Update as i32,
        model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
            xml.to_vec(),
        )),
        ..Default::default()
    }
    .encode_to_vec();
    crate::state::parse_model_repo(&payload).unwrap()
}

/// A ModelRepo update that carries no payload, so it installs a catalog without any model.
fn model_repo_without_models() -> crate::state::ModelCatalog {
    let payload = pa::ModelRepoMessage {
        action: pa::message_action::Enum::Update as i32,
        ..Default::default()
    }
    .encode_to_vec();
    crate::state::parse_model_repo(&payload).unwrap()
}

fn state_without_catalog() -> DeviceStateRuntime {
    let mut state = DeviceStateRuntime::new();
    state.snapshot_mut().has_preset = true;
    state
}

fn state_with(xml: &[u8]) -> DeviceStateRuntime {
    let mut state = state_without_catalog();
    state.install_model_catalog(model_repo(xml));
    state
}

fn add(model_id: u32) -> PlannedWrite {
    PlannedWrite::HidOperation(DeviceOperation::AddBlock {
        row: 0,
        column: 1,
        model_id,
    })
}

fn replace(model_id: u32) -> PlannedWrite {
    PlannedWrite::HidOperation(DeviceOperation::ReplaceBlock {
        row: 0,
        column: 1,
        model_id,
    })
}

fn pin(model_id: u32, pinned: bool) -> PlannedWrite {
    PlannedWrite::HidOperation(DeviceOperation::SetModelPinned { model_id, pinned })
}

/// The three writes that need a listed model, in a fixed order (add, replace, pin).
fn model_writes(model_id: u32) -> [(&'static str, PlannedWrite); 3] {
    [
        (ADD, add(model_id)),
        (REPLACE, replace(model_id)),
        (PIN, pin(model_id, true)),
    ]
}

fn capture(
    method: &str,
    write: &PlannedWrite,
    state: &DeviceStateRuntime,
) -> Result<DispatchOperands, String> {
    DispatchOperands::capture(method, &json!({}), write, state)
}

#[test]
fn replace_block_needs_catalog_presence_at_capture_and_again_at_dispatch() {
    let mut state = state_with(LISTED);
    let plan = capture(REPLACE, &replace(7), &state).unwrap();
    // A plugin model is admitted on catalog presence alone, as for addBlock.
    assert!(capture(REPLACE, &replace(9022), &state).is_ok());
    assert_eq!(
        capture(REPLACE, &replace(99999), &state).unwrap_err(),
        ABSENT_99999
    );
    assert_eq!(
        capture(REPLACE, &replace(12000), &state).unwrap_err(),
        HIDDEN_12000
    );
    assert!(plan.validate(&state).is_ok());

    // The check runs again before the first report: a model the catalog now hides is refused.
    state.install_model_catalog(model_repo(
        br#"<Models><Category name="Delay"><Model id="7" name="Echo" hidden="true"/></Category></Models>"#,
    ));
    assert_eq!(
        plan.validate(&state).unwrap_err(),
        "Model 7 is not a selectable QC model"
    );

    // So is one the catalog no longer lists.
    state.install_model_catalog(model_repo(
        br#"<Models><Category name="Delay"><Model id="42" name="Other"/></Category></Models>"#,
    ));
    assert_eq!(
        plan.validate(&state).unwrap_err(),
        "Model 7 is absent from the synchronized QC catalog"
    );
}

#[test]
fn add_block_admission_is_otherwise_unchanged() {
    let state = state_with(LISTED);
    assert!(capture(ADD, &add(7), &state).is_ok());
    assert!(capture(ADD, &add(9022), &state).is_ok());
    assert_eq!(capture(ADD, &add(99999), &state).unwrap_err(), ABSENT_99999);
    assert_eq!(capture(ADD, &add(12000), &state).unwrap_err(), HIDDEN_12000);
}

#[test]
fn absent_and_hidden_messages_are_unchanged_through_the_runtime() {
    let state = state_with(LISTED);
    assert!(state.assert_model_available(7).is_ok());
    assert_eq!(
        state.assert_model_available(99999).unwrap_err(),
        ABSENT_99999
    );
    assert_eq!(
        state.assert_model_available(12000).unwrap_err(),
        HIDDEN_12000
    );
    // Only a session with no catalog at all is typed.
    assert_eq!(
        state_without_catalog()
            .assert_model_available(7)
            .unwrap_err(),
        UNAVAILABLE
    );
    // list_models keeps its own plain wording for the same condition.
    assert_eq!(
        state_without_catalog().model_list().unwrap_err(),
        "The Quad Cortex model catalog has not been synchronized yet"
    );
}

#[test]
fn an_empty_catalog_answers_state_unavailable_instead_of_absent() {
    let mut state = state_without_catalog();
    let writes = model_writes(7);
    // Add, replace and pin must each be exercised.
    assert_eq!(writes.len(), 3);
    for (method, write) in writes {
        assert_eq!(
            capture(method, &write, &state).unwrap_err(),
            UNAVAILABLE,
            "{method}"
        );
    }

    // A ModelRepo that installed no model leaves the session in the same position.
    state.install_model_catalog(model_repo_without_models());
    let writes = model_writes(7);
    // Add, replace and pin must each be exercised.
    assert_eq!(writes.len(), 3);
    for (method, write) in writes {
        assert_eq!(
            capture(method, &write, &state).unwrap_err(),
            UNAVAILABLE,
            "{method}"
        );
    }

    // With a catalog installed, a miss is the plain sentence again, never the typed one.
    state.install_model_catalog(model_repo(LISTED));
    let writes = model_writes(99999);
    // Add, replace and pin must each be exercised.
    assert_eq!(writes.len(), 3);
    for (method, write) in writes {
        assert_eq!(
            capture(method, &write, &state).unwrap_err(),
            ABSENT_99999,
            "{method}"
        );
    }
    let writes = model_writes(12000);
    // Add, replace and pin must each be exercised.
    assert_eq!(writes.len(), 3);
    for (method, write) in writes {
        assert_eq!(
            capture(method, &write, &state).unwrap_err(),
            HIDDEN_12000,
            "{method}"
        );
    }
}

#[test]
fn a_catalog_that_goes_away_after_planning_is_refused_as_unavailable() {
    let mut state = state_with(LISTED);
    let plans: Vec<_> = model_writes(7)
        .into_iter()
        .map(|(method, write)| capture(method, &write, &state).unwrap())
        .collect();
    state.install_model_catalog(model_repo_without_models());
    assert_eq!(plans.len(), 3);
    for plan in &plans {
        assert_eq!(plan.validate_targets(&state).unwrap_err(), UNAVAILABLE);
    }
}

// Guard exercised, first half: the preset source (the reset also drops the current preset, which
// the placement plans bind), so the add and replace assertions would pass without the catalog guard.
// Only the pin plan, which has no preset source, and the second half reach the catalog guard. The
// isolating variant is `a_reset_decoder_with_the_snapshot_republished_is_refused_by_the_catalog_guard`.
#[test]
fn a_reset_session_admits_nothing_from_the_previous_catalog() {
    let mut state = state_with(LISTED);
    let plans: Vec<_> = model_writes(7)
        .into_iter()
        .map(|(method, write)| capture(method, &write, &state).unwrap())
        .collect();
    state.reset();
    // The placement plans lose their preset as well; the pin plan has only the catalog to lose.
    assert!(plans[0].validate_targets(&state).is_err());
    assert!(plans[1].validate_targets(&state).is_err());
    assert_eq!(plans[2].validate_targets(&state).unwrap_err(), UNAVAILABLE);

    // A write planned in the new session sees no catalog until one is installed in it.
    state.snapshot_mut().has_preset = true;
    let writes = model_writes(7);
    // Add, replace and pin must each be exercised.
    assert_eq!(writes.len(), 3);
    for (method, write) in writes {
        assert_eq!(
            capture(method, &write, &state).unwrap_err(),
            UNAVAILABLE,
            "{method}"
        );
    }
}

#[test]
fn pinning_needs_a_listed_model_but_unpinning_does_not() {
    // Unpin cleans up an entry that may already be stale, so no catalog is needed.
    let state = state_without_catalog();
    let unpin = capture(PIN, &pin(42, false), &state).unwrap();
    assert!(unpin.validate_targets(&state).is_ok());
    assert_eq!(
        capture(PIN, &pin(42, true), &state).unwrap_err(),
        UNAVAILABLE
    );

    let mut state = state_with(LISTED);
    assert!(capture(PIN, &pin(7, true), &state).is_ok());
    assert!(capture(PIN, &pin(9022, true), &state).is_ok());
    assert_eq!(
        capture(PIN, &pin(99999, true), &state).unwrap_err(),
        ABSENT_99999
    );
    // A hidden model cannot be shown in the browser, so it is not pinnable.
    assert_eq!(
        capture(PIN, &pin(12000, true), &state).unwrap_err(),
        HIDDEN_12000
    );
    for model_id in [7, 12000, 99999] {
        assert!(
            capture(PIN, &pin(model_id, false), &state).is_ok(),
            "unpinning {model_id}"
        );
    }

    // The unpin plan stays valid when the session's catalog is reset or replaced.
    let unpin = capture(PIN, &pin(99999, false), &state).unwrap();
    state.install_model_catalog(model_repo_without_models());
    assert!(unpin.validate_targets(&state).is_ok());
    state.reset();
    assert!(unpin.validate_targets(&state).is_ok());
}

#[test]
fn a_planned_replace_is_admitted_only_against_the_installed_catalog() {
    let snapshot = GatewaySnapshot {
        preset_name: "Live".into(),
        grid_layout_complete: true,
        has_preset: true,
        blocks: vec![GridBlock {
            id: "0:0".into(),
            model_id: Some(41),
            row: 0,
            column: 0,
            ..Default::default()
        }],
        ..GatewaySnapshot::default()
    };
    let params = |model_id: u32| {
        json!({"row": 0, "column": 0, "modelId": model_id, "expectedModelId": 41,
            "expectedPresetName": "Live"})
    };
    // Planning does not consult the catalog; admission does.
    let plan = plan_gateway_write(REPLACE, &params(42), Some(&snapshot)).unwrap();
    let unlisted = plan_gateway_write(REPLACE, &params(99999), Some(&snapshot)).unwrap();
    let mut state = DeviceStateRuntime::new();
    *state.snapshot_mut() = snapshot;

    assert_eq!(
        DispatchOperands::capture(REPLACE, &params(42), &plan.write, &state).unwrap_err(),
        UNAVAILABLE
    );
    state.install_model_catalog(model_repo(LISTED));
    let operands = DispatchOperands::capture(REPLACE, &params(42), &plan.write, &state).unwrap();
    assert!(operands.validate(&state).is_ok());
    assert_eq!(
        DispatchOperands::capture(REPLACE, &params(99999), &unlisted.write, &state).unwrap_err(),
        ABSENT_99999
    );
}

/// Both Neural Capture model IDs, listed and visible, as a unit's ModelRepo lists them.
const WITH_CAPTURES: &[u8] = br#"<Models><Category name="Capture"><Model id="14000" name="Neural Capture"/><Model id="14001" name="Neural Capture 2"/></Category></Models>"#;

fn pin_params(model_id: u32, pinned: bool) -> Value {
    json!({"modelId": model_id, "expectedPinned": !pinned, "pinned": pinned})
}

#[test]
fn pinning_a_neural_capture_model_is_refused_by_the_planner() {
    for model_id in [14000, 14001] {
        let refused = plan_gateway_write(PIN, &pin_params(model_id, true), None).unwrap_err();
        assert!(refused.contains("Neural Capture"), "{refused}");
    }
    // The neighbouring numeric ids and ordinary models still plan.
    for model_id in [13999, 14002, 42] {
        let plan = plan_gateway_write(PIN, &pin_params(model_id, true), None).unwrap();
        assert!(matches!(
            plan.write,
            PlannedWrite::HidOperation(DeviceOperation::SetModelPinned { pinned: true, .. })
        ));
    }
}

#[test]
fn unpinning_any_model_id_plans_including_the_neural_capture_ids() {
    // Undoing an existing pin must not acquire new proof: no id is refused when unpinning.
    for model_id in [14000, 14001, 13999, 14002, 42] {
        let plan = plan_gateway_write(PIN, &pin_params(model_id, false), None)
            .unwrap_or_else(|error| panic!("unpin {model_id}: {error}"));
        assert!(matches!(
            plan.write,
            PlannedWrite::HidOperation(DeviceOperation::SetModelPinned { pinned: false, .. })
        ));
    }
}

#[test]
fn an_unpin_of_a_neural_capture_id_is_admitted_with_any_catalog_or_none() {
    for model_id in [14000, 14001] {
        let params = pin_params(model_id, false);
        let plan = plan_gateway_write(PIN, &params, None).unwrap();
        let mut empty = state_without_catalog();
        empty.install_model_catalog(model_repo_without_models());
        let states = [
            state_without_catalog(),
            empty,
            state_with(LISTED),
            state_with(WITH_CAPTURES),
        ];
        // Missing, empty, ordinary and capture-containing catalogs.
        assert_eq!(states.len(), 4);
        for state in &states {
            let operands = DispatchOperands::capture(PIN, &params, &plan.write, state).unwrap();
            assert!(operands.validate_targets(state).is_ok());
        }
        // The plan also survives its catalog being reset or replaced after planning.
        let mut state = state_with(WITH_CAPTURES);
        let operands = DispatchOperands::capture(PIN, &params, &plan.write, &state).unwrap();
        state.install_model_catalog(model_repo_without_models());
        assert!(operands.validate_targets(&state).is_ok());
        state.reset();
        assert!(operands.validate_targets(&state).is_ok());
    }
}

#[test]
fn an_unpin_never_reads_the_catalog() {
    // An unpin plan carries no catalog source at all, so nothing can call assert_model_available
    // for it, at capture or before the first frame; a pin carries exactly that one source.
    for model_id in [7, 12000, 14000, 99999] {
        let mut sources = Vec::new();
        operation_sources(
            &DeviceOperation::SetModelPinned {
                model_id,
                pinned: false,
            },
            &mut sources,
        );
        assert!(sources.is_empty(), "unpin {model_id}: {sources:?}");
    }
    let mut sources = Vec::new();
    operation_sources(
        &DeviceOperation::SetModelPinned {
            model_id: 7,
            pinned: true,
        },
        &mut sources,
    );
    assert_eq!(sources, vec![Source::ModelAvailability { model_id: 7 }]);
}

#[test]
fn pin_true_of_a_hidden_model_is_refused_with_the_existing_hidden_text() {
    let state = state_with(LISTED);
    assert_eq!(
        capture(PIN, &pin(12000, true), &state).unwrap_err(),
        "Model 12000 is not a selectable QC model"
    );
}

#[test]
fn pin_true_of_an_absent_id_is_refused_with_the_existing_absent_text() {
    let state = state_with(LISTED);
    assert_eq!(
        capture(PIN, &pin(99999, true), &state).unwrap_err(),
        "Model 99999 is absent from the synchronized QC catalog"
    );
}

#[test]
fn pin_true_with_a_missing_or_empty_catalog_is_state_unavailable() {
    let mut state = state_without_catalog();
    assert_eq!(
        capture(PIN, &pin(7, true), &state).unwrap_err(),
        UNAVAILABLE
    );
    state.install_model_catalog(model_repo_without_models());
    assert_eq!(
        capture(PIN, &pin(7, true), &state).unwrap_err(),
        UNAVAILABLE
    );
}

#[test]
fn replace_block_with_a_missing_or_empty_catalog_is_state_unavailable() {
    let mut state = state_without_catalog();
    assert_eq!(
        capture(REPLACE, &replace(7), &state).unwrap_err(),
        UNAVAILABLE
    );
    state.install_model_catalog(model_repo_without_models());
    assert_eq!(
        capture(REPLACE, &replace(7), &state).unwrap_err(),
        UNAVAILABLE
    );
}

/// A live owner (an active lifecycle, no USB) whose state holds a current preset and the catalog
/// `LISTED`.
fn live_owner() -> (SessionRuntime, u64) {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner.lifecycle = Some(SessionDriver::start(7, "session".into(), 0));
    owner.state.snapshot_mut().has_preset = true;
    owner.state.install_model_catalog(model_repo(LISTED));
    (owner, connection)
}

#[test]
fn a_replace_is_refused_before_its_first_frame_when_the_catalog_goes_away() {
    let (mut owner, _connection) = live_owner();
    let context = owner
        .context_for_write(REPLACE, &json!({}), &replace(7))
        .unwrap();
    let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
    let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();

    // The session is intact; only its model catalog is gone.
    owner
        .state
        .install_model_catalog(model_repo_without_models());
    let refused = owner.begin_native_report(&delivery, 0, 1).unwrap_err();
    assert_eq!(refused, UNAVAILABLE);
    // The refused delivery cannot be started again.
    assert!(owner.begin_native_report(&delivery, 0, 2).is_err());
}

// Guard exercised: the SESSION FENCE and the delivery journal. `reset_state` drops the lifecycle and
// the journal, so this would still pass without the catalog guard; the variants that isolate the
// catalog guard are `a_catalog_replaced_under_an_unchanged_fence_is_refused_by_the_catalog_guard`
// and `a_decoder_reset_under_an_unchanged_fence_is_refused_by_the_catalog_guard` at the end of the file.
#[test]
fn a_replace_is_refused_before_its_first_frame_when_the_session_is_reset() {
    let (mut owner, _connection) = live_owner();
    let context = owner
        .context_for_write(REPLACE, &json!({}), &replace(7))
        .unwrap();
    let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
    let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();

    owner.reset_state();
    assert!(owner.begin_native_report(&delivery, 0, 1).is_err());
    // A write planned after the reset has no session to be planned in.
    assert!(owner
        .context_for_write(REPLACE, &json!({}), &replace(7))
        .is_err());
}

#[test]
fn a_pin_is_refused_before_its_first_frame_when_the_catalog_goes_away() {
    let (mut owner, connection) = live_owner();
    let params = json!({"modelId": 7, "expectedPinned": false, "pinned": true});
    let plan = plan_gateway_write(PIN, &params, None).unwrap();
    let context = owner.context_for_write(PIN, &params, &plan.write).unwrap();
    let publication = pa::PinnedModelsMessage {
        action: pa::message_action::Enum::Update as i32,
        models: vec![8],
        ..Default::default()
    }
    .encode_to_vec();
    let ticket = owner
        .capture_observation_with_ingress(connection, None)
        .unwrap();
    owner
        .observe_state(
            &ticket,
            crate::profile::MESSAGE_TYPE_PINNED_MODELS,
            &publication,
        )
        .unwrap()
        .unwrap();
    let (bound, _) = owner
        .bind_write_preflight(&context, PIN, &params, &json!({"models": [8]}), 0)
        .unwrap();
    let operation = owner.plan_operation_in_context(&bound, 0, 100).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
    let delivery = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();

    owner
        .state
        .install_model_catalog(model_repo_without_models());
    let refused = owner.begin_native_report(&delivery, 0, 1).unwrap_err();
    assert_eq!(refused, UNAVAILABLE);
}

// The tests below isolate the catalog guard (`Source::ModelAvailability`). Each one changes only the
// retained catalog and asserts that the session fence is unchanged, so the fence cannot be what
// refuses the write; the refusal text is the catalog guard's own.

/// Queues a replace of model 7 on `owner` up to, and not including, its first report.
fn queued_replace(owner: &mut SessionRuntime) -> crate::device::delivery::DeliveryTicket {
    let context = owner
        .context_for_write(REPLACE, &json!({}), &replace(7))
        .unwrap();
    let operation = owner.plan_operation_in_context(&context, 0, 100).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
    owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap()
}

/// The first report is refused with exactly `refusal`, and the journal shows that none started.
fn assert_refused_before_any_report(
    owner: &mut SessionRuntime,
    delivery: &crate::device::delivery::DeliveryTicket,
    refusal: &str,
) {
    use crate::device::delivery::{DeliveryStatus, DeliveryStop};
    let refused = owner.begin_native_report(delivery, 0, 1).unwrap_err();
    assert_eq!(refused, refusal);
    let record = owner.delivery_record(delivery).unwrap();
    assert_eq!(record.status, DeliveryStatus::Cancelled, "{record:?}");
    assert_eq!(record.stop, Some(DeliveryStop::AdmissionRevoked));
    assert_eq!(record.started_at_ms, None, "no report may have started");
}

#[test]
fn an_unchanged_catalog_admits_the_queued_replace_at_the_first_report() {
    for reset_decoder in [false, true] {
        let (mut owner, _connection) = live_owner();
        let delivery = queued_replace(&mut owner);
        if reset_decoder {
            let retained = owner.state.snapshot().clone();
            owner.state.reset();
            *owner.state.snapshot_mut() = retained;
            owner.state.install_model_catalog(model_repo(LISTED));
        }
        owner.begin_native_report(&delivery, 0, 1).unwrap();
        assert_eq!(
            owner.delivery_record(&delivery).unwrap().started_at_ms,
            Some(1)
        );
    }
}

#[test]
fn a_catalog_replaced_under_an_unchanged_fence_is_refused_by_the_catalog_guard() {
    // Guard exercised: the operand re-check only. Removing the ModelAvailability arm makes every
    // case below start its report, so none of them can pass without the guard.
    let cases = [
        (model_repo_without_models(), UNAVAILABLE),
        (
            model_repo(
                br#"<Models><Category name="Delay"><Model id="7" name="Echo" hidden="true"/></Category></Models>"#,
            ),
            "Model 7 is not a selectable QC model",
        ),
        (
            model_repo(
                br#"<Models><Category name="Delay"><Model id="42" name="Other"/></Category></Models>"#,
            ),
            "Model 7 is absent from the synchronized QC catalog",
        ),
    ];
    for (catalog, refusal) in cases {
        let (mut owner, _connection) = live_owner();
        let delivery = queued_replace(&mut owner);
        let fence = owner.session_fence().unwrap();
        owner.state.install_model_catalog(catalog);
        assert!(
            owner.session_fence().unwrap() == fence,
            "the fence must be untouched for this test to isolate the catalog guard"
        );
        assert_refused_before_any_report(&mut owner, &delivery, refusal);
    }
}

#[test]
fn a_decoder_reset_under_an_unchanged_fence_is_refused_by_the_catalog_guard() {
    // Guard exercised: the operand re-check only. The decoder is reset directly (the owner does not
    // take this path on its own: a new session installs a new lifecycle and epoch), and the retained
    // snapshot is republished unchanged, so the preset and block sources compare equal and the
    // catalog, which the reset emptied, is the only thing left to refuse the write.
    let (mut owner, _connection) = live_owner();
    let delivery = queued_replace(&mut owner);
    let fence = owner.session_fence().unwrap();
    let retained = owner.state.snapshot().clone();
    owner.state.reset();
    *owner.state.snapshot_mut() = retained;
    assert!(
        owner.session_fence().unwrap() == fence,
        "the fence must be untouched for this test to isolate the catalog guard"
    );
    assert_eq!(owner.state.model_count(), 0);
    assert_refused_before_any_report(&mut owner, &delivery, UNAVAILABLE);
}

#[test]
fn a_reset_decoder_with_the_snapshot_republished_is_refused_by_the_catalog_guard() {
    // Guard exercised: the operand re-check only, for add, replace and pin. The snapshot is put
    // back as it was, so the preset and block sources agree and only the emptied catalog can refuse.
    let mut state = state_with(LISTED);
    let plans: Vec<_> = model_writes(7)
        .into_iter()
        .map(|(method, write)| capture(method, &write, &state).unwrap())
        .collect();
    let retained = state.snapshot().clone();
    state.reset();
    *state.snapshot_mut() = retained;
    assert_eq!(plans.len(), 3);
    for plan in &plans {
        assert_eq!(plan.validate_targets(&state).unwrap_err(), UNAVAILABLE);
    }
}
