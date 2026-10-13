import { useCallback, useMemo, useState } from "react";
import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import type { PresetLifetime, QcStateUpdate } from "@qc-remote/core";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceCommandOptions } from "./workflow-options.ts";

export interface QcDeviceUndoOptions extends DeviceCommandOptions {
  /** The app's own verified one-step history (useDeviceHistory). */
  history: { undoEntry?: { label: string }; redoEntry?: { label: string }; undo(): Promise<unknown>; redo(): Promise<unknown> };
  snapshot: Pick<PresetSnapshot, "presetIdentity" | "setlistKey" | "presetPosition" | "stateVersion">;
  gateway: Pick<GatewayTransport, "undo" | "redo" | "currentSnapshot">;
  reconcile(snapshot: PresetSnapshot): void;
  /** Explicit same-address reloads also replace the loaded preset's history. */
  presetLifetime?: PresetLifetime;
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
export function useQcDeviceUndo(options: QcDeviceUndoOptions): QcDeviceUndo {
  const { snapshot } = options;
  // History belongs to a loaded preset in one transport session. A revision of
  // that same preset is not a navigation boundary: its history can still be used.
  const sessionKey = useMemo(() => ({
    host: options.sessionKey, gateway: options.gateway,
    native: snapshot.stateVersion?.session,
    setlist: snapshot.setlistKey, position: snapshot.presetPosition,
    preset: options.presetLifetime, revision: options.presetLifetime?.version
  }), [options.sessionKey, options.gateway, snapshot.stateVersion?.session, snapshot.setlistKey, snapshot.presetPosition,
    options.presetLifetime, options.presetLifetime?.version]);
  const lifetime = useDeviceWorkflowSession({ ...options, sessionKey,
    commandSessionKey: options.commandSessionKey ?? options.sessionKey ?? options.gateway,
    captureSessionCurrent: () => {
      const hostCurrent = options.captureSessionCurrent?.() ?? (() => true);
      const preset = options.presetLifetime?.checkpoint();
      return () => hostCurrent() && (!preset || options.presetLifetime!.current(preset));
    }
  });
  const { history, gateway, reconcile } = lifetime.options;
  const guarded = useGuardedMutation(lifetime.options, "changing the live preset");
  const [observed, setObserved] = useState<{ key: unknown; undo: boolean; redo: boolean }>();
  const current = lifetime.current();
  const undoAvailable = current && observed?.key === lifetime.key ? observed.undo : false;
  const redoAvailable = current && observed?.key === lifetime.key ? observed.redo : false;
  const consume = useCallback((states: readonly QcStateUpdate[]) => {
    if (!lifetime.current()) return;
    for (const state of states) {
      if (state.kind !== "history") continue;
      if (state.stateVersion && state.stateVersion.session !== snapshot.stateVersion?.session) continue;
      setObserved((previous) => ({
        key: lifetime.key,
        undo: state.undoAvailable ?? (previous?.key === lifetime.key ? previous.undo : false),
        redo: state.redoAvailable ?? (previous?.key === lifetime.key ? previous.redo : false)
      }));
    }
  }, [lifetime.key, snapshot.stateVersion?.session]);
  const run = async (direction: "undo" | "redo") => {
    if (!lifetime.current()) return;
    if (direction === "undo" ? history.undoEntry : history.redoEntry) { await history[direction](); return; }
    if (!(direction === "undo" ? undoAvailable : redoAvailable) || !snapshot.presetIdentity) return;
    const identity = snapshot.presetIdentity;
    await guarded.run({
      write: () => gateway[direction](identity),
      settle: async (result) => {
        const next = result.snapshot ?? await gateway.currentSnapshot();
        lifetime.apply(() => reconcile(next));
      }
    });
  };
  return {
    undoAvailable, redoAvailable, consume,
    undo: () => run("undo"),
    redo: () => run("redo"),
    canUndo: current && (Boolean(history.undoEntry) || undoAvailable),
    undoLabel: current ? history.undoEntry?.label : undefined
  };
}
