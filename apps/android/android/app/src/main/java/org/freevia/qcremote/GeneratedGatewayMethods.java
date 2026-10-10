// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
package org.freevia.qcremote;

final class GeneratedGatewayMethods {
    private GeneratedGatewayMethods() {}

    static final int API_VERSION = 31;
    static final String[] CAPABILITIES = {
        "nativePresetIdentity",
        "modelRepoParameterMetadata",
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
        "nativeGateway",
        "androidUsbRelay"
    };

    static final String[] ALL = {
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
        "device.showGigView"
    };

    /** Verification values a verified:true result may carry. */
    static final String[] DEVICE_ACTION_VERIFIED_VERIFICATIONS = { "authoritative_readback", "authoritative_reconnect_readback", "authoritative_preflight" };
    /** Verification values a verified:false result may carry. */
    static final String[] DEVICE_ACTION_UNVERIFIED_VERIFICATIONS = { "accepted_unverified" };
    /** Outcomes that sent no mutation, so they may never report changed:true. */
    static final String[] DEVICE_ACTION_UNCHANGED_VERIFICATIONS = { "authoritative_preflight" };
    /** Outcomes that observed nothing, so they may not report changed at all. */
    static final String[] DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS = { "accepted_unverified" };

    static boolean contains(String method) {
        for (String candidate : ALL) if (candidate.equals(method)) return true;
        return false;
    }

    private static boolean includes(String[] values, String value) {
        for (String candidate : values) if (candidate.equals(value)) return true;
        return false;
    }

    static String dispatchKind(String method) {
        switch (method) {
            case "system.status": return "SYSTEM";
            case "device.reconnect": return "RECONNECT";
            case "device.resetSession": return "RECONNECT";
            case "device.disconnect": return "DISCONNECT";
            case "device.snapshot": return "SNAPSHOT";
            case "device.currentPreset": return "CURRENT_PRESET";
            case "device.stateEvents": return "STATE_EVENTS";
            case "device.tempoClock": return "TEMPO_CLOCK";
            case "device.listModels": return "MODELS";
            case "device.identity": return "CORRELATED_READ";
            case "device.setDeviceName": return "SET_DEVICE_NAME";
            case "device.undo": return "PLANNED_WRITE";
            case "device.redo": return "PLANNED_WRITE";
            case "device.inhibitedModules": return "CORRELATED_READ";
            case "device.tunerSettings": return "CORRELATED_READ";
            case "device.setTunerInput": return "PLANNED_WRITE";
            case "device.setTunerMute": return "PLANNED_WRITE";
            case "device.setTunerReference": return "PLANNED_WRITE";
            case "device.setTunerMeter": return "PLANNED_WRITE";
            case "device.generalSettings": return "GENERAL_SETTINGS";
            case "device.ioSettings": return "IO_SETTINGS";
            case "device.setInputPort": return "PLANNED_WRITE";
            case "device.setOutputPort": return "PLANNED_WRITE";
            case "device.setUsbPort": return "PLANNED_WRITE";
            case "device.setMidiThru": return "PLANNED_WRITE";
            case "device.setOutputPairing": return "PLANNED_WRITE";
            case "device.globalEq": return "CORRELATED_READ";
            case "device.setGlobalEqBypassed": return "PLANNED_WRITE";
            case "device.setGlobalEqBand": return "PLANNED_WRITE";
            case "device.setGlobalEqOutput": return "PLANNED_WRITE";
            case "device.loadGlobalEqModelPreset": return "PLANNED_WRITE";
            case "device.modeCycle": return "CORRELATED_READ";
            case "device.setModeCycle": return "PLANNED_WRITE";
            case "device.globalTempoSettings": return "CORRELATED_READ";
            case "device.setTempoMetronome": return "PLANNED_WRITE";
            case "device.setTempoMode": return "PLANNED_WRITE";
            case "device.setGlobalTempo": return "PLANNED_WRITE";
            case "device.looperStatus": return "CORRELATED_READ";
            case "device.controlLooper": return "PLANNED_WRITE";
            case "device.recents": return "CORRELATED_READ";
            case "device.favorites": return "CORRELATED_READ";
            case "device.setFavorite": return "PLANNED_WRITE";
            case "device.pinnedModels": return "CORRELATED_READ";
            case "device.setModelPinned": return "PLANNED_WRITE";
            case "device.captures": return "RETAINED_CATALOG";
            case "device.loadCapture": return "PLANNED_WRITE";
            case "device.irs": return "RETAINED_CATALOG";
            case "device.loadIr": return "PLANNED_WRITE";
            case "device.createSetlist": return "PERSISTENT_WRITE";
            case "device.deleteSetlist": return "PERSISTENT_WRITE";
            case "device.duplicateSetlist": return "PERSISTENT_WRITE";
            case "device.deletePreset": return "PERSISTENT_WRITE";
            case "device.movePreset": return "PERSISTENT_WRITE";
            case "device.setGeneralInteger": return "PLANNED_WRITE";
            case "device.setGeneralToggle": return "PLANNED_WRITE";
            case "device.setSceneBypassBehavior": return "PLANNED_WRITE";
            case "device.setMasterVolumeAssignment": return "PLANNED_WRITE";
            case "device.setGlobalBypass": return "PLANNED_WRITE";
            case "device.presetScreenshot": return "CORRELATED_READ";
            case "device.captureScreen": return "CORRELATED_READ";
            case "device.diagnostics": return "CORRELATED_READ";
            case "device.graphicsTree": return "CORRELATED_READ";
            case "device.tapScreen": return "TAP_SCREEN";
            case "device.longTapScreen": return "TAP_SCREEN";
            case "device.swipeScreen": return "TAP_SCREEN";
            case "device.dragScreen": return "TAP_SCREEN";
            case "device.mouseFrame": return "TAP_SCREEN";
            case "device.selectScene": return "PLANNED_WRITE";
            case "device.copyScene": return "PLANNED_WRITE";
            case "device.setSceneLabel": return "PLANNED_WRITE";
            case "device.setSceneColor": return "PLANNED_WRITE";
            case "device.toggleBypass": return "PLANNED_WRITE";
            case "device.moveBlock": return "PLANNED_WRITE";
            case "device.addBlock": return "PLANNED_WRITE";
            case "device.replaceBlock": return "PLANNED_WRITE";
            case "device.createModelPreset": return "PLANNED_WRITE";
            case "device.setLooperStompAssignments": return "PLANNED_WRITE";
            case "device.resetBlockParameters": return "PLANNED_WRITE";
            case "device.removeBlock": return "PLANNED_WRITE";
            case "device.setBlockFootswitch": return "PLANNED_WRITE";
            case "device.setStompMomentary": return "PLANNED_WRITE";
            case "device.setStompLabel": return "PLANNED_WRITE";
            case "device.setMidiOut": return "PLANNED_WRITE";
            case "device.setPresetLoadMidiOut": return "PLANNED_WRITE";
            case "device.setExpressionBypass": return "PLANNED_WRITE";
            case "device.setChainInput": return "PLANNED_WRITE";
            case "device.setChainOutput": return "PLANNED_WRITE";
            case "device.setChainSplit": return "PLANNED_WRITE";
            case "device.setSplitMute": return "PLANNED_WRITE";
            case "device.listPresets": return "RETAINED_CATALOG";
            case "device.listPresetFolders": return "RETAINED_CATALOG";
            case "device.navigateBank": return "PRESET_WRITE";
            case "device.recallPreset": return "PRESET_WRITE";
            case "device.reloadPreset": return "PRESET_WRITE";
            case "device.blockDetails": return "BLOCK_DETAILS";
            case "device.laneControlDetails": return "BLOCK_DETAILS";
            case "device.previewParameter": return "PREVIEW_PARAMETER";
            case "device.previewLaneControlParameter": return "PREVIEW_PARAMETER";
            case "device.setParameter": return "PLANNED_WRITE";
            case "device.setLaneControlParameter": return "PLANNED_WRITE";
            case "device.setLaneControlSceneMode": return "PLANNED_WRITE";
            case "device.setLaneControlExpression": return "PLANNED_WRITE";
            case "device.setParameterSceneMode": return "PLANNED_WRITE";
            case "device.setParameterExpression": return "PLANNED_WRITE";
            case "device.setTempo": return "PLANNED_WRITE";
            case "device.setMasterVolume": return "PLANNED_WRITE";
            case "device.masterVolume": return "MASTER_VOLUME";
            case "device.pressFootswitch": return "PLANNED_WRITE";
            case "device.tapTempo": return "PLANNED_WRITE";
            case "device.selectModeSlot": return "PLANNED_WRITE";
            case "device.listPresetSlots": return "RETAINED_CATALOG";
            case "device.savePresetAs": return "PERSISTENT_WRITE";
            case "device.copyPreset": return "PERSISTENT_WRITE";
            case "device.renameCurrentPreset": return "PERSISTENT_WRITE";
            case "device.createBackup": return "BACKUP";
            case "device.showTuner": return "PLANNED_WRITE";
            case "device.showGigView": return "PLANNED_WRITE";
            default: return "UNSUPPORTED";
        }
    }

