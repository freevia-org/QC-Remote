import { claimDeviceCommand, completeDeviceCommand, deviceCommandBusy, registerDeviceCommandSession } from "./device-command-ownership.ts";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  const [, replaceMountedGuard] = useState(0);
  const latest = useRef(options);
  latest.current = options;
  const mounted = useRef(true);
  const generation = useRef(0);
  const operation = useRef<{ current(): boolean; started: boolean; setter: DeviceCommandOptions["setPending"] } | null>(null);
  const scope = useRef({ key: options.sessionKey, connected: options.connected });
  const pendingReset = useRef(false);
  const physicalKey = options.commandSessionKey ?? options.sessionKey;
  if (physicalKey !== undefined) pendingReset.current = registerDeviceCommandSession(options.setPending, physicalKey, options.commandSessionKey !== undefined) || pendingReset.current;
  if (!Object.is(scope.current.key, options.sessionKey) || scope.current.connected !== options.connected) {
    scope.current = { key: options.sessionKey, connected: options.connected };
    generation.current++;
    // The host context registration above retires genuinely replaced sessions.
    // A callback owner change alone leaves an already dispatched lease held.
    operation.current = null;
  }
  useEffect(() => {
    if (pendingReset.current) {
      pendingReset.current = false;
      if (!deviceCommandBusy(latest.current.setPending)) latest.current.setPending(false);
    }
  }, [physicalKey]);
  useEffect(() => {
    const replayed = !mounted.current;
    mounted.current = true;
    if (replayed) replaceMountedGuard((version) => version + 1);
    return () => {
      mounted.current = false;
      generation.current++;
      const held = operation.current;
      operation.current = null;
      if (held && !held.started) completeDeviceCommand(held.setter, held);
    };
  }, []);
  const renderGeneration = generation.current;
  const admit = useCallback((override?: string) => {
    if (!mounted.current || renderGeneration !== generation.current) return false;
    const host = latest.current;
    if (!host.connected) { host.notice(connectBeforeNotice(override ?? action)); return false; }
    if (host.pending || operation.current || deviceCommandBusy(host.setPending)) { host.notice(DEVICE_COMMAND_BUSY_NOTICE); return false; }
    return true;
  }, [action, renderGeneration]);

  const run = useCallback(async <T,>(step: GuardedMutationStep<T>) => {
    if (!admit(step.action)) return false;
    const host = latest.current;
    const probe = host.captureSessionCurrent?.() ?? (() => true);
    if (!probe()) return false;
    const held = { current: () => false, started: false, setter: host.setPending };
    if (!claimDeviceCommand(host.setPending, held)) { host.notice(DEVICE_COMMAND_BUSY_NOTICE); return false; }
    held.current = () => mounted.current && renderGeneration === generation.current && operation.current === held && probe();
    operation.current = held;
    host.setPending(true);
    if (step.progress) host.notice(step.progress);
    try {
      if (!held.current()) return false;
      held.started = true;
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
      if (operation.current === held) operation.current = null;
      // Exact-token completion reaches the host even after scoped UI cleanup.
      completeDeviceCommand(held.setter, held);
    }
  }, [admit, renderGeneration]);

  return useMemo(() => ({ admit, run }), [admit, run]);
}
