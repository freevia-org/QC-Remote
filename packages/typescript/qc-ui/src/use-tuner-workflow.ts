import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DeviceActionResult, TunerSettings } from "@qc-remote/client";
import { useLatestValueWriter } from "./latest-value-writer";
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
  const session = useDeviceWorkflowSession(options);
  const { gateway, connected, pending, setPending, notice, fail } = session.options;
  const stateOwner = useRef(session.key);
  const currentState = stateOwner.current === session.key;
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [available, setAvailable] = useState(true);
  const [settings, setSettings] = useState<TunerSettings>();

  useEffect(() => { stateOwner.current = session.key; setOpen(false); setSettings(undefined); setLoading(false); setAvailable(true); }, [session.key]);

  const refresh = useCallback(async () => {
    if (!session.current()) return;
    try {
      const next = await gateway.tunerSettings();
      session.apply(() => { setSettings(next); setAvailable(true); });
    } catch {
      session.apply(() => setAvailable(false));
    }
  }, [gateway, session.apply, session.current]);

  const openTuner = useCallback(() => {
    if (!session.current()) return;
    setOpen(true);
    setLoading(true);
    void gateway.tunerSettings()
      .then((value) => session.apply(() => { setSettings(value); setAvailable(true); }))
      .catch(() => session.apply(() => setAvailable(false)))
      .finally(() => session.apply(() => setLoading(false)));
  }, [gateway, session.apply, session.current]);

  const close = useCallback(() => session.apply(() => setOpen(false)), [session.apply]);

  const reference = useLatestValueWriter<number>({ pending, setPending, resetKey: session.key });

  const commit = useCallback((label: string, action: () => Promise<DeviceActionResult>) => {
    if (!session.current()) return;
    if (!connected) {
      notice(connectBeforeNotice(TUNER_ACTION));
      return;
    }
    const accepted = reference.runCommand(async () => {
      try {
        const result = await action();
        notice(result.detail ?? label);
        await refresh();
      } catch (error) { fail(error); }
    }, fail);
    if (!accepted) notice(DEVICE_COMMAND_BUSY_NOTICE);
  }, [connected, fail, notice, reference.runCommand, refresh, session.current]);

  const setInput = useCallback((inputPortId: TunerSettings["inputPortId"]) =>
    commit("Tuner input updated.", () => gateway.setTunerInput(inputPortId, true)), [commit, gateway]);
  const setMuted = useCallback((muted: boolean) =>
    commit(muted ? "Tuner audio muted." : "Tuner audio restored.", () => gateway.setTunerMute(muted, true)), [commit, gateway]);
  const setReferenceOffset = useCallback((referenceOffsetHz: number) => {
    if (!session.current()) return;
    if (!connected) { notice(connectBeforeNotice(TUNER_ACTION)); return; }
    const accepted = reference.submit("reference", referenceOffsetHz, {
      write: (value) => gateway.setTunerReference(value, true),
      settle: async (last) => {
        notice(last?.detail ?? "Tuner reference updated.");
        await refresh();
      },
      error: fail
    });
    if (!accepted) notice(DEVICE_COMMAND_BUSY_NOTICE);
  }, [connected, fail, gateway, notice, reference.submit, refresh, session.current]);
  const setMeterEnabled = useCallback((enabled: boolean) =>
    commit(`Live Tuner ${enabled ? "enabled" : "disabled"}.`, () => gateway.setTunerMeter(enabled, true)), [commit, gateway]);

  const draftOffset = reference.drafts.get("reference");
  const view = useMemo(() => settings && draftOffset !== undefined
    ? { ...settings, referenceOffsetHz: draftOffset, referenceHz: settings.referenceHz - settings.referenceOffsetHz + draftOffset }
    : settings, [draftOffset, settings]);

  // The reference dial stays live while its own value is being written.
  return { open: currentState && open, openTuner, close, loading: currentState && loading, available: !currentState || available, pending: pending && !reference.active, settings: currentState ? view : undefined, refresh, setInput, setMuted, setReferenceOffset, setMeterEnabled };
}

export type TunerWorkflow = ReturnType<typeof useTunerWorkflow>;