    static String resultKind(String method) {
        switch (method) {
            case "system.status": return "Object";
            case "device.reconnect": return "Object";
            case "device.resetSession": return "Object";
            case "device.disconnect": return "Object";
            case "device.snapshot": return "PresetSnapshot";
            case "device.currentPreset": return "PresetSnapshot";
            case "device.stateEvents": return "Object";
            case "device.tempoClock": return "Object";
            case "device.listModels": return "Object";
            case "device.identity": return "Object";
            case "device.setDeviceName": return "DeviceActionResult";
            case "device.undo": return "DeviceActionResult";
            case "device.redo": return "DeviceActionResult";
            case "device.inhibitedModules": return "Object";
            case "device.tunerSettings": return "Object";
            case "device.setTunerInput": return "DeviceActionResult";
            case "device.setTunerMute": return "DeviceActionResult";
            case "device.setTunerReference": return "DeviceActionResult";
            case "device.setTunerMeter": return "DeviceActionResult";
            case "device.generalSettings": return "Object";
            case "device.ioSettings": return "Object";
            case "device.setInputPort": return "DeviceActionResult";
            case "device.setOutputPort": return "DeviceActionResult";
            case "device.setUsbPort": return "DeviceActionResult";
            case "device.setMidiThru": return "DeviceActionResult";
            case "device.setOutputPairing": return "DeviceActionResult";
            case "device.globalEq": return "Object";
            case "device.setGlobalEqBypassed": return "DeviceActionResult";
            case "device.setGlobalEqBand": return "DeviceActionResult";
            case "device.setGlobalEqOutput": return "DeviceActionResult";
            case "device.loadGlobalEqModelPreset": return "DeviceActionResult";
            case "device.modeCycle": return "Object";
            case "device.setModeCycle": return "DeviceActionResult";
            case "device.globalTempoSettings": return "Object";
            case "device.setTempoMetronome": return "DeviceActionResult";
            case "device.setTempoMode": return "DeviceActionResult";
            case "device.setGlobalTempo": return "DeviceActionResult";
            case "device.looperStatus": return "Object";
            case "device.controlLooper": return "DeviceActionResult";
            case "device.recents": return "Object";
            case "device.favorites": return "Object";
            case "device.setFavorite": return "DeviceActionResult";
            case "device.pinnedModels": return "Object";
            case "device.setModelPinned": return "DeviceActionResult";
            case "device.captures": return "Object";
            case "device.loadCapture": return "DeviceActionResult";
            case "device.irs": return "Object";
            case "device.loadIr": return "DeviceActionResult";
            case "device.createSetlist": return "DeviceActionResult";
            case "device.deleteSetlist": return "DeviceActionResult";
            case "device.duplicateSetlist": return "DeviceActionResult";
            case "device.deletePreset": return "DeviceActionResult";
            case "device.movePreset": return "DeviceActionResult";
            case "device.setGeneralInteger": return "DeviceActionResult";
            case "device.setGeneralToggle": return "DeviceActionResult";
            case "device.setSceneBypassBehavior": return "DeviceActionResult";
            case "device.setMasterVolumeAssignment": return "DeviceActionResult";
            case "device.setGlobalBypass": return "DeviceActionResult";
            case "device.presetScreenshot": return "Object";
            case "device.captureScreen": return "Object";
            case "device.diagnostics": return "Object";
            case "device.graphicsTree": return "Object";
            case "device.tapScreen": return "DeviceActionResult";
            case "device.longTapScreen": return "DeviceActionResult";
            case "device.swipeScreen": return "DeviceActionResult";
            case "device.dragScreen": return "DeviceActionResult";
            case "device.mouseFrame": return "DeviceActionResult";
            case "device.selectScene": return "DeviceActionResult";
            case "device.copyScene": return "DeviceActionResult";
            case "device.setSceneLabel": return "DeviceActionResult";
            case "device.setSceneColor": return "DeviceActionResult";
            case "device.toggleBypass": return "DeviceActionResult";
            case "device.moveBlock": return "DeviceActionResult";
            case "device.addBlock": return "DeviceActionResult";
            case "device.replaceBlock": return "DeviceActionResult";
            case "device.createModelPreset": return "DeviceActionResult";
            case "device.setLooperStompAssignments": return "DeviceActionResult";
            case "device.resetBlockParameters": return "DeviceActionResult";
            case "device.removeBlock": return "DeviceActionResult";
            case "device.setBlockFootswitch": return "DeviceActionResult";
            case "device.setStompMomentary": return "DeviceActionResult";
            case "device.setStompLabel": return "DeviceActionResult";
            case "device.setMidiOut": return "DeviceActionResult";
            case "device.setPresetLoadMidiOut": return "DeviceActionResult";
            case "device.setExpressionBypass": return "DeviceActionResult";
            case "device.setChainInput": return "DeviceActionResult";
            case "device.setChainOutput": return "DeviceActionResult";
            case "device.setChainSplit": return "DeviceActionResult";
            case "device.setSplitMute": return "DeviceActionResult";
            case "device.listPresets": return "Object";
            case "device.listPresetFolders": return "Object";
            case "device.navigateBank": return "DeviceActionResult";
            case "device.recallPreset": return "DeviceActionResult";
            case "device.reloadPreset": return "DeviceActionResult";
            case "device.blockDetails": return "Object";
            case "device.laneControlDetails": return "Object";
            case "device.previewParameter": return "Object";
            case "device.previewLaneControlParameter": return "Object";
            case "device.setParameter": return "DeviceActionResult";
            case "device.setLaneControlParameter": return "DeviceActionResult";
            case "device.setLaneControlSceneMode": return "DeviceActionResult";
            case "device.setLaneControlExpression": return "DeviceActionResult";
            case "device.setParameterSceneMode": return "DeviceActionResult";
            case "device.setParameterExpression": return "DeviceActionResult";
            case "device.setTempo": return "DeviceActionResult";
            case "device.setMasterVolume": return "DeviceActionResult";
            case "device.masterVolume": return "Object";
            case "device.pressFootswitch": return "DeviceActionResult";
            case "device.tapTempo": return "DeviceActionResult";
            case "device.selectModeSlot": return "DeviceActionResult";
            case "device.listPresetSlots": return "Object";
            case "device.savePresetAs": return "DeviceActionResult";
            case "device.copyPreset": return "DeviceActionResult";
            case "device.renameCurrentPreset": return "DeviceActionResult";
            case "device.createBackup": return "Object";
            case "device.showTuner": return "DeviceActionResult";
            case "device.showGigView": return "DeviceActionResult";
            default: return "Unsupported";
        }
    }

