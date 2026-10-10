import {
  QC_GRID_COLUMNS,
  QC_GRID_ROWS,
  QC_SCENE_COUNT,
  requirePresetIdentity,
  type BlockDetails,
  type DeviceActionResult,
  type GatewayTransport,
  type GridBlock,
  type GridRoute,
  type LaneControl,
  type PresetSnapshot
} from "@qc-remote/client";
import type { VirtualDeviceState, VirtualDeviceStore, VirtualGridBlock, VirtualParameterState, VirtualPreset, VirtualPresetSkippedBlock } from "./virtual-gateway.ts";

/**
 * Copy and paste a preset, or the whole demo library, as plain text.
 *
 * A preset travels as its published `PresetSnapshot`, with each Grid block
 * carrying the `parameterValues` field the demo device already publishes. A
 * copy taken from the demo device also carries the demo's own exact preset
 * (every scene's values, lanes and expressions), so a demo-to-demo paste is
 * lossless. The same text pastes onto real hardware by replaying it as the
 * ordinary single-block and single-parameter writes the app already makes.
 */

export const PRESET_TEXT_FORMAT = "qc-remote.preset";
export const DEMO_LIBRARY_TEXT_FORMAT = "qc-remote.demo-library";

/**
 * Clipboard text can be any size. A preset is tens of kilobytes (a full Grid
 * with every parameter is well under 200 KB), so anything past this is not a
 * preset and is refused before it is parsed rather than hanging the window.
 */
export const PRESET_TEXT_MAX_CHARACTERS = 1_000_000;
/**
 * A full demo library of 256 presets is a few megabytes. The cap stays well
 * under the 64 MB the native store accepts for one document.
 */
export const DEMO_LIBRARY_TEXT_MAX_CHARACTERS = 32_000_000;

/** Which parameter values a copied preset holds, so a paste can say so honestly. */
export type PresetTextValues = "complete" | "partial" | "none";

export interface PresetTextDocument {
  format: typeof PRESET_TEXT_FORMAT;
  version: 1;
  /** Where the copy came from: real hardware or the demo device. */
  source: "device" | "virtual";
  parameterValues: PresetTextValues;
  /** The preset as published, without its session-bound identity. Blocks may carry `parameterValues`. */
  snapshot: PresetSnapshot;
  /** The demo device's exact preset. Present only on a copy taken from the demo device. */
  demoPreset?: unknown;
}

export interface DemoLibraryTextDocument {
  format: typeof DEMO_LIBRARY_TEXT_FORMAT;
  version: 1;
  library: VirtualDeviceState;
}

export type ParsedText<T> = { ok: true; document: T } | { ok: false; message: string };

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const isIntegerIn = (value: unknown, minimum: number, maximum: number): value is number =>
  Number.isInteger(value) && (value as number) >= minimum && (value as number) <= maximum;
const isNormalized = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
const cell = (block: { row: number; column: number }) => `row ${block.row + 1}, column ${block.column + 1}`;
const Cell = (block: { row: number; column: number }) => `Row ${block.row + 1}, column ${block.column + 1}`;
const plural = (count: number, noun: string) => `${count} ${noun}${count === 1 ? "" : "s"}`;

/** A block that is a real Grid device, not an IN/OUT pseudo-block. The Grid workflow's own test. */
export function isGridDevice(block: GridBlock): boolean {
  return block.column >= 0 && block.modelId !== undefined && block.kind !== "input" && block.kind !== "output";
}

/** Too long to parse: a clear refusal instead of a stalled window or a truncated result. */
function oversized(length: number, limit: number, what: string): string {
  return `The clipboard holds ${length.toLocaleString("en-US")} characters, more than the ${limit.toLocaleString("en-US")} a ${what} can be. Nothing was pasted.`;
}

// ---------------------------------------------------------------------------
// Copy

export interface CapturedPresetText {
  text: string;
  document: PresetTextDocument;
  /** What was copied, worded for the notice line. Honest about any values that are missing. */
  summary: string;
}

export interface CapturePresetOptions {
  /** The active transport. It is only read from: `blockDetails` for blocks that do not carry their values. */
  gateway: Pick<GatewayTransport, "blockDetails">;
  snapshot: PresetSnapshot;
  source: "device" | "virtual";
  /** The demo device's exact preset, when copying from the demo device. */
  demoPreset?: unknown;
  onProgress?: (message: string) => void;
}

/**
 * Serialize the active preset as text. A block that does not already carry its
 * parameter values (every block on real hardware) has them read with one
 * `blockDetails` call each, the read the block editor already makes, so the
 * copy is complete for the active scene. A block whose read fails is copied
 * without values, and the summary says which.
 */
