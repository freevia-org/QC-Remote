import { useCallback, useMemo, useState } from "react";
import type { DeviceActionResult, GlobalEqSettings, IoSettings, ModelPresetSummary, PresetSnapshot } from "@qc-remote/client";
import { admitGlobalEqPreset, globalEqPresetsFor, type GlobalEqPresets } from "@qc-remote/core";
import { useLatestValueWriter } from "./latest-value-writer";
import { connectBeforeNotice, DEVICE_COMMAND_BUSY_NOTICE } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";

export type IoEqWorkflowOptions = DeviceGatewayWorkflowOptions & {
  /** The live snapshot. Its retained model preset projection and state session decide which Global EQ presets are offered.
   * Without one no list is offered and no preset is loaded. */
  snapshot?: Pick<PresetSnapshot, "modelPresetCatalog" | "stateVersion">;
};

const IO_EQ_ACTION = "changing I/O or Global EQ settings";

/** Device-wide I/O and Global EQ state. Every accepted write is followed by
 * an authoritative read. Dials write through the shared latest-value writer:
 * a drag's steps coalesce to one write in flight, the last step is always
 * written, and the dragged value is shown only until that read lands. */
export function useIoEqWorkflows(options: IoEqWorkflowOptions) {
  const { gateway, connected, pending, setPending, notice, fail, snapshot } = options;
  const [io, setIo] = useState<IoSettings>();
  const [eq, setEq] = useState<GlobalEqSettings>();
  const [loading, setLoading] = useState(false);

  const refreshIo = useCallback(async () => setIo(await gateway.ioSettings()), [gateway]);
  const refreshEq = useCallback(async () => setEq(await gateway.globalEq()), [gateway]);
  const refresh = useCallback(async () => {
    setLoading(true);
    try { await Promise.all([refreshIo(), refreshEq()]); }
    catch (error) { fail(error); }
    finally { setLoading(false); }
  }, [fail, refreshEq, refreshIo]);

  const dials = useLatestValueWriter<number>({ pending, setPending });

  const mutate = useCallback((write: () => Promise<{ detail: string }>, read: () => Promise<unknown>) => {
    // The shared guarded-mutation policy: a refused write says why, never
    // nothing at all. Admission itself belongs to the dial writer, so a switch
    // pressed mid-drag queues behind the drag's last value.
    if (!connected) { notice(connectBeforeNotice(IO_EQ_ACTION)); return; }
    const accepted = dials.runCommand(async () => {
      try {
        const result = await write();
        await read();
        notice(result.detail);
      } catch (error) { fail(error); }
    }, fail);
    if (!accepted) notice(DEVICE_COMMAND_BUSY_NOTICE);
  }, [connected, dials.runCommand, fail, notice]);

  const ioWrite = (write: () => Promise<{ detail: string }>) => mutate(write, refreshIo);
  const eqWrite = (write: () => Promise<{ detail: string }>) => mutate(write, refreshEq);

  /** One dial step. `key` names the field; the newest value per field wins. */
  const dial = (key: string, value: number, write: () => Promise<DeviceActionResult>, read: () => Promise<unknown>) => {
    if (!connected) { notice(connectBeforeNotice(IO_EQ_ACTION)); return; }
    const accepted = dials.submit(key, value, {
      write: () => write(),
      settle: async (last) => {
        await read();
        if (last?.detail) notice(last.detail);
      },
      error: fail
    });
    if (!accepted) notice(DEVICE_COMMAND_BUSY_NOTICE);
  };
  const ioDial = (key: string, value: number, write: () => Promise<DeviceActionResult>) => dial(key, value, write, refreshIo);
  const eqDial = (key: string, value: number, write: () => Promise<DeviceActionResult>) => dial(key, value, write, refreshEq);

  const ioView = useMemo(() => withIoDrafts(io, dials.drafts), [dials.drafts, io]);
  const eqView = useMemo(() => withEqDrafts(eq, dials.drafts), [dials.drafts, eq]);
  // The host's fold, read from the snapshot: a late subscriber and a gap recovery both get it there.
  // Unavailable is not "none": the picker says which. A list kept from before a disconnect is not offered.
  const catalog = snapshot?.modelPresetCatalog;
  const catalogSession = snapshot?.stateVersion?.session;
  const globalEq = useMemo(
    (): GlobalEqPresets => connected ? globalEqPresetsFor(catalog, catalogSession) : { status: "unavailable", reason: "not-observed" },
    [connected, catalog, catalogSession]
  );

  return {
    io: ioView, eq: eqView, globalEq, loading,
    // A dial stays live while its own value is being written.
    pending: pending && !dials.active,
    refresh,
    setInputPort: (id: number, levelDb: number | null, impedance: number | null, inputType: number | null, groundLift: number | null) => {
      const write = () => gateway.setInputPort(id, levelDb, impedance, inputType, groundLift);
      if (inputType === null && groundLift === null) {
        if (levelDb !== null && impedance === null) return ioDial(`input:${id}:levelDb`, levelDb, write);
        if (impedance !== null && levelDb === null) return ioDial(`input:${id}:impedance`, impedance, write);
      }
      return ioWrite(write);
    },
    setOutputPort: (id: number, level: number | null, groundLift: number | null, mute: boolean | null) => {
      const write = () => gateway.setOutputPort(id, level, groundLift, mute);
      if (level !== null && groundLift === null && mute === null) return ioDial(`output:${id}:levelDb`, level, write);
      return ioWrite(write);
    },
    setUsbPort: (level: number | null, headphonesSource: number | null, dryWet: number | null) => {
      const write = () => gateway.setUsbPort(level, headphonesSource, dryWet);
      if (dryWet === null) {
        if (level !== null && headphonesSource === null) return ioDial("usb:level", level, write);
        if (headphonesSource !== null && level === null) return ioDial("usb:headphonesSource", headphonesSource, write);
      }
      return ioWrite(write);
    },
    setMidiThru: (enabled: boolean) => ioWrite(() => gateway.setMidiThru(enabled)),
    setOutputPairing: (xlr12Linked: boolean | null, out34Linked: boolean | null) => ioWrite(() => gateway.setOutputPairing(xlr12Linked, out34Linked)),
    setEqBypassed: (bypassed: boolean) => eqWrite(() => gateway.setGlobalEqBypassed(bypassed)),
    setEqBand: (band: number, gain: number | null, frequency: number | null, q: number | null, filterType: number | null, enabled: boolean | null) => {
      const write = () => gateway.setGlobalEqBand(band, gain, frequency, q, filterType, enabled);
      const continuous = [gain, frequency, q];
      const offset = continuous.findIndex((value) => value !== null);
      if (filterType === null && enabled === null && offset >= 0 && continuous.filter((value) => value !== null).length === 1) {
        return eqDial(`eq:${(band - 1) * 5 + offset}`, continuous[offset] as number, write);
      }
      return eqWrite(write);
    },
    setEqOutput: (level: number | null, out12: boolean | null, out34: boolean | null) => {
      const write = () => gateway.setGlobalEqOutput(level, out12, out34);
      // The output level is written in dB and read back normalised over -20..+20 dB.
      if (level !== null && out12 === null && out34 === null) return eqDial(`eq:${GLOBAL_EQ_OUTPUT_LEVEL_INDEX}`, (level + 20) / 40, write);
      return eqWrite(write);
    },
    loadEqPreset: (preset: ModelPresetSummary) => {
      const hash = preset.modelHash;
      if (!preset.id.trim() || hash === undefined) {
        fail(new Error("The Quad Cortex did not provide a complete identity for this Global EQ preset."));
        return;
      }
      // A tuple the host's fold does not list now (deleted, another session, another family) never
      // reaches the gateway. The host repeats this check and is the authority.
      const refusal = admitGlobalEqPreset(snapshot, preset);
      if (refusal) {
        fail(new Error(refusal));
        return;
      }
      return eqWrite(() => gateway.loadGlobalEqModelPreset(preset.id, preset.isFactory, hash));
    }
  };
}

