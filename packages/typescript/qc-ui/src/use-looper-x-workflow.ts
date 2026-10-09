import { useCallback, useState } from "react";
import type { LooperStatus } from "@qc-remote/client";
import { connectBeforeNotice, useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";

export type LooperXWorkflowOptions = DeviceGatewayWorkflowOptions;

/** Device-authoritative Looper X workflow. Looper commands have no correlated
 * acknowledgement, so accepted commands are followed by a best-effort status read. */
export function useLooperXWorkflow(options: LooperXWorkflowOptions) {
  const { gateway, connected, pending, notice, fail } = options;
  const guarded = useGuardedMutation(options, "controlling Looper X");
  const [status, setStatus] = useState<LooperStatus>();
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    if (!connected) { notice(connectBeforeNotice("reading Looper X status")); return; }
    setLoading(true);
    try { setStatus(await gateway.looperStatus()); }
    catch (error) { fail(error); }
    finally { setLoading(false); }
  }, [connected, fail, gateway, notice]);

  const run = useCallback(async (command: string, value: number | null = null) => {
    await guarded.run({
      write: () => gateway.controlLooper(command, value),
      settle: async (result) => {
        notice(result.detail || `${command} sent; device confirmation is not available.`);
        // The status read is best effort: a failed read leaves the command
        // sent but unconfirmed, which is not a command failure.
        try { setStatus(await gateway.looperStatus()); }
        catch { notice(`${result.detail || `${command} sent.`} Status could not be refreshed; the command is not confirmed.`); }
      }
    });
  }, [gateway, guarded.run, notice]);

  return { status, loading, pending, refresh, run };
}

export type LooperXWorkflow = ReturnType<typeof useLooperXWorkflow>;