export async function capturePresetText(options: CapturePresetOptions): Promise<CapturedPresetText> {
  const { gateway, snapshot, source, demoPreset, onProgress } = options;
  const identity = requirePresetIdentity(snapshot);
  const devices = snapshot.blocks.filter(isGridDevice) as VirtualGridBlock[];
  const toRead = devices.filter((device) => !device.parameterValues).length;
  const unread: string[] = [];
  const blocks: VirtualGridBlock[] = [];
  let read = 0;
  for (const block of snapshot.blocks as VirtualGridBlock[]) {
    if (!isGridDevice(block) || block.parameterValues) { blocks.push(structuredClone(block)); continue; }
    read += 1;
    onProgress?.(`Reading ${block.name} (${read} of ${toRead})…`);
    try {
      const details = await gateway.blockDetails(block.row, block.column, snapshot.presetName, identity);
      if (details.modelId !== block.modelId) throw new Error(`${Cell(block)} changed while it was being read`);
      const parameterValues: Record<number, number> = {};
      for (const parameter of details.parameters) {
        if (parameter.writable && isNormalized(parameter.normalizedValue)) parameterValues[parameter.index] = parameter.normalizedValue;
      }
      blocks.push({ ...structuredClone(block), parameterValues });
    } catch (error) {
      unread.push(`${Cell(block)} (${block.name}): ${errorText(error)}`);
      blocks.push(structuredClone(block));
    }
  }
  // The identity is bound to this session and this device; it means nothing anywhere else.
  const published: PresetSnapshot = structuredClone(snapshot);
  delete published.presetIdentity;
  delete published.stateVersion;
  // The retained model preset list belongs to this session and this device, like the identity.
  delete published.modelPresetCatalog;
  delete published.tempoPulseEpochMs;
  const parameterValues: PresetTextValues = unread.length === 0 ? "complete" : unread.length === devices.length ? "none" : "partial";
  const document: PresetTextDocument = {
    format: PRESET_TEXT_FORMAT,
    version: 1,
    source,
    parameterValues,
    snapshot: { ...published, blocks },
    ...(demoPreset !== undefined ? { demoPreset: structuredClone(demoPreset) } : {})
  };
  const text = JSON.stringify(document, null, 2);
  if (text.length > PRESET_TEXT_MAX_CHARACTERS) throw new Error(`The preset text would be ${text.length.toLocaleString("en-US")} characters, more than a preset can be. Nothing was copied.`);
  const name = snapshot.presetName || "the unnamed preset";
  const count = plural(devices.length, "device");
  let summary: string;
  if (source === "virtual" && demoPreset !== undefined) {
    summary = `Copied “${name}” as text: ${count} with every scene's parameter values. Pasted into a demo device it arrives whole; pasted onto a Quad Cortex, the active scene's values are written to every parameter.`;
  } else if (parameterValues === "complete") {
    summary = `Copied “${name}” as text: ${count} with the parameter values of scene ${String.fromCharCode(65 + snapshot.activeScene)}. Other scenes' values, and the contents of Neural Captures and IRs, are not included.`;
  } else {
    summary = `Copied “${name}” as text: ${count}, but ${plural(unread.length, "device")} could not be read and ${unread.length === 1 ? "was" : "were"} copied without values (${unread.join("; ")}).`;
  }
  return { text, document, summary };
}

// ---------------------------------------------------------------------------
// Parse

const GRID_KINDS: readonly GridBlock["kind"][] = ["utility", "capture", "amp", "cab", "mod", "delay", "reverb"];

function readBlock(value: unknown, position: number): VirtualGridBlock | undefined {
  const where = `Block ${position + 1} of the pasted preset`;
  if (!isRecord(value)) throw new Error(`${where} is not an object.`);
  // IN/OUT pseudo-blocks sit off the Grid; they carry nothing to paste.
  if (value.kind === "input" || value.kind === "output" || (typeof value.column === "number" && value.column < 0)) return undefined;
  const { row, column, modelId } = value;
  if (!isIntegerIn(row, 0, QC_GRID_ROWS - 1) || !isIntegerIn(column, 0, QC_GRID_COLUMNS - 1)) throw new Error(`${where} is not on the Grid.`);
  if (!isIntegerIn(modelId, 1, 0x7fff_ffff)) throw new Error(`${where} (${cell({ row, column })}) names no model.`);
  const name = typeof value.name === "string" && value.name.trim() ? value.name : `Model ${modelId}`;
  const block: VirtualGridBlock = {
    id: `${row}:${column}`, modelId, name,
    kind: GRID_KINDS.includes(value.kind as GridBlock["kind"]) ? value.kind as GridBlock["kind"] : "utility",
    row, column,
    bypassed: value.bypassed === true
  };
  if (typeof value.category === "string") block.category = value.category;
  if (isIntegerIn(value.footswitch, 0, QC_SCENE_COUNT - 1)) {
    block.footswitch = value.footswitch;
    if (isIntegerIn(value.footswitchOrder, 0, QC_GRID_ROWS * QC_GRID_COLUMNS)) block.footswitchOrder = value.footswitchOrder;
    if (value.footswitchAssignmentType === "primary" || value.footswitchAssignmentType === "secondary") block.footswitchAssignmentType = value.footswitchAssignmentType;
  }
  if (value.parameterValues !== undefined) {
    if (!isRecord(value.parameterValues)) throw new Error(`${where} (${cell(block)}) has parameter values that are not an object.`);
    const values: Record<number, number> = {};
    for (const [key, entry] of Object.entries(value.parameterValues)) {
      if (!/^\d+$/.test(key)) throw new Error(`${where} (${cell(block)}) names ${JSON.stringify(key)}, which is not a parameter index.`);
      if (!isNormalized(entry)) throw new Error(`${where} (${cell(block)}) gives parameter ${key} a value outside 0 through 1.`);
      values[Number(key)] = entry;
    }
    block.parameterValues = values;
  }
  return block;
}

