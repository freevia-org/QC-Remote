import { useCallback, useMemo, useState } from "react";
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
  const { gateway, connected, pending, setPending, notice, fail } = options;
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [available, setAvailable] = useState(true);
  const [settings, setSettings] = useState<TunerSettings>();

  const refresh = useCallback(async () => {
    try {
      setSettings(await gateway.tunerSettings());
      setAvailable(true);
    } catch {
      setAvailable(false);
    }
  }, [gateway]);

  const openTuner = useCallback(() => {
    setOpen(true);
    setLoading(true);
    void gateway.tunerSettings()
      .then((value) => { setSettings(value); setAvailable(true); })
      .catch(() => setAvailable(false))
      .finally(() => setLoading(false));
  }, [gateway]);

  const close = useCallback(() => setOpen(false), []);

  const reference = useLatestValueWriter<number>({ pending, setPending });

  const commit = useCallback((label: string, action: () => Promise<DeviceActionResult>) => {
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
  }, [connected, fail, notice, reference.runCommand, refresh]);

  const setInput = useCallback((inputPortId: TunerSettings["inputPortId"]) =>
    commit("Tuner input updated.", () => gateway.setTunerInput(inputPortId, true)), [commit, gateway]);
  const setMuted = useCallback((muted: boolean) =>
    commit(muted ? "Tuner audio muted." : "Tuner audio restored.", () => gateway.setTunerMute(muted, true)), [commit, gateway]);
  const setReferenceOffset = useCallback((referenceOffsetHz: number) => {
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
  }, [connected, fail, gateway, notice, reference.submit, refresh]);
  const setMeterEnabled = useCallback((enabled: boolean) =>
    commit(`Live Tuner ${enabled ? "enabled" : "disabled"}.`, () => gateway.setTunerMeter(enabled, true)), [commit, gateway]);

  const draftOffset = reference.drafts.get("reference");
  const view = useMemo(() => settings && draftOffset !== undefined
    ? { ...settings, referenceOffsetHz: draftOffset, referenceHz: settings.referenceHz - settings.referenceOffsetHz + draftOffset }
    : settings, [draftOffset, settings]);

  // The reference dial stays live while its own value is being written.
  return { open, openTuner, close, loading, available, pending: pending && !reference.active, settings: view, refresh, setInput, setMuted, setReferenceOffset, setMeterEnabled };
}

export type TunerWorkflow = ReturnType<typeof useTunerWorkflow>;
