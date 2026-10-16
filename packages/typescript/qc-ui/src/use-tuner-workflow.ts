import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DeviceActionResult, GatewayTransport, TunerSettings } from "@qc-remote/client";
import { captureGatewaySession } from "./gateway-session.ts";
import { useLatestValueWriter } from "./latest-value-writer.ts";
import { connectBeforeNotice, DEVICE_COMMAND_BUSY_NOTICE } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";

const TUNER_ACTION = "changing tuner settings";

export type TunerWorkflowOptions = DeviceGatewayWorkflowOptions;

/** Stateful, semantic controller for the QC Tuner screen. Meter samples are
 * intentionally optional; every setting control uses the native gateway and
 * is reconciled from a fresh tunerSettings() read after a successful write.
 * The reference dial writes through the shared latest-value writer, so the
 * last position of a drag is the one written and read back. */
export function useTunerWorkflow(options: TunerWorkflowOptions) {
  const workflowSession = useDeviceWorkflowSession(options);
  const stateOwner = useRef(workflowSession.key);
  const currentState = stateOwner.current === workflowSession.key && workflowSession.current();
  useEffect(() => { stateOwner.current = workflowSession.key; }, [workflowSession.key]);
  const { gateway, lifetime, connected, pending, setPending, notice, fail } = workflowSession.options;
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [available, setAvailable] = useState(true);
  const [settings, setSettings] = useState<TunerSettings>();
  const latestGateway = useRef(gateway); latestGateway.current = gateway;
  const generation = useRef(0);
  const reads = useRef(0);
  const mounted = useRef(true);
  const [, refreshPhysicalSession] = useState(0);
  const physicalSession = lifetime?.checkpoint().session;
  // Retained controls belong to the physical session that rendered them.
  // A normal preset change does not invalidate device-wide tuner settings.
  const renderedOrigin = useMemo(() => lifetime?.checkpoint(), [lifetime, physicalSession]);
  const reference = useLatestValueWriter<number>({ pending, setPending, lifetime,
    resetKey: workflowSession.key, commandSessionKey: workflowSession.options.commandSessionKey });
  const capture = useCallback(() => {
    const origin = renderedOrigin; const owner = generation.current;
    return captureGatewaySession(gateway, () => mounted.current && latestGateway.current === gateway && owner === generation.current && (!origin || lifetime!.currentSession(origin)));
  }, [gateway, lifetime, renderedOrigin]);
  const reset = useCallback(() => {
    generation.current++; reads.current++; reference.cancel();
    refreshPhysicalSession((value) => value + 1);
    setOpen(false); setLoading(false); setSettings(undefined); setAvailable(true);
  }, [reference.cancel]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; generation.current++; reads.current++; }; }, []);
  useEffect(() => { reset(); }, [gateway, reset]);
  useEffect(() => {
    let origin = lifetime?.checkpoint();
    return lifetime?.subscribe(() => { if (origin && !lifetime.currentSession(origin)) { origin=lifetime.checkpoint(); reset(); } });
  }, [lifetime, reset]);

  const refresh = useCallback(async (session = capture()) => {
    if (!session.current()) return;
    const read = ++reads.current;
    const current = () => session.current() && read === reads.current;
    try {
      const observed = await session.gateway.tunerSettings();
      if (current()) { setSettings(observed); setAvailable(true); }
    } catch {
      if (current()) setAvailable(false);
    }
    if (current()) setLoading(false);
  }, [capture]);

  const openTuner = useCallback(() => {
    const session = capture();
    if (!session.current()) return;
    setOpen(true);
    setLoading(true);
    void refresh(session);
  }, [capture, refresh]);

  const close = useCallback(() => { generation.current++; reads.current++; reference.cancel(); setOpen(false); setLoading(false); }, [reference.cancel]);

  const commit = useCallback((label: string, action: (gateway: GatewayTransport) => Promise<DeviceActionResult>) => {
    if (!connected) {
      notice(connectBeforeNotice(TUNER_ACTION));
      return;
    }
    const session = capture();
    if (!session.current()) return;
    const accepted = reference.runCommand(async () => {
      if (!session.current()) return;
      try {
        const result = await action(session.gateway);
        if (!session.current()) return;
        notice(result.detail ?? label);
        await refresh(session);
      } catch (error) { if (session.current()) fail(error); }
    }, (error) => { if (session.current()) fail(error); });
    if (!accepted) notice(DEVICE_COMMAND_BUSY_NOTICE);
  }, [capture, connected, fail, notice, reference.runCommand, refresh]);

  const setInput = useCallback((inputPortId: TunerSettings["inputPortId"]) =>
    commit("Tuner input updated.", (gateway) => gateway.setTunerInput(inputPortId, true)), [commit]);
  const setMuted = useCallback((muted: boolean) =>
    commit(muted ? "Tuner audio muted." : "Tuner audio restored.", (gateway) => gateway.setTunerMute(muted, true)), [commit]);
  const setReferenceOffset = useCallback((referenceOffsetHz: number) => {
    if (!connected) { notice(connectBeforeNotice(TUNER_ACTION)); return; }
    const session = capture();
    if (!session.current()) return;
    const accepted = reference.submit("reference", referenceOffsetHz, {
      write: (value) => session.gateway.setTunerReference(value, true),
      settle: async (last) => {
        if (!session.current()) return;
        notice(last?.detail ?? "Tuner reference updated.");
        await refresh(session);
      },
      error: (error) => { if (session.current()) fail(error); }
    });
    if (!accepted) notice(DEVICE_COMMAND_BUSY_NOTICE);
  }, [capture, connected, fail, notice, reference.submit, refresh]);
  const setMeterEnabled = useCallback((enabled: boolean) =>
    commit(`Live Tuner ${enabled ? "enabled" : "disabled"}.`, (gateway) => gateway.setTunerMeter(enabled, true)), [commit]);

  const draftOffset = reference.drafts.get("reference");
  const view = useMemo(() => settings && draftOffset !== undefined
    ? { ...settings, referenceOffsetHz: draftOffset, referenceHz: settings.referenceHz - settings.referenceOffsetHz + draftOffset }
    : settings, [draftOffset, settings]);

  // The reference dial stays live while its own value is being written.
  return { open: currentState && open, openTuner, close, loading: currentState && loading, available: currentState && available, pending: pending && !reference.active, settings: currentState ? view : undefined, refresh, setInput, setMuted, setReferenceOffset, setMeterEnabled };
}

export type TunerWorkflow = ReturnType<typeof useTunerWorkflow>;