function readRoutes(value: unknown): GridRoute[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((route): GridRoute[] => {
    if (!isRecord(route) || !isIntegerIn(route.row, 0, QC_GRID_ROWS - 1)) return [];
    return [{
      row: route.row,
      input: typeof route.input === "string" ? route.input : "",
      output: typeof route.output === "string" ? route.output : "",
      splitMuted: route.splitMuted === true,
      ...(isIntegerIn(route.inputId, 0, 0xffff) ? { inputId: route.inputId } : {}),
      ...(isIntegerIn(route.outputId, 0, 0xffff) ? { outputId: route.outputId } : {}),
      ...(isIntegerIn(route.splitColumn, 0, QC_GRID_COLUMNS - 1) ? { splitColumn: route.splitColumn } : {}),
      ...(isIntegerIn(route.mixColumn, 0, QC_GRID_COLUMNS - 1) ? { mixColumn: route.mixColumn } : {})
    }];
  });
}

/** Rebuild a snapshot from untrusted input, keeping only what a paste uses. Throws a message naming the problem. */
function readSnapshot(value: unknown): PresetSnapshot {
  if (!isRecord(value)) throw new Error("The pasted text holds no preset.");
  if (typeof value.presetName !== "string") throw new Error("The pasted preset has no name.");
  if (!Array.isArray(value.blocks)) throw new Error("The pasted preset has no block list.");
  if (value.blocks.length > QC_GRID_ROWS * QC_GRID_COLUMNS + 8) throw new Error("The pasted preset lists more blocks than a Grid holds.");
  const blocks = value.blocks.map(readBlock).filter((block): block is VirtualGridBlock => block !== undefined);
  const cells = new Set<string>();
  for (const block of blocks) {
    const key = `${block.row}:${block.column}`;
    if (cells.has(key)) throw new Error(`The pasted preset places two blocks at ${cell(block)}.`);
    cells.add(key);
  }
  const strings = (list: unknown) => Array.isArray(list) && list.length === QC_SCENE_COUNT && list.every((entry) => typeof entry === "string") ? [...list] as string[] : undefined;
  const scenes = strings(value.scenes) ?? Array.from({ length: QC_SCENE_COUNT }, (_, scene) => `Scene ${String.fromCharCode(65 + scene)}`);
  const sceneColors = strings(value.sceneColors);
  const footswitchStates = Array.isArray(value.footswitchStates)
    ? value.footswitchStates.flatMap((state) => isRecord(state) && isIntegerIn(state.index, 0, QC_SCENE_COUNT - 1) ? [{
      index: state.index, active: state.active === true, assigned: state.assigned === true,
      color: typeof state.color === "string" ? state.color : "",
      ...(typeof state.momentary === "boolean" ? { momentary: state.momentary } : {}),
      ...(typeof state.label === "string" ? { label: state.label } : {})
    }] : [])
    : undefined;
  const mode = ["PRESET", "SCENE", "STOMP", "HYBRID"].includes(value.mode as string) ? value.mode as PresetSnapshot["mode"] : "SCENE";
  return {
    deviceName: typeof value.deviceName === "string" ? value.deviceName : "",
    presetName: value.presetName,
    presetLocation: typeof value.presetLocation === "string" ? value.presetLocation : "",
    presetPosition: isIntegerIn(value.presetPosition, 0, 0xffff) ? value.presetPosition : 0,
    setlistKey: typeof value.setlistKey === "string" ? value.setlistKey : "",
    setlistName: typeof value.setlistName === "string" ? value.setlistName : "",
    mode,
    activeScene: isIntegerIn(value.activeScene, 0, QC_SCENE_COUNT - 1) ? value.activeScene : 0,
    scenes,
    ...(sceneColors ? { sceneColors } : {}),
    ...(footswitchStates ? { footswitchStates } : {}),
    blocks,
    routes: readRoutes(value.routes),
    tempo: typeof value.tempo === "number" && Number.isFinite(value.tempo) ? value.tempo : 120,
    masterVolume: isIntegerIn(value.masterVolume, 0, 100) ? value.masterVolume : 0,
    dirty: false
  };
}

function parseJson(text: string, limit: number, what: string): ParsedText<unknown> {
  if (typeof text !== "string" || !text.trim()) return { ok: false, message: "The clipboard is empty. Copy a preset as text first." };
  if (text.length > limit) return { ok: false, message: oversized(text.length, limit, what) };
  try {
    return { ok: true, document: JSON.parse(text) as unknown };
  } catch (error) {
    return { ok: false, message: `The clipboard does not hold a QC Remote ${what}: it is not valid JSON (${errorText(error)}). Nothing was pasted.` };
  }
}

/**
 * Parse clipboard text as a preset. Accepts the document `capturePresetText`
 * writes, or a bare `PresetSnapshot`. Never throws: anything else comes back as
 * a message that says what was wrong.
 */
