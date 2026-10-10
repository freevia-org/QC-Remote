// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
/// Version of the gateway API this contract describes.
pub const API_VERSION: u64 = 31;
/// Capability flags the gateway advertises.
pub const CAPABILITIES: &[&str] = &[
    "nativePresetIdentity",
    "modelRepoParameterMetadata",
    "nativeBroker",
    "nativeStateEvents",
    "nativeDeviceIdentity",
    "nativeDiagnostics",
    "nativeRemoteScreen",
    "nativeGraphicsTree",
    "nativeScreenGestures",
    "nativeSceneManagement",
    "nativeParameterAssignments",
    "nativeDefaultParameters",
    "nativeStompMetadata",
    "nativePresetMidiOut",
    "nativeExpressionBypass",
    "nativeTunerSettings",
    "nativeTunerWrites",
    "nativeTunerMeter",
    "nativeLaneControls",
    "nativeGeneralSettings",
    "nativeIoSettings",
    "nativeGlobalEq",
    "nativeModeCycle",
    "nativeLooper",
    "nativeLibraryManagement",
    "nativeSplitMute",
    "nativeTempoMetronome",
    "nativeGlobalTempoWrite",
];
/// Every gateway method name, in contract order.
pub const METHODS: &[&str] = &[
    "system.status",
    "device.reconnect",
    "device.resetSession",
    "device.disconnect",
    "device.snapshot",
    "device.currentPreset",
    "device.stateEvents",
    "device.tempoClock",
    "device.listModels",
    "device.identity",
    "device.setDeviceName",
    "device.undo",
    "device.redo",
    "device.inhibitedModules",
    "device.tunerSettings",
    "device.setTunerInput",
    "device.setTunerMute",
    "device.setTunerReference",
    "device.setTunerMeter",
    "device.generalSettings",
    "device.ioSettings",
    "device.setInputPort",
    "device.setOutputPort",
    "device.setUsbPort",
    "device.setMidiThru",
    "device.setOutputPairing",
    "device.globalEq",
    "device.setGlobalEqBypassed",
    "device.setGlobalEqBand",
    "device.setGlobalEqOutput",
    "device.loadGlobalEqModelPreset",
    "device.modeCycle",
    "device.setModeCycle",
    "device.globalTempoSettings",
    "device.setTempoMetronome",
    "device.setTempoMode",
    "device.setGlobalTempo",
    "device.looperStatus",
    "device.controlLooper",
    "device.recents",
    "device.favorites",
    "device.setFavorite",
    "device.pinnedModels",
    "device.setModelPinned",
    "device.captures",
    "device.loadCapture",
    "device.irs",
    "device.loadIr",
    "device.createSetlist",
    "device.deleteSetlist",
    "device.duplicateSetlist",
    "device.deletePreset",
    "device.movePreset",
    "device.setGeneralInteger",
    "device.setGeneralToggle",
    "device.setSceneBypassBehavior",
    "device.setMasterVolumeAssignment",
    "device.setGlobalBypass",
    "device.presetScreenshot",
    "device.captureScreen",
    "device.diagnostics",
    "device.graphicsTree",
    "device.tapScreen",
    "device.longTapScreen",
    "device.swipeScreen",
    "device.dragScreen",
    "device.mouseFrame",
    "device.selectScene",
    "device.copyScene",
    "device.setSceneLabel",
    "device.setSceneColor",
    "device.toggleBypass",
    "device.moveBlock",
    "device.addBlock",
    "device.replaceBlock",
    "device.createModelPreset",
    "device.setLooperStompAssignments",
    "device.resetBlockParameters",
    "device.removeBlock",
    "device.setBlockFootswitch",
    "device.setStompMomentary",
    "device.setStompLabel",
    "device.setMidiOut",
    "device.setPresetLoadMidiOut",
    "device.setExpressionBypass",
    "device.setChainInput",
    "device.setChainOutput",
    "device.setChainSplit",
    "device.setSplitMute",
    "device.listPresets",
    "device.listPresetFolders",
    "device.navigateBank",
    "device.recallPreset",
    "device.reloadPreset",
    "device.blockDetails",
    "device.laneControlDetails",
    "device.previewParameter",
    "device.previewLaneControlParameter",
    "device.setParameter",
    "device.setLaneControlParameter",
    "device.setLaneControlSceneMode",
    "device.setLaneControlExpression",
    "device.setParameterSceneMode",
    "device.setParameterExpression",
    "device.setTempo",
    "device.setMasterVolume",
    "device.masterVolume",
    "device.pressFootswitch",
    "device.tapTempo",
    "device.selectModeSlot",
    "device.listPresetSlots",
    "device.savePresetAs",
    "device.copyPreset",
    "device.renameCurrentPreset",
    "device.createBackup",
    "device.showTuner",
    "device.showGigView",
];
/// The methods sent as host MIDI performance messages rather than as device
/// protocol messages: the `PERFORMANCE_MIDI` dispatch group.
pub const PERFORMANCE_MIDI_METHODS: &[&str] = &[
    "device.controlLooper",
    "device.pressFootswitch",
    "device.tapTempo",
];

/// Number of normalized controls in one Global EQ band.
pub const GLOBAL_EQ_BAND_STRIDE: i64 = 5;
/// Number of Global EQ bands exposed by the device.
pub const GLOBAL_EQ_BANDS: u32 = 5;
/// Offset of GAIN within one Global EQ band.
pub const GLOBAL_EQ_BAND_GAIN: i64 = 0;
/// Offset of FREQUENCY within one Global EQ band.
pub const GLOBAL_EQ_BAND_FREQUENCY: i64 = 1;
/// Offset of Q within one Global EQ band.
pub const GLOBAL_EQ_BAND_Q: i64 = 2;
/// Offset of TYPE within one Global EQ band.
pub const GLOBAL_EQ_BAND_TYPE: i64 = 3;
/// Offset of ENABLED within one Global EQ band.
pub const GLOBAL_EQ_BAND_ENABLED: i64 = 4;

/// The kind of result a gateway method returns. It decides what
/// [`validate_result`] checks beyond the result being a JSON object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayResultKind {
    /// A plain JSON object with no fixed shape beyond that.
    Object,
    /// A preset snapshot: an object with a string `presetName` and an array `blocks`.
    PresetSnapshot,
    /// A device-action outcome carrying `accepted`, `verified`, `verification`
    /// and `detail`, and optionally `changed`.
    DeviceActionResult,
}

