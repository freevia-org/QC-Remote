import type { GridBlock, PresetSnapshot } from "@qc-remote/client";
import { QC_SCENE_COLORS } from "@qc-remote/core";
import { QC_COLORS } from "@qc-remote/theme";
import { COROS_410_BLOCK_NAMES, OFFICIAL_BRIT_2203_BLOCK_NAMES, OFFICIAL_BRIT_2203_PRESET_NAME } from "./device-reference/coros-fixture-snapshot-names.ts";

export type CorOsScreenView = "editor-overlord-synth" | "editor-overlord-synth-oscillator" | "grid" | "grid-official-brit" | "reference-device-browser-root" | "reference-device-browser-top" | "reference-device-browser-models" | "reference-device-browser-models-clean" | "capture-type" | "device-browser-middle-deep" | "device-browser-middle-reverb" | "device-browser-base" | "device-browser-dsp-limited" | "gig" | "gig-live-tuner" | "gig-view-preset" | "gig-view-scene" | "gig-view-hybrid" | "gig-official-preset" | "gig-official-scene" | "gig-official-stomp" | "gig-official-hybrid" | "tuner" | "tuner-live-enabled" | "tempo" | "midi-out" | "cpu-monitor" | "io-overview" | "io-input" | "io-output" | "io-send-return" | "io-usb" | "io-headphones" | "global-eq" | "global-eq-on" | "global-eq-enabled" | "global-eq-high-pass" | "global-eq-filter-type" | "power-overlay" | "splitter-placement" | "splitter-editor" | "mixer-editor" | "empty-slot" | "device-search" | "device-search-entry" | "device-search-suggestions" | "device-search-results" | "device-favorites" | "device-recents" | "device-browser-neural-capture" | "device-browser-amp-official" | "plugin-devices-official" | "plugin-folders" | "plugin-list" | "plugin-list-ready" | "plugin-list-reference" | "plugin-list-unsaved" | "plugin-refresh-ready" | "plugin-models" | "plugin-locked" | "plugin-refresh" | "looper-editor" | "device-presets" | "device-presets-user" | "device-presets-official" | "device-preset-actions" | "device-preset-actions-official" | "device-preset-save" | "stomp-assignment" | "scene-assignment" | "expression-parameter" | "expression-bypass" | "block-context" | "block-context-bottom" | "grid-context-menu" | "grid-context-menu-favorite" | "grid-context-menu-bottom" | "input-route-selector" | "input-route-selector-top" | "output-route-selector" | "output-route-selector-top" | "grid-scene-selector" | "directory-presets" | "directory-categories" | "directory-captures" | "directory-captures-official" | "directory-irs" | "directory-irs-official" | "directory-plugins" | "directory-favorites" | "directory-search" | "directory-search-results" | "directory-search-results-physical" | "directory-sort" | "directory-filter" | "directory-arrange" | "directory-copy" | "directory-nested" | "directory-new-folder" | "directory-item-context" | "directory-cloud-upload" | "directory-cloud-upload-physical" | "capture-intro" | "capture-monitoring" | "capture-connect-out" | "capture-connect-input-2" | "capture-routing" | "capture-calibration" | "capture-progress" | "capture-sanity-error" | "capture-result" | "capture-save" | "settings-account" | "settings-system" | "settings-device" | "settings-support" | "settings-wifi" | "settings-update" | "settings-storage" | "settings-system-power" | "settings-system-volume" | "settings-update-idle" | "settings-system-reset" | "settings-device-scene-bypass" | "settings-device-stomp-bypass" | "settings-device-hold-timing" | "settings-device-swap-tempo-tuner" | "settings-device-gig-view" | "settings-device-latency" | "settings-support-report" | "settings-support-licenses" | "settings-account-backups" | "settings-restart-confirm" | "settings-midi" | "settings-info" | "settings-diagnostics" | "settings-diagnostics-dsp" | "settings-diagnostics-footswitch" | "settings-diagnostics-usb" | "recovery-entry" | "recovery-options" | "overlay-keyboard" | "overlay-confirmation" | "overlay-overwrite" | "overlay-error" | "overlay-busy" | "fixture-boot" | "fixture-shutdown" | "fixture-copy-scene" | "fixture-swap-scene" | "fixture-delete" | "fixture-input-gate" | "fixture-editor-pages" | "fixture-editor-cab" | "fixture-editor-eq" | "fixture-editor-capture" | "fixture-warning-clip" | "fixture-warning-dsp" | "modes" | "unit-mirror" | "modes-official" | "save-as" | "edit-details";
function officialBrit2203Snapshot(base: PresetSnapshot): PresetSnapshot {
  const blocks: GridBlock[] = [
    { id: "brit-gate", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-gate"], kind: "utility", category: "Gate", row: 0, column: 0, bypassed: true },
    { id: "brit-wah", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-wah"], kind: "utility", category: "Wah", row: 0, column: 1, bypassed: true },
    { id: "brit-plugin", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-plugin"], kind: "utility", category: "Plugin", row: 0, column: 2, bypassed: true },
    { id: "brit-drive-a", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-drive-a"], kind: "utility", category: "Overdrive", row: 0, column: 3, bypassed: true },
    { id: "brit-drive-b", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-drive-b"], kind: "utility", category: "Overdrive", row: 0, column: 4, bypassed: true },
    { id: "brit-amp", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-amp"], kind: "amp", category: "Amp", row: 0, column: 5 },
    { id: "brit-utility", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-utility"], kind: "utility", category: "Utility", row: 0, column: 6 },
    { id: "brit-cab", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-cab"], kind: "cab", category: "Cab", row: 0, column: 7 },
    { id: "brit-row3-ir", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-row3-ir"], kind: "utility", category: "IR Loader", row: 2, column: 1, bypassed: true },
    { id: "brit-row3-utility", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-row3-utility"], kind: "utility", category: "Utility", row: 2, column: 2 },
    { id: "brit-delay-a", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-delay-a"], kind: "delay", category: "Delay", row: 2, column: 3, bypassed: true },
    { id: "brit-delay-b", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-delay-b"], kind: "delay", category: "Delay", row: 2, column: 4, bypassed: true },
    { id: "brit-row3-reverb", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-row3-reverb"], kind: "utility", category: "Reverb", row: 2, column: 5 },
    { id: "brit-looper", name: OFFICIAL_BRIT_2203_BLOCK_NAMES["brit-looper"], kind: "utility", category: "Looper", row: 2, column: 7 }
  ];
  return { ...base, presetLocation: "1A", presetPosition: 0, presetName: OFFICIAL_BRIT_2203_PRESET_NAME, mode: "PRESET", activeScene: 0, dirty: false, blocks, routes: [{ row: 0, input: "In 1", output: "Row 3", splitMuted: false }, { row: 1, input: "", output: "", splitMuted: false }, { row: 2, input: "Prev. Row", output: "Multi Out", splitMuted: false }, { row: 3, input: "", output: "", splitMuted: false }] };
}

function captureTypeSnapshot(base: PresetSnapshot): PresetSnapshot {
  return {
    ...base,
    presetLocation: "2F",
    presetPosition: 13,
    presetName: "QC MCP TEST",
    mode: "PRESET",
    footswitchModes: ["PRESET", "PRESET"],
    activeScene: 0,
    sceneColors: [QC_SCENE_COLORS[6], ...QC_SCENE_COLORS.slice(1)],
    dirty: false,
    blocks: [{ id: "capture-type", name: "Compressor", kind: "utility", category: "Compressor", glyph: "capture-wave", row: 0, column: 0 }],
    routes: [
      { row: 0, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false },
      { row: 1, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 2, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 3, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false }
    ]
  };
}

function referenceModalSnapshot(base: PresetSnapshot): PresetSnapshot {
  return {
    ...base,
    presetLocation: "5C",
    presetPosition: 34,
    // Neutral stand-in: the real preset name is not reproduced here.
    presetName: "Demo",
    mode: "PRESET",
    footswitchModes: ["PRESET", "PRESET"],
    activeScene: 0,
    sceneColors: [QC_COLORS.reference.presetBrown, ...QC_SCENE_COLORS.slice(1)],
    dirty: false,
    footswitchStates: [{ index: 2, active: true, assigned: true, color: QC_COLORS.reference.presetBrown }],
    blocks: [
      { id: "demo-reverb", name: "Reverb", kind: "reverb", category: "Reverb", row: 0, column: 1, bypassed: true },
      { id: "demo-looper", name: "Looper", kind: "utility", category: "Looper", row: 0, column: 2, bypassed: true }
    ],
    routes: [
      { row: 0, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false },
      { row: 1, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 2, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 3, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false }
    ]
  };
}

function referenceBrowserSnapshot(base: PresetSnapshot): PresetSnapshot {
  return {
    ...captureTypeSnapshot(base),
    presetLocation: "2F",
    presetPosition: 13
  };
}

export function coros410FixtureSnapshot(base: PresetSnapshot, overrides: Partial<Pick<PresetSnapshot, "tempo" | "mode">> = {}): PresetSnapshot {
  return {
    ...base,
    presetName: "pyquadcortex scratch",
    presetLocation: "32H",
    presetPosition: 255,
    setlistKey: "/user/A/",
    setlistName: "My Presets",
    mode: "STOMP",
    footswitchModes: ["STOMP", "STOMP"],
    activeScene: 0,
    scenes: ["Default scene", "Scene B", "Scene C", "Scene D", "Scene E", "Scene F", "Scene G", "Scene H"],
    tempo: 120,
    tempoLedEnabled: true,
    masterVolume: 40,
    dirty: false,
    routes: [
      { row: 0, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false },
      { row: 1, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 2, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 3, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false }
    ],
    ioPorts: [{ kind: "input", id: 1, label: "In 1", plugged: false }],
    footswitchStates: [{ index: 7, active: true, assigned: true, color: QC_SCENE_COLORS[0] }],
    blocks: [
      { id: "simple-gate", modelId: 1, name: COROS_410_BLOCK_NAMES["simple-gate"], kind: "utility", category: "Utility", row: 0, column: 0, footswitch: 0 },
      { id: "chief-ds1", modelId: 2, name: COROS_410_BLOCK_NAMES["chief-ds1"], kind: "utility", category: "Guitar Overdrive", row: 0, column: 1, footswitch: 1 },
      { id: "digital-flanger", modelId: 3, name: COROS_410_BLOCK_NAMES["digital-flanger"], kind: "mod", category: "Modulation", row: 0, column: 2, footswitch: 2, bypassed: true },
      { id: "ukc30-topboost", modelId: 4, name: COROS_410_BLOCK_NAMES["ukc30-topboost"], kind: "amp", category: "Amp", row: 0, column: 3, footswitch: 3 },
      { id: "ukc30-cab", modelId: 5, name: COROS_410_BLOCK_NAMES["ukc30-cab"], kind: "cab", category: "Cab", row: 0, column: 4, footswitch: 4 },
      { id: "parametric-8", modelId: 6, name: COROS_410_BLOCK_NAMES["parametric-8"], kind: "utility", category: "EQ", row: 0, column: 5, footswitch: 5 },
      { id: "ambience", modelId: 7, name: COROS_410_BLOCK_NAMES["ambience"], kind: "reverb", category: "Reverb", row: 0, column: 6, footswitch: 6 }
    ],
    ...overrides
  };
}

export interface CorOsFixtureConfiguration {
  enabled: boolean;
  initialSnapshot: PresetSnapshot;
  screenView: CorOsScreenView | null;
}

/** Resolve the visual-conformance URL once so every host captures identical state. */
export function corosFixtureConfiguration(search: string, base: PresetSnapshot): CorOsFixtureConfiguration {
  const params = new URLSearchParams(search);
  const enabled = params.get("fixture") === "coros410";
  const mode = params.get("mode");
  const tempo = Number(params.get("tempo"));
  const overrides: Partial<Pick<PresetSnapshot, "tempo" | "mode">> = {
    ...(Number.isFinite(tempo) && tempo > 0 ? { tempo } : {}),
    ...(["PRESET", "SCENE", "STOMP", "HYBRID"].includes(mode ?? "")
      ? { mode: mode as PresetSnapshot["mode"] }
      : {})
  };
  const initialSnapshot = enabled ? coros410FixtureSnapshot(base, overrides) : base;
  return {
    enabled,
    initialSnapshot: enabled && params.get("variant") === "capture-type"
      ? captureTypeSnapshot(initialSnapshot)
      : enabled && params.get("variant") === "reference-modal"
        ? referenceModalSnapshot(initialSnapshot)
        : enabled && params.get("variant") === "reference-browser"
          ? referenceBrowserSnapshot(initialSnapshot)
          : initialSnapshot,
    screenView: params.get("screen") as CorOsScreenView | null
  };
}

export function fixtureSnapshot(view: CorOsScreenView, snapshot: PresetSnapshot): PresetSnapshot {
  return view === "grid-official-brit" ? officialBrit2203Snapshot(snapshot) : snapshot;
}