export function parsePresetText(text: string): ParsedText<PresetTextDocument> {
  const json = parseJson(text, PRESET_TEXT_MAX_CHARACTERS, "preset");
  if (!json.ok) return json;
  const value = json.document;
  try {
    if (isRecord(value) && value.format === DEMO_LIBRARY_TEXT_FORMAT) {
      return { ok: false, message: "The clipboard holds a whole demo library, not one preset. Use Paste demo library in the USB panel's demo device section." };
    }
    if (isRecord(value) && value.format !== undefined) {
      if (value.format !== PRESET_TEXT_FORMAT) return { ok: false, message: "The clipboard holds JSON, but not a QC Remote preset. Nothing was pasted." };
      if (value.version !== 1) return { ok: false, message: `The clipboard holds a version ${String(value.version)} preset, which this QC Remote cannot read. Nothing was pasted.` };
      const snapshot = readSnapshot(value.snapshot);
      const declared = ["complete", "partial", "none"].includes(value.parameterValues as string) ? value.parameterValues as PresetTextValues : undefined;
      return {
        ok: true,
        document: {
          format: PRESET_TEXT_FORMAT, version: 1,
          source: value.source === "virtual" ? "virtual" : "device",
          parameterValues: declared ?? valuesCarried(snapshot),
          snapshot,
          ...(value.demoPreset !== undefined ? { demoPreset: value.demoPreset } : {})
        }
      };
    }
    // A bare snapshot, as a diagnostics report or an AI app might hand one over.
    if (isRecord(value) && Array.isArray(value.blocks) && typeof value.presetName === "string") {
      const snapshot = readSnapshot(value);
      return { ok: true, document: { format: PRESET_TEXT_FORMAT, version: 1, source: "device", parameterValues: valuesCarried(snapshot), snapshot } };
    }
    return { ok: false, message: "The clipboard holds JSON, but not a QC Remote preset. Nothing was pasted." };
  } catch (error) {
    return { ok: false, message: `${errorText(error)} Nothing was pasted.` };
  }
}

function valuesCarried(snapshot: PresetSnapshot): PresetTextValues {
  const devices = snapshot.blocks as VirtualGridBlock[];
  const carrying = devices.filter((block) => block.parameterValues).length;
  return carrying === 0 ? "none" : carrying === devices.length ? "complete" : "partial";
}

// ---------------------------------------------------------------------------
// Paste into the demo device

export interface DemoPasteResult {
  /** "exact" when the demo's own preset was restored; "snapshot" when it was rebuilt from the published snapshot. */
  method: "exact" | "snapshot";
  skipped: VirtualPresetSkippedBlock[];
  summary: string;
}

/**
 * Replace the demo device's active preset with a pasted one and persist it. A
 * copy from the demo device restores its exact preset; any other copy is
 * rebuilt from the snapshot, and blocks the demo cannot simulate are reported.
 */
export async function pastePresetIntoDemo(store: Pick<VirtualDeviceStore, "replaceCurrentPreset" | "presetFromSnapshot">, document: PresetTextDocument): Promise<DemoPasteResult> {
  const name = document.snapshot.presetName || "the unnamed preset";
  if (document.demoPreset !== undefined) {
    await store.replaceCurrentPreset(document.demoPreset);
    return { method: "exact", skipped: [], summary: `Pasted “${name}” into the demo device, with every scene's values. It is unsaved until you save it to a slot.` };
  }
  const { preset, skipped } = store.presetFromSnapshot(document.snapshot);
  await store.replaceCurrentPreset(preset);
  const values = document.parameterValues === "complete"
    ? " Every scene takes the copied scene's values."
    : document.parameterValues === "partial"
      ? " Devices copied without values start from the demo device's starting values."
      : " The copy carried no parameter values, so every device starts from the demo device's starting values.";
  const left = skipped.length
    ? ` Left out ${plural(skipped.length, "device")} the demo cannot simulate: ${skipped.map((block) => `${Cell(block)} (${block.name})`).join(", ")}.`
    : "";
  return { method: "snapshot", skipped, summary: `Pasted “${name}” into the demo device.${values}${left} It is unsaved until you save it to a slot.` };
}

// ---------------------------------------------------------------------------
// Paste onto real hardware

export interface PresetReplayReport {
  presetName: string;
  removed: number;
  removeTotal: number;
  blocksAdded: number;
  blocksTotal: number;
  parametersSet: number;
  parametersTotal: number;
  /** Devices never tried because the connection was lost first. */
  notAttempted: number;
  /** The connection-loss message the replay stopped on, if it stopped early. */
  stoppedBy?: string;
  /** Every write the device refused or that could not be attempted, in order, worded for the user. */
  failures: string[];
  /** The last snapshot a write returned. */
  snapshot?: PresetSnapshot;
}

export interface ReplayPresetOptions {
  gateway: Pick<GatewayTransport, "addBlock" | "removeBlock" | "blockDetails" | "setParameter">;
  document: PresetTextDocument;
  /** The device's active preset, as last observed: its name and identity guard every write. */
  target: PresetSnapshot;
  /** Whether a failure means the connection itself is gone; the replay then stops rather than timing out on every remaining write. */
  isConnectionLost?: (message: string) => boolean;
  onProgress?: (message: string) => void;
}

