// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultKind {
    Object,
    PresetSnapshot,
    DeviceActionResult,
}
pub fn result_kind(method: &str) -> Option<ResultKind> {
    match method {
        "system.status" => Some(ResultKind::Object),
        "device.reconnect" => Some(ResultKind::Object),
        "device.resetSession" => Some(ResultKind::Object),
        "device.disconnect" => Some(ResultKind::Object),
        "device.snapshot" => Some(ResultKind::PresetSnapshot),
        "device.currentPreset" => Some(ResultKind::PresetSnapshot),
        "device.stateEvents" => Some(ResultKind::Object),
        "device.tempoClock" => Some(ResultKind::Object),
        "device.listModels" => Some(ResultKind::Object),
        "device.identity" => Some(ResultKind::Object),
        "device.setDeviceName" => Some(ResultKind::DeviceActionResult),
        "device.undo" => Some(ResultKind::DeviceActionResult),
        "device.redo" => Some(ResultKind::DeviceActionResult),
        "device.inhibitedModules" => Some(ResultKind::Object),
        "device.tunerSettings" => Some(ResultKind::Object),
        "device.setTunerInput" => Some(ResultKind::DeviceActionResult),
        "device.setTunerMute" => Some(ResultKind::DeviceActionResult),
        "device.setTunerReference" => Some(ResultKind::DeviceActionResult),
        "device.setTunerMeter" => Some(ResultKind::DeviceActionResult),
        "device.generalSettings" => Some(ResultKind::Object),
        "device.ioSettings" => Some(ResultKind::Object),
        "device.setInputPort" => Some(ResultKind::DeviceActionResult),
        "device.setOutputPort" => Some(ResultKind::DeviceActionResult),
        "device.setUsbPort" => Some(ResultKind::DeviceActionResult),
        "device.setMidiThru" => Some(ResultKind::DeviceActionResult),
        "device.setOutputPairing" => Some(ResultKind::DeviceActionResult),
        "device.globalEq" => Some(ResultKind::Object),
        "device.setGlobalEqBypassed" => Some(ResultKind::DeviceActionResult),
        "device.setGlobalEqBand" => Some(ResultKind::DeviceActionResult),
        "device.setGlobalEqOutput" => Some(ResultKind::DeviceActionResult),
        "device.loadGlobalEqModelPreset" => Some(ResultKind::DeviceActionResult),
        "device.modeCycle" => Some(ResultKind::Object),
        "device.setModeCycle" => Some(ResultKind::DeviceActionResult),
        "device.globalTempoSettings" => Some(ResultKind::Object),
        "device.setTempoMetronome" => Some(ResultKind::DeviceActionResult),
        "device.setTempoMode" => Some(ResultKind::DeviceActionResult),
        "device.setGlobalTempo" => Some(ResultKind::DeviceActionResult),
        "device.looperStatus" => Some(ResultKind::Object),
        "device.controlLooper" => Some(ResultKind::DeviceActionResult),
        "device.recents" => Some(ResultKind::Object),
        "device.favorites" => Some(ResultKind::Object),
        "device.setFavorite" => Some(ResultKind::DeviceActionResult),
        "device.pinnedModels" => Some(ResultKind::Object),
        "device.setModelPinned" => Some(ResultKind::DeviceActionResult),
        "device.captures" => Some(ResultKind::Object),
        "device.loadCapture" => Some(ResultKind::DeviceActionResult),
        "device.irs" => Some(ResultKind::Object),
        "device.loadIr" => Some(ResultKind::DeviceActionResult),
        "device.createSetlist" => Some(ResultKind::DeviceActionResult),
        "device.deleteSetlist" => Some(ResultKind::DeviceActionResult),
        "device.duplicateSetlist" => Some(ResultKind::DeviceActionResult),
        "device.deletePreset" => Some(ResultKind::DeviceActionResult),
        "device.movePreset" => Some(ResultKind::DeviceActionResult),
        "device.setGeneralInteger" => Some(ResultKind::DeviceActionResult),
        "device.setGeneralToggle" => Some(ResultKind::DeviceActionResult),
        "device.setSceneBypassBehavior" => Some(ResultKind::DeviceActionResult),
        "device.setMasterVolumeAssignment" => Some(ResultKind::DeviceActionResult),
        "device.setGlobalBypass" => Some(ResultKind::DeviceActionResult),
        "device.presetScreenshot" => Some(ResultKind::Object),
        "device.captureScreen" => Some(ResultKind::Object),
        "device.diagnostics" => Some(ResultKind::Object),
        "device.graphicsTree" => Some(ResultKind::Object),
        "device.tapScreen" => Some(ResultKind::DeviceActionResult),
        "device.longTapScreen" => Some(ResultKind::DeviceActionResult),
        "device.swipeScreen" => Some(ResultKind::DeviceActionResult),
        "device.dragScreen" => Some(ResultKind::DeviceActionResult),
        "device.mouseFrame" => Some(ResultKind::DeviceActionResult),
        "device.selectScene" => Some(ResultKind::DeviceActionResult),
        "device.copyScene" => Some(ResultKind::DeviceActionResult),
        "device.setSceneLabel" => Some(ResultKind::DeviceActionResult),
        "device.setSceneColor" => Some(ResultKind::DeviceActionResult),
        "device.toggleBypass" => Some(ResultKind::DeviceActionResult),
        "device.moveBlock" => Some(ResultKind::DeviceActionResult),
        "device.addBlock" => Some(ResultKind::DeviceActionResult),
        "device.replaceBlock" => Some(ResultKind::DeviceActionResult),
        "device.createModelPreset" => Some(ResultKind::DeviceActionResult),
        "device.setLooperStompAssignments" => Some(ResultKind::DeviceActionResult),
        "device.resetBlockParameters" => Some(ResultKind::DeviceActionResult),
        "device.removeBlock" => Some(ResultKind::DeviceActionResult),
        "device.setBlockFootswitch" => Some(ResultKind::DeviceActionResult),
        "device.setStompMomentary" => Some(ResultKind::DeviceActionResult),
        "device.setStompLabel" => Some(ResultKind::DeviceActionResult),
        "device.setMidiOut" => Some(ResultKind::DeviceActionResult),
        "device.setPresetLoadMidiOut" => Some(ResultKind::DeviceActionResult),
        "device.setExpressionBypass" => Some(ResultKind::DeviceActionResult),
        "device.setChainInput" => Some(ResultKind::DeviceActionResult),
        "device.setChainOutput" => Some(ResultKind::DeviceActionResult),
        "device.setChainSplit" => Some(ResultKind::DeviceActionResult),
        "device.setSplitMute" => Some(ResultKind::DeviceActionResult),
        "device.listPresets" => Some(ResultKind::Object),
        "device.listPresetFolders" => Some(ResultKind::Object),
        "device.navigateBank" => Some(ResultKind::DeviceActionResult),
        "device.recallPreset" => Some(ResultKind::DeviceActionResult),
        "device.reloadPreset" => Some(ResultKind::DeviceActionResult),
        "device.blockDetails" => Some(ResultKind::Object),
        "device.laneControlDetails" => Some(ResultKind::Object),
        "device.previewParameter" => Some(ResultKind::Object),
        "device.previewLaneControlParameter" => Some(ResultKind::Object),
        "device.setParameter" => Some(ResultKind::DeviceActionResult),
        "device.setLaneControlParameter" => Some(ResultKind::DeviceActionResult),
        "device.setLaneControlSceneMode" => Some(ResultKind::DeviceActionResult),
        "device.setLaneControlExpression" => Some(ResultKind::DeviceActionResult),
        "device.setParameterSceneMode" => Some(ResultKind::DeviceActionResult),
        "device.setParameterExpression" => Some(ResultKind::DeviceActionResult),
        "device.setTempo" => Some(ResultKind::DeviceActionResult),
        "device.setMasterVolume" => Some(ResultKind::DeviceActionResult),
        "device.masterVolume" => Some(ResultKind::Object),
        "device.pressFootswitch" => Some(ResultKind::DeviceActionResult),
        "device.tapTempo" => Some(ResultKind::DeviceActionResult),
        "device.selectModeSlot" => Some(ResultKind::DeviceActionResult),
        "device.listPresetSlots" => Some(ResultKind::Object),
        "device.savePresetAs" => Some(ResultKind::DeviceActionResult),
        "device.copyPreset" => Some(ResultKind::DeviceActionResult),
        "device.renameCurrentPreset" => Some(ResultKind::DeviceActionResult),
        "device.createBackup" => Some(ResultKind::Object),
        "device.showTuner" => Some(ResultKind::DeviceActionResult),
        "device.showGigView" => Some(ResultKind::DeviceActionResult),
        _ => None,
    }
}

