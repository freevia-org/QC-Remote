import {
  QC_GRID_COLUMNS,
  QC_GRID_ROWS,
  QC_INPUT_ROUTES,
  QC_MAXIMUM_TEMPO_BPM,
  QC_MINIMUM_TEMPO_BPM,
  QC_OUTPUT_ROUTES,
  QC_SCENE_COUNT,
  QC_STORED_PRESET_NAME_CHARACTERS,
  qcCategoryKey,
  type BlockDetails,
  type BlockParameter,
  type BypassExpression,
  type ConnectionState,
  type DeviceActionResult,
  type DeviceIdentity,
  type FootswitchState,
  type GatewayTransport,
  type GridBlock,
  type GridRoute,
  type LaneControl,
  type ModeSlot,
  type ModelEntry,
  type PresetFolder,
  type PresetSnapshot
} from "@qc-remote/client";
import { demoSnapshot, QC_CATEGORY_COLORS, QC_HARDWARE_COLORS, QC_SCENE_COLORS, SHARED_QC_ACTIONS, trimTrailingSlashes } from "@qc-remote/core";
import {
  defaultParameterValues,
  normalizedToReal,
  type MeasuredParameterDefaults,
  type VirtualCatalogueModel,
  type VirtualCatalogueParameter,
  type VirtualModelCatalogue
} from "./virtual-device-defaults.ts";

/**
 * A virtual Quad Cortex: a second implementation of `GatewayTransport` backed
 * by local state instead of the device broker.
 *
 * Only the "build presets and move around" surface is simulated - the Grid,
 * parameters, scenes, routing, tempo, master volume, footswitch modes and the
 * preset library. Everything else the real device offers (tuner, global EQ,
 * physical I/O, MIDI, looper, captures, IRs, screen mirroring, backups, the
 * device's own undo stack) rejects with the same "Connect the Quad Cortex
 * before ..." wording today's static demo mode already uses for the tuner and
 * Gig View.
 *
 * Every simulated write honours the real gateway's optimistic-concurrency
 * guards and rejects a stale expectation with the real gateway's own message
 * (the Rust planner in `packages/rust/quadcortex/src/device/request.rs`), so a
 * workflow hook's error handling behaves identically on either transport, and
 * the working surface reads exactly as it does on real hardware. Only the
 * connection badge, its panel and the diagnostics say which one is serving.
 * One exception: a missing route is not worded "not synchronized" as the
 * gateway words it, because `isQcConnectionFailure` would read that as a lost
 * USB connection and send a demo session into hardware recovery.
 */

// ---------------------------------------------------------------------------
// Persistence

/**
 * Where the virtual device's JSON document lives, relative to
 * `%LOCALAPPDATA%` on Windows. Every other per-user file the app writes sits
 * under `%LOCALAPPDATA%\QC Remote\` (runtime health, integrations), so this
 * follows that convention rather than the Tauri bundle identifier.
 */
export const VIRTUAL_DEVICE_STORE_RELATIVE_PATH = "QC Remote/virtual-device/store.json";

/** Reads and writes the store's single JSON document. `read` resolves null when none exists yet. */
export interface VirtualDeviceStorage {
  read(): Promise<string | null>;
  write(text: string): Promise<void>;
}

/** An in-memory storage port, for previews and tests that do not need a file. */
export function createMemoryVirtualDeviceStorage(initial: string | null = null): VirtualDeviceStorage & { readonly text: string | null } {
  let text = initial;
  return {
    get text() { return text; },
    async read() { return text; },
    async write(next) { text = next; }
  };
}

// ---------------------------------------------------------------------------
// Persisted state

export interface ParameterExpression { pedal: 1 | 2; minimum: number; maximum: number; }

/** Parameter state shared by Grid blocks and the two per-row lane controls. */
export interface VirtualParameterState {
  /** parameterIndex -> normalized value, for parameters not in scene mode. */
  parameterValues: Record<number, number>;
  /** parameterIndex -> one normalized value per scene, for parameters in scene mode. */
  sceneValues?: Record<number, number[]>;
  expressions?: Record<number, ParameterExpression>;
}

export interface VirtualBlock extends VirtualParameterState {
  row: number;
  column: number;
  modelId: number;
  /** One bypass state per scene, as the device keeps them. */
  sceneBypassed: boolean[];
  footswitch?: number;
  footswitchOrder?: number;
  footswitchAssignmentType?: "primary" | "secondary";
  bypassExpression?: BypassExpression;
}

export interface VirtualRoute {
  row: number;
  inputId: number;
  outputId: number;
  splitColumn?: number;
  mixColumn?: number;
  /** One splitter/mixer mute state per scene. */
  splitMuted: boolean[];
}

export interface VirtualStomp { label: string; momentary: boolean; }

export interface VirtualPreset {
  name: string;
  scenes: string[];
  sceneColors: string[];
  activeScene: number;
  tempo: number;
  blocks: VirtualBlock[];
  routes: VirtualRoute[];
  stomps: VirtualStomp[];
  lanes: Record<number, Partial<Record<LaneControl, VirtualParameterState>>>;
}

export interface VirtualSetlist {
  key: string;
  name: string;
  /** Built-in Demo content may be recalled and edited, but never overwritten. */
  isFactory?: boolean;
  /** position -> stored preset. An absent position is an empty slot. */
  presets: Record<number, VirtualPreset>;
}

export interface VirtualDeviceState {
  version: 1;
  /** Changes on every session reset; part of the preset identity. */
  session: string;
  /** Changes whenever the active preset's address or stored name changes. */
  addressRevision: number;
  device: {
    serial: string;
    customName: string;
    mode: PresetSnapshot["mode"];
    modeSlots: ModeSlot[];
    footswitchModes: ("PRESET" | "SCENE" | "STOMP")[];
    masterVolume: number;
    masterVolumeAssignment: { out12: boolean; out34: boolean; send12: boolean; headphones: boolean };
    sceneBypassBehavior: "alwaysOverwrite" | "nonstompOverwrite" | "neverOverwrite";
  };
  current: { setlistKey: string; position: number; dirty: boolean; preset: VirtualPreset };
  setlists: VirtualSetlist[];
}

/** A Grid block as the virtual device publishes it: the real shape plus its parameter values. */
export type VirtualGridBlock = GridBlock & {
  /** parameterIndex -> normalized value in the active scene. Virtual-device path only. */
  parameterValues?: Record<number, number>;
};

// ---------------------------------------------------------------------------
// Constants

const USER_SETLIST_ROOT = "/media/p4/Presets";
const DEMO_FACTORY_SETLIST_KEY = "/factory/Demo Presets";
const DEMO_FACTORY_SETLIST_NAME = "Demo Factory Presets";
const PRESET_SLOTS = 256;
const LANE_MODELS: Record<LaneControl, { modelId: number; column: number }> = {
  inputGate: { modelId: 28_000, column: 10 },
  laneOutput: { modelId: 23_000, column: 11 }
};
/** The seven outputs a chain can be written to, from the action contract rather than retyped. */
const CHAIN_ASSIGNABLE_OUTPUTS: readonly number[] = (() => {
  const action = SHARED_QC_ACTIONS.find((candidate) => candidate.name === "set_chain_output") as
    { inputSchema?: { properties?: { output_id?: { enum?: readonly number[] } } } } | undefined;
  const ids = action?.inputSchema?.properties?.output_id?.enum;
  if (!ids?.length) throw new Error("The action contract no longer declares the assignable chain outputs.");
  return ids;
})();
const PARALLEL_ROWS = [0, 2];

/**
 * Demo Mode's own default preset name - never demoSnapshot's "Brit 2203",
 * which is a showcase label used elsewhere as a generic loading placeholder
 * and could read as genuine device content someone mistakes for their own.
 *
 * The blocks themselves are kept (see STARTER_MODEL_IDS below): a small,
 * working example a person can inspect, rearrange and bypass shows off more
 * of the app than a blank Grid would, and dozens of existing tests already
 * exercise moveBlock/removeBlock/addBlock/etc. against this exact populated
 * layout. Renaming only the preset is enough to satisfy the actual
 * requirement - it must read unambiguously as a demo, not a real preset -
 * without a wholesale rewrite of that coverage for no behavioral gain.
 */
const DEMO_MODE_PRESET_NAME = "Demo Mode";

/**
 * The starter preset reuses demoSnapshot's layout, but every block must be a
 * real catalogue model or it could not be edited: demo names such as "British
 * 2203" or "Plini Cab" are showcase labels, not ModelRepo entries. Each demo
 * block is mapped to the catalogue model it stands for (or the nearest real one
 * in its category). The Neural Capture, IR loader and "Rabea Device" plugin
 * blocks are left out: captures and IRs are outside the virtual surface, and
 * the plugin names no catalogue model.
 */
const STARTER_MODEL_IDS: Record<string, number> = {
  gate: 16_001, // Adaptive Gate
  amp: 1_001, // Brit 2203
  cab: 12_005, // 412 Brit 60B GB 90s (M)
  overdrive: 37, // Gojira OD
  delay: 6_001, // Analog Delay (M)
  reverb: 8_021, // Nolly Reverb
  compressor: 5_007, // Opto Comp (M)
  pitch: 18_019, // Misha Dual Octaver
  modulation: 7_036, // Harmonic Tremolo
  morph: 26_001, // Freeze
  synth: 30_001, // Overlord Synth
  filter: 24_003, // Envelope Filter
  equalizer: 4_000, // Parametric-8
  wah: 9_013, // Crying Wah
  "fx-loop": 13_004, // FX Loop 1
  looper: 27_000, // Looper X
  utility: 16_005 // Gain
};

// ---------------------------------------------------------------------------
// Small helpers

const clone = <T,>(value: T): T => structuredClone(value);
const sceneLetter = (scene: number) => String.fromCharCode(65 + Math.min(Math.max(scene, 0), 7));
const defaultSceneLabel = (scene: number) => `Scene ${sceneLetter(scene)}`;
const perScene = <T,>(value: T): T[] => Array.from({ length: QC_SCENE_COUNT }, () => value);
const rustOption = (value: number | null | undefined) => value == null ? "None" : `Some(${value})`;
const setlistKeyFor = (name: string) => `${USER_SETLIST_ROOT}/${name}`;
const trimKey = trimTrailingSlashes;

/** CorOS's slot name: bank number then A-H. */
export function virtualSlotName(position: number): string {
  return `${Math.floor(position / 8) + 1}${String.fromCharCode(65 + (position % 8))}`;
}

function inputLabel(id: number): string {
  return QC_INPUT_ROUTES.find((route) => route.id === id)?.label ?? `Input ${id}`;
}

function outputLabel(id: number): string {
  return QC_OUTPUT_ROUTES.find((route) => route.id === id)?.label ?? `Output ${id}`;
}

/** Mirrors the broker's `display_category` so a category reads the same across endpoints. */
function displayCategory(value: string): string {
  const key = value.toLowerCase().replace(/[^a-z0-9]/g, "");
  const table: Record<string, string> = {
    bassamplifier: "Bass Amp", bassamp: "Bass Amp", bassoverdrive: "Bass Overdrive",
    cabsimbassm: "Bass Cab", cabsimbassst: "Bass Cab", basscabinet: "Bass Cab", basscab: "Bass Cab",
    cabsimguitarm: "Guitar Cab", cabsimguitarst: "Guitar Cab", guitarcabinet: "Guitar Cab", guitarcab: "Guitar Cab",
    compressor: "Compressor", delay: "Delay", equalizer: "EQ", eq: "EQ", filter: "Filter", fxloop: "FX Loop",
    guitaramplifier: "Guitar Amp", guitaramp: "Guitar Amp", amp: "Guitar Amp", guitaroverdrive: "Guitar Overdrive",
    irloaders: "IR Loader", irloader: "IR Loader", loopers: "Looper", looper: "Looper", modulation: "Modulation",
    morph: "Morph", neuralcapture: "Neural Capture", pitch: "Pitch", reverb: "Reverb", synth: "Synth",
    utility: "Utility", wah: "Wah",
    // Model 28000's raw ModelRepo category is "InputGateControl" - no spaces
    // at all - so without this entry, editorCategoryLabel's toUpperCase()
    // prints "INPUTGATECONTROL" instead of matching fixture-input-gate's own
    // "INPUT GATE CONTROL". Model 23000's "Lane output control" already has
    // real word spaces (toUpperCase alone gets it to "LANE OUTPUT CONTROL"),
    // so it needs no entry here - listed anyway so both lane-control models
    // are covered by an explicit mapping rather than one relying on an
    // accident of its raw category text.
    inputgatecontrol: "Input Gate Control", laneoutputcontrol: "Lane Output Control"
  };
  return table[key] ?? value.trim();
}

/** Mirrors the broker's catalogue kind derivation from the raw ModelRepo category. */
function blockKind(category: string): GridBlock["kind"] {
  const lower = category.toLowerCase();
  if (lower.includes("amp")) return "amp";
  if (lower.includes("cab") || lower.includes("impulse")) return "cab";
  if (lower.includes("delay")) return "delay";
  if (lower.includes("reverb")) return "reverb";
  if (lower.includes("mod") || lower.includes("pitch")) return "mod";
  if (lower.includes("capture")) return "capture";
  if (lower.includes("input")) return "input";
  if (lower.includes("output")) return "output";
  return "utility";
}