    static void validateResult(String method, org.json.JSONObject result) {
        String kind = resultKind(method);
        if ("Unsupported".equals(kind)) throw new IllegalArgumentException("Unsupported gateway method: " + method);
        if ("PresetSnapshot".equals(kind)
                && (!(result.opt("presetName") instanceof String) || !(result.opt("blocks") instanceof org.json.JSONArray)))
            throw new IllegalArgumentException(method + " returned a malformed PresetSnapshot result");
        boolean hasOutcome = result.has("accepted") || result.has("verified") || result.has("verification");
        if ("DeviceActionResult".equals(kind) && !hasOutcome)
            throw new IllegalArgumentException(method + " returned a device action result without verification semantics");
        if (hasOutcome) {
            Object accepted = result.opt("accepted");
            Object verified = result.opt("verified");
            String[] allowed = Boolean.TRUE.equals(verified) ? DEVICE_ACTION_VERIFIED_VERIFICATIONS
                    : Boolean.FALSE.equals(verified) ? DEVICE_ACTION_UNVERIFIED_VERIFICATIONS : new String[0];
            String verification = result.optString("verification", "");
            if (!Boolean.TRUE.equals(accepted) || !(verified instanceof Boolean)
                    || !includes(allowed, verification)
                    || !(result.opt("detail") instanceof String) || result.optString("detail").codePointCount(0, result.optString("detail").length()) > 4096)
                throw new IllegalArgumentException(method + " returned a malformed device action result");
            if (result.has("changed") && !(result.opt("changed") instanceof Boolean))
                throw new IllegalArgumentException(method + " returned a malformed device action result");
            if (includes(DEVICE_ACTION_UNCHANGED_VERIFICATIONS, verification) && Boolean.TRUE.equals(result.opt("changed")))
                throw new IllegalArgumentException(
                    method + " reported " + verification + " while claiming it changed device state");
            if (includes(DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS, verification) && result.has("changed"))
                throw new IllegalArgumentException(
                    method + " reported " + verification + " while claiming to know whether device state changed");
            if ("device.duplicateSetlist".equals(method)
                    && (!(result.opt("setlistKey") instanceof String) || result.optString("setlistKey").isEmpty()))
                throw new IllegalArgumentException(
                    "device.duplicateSetlist returned no authoritative created setlist key");
        }
    }

