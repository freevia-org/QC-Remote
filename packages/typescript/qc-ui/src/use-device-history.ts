import { useCallback, useMemo } from "react";
import type { DeviceActionResult, PresetSnapshot } from "@qc-remote/client";
import { useCommandJournal } from "./use-command-journal.ts";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";

export interface DeviceHistoryEntry {
  label: string;
  execute(current: PresetSnapshot): Promise<DeviceActionResult>;
  redo(current: PresetSnapshot): Promise<DeviceActionResult>;
}

export interface DeviceHistoryOptions extends DeviceGatewayWorkflowOptions {
  snapshot: PresetSnapshot;
  reconcile(snapshot: PresetSnapshot): void;
}

/** Shared verified one-step history used by every device-editing workflow. */
export function useDeviceHistory(options: DeviceHistoryOptions) {
  const journal = useCommandJournal<DeviceHistoryEntry>();
  const { undoEntry, redoEntry, record, clear, markUndone, markRedone } = journal;
  const { gateway, snapshot, reconcile, notice } = options;
  const guarded = useGuardedMutation(options, "changing the live preset");

  const run = useCallback(async (direction: "undo" | "redo") => {
    const entry = direction === "undo" ? undoEntry : redoEntry;
    if (!entry) {
      notice(`There is no app action to ${direction}.`);
      return;
    }
    await guarded.run({
      action: `${direction}ing a live action`,
      progress: `${direction === "undo" ? "Undoing" : "Redoing"} ${entry.label}…`,
      write: () => direction === "undo" ? entry.execute(snapshot) : entry.redo(snapshot),
      settle: async (result) => {
        const verified = result.snapshot ?? await gateway.currentSnapshot();
        reconcile(verified);
        if (direction === "undo") markUndone(entry);
        else markRedone(entry);
        notice(`${direction === "undo" ? "Undid" : "Redid"} ${entry.label}. ${result.detail}`);
      },
      // A failed step keeps its entry for retry; re-read what the device holds.
      recover: async () => reconcile(await gateway.currentSnapshot())
    });
  }, [gateway, guarded.run, markRedone, markUndone, notice, reconcile, redoEntry, snapshot, undoEntry]);

  const undo = useCallback(() => run("undo"), [run]);
  const redo = useCallback(() => run("redo"), [run]);
  return useMemo(() => ({ undoEntry, redoEntry, record, clear, markUndone, markRedone, undo, redo }),
    [clear, markRedone, markUndone, record, redo, redoEntry, undo, undoEntry]);
}