function trimNumber(value: number, digits: number): string {
  return String(Number(value.toFixed(digits)));
}

// ---------------------------------------------------------------------------
// Starting state

function blankRoutes(): VirtualRoute[] {
  return Array.from({ length: QC_GRID_ROWS }, (_, row) => {
    const template = demoSnapshot.routes.find((route) => route.row === row);
    return { row, inputId: template?.inputId ?? 0, outputId: template?.outputId ?? 0, splitMuted: perScene(false) };
  });
}

function blankPreset(name: string): VirtualPreset {
  return {
    name,
    scenes: Array.from({ length: QC_SCENE_COUNT }, (_, scene) => defaultSceneLabel(scene)),
    sceneColors: [...QC_SCENE_COLORS],
    activeScene: 0,
    tempo: 120,
    blocks: [],
    routes: blankRoutes(),
    stomps: perScene(null).map(() => ({ label: "", momentary: false })),
    lanes: {}
  };
}

function starterPreset(catalogue: CatalogueIndex): VirtualPreset {
  // Named DEMO_MODE_PRESET_NAME, not demoSnapshot's own "Brit 2203" - see
  // that constant's comment; the layout itself is still demoSnapshot's.
  const preset = blankPreset(DEMO_MODE_PRESET_NAME);
  preset.scenes = [...demoSnapshot.scenes];
  preset.sceneColors = [...(demoSnapshot.sceneColors ?? QC_SCENE_COLORS)];
  preset.activeScene = demoSnapshot.activeScene;
  preset.tempo = demoSnapshot.tempo;
  for (const block of demoSnapshot.blocks) {
    const modelId = STARTER_MODEL_IDS[block.id];
    if (modelId === undefined || block.column < 0 || block.column >= QC_GRID_COLUMNS) continue;
    // A model missing from the catalogue handed in keeps its place with no
    // parameter values, exactly as an unresolved add would.
    preset.blocks.push({
      row: block.row,
      column: block.column,
      modelId,
      sceneBypassed: perScene(block.bypassed === true),
      parameterValues: catalogue.defaults(modelId)
    });
  }
  return preset;
}

/**
 * Small, useful built-in library for Demo Mode. These are deliberately named
 * as demo presets: they exercise real catalogue models and their parameter
 * screens without pretending to be Neural DSP's copyrighted factory library.
 */
const DEMO_FACTORY_PRESETS: ReadonlyArray<readonly [string, readonly number[]]> = [
  ["Essential Rig", [16_001, 37, 1_001, 12_005, 6_001, 8_021]],
  ["Driven Delay", [37, 1_001, 12_005, 4_000, 6_001]],
  ["Ambient Freeze", [5_007, 1_001, 12_005, 26_001, 6_001, 8_021]],
  ["Octave Lead", [18_019, 37, 1_001, 12_005, 6_001, 8_021]],
  ["Wah Drive", [9_013, 37, 1_001, 12_005, 6_001]],
  ["Tremolo Space", [1_001, 12_005, 7_036, 6_001, 8_021]],
  ["Filter Motion", [24_003, 5_007, 1_001, 12_005, 6_001]],
  ["Looper Practice", [16_001, 1_001, 12_005, 27_000, 8_021]]
];

function demoFactoryPreset(catalogue: CatalogueIndex, name: string, modelIds: readonly number[]): VirtualPreset {
  const preset = blankPreset(name);
  modelIds.forEach((modelId, column) => {
    const model = catalogue.model(modelId);
    if (!model || model.hidden || column >= QC_GRID_COLUMNS) return;
    preset.blocks.push({
      row: 0,
      column,
      modelId,
      sceneBypassed: perScene(false),
      parameterValues: catalogue.defaults(modelId),
      footswitch: column,
      footswitchOrder: column,
      footswitchAssignmentType: "primary"
    });
    preset.stomps[column] = { label: model.name.slice(0, 32), momentary: false };
  });
  return preset;
}

function demoFactorySetlist(catalogue: CatalogueIndex): VirtualSetlist {
  return {
    key: DEMO_FACTORY_SETLIST_KEY,
    name: DEMO_FACTORY_SETLIST_NAME,
    isFactory: true,
    presets: Object.fromEntries(DEMO_FACTORY_PRESETS.map(([name, models], position) => [position, demoFactoryPreset(catalogue, name, models)]))
  };
}

/** Add or refresh the built-in library without disturbing any user setlist. */
function withDemoFactoryLibrary(state: VirtualDeviceState, catalogue: CatalogueIndex): VirtualDeviceState {
  state.setlists = [
    ...state.setlists.filter((setlist) => !setlist.isFactory && trimKey(setlist.key) !== trimKey(DEMO_FACTORY_SETLIST_KEY)),
    demoFactorySetlist(catalogue)
  ];
  const currentIsFactory = trimKey(state.current.setlistKey) === trimKey(DEMO_FACTORY_SETLIST_KEY);
  if (currentIsFactory && !state.current.dirty) {
    const stored = setlistOf(state, DEMO_FACTORY_SETLIST_KEY)?.presets[state.current.position];
    state.current.preset = stored ? clone(stored) : blankPreset("");
  }
  return state;
}

function addUserSetlist(state: VirtualDeviceState, setlist: VirtualSetlist): void {
  const factoryPosition = state.setlists.findIndex((candidate) => candidate.isFactory);
  if (factoryPosition < 0) state.setlists.push(setlist);
  else state.setlists.splice(factoryPosition, 0, setlist);
}

function starterState(catalogue: CatalogueIndex, session: string): VirtualDeviceState {
  const setlistName = demoSnapshot.setlistName;
  const setlistKey = setlistKeyFor(setlistName);
  const preset = starterPreset(catalogue);
  return withDemoFactoryLibrary({
    version: 1,
    session,
    addressRevision: 1,
    device: {
      serial: "VIRTUAL-QC",
      // Not demoSnapshot.deviceName ("Quad Cortex"): a demo session must never
      // present itself under the hardware's name.
      customName: "Demo device",
      mode: demoSnapshot.mode,
      modeSlots: clone(demoSnapshot.modeSlots ?? []),
      footswitchModes: [...(demoSnapshot.footswitchModes ?? [])],
      masterVolume: demoSnapshot.masterVolume,
      masterVolumeAssignment: { out12: true, out34: true, send12: false, headphones: true },
      sceneBypassBehavior: "alwaysOverwrite"
    },
    current: { setlistKey, position: demoSnapshot.presetPosition, dirty: false, preset: clone(preset) },
    setlists: [{ key: setlistKey, name: setlistName, presets: { [demoSnapshot.presetPosition]: preset } }]
  }, catalogue);
}

function isVirtualDeviceState(value: unknown): value is VirtualDeviceState {
  const candidate = value as Partial<VirtualDeviceState> | null;
  return !!candidate && candidate.version === 1 && typeof candidate.session === "string"
    && typeof candidate.addressRevision === "number" && !!candidate.device && !!candidate.current
    && Array.isArray(candidate.setlists);
}

// ---------------------------------------------------------------------------
// Reading a preset or a library that came from outside (pasted text)
//
// Pasted text is untrusted: every field is checked and copied into a fresh
// object, so nothing the projection or the guards later rely on can be missing,
// mistyped or out of range, and nothing unexpected rides along into the store.

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const isIntegerIn = (value: unknown, minimum: number, maximum: number): value is number =>
  Number.isInteger(value) && (value as number) >= minimum && (value as number) <= maximum;
const isNormalized = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
/** Parameter indices are small integers; a key beyond this is not a parameter. */
const PARAMETER_INDEX_LIMIT = 100_000;
/** More setlists than any real library holds; a guard against a runaway document, not a device rule. */
const LIBRARY_SETLIST_LIMIT = 1_000;
const FOOTSWITCH_MODES = ["PRESET", "SCENE", "STOMP"] as const;
const DEVICE_MODES = ["PRESET", "SCENE", "STOMP", "HYBRID"] as const;
const SCENE_BYPASS_BEHAVIORS = ["alwaysOverwrite", "nonstompOverwrite", "neverOverwrite"] as const;

function invalid(where: string, problem: string): never {
  throw new Error(`${where} ${problem}.`);
}

function readSceneFlags(value: unknown, where: string): boolean[] {
  if (!Array.isArray(value) || value.length !== QC_SCENE_COUNT || !value.every((entry) => typeof entry === "boolean")) {
    invalid(where, `must list ${QC_SCENE_COUNT} true/false values, one per scene`);
  }
  return [...value] as boolean[];
}

function readSceneStrings(value: unknown, where: string, field: string): string[] {
  if (!Array.isArray(value) || value.length !== QC_SCENE_COUNT || !value.every((entry) => typeof entry === "string")) {
    invalid(where, `must list ${QC_SCENE_COUNT} ${field}`);
  }
  return [...value] as string[];
}

function readParameterIndex(key: string, where: string): number {
  if (!/^\d+$/.test(key) || !isIntegerIn(Number(key), 0, PARAMETER_INDEX_LIMIT)) invalid(where, `names ${JSON.stringify(key)}, which is not a parameter index`);
  return Number(key);
}

function readParameterState(value: Record<string, unknown>, where: string): VirtualParameterState {
  if (!isRecord(value.parameterValues)) invalid(where, "has no parameterValues object");
  const state: VirtualParameterState = { parameterValues: {} };
  for (const [key, entry] of Object.entries(value.parameterValues)) {
    const parameter = readParameterIndex(key, where);
    if (!isNormalized(entry)) invalid(where, `gives parameter ${parameter} a value outside 0 through 1`);
    state.parameterValues[parameter] = entry;
  }
  if (value.sceneValues !== undefined) {
    if (!isRecord(value.sceneValues)) invalid(where, "has a sceneValues entry that is not an object");
    state.sceneValues = {};
    for (const [key, entry] of Object.entries(value.sceneValues)) {
      const parameter = readParameterIndex(key, where);
      if (!Array.isArray(entry) || entry.length !== QC_SCENE_COUNT || !entry.every((scene) => isNormalized(scene))) {
        invalid(where, `must give parameter ${parameter} ${QC_SCENE_COUNT} scene values from 0 through 1`);
      }
      state.sceneValues[parameter] = [...entry] as number[];
    }
  }
  if (value.expressions !== undefined) {
    if (!isRecord(value.expressions)) invalid(where, "has an expressions entry that is not an object");
    state.expressions = {};
    for (const [key, entry] of Object.entries(value.expressions)) {
      const parameter = readParameterIndex(key, where);
      if (!isRecord(entry) || (entry.pedal !== 1 && entry.pedal !== 2) || !isNormalized(entry.minimum) || !isNormalized(entry.maximum)) {
        invalid(where, `has an invalid expression pedal assignment on parameter ${parameter}`);
      }
      state.expressions[parameter] = { pedal: entry.pedal, minimum: entry.minimum, maximum: entry.maximum };
    }
  }
  return state;
}

function readBypassExpression(value: unknown, where: string): BypassExpression {
  const numeric = ["pedal", "minimum", "maximum", "mode", "delayMs"] as const;
  if (!isRecord(value) || !numeric.every((key) => typeof value[key] === "number" && Number.isFinite(value[key]))
    || typeof value.invert !== "boolean" || typeof value.latchEmulation !== "boolean") {
    invalid(where, "has an invalid bypass expression");
  }
  return {
    pedal: value.pedal as number, minimum: value.minimum as number, maximum: value.maximum as number, mode: value.mode as number,
    invert: value.invert, delayMs: value.delayMs as number, latchEmulation: value.latchEmulation
  };
}

function readBlock(value: unknown, index: CatalogueIndex, where: string): VirtualBlock {
  if (!isRecord(value)) invalid(where, "is not an object");
  const { row, column, modelId } = value;
  if (!isIntegerIn(row, 0, QC_GRID_ROWS - 1) || !isIntegerIn(column, 0, QC_GRID_COLUMNS - 1)) invalid(where, "is not on the Grid");
  const at = `${where} (row ${row + 1}, column ${column + 1})`;
  if (!isIntegerIn(modelId, 1, 0x7fff_ffff)) invalid(at, "names no model");
  const model = index.model(modelId);
  if (!model || model.hidden) invalid(at, `holds model ${modelId}, which the demo device's model catalogue does not offer`);
  const block: VirtualBlock = { row, column, modelId, sceneBypassed: readSceneFlags(value.sceneBypassed, at), ...readParameterState(value, at) };
  if (value.footswitch !== undefined) {
    if (!isIntegerIn(value.footswitch, 0, QC_SCENE_COUNT - 1)) invalid(at, "has an invalid footswitch");
    block.footswitch = value.footswitch;
    if (value.footswitchOrder !== undefined) {
      if (!isIntegerIn(value.footswitchOrder, 0, QC_GRID_ROWS * QC_GRID_COLUMNS)) invalid(at, "has an invalid footswitch order");
      block.footswitchOrder = value.footswitchOrder;
    }
    if (value.footswitchAssignmentType !== undefined) {
      if (value.footswitchAssignmentType !== "primary" && value.footswitchAssignmentType !== "secondary") invalid(at, "has an invalid footswitch assignment type");
      block.footswitchAssignmentType = value.footswitchAssignmentType;
    }
  }
  if (value.bypassExpression !== undefined) block.bypassExpression = readBypassExpression(value.bypassExpression, at);
  return block;
}

