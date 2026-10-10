import { useCallback, useState } from "react";
import type { GlobalEqSettings, IoSettings, PresetSnapshot, TunerSettings } from "@qc-remote/client";
import { CorOsAddDevicePicker } from "./add-device-picker";
import { CorOsGlobalEqScreen, CorOsIoSettingsScreen } from "./io-global-eq-screens";
import { LooperXScreen } from "./looper-x-screen";
import { ModesConfigurationScreen } from "./modes-configuration-screen";
import { PresetMidiOutEditor } from "./preset-midi-out-editor";
import { CorOsTunerScreen } from "./tuner-screen";
import { CorOsCpuMonitor } from "./coros-cpu-monitor";
import type { IoEqWorkflow } from "./use-io-eq-workflows";
import type { TunerWorkflow } from "./use-tuner-workflow";
import type { PresetMidiOutWorkflow } from "./use-preset-midi-out-workflow";

const ignore = () => undefined;

/** Development-only production-component host used by the visual audit.
 * It renders the same components the apps use, with stable device-shaped data,
 * so comparisons never substitute the older visual-only corpus fixtures. */
export function CorOsProductionScreenAudit({ view, snapshot }: { view: string; snapshot: PresetSnapshot }) {
  const [query, setQuery] = useState("");
  const [midiOutStatus, setMidiOutStatus] = useState<PresetMidiOutWorkflow["status"]>(null);
  // The editor re-runs its reset effects when these change identity.
  const clearMidiOutStatus = useCallback(() => setMidiOutStatus(null), []);
  const tunerSettings: TunerSettings = { inputPortId: 1, referenceOffsetHz: 0, referenceHz: 440, muted: true, meterEnabled: true, meter: -1.4 };
  const tuner = {
    open: true, loading: false, available: true, pending: false, settings: tunerSettings,
    openTuner() {}, close() {}, async refresh() {}, async setInput() {}, async setMuted() {}, async setReferenceOffset() {}, async setMeterEnabled() {}
  } as unknown as TunerWorkflow;
  const io: IoSettings = {
    inputs: [{ inputPortId: 1, levelDb: 0, impedance: 0.875, inputType: 0, groundLift: 0, plugged: true }, { inputPortId: 2, levelDb: 0, impedance: 0.875, inputType: 0, groundLift: 0 }],
    outputs: [1,2,4,5,6,7,8].map((outputPortId) => ({ outputPortId, levelDb: 0, groundLift: 0, muted: false, plugged: outputPortId < 6 })),
    expressionPorts: [{ expressionPortId: 1 }, { expressionPortId: 2 }],
    headphones: { feeds: [], level: 0.72, plugged: true }, usb: { level: 0.5, headphonesSource: 1 / 3, dryWet: 0, plugged: true }, midi: { thru: 0 }, xlr12Linked: true, out34Linked: false
  };
  const referenceFrequency = Math.log10(50 / 20) / 3;
  const referenceQ = (0.71 - 0.1) / 9.9;
  const eq: GlobalEqSettings = { bypassed: true, parameters: Array.from({ length: 28 }, (_, parameterIndex) => ({
    parameterIndex,
    value: parameterIndex < 25
      ? parameterIndex % 5 === 1 ? referenceFrequency
        : parameterIndex % 5 === 2 ? referenceQ
          : parameterIndex % 5 === 3 ? 4
            : parameterIndex % 5 === 4 ? 1
              : 0.5
      : 0.5
  })) };
  // The audit never reads a device, so it shows the state a fresh session has: no preset list yet.
  const globalEq: IoEqWorkflow["globalEq"] = { status: "unavailable", reason: "not-observed" };
  const ioeq = {
    io, eq, globalEq, loading: false, pending: false, async refresh() {}, async setInputPort() {}, async setOutputPort() {}, async setUsbPort() {}, async setMidiThru() {}, async setOutputPairing() {}, async setEqBypassed() {}, async setEqBand() {}, async setEqOutput() {}, async loadEqPreset() {}
  } satisfies Partial<IoEqWorkflow> as IoEqWorkflow;
  const looper = {
    status: { state: 0, loopLength: 12_672_000, freeSamples: 12_672_000, oneShot: false, halfSpeed: false, redoAvailable: false },
    loading: false, pending: false, async refresh() {}, async run() {}
  };
  const modes = { slots: [0, 6], loading: false, pending: false, async refresh() {}, async commit() {} };
  // The audit never talks to a device. Its MIDI Out writes succeed locally so
  // Save and Clear all show their real outcome instead of a missing-method error.
  const midiOut: PresetMidiOutWorkflow = {
    snapshot, demo: false, pending: false, error: "", status: midiOutStatus,
    async save() { setMidiOutStatus("saved"); },
    async clearAll() { setMidiOutStatus("cleared"); },
    clearError: ignore,
    clearStatus: clearMidiOutStatus
  };
  const close = () => undefined;

  if (view === "tuner") return <CorOsTunerScreen workflow={tuner} onClose={close} />;
  if (view === "looper-editor") return <LooperXScreen workflow={looper} activeScene={snapshot.activeScene} onParameters={close} onSelectScene={close} onClose={close} />;
  if (view === "modes-configuration") return <ModesConfigurationScreen workflow={modes} onClose={close} />;
  if (view === "global-eq") return <CorOsGlobalEqScreen workflow={ioeq} onBack={close} />;
  if (view.startsWith("io-")) return <CorOsIoSettingsScreen workflow={ioeq} initialView={view.slice(3) as "overview" | "input" | "output" | "send-return" | "usb" | "headphones"} onGlobalEq={close} onClose={close} />;
  if (view === "preset-midi-out") return <PresetMidiOutEditor workflow={midiOut} onClose={close} />;
  if (view === "cpu-monitor") return <CorOsCpuMonitor snapshot={{
    ...snapshot,
    presetLocation: "4E",
    presetPosition: 28,
    presetName: "QC MCP TEST_2",
    blocks: [{ id: "audit-cpu-block", name: "Adaptive Gate", kind: "utility", category: "Utility", row: 0, column: 0, bypassed: true }],
    routes: [
      { row: 0, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false },
      { row: 1, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false },
      { row: 2, inputId: 1, outputId: 19, input: "In 1", output: "Multi Out", splitMuted: false },
      { row: 3, inputId: 0, outputId: 0, input: "", output: "", splitMuted: false }
    ]
  }} cpuLoad={0.07} cpuColumns={[{ load: 0.07, core: 1 }]} onClose={close} />;
  if (view === "neural-capture-picker") return <CorOsAddDevicePicker cellCenter={{ x: 357, y: 241 }} picker={{
    row: 1, column: 3, models: [], captures: Array.from({ length: 2_127 }, (_, index) => ({ key: `audit-capture-${index + 1}`, name: index < 7 ? `4-Comp Custom ${index + 1}` : `ZZZ Factory Capture ${index + 1}`, author: "NeuralDSP", version: "1", instrument: 4, isFactory: true, isPlugin: false, folderKey: "local_nc_root", folderName: "Captures Library" })),
    captureFolders: [
      { key: "local_nc_root", name: "Captures Library", isFactory: false, isDownloads: false, isLibrary: true },
      { key: "factory-v1", name: "Factory Captures V1", parentKey: "local_nc_root", isFactory: true, isDownloads: false, isLibrary: false },
      { key: "factory-v2", name: "Factory Captures V2", parentKey: "local_nc_root", isFactory: true, isDownloads: false, isLibrary: false },
      { key: "my-captures", name: "My Captures", parentKey: "local_nc_root", isFactory: false, isDownloads: false, isLibrary: false }
    ], loading: false, pending: false, query, onQueryChange: setQuery, activeCategory: "Neural Capture", onCategoryChange() {}, pinnedModelIds: [], onSelect() {}, onTogglePinned() {}, onSelectCapture() {}, onClose: close
  }} />;
  return null;
}
