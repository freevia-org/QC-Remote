import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { captureGatewaySession } from "./gateway-session.ts";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options.ts";

export type ModesConfigurationWorkflowOptions = DeviceGatewayWorkflowOptions;
export function useModesConfigurationWorkflow(options: ModesConfigurationWorkflowOptions) {
  const workflowSession = useDeviceWorkflowSession(options);
  options = workflowSession.options;
  const { gateway, lifetime, connected, pending, notice, fail } = options;
  const guarded=useGuardedMutation(options,"changing Modes Configuration");
  const [slots, setSlots] = useState<number[]>();
  const stateOwner = useRef(workflowSession.key);
  useEffect(() => { stateOwner.current = workflowSession.key; }, [workflowSession.key]);
  const [loading, setLoading] = useState(false);
  const [contextVersion, setContextVersion] = useState(0);
  const session = lifetime?.checkpoint().session;
  const renderedOrigin = useMemo(() => lifetime?.checkpoint(), [lifetime, session]);
  const reads=useRef(0); const mounted=useRef(true); const latestGateway=useRef(gateway);latestGateway.current=gateway;
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;reads.current++;};},[]);
  const capture=useCallback(()=>captureGatewaySession(gateway,()=>mounted.current && latestGateway.current===gateway && (!renderedOrigin || lifetime!.currentSession(renderedOrigin))),[gateway,lifetime,renderedOrigin]);
  useEffect(()=>{const reset=()=>{reads.current++;setSlots(undefined);setLoading(false);setContextVersion(version=>version+1);};reset();let origin=lifetime?.checkpoint();return lifetime?.subscribe(()=>{if(origin && !lifetime.currentSession(origin)){origin=lifetime.checkpoint();reset();}});},[gateway,lifetime]);
  const refresh = useCallback(async () => {
    if (!connected) { notice("Connect the Quad Cortex to read Modes Configuration."); return; }
    const session=capture();if(!session.current()) return;
    const read=++reads.current;const current=()=>session.current() && reads.current===read;
    setLoading(true);
    try {const observed=await session.gateway.modeCycle();if(current()) setSlots([...observed.slots]);}
    catch (error) {if(current()) fail(error);}
    finally {if(current()) setLoading(false);}
  }, [capture,connected, fail, notice]);
  const commit = useCallback(async (next: number[]) => {
    if (!connected || pending) { notice(!connected ? "Connect the Quad Cortex before changing Modes Configuration." : "A device command is already in progress."); return; }
    if (next.length < 1 || next.length > 3 || new Set(next).size !== next.length || next.some((slot) => !Number.isInteger(slot) || slot < 0 || slot > 8)) {
      notice("Modes Configuration must contain one to three unique modes."); return;
    }
    const session=capture();
    await guarded.run({current:session.current,write:()=>session.gateway.setModeCycle([...next]),settle:async(result)=>{
      const read=++reads.current;
      try {
      const readback = await session.gateway.modeCycle();
      if(!session.current() || read!==reads.current) return;
      if (readback.slots.length !== next.length || readback.slots.some((slot, i) => slot !== next[i])) throw new Error("Modes Configuration readback did not match the requested order.");
      setSlots([...readback.slots]);
      notice(result.detail || "Modes Configuration updated and verified.");
      } finally { if(session.current() && read===reads.current) setLoading(false); }
    }});
  }, [capture,connected, guarded.run,notice,pending]);
  return { slots: stateOwner.current === workflowSession.key ? slots : undefined,
    loading: stateOwner.current === workflowSession.key && loading, pending, refresh, commit, contextVersion };
}
export type ModesConfigurationWorkflow = ReturnType<typeof useModesConfigurationWorkflow>;