function readRoute(value: unknown, where: string): VirtualRoute {
  if (!isRecord(value)) invalid(where, "is not an object");
  const { row, inputId, outputId, splitColumn, mixColumn } = value;
  if (!isIntegerIn(row, 0, QC_GRID_ROWS - 1)) invalid(where, "names no Grid row");
  if (!isIntegerIn(inputId, 0, 0xffff) || !isIntegerIn(outputId, 0, 0xffff)) invalid(where, "has an invalid input or output");
  const route: VirtualRoute = { row, inputId, outputId, splitMuted: readSceneFlags(value.splitMuted, where) };
  if (splitColumn !== undefined) {
    if (!isIntegerIn(splitColumn, 0, QC_GRID_COLUMNS - 1)) invalid(where, "has an invalid split column");
    route.splitColumn = splitColumn;
  }
  if (mixColumn !== undefined) {
    if (!isIntegerIn(mixColumn, 0, QC_GRID_COLUMNS - 1)) invalid(where, "has an invalid mix column");
    route.mixColumn = mixColumn;
  }
  return route;
}

function readLanes(value: unknown, where: string): VirtualPreset["lanes"] {
  if (!isRecord(value)) invalid(where, "has no lanes object");
  const lanes: VirtualPreset["lanes"] = {};
  for (const [key, entry] of Object.entries(value)) {
    const row = Number(key);
    if (!/^\d+$/.test(key) || !isIntegerIn(row, 0, QC_GRID_ROWS - 1)) invalid(where, `has lane controls for ${JSON.stringify(key)}, which is not a Grid row`);
    if (!isRecord(entry)) invalid(where, `has invalid lane controls on row ${row + 1}`);
    const controls: Partial<Record<LaneControl, VirtualParameterState>> = {};
    for (const [control, state] of Object.entries(entry)) {
      if (!(control in LANE_MODELS)) invalid(where, `has an unknown lane control ${JSON.stringify(control)} on row ${row + 1}`);
      if (!isRecord(state)) invalid(where, `has an invalid ${control} on row ${row + 1}`);
      controls[control as LaneControl] = readParameterState(state, `${where}, row ${row + 1} ${control}`);
    }
    lanes[row] = controls;
  }
  return lanes;
}

/** Check and copy one preset as the demo device keeps it. Throws a message naming what is wrong. */
function readPreset(value: unknown, index: CatalogueIndex, where: string): VirtualPreset {
  if (!isRecord(value)) invalid(where, "is not an object");
  const { name, activeScene, tempo, blocks, routes, stomps } = value;
  if (typeof name !== "string" || name.length > QC_STORED_PRESET_NAME_CHARACTERS) invalid(where, `needs a name of at most ${QC_STORED_PRESET_NAME_CHARACTERS} characters`);
  if (!isIntegerIn(activeScene, 0, QC_SCENE_COUNT - 1)) invalid(where, "has no valid active scene");
  if (typeof tempo !== "number" || !Number.isFinite(tempo) || tempo < QC_MINIMUM_TEMPO_BPM || tempo > QC_MAXIMUM_TEMPO_BPM) {
    invalid(where, `needs a tempo from ${QC_MINIMUM_TEMPO_BPM} through ${QC_MAXIMUM_TEMPO_BPM} BPM`);
  }
  if (!Array.isArray(blocks) || blocks.length > QC_GRID_ROWS * QC_GRID_COLUMNS) invalid(where, "has no valid block list");
  const placed = blocks.map((block, position) => readBlock(block, index, `${where}, block ${position + 1}`));
  const cells = new Set<string>();
  for (const block of placed) {
    const cell = `${block.row}:${block.column}`;
    if (cells.has(cell)) invalid(where, `places two blocks at row ${block.row + 1}, column ${block.column + 1}`);
    cells.add(cell);
  }
  if (!Array.isArray(routes)) invalid(where, "has no routing list");
  const routed = routes.map((route, position) => readRoute(route, `${where}, route ${position + 1}`));
  if (routed.length !== QC_GRID_ROWS || new Set(routed.map((route) => route.row)).size !== QC_GRID_ROWS) invalid(where, `must route each of the ${QC_GRID_ROWS} Grid rows once`);
  if (!Array.isArray(stomps) || stomps.length !== QC_SCENE_COUNT
    || !stomps.every((stomp) => isRecord(stomp) && typeof stomp.label === "string" && typeof stomp.momentary === "boolean")) {
    invalid(where, `must list ${QC_SCENE_COUNT} footswitch labels`);
  }
  return {
    name,
    scenes: readSceneStrings(value.scenes, where, "scene names"),
    sceneColors: readSceneStrings(value.sceneColors, where, "scene colours"),
    activeScene,
    tempo,
    blocks: placed,
    routes: routed,
    stomps: (stomps as VirtualStomp[]).map((stomp) => ({ label: stomp.label, momentary: stomp.momentary })),
    lanes: readLanes(value.lanes, where)
  };
}

type VirtualLibrary = Pick<VirtualDeviceState, "device" | "current" | "setlists">;

/** Check and copy a whole demo library (a version 1 store document). Session and revision are the receiver's own. */
function readLibrary(value: unknown, index: CatalogueIndex): VirtualLibrary {
  const where = "The pasted demo library";
  if (!isRecord(value) || value.version !== 1) invalid(where, "is not a version 1 demo device document");
  const { device, current, setlists } = value;
  if (!isRecord(device)) invalid(where, "has no device settings");
  const { serial, customName, mode, modeSlots, footswitchModes, masterVolume, masterVolumeAssignment, sceneBypassBehavior } = device;
  if (typeof serial !== "string" || serial.length > 64) invalid(where, "has an invalid serial");
  if (typeof customName !== "string" || customName.length > 32) invalid(where, "needs a device name of at most 32 characters");
  if (!DEVICE_MODES.includes(mode as never)) invalid(where, "has an invalid footswitch mode");
  if (!Array.isArray(modeSlots) || modeSlots.length > 3 || !modeSlots.every((slot) => isRecord(slot) && isIntegerIn(slot.slot, 0, 2)
    && typeof slot.label === "string" && DEVICE_MODES.includes(slot.mode as never))) invalid(where, "has invalid mode slots");
  if (!Array.isArray(footswitchModes) || footswitchModes.length > 3 || !footswitchModes.every((entry) => FOOTSWITCH_MODES.includes(entry as never))) {
    invalid(where, "has invalid footswitch modes");
  }
  if (!isIntegerIn(masterVolume, 0, 100)) invalid(where, "needs a Master Volume from 0 through 100");
  const outputs = ["out12", "out34", "send12", "headphones"] as const;
  if (!isRecord(masterVolumeAssignment) || !outputs.every((output) => typeof masterVolumeAssignment[output] === "boolean")) invalid(where, "has an invalid Master Volume assignment");
  if (!SCENE_BYPASS_BEHAVIORS.includes(sceneBypassBehavior as never)) invalid(where, "has an invalid scene bypass behaviour");

  if (!Array.isArray(setlists) || setlists.length < 1 || setlists.length > LIBRARY_SETLIST_LIMIT) invalid(where, `must hold from 1 through ${LIBRARY_SETLIST_LIMIT} setlists`);
  const keys = new Set<string>();
  const library = setlists.map((setlist, position): VirtualSetlist => {
    const at = `${where}, setlist ${position + 1}`;
    if (!isRecord(setlist) || typeof setlist.key !== "string" || !setlist.key.trim() || typeof setlist.name !== "string" || !isRecord(setlist.presets)) {
      invalid(at, "needs a key, a name and a presets object");
    }
    if (keys.has(trimKey(setlist.key))) invalid(at, `repeats the setlist ${JSON.stringify(setlist.key)}`);
    keys.add(trimKey(setlist.key));
    const presets: Record<number, VirtualPreset> = {};
    for (const [slot, preset] of Object.entries(setlist.presets)) {
      if (!/^\d+$/.test(slot) || !isIntegerIn(Number(slot), 0, PRESET_SLOTS - 1)) invalid(at, `has a preset in ${JSON.stringify(slot)}, which is not a slot`);
      presets[Number(slot)] = readPreset(preset, index, `${at}, slot ${virtualSlotName(Number(slot))}`);
    }
    return { key: setlist.key, name: setlist.name, presets };
  });

  if (!isRecord(current)) invalid(where, "has no active preset");
  if (typeof current.setlistKey !== "string" || !keys.has(trimKey(current.setlistKey))) invalid(where, "names an active setlist it does not hold");
  if (!isIntegerIn(current.position, 0, PRESET_SLOTS - 1)) invalid(where, "has no valid active slot");
  if (typeof current.dirty !== "boolean") invalid(where, "does not say whether the active preset has unsaved changes");
  return {
    device: {
      serial, customName,
      mode: mode as VirtualDeviceState["device"]["mode"],
      modeSlots: (modeSlots as ModeSlot[]).map((slot) => ({ slot: slot.slot, label: slot.label, mode: slot.mode })),
      footswitchModes: [...footswitchModes] as VirtualDeviceState["device"]["footswitchModes"],
      masterVolume,
      masterVolumeAssignment: {
        out12: masterVolumeAssignment.out12 as boolean, out34: masterVolumeAssignment.out34 as boolean,
        send12: masterVolumeAssignment.send12 as boolean, headphones: masterVolumeAssignment.headphones as boolean
      },
      sceneBypassBehavior: sceneBypassBehavior as VirtualDeviceState["device"]["sceneBypassBehavior"]
    },
    current: { setlistKey: current.setlistKey, position: current.position, dirty: current.dirty, preset: readPreset(current.preset, index, `${where}, active preset`) },
    setlists: library
  };
}

/** A block the demo device could not take from a pasted snapshot, and why. */
export interface VirtualPresetSkippedBlock { row: number; column: number; name: string; reason: string; }

/**
 * Build a demo preset from a published snapshot, for a paste that carries no
 * demo-exact preset (a copy taken from real hardware). A snapshot shows only
 * the active scene, so every scene gets that scene's bypass state and values;
 * parameters the snapshot does not carry keep the starting values (measured or
 * synthesized) an added block gets. Blocks whose model the demo catalogue does not offer
 * (Neural Captures, IRs, plugins) are left out and reported, not guessed.
 */
function presetFromSnapshot(index: CatalogueIndex, snapshot: PresetSnapshot): { preset: VirtualPreset; skipped: VirtualPresetSkippedBlock[] } {
  const name = snapshot.presetName.slice(0, QC_STORED_PRESET_NAME_CHARACTERS);
  const preset = blankPreset(name);
  if (snapshot.scenes.length === QC_SCENE_COUNT) preset.scenes = [...snapshot.scenes];
  if (snapshot.sceneColors?.length === QC_SCENE_COUNT) preset.sceneColors = [...snapshot.sceneColors];
  preset.activeScene = isIntegerIn(snapshot.activeScene, 0, QC_SCENE_COUNT - 1) ? snapshot.activeScene : 0;
  preset.tempo = Number.isFinite(snapshot.tempo) ? Math.min(QC_MAXIMUM_TEMPO_BPM, Math.max(QC_MINIMUM_TEMPO_BPM, snapshot.tempo)) : preset.tempo;
  const skipped: VirtualPresetSkippedBlock[] = [];
  for (const block of snapshot.blocks as VirtualGridBlock[]) {
    if (block.column < 0 || block.modelId === undefined) continue;
    const skip = (reason: string) => skipped.push({ row: block.row, column: block.column, name: block.name, reason });
    if (!isIntegerIn(block.row, 0, QC_GRID_ROWS - 1) || !isIntegerIn(block.column, 0, QC_GRID_COLUMNS - 1)) { skip("it is not on the Grid"); continue; }
    const model = index.model(block.modelId);
    if (!model || model.hidden) { skip("the demo device does not simulate this model"); continue; }
    if (blockAt(preset, block.row, block.column)) { skip("another block already holds that cell"); continue; }
    const parameterValues = index.defaults(model.id);
    for (const [key, value] of Object.entries(block.parameterValues ?? {})) {
      const parameter = Number(key);
      if (parameterValues[parameter] !== undefined && isNormalized(value)) parameterValues[parameter] = value;
    }
    preset.blocks.push({
      row: block.row, column: block.column, modelId: block.modelId,
      sceneBypassed: perScene(block.bypassed === true),
      parameterValues,
      ...(isIntegerIn(block.footswitch, 0, QC_SCENE_COUNT - 1) ? {
        footswitch: block.footswitch,
        ...(isIntegerIn(block.footswitchOrder, 0, QC_GRID_ROWS * QC_GRID_COLUMNS) ? { footswitchOrder: block.footswitchOrder } : {}),
        ...(block.footswitchAssignmentType === "primary" || block.footswitchAssignmentType === "secondary" ? { footswitchAssignmentType: block.footswitchAssignmentType } : {})
      } : {}),
      ...(block.bypassExpression ? { bypassExpression: clone(block.bypassExpression) } : {})
    });
  }
  for (const route of snapshot.routes) {
    const target = preset.routes.find((candidate) => candidate.row === route.row);
    if (!target) continue;
    if (isIntegerIn(route.inputId, 0, 0xffff)) target.inputId = route.inputId;
    if (isIntegerIn(route.outputId, 0, 0xffff)) target.outputId = route.outputId;
    if (isIntegerIn(route.splitColumn, 0, QC_GRID_COLUMNS - 1)) target.splitColumn = route.splitColumn;
    if (isIntegerIn(route.mixColumn, 0, QC_GRID_COLUMNS - 1)) target.mixColumn = route.mixColumn;
    target.splitMuted = perScene(route.splitMuted === true);
  }
  for (const state of snapshot.footswitchStates ?? []) {
    const stomp = preset.stomps[state.index];
    if (!stomp) continue;
    if (typeof state.label === "string") stomp.label = state.label;
    if (typeof state.momentary === "boolean") stomp.momentary = state.momentary;
  }
  return { preset, skipped };
}

