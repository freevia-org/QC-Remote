import type { QcSurfaceAction } from "@qc-remote/core";

/*
 * Pure decisions behind the Grid block pointer gestures, kept free of React
 * and the DOM so they can be tested without a browser. The surface component
 * owns the pointer plumbing; everything it decides is decided here.
 *
 *   press, release in place ............ a tap: selects once GRID_DOUBLE_TAP_MS
 *                                        passes without a second tap
 *   two taps on one block inside it .... toggle bypass, and never select
 *   press and hold GRID_LONG_PRESS_MS .. lift the block into move mode (the
 *                                        unit's own behaviour); it then follows
 *                                        the pointer like a drag and drops by
 *                                        the rules below
 *   press, move > threshold, release ... move to the empty cell underneath,
 *                                        remove when released off the Grid,
 *                                        nothing when released on its own
 *                                        cell or on an occupied one
 */

/** Movement, in CSS pixels, before a press becomes a drag. */
export const GRID_DRAG_THRESHOLD_PX = 6;
/** How long a still press is held before the block lifts into move mode. */
export const GRID_LONG_PRESS_MS = 550;
/**
 * How long a tap waits for a second tap on the same block before it selects.
 * 250 ms: a deliberate double tap lands its second tap well inside it (the
 * platforms allow more - Android's double-tap timeout is 300 ms, Windows'
 * double-click time 500 ms - but only to catch slow double clicks), while a
 * single tap's added wait stays at the edge of what reads as instant. It is
 * also well clear of the 550 ms long press, so the two can never overlap.
 */
export const GRID_DOUBLE_TAP_MS = 250;
/** A click this soon after a pointer release is that release's own click, which the tap machine already owns. */
export const GRID_POINTER_CLICK_ECHO_MS = 400;

export interface GridCellLayout {
  /** Cell centres along x, in the Grid drawing's own units. */
  columns: readonly number[];
  /** Cell centres along y, in the same units. */
  rows: readonly number[];
}

export interface GridCellHit {
  row: number;
  column: number;
  /** False once the point leaves the Grid's drawn cells: dropping there removes the block. */
  inBounds: boolean;
}

export interface GridPoint { x: number; y: number }

function nearestIndex(centres: readonly number[], value: number) {
  let best = 0;
  for (let index = 1; index < centres.length; index += 1) {
    if (Math.abs(centres[index] - value) < Math.abs(centres[best] - value)) best = index;
  }
  return best;
}

/** The drawn extent of a run of cell centres: half a pitch beyond the first and last centre. */
function extent(centres: readonly number[]): [number, number] {
  if (centres.length < 2) throw new Error("A Grid axis needs at least two cells.");
  const last = centres.length - 1;
  return [centres[0] - (centres[1] - centres[0]) / 2, centres[last] + (centres[last] - centres[last - 1]) / 2];
}

/** The cell nearest to a point in Grid drawing units, and whether the point is on the Grid at all. */
export function resolveGridCellFromPoint(x: number, y: number, layout: GridCellLayout): GridCellHit {
  const [left, right] = extent(layout.columns);
  const [top, bottom] = extent(layout.rows);
  return {
    row: nearestIndex(layout.rows, y),
    column: nearestIndex(layout.columns, x),
    inBounds: x >= left && x <= right && y >= top && y <= bottom
  };
}

/** Map a client-space pointer position into the Grid's drawing units via the element that spans the drawing. */
export function clientPointToGrid(clientX: number, clientY: number, rect: { left: number; top: number; width: number; height: number }, drawing: { width: number; height: number }): GridPoint {
  return {
    x: (clientX - rect.left) * drawing.width / rect.width,
    y: (clientY - rect.top) * drawing.height / rect.height
  };
}

export function exceedsDragThreshold(dx: number, dy: number, threshold = GRID_DRAG_THRESHOLD_PX) {
  return Math.hypot(dx, dy) > threshold;
}

export type GridDropTarget = "move" | "remove" | "none";

/** What releasing a dragged block over `over` would do. */
export function gridDropTarget(over: GridCellHit, start: { row: number; column: number }, occupied: (row: number, column: number) => boolean): GridDropTarget {
  if (!over.inBounds) return "remove";
  if (over.row === start.row && over.column === start.column) return "none";
  return occupied(over.row, over.column) ? "none" : "move";
}

export interface GridGestureRelease {
  blockId: string;
  phase: "pressed" | "dragging";
  longPressFired: boolean;
  startRow: number;
  startColumn: number;
  /** Undefined when the release position could not be mapped onto the Grid. */
  over?: GridCellHit;
  occupied(row: number, column: number): boolean;
}

export interface GridGestureOutcome {
  action?: QcSurfaceAction;
  /** The gesture used the pointer, so the click the browser sends after it must not also select. */
  consumed: boolean;
}

export function resolveGridGestureRelease(release: GridGestureRelease): GridGestureOutcome {
  if (release.phase === "pressed") return { consumed: release.longPressFired };
  if (!release.over) return { consumed: true };
  const start = { row: release.startRow, column: release.startColumn };
  const target = gridDropTarget(release.over, start, release.occupied);
  if (target === "remove") return { action: { kind: "remove-block-by-drag", blockId: release.blockId }, consumed: true };
  if (target === "none") return { consumed: true };
  return {
    action: { kind: "move-block-to", blockId: release.blockId, fromRow: start.row, fromColumn: start.column, toRow: release.over.row, toColumn: release.over.column },
    consumed: true
  };
}

/**
 * Counts taps itself rather than trusting native click/dblclick: with those,
 * a double click's first click selected - opening or closing the parameter
 * editor - before the second toggled bypass. A tap here only becomes a select
 * once the double-tap window closes without a second tap on the same block.
 *
 * The host dispatches whatever `tap` and `expire` return, and calls `expire`
 * when the timer it schedules for `pendingUntil()` fires.
 */
export interface GridTapMachine {
  /** A plain tap on `blockId` at `now`; returns the actions due immediately. */
  tap(blockId: string, now: number): QcSurfaceAction[];
  /** The double-tap window of the waiting tap has closed; returns its select. */
  expire(): QcSurfaceAction[];
  /** When the waiting tap's window closes, or undefined when nothing waits. */
  pendingUntil(): number | undefined;
  /** Forget a waiting tap without acting on it. */
  cancel(): void;
}

export function createGridTapMachine(windowMs = GRID_DOUBLE_TAP_MS): GridTapMachine {
  let pending: { blockId: string; at: number } | undefined;
  const select = (blockId: string): QcSurfaceAction => ({ kind: "select-block", blockId });
  return {
    tap(blockId, now) {
      const previous = pending;
      if (previous?.blockId === blockId && now - previous.at < windowMs) {
        pending = undefined;
        return [{ kind: "toggle-bypass", blockId }];
      }
      // A waiting tap on another block, or one whose window closed before its
      // timer ran, is a select in its own right and goes first.
      pending = { blockId, at: now };
      return previous ? [select(previous.blockId)] : [];
    },
    expire() {
      const previous = pending;
      pending = undefined;
      return previous ? [select(previous.blockId)] : [];
    },
    pendingUntil: () => pending && pending.at + windowMs,
    cancel() { pending = undefined; }
  };
}

/**
 * Whether a click on a block is only the echo of a pointer release the gesture
 * machine already handled. A click with no release just before it came from
 * the keyboard or from assistive technology, and selects at once.
 */
export function gridClickIsPointerEcho(now: number, lastPointerReleaseAt: number) {
  return now - lastPointerReleaseAt < GRID_POINTER_CLICK_ECHO_MS;
}