/// The result kind `method` returns, or `None` for an unknown method.
pub fn result_kind(method: &str) -> Option<GatewayResultKind> {
    match method {
        "system.status" => Some(GatewayResultKind::Object),
        "device.reconnect" => Some(GatewayResultKind::Object),
        "device.resetSession" => Some(GatewayResultKind::Object),
        "device.disconnect" => Some(GatewayResultKind::Object),
        "device.snapshot" => Some(GatewayResultKind::PresetSnapshot),
        "device.currentPreset" => Some(GatewayResultKind::PresetSnapshot),
        "device.stateEvents" => Some(GatewayResultKind::Object),
        "device.tempoClock" => Some(GatewayResultKind::Object),
        "device.listModels" => Some(GatewayResultKind::Object),
        "device.identity" => Some(GatewayResultKind::Object),
        "device.setDeviceName" => Some(GatewayResultKind::DeviceActionResult),
        "device.undo" => Some(GatewayResultKind::DeviceActionResult),
        "device.redo" => Some(GatewayResultKind::DeviceActionResult),
        "device.inhibitedModules" => Some(GatewayResultKind::Object),
        "device.tunerSettings" => Some(GatewayResultKind::Object),
        "device.setTunerInput" => Some(GatewayResultKind::DeviceActionResult),
        "device.setTunerMute" => Some(GatewayResultKind::DeviceActionResult),
        "device.setTunerReference" => Some(GatewayResultKind::DeviceActionResult),
        "device.setTunerMeter" => Some(GatewayResultKind::DeviceActionResult),
        "device.generalSettings" => Some(GatewayResultKind::Object),
        "device.ioSettings" => Some(GatewayResultKind::Object),
        "device.setInputPort" => Some(GatewayResultKind::DeviceActionResult),
        "device.setOutputPort" => Some(GatewayResultKind::DeviceActionResult),
        "device.setUsbPort" => Some(GatewayResultKind::DeviceActionResult),
        "device.setMidiThru" => Some(GatewayResultKind::DeviceActionResult),
        "device.setOutputPairing" => Some(GatewayResultKind::DeviceActionResult),
        "device.globalEq" => Some(GatewayResultKind::Object),
        "device.setGlobalEqBypassed" => Some(GatewayResultKind::DeviceActionResult),
        "device.setGlobalEqBand" => Some(GatewayResultKind::DeviceActionResult),
        "device.setGlobalEqOutput" => Some(GatewayResultKind::DeviceActionResult),
        "device.loadGlobalEqModelPreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.modeCycle" => Some(GatewayResultKind::Object),
        "device.setModeCycle" => Some(GatewayResultKind::DeviceActionResult),
        "device.globalTempoSettings" => Some(GatewayResultKind::Object),
        "device.setTempoMetronome" => Some(GatewayResultKind::DeviceActionResult),
        "device.setTempoMode" => Some(GatewayResultKind::DeviceActionResult),
        "device.setGlobalTempo" => Some(GatewayResultKind::DeviceActionResult),
        "device.looperStatus" => Some(GatewayResultKind::Object),
        "device.controlLooper" => Some(GatewayResultKind::DeviceActionResult),
        "device.recents" => Some(GatewayResultKind::Object),
        "device.favorites" => Some(GatewayResultKind::Object),
        "device.setFavorite" => Some(GatewayResultKind::DeviceActionResult),
        "device.pinnedModels" => Some(GatewayResultKind::Object),
        "device.setModelPinned" => Some(GatewayResultKind::DeviceActionResult),
        "device.captures" => Some(GatewayResultKind::Object),
        "device.loadCapture" => Some(GatewayResultKind::DeviceActionResult),
        "device.irs" => Some(GatewayResultKind::Object),
        "device.loadIr" => Some(GatewayResultKind::DeviceActionResult),
        "device.createSetlist" => Some(GatewayResultKind::DeviceActionResult),
        "device.deleteSetlist" => Some(GatewayResultKind::DeviceActionResult),
        "device.duplicateSetlist" => Some(GatewayResultKind::DeviceActionResult),
        "device.deletePreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.movePreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.setGeneralInteger" => Some(GatewayResultKind::DeviceActionResult),
        "device.setGeneralToggle" => Some(GatewayResultKind::DeviceActionResult),
        "device.setSceneBypassBehavior" => Some(GatewayResultKind::DeviceActionResult),
        "device.setMasterVolumeAssignment" => Some(GatewayResultKind::DeviceActionResult),
        "device.setGlobalBypass" => Some(GatewayResultKind::DeviceActionResult),
        "device.presetScreenshot" => Some(GatewayResultKind::Object),
        "device.captureScreen" => Some(GatewayResultKind::Object),
        "device.diagnostics" => Some(GatewayResultKind::Object),
        "device.graphicsTree" => Some(GatewayResultKind::Object),
        "device.tapScreen" => Some(GatewayResultKind::DeviceActionResult),
        "device.longTapScreen" => Some(GatewayResultKind::DeviceActionResult),
        "device.swipeScreen" => Some(GatewayResultKind::DeviceActionResult),
        "device.dragScreen" => Some(GatewayResultKind::DeviceActionResult),
        "device.mouseFrame" => Some(GatewayResultKind::DeviceActionResult),
        "device.selectScene" => Some(GatewayResultKind::DeviceActionResult),
        "device.copyScene" => Some(GatewayResultKind::DeviceActionResult),
        "device.setSceneLabel" => Some(GatewayResultKind::DeviceActionResult),
        "device.setSceneColor" => Some(GatewayResultKind::DeviceActionResult),
        "device.toggleBypass" => Some(GatewayResultKind::DeviceActionResult),
        "device.moveBlock" => Some(GatewayResultKind::DeviceActionResult),
        "device.addBlock" => Some(GatewayResultKind::DeviceActionResult),
        "device.replaceBlock" => Some(GatewayResultKind::DeviceActionResult),
        "device.createModelPreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.setLooperStompAssignments" => Some(GatewayResultKind::DeviceActionResult),
        "device.resetBlockParameters" => Some(GatewayResultKind::DeviceActionResult),
        "device.removeBlock" => Some(GatewayResultKind::DeviceActionResult),
        "device.setBlockFootswitch" => Some(GatewayResultKind::DeviceActionResult),
        "device.setStompMomentary" => Some(GatewayResultKind::DeviceActionResult),
        "device.setStompLabel" => Some(GatewayResultKind::DeviceActionResult),
        "device.setMidiOut" => Some(GatewayResultKind::DeviceActionResult),
        "device.setPresetLoadMidiOut" => Some(GatewayResultKind::DeviceActionResult),
        "device.setExpressionBypass" => Some(GatewayResultKind::DeviceActionResult),
        "device.setChainInput" => Some(GatewayResultKind::DeviceActionResult),
        "device.setChainOutput" => Some(GatewayResultKind::DeviceActionResult),
        "device.setChainSplit" => Some(GatewayResultKind::DeviceActionResult),
        "device.setSplitMute" => Some(GatewayResultKind::DeviceActionResult),
        "device.listPresets" => Some(GatewayResultKind::Object),
        "device.listPresetFolders" => Some(GatewayResultKind::Object),
        "device.navigateBank" => Some(GatewayResultKind::DeviceActionResult),
        "device.recallPreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.reloadPreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.blockDetails" => Some(GatewayResultKind::Object),
        "device.laneControlDetails" => Some(GatewayResultKind::Object),
        "device.previewParameter" => Some(GatewayResultKind::Object),
        "device.previewLaneControlParameter" => Some(GatewayResultKind::Object),
        "device.setParameter" => Some(GatewayResultKind::DeviceActionResult),
        "device.setLaneControlParameter" => Some(GatewayResultKind::DeviceActionResult),
        "device.setLaneControlSceneMode" => Some(GatewayResultKind::DeviceActionResult),
        "device.setLaneControlExpression" => Some(GatewayResultKind::DeviceActionResult),
        "device.setParameterSceneMode" => Some(GatewayResultKind::DeviceActionResult),
        "device.setParameterExpression" => Some(GatewayResultKind::DeviceActionResult),
        "device.setTempo" => Some(GatewayResultKind::DeviceActionResult),
        "device.setMasterVolume" => Some(GatewayResultKind::DeviceActionResult),
        "device.masterVolume" => Some(GatewayResultKind::Object),
        "device.pressFootswitch" => Some(GatewayResultKind::DeviceActionResult),
        "device.tapTempo" => Some(GatewayResultKind::DeviceActionResult),
        "device.selectModeSlot" => Some(GatewayResultKind::DeviceActionResult),
        "device.listPresetSlots" => Some(GatewayResultKind::Object),
        "device.savePresetAs" => Some(GatewayResultKind::DeviceActionResult),
        "device.copyPreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.renameCurrentPreset" => Some(GatewayResultKind::DeviceActionResult),
        "device.createBackup" => Some(GatewayResultKind::Object),
        "device.showTuner" => Some(GatewayResultKind::DeviceActionResult),
        "device.showGigView" => Some(GatewayResultKind::DeviceActionResult),
        _ => None,
    }
}

/// Generated from qc-actions.v1.json. This is protocol intent, not response
/// shape: preview and backup mutations intentionally return plain objects.
pub fn is_mutation_method(method: &str) -> bool {
    matches!(
        method,
        "device.reconnect"
            | "device.resetSession"
            | "device.disconnect"
            | "device.setDeviceName"
            | "device.undo"
            | "device.redo"
            | "device.setTunerInput"
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
            | "device.controlLooper"
            | "device.setFavorite"
            | "device.setModelPinned"
            | "device.loadCapture"
            | "device.loadIr"
            | "device.createSetlist"
            | "device.deleteSetlist"
            | "device.duplicateSetlist"
            | "device.deletePreset"
            | "device.movePreset"
            | "device.setGeneralInteger"
            | "device.setGeneralToggle"
            | "device.setSceneBypassBehavior"
            | "device.setMasterVolumeAssignment"
            | "device.setGlobalBypass"
            | "device.tapScreen"
            | "device.longTapScreen"
            | "device.swipeScreen"
            | "device.dragScreen"
            | "device.mouseFrame"
            | "device.selectScene"
            | "device.copyScene"
            | "device.setSceneLabel"
            | "device.setSceneColor"
            | "device.toggleBypass"
            | "device.moveBlock"
            | "device.addBlock"
            | "device.replaceBlock"
            | "device.createModelPreset"
            | "device.setLooperStompAssignments"
            | "device.resetBlockParameters"
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
            | "device.navigateBank"
            | "device.recallPreset"
            | "device.reloadPreset"
            | "device.previewParameter"
            | "device.previewLaneControlParameter"
            | "device.setParameter"
            | "device.setLaneControlParameter"
            | "device.setLaneControlSceneMode"
            | "device.setLaneControlExpression"
            | "device.setParameterSceneMode"
            | "device.setParameterExpression"
            | "device.setTempo"
            | "device.setMasterVolume"
            | "device.pressFootswitch"
            | "device.tapTempo"
            | "device.selectModeSlot"
            | "device.savePresetAs"
            | "device.copyPreset"
            | "device.renameCurrentPreset"
            | "device.createBackup"
            | "device.showTuner"
            | "device.showGigView"
    )
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

/// Check a gateway `result` against the contract for `method`.
///
/// The result must be a JSON object. A `PresetSnapshot` needs `presetName` and `blocks`, and a
/// device-action outcome needs a consistent `accepted`, `verified`, `verification` and `detail`
/// with an optional `changed`. Returns a description of the first violation.
pub fn validate_result(method: &str, result: &serde_json::Value) -> Result<(), String> {
    let result_kind =
        result_kind(method).ok_or_else(|| format!("Unknown gateway result contract: {method}"))?;
    let object = result
        .as_object()
        .ok_or_else(|| format!("{method} returned a malformed result"))?;
    if result_kind == GatewayResultKind::PresetSnapshot
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
    if result_kind == GatewayResultKind::DeviceActionResult && !has_outcome {
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
                ))
            }
        };
        if DEVICE_ACTION_UNCHANGED_VERIFICATIONS.contains(&verification) && changed == Some(true) {
            return Err(format!(
                "{method} reported {verification} while claiming it changed device state"
            ));
        }
        if DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS.contains(&verification) && changed.is_some() {
            return Err(format!("{method} reported {verification} while claiming to know whether device state changed"));
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

/// How the device broker dispatches a gateway method. Every method belongs to
/// exactly one group, and each variant names a group handled by one code path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrokerDispatch {
    /// Dispatch group `SYSTEM_STATUS`: `system.status`.
    SystemStatus,
    /// Dispatch group `RECONNECT`: `device.reconnect`.
    Reconnect,
    /// Dispatch group `RESET_SESSION`: `device.resetSession`.
    ResetSession,
    /// Dispatch group `DISCONNECT`: `device.disconnect`.
    Disconnect,
    /// Dispatch group `SNAPSHOT`: `device.snapshot`.
    Snapshot,
    /// Dispatch group `CURRENT_PRESET`: `device.currentPreset`.
    CurrentPreset,
    /// Dispatch group `STATE_EVENTS`: `device.stateEvents`.
    StateEvents,
    /// Dispatch group `TEMPO_CLOCK`: `device.tempoClock`.
    TempoClock,
    /// Dispatch group `LIST_MODELS`: `device.listModels`.
    ListModels,
    /// Dispatch group `IDENTITY`: `device.identity`.
    Identity,
    /// Dispatch group `SET_DEVICE_NAME`: `device.setDeviceName`.
    SetDeviceName,
    /// Dispatch group `UNDO`: `device.undo`.
    Undo,
    /// Dispatch group `REDO`: `device.redo`.
    Redo,
    /// Dispatch group `INHIBITED_MODULES`: `device.inhibitedModules`.
    InhibitedModules,
    /// Dispatch group `GATEWAY_READ`: 10 methods, starting with `device.diagnostics`, `device.tunerSettings`, `device.globalEq`.
    GatewayRead,
    /// Dispatch group `GENERAL_SETTINGS`: `device.generalSettings`.
    GeneralSettings,
    /// Dispatch group `IO_SETTINGS`: `device.ioSettings`.
    IoSettings,
    /// Dispatch group `LIBRARY_READ`: `device.captures`, `device.irs`.
    LibraryRead,
    /// Dispatch group `GATEWAY_OPERATION`: 49 methods, starting with `device.setTunerInput`, `device.setTunerMute`, `device.setTunerMeter`.
    GatewayOperation,
    /// Dispatch group `DELETE_SETLIST`: `device.deleteSetlist`.
    DeleteSetlist,
    /// Dispatch group `PRESET_SCREENSHOT`: `device.presetScreenshot`.
    PresetScreenshot,
    /// Dispatch group `CAPTURE_SCREEN`: `device.captureScreen`.
    CaptureScreen,
    /// Dispatch group `TAP_SCREEN`: 5 methods, starting with `device.tapScreen`, `device.longTapScreen`, `device.swipeScreen`.
    TapScreen,
    /// Dispatch group `SELECT_SCENE`: `device.selectScene`.
    SelectScene,
    /// Dispatch group `TOGGLE_BYPASS`: `device.toggleBypass`.
    ToggleBypass,
    /// Dispatch group `BLOCK_DETAILS`: `device.blockDetails`.
    BlockDetails,
    /// Dispatch group `LANE_CONTROL_DETAILS`: `device.laneControlDetails`.
    LaneControlDetails,
    /// Dispatch group `PREVIEW_PARAMETER`: `device.previewParameter`.
    PreviewParameter,
    /// Dispatch group `PREVIEW_LANE_CONTROL_PARAMETER`: `device.previewLaneControlParameter`.
    PreviewLaneControlParameter,
    /// Dispatch group `SET_PARAMETER`: `device.setParameter`.
    SetParameter,
    /// Dispatch group `SET_LANE_CONTROL_PARAMETER`: `device.setLaneControlParameter`.
    SetLaneControlParameter,
    /// Dispatch group `SET_LANE_CONTROL_SCENE_MODE`: `device.setLaneControlSceneMode`.
    SetLaneControlSceneMode,
    /// Dispatch group `SET_LANE_CONTROL_EXPRESSION`: `device.setLaneControlExpression`.
    SetLaneControlExpression,
    /// Dispatch group `PARAMETER_ASSIGNMENT`: `device.setParameterSceneMode`, `device.setParameterExpression`.
    ParameterAssignment,
    /// Dispatch group `SET_TEMPO`: `device.setTempo`.
    SetTempo,
    /// Dispatch group `SET_MASTER_VOLUME`: `device.setMasterVolume`.
    SetMasterVolume,
    /// Dispatch group `MASTER_VOLUME`: `device.masterVolume`.
    MasterVolume,
    /// Dispatch group `PERFORMANCE_MIDI`: `device.pressFootswitch`, `device.tapTempo`, `device.controlLooper`.
    PerformanceMidi,
    /// Dispatch group `LIST_PRESETS`: `device.listPresets`.
    ListPresets,
    /// Dispatch group `LIST_PRESET_FOLDERS`: `device.listPresetFolders`.
    ListPresetFolders,
    /// Dispatch group `NAVIGATE_BANK`: `device.navigateBank`.
    NavigateBank,
    /// Dispatch group `RECALL_PRESET`: `device.recallPreset`.
    RecallPreset,
    /// Dispatch group `RELOAD_PRESET`: `device.reloadPreset`.
    ReloadPreset,
    /// Dispatch group `LIST_PRESET_SLOTS`: `device.listPresetSlots`.
    ListPresetSlots,
    /// Dispatch group `SAVE_PRESET_AS`: `device.savePresetAs`.
    SavePresetAs,
    /// Dispatch group `COPY_PRESET`: `device.copyPreset`.
    CopyPreset,
    /// Dispatch group `RENAME_CURRENT_PRESET`: `device.renameCurrentPreset`.
    RenameCurrentPreset,
    /// Dispatch group `CREATE_BACKUP`: `device.createBackup`.
    CreateBackup,
    /// Dispatch group `CREATE_SETLIST`: `device.createSetlist`.
    CreateSetlist,
    /// Dispatch group `DUPLICATE_SETLIST`: `device.duplicateSetlist`.
    DuplicateSetlist,
    /// Dispatch group `DELETE_PRESET`: `device.deletePreset`.
    DeletePreset,
    /// Dispatch group `MOVE_PRESET`: `device.movePreset`.
    MovePreset,
}