// ---------------------------------------------------------------------------
// Catalogue

class CatalogueIndex {
  private readonly byId = new Map<number, VirtualCatalogueModel>();
  readonly catalogue: VirtualModelCatalogue;
  readonly measuredDefaults: MeasuredParameterDefaults | undefined;
  constructor(catalogue: VirtualModelCatalogue, measuredDefaults?: MeasuredParameterDefaults) {
    this.catalogue = catalogue;
    this.measuredDefaults = measuredDefaults;
    for (const model of catalogue.models) this.byId.set(model.id, model);
  }
  model(id: number): VirtualCatalogueModel | undefined { return this.byId.get(id); }
  /** A fresh block's starting values: known factory defaults where available, synthesized otherwise. */
  defaults(modelId: number): Record<number, number> { return defaultParameterValues(this.byId.get(modelId), this.measuredDefaults); }
  parameter(modelId: number, index: number): VirtualCatalogueParameter | undefined {
    return this.byId.get(modelId)?.parameters.find((parameter) => parameter.index === index);
  }
}

// ---------------------------------------------------------------------------
// The store

export interface VirtualDeviceStoreOptions {
  storage: VirtualDeviceStorage;
  /** The parsed `docs/reference/qc-live-parameter-scales.json`, or a subset of it. */
  catalogue: VirtualModelCatalogue;
  /**
   * The parsed `docs/reference/qc-live-parameter-defaults.json`: known factory
   * defaults. Without it, or for a model it lacks, every starting value is
   * the synthesized approximation.
   */
  measuredDefaults?: MeasuredParameterDefaults;
  now?: () => number;
  newSessionId?: () => string;
}

/**
 * Owns the virtual device's state and persists it as one JSON document.
 *
 * A write is applied to a copy, persisted, and only then made current, so a
 * failed guard or a failed write leaves both memory and disk untouched rather
 * than letting the two disagree.
 */
export class VirtualDeviceStore {
  private stateValue: VirtualDeviceState;
  private queue: Promise<unknown> = Promise.resolve();
  private readonly storage: VirtualDeviceStorage;
  readonly index: CatalogueIndex;
  readonly now: () => number;
  readonly newSessionId: () => string;

  private constructor(storage: VirtualDeviceStorage, index: CatalogueIndex, state: VirtualDeviceState, now: () => number, newSessionId: () => string) {
    this.storage = storage;
    this.index = index;
    this.stateValue = state;
    this.now = now;
    this.newSessionId = newSessionId;
  }

  /** Load the persisted document, or start from the built-in starter preset when there is none. */
  static async open(options: VirtualDeviceStoreOptions): Promise<VirtualDeviceStore> {
    const index = new CatalogueIndex(options.catalogue, options.measuredDefaults);
    const newSessionId = options.newSessionId ?? (() => globalThis.crypto.randomUUID());
    const text = await options.storage.read();
    let state: VirtualDeviceState;
    if (text === null) {
      state = starterState(index, newSessionId());
    } else {
      let parsed: unknown;
      try { parsed = JSON.parse(text); }
      catch (error) { throw new Error(`The virtual device store is not valid JSON: ${(error as Error).message}`); }
      // Never replace a document this version cannot read: it is the user's library.
      if (!isVirtualDeviceState(parsed)) throw new Error("The virtual device store is not a version 1 virtual device document.");
      // Version 1 predates the built-in Demo factory library. Add it in memory
      // on open and refresh its canonical content on later app versions; the
      // first ordinary write persists the migration alongside the user's data.
      state = withDemoFactoryLibrary(parsed, index);
    }
    return new VirtualDeviceStore(options.storage, index, state, options.now ?? Date.now, newSessionId);
  }

  get catalogue(): VirtualModelCatalogue { return this.index.catalogue; }

  /** A copy of the current state. */
  get state(): VirtualDeviceState { return clone(this.stateValue); }

  /** Persist the current state as it is. */
  save(): Promise<void> { return this.update(() => undefined); }

  /** Apply `mutate` to a copy, persist it, then make it current. Writes run one at a time. */
  update<T>(mutate: (draft: VirtualDeviceState) => T): Promise<T> {
    const run = async () => {
      const draft = clone(this.stateValue);
      const result = mutate(draft);
      await this.storage.write(`${JSON.stringify(draft, null, 2)}\n`);
      this.stateValue = draft;
      return result;
    };
    const next = this.queue.then(run, run);
    this.queue = next.catch(() => undefined);
    return next;
  }

  // -- Transfer (copy and paste as text) --------------------------------------

  /** The active preset exactly as the demo device keeps it: every scene's values, lanes and expressions. */
  currentPreset(): VirtualPreset { return clone(this.stateValue.current.preset); }

  /**
   * Replace the active preset with a pasted one. `preset` is untrusted and is
   * checked field by field first; nothing is written if any of it is invalid.
   * The result is unsaved (dirty), exactly like an edit, so Discard Changes
   * returns to what the slot stores. The preset identity also moves on, which
   * fences every operand captured before the paste (undo entries, open
   * editors) against the replaced content.
   */
  async replaceCurrentPreset(preset: unknown): Promise<void> {
    const next = readPreset(preset, this.index, "The pasted preset");
    await this.update((state) => {
      state.current.preset = next;
      state.current.dirty = true;
      state.addressRevision += 1;
    });
  }

  /** Build a demo preset from a published snapshot (a copy from real hardware), reporting blocks it cannot take. */
  presetFromSnapshot(snapshot: PresetSnapshot): { preset: VirtualPreset; skipped: VirtualPresetSkippedBlock[] } {
    return presetFromSnapshot(this.index, snapshot);
  }

  /**
   * Add a captured setlist at its original slot positions. Every preset is
   * validated before the persisted document changes; name collisions use the
   * same Copy/Copy 2 suffixing as Duplicate Setlist.
   */
  async importSetlist(name: string, presets: Readonly<Record<number, unknown>>): Promise<{ key: string; name: string; presets: number }> {
    const setlistName = validateName(name, "setlist name", 64);
    const checked: Record<number, VirtualPreset> = {};
    for (const [slot, preset] of Object.entries(presets)) {
      const position = Number(slot);
      if (!/^\d+$/.test(slot) || !isIntegerIn(position, 0, PRESET_SLOTS - 1)) throw new Error(`The imported setlist has a preset in ${JSON.stringify(slot)}, which is not a slot.`);
      checked[position] = readPreset(preset, this.index, `The imported setlist, slot ${virtualSlotName(position)}`);
    }
    return this.update((state) => {
      if (state.setlists.length >= LIBRARY_SETLIST_LIMIT) throw new Error(`The demo device already contains the maximum of ${LIBRARY_SETLIST_LIMIT} setlists.`);
      const taken = new Set(state.setlists.map((setlist) => setlist.name.toLowerCase()));
      let importedName = setlistName;
      for (let suffix = 1; taken.has(importedName.toLowerCase()); suffix += 1) importedName = `${setlistName} Copy${suffix === 1 ? "" : ` ${suffix}`}`;
      const key = setlistKeyFor(importedName);
      addUserSetlist(state, { key, name: importedName, presets: clone(checked) });
      return { key, name: importedName, presets: Object.keys(checked).length };
    });
  }

  /**
   * Replace the whole demo library - every setlist, the active preset and the
   * demo device's settings - with a pasted one. Checked in full before anything
   * is written. A new session starts, so no operand from before survives it.
   */
  async replaceLibrary(document: unknown): Promise<void> {
    const library = readLibrary(document, this.index);
    await this.update((state) => {
      state.device = library.device;
      state.current = library.current;
      state.setlists = library.setlists;
      withDemoFactoryLibrary(state, this.index);
      state.session = this.newSessionId();
      state.addressRevision += 1;
    });
  }
}

// ---------------------------------------------------------------------------
// Projection onto the gateway's published shapes

function presetIdentity(state: VirtualDeviceState): string {
  return `virtual-preset-v1.${state.session}.${state.addressRevision}`;
}

function setlistOf(state: VirtualDeviceState, key: string): VirtualSetlist | undefined {
  return state.setlists.find((setlist) => trimKey(setlist.key) === trimKey(key));
}

function effectiveValues(parameters: VirtualParameterState, scene: number): Record<number, number> {
  const values = { ...parameters.parameterValues };
  for (const [index, scenes] of Object.entries(parameters.sceneValues ?? {})) values[Number(index)] = scenes[scene];
  return values;
}

function projectBlock(index: CatalogueIndex, block: VirtualBlock, scene: number): VirtualGridBlock {
  const model = index.model(block.modelId);
  const category = model ? displayCategory(model.category) : "Utility";
  return {
    id: `${block.row}:${block.column}`,
    modelId: block.modelId,
    name: model?.name ?? `Model ${block.modelId}`,
    kind: model ? blockKind(model.category) : "utility",
    category,
    plugin: false,
    row: block.row,
    column: block.column,
    bypassed: block.sceneBypassed[scene] ?? false,
    ...(block.bypassExpression ? { bypassExpression: clone(block.bypassExpression) } : {}),
    ...(block.footswitch !== undefined ? { footswitch: block.footswitch, footswitchOrder: block.footswitchOrder, footswitchAssignmentType: block.footswitchAssignmentType } : {}),
    parameterValues: effectiveValues(block, scene)
  };
}

function projectRoute(route: VirtualRoute, scene: number): GridRoute {
  return {
    row: route.row,
    inputId: route.inputId,
    outputId: route.outputId,
    input: inputLabel(route.inputId),
    output: outputLabel(route.outputId),
    ...(route.splitColumn !== undefined ? { splitColumn: route.splitColumn } : {}),
    ...(route.mixColumn !== undefined ? { mixColumn: route.mixColumn } : {}),
    splitMuted: route.splitMuted[scene] ?? false
  };
}

function projectFootswitches(preset: VirtualPreset, blocks: VirtualGridBlock[]): FootswitchState[] {
  return preset.stomps.map((stomp, footswitch) => {
    const targets = blocks.filter((block) => block.footswitch === footswitch)
      .sort((left, right) => (left.footswitchOrder ?? Infinity) - (right.footswitchOrder ?? Infinity));
    const first = targets[0];
    return {
      index: footswitch,
      active: first?.bypassed === false,
      assigned: targets.length > 0,
      color: first ? QC_CATEGORY_COLORS[qcCategoryKey(first.category ?? "", first.name)] : QC_HARDWARE_COLORS.idleLed,
      momentary: stomp.momentary,
      label: stomp.label
    };
  });
}

function projectSnapshot(index: CatalogueIndex, state: VirtualDeviceState): PresetSnapshot {
  const { device, current } = state;
  const preset = current.preset;
  const scene = preset.activeScene;
  const blocks = preset.blocks.map((block) => projectBlock(index, block, scene))
    .sort((left, right) => left.row - right.row || left.column - right.column);
  return {
    deviceName: device.customName,
    presetName: preset.name,
    presetLocation: virtualSlotName(current.position),
    presetPosition: current.position,
    setlistKey: current.setlistKey,
    setlistName: setlistOf(state, current.setlistKey)?.name ?? "",
    mode: device.mode,
    modeSlots: clone(device.modeSlots),
    footswitchModes: [...device.footswitchModes],
    activeScene: scene,
    scenes: [...preset.scenes],
    sceneColors: [...preset.sceneColors],
    footswitchStates: projectFootswitches(preset, blocks),
    blocks,
    routes: preset.routes.map((route) => projectRoute(route, scene)),
    tempo: preset.tempo,
    tempoLedEnabled: true,
    masterVolume: device.masterVolume,
    dirty: current.dirty,
    presetIdentity: presetIdentity(state)
  };
}

function formatDisplay(parameter: VirtualCatalogueParameter, normalized: number): string {
  if (parameter.options.length) {
    return parameter.options[Math.min(parameter.options.length - 1, Math.round(normalized * (parameter.options.length - 1)))];
  }
  if (parameter.scaleKnown === false) return trimNumber(normalized, 3);
  const real = normalizedToReal(parameter, normalized);
  if (parameter.showAsInteger) return String(Math.round(real));
  return typeof parameter.displayPrecision === "number" ? real.toFixed(parameter.displayPrecision) : trimNumber(real, 3);
}