/// Verification values a `verified: true` result may carry.
pub const DEVICE_ACTION_VERIFIED_VERIFICATIONS: &[&str] = &[
    "authoritative_readback",
    "authoritative_reconnect_readback",
    "authoritative_preflight",
];
/// Verification values a `verified: false` result may carry.
pub const DEVICE_ACTION_UNVERIFIED_VERIFICATIONS: &[&str] = &["accepted_unverified"];
/// Outcomes that sent no mutation, so they may never report `changed: true`.
pub const DEVICE_ACTION_UNCHANGED_VERIFICATIONS: &[&str] = &["authoritative_preflight"];
/// Outcomes that observed nothing, so they may not report `changed` at all.
pub const DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS: &[&str] = &["accepted_unverified"];

pub fn validate_result(method: &str, result: &serde_json::Value) -> Result<(), String> {
    let result_kind =
        result_kind(method).ok_or_else(|| format!("Unknown gateway result contract: {method}"))?;
    let object = result
        .as_object()
        .ok_or_else(|| format!("{method} returned a malformed result"))?;
    if result_kind == ResultKind::PresetSnapshot
        && (!object
            .get("presetName")
            .is_some_and(serde_json::Value::is_string)
            || !object
                .get("blocks")
                .is_some_and(serde_json::Value::is_array))
    {
        return Err(format!(
            "{method} returned a malformed PresetSnapshot result"
        ));
    }
    let has_outcome = ["accepted", "verified", "verification"]
        .iter()
        .any(|key| object.contains_key(*key));
    if result_kind == ResultKind::DeviceActionResult && !has_outcome {
        return Err(format!(
            "{method} returned a device action result without verification semantics"
        ));
    }
    if has_outcome {
        let verified = object.get("verified").and_then(serde_json::Value::as_bool);
        let allowed: &[&str] = match verified {
            Some(true) => DEVICE_ACTION_VERIFIED_VERIFICATIONS,
            Some(false) => DEVICE_ACTION_UNVERIFIED_VERIFICATIONS,
            None => &[],
        };
        let verification = object
            .get("verification")
            .and_then(serde_json::Value::as_str);
        if object.get("accepted").and_then(serde_json::Value::as_bool) != Some(true)
            || !verification.is_some_and(|value| allowed.contains(&value))
            || object
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|detail| detail.chars().count() > 4096)
        {
            return Err(format!(
                "{method} returned a malformed device action result"
            ));
        }
        let verification = verification.unwrap_or_default();
        let changed = match object.get("changed") {
            None => None,
            Some(serde_json::Value::Bool(value)) => Some(*value),
            Some(_) => {
                return Err(format!(
                    "{method} returned a malformed device action result"
                ));
            }
        };
        if DEVICE_ACTION_UNCHANGED_VERIFICATIONS.contains(&verification) && changed == Some(true) {
            return Err(format!(
                "{method} reported {verification} while claiming it changed device state"
            ));
        }
        if DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS.contains(&verification) && changed.is_some() {
            return Err(format!(
                "{method} reported {verification} while claiming to know whether device state changed"
            ));
        }
        if method == "device.duplicateSetlist"
            && object
                .get("setlistKey")
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(
                "device.duplicateSetlist returned no authoritative created setlist key".into(),
            );
        }
    }
    Ok(())
}
