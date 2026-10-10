import { useCallback, useMemo } from "react";
import type { DeviceCommandOptions } from "./workflow-options";

/** The notice for a write refused because another device command owns the pending flag. */
export const DEVICE_COMMAND_BUSY_NOTICE = "A device command is already in progress.";

/** The notice for a write refused because no Quad Cortex is connected. */
export function connectBeforeNotice(action: string): string {
  return `Connect the Quad Cortex before ${action}.`;
}

export interface GuardedMutationStep<T> {
  /** Overrides the hook's action wording in the "Connect the Quad Cortex before …" notice. */
  action?: string;
  /** Announced once the write is admitted, before it is sent. */
  progress?: string;
  /** The device write. */
  write(): Promise<T>;
  /** Readback, reconciliation and the completion notice, still inside the pending window. */
  settle?(result: T): void | Promise<void>;
  /** Runs after a failure has been reported, e.g. to re-read device state. Its own errors are dropped. */
  recover?(error: unknown): void | Promise<void>;
}

export interface GuardedMutation {
  /**
   * Whether a write may start now. When it may not, posts the notice that
   * names why (not connected, or another command is in flight) and returns false.
   */
  admit(action?: string): boolean;
  /**
   * Admit, then run one guarded write. Resolves true once the write and its
   * settle step have both completed, false when it was refused or failed.
   */
  run<T>(step: GuardedMutationStep<T>): Promise<boolean>;
}

/**
 * The one policy every device-mutation workflow follows, instead of each hook
 * re-implementing it with its own variations:
 *
 * - A write is refused, with a notice that says why, while no Quad Cortex is
 *   connected or another command owns the app-wide pending flag. Nothing is
 *   ever refused silently, and nothing refused is sent.
 * - An admitted write holds the pending flag for the write and its settle step
 *   (readback, reconciliation, completion notice), then always releases it.
 * - Any failure in the write or the settle step goes to `fail`, once; the
 *   optional `recover` step then runs and cannot replace that error.
 *
 * `action` completes "Connect the Quad Cortex before …" for this workflow.
 */
export function useGuardedMutation(options: DeviceCommandOptions, action: string): GuardedMutation {
  const { connected, pending, setPending, notice, fail } = options;

  const admit = useCallback((override?: string) => {
    if (!connected) { notice(connectBeforeNotice(override ?? action)); return false; }
    if (pending) { notice(DEVICE_COMMAND_BUSY_NOTICE); return false; }
    return true;
  }, [action, connected, notice, pending]);

  const run = useCallback(async <T,>(step: GuardedMutationStep<T>) => {
    if (!admit(step.action)) return false;
    setPending(true);
    if (step.progress) notice(step.progress);
    try {
      const result = await step.write();
      await step.settle?.(result);
      return true;
    } catch (error) {
      fail(error);
      if (step.recover) {
        try { await step.recover(error); } catch { /* The reported error stands. */ }
      }
      return false;
    } finally {
      setPending(false);
    }
  }, [admit, fail, notice, setPending]);

  return useMemo(() => ({ admit, run }), [admit, run]);
}
