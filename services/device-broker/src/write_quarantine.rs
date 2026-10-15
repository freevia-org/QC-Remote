//! What a write quarantine is, what it still lets through, and how it clears.
//!
//! Host policy (not a Cortex Control frame): the quarantine and its scopes exist to protect
//! unsaved work from a write whose outcome is genuinely unknown. Cortex Control has nothing like
//! it. It sends no frame and reads nothing: there are no settle reads and no resync reads.
//!
//! A quarantine is set only when a mutation reached the wire and the host cannot say whether it
//! was delivered: a lost USB link, a native delivery that timed out, a session that restarted
//! under the write, or a persistent File workflow whose frames were attempted but not all
//! delivered. An expired confirmation window is NOT such a case: the write was delivered, the
//! unit simply pushed no evidence, and the caller gets `accepted_unverified` with no quarantine.
//! It protects one thing: the host must not plan or send further writes against a device state it
//! cannot vouch for.
//!
//! # What is gated (the reasoning, written down)
//!
//! Every later write depends on the unverified state only if it reads or
//! replaces the live preset buffer, or is verified against it. The quarantine
//! therefore records a *scope*:
//!
//! * [`QuarantineScope::PresetGrid`] - the ambiguous write was an edit of the
//!   live preset (grid, scene, parameter, history, preset recall). The device
//!   state in doubt is the live preset. Still gated: every other edit of it
//!   (their plans are built from the cached grid and are verified against it),
//!   `save_preset_as` (it would persist the unverified buffer),
//!   `rename_current_preset` / `copy_preset` (host policy: each sends a single
//!   File frame that does not persist the buffer, but both stay gated rather
//!   than widen what an unresolved write may do), preset recall / bank navigation / reload
//!   (a late-arriving write could still land in the newly selected preset, and
//!   they discard the buffer under dispute), scene and bypass selection, and
//!   any library write that names the active preset's setlist (a move or delete
//!   there can shift or remove the very slot the buffer belongs to).
//!   Not gated, because none of them reads, plans from, or verifies against the
//!   live preset buffer: device and global settings (tuner, I/O, MIDI thru,
//!   global EQ, mode cycle, global tempo, general settings, scene-bypass
//!   behaviour, master-volume assignment, global bypass, device name),
//!   favourites and pinned models, the looper, setlist creation / duplication,
//!   and moving or deleting a preset in a setlist other than the active one.
//! * [`QuarantineScope::Device`] - the ambiguous write was anything else
//!   (settings, library, backup, a lost USB link). Nothing narrower can be
//!   shown to be safe, so every mutation stays gated, as before.
//!
//! # How it clears
//!
//! Only by a new device session (a reconnect, a QC-initiated state rebuild) or
//! `reset_device_session`; both scopes clear the same way. A record from an
//! earlier session epoch is never a gate ([`is_stale`]). The broker takes no
//! reads to clear it: the earlier automatic resync (two identical complete type-15
//! READs after 3 s) is removed because Cortex Control reads nothing back and the
//! forced read could itself time out against a unit that had applied the write.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QuarantineScope {
    PresetGrid,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteQuarantine {
    pub reason: String,
    /// The RPC that was transmitted and could not be settled.
    pub method: String,
    pub scope: QuarantineScope,
    pub since_unix_ms: u64,
    pub session_epoch: u64,
    /// Plain-language statement of exactly how this quarantine clears.
    pub clears_by: String,
}

impl WriteQuarantine {
    pub fn new(
        method: &str,
        reason: impl Into<String>,
        since_unix_ms: u64,
        session_epoch: u64,
    ) -> Self {
        let scope = scope_for_method(method);
        Self {
            reason: reason.into(),
            method: method.into(),
            scope,
            since_unix_ms,
            session_epoch,
            clears_by: clears_by(scope).into(),
        }
    }
}

fn clears_by(_scope: QuarantineScope) -> &'static str {
    "only by a new device session (a reconnect after a disconnect or a QC-initiated rebuild) or \
     reset_device_session; the broker sends no read to clear it"
}