/**
 * Write a pasted preset onto a connected Quad Cortex using only the writes the
 * app already makes, each with its usual expected-state guards: remove each
 * block on the target preset, then, block by block in Grid order, `addBlock`
 * followed by one `setParameter` per copied value that differs from what the
 * new block reads back. There is no bulk write, so this can stop partway: a
 * refused write is recorded and the replay continues with the next block, so
 * the report says exactly what did and did not land. A lost connection stops
 * it at once, and the devices never tried are counted.
 *
 * Every value the pasted document carries is written, whatever its source: a
 * paste is a full overwrite. A copy from the demo device carries a value for
 * every parameter, including ones never touched there, which hold the demo's
 * starting value (a known factory default where available, approximated where
 * not). The caller discloses that before the user confirms the paste.
 */
export async function replayPresetOnDevice(options: ReplayPresetOptions): Promise<PresetReplayReport> {
  const { gateway, document, target, onProgress, isConnectionLost } = options;
  const identity = requirePresetIdentity(target);
  const scene = target.activeScene;
  let presetName = target.presetName;
  const incoming = (document.snapshot.blocks as VirtualGridBlock[]).filter(isGridDevice)
    .sort((left, right) => left.row - right.row || left.column - right.column);
  const report: PresetReplayReport = {
    presetName: document.snapshot.presetName,
    removed: 0, removeTotal: 0, blocksAdded: 0, blocksTotal: incoming.length, parametersSet: 0, parametersTotal: 0,
    notAttempted: 0, failures: []
  };
  const observe = (result: DeviceActionResult) => {
    if (!result.snapshot) return;
    report.snapshot = result.snapshot;
    presetName = result.snapshot.presetName;
  };
  /** Record a connection loss; the caller stops. */
  const lost = (error: unknown): boolean => {
    const message = errorText(error);
    if (!isConnectionLost?.(message)) return false;
    report.stoppedBy = message;
    return true;
  };

  const existing = target.blocks.filter(isGridDevice);
  report.removeTotal = existing.length;
  for (const block of existing) {
    onProgress?.(`Clearing ${block.name} from ${cell(block)}…`);
    try {
      observe(await gateway.removeBlock(block.row, block.column, block.modelId!, presetName, identity));
      report.removed += 1;
    } catch (error) {
      report.failures.push(`${Cell(block)} (${block.name}) could not be removed: ${errorText(error)}`);
      if (lost(error)) { report.notAttempted = incoming.length; return report; }
    }
  }

  for (const [position, block] of incoming.entries()) {
    const modelId = block.modelId!;
    onProgress?.(`Adding ${block.name} (${position + 1} of ${incoming.length}) at ${cell(block)}…`);
    try {
      observe(await gateway.addBlock(block.row, block.column, modelId, presetName, null, identity));
      report.blocksAdded += 1;
    } catch (error) {
      report.failures.push(`${Cell(block)} (${block.name}) was refused: ${errorText(error)}`);
      if (lost(error)) { report.notAttempted = incoming.length - position - 1; return report; }
      continue;
    }
    const values = Object.entries(block.parameterValues ?? {})
      .map(([index, value]) => ({ index: Number(index), value }))
      .sort((left, right) => left.index - right.index);
    if (!values.length) continue;
    report.parametersTotal += values.length;
    let details;
    try {
      details = await gateway.blockDetails(block.row, block.column, presetName, identity);
    } catch (error) {
      report.failures.push(`${Cell(block)} (${block.name}) was added, but its parameters could not be read, so none of its ${values.length} values were set: ${errorText(error)}`);
      if (lost(error)) { report.notAttempted = incoming.length - position - 1; return report; }
      continue;
    }
    const refused: string[] = [];
    let stopped = false;
    for (const { index, value } of values) {
      const parameter = details.parameters.find((candidate) => candidate.index === index);
      if (!parameter || !parameter.writable || parameter.normalizedValue === null) {
        refused.push(`parameter ${index} is not writable on this block`);
        continue;
      }
      if (Math.abs(parameter.normalizedValue - value) <= 0.000_01) { report.parametersSet += 1; continue; }
      try {
        observe(await gateway.setParameter(block.row, block.column, index, value, parameter.normalizedValue, scene, modelId, presetName, identity));
        report.parametersSet += 1;
      } catch (error) {
        refused.push(`${parameter.name}: ${errorText(error)}`);
        if (lost(error)) { stopped = true; break; }
      }
    }
    if (refused.length) {
      report.failures.push(`${Cell(block)} (${block.name}): ${refused.length} of ${values.length} parameter values were not set (${refused.slice(0, 3).join("; ")}${refused.length > 3 ? `; and ${refused.length - 3} more` : ""})`);
    }
    if (stopped) { report.notAttempted = incoming.length - position - 1; return report; }
  }
  return report;
}

