#![cfg(test)]

//! The native guard of a Global EQ preset recall against the session's retained fold.

use super::*;
use crate::proto::cortex_protobuf_v2 as pa;
use prost::Message;
use serde_json::json;

fn model_preset_push(action: pa::message_action::Enum, entries: &[(&str, bool, u32)]) -> Vec<u8> {
    pa::ModelPresetMessage {
        action: action as i32,
        presets: entries
            .iter()
            .map(|(value, is_factory, hash)| pa::ModelPreset {
                id: Some(pa::model_preset::Id::Id(pa::ModelPresetId {
                    value: Some(pa::model_preset_id::Value::Value((*value).into())),
                    is_factory: Some(pa::model_preset_id::IsFactory::IsFactory(*is_factory)),
                    hash: Some(pa::model_preset_id::Hash::Hash(*hash)),
                })),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
    .encode_to_vec()
}

fn global_eq_recall(value: &str, is_factory: bool, hash: u32) -> Value {
    json!({"value": value, "isFactory": is_factory, "hash": hash})
}

const GLOBAL_EQ_RECALL: &str = "device.loadGlobalEqModelPreset";

type StateRuntime = crate::device::state_runtime::DeviceStateRuntime;

fn push_model_presets(
    state: &mut StateRuntime,
    action: pa::message_action::Enum,
    entries: &[(&str, bool, u32)],
) {
    state
        .ingest(
            profile::MESSAGE_TYPE_MODEL_PRESET,
            &model_preset_push(action, entries),
        )
        .unwrap();
}

fn plan_recall(
    state: &StateRuntime,
    value: &str,
    is_factory: bool,
    hash: u32,
) -> Result<GatewayWritePlan, String> {
    plan_gateway_write_with_state(
        GLOBAL_EQ_RECALL,
        &global_eq_recall(value, is_factory, hash),
        state,
    )
}

#[test]
fn global_eq_recall_is_planned_only_for_an_announced_scope_identity() {
    use crate::state::GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE;
    let mut state = StateRuntime::new();
    push_model_presets(
        &mut state,
        pa::message_action::Enum::Update,
        &[
            ("flat", true, SCOPE),
            ("mine", false, SCOPE),
            ("amp", false, 77),
        ],
    );
    let plan = plan_recall(&state, "mine", false, SCOPE).unwrap();
    assert!(matches!(
        plan.write,
        PlannedWrite::HidOperation(DeviceOperation::LoadGlobalEqModelPreset { .. })
    ));
    // Not the scope hash, even though the device announced it.
    let refused = plan_recall(&state, "amp", false, 77).unwrap_err();
    assert!(
        refused.contains("is not the Global EQ preset family"),
        "{refused}"
    );
    assert!(!refused.starts_with("STATE_UNAVAILABLE"), "{refused}");
    // Announced with the other factory flag, and never announced at all.
    for (value, is_factory) in [("mine", true), ("flat", false), ("nobody", false)] {
        let refused = plan_recall(&state, value, is_factory, SCOPE).unwrap_err();
        assert!(refused.contains("was not announced"), "{refused}");
        assert!(!refused.starts_with("STATE_UNAVAILABLE"), "{refused}");
    }
    // The shape checks still come first.
    assert!(plan_gateway_write_with_state(
        GLOBAL_EQ_RECALL,
        &json!({"value": "mine", "isFactory": false}),
        &state
    )
    .is_err());
}

#[test]
fn global_eq_recall_is_unavailable_without_state_or_an_observed_list() {
    use crate::state::GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE;
    let params = global_eq_recall("mine", false, SCOPE);
    let stateless = plan_gateway_write(GLOBAL_EQ_RECALL, &params, None).unwrap_err();
    assert!(stateless.starts_with("STATE_UNAVAILABLE:"), "{stateless}");
    let state = StateRuntime::new();
    let empty = plan_recall(&state, "mine", false, SCOPE).unwrap_err();
    assert!(empty.starts_with("STATE_UNAVAILABLE:"), "{empty}");
    // A first message that adds nothing leaves the list unavailable.
    let mut state = StateRuntime::new();
    push_model_presets(&mut state, pa::message_action::Enum::Update, &[]);
    push_model_presets(&mut state, pa::message_action::Enum::Delete, &[]);
    let still = plan_recall(&state, "mine", false, SCOPE).unwrap_err();
    assert!(still.starts_with("STATE_UNAVAILABLE:"), "{still}");
}

#[test]
fn global_eq_recall_refuses_a_tuple_deleted_after_observation_and_one_from_an_old_session() {
    use crate::state::GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE;
    let mut state = StateRuntime::new();
    let entry = [("mine", false, SCOPE)];
    push_model_presets(&mut state, pa::message_action::Enum::Create, &entry);
    assert!(plan_recall(&state, "mine", false, SCOPE).is_ok());
    push_model_presets(&mut state, pa::message_action::Enum::Delete, &entry);
    assert!(plan_recall(&state, "mine", false, SCOPE)
        .unwrap_err()
        .contains("deleted"));
    push_model_presets(&mut state, pa::message_action::Enum::Create, &entry);
    assert!(plan_recall(&state, "mine", false, SCOPE).is_ok());
    // A new session starts with nothing announced.
    state.reset();
    assert!(plan_recall(&state, "mine", false, SCOPE)
        .unwrap_err()
        .starts_with("STATE_UNAVAILABLE:"));
}

#[test]
fn global_eq_recall_excludes_a_duplicate_announcement_from_admission() {
    use crate::state::GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE;
    let mut state = StateRuntime::new();
    push_model_presets(
        &mut state,
        pa::message_action::Enum::Create,
        &[
            ("twice", false, SCOPE),
            ("twice", false, SCOPE),
            ("once", false, SCOPE),
        ],
    );
    let refused = plan_recall(&state, "twice", false, SCOPE).unwrap_err();
    assert!(refused.contains("announced more than once"), "{refused}");
    assert!(!refused.starts_with("STATE_UNAVAILABLE"), "{refused}");
    assert!(plan_recall(&state, "once", false, SCOPE).is_ok());
}

#[test]
fn global_eq_recall_is_refused_from_a_list_that_outgrew_the_publication_limit() {
    use crate::state::{GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE, PUBLISHED_ENTRY_LIMIT};
    let ids: Vec<String> = (0..=PUBLISHED_ENTRY_LIMIT)
        .map(|n| format!("p{n}"))
        .collect();
    let entries: Vec<(&str, bool, u32)> =
        ids.iter().map(|id| (id.as_str(), false, SCOPE)).collect();
    let mut state = StateRuntime::new();
    push_model_presets(&mut state, pa::message_action::Enum::Create, &entries);
    // The list was never cut for publication: it is unusable, so even an entry that was
    // announced is refused, and the refusal is the typed unavailable one.
    for id in ["p0", "p255", "p256"] {
        let refused = plan_recall(&state, id, false, SCOPE).unwrap_err();
        assert!(refused.starts_with("STATE_UNAVAILABLE:"), "{refused}");
        assert!(refused.contains("publication-limit"), "{refused}");
    }
    let view = state.snapshot().model_preset_catalog.clone().unwrap();
    assert_eq!(view.state, "invalidated");
    assert_eq!(view.entries, None);
}

#[test]
fn a_planned_global_eq_recall_is_asked_again_at_native_admission() {
    use crate::device::operands::DispatchOperands;
    use crate::state::GLOBAL_EQ_MODEL_PRESET_HASH as SCOPE;
    let mut state = StateRuntime::new();
    let entries = [("mine", false, SCOPE), ("other", false, SCOPE)];
    push_model_presets(&mut state, pa::message_action::Enum::Create, &entries);
    let params = global_eq_recall("mine", false, SCOPE);
    let plan = plan_gateway_write_with_state(GLOBAL_EQ_RECALL, &params, &state).unwrap();
    let operands =
        DispatchOperands::capture(GLOBAL_EQ_RECALL, &params, &plan.write, &state).unwrap();
    assert!(operands.validate(&state).is_ok());

    // The owner observes a delete between planning and native admission.
    push_model_presets(&mut state, pa::message_action::Enum::Delete, &[entries[0]]);
    let refused = operands.validate(&state).unwrap_err();
    assert!(refused.contains("not announced"), "{refused}");

    // A fresh plan after the delete is refused too, and so is capturing without a plan.
    assert!(plan_recall(&state, "mine", false, SCOPE).is_err());
    assert!(DispatchOperands::capture(GLOBAL_EQ_RECALL, &params, &plan.write, &state).is_err());

    // An invalidation the owner sees before admission refuses the other entry as well.
    let other = global_eq_recall("other", false, SCOPE);
    let plan = plan_gateway_write_with_state(GLOBAL_EQ_RECALL, &other, &state).unwrap();
    let operands =
        DispatchOperands::capture(GLOBAL_EQ_RECALL, &other, &plan.write, &state).unwrap();
    state.seed_model_preset_loss(0);
    assert!(state.observe_model_preset_loss(1).is_some());
    let refused = operands.validate(&state).unwrap_err();
    assert!(refused.starts_with("STATE_UNAVAILABLE:"), "{refused}");
}