/// Methods whose ambiguity is about the live preset.
pub fn is_live_preset_method(method: &str) -> bool {
    matches!(
        method,
        "device.moveBlock"
            | "device.addBlock"
            | "device.removeBlock"
            | "device.setBlockFootswitch"
            | "device.setStompMomentary"
            | "device.setStompLabel"
            | "device.setMidiOut"
            | "device.setPresetLoadMidiOut"
            | "device.setExpressionBypass"
            | "device.setChainInput"
            | "device.setChainOutput"
            | "device.setChainSplit"
            | "device.setSplitMute"
            | "device.selectScene"
            | "device.copyScene"
            | "device.setSceneLabel"
            | "device.setSceneColor"
            | "device.toggleBypass"
            | "device.setParameter"
            | "device.setLaneControlParameter"
            | "device.setLaneControlSceneMode"
            | "device.setLaneControlExpression"
            | "device.setParameterSceneMode"
            | "device.setParameterExpression"
            | "device.undo"
            | "device.redo"
            | "device.loadCapture"
            | "device.loadIr"
            | "device.recallPreset"
            | "device.navigateBank"
            | "device.reloadPreset"
    )
}

pub fn scope_for_method(method: &str) -> QuarantineScope {
    if is_live_preset_method(method) {
        QuarantineScope::PresetGrid
    } else {
        QuarantineScope::Device
    }
}

/// Mutations that read, plan from, persist, or are verified against none of
/// the live preset buffer. See the module documentation for the reasoning.
fn is_independent_of_live_preset(
    method: &str,
    params: &Value,
    active_setlist_key: Option<&str>,
) -> bool {
    if matches!(
        method,
        "device.setTunerInput"
            | "device.setTunerMute"
            | "device.setTunerReference"
            | "device.setTunerMeter"
            | "device.setInputPort"
            | "device.setOutputPort"
            | "device.setUsbPort"
            | "device.setMidiThru"
            | "device.setOutputPairing"
            | "device.setGlobalEqBypassed"
            | "device.setGlobalEqBand"
            | "device.setGlobalEqOutput"
            | "device.loadGlobalEqModelPreset"
            | "device.setModeCycle"
            | "device.setTempoMetronome"
            | "device.setTempoMode"
            | "device.setGlobalTempo"
            | "device.setGeneralInteger"
            | "device.setGeneralToggle"
            | "device.setSceneBypassBehavior"
            | "device.setMasterVolumeAssignment"
            | "device.setGlobalBypass"
            | "device.setDeviceName"
            | "device.setFavorite"
            | "device.setModelPinned"
            | "device.controlLooper"
            | "device.createSetlist"
            | "device.duplicateSetlist"
    ) {
        return true;
    }
    if matches!(method, "device.deletePreset" | "device.movePreset") {
        // Only a setlist other than the active preset's can be shown not to
        // shift or remove the slot the live buffer belongs to.
        let target = params.get("setlistKey").and_then(Value::as_str);
        return match (target, active_setlist_key) {
            (Some(target), Some(active)) => {
                target.trim_end_matches('/') != active.trim_end_matches('/')
            }
            _ => false,
        };
    }
    false
}

/// Does this quarantine refuse `method`?
pub fn gates(
    quarantine: &WriteQuarantine,
    method: &str,
    params: &Value,
    active_setlist_key: Option<&str>,
) -> bool {
    match quarantine.scope {
        QuarantineScope::Device => true,
        QuarantineScope::PresetGrid => {
            !is_independent_of_live_preset(method, params, active_setlist_key)
        }
    }
}

/// Whether `quarantine` belongs to an earlier session epoch than `current_session_epoch`. A
/// stale record was already cleared by the new session; it is never a gate.
pub fn is_stale(quarantine: &WriteQuarantine, current_session_epoch: u64) -> bool {
    quarantine.session_epoch != current_session_epoch
}