/**
 * The confirmation to show before {@link replayPresetOnDevice} writes a pasted
 * copy onto a connected Quad Cortex, or `undefined` when nothing needs
 * confirming (an empty, unmodified target and a copy from real hardware).
 *
 * A copy from the demo device always asks: every parameter it carries is
 * written, including ones never changed on the demo, whose values are the
 * demo's starting values rather than necessarily this unit's own defaults.
 */
export function presetReplayConfirmation(document: PresetTextDocument, target: PresetSnapshot): string | undefined {
  const occupied = target.blocks.filter(isGridDevice).length;
  const fromDemo = document.source === "virtual";
  if (!occupied && !target.dirty && !fromDemo) return undefined;
  const pastedName = document.snapshot.presetName || "the unnamed preset";
  const incoming = plural(document.snapshot.blocks.filter(isGridDevice).length, "device");
  const clears = occupied || target.dirty
    ? `This removes the ${plural(occupied, "device")} on it${target.dirty ? " along with its unsaved changes" : ""}, then adds ${incoming} one at a time.`
    : `This adds ${incoming} one at a time.`;
  const demo = fromDemo
    ? " This preset was built on the demo device. Any parameter never changed there carries an approximated value (the known factory default where one exists, a guess where not), not necessarily this unit's own default. Confirming replaces every parameter on the target with these values."
    : "";
  return `Paste “${pastedName}” onto ${target.presetLocation} · ${target.presetName || "the unnamed preset"}? ${clears}${demo} The QC may refuse a device if the preset runs out of DSP. This is temporary until the preset is saved.`;
}

/** The notice line for a replay: what landed, what was refused, and what the replay never touches. */
export function presetReplaySummary(report: PresetReplayReport, document: PresetTextDocument): string {
  const name = report.presetName || "the unnamed preset";
  const parts = [`Pasted “${name}” onto the Quad Cortex: added ${report.blocksAdded} of ${plural(report.blocksTotal, "device")}`];
  if (report.parametersTotal) parts[0] += ` and set ${report.parametersSet} of ${plural(report.parametersTotal, "parameter value")}`;
  parts[0] += ".";
  if (report.stoppedBy) {
    parts.push(`The connection was lost (${report.stoppedBy}), so the paste stopped${report.notAttempted ? `: ${plural(report.notAttempted, "device")} ${report.notAttempted === 1 ? "was" : "were"} not attempted` : ""}.`);
  }
  if (report.removeTotal) parts.push(`Cleared ${report.removed} of ${plural(report.removeTotal, "device")} that were on the preset.`);
  if (document.parameterValues === "none") parts.push("The copy carried no parameter values, so each device starts from the Quad Cortex's defaults.");
  if (report.failures.length) {
    const shown = report.failures.slice(0, 3);
    parts.push(`${shown.join(". ")}${report.failures.length > shown.length ? `. And ${report.failures.length - shown.length} more refused.` : "."}`);
  }
  parts.push("Bypass states, footswitch assignments, scene names, routing and tempo were not changed. Nothing is stored until you save the preset.");
  return parts.join(" ");
}

// ---------------------------------------------------------------------------
// Complete setlist capture for a physical-QC -> demo transfer

export interface CapturedSetlistScene {
  snapshot: PresetSnapshot;
  blocks: Record<string, BlockDetails>;
  lanes: Partial<Record<number, Partial<Record<LaneControl, BlockDetails>>>>;
}

export interface CapturedSetlistPreset {
  position: number;
  name: string;
  activeScene: number;
  scenes: CapturedSetlistScene[];
}

export interface CapturedSetlist {
  sourceSerial: string;
  setlistKey: string;
  setlistName: string;
  presets: CapturedSetlistPreset[];
}

const blockAddress = (row: number, column: number) => `${row}:${column}`;
const laneControls: readonly LaneControl[] = ["inputGate", "laneOutput"];

async function observedSnapshot(gateway: GatewayTransport, result: DeviceActionResult): Promise<PresetSnapshot> {
  return result.snapshot ?? gateway.currentSnapshot();
}

async function captureSetlistScene(gateway: GatewayTransport, snapshot: PresetSnapshot): Promise<CapturedSetlistScene> {
  const identity = requirePresetIdentity(snapshot);
  const blocks: Record<string, BlockDetails> = {};
  for (const block of snapshot.blocks.filter(isGridDevice)) {
    blocks[blockAddress(block.row, block.column)] = await gateway.blockDetails(block.row, block.column, snapshot.presetName, identity);
  }
  const lanes: CapturedSetlistScene["lanes"] = {};
  for (let row = 0; row < QC_GRID_ROWS; row += 1) {
    const controls: Partial<Record<LaneControl, BlockDetails>> = {};
    for (const control of laneControls) controls[control] = await gateway.laneControlDetails(row, control, snapshot.presetName, identity);
    lanes[row] = controls;
  }
  return { snapshot: structuredClone(snapshot), blocks, lanes };
}

/**
 * Read every occupied slot and every scene from one setlist. Reads are guarded
 * by the current preset identity, and the originally active preset/scene is
 * restored even when a later read fails. No source preset is written.
 */