/// The dispatch group of `method`, or `None` for an unknown method.
pub fn broker_dispatch(method: &str) -> Option<BrokerDispatch> {
    match method {
        "system.status" => Some(BrokerDispatch::SystemStatus),
        "device.reconnect" => Some(BrokerDispatch::Reconnect),
        "device.resetSession" => Some(BrokerDispatch::ResetSession),
        "device.disconnect" => Some(BrokerDispatch::Disconnect),
        "device.snapshot" => Some(BrokerDispatch::Snapshot),
        "device.currentPreset" => Some(BrokerDispatch::CurrentPreset),
        "device.stateEvents" => Some(BrokerDispatch::StateEvents),
        "device.tempoClock" => Some(BrokerDispatch::TempoClock),
        "device.listModels" => Some(BrokerDispatch::ListModels),
        "device.identity" => Some(BrokerDispatch::Identity),
        "device.setDeviceName" => Some(BrokerDispatch::SetDeviceName),
        "device.undo" => Some(BrokerDispatch::Undo),
        "device.redo" => Some(BrokerDispatch::Redo),
        "device.inhibitedModules" => Some(BrokerDispatch::InhibitedModules),
        "device.tunerSettings" => Some(BrokerDispatch::GatewayRead),
        "device.setTunerInput" => Some(BrokerDispatch::GatewayOperation),
        "device.setTunerMute" => Some(BrokerDispatch::GatewayOperation),
        "device.setTunerReference" => Some(BrokerDispatch::GatewayOperation),
        "device.setTunerMeter" => Some(BrokerDispatch::GatewayOperation),
        "device.generalSettings" => Some(BrokerDispatch::GeneralSettings),
        "device.ioSettings" => Some(BrokerDispatch::IoSettings),
        "device.setInputPort" => Some(BrokerDispatch::GatewayOperation),
        "device.setOutputPort" => Some(BrokerDispatch::GatewayOperation),
        "device.setUsbPort" => Some(BrokerDispatch::GatewayOperation),
        "device.setMidiThru" => Some(BrokerDispatch::GatewayOperation),
        "device.setOutputPairing" => Some(BrokerDispatch::GatewayOperation),
        "device.globalEq" => Some(BrokerDispatch::GatewayRead),
        "device.setGlobalEqBypassed" => Some(BrokerDispatch::GatewayOperation),
        "device.setGlobalEqBand" => Some(BrokerDispatch::GatewayOperation),
        "device.setGlobalEqOutput" => Some(BrokerDispatch::GatewayOperation),
        "device.loadGlobalEqModelPreset" => Some(BrokerDispatch::GatewayOperation),
        "device.modeCycle" => Some(BrokerDispatch::GatewayRead),
        "device.setModeCycle" => Some(BrokerDispatch::GatewayOperation),
        "device.globalTempoSettings" => Some(BrokerDispatch::GatewayRead),
        "device.setTempoMetronome" => Some(BrokerDispatch::GatewayOperation),
        "device.setTempoMode" => Some(BrokerDispatch::GatewayOperation),
        "device.setGlobalTempo" => Some(BrokerDispatch::GatewayOperation),
        "device.looperStatus" => Some(BrokerDispatch::GatewayRead),
        "device.controlLooper" => Some(BrokerDispatch::PerformanceMidi),
        "device.recents" => Some(BrokerDispatch::GatewayRead),
        "device.favorites" => Some(BrokerDispatch::GatewayRead),
        "device.setFavorite" => Some(BrokerDispatch::GatewayOperation),
        "device.pinnedModels" => Some(BrokerDispatch::GatewayRead),
        "device.setModelPinned" => Some(BrokerDispatch::GatewayOperation),
        "device.captures" => Some(BrokerDispatch::LibraryRead),
        "device.loadCapture" => Some(BrokerDispatch::GatewayOperation),
        "device.irs" => Some(BrokerDispatch::LibraryRead),
        "device.loadIr" => Some(BrokerDispatch::GatewayOperation),
        "device.createSetlist" => Some(BrokerDispatch::CreateSetlist),
        "device.deleteSetlist" => Some(BrokerDispatch::DeleteSetlist),
        "device.duplicateSetlist" => Some(BrokerDispatch::DuplicateSetlist),
        "device.deletePreset" => Some(BrokerDispatch::DeletePreset),
        "device.movePreset" => Some(BrokerDispatch::MovePreset),
        "device.setGeneralInteger" => Some(BrokerDispatch::GatewayOperation),
        "device.setGeneralToggle" => Some(BrokerDispatch::GatewayOperation),
        "device.setSceneBypassBehavior" => Some(BrokerDispatch::GatewayOperation),
        "device.setMasterVolumeAssignment" => Some(BrokerDispatch::GatewayOperation),
        "device.setGlobalBypass" => Some(BrokerDispatch::GatewayOperation),
        "device.presetScreenshot" => Some(BrokerDispatch::PresetScreenshot),
        "device.captureScreen" => Some(BrokerDispatch::CaptureScreen),
        "device.diagnostics" => Some(BrokerDispatch::GatewayRead),
        "device.graphicsTree" => Some(BrokerDispatch::GatewayRead),
        "device.tapScreen" => Some(BrokerDispatch::TapScreen),
        "device.longTapScreen" => Some(BrokerDispatch::TapScreen),
        "device.swipeScreen" => Some(BrokerDispatch::TapScreen),
        "device.dragScreen" => Some(BrokerDispatch::TapScreen),
        "device.mouseFrame" => Some(BrokerDispatch::TapScreen),
        "device.selectScene" => Some(BrokerDispatch::SelectScene),
        "device.copyScene" => Some(BrokerDispatch::GatewayOperation),
        "device.setSceneLabel" => Some(BrokerDispatch::GatewayOperation),
        "device.setSceneColor" => Some(BrokerDispatch::GatewayOperation),
        "device.toggleBypass" => Some(BrokerDispatch::ToggleBypass),
        "device.moveBlock" => Some(BrokerDispatch::GatewayOperation),
        "device.addBlock" => Some(BrokerDispatch::GatewayOperation),
        "device.replaceBlock" => Some(BrokerDispatch::GatewayOperation),
        "device.createModelPreset" => Some(BrokerDispatch::GatewayOperation),
        "device.setLooperStompAssignments" => Some(BrokerDispatch::GatewayOperation),
        "device.resetBlockParameters" => Some(BrokerDispatch::GatewayOperation),
        "device.removeBlock" => Some(BrokerDispatch::GatewayOperation),
        "device.setBlockFootswitch" => Some(BrokerDispatch::GatewayOperation),
        "device.setStompMomentary" => Some(BrokerDispatch::GatewayOperation),
        "device.setStompLabel" => Some(BrokerDispatch::GatewayOperation),
        "device.setMidiOut" => Some(BrokerDispatch::GatewayOperation),
        "device.setPresetLoadMidiOut" => Some(BrokerDispatch::GatewayOperation),
        "device.setExpressionBypass" => Some(BrokerDispatch::GatewayOperation),
        "device.setChainInput" => Some(BrokerDispatch::GatewayOperation),
        "device.setChainOutput" => Some(BrokerDispatch::GatewayOperation),
        "device.setChainSplit" => Some(BrokerDispatch::GatewayOperation),
        "device.setSplitMute" => Some(BrokerDispatch::GatewayOperation),
        "device.listPresets" => Some(BrokerDispatch::ListPresets),
        "device.listPresetFolders" => Some(BrokerDispatch::ListPresetFolders),
        "device.navigateBank" => Some(BrokerDispatch::NavigateBank),
        "device.recallPreset" => Some(BrokerDispatch::RecallPreset),
        "device.reloadPreset" => Some(BrokerDispatch::ReloadPreset),
        "device.blockDetails" => Some(BrokerDispatch::BlockDetails),
        "device.laneControlDetails" => Some(BrokerDispatch::LaneControlDetails),
        "device.previewParameter" => Some(BrokerDispatch::PreviewParameter),
        "device.previewLaneControlParameter" => Some(BrokerDispatch::PreviewLaneControlParameter),
        "device.setParameter" => Some(BrokerDispatch::SetParameter),
        "device.setLaneControlParameter" => Some(BrokerDispatch::SetLaneControlParameter),
        "device.setLaneControlSceneMode" => Some(BrokerDispatch::SetLaneControlSceneMode),
        "device.setLaneControlExpression" => Some(BrokerDispatch::SetLaneControlExpression),
        "device.setParameterSceneMode" => Some(BrokerDispatch::ParameterAssignment),
        "device.setParameterExpression" => Some(BrokerDispatch::ParameterAssignment),
        "device.setTempo" => Some(BrokerDispatch::SetTempo),
        "device.setMasterVolume" => Some(BrokerDispatch::SetMasterVolume),
        "device.masterVolume" => Some(BrokerDispatch::MasterVolume),
        "device.pressFootswitch" => Some(BrokerDispatch::PerformanceMidi),
        "device.tapTempo" => Some(BrokerDispatch::PerformanceMidi),
        "device.selectModeSlot" => Some(BrokerDispatch::GatewayOperation),
        "device.listPresetSlots" => Some(BrokerDispatch::ListPresetSlots),
        "device.savePresetAs" => Some(BrokerDispatch::SavePresetAs),
        "device.copyPreset" => Some(BrokerDispatch::CopyPreset),
        "device.renameCurrentPreset" => Some(BrokerDispatch::RenameCurrentPreset),
        "device.createBackup" => Some(BrokerDispatch::CreateBackup),
        "device.showTuner" => Some(BrokerDispatch::GatewayOperation),
        "device.showGigView" => Some(BrokerDispatch::GatewayOperation),
        _ => None,
    }
}

