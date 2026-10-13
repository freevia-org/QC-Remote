import type { ReactNode } from "react";
import type { GatewayTransport, GridBlock, PresetSnapshot } from "@qc-remote/client";
import { CorOsIoSettingsScreen, CorOsGlobalEqScreen } from "./io-global-eq-screens";
import { CorOsDeviceSettings } from "./device-settings-screen";
import { CorOsTempoScreen } from "./tempo-screen";
import { CorOsTunerScreen } from "./tuner-screen";
import { LooperXScreen } from "./looper-x-screen";
import { ModesConfigurationScreen, type ModesConfigurationWorkflow } from "./modes-configuration-screen";
import { PresetMidiOutEditor } from "./preset-midi-out-editor";
import { qcLooperScreenActions, resolveQcDeviceScreen } from "./qc-device-screen";
import { QcUnitScreenMirror } from "./qc-unit-screen-mirror";
import type { DeviceSettingsWorkflow } from "./use-device-settings-workflow";
import type { IoEqWorkflow } from "./use-io-eq-workflows";
import type { LooperXWorkflow } from "./use-looper-x-workflow";
import type { PresetMidiOutWorkflow } from "./use-preset-midi-out-workflow";
import type { QcScreenDestination } from "./use-qc-screen-destination";
import type { TempoWorkflow } from "./use-tempo-workflow";
import type { TunerWorkflow } from "./use-tuner-workflow";

export interface QcDeviceScreenContentOptions {
  screenGestureContextKey?: string;
  captureSessionCurrent?(): () => boolean;
  snapshot: PresetSnapshot;
  destination: Pick<QcScreenDestination, "view" | "returnFocusTarget" | "setView">;
  workflows: {
    deviceSettings: DeviceSettingsWorkflow;
    tempo: TempoWorkflow;
    tuner: TunerWorkflow;
    looper: LooperXWorkflow;
    modes: ModesConfigurationWorkflow;
    midiOut: PresetMidiOutWorkflow;
    ioEq: IoEqWorkflow;
    grid: { openBlock(block: GridBlock): Promise<unknown> };
  };
  /** The unit, for screens the app mirrors as a picture. */
  mirror: Pick<GatewayTransport, "captureScreen">;
  selectScene(scene: number): unknown;
  queueTempo(bpm: number, source?: "Encoder" | "Tap"): void;
  setParameterMenuInitiallyOpen(open: boolean): void;
  /** Closes the screen on the unit as well as in the app (Tuner, I/O). */
  closeScreen(): void | Promise<void>;
  notice(message: string): void;
}

/**
 * Maps the current device screen to the CorOS screen component that draws it.
 * Returns undefined when the Grid should show, so the result can be passed
 * straight to the surface's screenContent.
 */
export function qcDeviceScreenContent(options: QcDeviceScreenContentOptions): ReactNode | undefined {
  const { snapshot, destination, workflows, mirror, selectScene, queueTempo, setParameterMenuInitiallyOpen, closeScreen, notice } = options;
  const { setView, returnFocusTarget } = destination;
  const kind = resolveQcDeviceScreen(destination.view, { deviceSettings: workflows.deviceSettings.open, tempo: workflows.tempo.open });
  switch (kind) {
    case "device-settings": return <CorOsDeviceSettings workflow={workflows.deviceSettings} onClose={workflows.deviceSettings.close} />;
    case "tempo": return <CorOsTempoScreen workflow={workflows.tempo} screenGestureContextKey={options.screenGestureContextKey} captureSessionCurrent={options.captureSessionCurrent} snapshot={snapshot} queueTempo={queueTempo} onSelectScene={(index) => void selectScene(index)} onClose={workflows.tempo.close} />;
    case "tuner": return <CorOsTunerScreen workflow={workflows.tuner} onClose={() => void closeScreen()} />;
    case "looper-editor": {
      const looper = qcLooperScreenActions({ captureSessionCurrent: options.captureSessionCurrent, snapshot, looper: workflows.looper, grid: workflows.grid, setView, setParameterMenuInitiallyOpen, notice });
      return <LooperXScreen workflow={workflows.looper} activeScene={snapshot.activeScene} onSelectScene={(scene) => void selectScene(scene)} onParameters={looper.onParameters} onMenu={looper.onMenu} onClose={() => void looper.onClose()} />;
    }
    case "modes": return <ModesConfigurationScreen returnFocusTarget={returnFocusTarget} workflow={workflows.modes} onClose={() => setView(null)} />;
    case "unit-mirror": return <QcUnitScreenMirror key={options.screenGestureContextKey} gateway={mirror} captureSessionCurrent={options.captureSessionCurrent} />;
    case "midi-out": return <PresetMidiOutEditor returnFocusTarget={returnFocusTarget} workflow={workflows.midiOut} onClose={() => setView(null)} />;
    case "global-eq": return <CorOsGlobalEqScreen workflow={workflows.ioEq} onBack={() => setView("io-overview")} />;
    case "io": return <CorOsIoSettingsScreen workflow={workflows.ioEq} onGlobalEq={() => setView("global-eq")} onClose={() => void closeScreen()} />;
    default: return undefined;
  }
}
