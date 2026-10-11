import { useCallback, useState } from "react";
import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import type { QcStateUpdate } from "@qc-remote/core";

export interface QcDeviceUndoOptions {
  /** The app's own verified one-step history (useDeviceHistory). */
  history: { undoEntry?: { label: string }; redoEntry?: { label: string }; undo(): Promise<unknown>; redo(): Promise<unknown> };
  snapshot: Pick<PresetSnapshot, "presetIdentity">;
  gateway: Pick<GatewayTransport, "undo" | "redo">;
  fail(error: unknown): void;
}

export interface QcDeviceUndo {
  /** The unit reports something to undo or redo that the app did not record. */
  undoAvailable: boolean;
  redoAvailable: boolean;
  /** Feed every native state batch; only history updates are used. */
  consume(states: readonly QcStateUpdate[]): void;
  /** Undo/redo: the app's own step if it has one, otherwise the unit's. A failure is reported, never left unhandled. */
  undo(): Promise<void>;
  redo(): Promise<void>;
  canUndo: boolean;
  undoLabel?: string;
}

/**
 * The one undo/redo path behind the display's undo control, the menus and the
 * shortcuts: availability reported by the unit, the app's own history first,
 * the unit's history otherwise, and failures reported through the host.
 */
export function useQcDeviceUndo({ history, snapshot, gateway, fail }: QcDeviceUndoOptions): QcDeviceUndo {
  const [undoAvailable, setUndoAvailable] = useState(false);
  const [redoAvailable, setRedoAvailable] = useState(false);
  const consume = useCallback((states: readonly QcStateUpdate[]) => {
    for (const state of states) {
      if (state.kind !== "history") continue;
      if (state.undoAvailable !== undefined) setUndoAvailable(state.undoAvailable);
      if (state.redoAvailable !== undefined) setRedoAvailable(state.redoAvailable);
    }
  }, []);
  const run = async (direction: "undo" | "redo") => {
    if (direction === "undo" ? history.undoEntry : history.redoEntry) { await history[direction](); return; }
    if (!(direction === "undo" ? undoAvailable : redoAvailable) || !snapshot.presetIdentity) return;
    try { await gateway[direction](snapshot.presetIdentity); }
    catch (error) { fail(error); }
  };
  return {
    undoAvailable, redoAvailable, consume,
    undo: () => run("undo"),
    redo: () => run("redo"),
    canUndo: Boolean(history.undoEntry) || undoAvailable,
    undoLabel: history.undoEntry?.label
  };
}
