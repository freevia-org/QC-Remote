import { QC_SCENE_COUNT, qcCategoryKey, type GridBlock, type PresetSnapshot } from "@qc-remote/client";
import { QC_CATEGORY_COLORS, QC_HARDWARE_COLORS, QC_PRESET_SLOT_COLORS, QC_SCENE_COLORS } from "./device-reference/generated-device-colors.ts";
import { sceneLetter } from "./state.ts";

export interface FootswitchLed {
  active: boolean;
  assigned: boolean;
  color: string;
}

/**
 * Which of the two mode entries governs footswitch `index`: the first for A-D, the second
 * for E-H. BELIEVED, not verified on a unit: the library assumes a hybrid mode splits the
 * footswitches at four, and no read of the device has shown it. Every caller that needs the
 * split asks here, so the assumption lives in one place.
 */
export function hybridModeIndex(index: number): 0 | 1 {
  return index < 4 ? 0 : 1;
}

export type FootswitchIntent =
  | { kind: "select-scene"; scene: number }
  | { kind: "select-preset"; position: number }
  | { kind: "toggle-stomp"; index: number }
  | { kind: "none" };

/**
 * The lamp colour for a STOMP assignment, derived from the block's category.
 *
 * The classification is `qcCategoryKey`, generated from
 * `contracts/public/qc-domain.v1.json` so that this file, the app's Grid ink and the
 * gateway's `_block_color` cannot drift apart about which category a block is.
 * Only the palette differs by surface: the lamp is the 24-bit colour CorOS
 * stores (`QC_CATEGORY_COLORS`), the screen is its RGB565 rendering
 * (`QC_COLORS.browserCategory`).
 *
 * The gateway publishes the same value as `footswitchStates[n].color`, which
 * `stompLed` below prefers. So a snapshot's lamp colour is this table either
 * way - the unit never reports one. The only colour a preset carries is
 * `scene_colors`; the only lamp field in the protocol is a factory-test write.
 * `footswitchStates[n].color` is not read from the device; it mirrors this
 * table, round-tripped.
 *
 * Drive categories used to be substituted to `category.pitch` (yellow) here.
 * They are not: a drive block on a footswitch lights the same orange the
 * unit's own screen paints its Gig View tile and its browser category row
 * with.
 *
 * Every category whose 24-bit colour is `category.utility` grey - Neural
 * Capture, Wah, FX Loop, Utility itself, and anything unclassified - lights the
 * white lamp instead; the unit has no grey emitter.
 */
function stompLedColor(blocks: GridBlock[]): string {
  if (blocks.length > 1) return QC_HARDWARE_COLORS.whiteLed;
  const block = blocks[0];
  const color = QC_CATEGORY_COLORS[qcCategoryKey(`${block.category ?? ""} ${block.kind ?? ""}`, block.name)];
  return color === QC_CATEGORY_COLORS.utility ? QC_HARDWARE_COLORS.whiteLed : color;
}

/**
 * A footswitch the library cannot say anything about: the mode is not known, so neither is what
 * the switch does or how its lamp shows. Unlit and unassigned, in the idle colour.
 */
function unknownModeLed(): FootswitchLed {
  return { active: false, assigned: false, color: QC_HARDWARE_COLORS.idleLed };
}

function stompLed(snapshot: PresetSnapshot, index: number): FootswitchLed {
  const reported = snapshot.footswitchStates?.find((state) => state.index === index);
  if (reported) return { active: reported.active, assigned: reported.assigned, color: reported.color };
  const assigned = snapshot.blocks
    .filter((block) => block.footswitch === index)
    .sort((left, right) => (left.footswitchOrder ?? Number.MAX_SAFE_INTEGER) - (right.footswitchOrder ?? Number.MAX_SAFE_INTEGER));
  if (!assigned.length) return { active: false, assigned: false, color: QC_HARDWARE_COLORS.idleLed };
  return { active: !assigned[0].bypassed, assigned: true, color: stompLedColor(assigned) };
}