export async function captureSetlistForDemo(options: {
  gateway: GatewayTransport;
  setlistKey: string;
  setlistName: string;
  onProgress?: (message: string) => void;
}): Promise<CapturedSetlist> {
  const { gateway, setlistKey, setlistName, onProgress } = options;
  const identity = await gateway.identity();
  const original = await gateway.currentSnapshot();
  if (original.dirty) throw new Error("The active Quad Cortex preset has unsaved changes. Save or discard them before copying a setlist.");
  const listing = await gateway.listPresets(false, setlistKey);
  if (listing.setlistName !== setlistName) throw new Error(`The source setlist changed from ${JSON.stringify(setlistName)} to ${JSON.stringify(listing.setlistName)}. Refresh and retry.`);
  // Current gateways expose `occupied`; older ones represented an empty slot as
  // the synthetic name "Unsaved", so retain that fallback while clients roll
  // forward. A user preset with that literal name is still unambiguous on a new
  // gateway because its explicit flag wins.
  const entries = listing.presets
    .filter((entry) => entry.occupied ?? entry.name !== "Unsaved")
    .sort((left, right) => left.position - right.position);
  const presets: CapturedSetlistPreset[] = [];
  let current = original;
  let failure: unknown;
  try {
    for (const [presetIndex, entry] of entries.entries()) {
      onProgress?.(`Reading ${setlistName}: ${entry.location} · ${entry.name} (${presetIndex + 1} of ${entries.length})…`);
      if (current.setlistKey.replace(/\/+$/, "") !== setlistKey.replace(/\/+$/, "") || current.presetPosition !== entry.position) {
        current = await observedSnapshot(gateway, await gateway.recallPreset(
          setlistKey, entry.position, current.presetName, current.presetPosition,
          current.setlistKey, requirePresetIdentity(current)
        ));
      }
      const activeScene = current.activeScene;
      const scenes: CapturedSetlistScene[] = [];
      for (let scene = 0; scene < QC_SCENE_COUNT; scene += 1) {
        if (current.activeScene !== scene) current = await observedSnapshot(gateway, await gateway.selectScene(scene, current.presetName, requirePresetIdentity(current)));
        scenes.push(await captureSetlistScene(gateway, current));
      }
      presets.push({ position: entry.position, name: entry.name, activeScene, scenes });
    }
  } catch (error) {
    failure = error;
  }
  try {
    if (current.setlistKey.replace(/\/+$/, "") !== original.setlistKey.replace(/\/+$/, "") || current.presetPosition !== original.presetPosition) {
      current = await observedSnapshot(gateway, await gateway.recallPreset(
        original.setlistKey, original.presetPosition, current.presetName, current.presetPosition,
        current.setlistKey, requirePresetIdentity(current)
      ));
    }
    if (current.activeScene !== original.activeScene) await gateway.selectScene(original.activeScene, current.presetName, requirePresetIdentity(current));
  } catch (restoreError) {
    throw new Error(`${failure ? `${errorText(failure)} ` : ""}The original preset could not be restored after setlist capture: ${errorText(restoreError)}`);
  }
  if (failure) throw failure;
  return { sourceSerial: identity.serial, setlistKey, setlistName, presets };
}

function capturedParameterState(scenes: CapturedSetlistScene[], details: (scene: CapturedSetlistScene) => BlockDetails | undefined): VirtualParameterState | undefined {
  const first = details(scenes[0]);
  if (!first) return undefined;
  const parameterValues: Record<number, number> = {};
  const sceneValues: Record<number, number[]> = {};
  const expressions: NonNullable<VirtualParameterState["expressions"]> = {};
  for (const parameter of first.parameters) {
    if (parameter.normalizedValue === null) continue;
    if (parameter.sceneMode) {
      const values = scenes.map((scene) => details(scene)?.parameters.find((candidate) => candidate.index === parameter.index)?.normalizedValue);
      if (values.every((value): value is number => typeof value === "number" && Number.isFinite(value))) sceneValues[parameter.index] = values;
      else parameterValues[parameter.index] = parameter.normalizedValue;
    } else {
      parameterValues[parameter.index] = parameter.normalizedValue;
    }
    if ((parameter.expression === 1 || parameter.expression === 2) && typeof parameter.expressionMinimum === "number" && typeof parameter.expressionMaximum === "number") {
      expressions[parameter.index] = { pedal: parameter.expression, minimum: parameter.expressionMinimum, maximum: parameter.expressionMaximum };
    }
  }
  return {
    parameterValues,
    ...(Object.keys(sceneValues).length ? { sceneValues } : {}),
    ...(Object.keys(expressions).length ? { expressions } : {})
  };
}