export type IoEqWorkflow = ReturnType<typeof useIoEqWorkflows>;

const GLOBAL_EQ_OUTPUT_LEVEL_INDEX = 25;

/** The readback with every unsettled dial value laid over it. */
function withIoDrafts(settings: IoSettings | undefined, drafts: ReadonlyMap<string, number>): IoSettings | undefined {
  if (!settings || drafts.size === 0) return settings;
  let touched = false;
  const draft = (key: string) => {
    const value = drafts.get(key);
    if (value !== undefined) touched = true;
    return value;
  };
  const inputs = settings.inputs.map((port) => {
    const levelDb = draft(`input:${port.inputPortId}:levelDb`);
    const impedance = draft(`input:${port.inputPortId}:impedance`);
    return levelDb === undefined && impedance === undefined ? port : { ...port, ...(levelDb === undefined ? {} : { levelDb }), ...(impedance === undefined ? {} : { impedance }) };
  });
  const outputs = settings.outputs.map((port) => {
    const levelDb = draft(`output:${port.outputPortId}:levelDb`);
    return levelDb === undefined ? port : { ...port, levelDb };
  });
  const usbLevel = draft("usb:level");
  const headphonesSource = draft("usb:headphonesSource");
  const usb = settings.usb && (usbLevel !== undefined || headphonesSource !== undefined)
    ? { ...settings.usb, ...(usbLevel === undefined ? {} : { level: usbLevel }), ...(headphonesSource === undefined ? {} : { headphonesSource }) }
    : settings.usb;
  return touched ? { ...settings, inputs, outputs, usb } : settings;
}

function withEqDrafts(settings: GlobalEqSettings | undefined, drafts: ReadonlyMap<string, number>): GlobalEqSettings | undefined {
  if (!settings || drafts.size === 0) return settings;
  let touched = false;
  const parameters = settings.parameters.map((parameter) => {
    const value = drafts.get(`eq:${parameter.parameterIndex}`);
    if (value === undefined) return parameter;
    touched = true;
    return { ...parameter, value };
  });
  return touched ? { ...settings, parameters } : settings;
}
