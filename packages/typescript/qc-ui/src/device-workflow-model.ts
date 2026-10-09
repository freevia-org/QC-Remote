/**
 * The Looper X actions in the order the unit's own Looper X screen lists them on
 * footswitches A to H (observed on CorOS 4.1.0). This is a screen order for the
 * command buttons, not the numbering of the raw stomp-assignment indices; use
 * {@link LOOPER_X_ASSIGNMENT_INDEX_NAMES} for those.
 */
export const LOOPER_X_ACTIONS = [
  ["duplicate", "DUPLICATE"], ["oneShot", "ONE SHOT"], ["halfSpeed", "HALF SPEED"],
  ["punch", "PUNCH IN"], ["record", "RECORD"], ["play", "PLAY"],
  ["reverse", "REVERSE"], ["undoRedo", "UNDO / REDO"]
] as const;

/**
 * Names for the raw Looper X stomp-assignment indices, indexed by the raw value
 * the unit stores per footswitch. Observed on a CorOS 4.1.0 unit, 2026-10-06:
 * after writing [0, 1, 2, 3, 4, 5, 6, 7] the unit's Looper X editor showed
 * footswitches A to H as Record, Play, Undo, Duplicate, One Shot, Half Speed,
 * Punch In and Reverse. The arrangement the unit reports ([3, 4, 5, 6, 0, 1, 7, 2]
 * on A to H) is therefore the screen order in {@link LOOPER_X_ACTIONS}.
 */
export const LOOPER_X_ASSIGNMENT_INDEX_NAMES = [
  "RECORD", "PLAY", "UNDO", "DUPLICATE", "ONE SHOT", "HALF SPEED", "PUNCH IN", "REVERSE"
] as const;

export const QC_MODE_CYCLES = ["PRESET", "SCENE", "STOMP", "PRESET / SCENE", "PRESET / STOMP", "SCENE / PRESET", "SCENE / STOMP", "STOMP / PRESET", "STOMP / SCENE"] as const;

const MODE_HYBRIDS: Readonly<Record<string, number>> = {
  "0,1": 3, "0,2": 4, "1,0": 5, "1,2": 6, "2,0": 7, "2,1": 8
};
const MODE_HYBRID_PARTS: Readonly<Record<number, readonly [number, number]>> = {
  3: [0, 1], 4: [0, 2], 5: [1, 0], 6: [1, 2], 7: [2, 0], 8: [2, 1]
};

/** Merge two distinct base-mode entries, preserving their dragged order. */
export function mergeModeSlots(slots: readonly number[], sourceIndex: number, targetIndex: number): number[] | undefined {
  if (sourceIndex === targetIndex || sourceIndex < 0 || targetIndex < 0 || sourceIndex >= slots.length || targetIndex >= slots.length) return undefined;
  const hybrid = MODE_HYBRIDS[`${slots[sourceIndex]},${slots[targetIndex]}`];
  if (hybrid === undefined) return undefined;
  const next = [...slots];
  next.splice(Math.max(sourceIndex, targetIndex), 1);
  next.splice(Math.min(sourceIndex, targetIndex), 1, hybrid);
  return next;
}

/** Break a hybrid into its ordered base modes when the three-slot limit allows it. */
export function splitModeSlot(slots: readonly number[], index: number): number[] | undefined {
  if (slots.length >= 3 || index < 0 || index >= slots.length) return undefined;
  const parts = MODE_HYBRID_PARTS[slots[index]];
  if (!parts) return undefined;
  const next = [...slots];
  next.splice(index, 1, ...parts);
  return next;
}
