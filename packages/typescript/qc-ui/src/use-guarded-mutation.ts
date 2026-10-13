import { claimDeviceCommand, deviceCommandBusy, releaseDeviceCommand } from "./device-command-ownership.ts";
import { useCallback, useEffect, useMemo, useRef } from "react";
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
  const latest = useRef(options);
  latest.current = options;
  const operation = useRef<{ current(): boolean; release(): void; retire(): void } | null>(null);
  const scope = useRef({ key: options.sessionKey, connected: options.connected });
  const retiredRelease = useRef<(() => void) | undefined>(undefined);
  if (!Object.is(scope.current.key, options.sessionKey) || scope.current.connected !== options.connected) {
    scope.current = { key: options.sessionKey, connected: options.connected };
    retiredRelease.current = operation.current?.release;
    operation.current?.retire();
    operation.current = null;
  }
  useEffect(() => {
    const release = retiredRelease.current;
    retiredRelease.current = undefined;
    if (!operation.current) release?.();
  }, [options.connected, options.sessionKey]);
  useEffect(() => () => {
    const held = operation.current;
    operation.current = null;
    held?.retire();
    held?.release();
  }, []);

  const admit = useCallback((override?: string) => {
    const host = latest.current;
    if (!host.connected) { host.notice(connectBeforeNotice(override ?? action)); return false; }
    if (host.pending || operation.current || deviceCommandBusy(host.setPending)) { host.notice(DEVICE_COMMAND_BUSY_NOTICE); return false; }
    return true;
  }, [action]);

  const run = useCallback(async <T,>(step: GuardedMutationStep<T>) => {
    if (!admit(step.action)) return false;
    const host = latest.current;
    const probe = host.captureSessionCurrent?.() ?? (() => true);
    if (!probe()) return false;
    const held = { current: () => false,
      release: () => { if (!deviceCommandBusy(host.setPending)) host.setPending(false); },
      retire: () => { releaseDeviceCommand(host.setPending, held); }
    };
    if (!claimDeviceCommand(host.setPending, held)) { host.notice(DEVICE_COMMAND_BUSY_NOTICE); return false; }
    held.current = () => operation.current === held && probe();
    operation.current = held;
    host.setPending(true);
    if (step.progress) host.notice(step.progress);
    try {
      if (!held.current()) return false;
      const result = await step.write();
      if (!held.current()) return false;
      await step.settle?.(result);
      return held.current();
    } catch (error) {
      if (!held.current()) return false;
      host.fail(error);
      if (step.recover && held.current()) {
        try { await step.recover(error); } catch { /* The reported error stands. */ }
      }
      return false;
    } finally {
      if (operation.current === held) {
        operation.current = null;
        held.retire();
        held.release();
      }
    }
  }, [admit]);

  return useMemo(() => ({ admit, run }), [admit, run]);
}
