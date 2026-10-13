import { useCallback, useEffect, useRef, useState } from "react";
import type { LooperStatus } from "@qc-remote/client";
import { connectBeforeNotice, useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";

export type LooperXWorkflowOptions = DeviceGatewayWorkflowOptions;

/** Device-authoritative Looper X workflow. Looper commands have no correlated
 * acknowledgement, so accepted commands are followed by a best-effort status read. */
export function useLooperXWorkflow(options: LooperXWorkflowOptions) {
  const session = useDeviceWorkflowSession(options);
  const { gateway, connected, pending, notice, fail } = session.options;
  const guarded = useGuardedMutation(session.options, "controlling Looper X");
  const [status, setStatus] = useState<LooperStatus>();
  const [loading, setLoading] = useState(false);
  const stateOwner = useRef(session.key);
  useEffect(() => { stateOwner.current = session.key; setStatus(undefined); setLoading(false); }, [session.key]);

  const refresh = useCallback(async () => {
    if (!session.current()) return;
    if (!connected) { notice(connectBeforeNotice("reading Looper X status")); return; }
    setLoading(true);
    try { const value = await gateway.looperStatus(); session.apply(() => setStatus(value)); }
    catch (error) { fail(error); }
    finally { session.apply(() => setLoading(false)); }
  }, [connected, fail, gateway, notice, session]);

  const run = useCallback(async (command: string, value: number | null = null) => {
    if (!session.current()) return false;
    return await guarded.run({
      write: () => gateway.controlLooper(command, value),
      settle: async (result) => {
        notice(result.detail || `${command} sent; device confirmation is not available.`);
        // The status read is best effort: a failed read leaves the command
        // sent but unconfirmed, which is not a command failure.
        try { const value = await gateway.looperStatus(); session.apply(() => setStatus(value)); }
        catch { notice(`${result.detail || `${command} sent.`} Status could not be refreshed; the command is not confirmed.`); }
      }
    });
  }, [gateway, guarded.run, notice, session]);

  return { status: stateOwner.current === session.key ? status : undefined,
    loading: stateOwner.current === session.key && loading, pending, refresh, run };
}

export type LooperXWorkflow = ReturnType<typeof useLooperXWorkflow>;
