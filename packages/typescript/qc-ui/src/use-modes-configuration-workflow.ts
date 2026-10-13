import { useCallback, useEffect, useRef, useState } from "react";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";

export type ModesConfigurationWorkflowOptions = DeviceGatewayWorkflowOptions;

export function useModesConfigurationWorkflow(options: ModesConfigurationWorkflowOptions) {
  const session = useDeviceWorkflowSession(options);
  const { gateway, connected, pending, notice, fail } = session.options;
  const guarded = useGuardedMutation(session.options, "changing Modes Configuration");
  const [slots, setSlots] = useState<number[]>();
  const [loading, setLoading] = useState(false);
  const stateOwner = useRef(session.key);
  useEffect(() => { stateOwner.current = session.key; setSlots(undefined); setLoading(false); }, [session.key]);
  const refresh = useCallback(async () => {
    if (!session.current()) return;
    if (!connected) { notice("Connect the Quad Cortex to read Modes Configuration."); return; }
    setLoading(true);
    try { const value = await gateway.modeCycle(); session.apply(() => setSlots([...value.slots])); }
    catch (error) { fail(error); }
    finally { session.apply(() => setLoading(false)); }
  }, [connected, fail, gateway, notice, session]);
  const commit = useCallback(async (next: number[]) => {
    if (!session.current()) return;
    if (next.length < 1 || next.length > 3 || new Set(next).size !== next.length || next.some((slot) => !Number.isInteger(slot) || slot < 0 || slot > 8)) {
      notice("Modes Configuration must contain one to three unique modes."); return;
    }
    await guarded.run({
      write: () => gateway.setModeCycle(next),
      settle: async (result) => {
        const readback = await gateway.modeCycle();
        if (readback.slots.length !== next.length || readback.slots.some((slot, i) => slot !== next[i])) throw new Error("Modes Configuration readback did not match the requested order.");
        session.apply(() => setSlots([...readback.slots]));
        notice(result.detail || "Modes Configuration updated and verified.");
      }
    });
  }, [gateway, guarded.run, notice, session]);
  return { slots: stateOwner.current === session.key ? slots : undefined,
    loading: stateOwner.current === session.key && loading, pending, refresh, commit };
}

export type ModesConfigurationWorkflow = ReturnType<typeof useModesConfigurationWorkflow>;