/// Check request `params` against the contract for `method`.
///
/// The params must be absent or a JSON object, name no argument the method does
/// not declare, supply every required argument, and give each typed argument a
/// value of its declared JSON type (a `nullable-` kind also accepts null).
/// Returns a description of the first violation.
pub fn validate_params(method: &str, params: &serde_json::Value) -> Result<(), String> {
    let object = match params {
        serde_json::Value::Null => None,
        serde_json::Value::Object(object) => Some(object),
        _ => return Err("JSON-RPC params must be an object".into()),
    };
    let (allowed, required): (&[&str], &[&str]) = match method {
        "system.status" => (&[], &[]),
        "device.reconnect" => (&[], &[]),
        "device.resetSession" => (&[], &[]),
        "device.disconnect" => (&[], &[]),
        "device.snapshot" => (&[], &[]),
        "device.currentPreset" => (&[], &[]),
        "device.stateEvents" => (&["afterSequence", "limit"], &[]),
        "device.tempoClock" => (&[], &[]),
        "device.listModels" => (&[], &[]),
        "device.identity" => (&[], &[]),
        "device.setDeviceName" => (&["name"], &["name"]),
        "device.undo" => (&["expectedPresetIdentity"], &["expectedPresetIdentity"]),
        "device.redo" => (&["expectedPresetIdentity"], &["expectedPresetIdentity"]),
        "device.inhibitedModules" => (&[], &[]),
        "device.tunerSettings" => (&[], &[]),
        "device.setTunerInput" => (
            &["inputPortId", "confirmTunerActivation"],
            &["inputPortId", "confirmTunerActivation"],
        ),
        "device.setTunerMute" => (
            &["muted", "confirmTunerActivation"],
            &["muted", "confirmTunerActivation"],
        ),
        "device.setTunerReference" => (
            &["referenceOffsetHz", "confirmTunerActivation"],
            &["referenceOffsetHz", "confirmTunerActivation"],
        ),
        "device.setTunerMeter" => (
            &["enabled", "confirmTunerActivation"],
            &["enabled", "confirmTunerActivation"],
        ),
        "device.generalSettings" => (&[], &[]),
        "device.ioSettings" => (&[], &[]),
        "device.setInputPort" => (
            &[
                "inputPortId",
                "levelDb",
                "impedance",
                "inputType",
                "groundLift",
            ],
            &[
                "inputPortId",
                "levelDb",
                "impedance",
                "inputType",
                "groundLift",
            ],
        ),
        "device.setOutputPort" => (
            &["outputPortId", "level", "groundLift", "mute"],
            &["outputPortId", "level", "groundLift", "mute"],
        ),
        "device.setUsbPort" => (
            &["level", "headphonesSource", "dryWet"],
            &["level", "headphonesSource", "dryWet"],
        ),
        "device.setMidiThru" => (&["enabled"], &["enabled"]),
        "device.setOutputPairing" => (
            &["xlr12Linked", "out34Linked"],
            &["xlr12Linked", "out34Linked"],
        ),
        "device.globalEq" => (&[], &[]),
        "device.setGlobalEqBypassed" => (&["bypassed"], &["bypassed"]),
        "device.setGlobalEqBand" => (
            &["band", "gain", "frequency", "q", "filterType", "enabled"],
            &["band", "gain", "frequency", "q", "filterType", "enabled"],
        ),
        "device.setGlobalEqOutput" => (&["level", "out12", "out34"], &["level", "out12", "out34"]),
        "device.loadGlobalEqModelPreset" => (
            &["value", "isFactory", "hash"],
            &["value", "isFactory", "hash"],
        ),
        "device.modeCycle" => (&[], &[]),
        "device.setModeCycle" => (&["slots"], &["slots"]),
        "device.globalTempoSettings" => (&[], &[]),
        "device.setTempoMetronome" => (
            &[
                "ledEnabled",
                "volumeDb",
                "running",
                "pan",
                "timeSignature",
                "subdivision",
                "sound",
                "routing",
                "beats",
            ],
            &[
                "ledEnabled",
                "volumeDb",
                "running",
                "pan",
                "timeSignature",
                "subdivision",
                "sound",
                "routing",
                "beats",
            ],
        ),
        "device.setTempoMode" => (&["mode"], &["mode"]),
        "device.setGlobalTempo" => (
            &["bpm", "expectedMode", "expectedGlobalBpm"],
            &["bpm", "expectedMode", "expectedGlobalBpm"],
        ),
        "device.looperStatus" => (&[], &[]),
        "device.controlLooper" => (&["command", "value"], &["command", "value"]),
        "device.recents" => (&[], &[]),
        "device.favorites" => (&[], &[]),
        "device.setFavorite" => (
            &[
                "name",
                "folderKey",
                "productKey",
                "isFactory",
                "favorite",
                "expectedFavorite",
                "isPlugin",
            ],
            &[
                "name",
                "folderKey",
                "productKey",
                "isFactory",
                "favorite",
                "expectedFavorite",
            ],
        ),
        "device.pinnedModels" => (&[], &[]),
        "device.setModelPinned" => (
            &["modelId", "pinned", "expectedPinned"],
            &["modelId", "pinned", "expectedPinned"],
        ),
        "device.captures" => (&[], &[]),
        "device.loadCapture" => (
            &[
                "row",
                "column",
                "key",
                "name",
                "modelId",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "key",
                "name",
                "modelId",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.irs" => (&["folder"], &["folder"]),
        "device.loadIr" => (
            &[
                "row",
                "column",
                "key",
                "name",
                "slot",
                "modelId",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "key",
                "name",
                "slot",
                "modelId",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.createSetlist" => (&["name"], &["name"]),
        "device.deleteSetlist" => (&["name"], &["name"]),
        "device.duplicateSetlist" => (
            &["sourceSetlistKey", "expectedSourceName"],
            &["sourceSetlistKey", "expectedSourceName"],
        ),
        "device.deletePreset" => (
            &["setlistKey", "position", "expectedName"],
            &["setlistKey", "position", "expectedName"],
        ),
        "device.movePreset" => (
            &[
                "setlistKey",
                "sourcePosition",
                "sourceName",
                "destinationPosition",
            ],
            &[
                "setlistKey",
                "sourcePosition",
                "sourceName",
                "destinationPosition",
            ],
        ),
        "device.setGeneralInteger" => (&["setting", "value"], &["setting", "value"]),
        "device.setGeneralToggle" => (&["setting", "enabled"], &["setting", "enabled"]),
        "device.setSceneBypassBehavior" => (&["behavior"], &["behavior"]),
        "device.setMasterVolumeAssignment" => (
            &["out12", "out34", "send12", "headphones"],
            &["out12", "out34", "send12", "headphones"],
        ),
        "device.setGlobalBypass" => (&["cab", "ir"], &["cab", "ir"]),
        "device.presetScreenshot" => (
            &["folderName", "position", "isFactory"],
            &["folderName", "position"],
        ),
        "device.captureScreen" => (&[], &[]),
        "device.diagnostics" => (&[], &[]),
        "device.graphicsTree" => (&[], &[]),
        "device.tapScreen" => (&["x", "y"], &["x", "y"]),
        "device.longTapScreen" => (&["x", "y"], &["x", "y"]),
        "device.swipeScreen" => (&["x", "y", "toX", "toY"], &["x", "y", "toX", "toY"]),
        "device.dragScreen" => (
            &["x", "y", "toX", "toY", "steps", "holdMs"],
            &["x", "y", "toX", "toY"],
        ),
        "device.mouseFrame" => (&["type", "x", "y", "toX", "toY"], &["type", "x", "y"]),
        "device.selectScene" => (
            &["scene", "expectedPresetName", "expectedPresetIdentity"],
            &["scene", "expectedPresetIdentity"],
        ),
        "device.copyScene" => (
            &[
                "fromScene",
                "toScene",
                "swap",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["fromScene", "toScene", "expectedPresetIdentity"],
        ),
        "device.setSceneLabel" => (
            &[
                "scene",
                "label",
                "expectedLabel",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["scene", "label", "expectedLabel", "expectedPresetIdentity"],
        ),
        "device.setSceneColor" => (
            &[
                "scene",
                "color",
                "expectedColor",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["scene", "color", "expectedColor", "expectedPresetIdentity"],
        ),
        "device.toggleBypass" => (
            &[
                "row",
                "column",
                "expectedScene",
                "expectedBypassed",
                "desiredBypassed",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "expectedScene",
                "expectedBypassed",
                "desiredBypassed",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.moveBlock" => (
            &[
                "row",
                "fromColumn",
                "toColumn",
                "expectedModelId",
                "toRow",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "fromColumn",
                "toColumn",
                "expectedModelId",
                "toRow",
                "expectedPresetIdentity",
            ],
        ),
        "device.addBlock" => (
            &[
                "row",
                "column",
                "modelId",
                "expectedPresetName",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
            &["row", "column", "modelId", "expectedPresetIdentity"],
        ),
        "device.replaceBlock" => (
            &[
                "row",
                "column",
                "modelId",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "modelId",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.createModelPreset" => (
            &[
                "row",
                "column",
                "modelHash",
                "name",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "modelHash",
                "name",
                "expectedPresetIdentity",
            ],
        ),
        "device.setLooperStompAssignments" => (
            &["assignments", "expectedAssignments"],
            &["assignments", "expectedAssignments"],
        ),
        "device.resetBlockParameters" => (
            &[
                "row",
                "column",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["row", "column", "expectedModelId", "expectedPresetIdentity"],
        ),
        "device.removeBlock" => (
            &[
                "row",
                "column",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["row", "column", "expectedModelId", "expectedPresetIdentity"],
        ),
        "device.setBlockFootswitch" => (
            &[
                "row",
                "column",
                "footswitch",
                "assignmentType",
                "expectedFootswitch",
                "expectedAssignmentType",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "footswitch",
                "assignmentType",
                "expectedFootswitch",
                "expectedAssignmentType",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setStompMomentary" => (
            &[
                "footswitch",
                "momentary",
                "expectedMomentary",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "footswitch",
                "momentary",
                "expectedMomentary",
                "expectedPresetIdentity",
            ],
        ),
        "device.setStompLabel" => (
            &[
                "footswitch",
                "label",
                "expectedLabel",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "footswitch",
                "label",
                "expectedLabel",
                "expectedPresetIdentity",
            ],
        ),
        "device.setMidiOut" => (
            &[
                "source",
                "messages",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["source", "messages", "expectedPresetIdentity"],
        ),
        "device.setPresetLoadMidiOut" => (
            &[
                "messages",
                "expectedMessages",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["messages", "expectedMessages", "expectedPresetIdentity"],
        ),
        "device.setExpressionBypass" => (
            &[
                "row",
                "column",
                "pedal",
                "mode",
                "invert",
                "delayMs",
                "latchEmulation",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "pedal",
                "mode",
                "invert",
                "delayMs",
                "latchEmulation",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setChainInput" => (
            &[
                "row",
                "inputId",
                "expectedInputId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "inputId",
                "expectedInputId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setChainOutput" => (
            &[
                "row",
                "outputId",
                "expectedOutputId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "outputId",
                "expectedOutputId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setChainSplit" => (
            &[
                "row",
                "splitColumn",
                "mixColumn",
                "expectedSplitColumn",
                "expectedMixColumn",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "splitColumn",
                "mixColumn",
                "expectedSplitColumn",
                "expectedMixColumn",
                "expectedPresetIdentity",
            ],
        ),
        "device.setSplitMute" => (
            &[
                "row",
                "muted",
                "expectedMuted",
                "expectedScene",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "muted",
                "expectedMuted",
                "expectedScene",
                "expectedPresetIdentity",
            ],
        ),
        "device.listPresets" => (&["refresh", "setlistKey"], &["setlistKey"]),
        "device.listPresetFolders" => (&["refresh"], &[]),
        "device.navigateBank" => (
            &[
                "direction",
                "expectedPresetName",
                "expectedPosition",
                "expectedPresetIdentity",
            ],
            &["direction", "expectedPosition", "expectedPresetIdentity"],
        ),
        "device.recallPreset" => (
            &[
                "setlistKey",
                "position",
                "expectedPresetName",
                "expectedPosition",
                "expectedSetlistKey",
                "expectedPresetIdentity",
            ],
            &[
                "setlistKey",
                "position",
                "expectedPosition",
                "expectedSetlistKey",
                "expectedPresetIdentity",
            ],
        ),
        "device.reloadPreset" => (
            &[
                "expectedPresetName",
                "expectedPosition",
                "expectedPresetIdentity",
            ],
            &["expectedPosition", "expectedPresetIdentity"],
        ),
        "device.blockDetails" => (
            &[
                "row",
                "column",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["row", "column", "expectedPresetIdentity"],
        ),
        "device.laneControlDetails" => (
            &[
                "row",
                "control",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["row", "control", "expectedPresetIdentity"],
        ),
        "device.previewParameter" => (
            &[
                "row",
                "column",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedScene",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedScene",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.previewLaneControlParameter" => (
            &[
                "row",
                "control",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "control",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedPresetIdentity",
            ],
        ),
        "device.setParameter" => (
            &[
                "row",
                "column",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedScene",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedScene",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setLaneControlParameter" => (
            &[
                "row",
                "control",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "control",
                "parameterIndex",
                "value",
                "expectedValue",
                "expectedPresetIdentity",
            ],
        ),
        "device.setLaneControlSceneMode" => (
            &[
                "row",
                "control",
                "parameterIndex",
                "enabled",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "control",
                "parameterIndex",
                "enabled",
                "expectedPresetIdentity",
            ],
        ),
        "device.setLaneControlExpression" => (
            &[
                "row",
                "control",
                "parameterIndex",
                "pedal",
                "minimum",
                "maximum",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "control",
                "parameterIndex",
                "pedal",
                "minimum",
                "maximum",
                "expectedPresetIdentity",
            ],
        ),
        "device.setParameterSceneMode" => (
            &[
                "row",
                "column",
                "parameterIndex",
                "enabled",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "parameterIndex",
                "enabled",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setParameterExpression" => (
            &[
                "row",
                "column",
                "parameterIndex",
                "pedal",
                "minimum",
                "maximum",
                "expectedModelId",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &[
                "row",
                "column",
                "parameterIndex",
                "pedal",
                "minimum",
                "maximum",
                "expectedModelId",
                "expectedPresetIdentity",
            ],
        ),
        "device.setTempo" => (
            &[
                "bpm",
                "expectedTempo",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["bpm", "expectedTempo", "expectedPresetIdentity"],
        ),
        "device.setMasterVolume" => (&["value", "expectedValue"], &["value", "expectedValue"]),
        "device.masterVolume" => (&[], &[]),
        "device.pressFootswitch" => (
            &[
                "index",
                "expectedMode",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["index", "expectedMode", "expectedPresetIdentity"],
        ),
        "device.tapTempo" => (
            &[
                "expectedMode",
                "expectedPresetName",
                "expectedPresetIdentity",
            ],
            &["expectedMode", "expectedPresetIdentity"],
        ),
        "device.selectModeSlot" => (
            &["slot", "expectedPresetName", "expectedPresetIdentity"],
            &["slot", "expectedPresetIdentity"],
        ),
        "device.listPresetSlots" => (&[], &[]),
        "device.savePresetAs" => (
            &[
                "setlistKey",
                "position",
                "name",
                "expectedPresetName",
                "expectedPosition",
                "confirmOverwrite",
                "expectedPresetIdentity",
            ],
            &[
                "setlistKey",
                "position",
                "name",
                "expectedPosition",
                "expectedPresetIdentity",
            ],
        ),
        "device.copyPreset" => (
            &[
                "sourceSetlistKey",
                "sourcePosition",
                "sourceName",
                "destinationSetlistKey",
                "destinationPosition",
                "confirmOverwrite",
            ],
            &[
                "sourceSetlistKey",
                "sourcePosition",
                "destinationSetlistKey",
                "destinationPosition",
            ],
        ),
        "device.renameCurrentPreset" => (
            &[
                "name",
                "expectedPresetName",
                "expectedPosition",
                "confirmRename",
                "expectedPresetIdentity",
            ],
            &["name", "expectedPosition", "expectedPresetIdentity"],
        ),
        "device.createBackup" => (&["name"], &[]),
        "device.showTuner" => (&["shown"], &[]),
        "device.showGigView" => (&["shown"], &[]),
        _ => return Err(format!("Method not found: {method}")),
    };
    let supplied = object.map(|value| value.keys()).into_iter().flatten();
    for key in supplied {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unexpected argument for {method}: {key}"));
        }
    }
    for key in required {
        if !object.is_some_and(|value| value.contains_key(*key)) {
            return Err(format!("missing required argument for {method}: {key}"));
        }
    }
    if let Some(object) = object {
        let kinds: &[(&str, &str)] = match method {
            "system.status" => &[],
            "device.reconnect" => &[],
            "device.resetSession" => &[],
            "device.disconnect" => &[],
            "device.snapshot" => &[],
            "device.currentPreset" => &[],
            "device.stateEvents" => &[("afterSequence", "integer"), ("limit", "integer")],
            "device.tempoClock" => &[],
            "device.listModels" => &[],
            "device.identity" => &[],
            "device.setDeviceName" => &[("name", "string")],
            "device.undo" => &[("expectedPresetIdentity", "string")],
            "device.redo" => &[("expectedPresetIdentity", "string")],
            "device.inhibitedModules" => &[],
            "device.tunerSettings" => &[],
            "device.setTunerInput" => &[
                ("inputPortId", "integer"),
                ("confirmTunerActivation", "boolean"),
            ],
            "device.setTunerMute" => &[("muted", "boolean"), ("confirmTunerActivation", "boolean")],
            "device.setTunerReference" => &[
                ("referenceOffsetHz", "number"),
                ("confirmTunerActivation", "boolean"),
            ],
            "device.setTunerMeter" => &[
                ("enabled", "boolean"),
                ("confirmTunerActivation", "boolean"),
            ],
            "device.generalSettings" => &[],
            "device.ioSettings" => &[],
            "device.setInputPort" => &[
                ("inputPortId", "integer"),
                ("levelDb", "nullable-number"),
                ("impedance", "nullable-number"),
                ("inputType", "nullable-number"),
                ("groundLift", "nullable-number"),
            ],
            "device.setOutputPort" => &[
                ("outputPortId", "integer"),
                ("level", "nullable-number"),
                ("groundLift", "nullable-number"),
                ("mute", "nullable-boolean"),
            ],
            "device.setUsbPort" => &[
                ("level", "nullable-number"),
                ("headphonesSource", "nullable-number"),
                ("dryWet", "nullable-number"),
            ],
            "device.setMidiThru" => &[("enabled", "boolean")],
            "device.setOutputPairing" => &[
                ("xlr12Linked", "nullable-boolean"),
                ("out34Linked", "nullable-boolean"),
            ],
            "device.globalEq" => &[],
            "device.setGlobalEqBypassed" => &[("bypassed", "boolean")],
            "device.setGlobalEqBand" => &[
                ("band", "integer"),
                ("gain", "nullable-number"),
                ("frequency", "nullable-number"),
                ("q", "nullable-number"),
                ("filterType", "nullable-integer"),
                ("enabled", "nullable-boolean"),
            ],
            "device.setGlobalEqOutput" => &[
                ("level", "nullable-number"),
                ("out12", "nullable-boolean"),
                ("out34", "nullable-boolean"),
            ],
            "device.loadGlobalEqModelPreset" => &[
                ("value", "string"),
                ("isFactory", "boolean"),
                ("hash", "integer"),
            ],
            "device.modeCycle" => &[],
            "device.setModeCycle" => &[("slots", "array")],
            "device.globalTempoSettings" => &[],
            "device.setTempoMetronome" => &[
                ("ledEnabled", "nullable-boolean"),
                ("volumeDb", "nullable-number"),
                ("running", "nullable-boolean"),
                ("pan", "nullable-number"),
                ("timeSignature", "nullable-string"),
                ("subdivision", "nullable-string"),
                ("sound", "nullable-string"),
                ("routing", "nullable-string"),
                ("beats", "nullable-array"),
            ],
            "device.setTempoMode" => &[("mode", "string")],
            "device.setGlobalTempo" => &[
                ("bpm", "integer"),
                ("expectedMode", "string"),
                ("expectedGlobalBpm", "integer"),
            ],
            "device.looperStatus" => &[],
            "device.controlLooper" => &[("command", "string"), ("value", "nullable-integer")],
            "device.recents" => &[],
            "device.favorites" => &[],
            "device.setFavorite" => &[
                ("name", "string"),
                ("folderKey", "string"),
                ("productKey", "string"),
                ("isFactory", "boolean"),
                ("favorite", "boolean"),
                ("expectedFavorite", "boolean"),
                ("isPlugin", "boolean"),
            ],
            "device.pinnedModels" => &[],
            "device.setModelPinned" => &[
                ("modelId", "integer"),
                ("pinned", "boolean"),
                ("expectedPinned", "boolean"),
            ],
            "device.captures" => &[],
            "device.loadCapture" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("key", "string"),
                ("name", "string"),
                ("modelId", "nullable-integer"),
                ("expectedModelId", "nullable-integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.irs" => &[("folder", "nullable-string")],
            "device.loadIr" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("key", "string"),
                ("name", "string"),
                ("slot", "integer"),
                ("modelId", "nullable-integer"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.createSetlist" => &[("name", "string")],
            "device.deleteSetlist" => &[("name", "string")],
            "device.duplicateSetlist" => &[
                ("sourceSetlistKey", "string"),
                ("expectedSourceName", "string"),
            ],
            "device.deletePreset" => &[
                ("setlistKey", "string"),
                ("position", "integer"),
                ("expectedName", "string"),
            ],
            "device.movePreset" => &[
                ("setlistKey", "string"),
                ("sourcePosition", "integer"),
                ("sourceName", "string"),
                ("destinationPosition", "integer"),
            ],
            "device.setGeneralInteger" => &[("setting", "string"), ("value", "integer")],
            "device.setGeneralToggle" => &[("setting", "string"), ("enabled", "boolean")],
            "device.setSceneBypassBehavior" => &[("behavior", "string")],
            "device.setMasterVolumeAssignment" => &[
                ("out12", "boolean"),
                ("out34", "boolean"),
                ("send12", "boolean"),
                ("headphones", "boolean"),
            ],
            "device.setGlobalBypass" => &[("cab", "nullable-array"), ("ir", "nullable-array")],
            "device.presetScreenshot" => &[
                ("folderName", "string"),
                ("position", "integer"),
                ("isFactory", "boolean"),
            ],
            "device.captureScreen" => &[],
            "device.diagnostics" => &[],
            "device.graphicsTree" => &[],
            "device.tapScreen" => &[("x", "integer"), ("y", "integer")],
            "device.longTapScreen" => &[("x", "integer"), ("y", "integer")],
            "device.swipeScreen" => &[
                ("x", "integer"),
                ("y", "integer"),
                ("toX", "integer"),
                ("toY", "integer"),
            ],
            "device.dragScreen" => &[
                ("x", "integer"),
                ("y", "integer"),
                ("toX", "integer"),
                ("toY", "integer"),
                ("steps", "integer"),
                ("holdMs", "integer"),
            ],
            "device.mouseFrame" => &[
                ("type", "string"),
                ("x", "integer"),
                ("y", "integer"),
                ("toX", "nullable-integer"),
                ("toY", "nullable-integer"),
            ],
            "device.selectScene" => &[
                ("scene", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.copyScene" => &[
                ("fromScene", "integer"),
                ("toScene", "integer"),
                ("swap", "boolean"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setSceneLabel" => &[
                ("scene", "integer"),
                ("label", "nullable-string"),
                ("expectedLabel", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setSceneColor" => &[
                ("scene", "integer"),
                ("color", "integer"),
                ("expectedColor", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.toggleBypass" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("expectedScene", "integer"),
                ("expectedBypassed", "boolean"),
                ("desiredBypassed", "boolean"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.moveBlock" => &[
                ("row", "integer"),
                ("fromColumn", "integer"),
                ("toColumn", "integer"),
                ("expectedModelId", "integer"),
                ("toRow", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.addBlock" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("modelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedModelId", "nullable-integer"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.replaceBlock" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("modelId", "integer"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.createModelPreset" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("modelHash", "integer"),
                ("name", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setLooperStompAssignments" => {
                &[("assignments", "array"), ("expectedAssignments", "array")]
            }
            "device.resetBlockParameters" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.removeBlock" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setBlockFootswitch" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("footswitch", "nullable-integer"),
                ("assignmentType", "string"),
                ("expectedFootswitch", "nullable-integer"),
                ("expectedAssignmentType", "nullable-string"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setStompMomentary" => &[
                ("footswitch", "integer"),
                ("momentary", "boolean"),
                ("expectedMomentary", "boolean"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setStompLabel" => &[
                ("footswitch", "integer"),
                ("label", "string"),
                ("expectedLabel", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setMidiOut" => &[
                ("source", "integer"),
                ("messages", "array"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setPresetLoadMidiOut" => &[
                ("messages", "array"),
                ("expectedMessages", "array"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setExpressionBypass" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("pedal", "integer"),
                ("mode", "integer"),
                ("invert", "boolean"),
                ("delayMs", "integer"),
                ("latchEmulation", "boolean"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setChainInput" => &[
                ("row", "integer"),
                ("inputId", "integer"),
                ("expectedInputId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setChainOutput" => &[
                ("row", "integer"),
                ("outputId", "integer"),
                ("expectedOutputId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setChainSplit" => &[
                ("row", "integer"),
                ("splitColumn", "nullable-integer"),
                ("mixColumn", "nullable-integer"),
                ("expectedSplitColumn", "nullable-integer"),
                ("expectedMixColumn", "nullable-integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setSplitMute" => &[
                ("row", "integer"),
                ("muted", "boolean"),
                ("expectedMuted", "boolean"),
                ("expectedScene", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.listPresets" => &[("refresh", "boolean"), ("setlistKey", "nullable-string")],
            "device.listPresetFolders" => &[("refresh", "boolean")],
            "device.navigateBank" => &[
                ("direction", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPosition", "integer"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.recallPreset" => &[
                ("setlistKey", "string"),
                ("position", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPosition", "integer"),
                ("expectedSetlistKey", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.reloadPreset" => &[
                ("expectedPresetName", "string"),
                ("expectedPosition", "integer"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.blockDetails" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.laneControlDetails" => &[
                ("row", "integer"),
                ("control", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.previewParameter" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("parameterIndex", "integer"),
                ("value", "number"),
                ("expectedValue", "number"),
                ("expectedScene", "integer"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.previewLaneControlParameter" => &[
                ("row", "integer"),
                ("control", "string"),
                ("parameterIndex", "integer"),
                ("value", "number"),
                ("expectedValue", "number"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setParameter" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("parameterIndex", "integer"),
                ("value", "number"),
                ("expectedValue", "number"),
                ("expectedScene", "integer"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setLaneControlParameter" => &[
                ("row", "integer"),
                ("control", "string"),
                ("parameterIndex", "integer"),
                ("value", "number"),
                ("expectedValue", "number"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setLaneControlSceneMode" => &[
                ("row", "integer"),
                ("control", "string"),
                ("parameterIndex", "integer"),
                ("enabled", "boolean"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setLaneControlExpression" => &[
                ("row", "integer"),
                ("control", "string"),
                ("parameterIndex", "integer"),
                ("pedal", "integer"),
                ("minimum", "number"),
                ("maximum", "number"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setParameterSceneMode" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("parameterIndex", "integer"),
                ("enabled", "boolean"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setParameterExpression" => &[
                ("row", "integer"),
                ("column", "integer"),
                ("parameterIndex", "integer"),
                ("pedal", "integer"),
                ("minimum", "number"),
                ("maximum", "number"),
                ("expectedModelId", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setTempo" => &[
                ("bpm", "integer"),
                ("expectedTempo", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.setMasterVolume" => &[("value", "integer"), ("expectedValue", "integer")],
            "device.masterVolume" => &[],
            "device.pressFootswitch" => &[
                ("index", "integer"),
                ("expectedMode", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.tapTempo" => &[
                ("expectedMode", "string"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.selectModeSlot" => &[
                ("slot", "integer"),
                ("expectedPresetName", "string"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.listPresetSlots" => &[],
            "device.savePresetAs" => &[
                ("setlistKey", "string"),
                ("position", "integer"),
                ("name", "string"),
                ("expectedPresetName", "string"),
                ("expectedPosition", "integer"),
                ("confirmOverwrite", "boolean"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.copyPreset" => &[
                ("sourceSetlistKey", "string"),
                ("sourcePosition", "integer"),
                ("sourceName", "string"),
                ("destinationSetlistKey", "string"),
                ("destinationPosition", "integer"),
                ("confirmOverwrite", "boolean"),
            ],
            "device.renameCurrentPreset" => &[
                ("name", "string"),
                ("expectedPresetName", "string"),
                ("expectedPosition", "integer"),
                ("confirmRename", "boolean"),
                ("expectedPresetIdentity", "string"),
            ],
            "device.createBackup" => &[("name", "string")],
            "device.showTuner" => &[("shown", "boolean")],
            "device.showGigView" => &[("shown", "boolean")],
            _ => &[],
        };
        for (key, kind) in kinds {
            let Some(value) = object.get(*key) else {
                continue;
            };
            let (nullable, base) = kind
                .strip_prefix("nullable-")
                .map_or((false, *kind), |base| (true, base));
            if nullable && value.is_null() {
                continue;
            }
            let valid = match base {
                "boolean" => value.is_boolean(),
                "integer" => value.is_i64() || value.is_u64(),
                "number" => value.as_f64().is_some_and(f64::is_finite),
                "string" => value.is_string(),
                "array" => value.is_array(),
                _ => false,
            };
            if !valid {
                return Err(format!("{method}.{key} must be {kind}"));
            }
        }
    }
    Ok(())
}

/// Gateway method names as constants, each named after the typed client
/// method that calls it.
#[allow(dead_code)]
pub mod rpc {
    /// Report the gateway runtime's own health and connection status.
    pub const RUNTIME_STATUS: &str = "system.status";
    /// Reconnect the native Quad Cortex transport after explicit confirmation.
    pub const RECONNECT: &str = "device.reconnect";
    /// Reset and re-synchronize the native Quad Cortex communication session after explicit confirmation.
    pub const RESET_SESSION: &str = "device.resetSession";
    /// Close the native Quad Cortex transport after explicit confirmation.
    pub const DISCONNECT: &str = "device.disconnect";
    /// Read the current preset snapshot. It returns the same result kind as `device.currentPreset`.
    pub const CURRENT_SNAPSHOT: &str = "device.snapshot";
    /// Read the continuously synchronized current preset, scene, tempo and Grid state without changing the screen on the Quad Cortex.
    pub const CURRENT_PRESET: &str = "device.currentPreset";
    /// Read buffered native state frames strictly after a non-negative process-lifetime sequence cursor, up to the requested count and a 4 MiB page budget.
    pub const CURRENT_STATE_EVENTS: &str = "device.stateEvents";
    /// Read the latest uncorrelated native metronome status from this device session, including enabled, preroll, beat, bar, tick, and count-in counters.
    pub const CURRENT_TEMPO_CLOCK: &str = "device.tempoClock";
    /// Find block models in the model catalog the Quad Cortex published to this session, with their numeric IDs.
    pub const LIST_MODELS: &str = "device.listModels";
    /// Read the device identity from its Version message.
    pub const IDENTITY: &str = "device.identity";
    /// Change the Quad Cortex custom name (1 to 64 visible characters) after explicit confirmation.
    pub const SET_DEVICE_NAME: &str = "device.setDeviceName";
    /// Undo the most recent device edit after explicit confirmation.
    pub const UNDO: &str = "device.undo";
    /// Redo the most recently undone device edit after explicit confirmation.
    pub const REDO: &str = "device.redo";
    /// Read the authoritative Global Gate and Global EQ inhibition state.
    pub const INHIBITED_MODULES: &str = "device.inhibitedModules";
    /// Read tuner input, mute preference, reference pitch, and any reported meter fields without sending the desktop engagement messages.
    pub const TUNER_SETTINGS: &str = "device.tunerSettings";
    /// Choose the tuner input (id 1, 2, 3, 4, 5, 8 or 9: Input 1, Input 2, Input 1/2, Return 1, Return 2, USB 5, USB 6).
    pub const SET_TUNER_INPUT: &str = "device.setTunerInput";
    /// Change mute-while-tuning; muted=false restores audio and is the only unmute path.
    pub const SET_TUNER_MUTE: &str = "device.setTunerMute";
    /// Set tuner reference as an Hz offset from 440 (-40 to 40).
    pub const SET_TUNER_REFERENCE: &str = "device.setTunerReference";
    /// Enable or disable live tuner-meter reports.
    pub const SET_TUNER_METER: &str = "device.setTunerMeter";
    /// Read the QC Device Settings and System settings without changing them.
    pub const GENERAL_SETTINGS: &str = "device.generalSettings";
    /// Read complete input, output, headphone, USB, MIDI, expression-pedal, connection, and output-pairing settings.
    pub const IO_SETTINGS: &str = "device.ioSettings";
    /// Change one or more settings for a QC input; input gain is expressed safely in dB.
    pub const SET_INPUT_PORT: &str = "device.setInputPort";
    /// Change the dB level, ground lift, or mute state for one output settings port.
    pub const SET_OUTPUT_PORT: &str = "device.setOutputPort";
    /// Change USB level, headphone source, or dry/wet routing using normalized device values.
    pub const SET_USB_PORT: &str = "device.setUsbPort";
    /// Enable or disable the QC MIDI Thru setting after explicit confirmation.
    pub const SET_MIDI_THRU: &str = "device.setMidiThru";
    /// Pair or unpair output couples.
    pub const SET_OUTPUT_PAIRING: &str = "device.setOutputPairing";
    /// Read a Global EQ update: optional bypass state and sparse normalized parameters at indices 0 through 27.
    pub const GLOBAL_EQ: &str = "device.globalEq";
    /// Enable or bypass the global EQ after explicit confirmation.
    pub const SET_GLOBAL_EQ_BYPASSED: &str = "device.setGlobalEqBypassed";
    /// Update one Global EQ band with sparse normalized controls after explicit confirmation.
    pub const SET_GLOBAL_EQ_BAND: &str = "device.setGlobalEqBand";
    /// Update the Global EQ output level in dB and its output-pair assignments.
    pub const SET_GLOBAL_EQ_OUTPUT: &str = "device.setGlobalEqOutput";
    /// Recall a Global EQ model preset by the exact identity the Quad Cortex announced in this session, after explicit confirmation.
    pub const LOAD_GLOBAL_EQ_MODEL_PRESET: &str = "device.loadGlobalEqModelPreset";
    /// Read the configured footswitch modes in cycle order.
    pub const MODE_CYCLE: &str = "device.modeCycle";
    /// Replace the ordered footswitch mode cycle after explicit confirmation.
    pub const SET_MODE_CYCLE: &str = "device.setModeCycle";
    /// Read the device-global tempo block, including PRESET/GLOBAL mode, metronome options, routing, and all thirteen beat cells.
    pub const GLOBAL_TEMPO_SETTINGS: &str = "device.globalTempoSettings";
    /// Change named fields of the tempo/metronome settings.
    pub const SET_TEMPO_METRONOME: &str = "device.setTempoMetronome";
    /// Select whether the Quad Cortex uses the loaded preset tempo or its device-global tempo block.
    pub const SET_TEMPO_MODE: &str = "device.setTempoMode";
    /// Set tempo with ONE type-33 UPDATE frame (GlobalTempo parameter 0), exactly as the desktop application sends it.
    pub const SET_GLOBAL_TEMPO: &str = "device.setGlobalTempo";
    /// Read the complete Looper X transport and progress state when a looper is present.
    pub const LOOPER_STATUS: &str = "device.looperStatus";
    /// Control Looper X through its documented MIDI CC interface.
    pub const CONTROL_LOOPER: &str = "device.controlLooper";
    /// List recently used presets with authoritative folder metadata.
    pub const RECENTS: &str = "device.recents";
    /// List favorite presets using request correlation even when the reply omits its favorites flag.
    pub const FAVORITES: &str = "device.favorites";
    /// Add or remove one exact preset or plugin-preset library entry from Favorites.
    pub const SET_FAVORITE: &str = "device.setFavorite";
    /// List model IDs and capture keys pinned in the device model browser.
    pub const PINNED_MODELS: &str = "device.pinnedModels";
    /// Pin or unpin one numeric Virtual Device model ID.
    pub const SET_MODEL_PINNED: &str = "device.setModelPinned";
    /// List loadable Neural Captures with their content keys and names.
    pub const CAPTURES: &str = "device.captures";
    /// Load a Neural Capture using its authoritative library key.
    pub const LOAD_CAPTURE: &str = "device.loadCapture";
    /// List loadable Impulse Responses from the whole library or a selected device folder.
    pub const IRS: &str = "device.irs";
    /// Retarget one slot of an existing IR Loader using its authoritative library key; name is confirmation metadata and model_id must be null because placement is a separate unsupported workflow.
    pub const LOAD_IR: &str = "device.loadIr";
    /// Create a user setlist after explicit confirmation.
    pub const CREATE_SETLIST: &str = "device.createSetlist";
    /// Delete a named user setlist and its contents after explicit confirmation.
    pub const DELETE_SETLIST: &str = "device.deleteSetlist";
    /// Duplicate a complete device setlist with one atomic folder-copy command.
    pub const DUPLICATE_SETLIST: &str = "device.duplicateSetlist";
    /// Delete the preset at an exact user-setlist position after matching its expected name and explicit confirmation.
    pub const DELETE_PRESET: &str = "device.deletePreset";
    /// Relocate a stored preset within one setlist using its authoritative catalog identity, then verify the fresh catalog after explicit confirmation.
    pub const MOVE_PRESET: &str = "device.movePreset";
    /// Change one validated integer Device Setting after explicit confirmation.
    pub const SET_GENERAL_INTEGER: &str = "device.setGeneralInteger";
    /// Change one validated on/off Device Setting after explicit confirmation.
    pub const SET_GENERAL_TOGGLE: &str = "device.setGeneralToggle";
    /// Change the global scene block-bypass persistence behavior after explicit confirmation.
    pub const SET_SCENE_BYPASS_BEHAVIOR: &str = "device.setSceneBypassBehavior";
    /// Replace all four Master Volume output assignments atomically after explicit confirmation.
    pub const SET_MASTER_VOLUME_ASSIGNMENT: &str = "device.setMasterVolumeAssignment";
    /// Set the global Cab and/or IR bypass row flags (true means bypassed) after explicit confirmation.
    pub const SET_GLOBAL_BYPASS: &str = "device.setGlobalBypass";
    /// Request a schema-defined preset PNG from the device.
    pub const PRESET_SCREENSHOT: &str = "device.presetScreenshot";
    /// Capture the current Quad Cortex touchscreen as a PNG image.
    pub const CAPTURE_SCREEN: &str = "device.captureScreen";
    /// Not currently supported: the Quad Cortex does not answer this request, so the call times out after about 5 seconds and returns no data.
    pub const DEVICE_DIAGNOSTICS: &str = "device.diagnostics";
    /// Read the live Quad Cortex zenUI widget tree for structural screen inspection.
    pub const GRAPHICS_TREE: &str = "device.graphicsTree";
    /// Tap an exact touchscreen pixel with ONE TAP frame, the protocol's built-in tap.
    pub const TAP_SCREEN: &str = "device.tapScreen";
    /// Press and hold an exact touchscreen pixel, for the affordances that answer a hold rather than a tap - Modes Configuration, Splitter/Mixer placement.
    pub const LONG_TAP_SCREEN: &str = "device.longTapScreen";
    /// Swipe between exact touchscreen pixels as ONE DRAG frame, the protocol's built-in pointer drag (drag_screen sends the same frame).
    pub const SWIPE_SCREEN: &str = "device.swipeScreen";
    /// Drag between exact touchscreen pixels with ONE DRAG frame, the protocol's own pointer drag: the same frame, with the same behaviour, as swipe_screen (see it for what was observed: a list flings, a value control follows it, a long vertical drag on the Grid opens Gig View or I/O Settings).
    pub const DRAG_SCREEN: &str = "device.dragScreen";
    /// Send ONE raw RemoteControl mouse frame of the protocol: type press (0), release (1), move (2), tap (3) or drag (4).
    pub const MOUSE_FRAME: &str = "device.mouseFrame";
    /// Select performance scene 0 through 7; an authoritative already-active scene is verified without sending a redundant mutation.
    pub const SELECT_SCENE: &str = "device.selectScene";
    /// Copy one scene of the current preset over another, or swap the two, and verify the observable scene label/color readback.
    pub const COPY_SCENE: &str = "device.copyScene";
    /// Set or clear one scene label after proving its previously read value.
    pub const SET_SCENE_LABEL: &str = "device.setSceneLabel";
    /// Set the display color of one scene in the current preset after checking expected_color against the #rrggbb value from sceneColors in the observed current preset.
    pub const SET_SCENE_COLOR: &str = "device.setSceneColor";
    /// Immediately enable or bypass one Grid block, guarded by expected scene and bypass state.
    pub const TOGGLE_BYPASS: &str = "device.toggleBypass";
    /// Move an existing Grid block to an empty cell, including a destination row, with model and preset guards.
    pub const MOVE_BLOCK: &str = "device.moveBlock";
    /// Add an ordinary model to a confirmed-empty Grid cell and verify the resulting block.
    pub const ADD_BLOCK: &str = "device.addBlock";
    /// Replace one existing Grid block with another ordinary model that the session's model catalog lists and does not hide (the same check as add_block, including STATE_UNAVAILABLE for an unsynchronized catalog).
    pub const REPLACE_BLOCK: &str = "device.replaceBlock";
    /// Request creation of a named user device model preset from the selected Grid block, after explicit confirmation.
    pub const CREATE_MODEL_PRESET: &str = "device.createModelPreset";
    /// Replace the complete Looper X A-H action assignment permutation with ONE type-9 GeneralSettings UPDATE (field 33).
    pub const SET_LOOPER_STOMP_ASSIGNMENTS: &str = "device.setLooperStompAssignments";
    /// Reset every parameter of one Grid block to its stored defaults after validating the block and active preset.
    pub const RESET_BLOCK_PARAMETERS: &str = "device.resetBlockParameters";
    /// Remove a Grid block after validating its model and active preset.
    pub const REMOVE_BLOCK: &str = "device.removeBlock";
    /// Assign or clear a Grid block footswitch with preset, model and Primary/Secondary assignment guards.
    pub const SET_BLOCK_FOOTSWITCH: &str = "device.setBlockFootswitch";
    /// Set a single-block STOMP footswitch to momentary or latching behavior after proving its previously read behavior is still current, then verify device readback.
    pub const SET_STOMP_MOMENTARY: &str = "device.setStompMomentary";
    /// Set a 1-16 character STOMP footswitch label using the device's correct single- or multi-assignment storage after proving its previous label.
    pub const SET_STOMP_LABEL: &str = "device.setStompLabel";
    /// Replace up to 12 MIDI Out messages for a footswitch or expression-pedal source in the current preset.
    pub const SET_MIDI_OUT: &str = "device.setMidiOut";
    /// Replace the MIDI Out messages sent when the current preset loads after proving the previously read list is still current.
    pub const SET_PRESET_LOAD_MIDI_OUT: &str = "device.setPresetLoadMidiOut";
    /// Assign EXP 1 or EXP 2 to a block bypass with the QC switch mode, inversion, delay and latch-emulation settings, or clear the assignment with pedal 0.
    pub const SET_EXPRESSION_BYPASS: &str = "device.setExpressionBypass";
    /// Change a signal-row input after validating the current route and preset.
    pub const SET_CHAIN_INPUT: &str = "device.setChainInput";
    /// Change a signal-row output after validating the current route and preset.
    pub const SET_CHAIN_OUTPUT: &str = "device.setChainOutput";
    /// Set or clear independent split and mixer points on parallel signal rows 0 or 2, which the unit displays as rows 1 and 3, with complete stale-route guards.
    pub const SET_CHAIN_SPLIT: &str = "device.setChainSplit";
    /// Mute or unmute a splitter/mixer path in the selected scene with exact device readback.
    pub const SET_SPLIT_MUTE: &str = "device.setSplitMute";
    /// List presets in a setlist.
    pub const LIST_PRESETS: &str = "device.listPresets";
    /// List preset folders and setlists.
    pub const LIST_PRESET_FOLDERS: &str = "device.listPresetFolders";
    /// Immediately navigate one performance bank down (-1) or up (1).
    pub const NAVIGATE_BANK: &str = "device.navigateBank";
    /// Immediately recall a preset by setlist key and position, guarded by the currently active setlist, preset name, and position.
    pub const RECALL_PRESET: &str = "device.recallPreset";
    /// Resend the synchronized active preset address after the caller sets confirm_risky_operation, then require a newer position revision and clean device state.
    pub const RELOAD_PRESET: &str = "device.reloadPreset";
    /// Read live parameters for one occupied Grid block, splitter, or mixer.
    pub const BLOCK_DETAILS: &str = "device.blockDetails";
    /// Read the Input Gate or Lane Output parameters attached to a signal row.
    pub const LANE_CONTROL_DETAILS: &str = "device.laneControlDetails";
    /// Send a real block, splitter, or mixer parameter mutation and return without readback.
    pub const PREVIEW_PARAMETER: &str = "device.previewParameter";
    /// Send a real Input Gate or Lane Output parameter mutation and return without readback.
    pub const PREVIEW_LANE_CONTROL_PARAMETER: &str = "device.previewLaneControlParameter";
    /// Set one normalized block, splitter, or mixer parameter and wait for the QC's own pushed Grid echo to confirm it; no read is sent while waiting, and when no confirming state is pushed inside the window the result is accepted_unverified (not an error).
    pub const SET_PARAMETER: &str = "device.setParameter";
    /// Set one normalized Input Gate or Lane Output parameter and wait for the QC's own pushed state to confirm it; no read is sent while waiting, and when no confirming state is pushed inside the window the result is accepted_unverified (not an error).
    pub const SET_LANE_CONTROL_PARAMETER: &str = "device.setLaneControlParameter";
    /// Enable or disable per-scene storage for an Input Gate or Lane Output parameter and verify readback.
    pub const SET_LANE_CONTROL_SCENE_MODE: &str = "device.setLaneControlSceneMode";
    /// Assign EXP 1 or EXP 2 to an Input Gate or Lane Output parameter, or clear it with pedal 0, preserving the heel and toe range and verifying authoritative lane state.
    pub const SET_LANE_CONTROL_EXPRESSION: &str = "device.setLaneControlExpression";
    /// Enable or disable per-scene storage for a block, splitter, or mixer parameter and verify device readback.
    pub const SET_PARAMETER_SCENE_MODE: &str = "device.setParameterSceneMode";
    /// Assign EXP 1 or EXP 2 to a block, splitter, or mixer parameter, or clear it with pedal 0, preserving the requested heel and toe range.
    pub const SET_PARAMETER_EXPRESSION: &str = "device.setParameterExpression";
    /// Immediately set performance tempo from 40 through 240 BPM.
    pub const SET_TEMPO: &str = "device.setTempo";
    /// Set master output volume after explicit user confirmation and stale-value validation.
    pub const SET_MASTER_VOLUME: &str = "device.setMasterVolume";
    /// Read the authoritative master output volume as a whole-number percentage of the wire's 0.0 to 1.0 range.
    pub const CURRENT_MASTER_VOLUME: &str = "device.masterVolume";
    /// Send one host MIDI press event for Quad Cortex footswitch A through H (index 0 through 7), guarded by synchronized preset and mode state.
    pub const PRESS_FOOTSWITCH: &str = "device.pressFootswitch";
    /// Send one host MIDI tap event on the Quad Cortex reserved Tempo control (CC 44), guarded by synchronized preset and mode state.
    pub const TAP_TEMPO: &str = "device.tapTempo";
    /// Select device-configured performance Mode Slot A, B, or C through the native HID mode route: one type-14 Mode UPDATE carrying the mode enum the slot holds (0 PRESET, 1 SCENE, 2 STOMP; only enum values 0 to 2 are ever written).
    pub const SELECT_MODE_SLOT: &str = "device.selectModeSlot";
    /// List preset destinations and their occupancy before a persistent write.
    pub const LIST_PRESET_SLOTS: &str = "device.listPresetSlots";
    /// Save the current preset to a reviewed device slot with explicit overwrite confirmation.
    pub const SAVE_PRESET_AS: &str = "device.savePresetAs";
    /// Copy a stored preset to a reviewed destination slot with explicit overwrite confirmation.
    pub const COPY_PRESET: &str = "device.copyPreset";
    /// Rename the active stored preset after explicit confirmation.
    pub const RENAME_CURRENT_PRESET: &str = "device.renameCurrentPreset";
    /// Create a complete local device backup after explicit confirmation.
    pub const CREATE_DEVICE_BACKUP: &str = "device.createBackup";
    /// Request the device's tuner visibility workflow: sends three frames back to back in this order, the type-27 ShowTuner UPDATE, the type-6 Tuner enable_meter UPDATE and a type-6 Tuner READ, which is the sequence the desktop application itself sends.
    pub const SHOW_TUNER: &str = "device.showTuner";
    /// Show or hide Gig View, which takes over the unit's screen in front of the player.
    pub const SHOW_GIG_VIEW: &str = "device.showGigView";
}
