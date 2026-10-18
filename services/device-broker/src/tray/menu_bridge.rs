//! Authenticated native menu clients render the same presentation and send
//! actions to the same state thread as the Windows broker icon.
use super::{model, state::Shared};
use serde_json::{json, Value};
use std::sync::{mpsc::Sender, Arc, OnceLock};

static BRIDGE: OnceLock<(Arc<Shared>, Sender<model::MenuAction>)> = OnceLock::new();

pub(super) fn install(shared: Arc<Shared>, actions: Sender<model::MenuAction>) {
    let _ = BRIDGE.set((shared, actions));
}

fn presentation(shared: &Shared) -> model::Presentation {
    let mut snapshot = shared
        .snapshot
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    snapshot.stale = shared
        .refreshed
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .elapsed()
        > model::STALE_AFTER;
    model::presentation(&snapshot)
}

pub(crate) fn read() -> Result<Value, String> {
    let (shared, _) = BRIDGE.get().ok_or("The broker menu is not ready")?;
    let menu = presentation(shared);
    let confirmations: Vec<_> = model::commands(&menu.rows)
        .into_iter()
        .filter_map(|action| {
            model::confirmation(&action)
                .map(|question| json!({"action": action, "question": question}))
        })
        .collect();
    let snapshot = shared.snapshot.lock().unwrap_or_else(|p| p.into_inner());
    Ok(json!({"presentation": menu, "confirmations": confirmations,
        "tone": format!("{:?}", model::icon_tone(&snapshot)).to_lowercase(),
        "wanted": shared.wanted.load(std::sync::atomic::Ordering::Acquire)}))
}

fn enabled(rows: &[model::MenuRow], action: &model::MenuAction) -> bool {
    rows.iter().any(|row| match row {
        model::MenuRow::Action {
            enabled,
            action: candidate,
            ..
        } => *enabled && candidate == action,
        model::MenuRow::Choices { choices, .. } => {
            choices.iter().any(|(_, _, candidate)| candidate == action)
        }
        model::MenuRow::Submenu { rows, .. } => enabled(rows, action),
        _ => false,
    })
}

pub(crate) fn dispatch(params: Value) -> Result<Value, String> {
    let (shared, sender) = BRIDGE.get().ok_or("The broker menu is not ready")?;
    let action: model::MenuAction = serde_json::from_value(params["action"].clone())
        .map_err(|error| format!("Invalid broker menu action: {error}"))?;
    validate(
        &presentation(shared).rows,
        &action,
        params["confirmed"] == true,
    )?;
    sender
        .send(action)
        .map_err(|_| "The broker menu has stopped".to_owned())?;
    Ok(json!({"accepted": true}))
}

fn validate(
    rows: &[model::MenuRow],
    action: &model::MenuAction,
    confirmed: bool,
) -> Result<(), String> {
    if !enabled(rows, action) {
        return Err("This menu action is no longer available; reopen the menu".into());
    }
    if model::confirmation(action).is_some() && !confirmed {
        return Err("Confirm this remote unlink before sending it".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_unlink_requires_confirmation_and_the_current_session() {
        let action = model::MenuAction::UnlinkRemoteSession {
            session_id: "current".into(),
            app: "Client".into(),
        };
        let rows = vec![model::MenuRow::Action {
            label: "Unlink".into(),
            enabled: true,
            action: action.clone(),
        }];
        assert!(validate(&rows, &action, false).is_err());
        assert!(validate(&rows, &action, true).is_ok());
        assert!(validate(&[], &action, true).is_err());
        let wire = serde_json::to_value(&rows).unwrap();
        let decoded: Vec<model::MenuRow> = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, rows);
    }
    #[test]
    fn actions_must_still_be_enabled_in_the_current_menu() {
        let rows = vec![model::MenuRow::Submenu {
            label: "Client".into(),
            rows: vec![
                model::MenuRow::Action {
                    label: "Unlink".into(),
                    enabled: false,
                    action: model::MenuAction::EndLocalSession("old".into()),
                },
                model::MenuRow::Choices {
                    label: "Idle".into(),
                    choices: vec![("Never".into(), true, model::MenuAction::SetIdle(None))],
                },
            ],
        }];
        assert!(!enabled(
            &rows,
            &model::MenuAction::EndLocalSession("old".into())
        ));
        assert!(!enabled(
            &rows,
            &model::MenuAction::EndLocalSession("new".into())
        ));
        assert!(enabled(&rows, &model::MenuAction::SetIdle(None)));
    }
}