/** Convert a complete multi-scene capture into the Demo device's native preset document. */
export function capturedPresetForDemo(store: VirtualDeviceStore, captured: CapturedSetlistPreset): { preset: VirtualPreset; skipped: VirtualPresetSkippedBlock[] } {
  if (captured.scenes.length !== QC_SCENE_COUNT) throw new Error(`${captured.name} does not contain all ${QC_SCENE_COUNT} scenes.`);
  const { preset, skipped } = store.presetFromSnapshot(captured.scenes[0].snapshot);
  preset.activeScene = captured.activeScene;
  for (const block of preset.blocks) {
    const address = blockAddress(block.row, block.column);
    const state = capturedParameterState(captured.scenes, (scene) => scene.blocks[address]);
    if (state) Object.assign(block, state);
    block.sceneBypassed = captured.scenes.map((scene) => scene.snapshot.blocks.find((candidate) => candidate.row === block.row && candidate.column === block.column)?.bypassed === true);
    const first = captured.scenes[0].blocks[address];
    if (first?.bypassExpression) block.bypassExpression = structuredClone(first.bypassExpression);
  }
  for (const route of preset.routes) {
    route.splitMuted = captured.scenes.map((scene) => scene.snapshot.routes.find((candidate) => candidate.row === route.row)?.splitMuted === true);
  }
  preset.lanes = {};
  for (let row = 0; row < QC_GRID_ROWS; row += 1) {
    const controls: Partial<Record<LaneControl, VirtualParameterState>> = {};
    for (const control of laneControls) {
      const state = capturedParameterState(captured.scenes, (scene) => scene.lanes[row]?.[control]);
      if (state) controls[control] = state;
    }
    if (Object.keys(controls).length) preset.lanes[row] = controls;
  }
  return { preset, skipped };
}

/** Persist a captured physical setlist into the Demo library at identical slots. */
export async function importCapturedSetlistIntoDemo(store: VirtualDeviceStore, captured: CapturedSetlist): Promise<{ name: string; presets: number; skipped: VirtualPresetSkippedBlock[] }> {
  const presets: Record<number, VirtualPreset> = {};
  const skipped: VirtualPresetSkippedBlock[] = [];
  for (const entry of captured.presets) {
    const converted = capturedPresetForDemo(store, entry);
    presets[entry.position] = converted.preset;
    skipped.push(...converted.skipped);
  }
  const imported = await store.importSetlist(captured.setlistName, presets);
  return { name: imported.name, presets: imported.presets, skipped };
}

// ---------------------------------------------------------------------------
// The demo library

/**
 * Serialize the user's Demo library. Built-in factory examples are supplied by
 * the app version that opens the document, so copying them wastes space and
 * could replace newer examples with an older bundle.
 */
export function serializeDemoLibrary(library: VirtualDeviceState): string {
  const exported = structuredClone(library);
  exported.setlists = exported.setlists.filter((setlist) => !setlist.isFactory);
  if (!exported.setlists.some((setlist) => setlist.key === exported.current.setlistKey)) {
    const setlist = exported.setlists[0];
    if (!setlist) throw new Error("The demo library has no user setlist to copy.");
    const position = Object.keys(setlist.presets).map(Number).sort((a, b) => a - b)[0] ?? 0;
    exported.current = {
      setlistKey: setlist.key,
      position,
      dirty: false,
      preset: structuredClone(setlist.presets[position] ?? { ...exported.current.preset, name: "" })
    };
  }
  const document: DemoLibraryTextDocument = { format: DEMO_LIBRARY_TEXT_FORMAT, version: 1, library: exported };
  const text = JSON.stringify(document);
  if (text.length > DEMO_LIBRARY_TEXT_MAX_CHARACTERS) throw new Error(`The demo library would be ${text.length.toLocaleString("en-US")} characters, more than can be copied as text. Nothing was copied.`);
  return text;
}

/** Counts for a confirmation or a notice: presets stored across all setlists. */
export function demoLibraryCounts(library: Pick<VirtualDeviceState, "setlists">): { setlists: number; presets: number } {
  const userSetlists = library.setlists.filter((setlist) => !setlist.isFactory);
  return {
    setlists: userSetlists.length,
    presets: userSetlists.reduce((total, setlist) => total + Object.keys(setlist.presets ?? {}).length, 0)
  };
}

/**
 * Parse clipboard text as a demo library. Only the envelope is checked here;
 * `VirtualDeviceStore.replaceLibrary` checks every preset before it writes.
 */
export function parseDemoLibraryText(text: string): ParsedText<DemoLibraryTextDocument> {
  const json = parseJson(text, DEMO_LIBRARY_TEXT_MAX_CHARACTERS, "demo library");
  if (!json.ok) return json;
  const value = json.document;
  if (!isRecord(value) || value.format !== DEMO_LIBRARY_TEXT_FORMAT) {
    return {
      ok: false,
      message: isRecord(value) && value.format === PRESET_TEXT_FORMAT
        ? "The clipboard holds one preset, not a demo library. Use Edit > Paste Preset from Text."
        : "The clipboard does not hold a QC Remote demo library. Nothing was pasted."
    };
  }
  if (value.version !== 1) return { ok: false, message: `The clipboard holds a version ${String(value.version)} demo library, which this QC Remote cannot read. Nothing was pasted.` };
  if (!isRecord(value.library) || !Array.isArray(value.library.setlists)) return { ok: false, message: "The pasted demo library holds no setlists. Nothing was pasted." };
  if (!value.library.setlists.every((setlist) => isRecord(setlist) && isRecord(setlist.presets))) {
    return { ok: false, message: "The pasted demo library has a setlist with no presets object. Nothing was pasted." };
  }
  return { ok: true, document: { format: DEMO_LIBRARY_TEXT_FORMAT, version: 1, library: value.library as unknown as VirtualDeviceState } };
}
