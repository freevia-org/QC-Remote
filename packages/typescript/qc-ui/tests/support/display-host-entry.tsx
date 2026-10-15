// Bundled by esbuild (never loaded by node directly): two fake hosts that hand the shared display
// their own adapters, then render it to static markup. Everything a real host provides to the
// display is provided here; the two hosts differ only in the values a real host would differ in.
import { createElement, Fragment } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";
import {
  createQcDeviceDisplay, QcDeviceDisplay, QcDeviceDisplayOverlay, IDLE_PLACEHOLDER_SNAPSHOT,
  useDeviceSettingsWorkflow, useIoEqWorkflows, useLooperXWorkflow, useModesConfigurationWorkflow,
  usePresetMidiOutWorkflow, useTempoWorkflow, useTunerWorkflow
} from "../../src/index.ts";
import { useQcScreenDestination } from "../../src/use-qc-screen-destination.ts";

const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
const refusing = new Proxy({}, { get: () => async () => { throw new Error("no device in this test"); } }) as never;
const columns = [{ load: 0.4, core: 0 }, { load: 0.9, core: 1 }];

export type Flavour = "windows" | "android";
export interface Scenario { popup?: "grid-menu" | "scene-menu" | "mode-menu" | null; view: string | null; open?: "deviceSettings" | "tempo"; presentation?: "hardware" | "display"; cpuOpen?: boolean; information?: boolean }

/** What a host owns: its transports, its connection state and its own entry points. They differ per flavour on purpose. */
function adapter(flavour: Flavour) {
  const log: string[] = [];
  const windows = flavour === "windows";
  return {
    log,
    session: { demo: !windows, hardwareReady: windows, sessionReady: true },
    selectScene: (scene: number) => { log.push(`${flavour}:scene:${scene}`); },
    queueTempo: (bpm: number) => { log.push(`${flavour}:tempo:${bpm}`); },
    notice: (message: string) => { log.push(`${flavour}:notice:${message}`); },
    openPreset: () => { log.push(`${flavour}:openPreset`); },
    refresh: () => { log.push(`${flavour}:refresh`); },
    openRouting: () => { log.push(`${flavour}:openRouting`); }
  };
}

function useDisplay(flavour: Flavour, scenario: Scenario) {
  const host = adapter(flavour);
  const options = { gateway: refusing, connected: true, pending: false, setPending() {}, notice: host.notice, fail() {} };
  const deviceSettings = useDeviceSettingsWorkflow(options);
  const tempo = useTempoWorkflow(options);
  const tuner = useTunerWorkflow(options);
  const looper = useLooperXWorkflow(options);
  const modes = useModesConfigurationWorkflow(options);
  const ioEq = useIoEqWorkflows({ ...options, snapshot: IDLE_PLACEHOLDER_SNAPSHOT });
  const midiOut = usePresetMidiOutWorkflow({ gateway: refusing, snapshot: IDLE_PLACEHOLDER_SNAPSHOT, demo: false });
  // Use the production destination owner (including its generation lease),
  // not a partial fake missing the continuation admission boundary.
  const destination = useQcScreenDestination();
  if (destination.view !== scenario.view) destination.setView(scenario.view as never);
  if (destination.popup !== (scenario.popup ?? null)) destination.setPopup(scenario.popup ?? null);
  const display = createQcDeviceDisplay({
    snapshot: IDLE_PLACEHOLDER_SNAPSHOT,
    selectedBlockId: "",
    pending: false,
    destination,
    views: {
      deviceInformation: scenario.information ? { serial: "S-1", deviceName: "Rig", corOsVersion: "3.1" } : undefined,
      open: async () => {}, close: async () => {}, toggle: async () => {}, forwardGesture: async () => {}
    },
    cpu: {
      cpuLoad: 0.42, cpuColumns: columns, consume() {},
      monitor: (open, returnFocusTarget, onClose) => open ? { cpuLoad: 0.42, cpuColumns: columns, returnFocusTarget, onClose } : undefined
    },
    undo: { undoAvailable: true, redoAvailable: false, consume() {}, undo: async () => {}, redo: async () => {}, canUndo: true, undoLabel: "Bypass Amp" },
    workflows: {
      deviceSettings: scenario.open === "deviceSettings" ? { ...deviceSettings, open: true } : deviceSettings,
      tempo: scenario.open === "tempo" ? { ...tempo, open: true } : tempo,
      tuner, looper, modes, midiOut, ioEq,
      grid: { addPickerProps: undefined, openBlock: async () => {} },
      scene: { labelColorOpen: false, command: undefined, openLabelColor() {}, openCommandForScene() {} },
      preset: { openSave() {}, recall: async () => {}, saveProps: undefined, directoryProps: undefined, presetList: undefined },
      routing: { pickerProps: undefined }
    } as never,
    mirror: { captureScreen: async () => ({ pngBase64: "" }) as never },
    selectScene: host.selectScene,
    queueTempo: host.queueTempo,
    setParameterMenuInitiallyOpen() {},
    notice: host.notice,
    openPreset: host.openPreset,
    refresh: host.refresh,
    openRouting: host.openRouting
  });
  return display;
}

function Host({ flavour, scenario }: { flavour: Flavour; scenario: Scenario }) {
  const display = useDisplay(flavour, scenario);
  // A fragment, as the real hosts mount the display: a wrapper element between the host's container and the display would break the host's child selectors.
  return createElement(Fragment, null,
    createElement(QcDeviceDisplay, { display, formFactor, skin, presentation: scenario.presentation, onAction() {} }),
    createElement(QcDeviceDisplayOverlay, { display }));
}

/** The props the assembled display hands the surface. */
export async function surfaceKeys(flavour: Flavour): Promise<string[]> {
  let keys: string[] = [];
  function Probe() { keys = Object.keys((useDisplay(flavour, { view: null }) as unknown as { surface: object }).surface); return null; }
  await prerenderToNodeStream(createElement(Probe));
  return keys.sort();
}

export async function render(flavour: Flavour, scenario: Scenario): Promise<string> {
  const { prelude } = await prerenderToNodeStream(createElement(Host, { flavour, scenario }));
  const chunks: Buffer[] = [];
  for await (const chunk of prelude) chunks.push(Buffer.from(chunk));
  return Buffer.concat(chunks).toString("utf8");
}