/// The caller-facing refusal. Begins with the stable prefix the hardware
/// conformance tooling matches.
pub fn refusal_message(quarantine: &WriteQuarantine, now_unix_ms: u64) -> String {
    let age_s = now_unix_ms.saturating_sub(quarantine.since_unix_ms) / 1000;
    let affected = match quarantine.scope {
        QuarantineScope::PresetGrid => {
            "edits to the live preset, saves, and preset recalls are paused; device settings, \
             favourites, the looper and library operations on other setlists are still accepted"
        }
        QuarantineScope::Device => "all device writes are paused",
    };
    format!(
        "Device writes are quarantined after an ambiguous mutation: {reason}. \
         {method} was transmitted {age_s} s ago and its delivery could not be established; \
         {affected}. It clears {clears}. Next step: inspect the current device state; do not \
         repeat the ambiguous mutation blindly. Establish a new device session or call \
         reset_device_session before retrying affected writes with fresh expected-state guards.",
        reason = quarantine.reason,
        method = quarantine.method,
        clears = quarantine.clears_by,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grid_quarantine() -> WriteQuarantine {
        WriteQuarantine::new("device.moveBlock", "move could not be settled", 1_000, 4)
    }

    #[test]
    fn live_preset_edits_scope_to_the_grid_and_everything_else_to_the_device() {
        assert_eq!(
            scope_for_method("device.moveBlock"),
            QuarantineScope::PresetGrid
        );
        assert_eq!(
            scope_for_method("device.recallPreset"),
            QuarantineScope::PresetGrid
        );
        for method in [
            "device.setGeneralInteger",
            "device.savePresetAs",
            "device.createBackup",
            "device.deletePreset",
        ] {
            assert_eq!(
                scope_for_method(method),
                QuarantineScope::Device,
                "{method}"
            );
        }
    }

    #[test]
    fn a_grid_quarantine_still_gates_everything_that_touches_the_live_preset() {
        let quarantine = grid_quarantine();
        for method in [
            "device.removeBlock",
            "device.moveBlock",
            "device.setParameter",
            "device.savePresetAs",
            "device.renameCurrentPreset",
            "device.copyPreset",
            "device.recallPreset",
            "device.navigateBank",
            "device.selectScene",
            "device.toggleBypass",
            "device.undo",
            "device.setTempo",
            "device.pressFootswitch",
            "device.createBackup",
            "device.deleteSetlist",
        ] {
            assert!(
                gates(&quarantine, method, &json!({}), Some("/a")),
                "{method} must stay gated"
            );
        }
    }

    #[test]
    fn a_grid_quarantine_lets_independent_writes_through() {
        let quarantine = grid_quarantine();
        for method in [
            "device.setGeneralInteger",
            "device.setInputPort",
            "device.setGlobalEqBand",
            "device.setFavorite",
            "device.controlLooper",
            "device.createSetlist",
            "device.duplicateSetlist",
            "device.setDeviceName",
        ] {
            assert!(
                !gates(&quarantine, method, &json!({}), Some("/a")),
                "{method} does not depend on the live preset"
            );
        }
    }

    #[test]
    fn library_writes_pass_only_when_they_cannot_touch_the_active_slot() {
        let quarantine = grid_quarantine();
        let other = json!({ "setlistKey": "/b/" });
        let same = json!({ "setlistKey": "/a" });
        for method in ["device.deletePreset", "device.movePreset"] {
            assert!(!gates(&quarantine, method, &other, Some("/a/")), "{method}");
            assert!(gates(&quarantine, method, &same, Some("/a/")), "{method}");
            // Unknown active setlist or missing key: cannot prove it is safe.
            assert!(gates(&quarantine, method, &other, None), "{method}");
            assert!(
                gates(&quarantine, method, &json!({}), Some("/a")),
                "{method}"
            );
        }
    }

    #[test]
    fn a_device_quarantine_gates_everything_as_before() {
        let quarantine = WriteQuarantine::new("device.setGeneralInteger", "unconfirmed", 0, 1);
        assert!(gates(&quarantine, "device.setFavorite", &json!({}), None));
        assert!(gates(&quarantine, "device.moveBlock", &json!({}), None));
    }

    #[test]
    fn a_record_from_an_earlier_session_epoch_is_stale() {
        let quarantine = grid_quarantine();
        assert!(!is_stale(&quarantine, 4));
        assert!(is_stale(&quarantine, 5));
        let device = WriteQuarantine::new("device.setGeneralInteger", "x", 0, 4);
        assert!(!is_stale(&device, 4));
    }

    #[test]
    fn both_scopes_clear_only_by_a_new_session_and_never_by_a_read() {
        for quarantine in [
            grid_quarantine(),
            WriteQuarantine::new("device.setGeneralInteger", "x", 0, 4),
        ] {
            assert!(quarantine.clears_by.contains("new device session"));
            assert!(quarantine.clears_by.contains("reset_device_session"));
            assert!(quarantine.clears_by.contains("no read"));
            assert!(!quarantine.clears_by.to_lowercase().contains("re-reads"));
        }
    }

    #[test]
    fn the_refusal_names_the_reason_age_scope_exit_and_next_step() {
        let quarantine = grid_quarantine();
        let message = refusal_message(&quarantine, 11_000);
        assert!(
            message.starts_with("Device writes are quarantined after an ambiguous mutation: "),
            "{message}"
        );
        for expected in [
            "move could not be settled",
            "device.moveBlock was transmitted 10 s ago",
            "device settings",
            "new device session",
            "reset_device_session",
            "Next step",
        ] {
            assert!(
                message.contains(expected),
                "{expected} missing from {message}"
            );
        }
        assert!(!message.contains("resync"), "{message}");
        assert!(
            message.contains("before retrying affected writes"),
            "{message}"
        );
        assert!(
            !message.contains("then retry"),
            "reading alone does not clear the quarantine: {message}"
        );
    }
}