function projectParameters(model: VirtualCatalogueModel | undefined, state: VirtualParameterState, scene: number, laneControl: boolean): BlockParameter[] {
  if (!model) return [];
  const categoryKey = model.category.toLowerCase().replace(/[^a-z0-9]/g, "");
  const showOffscreen = categoryKey.includes("cabsim") || categoryKey.includes("irloader");
  const values = effectiveValues(state, scene);
  return model.parameters
    .filter((parameter) => laneControl || parameter.screenVisible !== false || showOffscreen)
    .filter((parameter) => values[parameter.index] !== undefined)
    .map((parameter) => {
      const normalized = values[parameter.index];
      const expression = state.expressions?.[parameter.index];
      return {
        index: parameter.index,
        displayPosition: parameter.displayPosition ?? parameter.index,
        name: parameter.name,
        normalizedValue: normalized,
        displayValue: formatDisplay(parameter, normalized),
        units: parameter.units ?? "",
        type: parameter.type,
        minimum: parameter.minimum,
        maximum: parameter.maximum,
        valueScale: parameter.options.length ? "options" : parameter.valueScale,
        scaleExponent: parameter.scaleExponent ?? null,
        scalePoints: clone(parameter.scalePoints ?? []),
        scaleKnown: parameter.options.length > 0 || parameter.scaleKnown !== false,
        displayPrecision: parameter.displayPrecision ?? null,
        minimumLabel: parameter.minimumLabel ?? null,
        midpointLabel: parameter.midpointLabel ?? null,
        maximumLabel: parameter.maximumLabel ?? null,
        showAsInteger: parameter.showAsInteger === true,
        hasOffPosition: false,
        floorDisplay: null,
        floorNormalized: null,
        steps: parameter.steps ?? null,
        sceneMode: state.sceneValues?.[parameter.index] !== undefined,
        options: [...parameter.options],
        writable: parameter.type !== "grMeter",
        enabled: true,
        expressionAssignable: parameter.expressionAssignable !== false,
        linkedSceneMode: parameter.linkedSceneMode ?? null,
        expression: expression?.pedal ?? null,
        expressionMinimum: expression?.minimum ?? null,
        expressionMaximum: expression?.maximum ?? null,
        wireValueKind: "numeric"
      } satisfies BlockParameter;
    });
}

// ---------------------------------------------------------------------------
// Guards, worded as the real gateway words them

function assertPreset(state: VirtualDeviceState, expectedName: string, expectedIdentity: string): void {
  if (!expectedIdentity) throw new Error("A preset identity from current device state is required");
  if (expectedIdentity !== presetIdentity(state)) throw new Error("The preset or QC session changed. Refresh the device state before retrying");
  if (!expectedName) return;
  const actual = state.current.preset.name;
  const placeholder = ["current preset", "empty preset", "unsaved"].includes(expectedName.toLowerCase());
  if (actual !== expectedName && !(actual === "" && placeholder)) {
    throw new Error(`Preset changed on the Quad Cortex: expected ${JSON.stringify(expectedName)}, but ${JSON.stringify(actual || "an unnamed preset")} is active. Refresh and retry.`);
  }
}

function assertScene(state: VirtualDeviceState, expected: number): void {
  const actual = state.current.preset.activeScene;
  if (actual !== expected) throw new Error(`Scene changed on the Quad Cortex: expected ${sceneLetter(expected)}, but ${sceneLetter(actual)} is active. Refresh and retry.`);
}

function assertMode(state: VirtualDeviceState, expected: PresetSnapshot["mode"]): void {
  if (state.device.mode !== expected) throw new Error(`Footswitch mode changed on the Quad Cortex: expected ${expected}, but ${state.device.mode} is active. Refresh and retry.`);
}

function assertPosition(state: VirtualDeviceState, expected: number): void {
  if (state.current.position !== expected) throw new Error("The active preset slot changed on the Quad Cortex. Refresh and retry.");
}

function assertInteger(value: number, minimum: number, maximum: number, field: string): void {
  if (!Number.isInteger(value) || value < minimum || value > maximum) throw new Error(`${field} must be an integer from ${minimum} through ${maximum}`);
}

function assertCell(row: number, column: number): void {
  assertInteger(row, 0, QC_GRID_ROWS - 1, "row");
  assertInteger(column, 0, QC_GRID_COLUMNS - 1, "column");
}

function blockAt(preset: VirtualPreset, row: number, column: number): VirtualBlock | undefined {
  return preset.blocks.find((block) => block.row === row && block.column === column);
}

function assertModel(preset: VirtualPreset, row: number, column: number, expected: number | null): VirtualBlock | undefined {
  const block = blockAt(preset, row, column);
  const actual = block?.modelId ?? null;
  if (actual !== expected) {
    throw new Error(`The block at row ${row}, column ${column} changed: expected model ${rustOption(expected)}, found ${rustOption(actual)}. Refresh and retry.`);
  }
  return block;
}

function requireBlock(preset: VirtualPreset, row: number, column: number, expectedModelId: number): VirtualBlock {
  if (!Number.isInteger(expectedModelId) || expectedModelId < 1) throw new Error("expectedModelId must be the positive model identifier from the observed block details");
  return assertModel(preset, row, column, expectedModelId)!;
}

/** The broker compares option parameters by position and every other value within 1e-5. */
function assertParameterValue(actual: number | undefined, expected: number, parameter: VirtualCatalogueParameter | undefined): void {
  if (actual === undefined) throw new Error("That parameter has no synchronized numeric value");
  const count = parameter?.options.length ?? 0;
  if (count >= 2) {
    if (Math.round(expected * (count - 1)) !== Math.round(actual * (count - 1))) throw new Error("The parameter changed on the Quad Cortex. Refresh and retry.");
    return;
  }
  if (Math.abs(actual - expected) > 0.000_01) throw new Error("The parameter changed on the Quad Cortex. Refresh and retry.");
}

function assertNormalized(value: number, field = "value"): void {
  if (!Number.isFinite(value) || value < 0 || value > 1) throw new Error(`${field} must be a normalized number from 0 through 1`);
}

function routeAt(preset: VirtualPreset, row: number): VirtualRoute {
  const route = preset.routes.find((candidate) => candidate.row === row);
  if (!route) throw new Error("The signal-chain routing for that row is missing. Refresh and retry.");
  return route;
}

function laneState(preset: VirtualPreset, index: CatalogueIndex, row: number, control: LaneControl): VirtualParameterState {
  assertInteger(row, 0, QC_GRID_ROWS - 1, "row");
  if (!(control in LANE_MODELS)) throw new Error("control must be inputGate or laneOutput");
  const lanes = preset.lanes[row] ??= {};
  return lanes[control] ??= { parameterValues: index.defaults(LANE_MODELS[control].modelId) };
}

function currentValue(state: VirtualParameterState, parameterIndex: number, scene: number): number | undefined {
  // A parameter index is a small non-negative integer; anything else (such as "__proto__") would address the prototype chain.
  if (!Number.isInteger(parameterIndex) || parameterIndex < 0 || parameterIndex > PARAMETER_INDEX_LIMIT) return undefined;
  return state.sceneValues?.[parameterIndex]?.[scene] ?? state.parameterValues[parameterIndex];
}

function writeValue(state: VirtualParameterState, parameterIndex: number, scene: number, value: number): void {
  assertInteger(parameterIndex, 0, PARAMETER_INDEX_LIMIT, "parameterIndex");
  const scenes = state.sceneValues?.[parameterIndex];
  if (scenes) scenes[scene] = value;
  else state.parameterValues[parameterIndex] = value;
}

function setSceneMode(state: VirtualParameterState, parameterIndex: number, scene: number, enabled: boolean): void {
  const value = currentValue(state, parameterIndex, scene);
  if (value === undefined) throw new Error("That parameter does not exist on this block.");
  if (enabled) {
    if (state.sceneValues?.[parameterIndex]) return;
    (state.sceneValues ??= {})[parameterIndex] = perScene(value);
  } else if (state.sceneValues?.[parameterIndex]) {
    state.parameterValues[parameterIndex] = value;
    delete state.sceneValues[parameterIndex];
  }
}

function setExpression(state: VirtualParameterState, parameterIndex: number, pedal: 0 | 1 | 2, minimum: number, maximum: number): void {
  if (currentValue(state, parameterIndex, 0) === undefined) throw new Error("That parameter does not exist on this block.");
  if (![0, 1, 2].includes(pedal)) throw new Error("pedal must be 0, 1 or 2");
  if (pedal === 0) { delete state.expressions?.[parameterIndex]; return; }
  assertNormalized(minimum, "minimum");
  assertNormalized(maximum, "maximum");
  (state.expressions ??= {})[parameterIndex] = { pedal, minimum, maximum };
}

function validateName(name: string, field: string, limit: number): string {
  const trimmed = name.trim();
  if (!trimmed) throw new Error(`${field} must not be empty`);
  if (trimmed.length > limit) throw new Error(`${field} must be at most ${limit} characters`);
  if (/[\\/\u0000-\u001f]/.test(trimmed) || trimmed === "." || trimmed === "..") throw new Error(`${field} must not contain slashes or control characters`);
  return trimmed;
}

// ---------------------------------------------------------------------------
// Rejections for the surface a virtual device does not offer

/**
 * The refusal wording for everything outside the virtual surface. It is the
 * sentence today's static demo mode already shows for the tuner and Gig View
 * (`use-performance-workflow.ts`: "Connect the Quad Cortex before opening the
 * tuner."), extended to each feature rather than inventing a second phrasing.
 */
export function virtualDeviceUnavailableMessage(activity: string): string {
  return `Connect the Quad Cortex before ${activity}.`;
}

const unavailable = (activity: string) => () => Promise.reject(new Error(virtualDeviceUnavailableMessage(activity)));

// ---------------------------------------------------------------------------
// The transport

/** Tap Tempo resets when the gap between two taps exceeds this, as a hardware tap does. */
const TAP_RESET_MS = 2_000;

/** Which store backs each virtual transport, so the app can reach the demo's own transfer methods. */
const storesByTransport = new WeakMap<GatewayTransport, VirtualDeviceStore>();

/** The store behind a transport made by {@link createVirtualGatewayTransport}; undefined for any other transport. */
export function virtualDeviceStoreOf(transport: GatewayTransport | undefined): VirtualDeviceStore | undefined {
  return transport ? storesByTransport.get(transport) : undefined;
}

