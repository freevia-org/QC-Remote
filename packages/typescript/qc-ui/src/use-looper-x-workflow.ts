import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import { useCallback, useEffect, useRef, useState } from "react";
import type { LooperStatus } from "@qc-remote/client";
import { connectBeforeNotice, useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";
import { captureGatewaySession } from "./gateway-session.ts";

export type LooperXWorkflowOptions = DeviceGatewayWorkflowOptions;

/** Device-authoritative Looper X workflow. Looper commands have no correlated
 * acknowledgement, so accepted commands are followed by a best-effort status read. */
export function useLooperXWorkflow(options: LooperXWorkflowOptions) {
  const workflowSession = useDeviceWorkflowSession(options);
  const stateOwner = useRef(workflowSession.key);
  const currentState = stateOwner.current === workflowSession.key && workflowSession.current();
  useEffect(() => { stateOwner.current = workflowSession.key; }, [workflowSession.key]);
  const { gateway, lifetime, connected, pending, notice, fail } = workflowSession.options;
  const guarded = useGuardedMutation(workflowSession.options, "controlling Looper X");
  const [status, setStatus] = useState<LooperStatus>();
  const [loading, setLoading] = useState(false);
  const latestGateway = useRef(gateway); latestGateway.current = gateway;
  const reads = useRef(0); const mounted = useRef(true);
  useEffect(() => { mounted.current=true; return () => { mounted.current=false; reads.current++; }; }, []);
  const capture = useCallback(() => {
    const origin=lifetime?.checkpoint();
    return captureGatewaySession(gateway, () => mounted.current && latestGateway.current === gateway && (!origin || lifetime!.currentSession(origin)));
  }, [gateway, lifetime]);
  useEffect(() => {
    const reset=()=>{ reads.current++; setStatus(undefined); setLoading(false); };
    reset(); let origin=lifetime?.checkpoint();
    return lifetime?.subscribe(()=>{ if(origin && !lifetime.currentSession(origin)) {origin=lifetime.checkpoint();reset();} });
  }, [gateway,lifetime]);

  const refresh = useCallback(async () => {
    if (!connected) { notice(connectBeforeNotice("reading Looper X status")); return; }
    const session=capture();
    if (!session.current()) return;
    const read=++reads.current;
    const current=()=>session.current() && read===reads.current;
    setLoading(true);
    try { const observed=await session.gateway.looperStatus(); if(current()) setStatus(observed); }
    catch (error) { if(current()) fail(error); }
    finally { if(current()) setLoading(false); }
  }, [capture, connected, fail, notice]);

  const run = useCallback(async (command: string, value: number | null = null) => {
    const session=capture();
    return guarded.run({
      current:session.current,
      write: () => session.gateway.controlLooper(command, value),
      settle: async (result) => {
        notice(result.detail || `${command} sent; device confirmation is not available.`);
        // The status read is best effort: a failed read leaves the command
        // sent but unconfirmed, which is not a command failure.
        const read=++reads.current;
        try { const observed=await session.gateway.looperStatus(); if(session.current() && read===reads.current) setStatus(observed); }
        catch { if(session.current()) notice(`${result.detail || `${command} sent.`} Status could not be refreshed; the command is not confirmed.`); }
        finally { if(session.current() && read===reads.current) setLoading(false); }
      }
    });
  }, [capture, guarded.run, notice]);

  const captureContinuation = useCallback(() => {
    const session = capture();
    const origin = lifetime?.checkpoint();
    return () => session.current() && (!origin || lifetime!.current(origin));
  }, [capture, lifetime]);
  return { status: currentState ? status : undefined, loading: currentState && loading, pending, refresh, run, captureContinuation };
}

export type LooperXWorkflow = ReturnType<typeof useLooperXWorkflow>;