    static void validateParams(String method, org.json.JSONObject params) {
        final String[] allowed;
        final String[] required;
        switch (method) {
            case "system.status": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.reconnect": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.resetSession": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.disconnect": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.snapshot": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.currentPreset": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.stateEvents": allowed = new String[] { "afterSequence", "limit" }; required = new String[] {  }; break;
            case "device.tempoClock": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.listModels": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.identity": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setDeviceName": allowed = new String[] { "name" }; required = new String[] { "name" }; break;
            case "device.undo": allowed = new String[] { "expectedPresetIdentity" }; required = new String[] { "expectedPresetIdentity" }; break;
            case "device.redo": allowed = new String[] { "expectedPresetIdentity" }; required = new String[] { "expectedPresetIdentity" }; break;
            case "device.inhibitedModules": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.tunerSettings": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setTunerInput": allowed = new String[] { "inputPortId", "confirmTunerActivation" }; required = new String[] { "inputPortId", "confirmTunerActivation" }; break;
            case "device.setTunerMute": allowed = new String[] { "muted", "confirmTunerActivation" }; required = new String[] { "muted", "confirmTunerActivation" }; break;
            case "device.setTunerReference": allowed = new String[] { "referenceOffsetHz", "confirmTunerActivation" }; required = new String[] { "referenceOffsetHz", "confirmTunerActivation" }; break;
            case "device.setTunerMeter": allowed = new String[] { "enabled", "confirmTunerActivation" }; required = new String[] { "enabled", "confirmTunerActivation" }; break;
            case "device.generalSettings": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.ioSettings": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setInputPort": allowed = new String[] { "inputPortId", "levelDb", "impedance", "inputType", "groundLift" }; required = new String[] { "inputPortId", "levelDb", "impedance", "inputType", "groundLift" }; break;
            case "device.setOutputPort": allowed = new String[] { "outputPortId", "level", "groundLift", "mute" }; required = new String[] { "outputPortId", "level", "groundLift", "mute" }; break;
            case "device.setUsbPort": allowed = new String[] { "level", "headphonesSource", "dryWet" }; required = new String[] { "level", "headphonesSource", "dryWet" }; break;
            case "device.setMidiThru": allowed = new String[] { "enabled" }; required = new String[] { "enabled" }; break;
            case "device.setOutputPairing": allowed = new String[] { "xlr12Linked", "out34Linked" }; required = new String[] { "xlr12Linked", "out34Linked" }; break;
            case "device.globalEq": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setGlobalEqBypassed": allowed = new String[] { "bypassed" }; required = new String[] { "bypassed" }; break;
            case "device.setGlobalEqBand": allowed = new String[] { "band", "gain", "frequency", "q", "filterType", "enabled" }; required = new String[] { "band", "gain", "frequency", "q", "filterType", "enabled" }; break;
            case "device.setGlobalEqOutput": allowed = new String[] { "level", "out12", "out34" }; required = new String[] { "level", "out12", "out34" }; break;
            case "device.loadGlobalEqModelPreset": allowed = new String[] { "value", "isFactory", "hash" }; required = new String[] { "value", "isFactory", "hash" }; break;
            case "device.modeCycle": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setModeCycle": allowed = new String[] { "slots" }; required = new String[] { "slots" }; break;
            case "device.globalTempoSettings": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setTempoMetronome": allowed = new String[] { "ledEnabled", "volumeDb", "running", "pan", "timeSignature", "subdivision", "sound", "routing", "beats" }; required = new String[] { "ledEnabled", "volumeDb", "running", "pan", "timeSignature", "subdivision", "sound", "routing", "beats" }; break;
            case "device.setTempoMode": allowed = new String[] { "mode" }; required = new String[] { "mode" }; break;
            case "device.setGlobalTempo": allowed = new String[] { "bpm", "expectedMode", "expectedGlobalBpm" }; required = new String[] { "bpm", "expectedMode", "expectedGlobalBpm" }; break;
            case "device.looperStatus": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.controlLooper": allowed = new String[] { "command", "value" }; required = new String[] { "command", "value" }; break;
            case "device.recents": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.favorites": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setFavorite": allowed = new String[] { "name", "folderKey", "productKey", "isFactory", "favorite", "expectedFavorite", "isPlugin" }; required = new String[] { "name", "folderKey", "productKey", "isFactory", "favorite", "expectedFavorite" }; break;
            case "device.pinnedModels": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.setModelPinned": allowed = new String[] { "modelId", "pinned", "expectedPinned" }; required = new String[] { "modelId", "pinned", "expectedPinned" }; break;
            case "device.captures": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.loadCapture": allowed = new String[] { "row", "column", "key", "name", "modelId", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "key", "name", "modelId", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.irs": allowed = new String[] { "folder" }; required = new String[] { "folder" }; break;
            case "device.loadIr": allowed = new String[] { "row", "column", "key", "name", "slot", "modelId", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "key", "name", "slot", "modelId", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.createSetlist": allowed = new String[] { "name" }; required = new String[] { "name" }; break;
            case "device.deleteSetlist": allowed = new String[] { "name" }; required = new String[] { "name" }; break;
            case "device.duplicateSetlist": allowed = new String[] { "sourceSetlistKey", "expectedSourceName" }; required = new String[] { "sourceSetlistKey", "expectedSourceName" }; break;
            case "device.deletePreset": allowed = new String[] { "setlistKey", "position", "expectedName" }; required = new String[] { "setlistKey", "position", "expectedName" }; break;
            case "device.movePreset": allowed = new String[] { "setlistKey", "sourcePosition", "sourceName", "destinationPosition" }; required = new String[] { "setlistKey", "sourcePosition", "sourceName", "destinationPosition" }; break;
            case "device.setGeneralInteger": allowed = new String[] { "setting", "value" }; required = new String[] { "setting", "value" }; break;
            case "device.setGeneralToggle": allowed = new String[] { "setting", "enabled" }; required = new String[] { "setting", "enabled" }; break;
            case "device.setSceneBypassBehavior": allowed = new String[] { "behavior" }; required = new String[] { "behavior" }; break;
            case "device.setMasterVolumeAssignment": allowed = new String[] { "out12", "out34", "send12", "headphones" }; required = new String[] { "out12", "out34", "send12", "headphones" }; break;
            case "device.setGlobalBypass": allowed = new String[] { "cab", "ir" }; required = new String[] { "cab", "ir" }; break;
            case "device.presetScreenshot": allowed = new String[] { "folderName", "position", "isFactory" }; required = new String[] { "folderName", "position" }; break;
            case "device.captureScreen": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.diagnostics": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.graphicsTree": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.tapScreen": allowed = new String[] { "x", "y" }; required = new String[] { "x", "y" }; break;
            case "device.longTapScreen": allowed = new String[] { "x", "y" }; required = new String[] { "x", "y" }; break;
            case "device.swipeScreen": allowed = new String[] { "x", "y", "toX", "toY" }; required = new String[] { "x", "y", "toX", "toY" }; break;
            case "device.dragScreen": allowed = new String[] { "x", "y", "toX", "toY", "steps", "holdMs" }; required = new String[] { "x", "y", "toX", "toY" }; break;
            case "device.mouseFrame": allowed = new String[] { "type", "x", "y", "toX", "toY" }; required = new String[] { "type", "x", "y" }; break;
            case "device.selectScene": allowed = new String[] { "scene", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "scene", "expectedPresetIdentity" }; break;
            case "device.copyScene": allowed = new String[] { "fromScene", "toScene", "swap", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "fromScene", "toScene", "expectedPresetIdentity" }; break;
            case "device.setSceneLabel": allowed = new String[] { "scene", "label", "expectedLabel", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "scene", "label", "expectedLabel", "expectedPresetIdentity" }; break;
            case "device.setSceneColor": allowed = new String[] { "scene", "color", "expectedColor", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "scene", "color", "expectedColor", "expectedPresetIdentity" }; break;
            case "device.toggleBypass": allowed = new String[] { "row", "column", "expectedScene", "expectedBypassed", "desiredBypassed", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "expectedScene", "expectedBypassed", "desiredBypassed", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.moveBlock": allowed = new String[] { "row", "fromColumn", "toColumn", "expectedModelId", "toRow", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "fromColumn", "toColumn", "expectedModelId", "toRow", "expectedPresetIdentity" }; break;
            case "device.addBlock": allowed = new String[] { "row", "column", "modelId", "expectedPresetName", "expectedModelId", "expectedPresetIdentity" }; required = new String[] { "row", "column", "modelId", "expectedPresetIdentity" }; break;
            case "device.replaceBlock": allowed = new String[] { "row", "column", "modelId", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "modelId", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.createModelPreset": allowed = new String[] { "row", "column", "modelHash", "name", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "modelHash", "name", "expectedPresetIdentity" }; break;
            case "device.setLooperStompAssignments": allowed = new String[] { "assignments", "expectedAssignments" }; required = new String[] { "assignments", "expectedAssignments" }; break;
            case "device.resetBlockParameters": allowed = new String[] { "row", "column", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.removeBlock": allowed = new String[] { "row", "column", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.setBlockFootswitch": allowed = new String[] { "row", "column", "footswitch", "assignmentType", "expectedFootswitch", "expectedAssignmentType", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "footswitch", "assignmentType", "expectedFootswitch", "expectedAssignmentType", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.setStompMomentary": allowed = new String[] { "footswitch", "momentary", "expectedMomentary", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "footswitch", "momentary", "expectedMomentary", "expectedPresetIdentity" }; break;
            case "device.setStompLabel": allowed = new String[] { "footswitch", "label", "expectedLabel", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "footswitch", "label", "expectedLabel", "expectedPresetIdentity" }; break;
            case "device.setMidiOut": allowed = new String[] { "source", "messages", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "source", "messages", "expectedPresetIdentity" }; break;
            case "device.setPresetLoadMidiOut": allowed = new String[] { "messages", "expectedMessages", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "messages", "expectedMessages", "expectedPresetIdentity" }; break;
            case "device.setExpressionBypass": allowed = new String[] { "row", "column", "pedal", "mode", "invert", "delayMs", "latchEmulation", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "pedal", "mode", "invert", "delayMs", "latchEmulation", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.setChainInput": allowed = new String[] { "row", "inputId", "expectedInputId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "inputId", "expectedInputId", "expectedPresetIdentity" }; break;
            case "device.setChainOutput": allowed = new String[] { "row", "outputId", "expectedOutputId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "outputId", "expectedOutputId", "expectedPresetIdentity" }; break;
            case "device.setChainSplit": allowed = new String[] { "row", "splitColumn", "mixColumn", "expectedSplitColumn", "expectedMixColumn", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "splitColumn", "mixColumn", "expectedSplitColumn", "expectedMixColumn", "expectedPresetIdentity" }; break;
            case "device.setSplitMute": allowed = new String[] { "row", "muted", "expectedMuted", "expectedScene", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "muted", "expectedMuted", "expectedScene", "expectedPresetIdentity" }; break;
            case "device.listPresets": allowed = new String[] { "refresh", "setlistKey" }; required = new String[] { "setlistKey" }; break;
            case "device.listPresetFolders": allowed = new String[] { "refresh" }; required = new String[] {  }; break;
            case "device.navigateBank": allowed = new String[] { "direction", "expectedPresetName", "expectedPosition", "expectedPresetIdentity" }; required = new String[] { "direction", "expectedPosition", "expectedPresetIdentity" }; break;
            case "device.recallPreset": allowed = new String[] { "setlistKey", "position", "expectedPresetName", "expectedPosition", "expectedSetlistKey", "expectedPresetIdentity" }; required = new String[] { "setlistKey", "position", "expectedPosition", "expectedSetlistKey", "expectedPresetIdentity" }; break;
            case "device.reloadPreset": allowed = new String[] { "expectedPresetName", "expectedPosition", "expectedPresetIdentity" }; required = new String[] { "expectedPosition", "expectedPresetIdentity" }; break;
            case "device.blockDetails": allowed = new String[] { "row", "column", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "expectedPresetIdentity" }; break;
            case "device.laneControlDetails": allowed = new String[] { "row", "control", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "control", "expectedPresetIdentity" }; break;
            case "device.previewParameter": allowed = new String[] { "row", "column", "parameterIndex", "value", "expectedValue", "expectedScene", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "parameterIndex", "value", "expectedValue", "expectedScene", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.previewLaneControlParameter": allowed = new String[] { "row", "control", "parameterIndex", "value", "expectedValue", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "control", "parameterIndex", "value", "expectedValue", "expectedPresetIdentity" }; break;
            case "device.setParameter": allowed = new String[] { "row", "column", "parameterIndex", "value", "expectedValue", "expectedScene", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "parameterIndex", "value", "expectedValue", "expectedScene", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.setLaneControlParameter": allowed = new String[] { "row", "control", "parameterIndex", "value", "expectedValue", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "control", "parameterIndex", "value", "expectedValue", "expectedPresetIdentity" }; break;
            case "device.setLaneControlSceneMode": allowed = new String[] { "row", "control", "parameterIndex", "enabled", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "control", "parameterIndex", "enabled", "expectedPresetIdentity" }; break;
            case "device.setLaneControlExpression": allowed = new String[] { "row", "control", "parameterIndex", "pedal", "minimum", "maximum", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "control", "parameterIndex", "pedal", "minimum", "maximum", "expectedPresetIdentity" }; break;
            case "device.setParameterSceneMode": allowed = new String[] { "row", "column", "parameterIndex", "enabled", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "parameterIndex", "enabled", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.setParameterExpression": allowed = new String[] { "row", "column", "parameterIndex", "pedal", "minimum", "maximum", "expectedModelId", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "row", "column", "parameterIndex", "pedal", "minimum", "maximum", "expectedModelId", "expectedPresetIdentity" }; break;
            case "device.setTempo": allowed = new String[] { "bpm", "expectedTempo", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "bpm", "expectedTempo", "expectedPresetIdentity" }; break;
            case "device.setMasterVolume": allowed = new String[] { "value", "expectedValue" }; required = new String[] { "value", "expectedValue" }; break;
            case "device.masterVolume": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.pressFootswitch": allowed = new String[] { "index", "expectedMode", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "index", "expectedMode", "expectedPresetIdentity" }; break;
            case "device.tapTempo": allowed = new String[] { "expectedMode", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "expectedMode", "expectedPresetIdentity" }; break;
            case "device.selectModeSlot": allowed = new String[] { "slot", "expectedPresetName", "expectedPresetIdentity" }; required = new String[] { "slot", "expectedPresetIdentity" }; break;
            case "device.listPresetSlots": allowed = new String[] {  }; required = new String[] {  }; break;
            case "device.savePresetAs": allowed = new String[] { "setlistKey", "position", "name", "expectedPresetName", "expectedPosition", "confirmOverwrite", "expectedPresetIdentity" }; required = new String[] { "setlistKey", "position", "name", "expectedPosition", "expectedPresetIdentity" }; break;
            case "device.copyPreset": allowed = new String[] { "sourceSetlistKey", "sourcePosition", "sourceName", "destinationSetlistKey", "destinationPosition", "confirmOverwrite" }; required = new String[] { "sourceSetlistKey", "sourcePosition", "destinationSetlistKey", "destinationPosition" }; break;
            case "device.renameCurrentPreset": allowed = new String[] { "name", "expectedPresetName", "expectedPosition", "confirmRename", "expectedPresetIdentity" }; required = new String[] { "name", "expectedPosition", "expectedPresetIdentity" }; break;
            case "device.createBackup": allowed = new String[] { "name" }; required = new String[] {  }; break;
            case "device.showTuner": allowed = new String[] { "shown" }; required = new String[] {  }; break;
            case "device.showGigView": allowed = new String[] { "shown" }; required = new String[] {  }; break;
            default: throw new IllegalArgumentException("Unsupported gateway method: " + method);
        }
        java.util.Iterator<String> keys = params.keys();
        while (keys.hasNext()) {
            String key = keys.next();
            boolean known = false;
            for (String candidate : allowed) if (candidate.equals(key)) { known = true; break; }
            if (!known) throw new IllegalArgumentException("Unexpected argument for " + method + ": " + key);
        }
        for (String key : required) {
            if (!params.has(key)) throw new IllegalArgumentException("Missing required argument for " + method + ": " + key);
        }
        final String[][] kinds;
        switch (method) {
            case "system.status": kinds = new String[][] {  }; break;
            case "device.reconnect": kinds = new String[][] {  }; break;
            case "device.resetSession": kinds = new String[][] {  }; break;
            case "device.disconnect": kinds = new String[][] {  }; break;
            case "device.snapshot": kinds = new String[][] {  }; break;
            case "device.currentPreset": kinds = new String[][] {  }; break;
            case "device.stateEvents": kinds = new String[][] { { "afterSequence", "integer" }, { "limit", "integer" } }; break;
            case "device.tempoClock": kinds = new String[][] {  }; break;
            case "device.listModels": kinds = new String[][] {  }; break;
            case "device.identity": kinds = new String[][] {  }; break;
            case "device.setDeviceName": kinds = new String[][] { { "name", "string" } }; break;
            case "device.undo": kinds = new String[][] { { "expectedPresetIdentity", "string" } }; break;
            case "device.redo": kinds = new String[][] { { "expectedPresetIdentity", "string" } }; break;
            case "device.inhibitedModules": kinds = new String[][] {  }; break;
            case "device.tunerSettings": kinds = new String[][] {  }; break;
            case "device.setTunerInput": kinds = new String[][] { { "inputPortId", "integer" }, { "confirmTunerActivation", "boolean" } }; break;
            case "device.setTunerMute": kinds = new String[][] { { "muted", "boolean" }, { "confirmTunerActivation", "boolean" } }; break;
            case "device.setTunerReference": kinds = new String[][] { { "referenceOffsetHz", "number" }, { "confirmTunerActivation", "boolean" } }; break;
            case "device.setTunerMeter": kinds = new String[][] { { "enabled", "boolean" }, { "confirmTunerActivation", "boolean" } }; break;
            case "device.generalSettings": kinds = new String[][] {  }; break;
            case "device.ioSettings": kinds = new String[][] {  }; break;
            case "device.setInputPort": kinds = new String[][] { { "inputPortId", "integer" }, { "levelDb", "nullable-number" }, { "impedance", "nullable-number" }, { "inputType", "nullable-number" }, { "groundLift", "nullable-number" } }; break;
            case "device.setOutputPort": kinds = new String[][] { { "outputPortId", "integer" }, { "level", "nullable-number" }, { "groundLift", "nullable-number" }, { "mute", "nullable-boolean" } }; break;
            case "device.setUsbPort": kinds = new String[][] { { "level", "nullable-number" }, { "headphonesSource", "nullable-number" }, { "dryWet", "nullable-number" } }; break;
            case "device.setMidiThru": kinds = new String[][] { { "enabled", "boolean" } }; break;
            case "device.setOutputPairing": kinds = new String[][] { { "xlr12Linked", "nullable-boolean" }, { "out34Linked", "nullable-boolean" } }; break;
            case "device.globalEq": kinds = new String[][] {  }; break;
            case "device.setGlobalEqBypassed": kinds = new String[][] { { "bypassed", "boolean" } }; break;
            case "device.setGlobalEqBand": kinds = new String[][] { { "band", "integer" }, { "gain", "nullable-number" }, { "frequency", "nullable-number" }, { "q", "nullable-number" }, { "filterType", "nullable-integer" }, { "enabled", "nullable-boolean" } }; break;
            case "device.setGlobalEqOutput": kinds = new String[][] { { "level", "nullable-number" }, { "out12", "nullable-boolean" }, { "out34", "nullable-boolean" } }; break;
            case "device.loadGlobalEqModelPreset": kinds = new String[][] { { "value", "string" }, { "isFactory", "boolean" }, { "hash", "integer" } }; break;
            case "device.modeCycle": kinds = new String[][] {  }; break;
            case "device.setModeCycle": kinds = new String[][] { { "slots", "array" } }; break;
            case "device.globalTempoSettings": kinds = new String[][] {  }; break;
            case "device.setTempoMetronome": kinds = new String[][] { { "ledEnabled", "nullable-boolean" }, { "volumeDb", "nullable-number" }, { "running", "nullable-boolean" }, { "pan", "nullable-number" }, { "timeSignature", "nullable-string" }, { "subdivision", "nullable-string" }, { "sound", "nullable-string" }, { "routing", "nullable-string" }, { "beats", "nullable-array" } }; break;
            case "device.setTempoMode": kinds = new String[][] { { "mode", "string" } }; break;
            case "device.setGlobalTempo": kinds = new String[][] { { "bpm", "integer" }, { "expectedMode", "string" }, { "expectedGlobalBpm", "integer" } }; break;
            case "device.looperStatus": kinds = new String[][] {  }; break;
            case "device.controlLooper": kinds = new String[][] { { "command", "string" }, { "value", "nullable-integer" } }; break;
            case "device.recents": kinds = new String[][] {  }; break;
            case "device.favorites": kinds = new String[][] {  }; break;
            case "device.setFavorite": kinds = new String[][] { { "name", "string" }, { "folderKey", "string" }, { "productKey", "string" }, { "isFactory", "boolean" }, { "favorite", "boolean" }, { "expectedFavorite", "boolean" }, { "isPlugin", "boolean" } }; break;
            case "device.pinnedModels": kinds = new String[][] {  }; break;
            case "device.setModelPinned": kinds = new String[][] { { "modelId", "integer" }, { "pinned", "boolean" }, { "expectedPinned", "boolean" } }; break;
            case "device.captures": kinds = new String[][] {  }; break;
            case "device.loadCapture": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "key", "string" }, { "name", "string" }, { "modelId", "nullable-integer" }, { "expectedModelId", "nullable-integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.irs": kinds = new String[][] { { "folder", "nullable-string" } }; break;
            case "device.loadIr": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "key", "string" }, { "name", "string" }, { "slot", "integer" }, { "modelId", "nullable-integer" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.createSetlist": kinds = new String[][] { { "name", "string" } }; break;
            case "device.deleteSetlist": kinds = new String[][] { { "name", "string" } }; break;
            case "device.duplicateSetlist": kinds = new String[][] { { "sourceSetlistKey", "string" }, { "expectedSourceName", "string" } }; break;
            case "device.deletePreset": kinds = new String[][] { { "setlistKey", "string" }, { "position", "integer" }, { "expectedName", "string" } }; break;
            case "device.movePreset": kinds = new String[][] { { "setlistKey", "string" }, { "sourcePosition", "integer" }, { "sourceName", "string" }, { "destinationPosition", "integer" } }; break;
            case "device.setGeneralInteger": kinds = new String[][] { { "setting", "string" }, { "value", "integer" } }; break;
            case "device.setGeneralToggle": kinds = new String[][] { { "setting", "string" }, { "enabled", "boolean" } }; break;
            case "device.setSceneBypassBehavior": kinds = new String[][] { { "behavior", "string" } }; break;
            case "device.setMasterVolumeAssignment": kinds = new String[][] { { "out12", "boolean" }, { "out34", "boolean" }, { "send12", "boolean" }, { "headphones", "boolean" } }; break;
            case "device.setGlobalBypass": kinds = new String[][] { { "cab", "nullable-array" }, { "ir", "nullable-array" } }; break;
            case "device.presetScreenshot": kinds = new String[][] { { "folderName", "string" }, { "position", "integer" }, { "isFactory", "boolean" } }; break;
            case "device.captureScreen": kinds = new String[][] {  }; break;
            case "device.diagnostics": kinds = new String[][] {  }; break;
            case "device.graphicsTree": kinds = new String[][] {  }; break;
            case "device.tapScreen": kinds = new String[][] { { "x", "integer" }, { "y", "integer" } }; break;
            case "device.longTapScreen": kinds = new String[][] { { "x", "integer" }, { "y", "integer" } }; break;
            case "device.swipeScreen": kinds = new String[][] { { "x", "integer" }, { "y", "integer" }, { "toX", "integer" }, { "toY", "integer" } }; break;
            case "device.dragScreen": kinds = new String[][] { { "x", "integer" }, { "y", "integer" }, { "toX", "integer" }, { "toY", "integer" }, { "steps", "integer" }, { "holdMs", "integer" } }; break;
            case "device.mouseFrame": kinds = new String[][] { { "type", "string" }, { "x", "integer" }, { "y", "integer" }, { "toX", "nullable-integer" }, { "toY", "nullable-integer" } }; break;
            case "device.selectScene": kinds = new String[][] { { "scene", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.copyScene": kinds = new String[][] { { "fromScene", "integer" }, { "toScene", "integer" }, { "swap", "boolean" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setSceneLabel": kinds = new String[][] { { "scene", "integer" }, { "label", "nullable-string" }, { "expectedLabel", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setSceneColor": kinds = new String[][] { { "scene", "integer" }, { "color", "integer" }, { "expectedColor", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.toggleBypass": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "expectedScene", "integer" }, { "expectedBypassed", "boolean" }, { "desiredBypassed", "boolean" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.moveBlock": kinds = new String[][] { { "row", "integer" }, { "fromColumn", "integer" }, { "toColumn", "integer" }, { "expectedModelId", "integer" }, { "toRow", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.addBlock": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "modelId", "integer" }, { "expectedPresetName", "string" }, { "expectedModelId", "nullable-integer" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.replaceBlock": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "modelId", "integer" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.createModelPreset": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "modelHash", "integer" }, { "name", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setLooperStompAssignments": kinds = new String[][] { { "assignments", "array" }, { "expectedAssignments", "array" } }; break;
            case "device.resetBlockParameters": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.removeBlock": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setBlockFootswitch": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "footswitch", "nullable-integer" }, { "assignmentType", "string" }, { "expectedFootswitch", "nullable-integer" }, { "expectedAssignmentType", "nullable-string" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setStompMomentary": kinds = new String[][] { { "footswitch", "integer" }, { "momentary", "boolean" }, { "expectedMomentary", "boolean" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setStompLabel": kinds = new String[][] { { "footswitch", "integer" }, { "label", "string" }, { "expectedLabel", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setMidiOut": kinds = new String[][] { { "source", "integer" }, { "messages", "array" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setPresetLoadMidiOut": kinds = new String[][] { { "messages", "array" }, { "expectedMessages", "array" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setExpressionBypass": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "pedal", "integer" }, { "mode", "integer" }, { "invert", "boolean" }, { "delayMs", "integer" }, { "latchEmulation", "boolean" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setChainInput": kinds = new String[][] { { "row", "integer" }, { "inputId", "integer" }, { "expectedInputId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setChainOutput": kinds = new String[][] { { "row", "integer" }, { "outputId", "integer" }, { "expectedOutputId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setChainSplit": kinds = new String[][] { { "row", "integer" }, { "splitColumn", "nullable-integer" }, { "mixColumn", "nullable-integer" }, { "expectedSplitColumn", "nullable-integer" }, { "expectedMixColumn", "nullable-integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setSplitMute": kinds = new String[][] { { "row", "integer" }, { "muted", "boolean" }, { "expectedMuted", "boolean" }, { "expectedScene", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.listPresets": kinds = new String[][] { { "refresh", "boolean" }, { "setlistKey", "nullable-string" } }; break;
            case "device.listPresetFolders": kinds = new String[][] { { "refresh", "boolean" } }; break;
            case "device.navigateBank": kinds = new String[][] { { "direction", "integer" }, { "expectedPresetName", "string" }, { "expectedPosition", "integer" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.recallPreset": kinds = new String[][] { { "setlistKey", "string" }, { "position", "integer" }, { "expectedPresetName", "string" }, { "expectedPosition", "integer" }, { "expectedSetlistKey", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.reloadPreset": kinds = new String[][] { { "expectedPresetName", "string" }, { "expectedPosition", "integer" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.blockDetails": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.laneControlDetails": kinds = new String[][] { { "row", "integer" }, { "control", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.previewParameter": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "parameterIndex", "integer" }, { "value", "number" }, { "expectedValue", "number" }, { "expectedScene", "integer" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.previewLaneControlParameter": kinds = new String[][] { { "row", "integer" }, { "control", "string" }, { "parameterIndex", "integer" }, { "value", "number" }, { "expectedValue", "number" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setParameter": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "parameterIndex", "integer" }, { "value", "number" }, { "expectedValue", "number" }, { "expectedScene", "integer" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setLaneControlParameter": kinds = new String[][] { { "row", "integer" }, { "control", "string" }, { "parameterIndex", "integer" }, { "value", "number" }, { "expectedValue", "number" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setLaneControlSceneMode": kinds = new String[][] { { "row", "integer" }, { "control", "string" }, { "parameterIndex", "integer" }, { "enabled", "boolean" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setLaneControlExpression": kinds = new String[][] { { "row", "integer" }, { "control", "string" }, { "parameterIndex", "integer" }, { "pedal", "integer" }, { "minimum", "number" }, { "maximum", "number" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setParameterSceneMode": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "parameterIndex", "integer" }, { "enabled", "boolean" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setParameterExpression": kinds = new String[][] { { "row", "integer" }, { "column", "integer" }, { "parameterIndex", "integer" }, { "pedal", "integer" }, { "minimum", "number" }, { "maximum", "number" }, { "expectedModelId", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setTempo": kinds = new String[][] { { "bpm", "integer" }, { "expectedTempo", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.setMasterVolume": kinds = new String[][] { { "value", "integer" }, { "expectedValue", "integer" } }; break;
            case "device.masterVolume": kinds = new String[][] {  }; break;
            case "device.pressFootswitch": kinds = new String[][] { { "index", "integer" }, { "expectedMode", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.tapTempo": kinds = new String[][] { { "expectedMode", "string" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.selectModeSlot": kinds = new String[][] { { "slot", "integer" }, { "expectedPresetName", "string" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.listPresetSlots": kinds = new String[][] {  }; break;
            case "device.savePresetAs": kinds = new String[][] { { "setlistKey", "string" }, { "position", "integer" }, { "name", "string" }, { "expectedPresetName", "string" }, { "expectedPosition", "integer" }, { "confirmOverwrite", "boolean" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.copyPreset": kinds = new String[][] { { "sourceSetlistKey", "string" }, { "sourcePosition", "integer" }, { "sourceName", "string" }, { "destinationSetlistKey", "string" }, { "destinationPosition", "integer" }, { "confirmOverwrite", "boolean" } }; break;
            case "device.renameCurrentPreset": kinds = new String[][] { { "name", "string" }, { "expectedPresetName", "string" }, { "expectedPosition", "integer" }, { "confirmRename", "boolean" }, { "expectedPresetIdentity", "string" } }; break;
            case "device.createBackup": kinds = new String[][] { { "name", "string" } }; break;
            case "device.showTuner": kinds = new String[][] { { "shown", "boolean" } }; break;
            case "device.showGigView": kinds = new String[][] { { "shown", "boolean" } }; break;
            default: kinds = new String[0][0];
        }
        for (String[] entry : kinds) {
            if (!params.has(entry[0])) continue;
            Object value = params.opt(entry[0]);
            boolean nullable = entry[1].startsWith("nullable-");
            String base = nullable ? entry[1].substring(9) : entry[1];
            if (nullable && value == org.json.JSONObject.NULL) continue;
            boolean valid = ("boolean".equals(base) && value instanceof Boolean)
                || ("integer".equals(base) && (value instanceof Byte || value instanceof Short || value instanceof Integer || value instanceof Long))
                || ("number".equals(base) && value instanceof Number && Double.isFinite(((Number) value).doubleValue()))
                || ("string".equals(base) && value instanceof String)
                || ("array".equals(base) && value instanceof org.json.JSONArray);
            if (!valid) throw new IllegalArgumentException(method + "." + entry[0] + " must be " + entry[1]);
        }
    }
}