export function createVirtualGatewayTransport(store: VirtualDeviceStore): GatewayTransport {
  const index = store.index;
  let tapTimes: number[] = [];
  let looperStompAssignments = [3, 4, 5, 6, 0, 1, 7, 2];

  const snapshot = (state: VirtualDeviceState) => projectSnapshot(index, state);
  const done = (detail: string, state: VirtualDeviceState, changed = true): DeviceActionResult => ({
    detail, accepted: true, verified: true, verification: "authoritative_readback", changed, snapshot: snapshot(state)
  });
  // recallPreset, navigateBank and reloadPreset report what the real hosts report: the
  // address the host observed in a position update, with no preset body bound to it
  // (packages/rust/quadcortex/src/device/request.rs preset_address_observed_result). The
  // virtual store does hold the body, but the demo is practice on the real UI, so its
  // results read exactly as the hardware's do. The snapshot stays attached, so the UI
  // still navigates by it.
  const addressObserved = (state: VirtualDeviceState, reload: boolean): DeviceActionResult => {
    const { setlistKey, position } = state.current;
    const address = `${trimKey(setlistKey)} position ${position}`;
    return {
      detail: reload
        ? `Active preset address resent; the host observed a position update for ${address} and a clean flag, which do not prove this request caused them, and no preset body can be bound to this reload, so the discard is not verified`
        : `Preset change sent; the host observed a position update for ${address}, which does not prove this request caused it, and no preset body can be bound to that address, so the loaded preset's contents are not verified`,
      accepted: true,
      verified: false,
      verification: "accepted_unverified",
      observation: {
        kind: "preset_address_observed", setlistKey, position, address: "observed", body: "unbound", causallyCorrelated: false,
        ...(reload ? { dirty: state.current.dirty } : {})
      },
      snapshot: snapshot(state)
    };
  };
  const edited = (state: VirtualDeviceState) => { state.current.dirty = true; };
  const readyState = (detail: string, restarted: boolean): ConnectionState => ({
    phase: "ready", detail, demo: false, source: "virtual", restarted, lastSync: new Date(store.now()).toISOString()
  });

  const blockDetailsOf = (state: VirtualDeviceState, row: number, column: number): BlockDetails => {
    const block = blockAt(state.current.preset, row, column);
    if (!block) throw new Error(`No block is placed at row ${row + 1}, column ${column + 1}.`);
    const model = index.model(block.modelId);
    const scene = state.current.preset.activeScene;
    return {
      row, column, modelId: block.modelId,
      name: model?.name ?? `Model ${block.modelId}`,
      category: model ? displayCategory(model.category) : "Utility",
      scene,
      ...(block.bypassExpression ? { bypassExpression: clone(block.bypassExpression) } : {}),
      parameters: projectParameters(model, block, scene, false)
    };
  };

  const laneDetailsOf = (state: VirtualDeviceState, row: number, control: LaneControl): BlockDetails => {
    const lane = laneState(state.current.preset, index, row, control);
    const { modelId, column } = LANE_MODELS[control];
    const model = index.model(modelId);
    const scene = state.current.preset.activeScene;
    return {
      row, column, modelId,
      name: model?.name ?? `Model ${modelId}`,
      category: model ? displayCategory(model.category) : "Utility",
      scene,
      parameters: projectParameters(model, lane, scene, true)
    };
  };

  /** Load a stored preset (or a blank one for an empty slot) as the active preset. */
  const load = (state: VirtualDeviceState, setlistKey: string, position: number) => {
    const setlist = setlistOf(state, setlistKey);
    if (!setlist) throw new Error(`The setlist ${JSON.stringify(setlistKey)} does not exist on this device.`);
    assertInteger(position, 0, PRESET_SLOTS - 1, "position");
    const stored = setlist.presets[position];
    const preset = stored ? clone(stored) : blankPreset("");
    preset.activeScene = 0;
    state.current = { setlistKey: setlist.key, position, dirty: false, preset };
    state.addressRevision += 1;
  };

  const refuseDirty = (state: VirtualDeviceState) => {
    if (state.current.dirty) throw new Error("The current preset has unsaved changes. Save or revert them before recalling another preset.");
  };

  const touchesActive = (state: VirtualDeviceState, setlistKey: string, ...positions: number[]) =>
    trimKey(state.current.setlistKey) === trimKey(setlistKey) && positions.includes(state.current.position);

  const presetList = (state: VirtualDeviceState, setlist: VirtualSetlist) => ({
    setlistKey: setlist.key,
    setlistName: setlist.name,
    currentPosition: trimKey(state.current.setlistKey) === trimKey(setlist.key) ? state.current.position : -1,
    presets: Array.from({ length: PRESET_SLOTS }, (_, position) => ({
      position, location: virtualSlotName(position), name: setlist.presets[position]?.name || "Unsaved", instrument: 0,
      occupied: setlist.presets[position] !== undefined
    })),
    folders: folders(state)
  });

  const folders = (state: VirtualDeviceState): PresetFolder[] => state.setlists.map((setlist) => ({ key: setlist.key, name: setlist.name, isFactory: setlist.isFactory === true }));

  const assertWritableSetlist = (setlist: VirtualSetlist, activity: string) => {
    if (setlist.isFactory) throw new Error(`Factory presets are read-only. Choose a user setlist before ${activity}.`);
  };

  const storedPreset = (state: VirtualDeviceState, setlistKey: string, position: number, expectedName: string, role: "preset" | "source") => {
    const setlist = setlistOf(state, setlistKey);
    if (!setlist) throw new Error(`The setlist ${JSON.stringify(setlistKey)} does not exist on this device.`);
    assertInteger(position, 0, PRESET_SLOTS - 1, "position");
    const stored = setlist.presets[position];
    const label = role === "preset" ? "selected preset" : "selected source";
    if (!stored) throw new Error(`The ${label}${role === "source" ? " preset" : ""} no longer exists${role === "preset" ? " in the device catalog" : ""}. Refresh and retry.`);
    if (stored.name !== expectedName) throw new Error(`The ${label} changed from ${JSON.stringify(expectedName)} to ${JSON.stringify(stored.name)}. Refresh and retry.`);
    return { setlist, stored };
  };

  const selectScene = (state: VirtualDeviceState, scene: number) => {
    assertInteger(scene, 0, QC_SCENE_COUNT - 1, "scene");
    state.current.preset.activeScene = scene;
  };

  const toggleStomp = (state: VirtualDeviceState, footswitch: number): string => {
    const preset = state.current.preset;
    const targets = preset.blocks.filter((block) => block.footswitch === footswitch);
    if (!targets.length) return `Footswitch ${sceneLetter(footswitch)} has no assigned block.`;
    const scene = preset.activeScene;
    const engage = targets[0].sceneBypassed[scene];
    for (const block of targets) block.sceneBypassed[scene] = !engage;
    edited(state);
    return `Footswitch ${sceneLetter(footswitch)} ${engage ? "engaged" : "bypassed"} ${targets.length} block${targets.length === 1 ? "" : "s"}.`;
  };

  const transport: GatewayTransport = {
    // -- Connection and status ------------------------------------------------
    async runtimeStatus() {
      return { platform: "Demo device", gatewayAvailable: true, message: "Demo device: a local session on this computer with no hardware attached." };
    },
    async reconnect() { return readyState("Demo device ready. Nothing is sent to any hardware.", false); },
    async resetSession() {
      await store.update((state) => { state.session = store.newSessionId(); });
      tapTimes = [];
      return readyState("Demo device session restarted.", true);
    },
    async disconnect() {
      return { phase: "disconnected", detail: "Demo device session closed.", demo: false, source: "virtual" };
    },
    async currentSnapshot() { return snapshot(store.state); },
    async currentPreset() { return snapshot(store.state); },
    async deviceDiagnostics() {
      const state = store.state;
      return {
        mode: "virtual",
        session: state.session,
        addressRevision: state.addressRevision,
        setlistCount: state.setlists.length,
        storedPresetCount: state.setlists.reduce((total, setlist) => total + Object.keys(setlist.presets).length, 0),
        catalogueModelCount: store.catalogue.models.length
      };
    },
    // Nothing streams in a virtual session: every change arrives as the result of the call that made it.
    async currentStateEvents() { return { native: false, frames: [] }; },
    async currentTempoClock() { return { available: false }; },
    async currentMasterVolume() { return { value: store.state.device.masterVolume }; },
    async listModels() {
      const visible = store.catalogue.models.filter((model) => !model.hidden);
      const models: ModelEntry[] = visible.map((model) => ({
        // ModelRepo's basedOn and plugin licensing are not in the parameter-scales
        // document, so neither is invented here.
        id: model.id, name: model.name, category: displayCategory(model.category), basedOn: "", pluginId: null, available: true
      })).sort((left, right) => left.category.toLowerCase().localeCompare(right.category.toLowerCase())
        || left.name.toLowerCase().localeCompare(right.name.toLowerCase()) || left.id - right.id);
      return {
        models,
        audit: {
          modelCount: models.length,
          parameterCount: visible.reduce((total, model) => total + model.parameters.length, 0),
          categoryCount: new Set(visible.map((model) => model.category)).size,
          exceptions: []
        }
      };
    },
    async identity() { return identityOf(store.state); },
    async setDeviceName(name) {
      const next = validateName(name, "Device name", 32);
      await store.update((draft) => { draft.device.customName = next; return draft; });
      return { detail: `Device name change to ${next} sent`, accepted: true, verified: false, verification: "accepted_unverified" };
    },

    // -- Grid -----------------------------------------------------------------
    moveBlock(row, fromColumn, toColumn, expectedModelId, toRow, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertCell(row, fromColumn);
        assertCell(toRow, toColumn);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, fromColumn, expectedModelId);
        if (row === toRow && fromColumn === toColumn) return done("Block moved", state, false);
        if (blockAt(preset, toRow, toColumn)) throw new Error(`Row ${toRow + 1}, column ${toColumn + 1} already holds a block. Move or remove it first.`);
        block.row = toRow;
        block.column = toColumn;
        edited(state);
        return done("Block moved", state);
      });
    },
    addBlock(row, column, modelId, expectedPresetName, expectedModelId, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertCell(row, column);
        assertModel(state.current.preset, row, column, expectedModelId);
        const model = index.model(modelId);
        if (!model) throw new Error(`Model ${modelId} is absent from the synchronized QC catalog`);
        if (model.hidden) throw new Error(`Model ${modelId} is not a selectable QC model`);
        // The unit's measured factory defaults where known; a synthesized
        // approximation for any model or parameter not measured.
        state.current.preset.blocks.push({
          row, column, modelId, sceneBypassed: perScene(false), parameterValues: index.defaults(modelId)
        });
        edited(state);
        return done(`${model.name} placed`, state);
      });
    },
    replaceBlock(row, column, modelId, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertCell(row, column);
        const block = requireBlock(state.current.preset, row, column, expectedModelId);
        const model = index.model(modelId);
        if (!model || model.hidden) throw new Error(`Model ${modelId} is not a selectable QC model`);
        const changed = block.modelId !== modelId;
        block.modelId = modelId;
        block.parameterValues = index.defaults(modelId);
        if (changed) edited(state);
        return done(`${model.name} replaced the selected device`, state, changed);
      });
    },
    createModelPreset(row, column, modelHash, name, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        requireBlock(state.current.preset, row, column, modelHash);
        validateName(name, "Device preset name", 80);
        return done(`Saved ${name} as a user device preset`, state, true);
      });
    },
    setLooperStompAssignments(assignments, expectedAssignments) {
      if (JSON.stringify(looperStompAssignments) !== JSON.stringify(expectedAssignments)) return Promise.reject(new Error("The Looper X assignments changed. Refresh and retry."));
      const sorted = [...assignments].sort((a, b) => a - b);
      if (sorted.some((value, index) => value !== index) || sorted.length !== 8) return Promise.reject(new Error("Looper X assignments must use every action exactly once."));
      const changed = JSON.stringify(assignments) !== JSON.stringify(looperStompAssignments);
      looperStompAssignments = [...assignments];
      return Promise.resolve({ detail: "Looper X footswitch actions updated", accepted: true, verified: true, verification: "authoritative_readback", changed, snapshot: snapshot(store.state) });
    },
    resetBlockParameters(row, column, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertCell(row, column);
        const block = requireBlock(state.current.preset, row, column, expectedModelId);
        block.parameterValues = index.defaults(block.modelId);
        edited(state);
        return done("Block parameters reset to defaults", state);
      });
    },
    removeBlock(row, column, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertCell(row, column);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, column, expectedModelId);
        preset.blocks = preset.blocks.filter((candidate) => candidate !== block);
        edited(state);
        return done("Block removed", state);
      });
    },
    toggleBypass(row, column, expectedScene, expectedBypassed, desiredBypassed, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertScene(state, expectedScene);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, column, expectedModelId);
        if (block.sceneBypassed[preset.activeScene] !== expectedBypassed) throw new Error("The block bypass state changed on the Quad Cortex. Refresh and retry.");
        const changed = block.sceneBypassed[preset.activeScene] !== desiredBypassed;
        block.sceneBypassed[preset.activeScene] = desiredBypassed;
        if (changed) edited(state);
        return done(desiredBypassed ? "Block bypassed" : "Block engaged", state, changed);
      });
    },
    setBlockFootswitch(row, column, footswitch, assignmentType, expectedFootswitch, expectedAssignmentType, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, column, expectedModelId);
        if ((block.footswitch ?? null) !== expectedFootswitch) throw new Error("The block footswitch assignment changed on the Quad Cortex. Refresh and retry.");
        if ((block.footswitchAssignmentType ?? null) !== expectedAssignmentType) throw new Error("The block footswitch assignment type changed on the Quad Cortex. Refresh and retry.");
        if (footswitch === null) {
          delete block.footswitch;
          delete block.footswitchOrder;
          delete block.footswitchAssignmentType;
        } else {
          assertInteger(footswitch, 0, QC_SCENE_COUNT - 1, "footswitch");
          if (assignmentType !== "primary" && assignmentType !== "secondary") throw new Error("assignmentType must be primary or secondary");
          if (block.footswitch !== footswitch) {
            block.footswitchOrder = preset.blocks.filter((candidate) => candidate !== block && candidate.footswitch === footswitch).length;
          }
          block.footswitch = footswitch;
          block.footswitchAssignmentType = assignmentType;
        }
        edited(state);
        return done("setFootswitch sent to the Quad Cortex", state);
      });
    },
    async blockDetails(row, column, expectedPresetName, expectedPresetIdentity) {
      const state = store.state;
      assertPreset(state, expectedPresetName, expectedPresetIdentity);
      return blockDetailsOf(state, row, column);
    },
    async laneControlDetails(row, control, expectedPresetName, expectedPresetIdentity) {
      const state = store.state;
      assertPreset(state, expectedPresetName, expectedPresetIdentity);
      return laneDetailsOf(state, row, control);
    },

    // -- Parameters -----------------------------------------------------------
    setParameter(row, column, parameterIndex, value, expectedValue, expectedScene, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertScene(state, expectedScene);
        assertNormalized(value);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, column, expectedModelId);
        assertParameterValue(currentValue(block, parameterIndex, preset.activeScene), expectedValue, index.parameter(block.modelId, parameterIndex));
        writeValue(block, parameterIndex, preset.activeScene, value);
        edited(state);
        return { ...done("Parameter set", state), block: blockDetailsOf(state, row, column) };
      });
    },
    previewParameter(row, column, parameterIndex, value, expectedValue, expectedScene, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertScene(state, expectedScene);
        assertNormalized(value);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, column, expectedModelId);
        assertParameterValue(currentValue(block, parameterIndex, preset.activeScene), expectedValue, index.parameter(block.modelId, parameterIndex));
        writeValue(block, parameterIndex, preset.activeScene, value);
        edited(state);
        return { detail: "Parameter preview sent", acceptedValue: value };
      });
    },
    setParameterSceneMode(row, column, parameterIndex, enabled, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const preset = state.current.preset;
        const block = requireBlock(preset, row, column, expectedModelId);
        setSceneMode(block, parameterIndex, preset.activeScene, enabled);
        edited(state);
        return done(`Scene mode ${enabled ? "enabled" : "disabled"}`, state);
      });
    },
    setParameterExpression(row, column, parameterIndex, pedal, minimum, maximum, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const block = requireBlock(state.current.preset, row, column, expectedModelId);
        setExpression(block, parameterIndex, pedal, minimum, maximum);
        edited(state);
        return done(pedal === 0 ? "Expression assignment removed" : `Expression pedal ${pedal} assigned`, state);
      });
    },
    setExpressionBypass(row, column, pedal, mode, invert, delayMs, latchEmulation, expectedModelId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const block = requireBlock(state.current.preset, row, column, expectedModelId);
        if (![0, 1, 2].includes(pedal)) throw new Error("pedal must be 0, 1 or 2");
        if (![0, 1, 2].includes(mode)) throw new Error("mode must be 0, 1 or 2");
        if (!Number.isFinite(delayMs) || delayMs < 0) throw new Error("delayMs must be a non-negative number");
        // Pedal 0 is the clear, and mode 0 is not. Mode 0 is STOP, an ordinary
        // switch behaviour, so deleting the assignment for it invented a clear
        // the device does not have while refusing the one it does. A cleared
        // bypass carries no assignment at all rather than a zero pedal, which
        // is what the hardware returns.
        if (pedal === 0) delete block.bypassExpression;
        else block.bypassExpression = { pedal, minimum: 0, maximum: 1, mode, invert, delayMs, latchEmulation };
        edited(state);
        return done(pedal === 0 ? "Expression bypass removed" : "Expression bypass set", state);
      });
    },
    setLaneControlParameter(row, control, parameterIndex, value, expectedValue, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertNormalized(value);
        const preset = state.current.preset;
        const lane = laneState(preset, index, row, control);
        assertParameterValue(currentValue(lane, parameterIndex, preset.activeScene), expectedValue, index.parameter(LANE_MODELS[control].modelId, parameterIndex));
        writeValue(lane, parameterIndex, preset.activeScene, value);
        edited(state);
        return { ...done("Lane control parameter set", state), block: laneDetailsOf(state, row, control) };
      });
    },
    previewLaneControlParameter(row, control, parameterIndex, value, expectedValue, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertNormalized(value);
        const preset = state.current.preset;
        const lane = laneState(preset, index, row, control);
        assertParameterValue(currentValue(lane, parameterIndex, preset.activeScene), expectedValue, index.parameter(LANE_MODELS[control].modelId, parameterIndex));
        writeValue(lane, parameterIndex, preset.activeScene, value);
        edited(state);
        return { detail: "Lane control preview sent", acceptedValue: value };
      });
    },
    setLaneControlSceneMode(row, control, parameterIndex, enabled, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const preset = state.current.preset;
        setSceneMode(laneState(preset, index, row, control), parameterIndex, preset.activeScene, enabled);
        edited(state);
        return done(`Scene mode ${enabled ? "enabled" : "disabled"}`, state);
      });
    },
    setLaneControlExpression(row, control, parameterIndex, pedal, minimum, maximum, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        setExpression(laneState(state.current.preset, index, row, control), parameterIndex, pedal, minimum, maximum);
        edited(state);
        return done(pedal === 0 ? "Expression assignment removed" : `Expression pedal ${pedal} assigned`, state);
      });
    },

    // -- Scenes ---------------------------------------------------------------
    selectScene(scene, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        selectScene(state, scene);
        return done(`Scene ${sceneLetter(scene)} selected`, state);
      });
    },
    copyScene(fromScene, toScene, swap, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(fromScene, 0, QC_SCENE_COUNT - 1, "fromScene");
        assertInteger(toScene, 0, QC_SCENE_COUNT - 1, "toScene");
        const preset = state.current.preset;
        // A scene is the per-scene state: block bypasses, scene-mode parameter
        // values and splitter/mixer mutes. Labels and colours stay put.
        const exchange = <T,>(values: T[]) => {
          const source = values[fromScene];
          if (swap) values[fromScene] = values[toScene];
          values[toScene] = source;
        };
        for (const block of preset.blocks) {
          exchange(block.sceneBypassed);
          for (const scenes of Object.values(block.sceneValues ?? {})) exchange(scenes);
        }
        for (const lanes of Object.values(preset.lanes)) {
          for (const lane of Object.values(lanes)) for (const scenes of Object.values(lane?.sceneValues ?? {})) exchange(scenes);
        }
        for (const route of preset.routes) exchange(route.splitMuted);
        edited(state);
        return done(`Scene ${sceneLetter(fromScene)} ${swap ? "swapped with" : "copied to"} Scene ${sceneLetter(toScene)}`, state);
      });
    },
    setSceneLabel(scene, label, expectedLabel, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(scene, 0, QC_SCENE_COUNT - 1, "scene");
        const preset = state.current.preset;
        if (preset.scenes[scene] !== expectedLabel) throw new Error("The scene label changed on the Quad Cortex. Refresh and retry.");
        preset.scenes[scene] = label === null || label === "" ? defaultSceneLabel(scene) : validateName(label, "Scene label", 32);
        edited(state);
        return done("setSceneLabel sent to the Quad Cortex", state);
      });
    },
    setSceneColor(scene, color, expectedColor, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(scene, 0, QC_SCENE_COUNT - 1, "scene");
        if (!/^#[0-9a-f]{6}$/i.test(expectedColor)) throw new Error("expectedColor must be a #rrggbb scene color from the current preset");
        if (!Number.isInteger(color) || color < 0 || color > 0xffff_ffff) throw new Error("color must be a 32-bit ARGB integer");
        const preset = state.current.preset;
        if (preset.sceneColors[scene]?.toLowerCase() !== expectedColor.toLowerCase()) throw new Error("The scene color changed on the Quad Cortex. Refresh and retry.");
        preset.sceneColors[scene] = `#${(color & 0xff_ffff).toString(16).padStart(6, "0")}`;
        edited(state);
        return done("setSceneColor sent to the Quad Cortex", state);
      });
    },
    setSceneBypassBehavior(behavior) {
      return store.update((state) => {
        if (!["alwaysOverwrite", "nonstompOverwrite", "neverOverwrite"].includes(behavior)) throw new Error("behavior must be alwaysOverwrite, nonstompOverwrite or neverOverwrite");
        state.device.sceneBypassBehavior = behavior;
        return done("Scene bypass behavior set", state);
      });
    },

    // -- Routing --------------------------------------------------------------
    setChainInput(row, inputId, expectedInputId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(row, 0, QC_GRID_ROWS - 1, "row");
        if (!QC_INPUT_ROUTES.some((route) => route.id === inputId)) throw new Error(`inputId ${inputId} is not a Quad Cortex input route`);
        const route = routeAt(state.current.preset, row);
        if (route.inputId !== expectedInputId) throw new Error("The signal-chain routing changed on the Quad Cortex. Refresh and retry.");
        route.inputId = inputId;
        edited(state);
        return done("setChainInput sent to the Quad Cortex", state);
      });
    },
    setChainOutput(row, outputId, expectedOutputId, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(row, 0, QC_GRID_ROWS - 1, "row");
        if (!CHAIN_ASSIGNABLE_OUTPUTS.includes(outputId)) {
          throw new Error(`outputId must be one of the assignable chain outputs ${CHAIN_ASSIGNABLE_OUTPUTS.join(", ")}`);
        }
        const route = routeAt(state.current.preset, row);
        if (route.outputId !== expectedOutputId) throw new Error("The signal-chain routing changed on the Quad Cortex. Refresh and retry.");
        route.outputId = outputId;
        edited(state);
        return done("setChainOutput sent to the Quad Cortex", state);
      });
    },
    setChainSplit(row, splitColumn, mixColumn, expectedSplitColumn, expectedMixColumn, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        if (!PARALLEL_ROWS.includes(row)) throw new Error("row must be 0 or 2: only rows 1 and 3 can split into a parallel path");
        const route = routeAt(state.current.preset, row);
        if ((route.splitColumn ?? null) !== expectedSplitColumn || (route.mixColumn ?? null) !== expectedMixColumn) {
          throw new Error("The signal-chain routing changed on the Quad Cortex. Refresh and retry.");
        }
        if (splitColumn !== null) assertInteger(splitColumn, 0, QC_GRID_COLUMNS - 1, "splitColumn");
        if (mixColumn !== null) assertInteger(mixColumn, 0, QC_GRID_COLUMNS - 1, "mixColumn");
        if (mixColumn !== null && (splitColumn === null || mixColumn <= splitColumn)) throw new Error("mixColumn must follow splitColumn");
        if (splitColumn === null) { delete route.splitColumn; delete route.mixColumn; }
        else {
          route.splitColumn = splitColumn;
          if (mixColumn === null) delete route.mixColumn;
          else route.mixColumn = mixColumn;
        }
        edited(state);
        return done("setChainSplit sent to the Quad Cortex", state);
      });
    },
    setSplitMute(row, muted, expectedMuted, expectedScene, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertScene(state, expectedScene);
        if (!PARALLEL_ROWS.includes(row)) throw new Error("row must be 0 or 2: only rows 1 and 3 can split into a parallel path");
        const preset = state.current.preset;
        const route = routeAt(preset, row);
        if ((route.splitMuted[preset.activeScene] ?? false) !== expectedMuted) throw new Error("The splitter/mixer mute state changed on the Quad Cortex. Refresh and retry.");
        route.splitMuted[preset.activeScene] = muted;
        edited(state);
        return done("setSplitMute sent to the Quad Cortex", state);
      });
    },

    // -- Tempo, volume, footswitches and modes ---------------------------------
    setTempo(bpm, expectedTempo, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const preset = state.current.preset;
        if (preset.tempo !== expectedTempo) throw new Error(`Tempo changed on the Quad Cortex: expected ${expectedTempo}, but ${preset.tempo} is active. Refresh and retry.`);
        assertInteger(bpm, QC_MINIMUM_TEMPO_BPM, QC_MAXIMUM_TEMPO_BPM, "bpm");
        const changed = preset.tempo !== bpm;
        preset.tempo = bpm;
        if (changed) edited(state);
        return done(`Tempo set to ${bpm} BPM`, state, changed);
      });
    },
    tapTempo(expectedMode, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertMode(state, expectedMode);
        const now = store.now();
        const last = tapTimes.at(-1);
        tapTimes = last !== undefined && now - last <= TAP_RESET_MS ? [...tapTimes, now].slice(-4) : [now];
        if (tapTimes.length < 2) return done("Tap Tempo received. Tap again to set the tempo.", state, false);
        const interval = (tapTimes[tapTimes.length - 1] - tapTimes[0]) / (tapTimes.length - 1);
        const bpm = Math.min(QC_MAXIMUM_TEMPO_BPM, Math.max(QC_MINIMUM_TEMPO_BPM, Math.round(60_000 / interval)));
        const preset = state.current.preset;
        const changed = preset.tempo !== bpm;
        preset.tempo = bpm;
        if (changed) edited(state);
        return done(`Tempo tapped to ${bpm} BPM`, state, changed);
      });
    },
    setMasterVolume(value, expectedValue) {
      return store.update((state) => {
        if (state.device.masterVolume !== expectedValue) {
          throw new Error(`Master Volume changed on the Quad Cortex: expected ${expectedValue}, but ${state.device.masterVolume} is active. Refresh and retry.`);
        }
        assertInteger(value, 0, 100, "value");
        state.device.masterVolume = value;
        return done(`Master Volume set to ${value}`, state, expectedValue !== value);
      });
    },
    setMasterVolumeAssignment(out12, out34, send12, headphones) {
      return store.update((state) => {
        state.device.masterVolumeAssignment = { out12, out34, send12, headphones };
        return done("Master Volume assignment set", state);
      });
    },
    pressFootswitch(footswitch, expectedMode, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertMode(state, expectedMode);
        assertInteger(footswitch, 0, QC_SCENE_COUNT - 1, "index");
        if (state.device.mode === "SCENE") {
          selectScene(state, footswitch);
          return done(`Scene ${sceneLetter(footswitch)} selected`, state);
        }
        if (state.device.mode === "PRESET") {
          refuseDirty(state);
          const position = Math.floor(state.current.position / 8) * 8 + footswitch;
          load(state, state.current.setlistKey, position);
          return done(`Preset ${virtualSlotName(position)} recalled`, state);
        }
        // STOMP, and HYBRID's stomp half: toggle the blocks assigned to the switch.
        return done(toggleStomp(state, footswitch), state);
      });
    },
    selectModeSlot(slot, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        const target = state.device.modeSlots.find((candidate) => candidate.slot === slot);
        if (!target) throw new Error("slot must be 0, 1 or 2");
        state.device.mode = target.mode;
        return done(`${target.label} mode selected`, state);
      });
    },
    setStompLabel(footswitch, label, expectedLabel, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(footswitch, 0, QC_SCENE_COUNT - 1, "footswitch");
        const stomp = state.current.preset.stomps[footswitch];
        if (stomp.label !== expectedLabel) throw new Error("The STOMP label changed on the Quad Cortex. Refresh and retry.");
        if (label.length > 32) throw new Error("label must be at most 32 characters");
        stomp.label = label;
        edited(state);
        return done("STOMP label set", state);
      });
    },
    setStompMomentary(footswitch, momentary, expectedMomentary, expectedPresetName, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertInteger(footswitch, 0, QC_SCENE_COUNT - 1, "footswitch");
        const stomp = state.current.preset.stomps[footswitch];
        if (stomp.momentary !== expectedMomentary) throw new Error("The STOMP momentary setting changed on the Quad Cortex. Refresh and retry.");
        stomp.momentary = momentary;
        edited(state);
        return done(`STOMP footswitch set to ${momentary ? "momentary" : "latching"}`, state);
      });
    },

    // -- Presets and library ---------------------------------------------------
    async listPresets(_refresh, setlistKey) {
      const state = store.state;
      const setlist = setlistOf(state, setlistKey ?? state.current.setlistKey);
      if (!setlist) throw new Error(`The setlist ${JSON.stringify(setlistKey)} does not exist on this device.`);
      return presetList(state, setlist);
    },
    async listPresetFolders() { return { folders: folders(store.state) }; },
    async listPresetSlots() {
      const state = store.state;
      const setlist = setlistOf(state, state.current.setlistKey);
      if (!setlist || setlist.isFactory) throw new Error("The active preset is not in a writable user setlist, so it has no destination slots to list.");
      return {
        setlistKey: setlist.key,
        setlistName: setlist.name,
        currentPosition: state.current.position,
        slots: Array.from({ length: PRESET_SLOTS }, (_, position) => {
          const stored = setlist.presets[position];
          return { position, location: virtualSlotName(position), occupied: stored !== undefined, name: stored?.name ?? "", instrument: 0 };
        })
      };
    },
    navigateBank(direction, expectedPresetName, expectedPosition, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertPosition(state, expectedPosition);
        refuseDirty(state);
        if (direction !== -1 && direction !== 1) throw new Error("direction must be -1 or 1");
        const target = state.current.position + direction * 8;
        if (target < 0 || target >= PRESET_SLOTS) throw new Error("Already at the first or last preset bank.");
        load(state, state.current.setlistKey, target);
        return addressObserved(state, false);
      });
    },
    recallPreset(setlistKey, position, expectedPresetName, expectedPosition, expectedSetlistKey, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertPosition(state, expectedPosition);
        if (expectedSetlistKey && trimKey(state.current.setlistKey) !== trimKey(expectedSetlistKey)) {
          throw new Error("The active setlist changed on the Quad Cortex. Refresh and retry.");
        }
        refuseDirty(state);
        load(state, setlistKey, position);
        return addressObserved(state, false);
      });
    },
    reloadPreset(expectedPresetName, expectedPosition, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertPosition(state, expectedPosition);
        load(state, state.current.setlistKey, state.current.position);
        return addressObserved(state, true);
      });
    },
    savePresetAs(setlistKey, position, name, expectedPresetName, expectedPosition, confirmOverwrite, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertPosition(state, expectedPosition);
        const savedName = validateName(name, "Preset name", QC_STORED_PRESET_NAME_CHARACTERS);
        const setlist = setlistOf(state, setlistKey);
        if (!setlist) throw new Error(`The setlist ${JSON.stringify(setlistKey)} does not exist on this device.`);
        assertWritableSetlist(setlist, "saving");
        assertInteger(position, 0, PRESET_SLOTS - 1, "position");
        if (setlist.presets[position] && !touchesActive(state, setlist.key, position) && !confirmOverwrite) {
          throw new Error(`Slot ${virtualSlotName(position)} already holds ${JSON.stringify(setlist.presets[position].name)}. Confirm the overwrite to replace it.`);
        }
        const preset = clone(state.current.preset);
        preset.name = savedName;
        setlist.presets[position] = clone(preset);
        state.current = { setlistKey: setlist.key, position, dirty: false, preset };
        state.addressRevision += 1;
        return { ...done(`Preset saved as ${savedName}`, state), savedName };
      });
    },
    renameCurrentPreset(name, expectedPresetName, expectedPosition, confirmRename, expectedPresetIdentity) {
      return store.update((state) => {
        assertPreset(state, expectedPresetName, expectedPresetIdentity);
        assertPosition(state, expectedPosition);
        if (!confirmRename) throw new Error("Renaming a stored preset requires explicit confirmation.");
        if (state.current.dirty) throw new Error("The current preset has unsaved changes. Save or revert them before renaming it.");
        const savedName = validateName(name, "Preset name", QC_STORED_PRESET_NAME_CHARACTERS);
        const setlist = setlistOf(state, state.current.setlistKey);
        if (!setlist?.presets[state.current.position]) throw new Error("The active preset has not been saved to a slot yet. Use Save As instead.");
        assertWritableSetlist(setlist, "renaming");
        setlist.presets[state.current.position].name = savedName;
        state.current.preset.name = savedName;
        state.addressRevision += 1;
        return { ...done(`Preset renamed to ${savedName}`, state), savedName };
      });
    },
    copyPreset(sourceSetlistKey, sourcePosition, sourceName, destinationSetlistKey, destinationPosition, confirmOverwrite) {
      return store.update((state) => {
        // Virtual device only (not evidence of unit behaviour): copy takes no
        // active-preset guard, matching the real File COPY frame.
        const { stored } = storedPreset(state, sourceSetlistKey, sourcePosition, sourceName, "source");
        const destination = setlistOf(state, destinationSetlistKey);
        if (!destination) throw new Error(`The setlist ${JSON.stringify(destinationSetlistKey)} does not exist on this device.`);
        assertWritableSetlist(destination, "pasting");
        assertInteger(destinationPosition, 0, PRESET_SLOTS - 1, "destinationPosition");
        if (destination.presets[destinationPosition] && !confirmOverwrite) {
          throw new Error(`Slot ${virtualSlotName(destinationPosition)} already holds ${JSON.stringify(destination.presets[destinationPosition].name)}. Confirm the overwrite to replace it.`);
        }
        destination.presets[destinationPosition] = clone(stored);
        if (touchesActive(state, destination.key, destinationPosition)) load(state, destination.key, destinationPosition);
        return { ...done(`Preset copied to ${virtualSlotName(destinationPosition)}`, state), savedName: stored.name };
      });
    },
    createSetlist(name) {
      return store.update((state) => {
        const setlistName = validateName(name, "setlist name", 64);
        if (state.setlists.some((setlist) => setlist.name.toLowerCase() === setlistName.toLowerCase())) {
          throw new Error(`A setlist named ${JSON.stringify(setlistName)} already exists.`);
        }
        const key = setlistKeyFor(setlistName);
        addUserSetlist(state, { key, name: setlistName, presets: {} });
        return { ...done(`Setlist ${setlistName} created`, state), setlistKey: key };
      });
    },
    deleteSetlist(name) {
      return store.update((state) => {
        const setlist = state.setlists.find((candidate) => candidate.name === name);
        if (!setlist) throw new Error(`No setlist named ${JSON.stringify(name)} exists.`);
        assertWritableSetlist(setlist, "deleting the setlist");
        if (trimKey(setlist.key) === trimKey(state.current.setlistKey)) {
          throw new Error("The active preset belongs to this setlist. Recall a preset from another setlist before deleting it.");
        }
        state.setlists = state.setlists.filter((candidate) => candidate !== setlist);
        return done(`Setlist ${name} deleted`, state);
      });
    },
    duplicateSetlist(sourceSetlistKey, expectedSourceName) {
      return store.update((state) => {
        const source = setlistOf(state, sourceSetlistKey);
        if (!source) throw new Error("The selected source setlist no longer exists. Refresh and retry.");
        if (source.name !== expectedSourceName) throw new Error(`The selected source changed from ${JSON.stringify(expectedSourceName)} to ${JSON.stringify(source.name)}. Refresh and retry.`);
        const taken = new Set(state.setlists.map((setlist) => setlist.name.toLowerCase()));
        let copyName = `${source.name} Copy`;
        for (let suffix = 2; taken.has(copyName.toLowerCase()); suffix += 1) copyName = `${source.name} Copy ${suffix}`;
        const key = setlistKeyFor(copyName);
        addUserSetlist(state, { key, name: copyName, presets: clone(source.presets) });
        return { ...done(`Setlist duplicated as ${copyName}`, state), setlistKey: key };
      });
    },
    deletePreset(setlistKey, position, expectedName) {
      return store.update((state) => {
        const { setlist } = storedPreset(state, setlistKey, position, expectedName, "preset");
        assertWritableSetlist(setlist, "deleting the preset");
        delete setlist.presets[position];
        // The active preset keeps its unsaved content, now at an empty address.
        if (touchesActive(state, setlist.key, position)) state.addressRevision += 1;
        return done(`Preset ${virtualSlotName(position)} deleted`, state);
      });
    },
    movePreset(setlistKey, sourcePosition, sourceName, destinationPosition) {
      return store.update((state) => {
        const { setlist, stored } = storedPreset(state, setlistKey, sourcePosition, sourceName, "source");
        assertWritableSetlist(setlist, "moving the preset");
        assertInteger(destinationPosition, 0, PRESET_SLOTS - 1, "destinationPosition");
        if (sourcePosition === destinationPosition) return done("Preset left in place", state, false);
        // Into an occupied slot the two presets swap; into an empty one the source moves.
        const displaced = setlist.presets[destinationPosition];
        setlist.presets[destinationPosition] = stored;
        if (displaced) setlist.presets[sourcePosition] = displaced;
        else delete setlist.presets[sourcePosition];
        if (touchesActive(state, setlist.key, sourcePosition, destinationPosition)) {
          if (state.current.position === sourcePosition) state.current.position = destinationPosition;
          else if (displaced) state.current.position = sourcePosition;
          state.addressRevision += 1;
        }
        return done(displaced ? "Presets swapped" : "Preset moved", state);
      });
    },

    // -- Not offered by a virtual device ----------------------------------------
    undo: unavailable("using the device's undo history"),
    redo: unavailable("using the device's redo history"),
    inhibitedModules: unavailable("reading global gate and EQ availability"),
    tunerSettings: unavailable("reading tuner settings"),
    setTunerInput: unavailable("changing tuner settings"),
    setTunerMute: unavailable("changing tuner settings"),
    setTunerReference: unavailable("changing tuner settings"),
    setTunerMeter: unavailable("changing tuner settings"),
    async generalSettings() { return { looperStompAssignments: [...looperStompAssignments] }; },
    ioSettings: unavailable("reading I/O settings"),
    setInputPort: unavailable("changing I/O settings"),
    setOutputPort: unavailable("changing I/O settings"),
    setUsbPort: unavailable("changing I/O settings"),
    setMidiThru: unavailable("changing MIDI settings"),
    setOutputPairing: unavailable("changing I/O settings"),
    globalEq: unavailable("reading the global EQ"),
    setGlobalEqBypassed: unavailable("changing the global EQ"),
    setGlobalEqBand: unavailable("changing the global EQ"),
    setGlobalEqOutput: unavailable("changing the global EQ"),
    loadGlobalEqModelPreset: unavailable("loading a global EQ preset"),
    modeCycle: unavailable("reading the mode cycle"),
    setModeCycle: unavailable("changing the mode cycle"),
    globalTempoSettings: unavailable("reading global tempo settings"),
    setTempoMetronome: unavailable("changing the metronome"),
    setTempoMode: unavailable("changing the tempo mode"),
    setGlobalTempo: unavailable("changing the global tempo"),
    looperStatus: unavailable("using the looper"),
    controlLooper: unavailable("using the looper"),
    recents: unavailable("browsing recent models"),
    favorites: unavailable("browsing favorites"),
    setFavorite: unavailable("changing favorites"),
    pinnedModels: unavailable("browsing pinned models"),
    setModelPinned: unavailable("pinning models"),
    captures: unavailable("browsing Neural Captures"),
    loadCapture: unavailable("loading a Neural Capture"),
    irs: unavailable("browsing impulse responses"),
    loadIr: unavailable("loading an impulse response"),
    setGeneralInteger: unavailable("changing device settings"),
    setGeneralToggle: unavailable("changing device settings"),
    setGlobalBypass: unavailable("changing global cab and IR bypass"),
    presetScreenshot: unavailable("reading preset screenshots"),
    captureScreen: unavailable("mirroring the device screen"),
    graphicsTree: unavailable("reading the device screen"),
    tapScreen: unavailable("touching the device screen"),
    longTapScreen: unavailable("touching the device screen"),
    swipeScreen: unavailable("touching the device screen"),
    dragScreen: unavailable("touching the device screen"),
    mouseFrame: unavailable("touching the device screen"),
    setMidiOut: unavailable("editing MIDI out messages"),
    setPresetLoadMidiOut: unavailable("editing MIDI out messages"),
    showTuner: unavailable("opening the tuner"),
    showGigView: unavailable("opening Gig View"),
    createDeviceBackup: unavailable("creating a device backup")
  };
  storesByTransport.set(transport, store);
  return transport;
}

function identityOf(state: VirtualDeviceState): DeviceIdentity {
  return { serial: state.device.serial, appFwVersion: "virtual", customName: state.device.customName };
}