function sceneLed(snapshot: PresetSnapshot, index: number): FootswitchLed {
  return { active: snapshot.activeScene === index, assigned: true, color: snapshot.sceneColors?.[index] ?? QC_SCENE_COLORS[index] };
}

/**
 * In PRESET mode the QC lights each of the eight slots in its own fixed
 * colour, the same eight colours for slots A through H in every bank
 * regardless of which presets fill them. The active slot is the one the
 * device highlights.
 *
 * This used to paint all eight lamps with the loaded preset's active scene
 * colour, which belongs to SCENE mode and matched the device only by accident.
 */
function presetLed(snapshot: PresetSnapshot, index: number): FootswitchLed {
  return {
    active: snapshot.presetPosition % 8 === index,
    assigned: true,
    color: QC_PRESET_SLOT_COLORS[index] ?? QC_SCENE_COLORS[0]
  };
}

export function footswitchLeds(snapshot: PresetSnapshot): FootswitchLed[] {
  const fallbackMode = snapshot.mode === "HYBRID" ? "SCENE" : snapshot.mode;
  const rowModes = snapshot.footswitchModes ?? [fallbackMode, fallbackMode];
  return Array.from({ length: QC_SCENE_COUNT }, (_, index) => {
    const mode = rowModes[hybridModeIndex(index)];
    if (mode === "STOMP") return stompLed(snapshot, index);
    if (mode === "PRESET") return presetLed(snapshot, index);
    if (mode === "SCENE") return sceneLed(snapshot, index);
    return unknownModeLed();
  });
}

export function footswitchIntent(snapshot: PresetSnapshot, index: number): FootswitchIntent {
  if (!Number.isInteger(index) || index < 0 || index >= QC_SCENE_COUNT) return { kind: "none" };
  const fallbackMode = snapshot.mode === "HYBRID" ? "SCENE" : snapshot.mode;
  const mode = snapshot.footswitchModes?.[hybridModeIndex(index)] ?? fallbackMode;
  if (mode === "SCENE") return { kind: "select-scene", scene: index };
  if (mode === "PRESET") return { kind: "select-preset", position: Math.floor(snapshot.presetPosition / 8) * 8 + index };
  // An unknown mode is not STOMP: nothing is pressed on a guess.
  if (mode !== "STOMP") return { kind: "none" };
  return snapshot.blocks.some((block) => block.footswitch === index)
    ? { kind: "toggle-stomp", index }
    : { kind: "none" };
}

export function applyFootswitchPreview(snapshot: PresetSnapshot, index: number): PresetSnapshot {
  const intent = footswitchIntent(snapshot, index);
  if (intent.kind === "select-scene") return { ...snapshot, activeScene: intent.scene };
  if (intent.kind === "select-preset") {
    const presetPosition = intent.position;
    return {
      ...snapshot,
      presetPosition,
      presetLocation: `${Math.floor(presetPosition / 8) + 1}${sceneLetter(presetPosition % 8)}`
    };
  }
  if (intent.kind === "toggle-stomp") return optimisticallyPressFootswitch(snapshot, intent.index);
  return snapshot;
}

export function optimisticallyPressFootswitch(snapshot: PresetSnapshot, index: number): PresetSnapshot {
  const fallbackMode = snapshot.mode === "HYBRID" ? "SCENE" : snapshot.mode;
  const rowModes = snapshot.footswitchModes ?? [fallbackMode, fallbackMode];
  if (rowModes[hybridModeIndex(index)] !== "STOMP") return snapshot;
  const assignedIds = new Set(snapshot.blocks.filter((block) => block.footswitch === index).map((block) => block.id));
  if (!assignedIds.size) return snapshot;
  return {
    ...snapshot,
    blocks: snapshot.blocks.map((block) => assignedIds.has(block.id) ? { ...block, bypassed: !block.bypassed } : block),
    footswitchStates: snapshot.footswitchStates?.map((state) => state.index === index ? { ...state, active: !state.active } : state)
  };
}
