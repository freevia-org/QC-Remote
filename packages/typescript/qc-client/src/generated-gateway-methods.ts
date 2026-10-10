// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
export const GATEWAY_API_VERSION = 31;
export const GATEWAY_CAPABILITIES = ["nativePresetIdentity","modelRepoParameterMetadata","nativeBroker","nativeStateEvents","nativeDeviceIdentity","nativeDiagnostics","nativeRemoteScreen","nativeGraphicsTree","nativeScreenGestures","nativeSceneManagement","nativeParameterAssignments","nativeDefaultParameters","nativeStompMetadata","nativePresetMidiOut","nativeExpressionBypass","nativeTunerSettings","nativeTunerWrites","nativeTunerMeter","nativeLaneControls","nativeGeneralSettings","nativeIoSettings","nativeGlobalEq","nativeModeCycle","nativeLooper","nativeLibraryManagement","nativeSplitMute","nativeTempoMetronome","nativeGlobalTempoWrite"] as const;
export const GATEWAY_METHODS = [
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
] as const;
export type GatewayRpcMethod = typeof GATEWAY_METHODS[number];
export const GATEWAY_RESULT_KINDS = {
  "system.status": "Object",
  "device.reconnect": "Object",
  "device.resetSession": "Object",
  "device.disconnect": "Object",
  "device.snapshot": "PresetSnapshot",
  "device.currentPreset": "PresetSnapshot",
  "device.stateEvents": "Object",
  "device.tempoClock": "Object",
  "device.listModels": "Object",
  "device.identity": "Object",
  "device.setDeviceName": "DeviceActionResult",
  "device.undo": "DeviceActionResult",
  "device.redo": "DeviceActionResult",
  "device.inhibitedModules": "Object",
  "device.tunerSettings": "Object",
  "device.setTunerInput": "DeviceActionResult",
  "device.setTunerMute": "DeviceActionResult",
  "device.setTunerReference": "DeviceActionResult",
  "device.setTunerMeter": "DeviceActionResult",
  "device.generalSettings": "Object",
  "device.ioSettings": "Object",
  "device.setInputPort": "DeviceActionResult",
  "device.setOutputPort": "DeviceActionResult",
  "device.setUsbPort": "DeviceActionResult",
  "device.setMidiThru": "DeviceActionResult",
  "device.setOutputPairing": "DeviceActionResult",
  "device.globalEq": "Object",
  "device.setGlobalEqBypassed": "DeviceActionResult",
  "device.setGlobalEqBand": "DeviceActionResult",
  "device.setGlobalEqOutput": "DeviceActionResult",
  "device.loadGlobalEqModelPreset": "DeviceActionResult",
  "device.modeCycle": "Object",
  "device.setModeCycle": "DeviceActionResult",
  "device.globalTempoSettings": "Object",
  "device.setTempoMetronome": "DeviceActionResult",
  "device.setTempoMode": "DeviceActionResult",
  "device.setGlobalTempo": "DeviceActionResult",
  "device.looperStatus": "Object",
  "device.controlLooper": "DeviceActionResult",
  "device.recents": "Object",
  "device.favorites": "Object",
  "device.setFavorite": "DeviceActionResult",
  "device.pinnedModels": "Object",
  "device.setModelPinned": "DeviceActionResult",
  "device.captures": "Object",
  "device.loadCapture": "DeviceActionResult",
  "device.irs": "Object",
  "device.loadIr": "DeviceActionResult",
  "device.createSetlist": "DeviceActionResult",
  "device.deleteSetlist": "DeviceActionResult",
  "device.duplicateSetlist": "DeviceActionResult",
  "device.deletePreset": "DeviceActionResult",
  "device.movePreset": "DeviceActionResult",
  "device.setGeneralInteger": "DeviceActionResult",
  "device.setGeneralToggle": "DeviceActionResult",
  "device.setSceneBypassBehavior": "DeviceActionResult",
  "device.setMasterVolumeAssignment": "DeviceActionResult",
  "device.setGlobalBypass": "DeviceActionResult",
  "device.presetScreenshot": "Object",
  "device.captureScreen": "Object",
  "device.diagnostics": "Object",
  "device.graphicsTree": "Object",
  "device.tapScreen": "DeviceActionResult",
  "device.longTapScreen": "DeviceActionResult",
  "device.swipeScreen": "DeviceActionResult",
  "device.dragScreen": "DeviceActionResult",
  "device.mouseFrame": "DeviceActionResult",
  "device.selectScene": "DeviceActionResult",
  "device.copyScene": "DeviceActionResult",
  "device.setSceneLabel": "DeviceActionResult",
  "device.setSceneColor": "DeviceActionResult",
  "device.toggleBypass": "DeviceActionResult",
  "device.moveBlock": "DeviceActionResult",
  "device.addBlock": "DeviceActionResult",
  "device.replaceBlock": "DeviceActionResult",
  "device.createModelPreset": "DeviceActionResult",
  "device.setLooperStompAssignments": "DeviceActionResult",
  "device.resetBlockParameters": "DeviceActionResult",
  "device.removeBlock": "DeviceActionResult",
  "device.setBlockFootswitch": "DeviceActionResult",
  "device.setStompMomentary": "DeviceActionResult",
  "device.setStompLabel": "DeviceActionResult",
  "device.setMidiOut": "DeviceActionResult",
  "device.setPresetLoadMidiOut": "DeviceActionResult",
  "device.setExpressionBypass": "DeviceActionResult",
  "device.setChainInput": "DeviceActionResult",
  "device.setChainOutput": "DeviceActionResult",
  "device.setChainSplit": "DeviceActionResult",
  "device.setSplitMute": "DeviceActionResult",
  "device.listPresets": "Object",
  "device.listPresetFolders": "Object",
  "device.navigateBank": "DeviceActionResult",
  "device.recallPreset": "DeviceActionResult",
  "device.reloadPreset": "DeviceActionResult",
  "device.blockDetails": "Object",
  "device.laneControlDetails": "Object",
  "device.previewParameter": "Object",
  "device.previewLaneControlParameter": "Object",
  "device.setParameter": "DeviceActionResult",
  "device.setLaneControlParameter": "DeviceActionResult",
  "device.setLaneControlSceneMode": "DeviceActionResult",
  "device.setLaneControlExpression": "DeviceActionResult",
  "device.setParameterSceneMode": "DeviceActionResult",
  "device.setParameterExpression": "DeviceActionResult",
  "device.setTempo": "DeviceActionResult",
  "device.setMasterVolume": "DeviceActionResult",
  "device.masterVolume": "Object",
  "device.pressFootswitch": "DeviceActionResult",
  "device.tapTempo": "DeviceActionResult",
  "device.selectModeSlot": "DeviceActionResult",
  "device.listPresetSlots": "Object",
  "device.savePresetAs": "DeviceActionResult",
  "device.copyPreset": "DeviceActionResult",
  "device.renameCurrentPreset": "DeviceActionResult",
  "device.createBackup": "Object",
  "device.showTuner": "DeviceActionResult",
  "device.showGigView": "DeviceActionResult"
} as const;
export const GATEWAY_CLIENT_METHODS = {
  "runtimeStatus": { rpc: "system.status", tauri: "runtime_status", params: [], defaults: {} },
  "reconnect": { rpc: "device.reconnect", tauri: "reconnect_device", params: [], defaults: {} },
  "resetSession": { rpc: "device.resetSession", tauri: "reset_device_session", params: [], defaults: {} },
  "disconnect": { rpc: "device.disconnect", tauri: "disconnect_device", params: [], defaults: {} },
  "currentSnapshot": { rpc: "device.snapshot", tauri: "current_snapshot", params: [], defaults: {} },
  "currentPreset": { rpc: "device.currentPreset", tauri: "current_preset", params: [], defaults: {} },
  "currentStateEvents": { rpc: "device.stateEvents", tauri: "current_state_events", params: ["afterSequence","limit"], defaults: {"afterSequence":0,"limit":256} },
  "currentTempoClock": { rpc: "device.tempoClock", tauri: "current_tempo_clock", params: [], defaults: {} },
  "listModels": { rpc: "device.listModels", tauri: "list_models", params: [], defaults: {} },
  "identity": { rpc: "device.identity", tauri: "device_identity", params: [], defaults: {} },
  "setDeviceName": { rpc: "device.setDeviceName", tauri: "set_device_name", params: ["name"], defaults: {} },
  "undo": { rpc: "device.undo", tauri: "undo_device", params: ["expectedPresetIdentity"], defaults: {} },
  "redo": { rpc: "device.redo", tauri: "redo_device", params: ["expectedPresetIdentity"], defaults: {} },
  "inhibitedModules": { rpc: "device.inhibitedModules", tauri: "inhibited_modules", params: [], defaults: {} },
  "tunerSettings": { rpc: "device.tunerSettings", tauri: "tuner_settings", params: [], defaults: {} },
  "setTunerInput": { rpc: "device.setTunerInput", tauri: "set_tuner_input", params: ["inputPortId","confirmTunerActivation"], defaults: {} },
  "setTunerMute": { rpc: "device.setTunerMute", tauri: "set_tuner_mute", params: ["muted","confirmTunerActivation"], defaults: {} },
  "setTunerReference": { rpc: "device.setTunerReference", tauri: "set_tuner_reference", params: ["referenceOffsetHz","confirmTunerActivation"], defaults: {} },
  "setTunerMeter": { rpc: "device.setTunerMeter", tauri: "set_tuner_meter", params: ["enabled","confirmTunerActivation"], defaults: {} },
  "generalSettings": { rpc: "device.generalSettings", tauri: "general_settings", params: [], defaults: {} },
  "ioSettings": { rpc: "device.ioSettings", tauri: "io_settings", params: [], defaults: {} },
  "setInputPort": { rpc: "device.setInputPort", tauri: "set_input_port", params: ["inputPortId","levelDb","impedance","inputType","groundLift"], defaults: {"levelDb":null,"impedance":null,"inputType":null,"groundLift":null} },
  "setOutputPort": { rpc: "device.setOutputPort", tauri: "set_output_port", params: ["outputPortId","level","groundLift","mute"], defaults: {"level":null,"groundLift":null,"mute":null} },
  "setUsbPort": { rpc: "device.setUsbPort", tauri: "set_usb_port", params: ["level","headphonesSource","dryWet"], defaults: {"level":null,"headphonesSource":null,"dryWet":null} },
  "setMidiThru": { rpc: "device.setMidiThru", tauri: "set_midi_thru", params: ["enabled"], defaults: {} },
  "setOutputPairing": { rpc: "device.setOutputPairing", tauri: "set_output_pairing", params: ["xlr12Linked","out34Linked"], defaults: {"xlr12Linked":null,"out34Linked":null} },
  "globalEq": { rpc: "device.globalEq", tauri: "global_eq", params: [], defaults: {} },
  "setGlobalEqBypassed": { rpc: "device.setGlobalEqBypassed", tauri: "set_global_eq_bypassed", params: ["bypassed"], defaults: {} },
  "setGlobalEqBand": { rpc: "device.setGlobalEqBand", tauri: "set_global_eq_band", params: ["band","gain","frequency","q","filterType","enabled"], defaults: {"gain":null,"frequency":null,"q":null,"filterType":null,"enabled":null} },
  "setGlobalEqOutput": { rpc: "device.setGlobalEqOutput", tauri: "set_global_eq_output", params: ["level","out12","out34"], defaults: {"level":null,"out12":null,"out34":null} },
  "loadGlobalEqModelPreset": { rpc: "device.loadGlobalEqModelPreset", tauri: "load_global_eq_model_preset", params: ["value","isFactory","hash"], defaults: {} },
  "modeCycle": { rpc: "device.modeCycle", tauri: "mode_cycle", params: [], defaults: {} },
  "setModeCycle": { rpc: "device.setModeCycle", tauri: "set_mode_cycle", params: ["slots"], defaults: {} },
  "globalTempoSettings": { rpc: "device.globalTempoSettings", tauri: "global_tempo_settings", params: [], defaults: {} },
  "setTempoMetronome": { rpc: "device.setTempoMetronome", tauri: "set_tempo_metronome", params: ["ledEnabled","volumeDb","running","pan","timeSignature","subdivision","sound","routing","beats"], defaults: {"ledEnabled":null,"volumeDb":null,"running":null,"pan":null,"timeSignature":null,"subdivision":null,"sound":null,"routing":null,"beats":null} },
  "setTempoMode": { rpc: "device.setTempoMode", tauri: "set_tempo_mode", params: ["mode"], defaults: {} },
  "setGlobalTempo": { rpc: "device.setGlobalTempo", tauri: "set_global_tempo", params: ["bpm","expectedMode","expectedGlobalBpm"], defaults: {} },
  "looperStatus": { rpc: "device.looperStatus", tauri: "looper_status", params: [], defaults: {} },
  "controlLooper": { rpc: "device.controlLooper", tauri: "control_looper", params: ["command","value"], defaults: {"value":null} },
  "recents": { rpc: "device.recents", tauri: "recents", params: [], defaults: {} },
  "favorites": { rpc: "device.favorites", tauri: "favorites", params: [], defaults: {} },
  "setFavorite": { rpc: "device.setFavorite", tauri: "set_favorite", params: ["name","folderKey","productKey","isFactory","favorite","expectedFavorite","isPlugin"], defaults: {"isPlugin":false} },
  "pinnedModels": { rpc: "device.pinnedModels", tauri: "pinned_models", params: [], defaults: {} },
  "setModelPinned": { rpc: "device.setModelPinned", tauri: "set_model_pinned", params: ["modelId","pinned","expectedPinned"], defaults: {} },
  "captures": { rpc: "device.captures", tauri: "captures", params: [], defaults: {} },
  "loadCapture": { rpc: "device.loadCapture", tauri: "load_capture", params: ["row","column","key","name","modelId","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"modelId":null,"expectedModelId":null,"expectedPresetName":""} },
  "irs": { rpc: "device.irs", tauri: "irs", params: ["folder"], defaults: {"folder":null} },
  "loadIr": { rpc: "device.loadIr", tauri: "load_ir", params: ["row","column","key","name","slot","modelId","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"modelId":null,"expectedPresetName":""} },
  "createSetlist": { rpc: "device.createSetlist", tauri: "create_setlist", params: ["name"], defaults: {} },
  "deleteSetlist": { rpc: "device.deleteSetlist", tauri: "delete_setlist", params: ["name"], defaults: {} },
  "duplicateSetlist": { rpc: "device.duplicateSetlist", tauri: "duplicate_setlist", params: ["sourceSetlistKey","expectedSourceName"], defaults: {} },
  "deletePreset": { rpc: "device.deletePreset", tauri: "delete_preset", params: ["setlistKey","position","expectedName"], defaults: {} },
  "movePreset": { rpc: "device.movePreset", tauri: "move_preset", params: ["setlistKey","sourcePosition","sourceName","destinationPosition"], defaults: {} },
  "setGeneralInteger": { rpc: "device.setGeneralInteger", tauri: "set_general_integer", params: ["setting","value"], defaults: {} },
  "setGeneralToggle": { rpc: "device.setGeneralToggle", tauri: "set_general_toggle", params: ["setting","enabled"], defaults: {} },
  "setSceneBypassBehavior": { rpc: "device.setSceneBypassBehavior", tauri: "set_scene_bypass_behavior", params: ["behavior"], defaults: {} },
  "setMasterVolumeAssignment": { rpc: "device.setMasterVolumeAssignment", tauri: "set_master_volume_assignment", params: ["out12","out34","send12","headphones"], defaults: {} },
  "setGlobalBypass": { rpc: "device.setGlobalBypass", tauri: "set_global_bypass", params: ["cab","ir"], defaults: {"cab":null,"ir":null} },
  "presetScreenshot": { rpc: "device.presetScreenshot", tauri: "preset_screenshot", params: ["folderName","position","isFactory"], defaults: {"isFactory":false} },
  "captureScreen": { rpc: "device.captureScreen", tauri: "capture_screen", params: [], defaults: {} },
  "deviceDiagnostics": { rpc: "device.diagnostics", tauri: "device_diagnostics", params: [], defaults: {} },
  "graphicsTree": { rpc: "device.graphicsTree", tauri: "graphics_tree", params: [], defaults: {} },
  "tapScreen": { rpc: "device.tapScreen", tauri: "tap_screen", params: ["x","y"], defaults: {} },
  "longTapScreen": { rpc: "device.longTapScreen", tauri: "long_tap_screen", params: ["x","y"], defaults: {} },
  "swipeScreen": { rpc: "device.swipeScreen", tauri: "swipe_screen", params: ["x","y","toX","toY"], defaults: {} },
  "dragScreen": { rpc: "device.dragScreen", tauri: "drag_screen", params: ["x","y","toX","toY","steps","holdMs"], defaults: {"steps":18,"holdMs":200} },
  "mouseFrame": { rpc: "device.mouseFrame", tauri: "mouse_frame", params: ["type","x","y","toX","toY"], defaults: {"toX":null,"toY":null} },
  "selectScene": { rpc: "device.selectScene", tauri: "select_scene", params: ["scene","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "copyScene": { rpc: "device.copyScene", tauri: "copy_scene", params: ["fromScene","toScene","swap","expectedPresetName","expectedPresetIdentity"], defaults: {"swap":false,"expectedPresetName":""} },
  "setSceneLabel": { rpc: "device.setSceneLabel", tauri: "set_scene_label", params: ["scene","label","expectedLabel","expectedPresetName","expectedPresetIdentity"], defaults: {"label":null,"expectedPresetName":""} },
  "setSceneColor": { rpc: "device.setSceneColor", tauri: "set_scene_color", params: ["scene","color","expectedColor","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "toggleBypass": { rpc: "device.toggleBypass", tauri: "toggle_bypass", params: ["row","column","expectedScene","expectedBypassed","desiredBypassed","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "moveBlock": { rpc: "device.moveBlock", tauri: "move_block", params: ["row","fromColumn","toColumn","expectedModelId","toRow","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "addBlock": { rpc: "device.addBlock", tauri: "add_block", params: ["row","column","modelId","expectedPresetName","expectedModelId","expectedPresetIdentity"], defaults: {"expectedPresetName":"","expectedModelId":null} },
  "replaceBlock": { rpc: "device.replaceBlock", tauri: "replace_block", params: ["row","column","modelId","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "createModelPreset": { rpc: "device.createModelPreset", tauri: "create_model_preset", params: ["row","column","modelHash","name","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setLooperStompAssignments": { rpc: "device.setLooperStompAssignments", tauri: "set_looper_stomp_assignments", params: ["assignments","expectedAssignments"], defaults: {} },
  "resetBlockParameters": { rpc: "device.resetBlockParameters", tauri: "reset_block_parameters", params: ["row","column","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "removeBlock": { rpc: "device.removeBlock", tauri: "remove_block", params: ["row","column","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setBlockFootswitch": { rpc: "device.setBlockFootswitch", tauri: "set_block_footswitch", params: ["row","column","footswitch","assignmentType","expectedFootswitch","expectedAssignmentType","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"footswitch":null,"expectedFootswitch":null,"expectedAssignmentType":null,"expectedPresetName":""} },
  "setStompMomentary": { rpc: "device.setStompMomentary", tauri: "set_stomp_momentary", params: ["footswitch","momentary","expectedMomentary","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setStompLabel": { rpc: "device.setStompLabel", tauri: "set_stomp_label", params: ["footswitch","label","expectedLabel","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setMidiOut": { rpc: "device.setMidiOut", tauri: "set_midi_out", params: ["source","messages","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setPresetLoadMidiOut": { rpc: "device.setPresetLoadMidiOut", tauri: "set_preset_load_midi_out", params: ["messages","expectedMessages","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setExpressionBypass": { rpc: "device.setExpressionBypass", tauri: "set_expression_bypass", params: ["row","column","pedal","mode","invert","delayMs","latchEmulation","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setChainInput": { rpc: "device.setChainInput", tauri: "set_chain_input", params: ["row","inputId","expectedInputId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setChainOutput": { rpc: "device.setChainOutput", tauri: "set_chain_output", params: ["row","outputId","expectedOutputId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setChainSplit": { rpc: "device.setChainSplit", tauri: "set_chain_split", params: ["row","splitColumn","mixColumn","expectedSplitColumn","expectedMixColumn","expectedPresetName","expectedPresetIdentity"], defaults: {"splitColumn":null,"mixColumn":null,"expectedSplitColumn":null,"expectedMixColumn":null,"expectedPresetName":""} },
  "setSplitMute": { rpc: "device.setSplitMute", tauri: "set_split_mute", params: ["row","muted","expectedMuted","expectedScene","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "listPresets": { rpc: "device.listPresets", tauri: "list_presets", params: ["refresh","setlistKey"], defaults: {"refresh":false,"setlistKey":null} },
  "listPresetFolders": { rpc: "device.listPresetFolders", tauri: "list_preset_folders", params: ["refresh"], defaults: {"refresh":false} },
  "navigateBank": { rpc: "device.navigateBank", tauri: "navigate_bank", params: ["direction","expectedPresetName","expectedPosition","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "recallPreset": { rpc: "device.recallPreset", tauri: "recall_preset", params: ["setlistKey","position","expectedPresetName","expectedPosition","expectedSetlistKey","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "reloadPreset": { rpc: "device.reloadPreset", tauri: "reload_preset", params: ["expectedPresetName","expectedPosition","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "blockDetails": { rpc: "device.blockDetails", tauri: "block_details", params: ["row","column","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "laneControlDetails": { rpc: "device.laneControlDetails", tauri: "lane_control_details", params: ["row","control","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "previewParameter": { rpc: "device.previewParameter", tauri: "preview_parameter", params: ["row","column","parameterIndex","value","expectedValue","expectedScene","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "previewLaneControlParameter": { rpc: "device.previewLaneControlParameter", tauri: "preview_lane_control_parameter", params: ["row","control","parameterIndex","value","expectedValue","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setParameter": { rpc: "device.setParameter", tauri: "set_parameter", params: ["row","column","parameterIndex","value","expectedValue","expectedScene","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setLaneControlParameter": { rpc: "device.setLaneControlParameter", tauri: "set_lane_control_parameter", params: ["row","control","parameterIndex","value","expectedValue","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setLaneControlSceneMode": { rpc: "device.setLaneControlSceneMode", tauri: "set_lane_control_scene_mode", params: ["row","control","parameterIndex","enabled","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setLaneControlExpression": { rpc: "device.setLaneControlExpression", tauri: "set_lane_control_expression", params: ["row","control","parameterIndex","pedal","minimum","maximum","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setParameterSceneMode": { rpc: "device.setParameterSceneMode", tauri: "set_parameter_scene_mode", params: ["row","column","parameterIndex","enabled","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setParameterExpression": { rpc: "device.setParameterExpression", tauri: "set_parameter_expression", params: ["row","column","parameterIndex","pedal","minimum","maximum","expectedModelId","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setTempo": { rpc: "device.setTempo", tauri: "set_tempo", params: ["bpm","expectedTempo","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "setMasterVolume": { rpc: "device.setMasterVolume", tauri: "set_master_volume", params: ["value","expectedValue"], defaults: {} },
  "currentMasterVolume": { rpc: "device.masterVolume", tauri: "current_master_volume", params: [], defaults: {} },
  "pressFootswitch": { rpc: "device.pressFootswitch", tauri: "press_footswitch", params: ["index","expectedMode","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "tapTempo": { rpc: "device.tapTempo", tauri: "tap_tempo", params: ["expectedMode","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "selectModeSlot": { rpc: "device.selectModeSlot", tauri: "select_mode_slot", params: ["slot","expectedPresetName","expectedPresetIdentity"], defaults: {"expectedPresetName":""} },
  "listPresetSlots": { rpc: "device.listPresetSlots", tauri: "list_preset_slots", params: [], defaults: {} },
  "savePresetAs": { rpc: "device.savePresetAs", tauri: "save_preset_as", params: ["setlistKey","position","name","expectedPresetName","expectedPosition","confirmOverwrite","expectedPresetIdentity"], defaults: {"expectedPresetName":"","confirmOverwrite":false} },
  "copyPreset": { rpc: "device.copyPreset", tauri: "copy_preset", params: ["sourceSetlistKey","sourcePosition","sourceName","destinationSetlistKey","destinationPosition","confirmOverwrite"], defaults: {"sourceName":"","confirmOverwrite":false} },
  "renameCurrentPreset": { rpc: "device.renameCurrentPreset", tauri: "rename_current_preset", params: ["name","expectedPresetName","expectedPosition","confirmRename","expectedPresetIdentity"], defaults: {"expectedPresetName":"","confirmRename":false} },
  "createDeviceBackup": { rpc: "device.createBackup", tauri: "create_device_backup", params: ["name"], defaults: {"name":""} },
  "showTuner": { rpc: "device.showTuner", tauri: "show_tuner", params: ["shown"], defaults: {"shown":true} },
  "showGigView": { rpc: "device.showGigView", tauri: "show_gig_view", params: ["shown"], defaults: {"shown":true} }
} as const;
export type GatewayClientMethod = keyof typeof GATEWAY_CLIENT_METHODS;
export type GatewayClientInvoker = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export class GatewayClientError extends Error {
  readonly code: string;
  readonly retryable: boolean;
  constructor(message: string, code = "DEVICE_ERROR", retryable = false) {
    super(message);
    this.name = "GatewayClientError";
    this.code = code;
    this.retryable = retryable;
  }
}

function gatewayClientError(error: unknown): GatewayClientError {
  if (error instanceof GatewayClientError) return error;
  if (error !== null && typeof error === "object") {
    const value = error as Record<string, unknown>;
    return new GatewayClientError(
      typeof value.message === "string" ? value.message : "The gateway request failed.",
      typeof value.code === "string" ? value.code : "DEVICE_ERROR",
      value.retryable === true || (value.data !== null && typeof value.data === "object"
        && (value.data as Record<string, unknown>).retryable === true),
    );
  }
  return new GatewayClientError(typeof error === "string" ? error : "The gateway request failed.");
}

/** The Device Settings this gateway can write. The hand-written union these
 * replace had drifted: it omitted `midiClockOut`, so a TypeScript caller could
 * read that setting and had no way to express writing it back. */
export const GENERAL_INTEGER_SETTINGS = ["screenBrightness","ledBrightness","dimmedLedBrightness","holdTiming","midiChannel","midiClockOut"] as const;
export type GeneralIntegerSetting = typeof GENERAL_INTEGER_SETTINGS[number];
export const GENERAL_TOGGLE_SETTINGS = ["lockScreenAndVolumeKnob","midiOverUsb","ignoreDuplicatePc","stompModeAutoAssign","swapTempoTunerAccess","disableInternetConnectionCheck","dynamicDelayCompensation","presetDimmed","sceneDimmed","stompDimmed","midiClockIn","gigViewStompAccess"] as const;
export type GeneralToggleSetting = typeof GENERAL_TOGGLE_SETTINGS[number];
/** Booleans `generalSettings` projects on read that no write path accepts. */
export const GENERAL_READ_ONLY_BOOLEANS = ["internalMidiClockEnabled"] as const;

const hasOwn = (value: Record<string, unknown>, key: string) => Object.prototype.hasOwnProperty.call(value, key);

/** Verification values a `verified: true` result may carry. */
export const DEVICE_ACTION_VERIFIED_VERIFICATIONS: readonly string[] = ["authoritative_readback","authoritative_reconnect_readback","authoritative_preflight"];
/** Verification values a `verified: false` result may carry. */
export const DEVICE_ACTION_UNVERIFIED_VERIFICATIONS: readonly string[] = ["accepted_unverified"];
/** Outcomes that sent no mutation, so they may never report `changed: true`. */
export const DEVICE_ACTION_UNCHANGED_VERIFICATIONS: readonly string[] = ["authoritative_preflight"];
/** Outcomes that observed nothing, so they may not report `changed` at all. */
export const DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS: readonly string[] = ["accepted_unverified"];

/** Reject partial or contradictory command acknowledgements at every TS host boundary. */
export function validateGatewayResult<T>(command: string, result: T): T {
  if (result === undefined) throw new TypeError(`${command} returned no result`);
  const kind = GATEWAY_RESULT_KINDS[command as GatewayRpcMethod];
  if (!kind) throw new TypeError(`Unknown gateway result contract: ${command}`);
  if (result === null || typeof result !== "object" || Array.isArray(result)) {
    throw new TypeError(`${command} returned a malformed ${kind} result`);
  }
  const value = result as Record<string, unknown>;
  if (kind === "PresetSnapshot" && (typeof value.presetName !== "string" || !Array.isArray(value.blocks))) {
    throw new TypeError(`${command} returned a malformed PresetSnapshot result`);
  }
  const hasOutcome = hasOwn(value, "accepted") || hasOwn(value, "verified") || hasOwn(value, "verification");
  if (kind === "DeviceActionResult" && !hasOutcome) {
    throw new TypeError(`${command} returned a device action result without verification semantics`);
  }
  if (!hasOutcome) return result;
  const verified = value.verified;
  const verification = value.verification;
  const allowed = verified === true
    ? DEVICE_ACTION_VERIFIED_VERIFICATIONS
    : DEVICE_ACTION_UNVERIFIED_VERIFICATIONS;
  if (value.accepted !== true || typeof verified !== "boolean"
      || typeof verification !== "string" || !allowed.includes(verification)
      || typeof value.detail !== "string" || [...value.detail].length > 4096) {
    throw new TypeError(`${command} returned a malformed device action result`);
  }
  const changed = hasOwn(value, "changed") ? value.changed : undefined;
  if (changed !== undefined && typeof changed !== "boolean") {
    throw new TypeError(`${command} returned a malformed device action result`);
  }
  if (DEVICE_ACTION_UNCHANGED_VERIFICATIONS.includes(verification) && changed === true) {
    throw new TypeError(`${command} reported ${verification} while claiming it changed device state`);
  }
  if (DEVICE_ACTION_CHANGE_BLIND_VERIFICATIONS.includes(verification) && changed !== undefined) {
    throw new TypeError(`${command} reported ${verification} while claiming to know whether device state changed`);
  }
  if (command === "device.duplicateSetlist"
      && (typeof value.setlistKey !== "string" || value.setlistKey.length === 0)) {
    throw new TypeError("device.duplicateSetlist returned no authoritative created setlist key");
  }
  return result;
}

/** Build a typed client from the contract instead of mirroring every native method by hand. */
export function createGatewayClientTransport<TTransport extends object>(invoke: GatewayClientInvoker, target: "tauri" | "rpc" = "rpc"): TTransport {
  return Object.fromEntries(Object.entries(GATEWAY_CLIENT_METHODS).map(([client, descriptor]) => [
    client,
    (...values: unknown[]) => {
      const defaults: Readonly<Record<string, unknown>> = descriptor.defaults;
      const args = Object.fromEntries(descriptor.params.map((name, index) =>
        [name, values[index] === undefined ? defaults[name] : values[index]]));
      const command = descriptor[target];
      return invoke(command, args)
        .then((result) => validateGatewayResult(descriptor.rpc, result))
        .catch((error) => { throw gatewayClientError(error); });
    }
  ])) as TTransport;
}
